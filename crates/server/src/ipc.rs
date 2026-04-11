//! Windows 名前付きパイプ IPC サーバー
//!
//! 要件定義書 §5「IPC: Windows 名前付きパイプ」に対応。
//! パイプ名: \\.\pipe\yatamux-{session_name}
//!
//! ## セキュリティ
//! - Named Pipe の DACL を現在ユーザー SID に限定し、他ユーザーからの接続を拒否する
//! - 受信メッセージは `MAX_MESSAGE_BYTES` 超でエラーを返して切断する
//! - broadcast lagged 発生時はクライアントを切断し、黙って継続しない

use anyhow::{Context, Result};
use std::collections::HashSet;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, error, info, warn};

use yatamux_protocol::types::PaneId;
use yatamux_protocol::{
    ClientMessage, ServerMessage, MIN_CLIENT_VERSION, PROTOCOL_VERSION, SERVER_CAPABILITIES,
};

/// 名前付きパイプのベース名
pub const PIPE_PREFIX: &str = r"\\.\pipe\yatamux-";

/// 1 メッセージの最大サイズ（バイト）。超過したクライアントは切断する。
const MAX_MESSAGE_BYTES: usize = 1024 * 1024; // 1 MiB

#[cfg(windows)]
fn create_named_pipe_server(
    pipe_name: &str,
    first_pipe_instance: bool,
) -> Result<tokio::net::windows::named_pipe::NamedPipeServer> {
    use tokio::net::windows::named_pipe::ServerOptions;
    use windows::Win32::Security::{
        AddAccessAllowedAce, GetLengthSid, GetTokenInformation, InitializeAcl,
        InitializeSecurityDescriptor, SetSecurityDescriptorDacl, TokenUser, ACE_REVISION, ACL,
        PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES, SECURITY_DESCRIPTOR, TOKEN_QUERY, TOKEN_USER,
    };
    use windows::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    // ACL_REVISION = 2, GENERIC_ALL = 0x10000000
    const GENERIC_ALL: u32 = 0x10000000;

    // sizeof(ACL) = 8, sizeof(ACCESS_ALLOWED_ACE) = 12, - sizeof(DWORD) = 4
    const ACE_OVERHEAD: u32 = 8 + 12 - 4; // = 16

    // 現在ユーザーの SID を取得し、そのユーザーのみ許可する DACL を構築する
    let (mut acl_buf, sid_buf) = unsafe {
        let mut token = windows::Win32::Foundation::HANDLE::default();
        OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token)
            .context("OpenProcessToken failed")?;

        // TOKEN_USER サイズを問い合わせ
        let mut needed = 0u32;
        let _ = GetTokenInformation(token, TokenUser, None, 0, &mut needed);
        let mut sid_buf = vec![0u8; needed as usize];
        GetTokenInformation(
            token,
            TokenUser,
            Some(sid_buf.as_mut_ptr() as *mut _),
            needed,
            &mut needed,
        )
        .context("GetTokenInformation failed")?;
        let _ = windows::Win32::Foundation::CloseHandle(token);

        let token_user = &*(sid_buf.as_ptr() as *const TOKEN_USER);
        let sid = token_user.User.Sid;
        let sid_len = GetLengthSid(sid);

        // ACL バッファを確保して初期化
        let acl_size = ACE_OVERHEAD + sid_len;
        let mut acl_buf = vec![0u8; acl_size as usize];
        let acl_ptr = acl_buf.as_mut_ptr() as *mut ACL;
        InitializeAcl(acl_ptr, acl_size, ACE_REVISION(2)).context("InitializeAcl failed")?;
        AddAccessAllowedAce(acl_ptr, ACE_REVISION(2), GENERIC_ALL, sid)
            .context("AddAccessAllowedAce failed")?;

        (acl_buf, sid_buf)
    };

    let mut security_descriptor = SECURITY_DESCRIPTOR::default();
    let descriptor_ptr = PSECURITY_DESCRIPTOR(&mut security_descriptor as *mut _ as *mut _);
    unsafe {
        InitializeSecurityDescriptor(descriptor_ptr, 1)
            .context("Failed to initialize named pipe security descriptor")?;
        // 同一ユーザー SID のみ接続を許可する明示 DACL を設定する
        SetSecurityDescriptorDacl(
            descriptor_ptr,
            true,
            Some(acl_buf.as_mut_ptr() as *mut ACL),
            false,
        )
        .context("Failed to configure named pipe security descriptor")?;
    }

    let mut attrs = SECURITY_ATTRIBUTES {
        nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: &mut security_descriptor as *mut _ as *mut _,
        bInheritHandle: false.into(),
    };

    // acl_buf / sid_buf は create_with_security_attributes_raw が完了するまで有効でなければならない
    let mut options = ServerOptions::new();
    options.first_pipe_instance(first_pipe_instance);

    let result = unsafe {
        options.create_with_security_attributes_raw(pipe_name, &mut attrs as *mut _ as _)
    }
    .with_context(|| format!("Failed to create named pipe: {}", pipe_name));

    // バッファを明示的に保持（コンパイラに最適化で drop させない）
    drop(acl_buf);
    drop(sid_buf);

    result
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
        ServerMessage::ExecResult { .. } | ServerMessage::CiStatus { .. } => true,
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
                        // メッセージサイズ上限チェック（DoS 防止）
                        if line.len() > MAX_MESSAGE_BYTES {
                            error!(
                                "IPC: oversized message ({} bytes, limit {} bytes); disconnecting client",
                                line.len(),
                                MAX_MESSAGE_BYTES,
                            );
                            let err_msg = ServerMessage::Error {
                                message: format!(
                                    "message too large ({} bytes); limit is {} bytes",
                                    line.len(),
                                    MAX_MESSAGE_BYTES,
                                ),
                            };
                            let _ = write_server_message(&mut writer, &err_msg).await;
                            break;
                        }

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
                                            debug!("IPC: pipe write error sending HandshakeAccepted; disconnecting client");
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
                                            // サーバーチャネルが閉じている（Server が停止した）
                                            info!("IPC: server channel closed; disconnecting client");
                                            break;
                                        }
                                    }
                                }
                            }
                            Err(e) => warn!("IPC: failed to parse client message: {}", e),
                        }
                    }
                    Ok(None) => {
                        // クライアントが接続を正常に閉じた
                        debug!("IPC: client closed connection (EOF)");
                        break;
                    }
                    Err(e) => {
                        // パイプ IO エラー（クライアントが異常切断した場合など）
                        debug!("IPC: pipe read error; disconnecting client: {}", e);
                        break;
                    }
                }
            }
            result = bcast_rx.recv() => {
                match result {
                    Ok(msg) => {
                        if should_forward_message(&msg, &subscriptions)
                            && write_server_message(&mut writer, &msg).await.is_err()
                        {
                            debug!("IPC: pipe write error; disconnecting client");
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        // lagged 発生時はエラーを送信した後に切断する（黙って継続しない）
                        error!("IPC broadcast lagged by {} messages; disconnecting client", n);
                        let lag_msg = ServerMessage::Error {
                            message: format!(
                                "subscription lagged by {} messages; reconnect and use capture-pane --json to resync",
                                n
                            ),
                        };
                        let _ = write_server_message(&mut writer, &lag_msg).await;
                        break;
                    }
                    Err(broadcast::error::RecvError::Closed) => {
                        // サーバーの broadcast チャネルが閉じた（Server 停止）
                        info!("IPC: broadcast channel closed; disconnecting client");
                        break;
                    }
                }
            }
        }
    }

    info!("IPC: client disconnected");
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
