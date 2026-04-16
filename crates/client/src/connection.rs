//! サーバーへの IPC 接続
//!
//! Windows 名前付きパイプ経由でサーバーに接続する。
//! パイプ名は ipc.rs の PIPE_PREFIX と同じ規則: \\.\pipe\yatamux-{session}

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;
use yatamux_protocol::{ClientMessage, ServerMessage, PROTOCOL_VERSION, SERVER_CAPABILITIES};

pub struct ServerConnection {
    pub tx: mpsc::Sender<ClientMessage>,
    pub rx: mpsc::Receiver<ServerMessage>,
    /// 接続先サーバー（yatamux GUI）のプロセス ID
    pub server_pid: u32,
}

impl ServerConnection {
    /// 既存のサーバーに接続する
    pub async fn connect(session: &str) -> Result<Self> {
        #[cfg(windows)]
        {
            use std::os::windows::io::AsRawHandle;
            use tokio::net::windows::named_pipe::ClientOptions;
            use tokio::time::{sleep, Duration};
            use windows::Win32::Foundation::HANDLE;
            use windows::Win32::System::Pipes::GetNamedPipeServerProcessId;

            let pipe_name = format!(r"\\.\pipe\yatamux-{}", session);
            let pipe = {
                const PIPE_BUSY_ERROR: i32 = 231;
                const CONNECT_RETRIES: usize = 20;
                const RETRY_DELAY_MS: u64 = 25;

                let mut last_busy_err = None;
                let mut pipe = None;
                for attempt in 0..CONNECT_RETRIES {
                    match ClientOptions::new().open(&pipe_name) {
                        Ok(opened) => {
                            pipe = Some(opened);
                            break;
                        }
                        Err(err) if err.raw_os_error() == Some(PIPE_BUSY_ERROR) => {
                            last_busy_err = Some(err);
                            if attempt + 1 < CONNECT_RETRIES {
                                sleep(Duration::from_millis(RETRY_DELAY_MS)).await;
                                continue;
                            }
                        }
                        Err(err) => {
                            return Err(err).with_context(|| {
                                format!("Failed to connect to named pipe: {}", pipe_name)
                            });
                        }
                    }
                }

                pipe.ok_or_else(|| {
                    last_busy_err
                        .map(anyhow::Error::from)
                        .unwrap_or_else(|| anyhow::anyhow!("named pipe remained busy"))
                })
                .with_context(|| format!("Failed to connect to named pipe: {}", pipe_name))?
            };

            // GUI プロセスの PID を取得（バイナリ置換の待機に使う）
            let server_pid = {
                let handle = HANDLE(pipe.as_raw_handle() as _);
                let mut pid = 0u32;
                unsafe { GetNamedPipeServerProcessId(handle, &mut pid) }
                    .context("GetNamedPipeServerProcessId に失敗")?;
                pid
            };

            let (reader, mut writer) = tokio::io::split(pipe);
            let mut lines = BufReader::new(reader).lines();

            // 接続直後にプロトコルハンドシェイクを送信する
            //
            // トークンファイル（%APPDATA%\yatamux\{session}.token）が存在すれば読み込み、
            // Handshake に含める。ファイルが存在しない場合はトークンなしで送信する。
            let token = {
                let token_path = std::env::var("APPDATA").ok().map(|appdata| {
                    std::path::Path::new(&appdata)
                        .join("yatamux")
                        .join(format!("{}.token", session))
                });
                token_path.and_then(|p| std::fs::read_to_string(p).ok())
            };
            let handshake = ClientMessage::Handshake {
                protocol_version: PROTOCOL_VERSION,
                capabilities: SERVER_CAPABILITIES.iter().map(|s| s.to_string()).collect(),
                token,
            };
            let handshake_json = serde_json::to_string(&handshake)
                .context("failed to serialize Handshake message")?;
            writer
                .write_all(format!("{}\n", handshake_json).as_bytes())
                .await
                .context("failed to send Handshake to server")?;

            // サーバーからの最初の応答を確認する（HandshakeAccepted or Error）。
            // 旧サーバー（handshake 未対応）は最初に別メッセージを返すことがあるため、
            // タイムアウト付きで待機し、応答なし・旧形式は後方互換として通過させる。
            use tokio::time::timeout;
            let first_response = timeout(Duration::from_secs(3), lines.next_line()).await;
            let first_line = match first_response {
                Ok(Ok(Some(line))) => Some(line),
                Ok(Ok(None)) => anyhow::bail!("server closed connection before handshake response"),
                Ok(Err(e)) => anyhow::bail!("pipe read error during handshake: {}", e),
                Err(_) => None, // タイムアウト: 旧サーバーと判断して続行
            };

            if let Some(line) = first_line {
                if let Ok(msg) = serde_json::from_str::<ServerMessage>(&line) {
                    match msg {
                        ServerMessage::HandshakeAccepted { .. } => {
                            // 正常: 続行
                        }
                        ServerMessage::Error { message, .. } => {
                            anyhow::bail!("server rejected handshake: {}", message);
                        }
                        other => {
                            // 旧サーバー or 想定外メッセージ: チャネルに転送して続行
                            // （後方互換のため切断しない）
                            let (client_tx, mut client_rx) = mpsc::channel::<ClientMessage>(64);
                            let (server_tx, server_rx) = mpsc::channel::<ServerMessage>(64);
                            // 先読みしたメッセージをチャネルに送り込む
                            let _ = server_tx.try_send(other);
                            tokio::spawn(async move {
                                while let Some(msg) = client_rx.recv().await {
                                    if let Ok(json) = serde_json::to_string(&msg) {
                                        if writer.write_all(format!("{}\n", json).as_bytes()).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            });
                            tokio::spawn(async move {
                                while let Ok(Some(line)) = lines.next_line().await {
                                    if let Ok(msg) = serde_json::from_str::<ServerMessage>(&line) {
                                        if matches!(msg, ServerMessage::HandshakeAccepted { .. }) {
                                            continue;
                                        }
                                        if server_tx.send(msg).await.is_err() {
                                            break;
                                        }
                                    }
                                }
                            });
                            return Ok(Self {
                                tx: client_tx,
                                rx: server_rx,
                                server_pid,
                            });
                        }
                    }
                }
            }

            let (client_tx, mut client_rx) = mpsc::channel::<ClientMessage>(64);
            let (server_tx, server_rx) = mpsc::channel::<ServerMessage>(64);

            // クライアントメッセージ → パイプ書き込みタスク
            tokio::spawn(async move {
                while let Some(msg) = client_rx.recv().await {
                    if let Ok(json) = serde_json::to_string(&msg) {
                        let line = format!("{}\n", json);
                        if writer.write_all(line.as_bytes()).await.is_err() {
                            break;
                        }
                    }
                }
            });

            // パイプ読み取り → サーバーメッセージチャネルタスク
            // HandshakeAccepted はプロトコル層で処理済みのため上位に転送しない。
            // SubscribeAccepted / UnsubscribeAccepted / ControlAccepted は IPC 確認応答として上位に転送する。
            tokio::spawn(async move {
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(msg) = serde_json::from_str::<ServerMessage>(&line) {
                        if matches!(msg, ServerMessage::HandshakeAccepted { .. }) {
                            continue;
                        }
                        if server_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                }
            });

            Ok(Self {
                tx: client_tx,
                rx: server_rx,
                server_pid,
            })
        }

        #[cfg(not(windows))]
        {
            anyhow::bail!("Named pipe connection is only available on Windows")
        }
    }
}
