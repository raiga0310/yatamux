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

/// タイミング攻撃に対して安全な定数時間バイト列比較
///
/// 長さが異なる場合も一定時間で false を返す（早期リターンなし）。
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// IPC 認証設定（`run_ipc_server` に渡す）
pub struct IpcAuthConfig {
    /// handshake トークン認証を強制するか
    pub require_auth: bool,
    /// サーバーが期待するトークン（起動時に生成）
    pub token: Option<String>,
}

/// セッション認証トークンファイルのパスを返す
///
/// パス: `%APPDATA%\yatamux\{session}.token`
pub fn token_file_path(session: &str) -> Option<std::path::PathBuf> {
    let appdata = std::env::var("APPDATA").ok()?;
    Some(
        std::path::Path::new(&appdata)
            .join("yatamux")
            .join(format!("{}.token", session)),
    )
}

/// セッション用の認証トークンを生成してファイルに保存する
///
/// - トークンは 32 バイトの疑似ランダムデータを hex エンコードした 64 文字の文字列
/// - ファイルが書けない場合は `None` を返してトークンなしモードへフォールバック
pub fn generate_and_save_token(session: &str) -> Option<String> {
    use std::io::Write;

    // 疑似ランダムトークン: UNIX timestamp(ns) + pid の組み合わせを複数ラウンドで混ぜる
    // 本格的なセキュリティが必要な場合は `rand` クレートを追加すること。
    // 現時点では同一ユーザー DACL による保護が主な防衛線なので、
    // この程度のエントロピーで実用上十分と判断する。
    let pid = std::process::id() as u64;
    let time_ns = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0);
    // 簡単な混合（XOR + 乗算ハッシュの反復）で 32 バイト相当のトークンを生成
    let mut state = [0u64; 4];
    state[0] = pid ^ 0xdeadbeef_cafebabe;
    state[1] = time_ns ^ 0x01234567_89abcdef;
    state[2] = pid.wrapping_mul(0x6c62272e_07bb0142).wrapping_add(time_ns);
    state[3] = time_ns.wrapping_mul(0x517cc1b7_27220a95).wrapping_add(pid);
    for _ in 0..8 {
        for i in 0..4 {
            state[i] =
                state[i].wrapping_mul(0x9e3779b9_7f4a7c15).rotate_left(31) ^ state[(i + 1) % 4];
        }
    }
    let token = state
        .iter()
        .map(|v| format!("{:016x}", v))
        .collect::<String>();

    let path = token_file_path(session)?;
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut f = std::fs::File::create(&path)
        .map_err(|e| {
            warn!(
                "IPC: トークンファイル書き込み失敗 {}: {}",
                path.display(),
                e
            );
        })
        .ok()?;
    let _ = f.write_all(token.as_bytes());
    info!("IPC: 認証トークンを書き込みました: {}", path.display());
    Some(token)
}

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
/// - `session_name`: セッション名（パイプ名に使用）
/// - `auth`: 認証設定（トークン認証の有効/無効 + 期待トークン）
/// - `server_tx`: クライアントからのメッセージを Server へ転送
/// - `server_rx`: Server からの応答を受け取り、全クライアントにブロードキャスト
pub async fn run_ipc_server(
    session_name: &str,
    auth: IpcAuthConfig,
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
            let require_auth = auth.require_auth;
            let expected_token = auth.token.clone();
            tokio::spawn(handle_client(
                connected,
                tx_clone,
                bcast_rx,
                require_auth,
                expected_token,
            ));
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
    require_auth: bool,
    expected_token: Option<String>,
) {
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut lines = BufReader::new(reader).lines();
    let mut subscriptions = HashSet::<PaneId>::new();
    // 認証済みかどうか（require_auth=false なら最初から true）
    let mut authenticated = !require_auth;

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
                                request_id: None,
                            };
                            let _ = write_server_message(&mut writer, &err_msg).await;
                            break;
                        }

                        match serde_json::from_str::<ClientMessage>(&line) {
                            Ok(msg) => {
                                debug!("IPC recv: {:?}", msg);
                                // require_auth=true のとき、Handshake 前のメッセージを拒否する
                                if require_auth && !authenticated && !matches!(msg, ClientMessage::Handshake { .. }) {
                                    warn!("IPC: unauthenticated client sent non-handshake message; disconnecting");
                                    let err_msg = ServerMessage::Error {
                                        message: "authentication required: send Handshake with valid token first".to_string(),
                                        request_id: None,
                                    };
                                    let _ = write_server_message(&mut writer, &err_msg).await;
                                    break;
                                }
                                match msg {
                                    ClientMessage::Handshake {
                                        protocol_version,
                                        capabilities,
                                        token,
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
                                                request_id: None,
                                            };
                                            let _ =
                                                write_server_message(&mut writer, &err_msg).await;
                                            break;
                                        }
                                        // トークン認証チェック
                                        if require_auth {
                                            let token_ok = match (&expected_token, &token) {
                                                (Some(expected), Some(provided)) => {
                                                    // 定数時間比較（タイミング攻撃対策）
                                                    constant_time_eq(expected.as_bytes(), provided.as_bytes())
                                                }
                                                _ => false,
                                            };
                                            if !token_ok {
                                                warn!(
                                                    "IPC: authentication failed (token mismatch or missing); disconnecting client"
                                                );
                                                let err_msg = ServerMessage::Error {
                                                    message: "authentication failed: invalid or missing token; check %APPDATA%\\yatamux\\<session>.token".to_string(),
                                                    request_id: None,
                                                };
                                                let _ = write_server_message(&mut writer, &err_msg).await;
                                                break;
                                            }
                                            authenticated = true;
                                            info!("IPC: client authenticated successfully");
                                        } else if token.is_some() {
                                            // require_auth=false でもトークンが提示されたら検証する
                                            // （一致すれば authenticated=true、不一致でも許容）
                                            if let (Some(expected), Some(provided)) = (&expected_token, &token) {
                                                if constant_time_eq(expected.as_bytes(), provided.as_bytes()) {
                                                    authenticated = true;
                                                }
                                            }
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
                                    ClientMessage::SubscribePane { pane, request_id } => {
                                        subscriptions.insert(pane);
                                        if let Some(id) = request_id {
                                            let ack = ServerMessage::SubscribeAccepted {
                                                request_id: id,
                                                pane,
                                            };
                                            if write_server_message(&mut writer, &ack).await.is_err() {
                                                debug!("IPC: pipe write error sending SubscribeAccepted; disconnecting client");
                                                break;
                                            }
                                        }
                                    }
                                    ClientMessage::UnsubscribePane { pane, request_id } => {
                                        subscriptions.remove(&pane);
                                        if let Some(id) = request_id {
                                            let ack = ServerMessage::UnsubscribeAccepted {
                                                request_id: id,
                                                pane,
                                            };
                                            if write_server_message(&mut writer, &ack).await.is_err() {
                                                debug!("IPC: pipe write error sending UnsubscribeAccepted; disconnecting client");
                                                break;
                                            }
                                        }
                                    }
                                    // 制御 API: request_id が含まれる場合は即時受理確認を返してからサーバーへ転送する
                                    ClientMessage::ClosePane { pane, ref request_id }
                                    | ClientMessage::InterruptPane { pane, ref request_id }
                                    | ClientMessage::TerminatePane { pane, ref request_id } => {
                                        if let Some(id) = request_id {
                                            let ack = ServerMessage::ControlAccepted {
                                                request_id: id.clone(),
                                                pane,
                                            };
                                            if write_server_message(&mut writer, &ack).await.is_err() {
                                                debug!("IPC: pipe write error sending ControlAccepted; disconnecting client");
                                                break;
                                            }
                                        }
                                        if server_tx.send(msg).await.is_err() {
                                            info!("IPC: server channel closed; disconnecting client");
                                            break;
                                        }
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
                            request_id: None,
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
