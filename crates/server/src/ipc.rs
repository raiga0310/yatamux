//! Windows 名前付きパイプ IPC サーバー
//!
//! 要件定義書 §5「IPC: Windows 名前付きパイプ」に対応。
//! パイプ名: \\.\pipe\yatamux-{session_name}

use anyhow::{Context, Result};
use std::collections::HashSet;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use yatamux_protocol::types::PaneId;
use yatamux_protocol::{
    ClientMessage, ServerMessage, MIN_CLIENT_VERSION, PROTOCOL_VERSION, SERVER_CAPABILITIES,
};

/// 名前付きパイプのベース名
pub const PIPE_PREFIX: &str = r"\\.\pipe\yatamux-";

#[cfg(windows)]
fn create_named_pipe_server(
    pipe_name: &str,
    first_pipe_instance: bool,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;
    use windows::Win32::Security::{
        InitializeSecurityDescriptor, SetSecurityDescriptorDacl, PSECURITY_DESCRIPTOR,
        SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR,
    };

    let mut security_descriptor = SECURITY_DESCRIPTOR::default();
    let descriptor_ptr = PSECURITY_DESCRIPTOR(&mut security_descriptor as *mut _ as *mut _);
    unsafe {
        InitializeSecurityDescriptor(descriptor_ptr, 1)
            .context("Failed to initialize named pipe security descriptor")?;
        // Allow local clients to open the pipe for duplex I/O. Remote clients are
        // still rejected by the default ServerOptions policy.
        SetSecurityDescriptorDacl(descriptor_ptr, true, None, false)
            .context("Failed to configure named pipe security descriptor")?;
    }

    let mut attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: &mut security_descriptor as *mut _ as *mut _,
        bInheritHandle: false.into(),
    };

    let mut options = ServerOptions::new();
    options.first_pipe_instance(first_pipe_instance);

    unsafe { options.create_with_security_attributes_raw(pipe_name, &mut attrs as *mut _ as _) }
        .with_context(|| format!("Failed to create named pipe: {}", pipe_name))
}

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
        let mut server = create_named_pipe_server(&pipe_name, true)
            .context("Failed to create named pipe (first instance)")?;

        loop {
            info!("Waiting for client connection...");
            server
                .connect()
                .await
                .context("Named pipe connect failed")?;
            info!("Client connected");

            // 次の接続用インスタンスを先に準備
            let next = create_named_pipe_server(&pipe_name, false)
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
fn should_forward_message(msg: &ServerMessage, subscriptions: &HashSet<PaneId>) -> bool {
    if subscriptions.is_empty() {
        return true;
    }

    match msg {
        ServerMessage::ExecResult { .. } => true,
        ServerMessage::Output { pane, .. }
        | ServerMessage::TitleChanged { pane, .. }
        | ServerMessage::Notification { pane, .. }
        | ServerMessage::ClipboardWrite { pane, .. }
        | ServerMessage::PaneClosed { pane }
        | ServerMessage::CommandFinished { pane, .. }
        | ServerMessage::PaneContent { pane, .. }
        | ServerMessage::PaneMetaUpdated { pane, .. } => subscriptions.contains(pane),
        _ => false,
    }
}

#[cfg(windows)]
async fn write_server_message(
    writer: &mut tokio::io::WriteHalf<tokio::net::windows::named_pipe::NamedPipeServer>,
    msg: &ServerMessage,
) -> std::io::Result<()> {
    let json = serde_json::to_string(msg).map_err(|err| {
        std::io::Error::other(format!("failed to serialize server message: {}", err))
    })?;
    let line = format!("{}\n", json);
    writer.write_all(line.as_bytes()).await
}

#[cfg(windows)]
async fn handle_client(
    pipe: tokio::net::windows::named_pipe::NamedPipeServer,
    server_tx: mpsc::Sender<ClientMessage>,
    mut bcast_rx: broadcast::Receiver<ServerMessage>,
) {
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut lines = BufReader::new(reader).lines();
    let mut subscriptions = HashSet::<PaneId>::new();

    loop {
        tokio::select! {
            result = lines.next_line() => {
                match result {
                    Ok(Some(line)) => {
                        match serde_json::from_str::<ClientMessage>(&line) {
                            Ok(msg) => {
                                debug!("IPC recv: {:?}", msg);
                                match msg {
                                    ClientMessage::Handshake {
                                        protocol_version,
                                        capabilities,
                                    } => {
                                        if protocol_version < MIN_CLIENT_VERSION {
                                            warn!(
                                                "IPC: client protocol version {} is below minimum {}; disconnecting",
                                                protocol_version, MIN_CLIENT_VERSION,
                                            );
                                            let err_msg = ServerMessage::Error {
                                                message: format!(
                                                    "client protocol version {} is not supported (minimum: {}); please upgrade yatamux",
                                                    protocol_version, MIN_CLIENT_VERSION,
                                                ),
                                            };
                                            let _ =
                                                write_server_message(&mut writer, &err_msg).await;
                                            break;
                                        }
                                        info!(
                                            "IPC: handshake from client v{}, capabilities: {:?}",
                                            protocol_version, capabilities
                                        );
                                        let accepted = ServerMessage::HandshakeAccepted {
                                            protocol_version: PROTOCOL_VERSION,
                                            min_client_version: MIN_CLIENT_VERSION,
                                            capabilities: SERVER_CAPABILITIES
                                                .iter()
                                                .map(|s| s.to_string())
                                                .collect(),
                                        };
                                        if write_server_message(&mut writer, &accepted)
                                            .await
                                            .is_err()
                                        {
                                            break;
                                        }
                                    }
                                    ClientMessage::SubscribePane { pane } => {
                                        subscriptions.insert(pane);
                                    }
                                    ClientMessage::UnsubscribePane { pane } => {
                                        subscriptions.remove(&pane);
                                    }
                                    other => {
                                        if server_tx.send(other).await.is_err() {
                                            break;
                                        }
                                    }
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
                        if should_forward_message(&msg, &subscriptions)
                            && write_server_message(&mut writer, &msg).await.is_err()
                        {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("IPC broadcast lagged by {} messages", n);
                        if !subscriptions.is_empty() {
                            let lag_msg = ServerMessage::Error {
                                message: format!(
                                    "subscription lagged by {} messages; stream output may be incomplete",
                                    n
                                ),
                            };
                            if write_server_message(&mut writer, &lag_msg).await.is_err() {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        }
    }

    info!("Client disconnected");
}

#[cfg(all(test, windows))]
mod tests {
    use super::should_forward_message;
    use std::collections::HashSet;
    use std::sync::Arc;
    use yatamux_protocol::types::{PaneId, SurfaceId};
    use yatamux_protocol::ServerMessage;

    #[test]
    fn subscribed_client_only_receives_target_pane_stream_events() {
        let mut subscriptions = HashSet::new();
        subscriptions.insert(PaneId(3));

        assert!(should_forward_message(
            &ServerMessage::Output {
                pane: PaneId(3),
                data: Arc::from(&b"ok"[..]),
            },
            &subscriptions,
        ));
        assert!(!should_forward_message(
            &ServerMessage::Output {
                pane: PaneId(4),
                data: Arc::from(&b"ng"[..]),
            },
            &subscriptions,
        ));
        assert!(!should_forward_message(
            &ServerMessage::PanesListed { panes: Vec::new() },
            &subscriptions,
        ));
        assert!(!should_forward_message(
            &ServerMessage::SurfaceCreated {
                id: SurfaceId(1),
                workspace: yatamux_protocol::types::WorkspaceId(1),
            },
            &subscriptions,
        ));
    }

    #[test]
    fn unsubscribed_client_keeps_receiving_broadcast_messages() {
        let subscriptions = HashSet::new();
        assert!(should_forward_message(
            &ServerMessage::PanesListed { panes: Vec::new() },
            &subscriptions,
        ));
        assert!(should_forward_message(
            &ServerMessage::PaneClosed { pane: PaneId(9) },
            &subscriptions,
        ));
    }

    #[test]
    fn subscribed_client_still_receives_exec_results() {
        let mut subscriptions = HashSet::new();
        subscriptions.insert(PaneId(3));

        assert!(should_forward_message(
            &ServerMessage::ExecResult {
                request_id: "req-1".to_string(),
                pane: PaneId(9),
                status: yatamux_protocol::types::ExecStatus::Completed,
                exit_code: Some(0),
                message: None,
            },
            &subscriptions,
        ));
    }
}
