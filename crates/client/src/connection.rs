//! サーバーへの IPC 接続
//!
//! Windows 名前付きパイプ経由でサーバーに接続する。
//! パイプ名は ipc.rs の PIPE_PREFIX と同じ規則: \\.\pipe\yatamux-{session}

use anyhow::{Context, Result};
use yatamux_protocol::{ClientMessage, ServerMessage};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::mpsc;

pub struct ServerConnection {
    pub tx: mpsc::Sender<ClientMessage>,
    pub rx: mpsc::Receiver<ServerMessage>,
}

impl ServerConnection {
    /// 既存のサーバーに接続する
    pub async fn connect(session: &str) -> Result<Self> {
        #[cfg(windows)]
        {
            use tokio::net::windows::named_pipe::ClientOptions;

            let pipe_name = format!(r"\\.\pipe\yatamux-{}", session);
            let pipe = ClientOptions::new()
                .open(&pipe_name)
                .with_context(|| format!("Failed to connect to named pipe: {}", pipe_name))?;

            let (reader, mut writer) = tokio::io::split(pipe);
            let mut lines = BufReader::new(reader).lines();

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
            tokio::spawn(async move {
                while let Ok(Some(line)) = lines.next_line().await {
                    if let Ok(msg) = serde_json::from_str::<ServerMessage>(&line) {
                        if server_tx.send(msg).await.is_err() {
                            break;
                        }
                    }
                }
            });

            Ok(Self {
                tx: client_tx,
                rx: server_rx,
            })
        }

        #[cfg(not(windows))]
        {
            anyhow::bail!("Named pipe connection is only available on Windows")
        }
    }
}
