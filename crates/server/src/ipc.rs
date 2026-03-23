//! Windows 名前付きパイプ IPC サーバー
//!
//! 要件定義書 §5「IPC: Windows 名前付きパイプ」に対応。
//! パイプ名: \\.\pipe\yatamux-{session_name}

use anyhow::{Context, Result};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use yatamux_protocol::{ClientMessage, ServerMessage};

/// 名前付きパイプのベース名
pub const PIPE_PREFIX: &str = r"\\.\pipe\yatamux-";

/// IPC サーバーを起動し、クライアント接続を受け付ける
///
/// - `server_tx`: クライアントからのメッセージを Server へ転送
/// - `server_rx`: Server からの応答を受け取り、全クライアントにブロードキャスト
pub async fn run_ipc_server(
    session_name: &str,
    server_tx: mpsc::Sender<ClientMessage>,
    mut server_rx: mpsc::Receiver<ServerMessage>,
) -> Result<()> {
    let pipe_name = format!("{}{}", PIPE_PREFIX, session_name);
    info!("IPC server listening on {}", pipe_name);

    #[cfg(windows)]
    {
        use tokio::net::windows::named_pipe::ServerOptions;

        // サーバー出力を全クライアントにブロードキャストするチャネル
        let (bcast_tx, _) = broadcast::channel::<ServerMessage>(256);
        let bcast_fwd = bcast_tx.clone();

        // server_rx → broadcast タスク
        tokio::spawn(async move {
            while let Some(msg) = server_rx.recv().await {
                // 受信者がいなくてもエラーは無視
                let _ = bcast_fwd.send(msg);
            }
        });

        // 最初のパイプインスタンス（排他作成）
        let mut server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .context("Failed to create named pipe (first instance)")?;

        loop {
            info!("Waiting for client connection...");
            server.connect().await.context("Named pipe connect failed")?;
            info!("Client connected");

            // 次の接続用インスタンスを先に準備
            let next = ServerOptions::new()
                .first_pipe_instance(false)
                .create(&pipe_name)
                .context("Failed to create named pipe (next instance)")?;

            let connected = server;
            server = next;

            let tx_clone = server_tx.clone();
            let bcast_rx = bcast_tx.subscribe();
            tokio::spawn(handle_client(connected, tx_clone, bcast_rx));
        }
    }

    #[cfg(not(windows))]
    {
        warn!("Named pipe IPC is only available on Windows. Using stdin for development.");
        let stdin = tokio::io::stdin();
        let mut lines = BufReader::new(stdin).lines();
        while let Some(line) = lines.next_line().await? {
            if let Ok(msg) = serde_json::from_str::<ClientMessage>(&line) {
                server_tx.send(msg).await?;
            }
        }
        drop(server_rx);
        Ok(())
    }
}

/// 1 クライアント接続のハンドラ
///
/// - パイプ読み取り: クライアントメッセージ → server_tx
/// - パイプ書き込み: bcast_rx からの ServerMessage → クライアント
#[cfg(windows)]
async fn handle_client(
    pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    server_tx: mpsc::Sender<ClientMessage>,
    mut bcast_rx: broadcast::Receiver<ServerMessage>,
) {
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut lines = BufReader::new(reader).lines();

    loop {
        tokio::select! {
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        match serde_json::from_str::<ClientMessage>(&line) {
                            Ok(msg) => {
                                debug!("IPC recv: {:?}", msg);
                                if server_tx.send(msg).await.is_err() {
                                    break;
                                }
                            }
                            Err(e) => warn!("Failed to parse client message: {}", e),
                        }
                    }
                    _ => break,
                }
            }
            result = bcast_rx.recv() => {
                match result {
                    Ok(msg) => {
                        if let Ok(json) = serde_json::to_string(&msg) {
                            let line = format!("{}\n", json);
                            if writer.write_all(line.as_bytes()).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("IPC broadcast lagged by {} messages", n);
                    }
                    Err(_) => break,
                }
            }
        }
    }

    info!("Client disconnected");
}
