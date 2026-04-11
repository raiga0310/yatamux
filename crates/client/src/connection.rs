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
            // 旧サーバーは未知メッセージとして warn して継続（後方互換）
            let handshake = ClientMessage::Handshake {
                protocol_version: PROTOCOL_VERSION,
                capabilities: SERVER_CAPABILITIES.iter().map(|s| s.to_string()).collect(),
            };
            if let Ok(json) = serde_json::to_string(&handshake) {
                let line = format!("{}\n", json);
                // 送信失敗は致命的ではないので無視する（旧サーバーとの互換維持）
                let _ = writer.write_all(line.as_bytes()).await;
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
            // HandshakeAccepted はプロトコル層で処理済みのため上位に転送しない
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
