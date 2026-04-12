//! IPC 統合テスト（F 系）
//!
//! Windows 名前付きパイプ経由のクライアント/サーバー通信を検証する。
//! ConPTY および Win32 名前付きパイプに依存するため Windows 専用。

#![cfg(windows)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::sync::mpsc;
use yatamux_client::connection::ServerConnection;
use yatamux_protocol::{ClientMessage, ServerMessage};
use yatamux_server::ipc::run_ipc_server;

/// テストごとにユニークなセッション名を生成する
fn unique_session() -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("test-ipc-{}-{}", std::process::id(), n)
}

/// サーバーを起動し (server_cmd_tx, server_event_rx) を返す
/// IPC サーバーはバックグラウンドタスクで動作し続ける
#[allow(dead_code)]
fn start_ipc_server(session: &str) -> (mpsc::Sender<ClientMessage>, mpsc::Receiver<ServerMessage>) {
    use yatamux_server::Server;

    let (server_msg_tx, server_msg_rx) = mpsc::channel::<ServerMessage>(256);
    let (client_msg_tx, client_msg_rx) = mpsc::channel::<ClientMessage>(256);

    // ロジックサーバーを起動
    let logic_server = Server::new(server_msg_tx);
    tokio::spawn(logic_server.run(client_msg_rx));

    // IPC サーバーを起動（ループするので spawn）
    let session_owned = session.to_string();
    let (ipc_client_tx, _ipc_server_rx) = mpsc::channel::<ClientMessage>(256);
    let (_ipc_server_tx, ipc_client_rx) = mpsc::channel::<ServerMessage>(256);

    // IPC がクライアントから受け取ったメッセージをロジックサーバーに転送
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_owned, ipc_client_tx, ipc_client_rx).await;
    });

    // ※ run_ipc_server は ipc 内部チャネルを使う。
    // テストでは直接 ServerConnection::connect でパイプに繋ぐ。
    (client_msg_tx, server_msg_rx)
}

// F-1: IPC サーバーが起動し、クライアントが接続できる
#[tokio::test]
async fn test_ipc_server_accepts_connection() {
    let session = unique_session();

    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);

    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));

    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });

    // サーバー起動を少し待つ
    tokio::time::sleep(Duration::from_millis(100)).await;

    // F-1: 接続が確立できる
    let result =
        tokio::time::timeout(Duration::from_secs(2), ServerConnection::connect(&session)).await;
    assert!(result.is_ok(), "Should be able to connect within timeout");
    assert!(result.unwrap().is_ok(), "Connection should succeed");
}

// F-2: クライアントが JSON メッセージを送受信できる
#[tokio::test]
async fn test_ipc_send_receive_message() {
    let session = unique_session();

    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);

    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));

    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let mut conn = ServerConnection::connect(&session).await.unwrap();

    // F-2: CreateWorkspace を送信 → WorkspaceCreated が返る
    conn.tx
        .send(ClientMessage::CreateWorkspace {
            name: Some("ipc-test".to_string()),
        })
        .await
        .unwrap();

    let resp = tokio::time::timeout(Duration::from_secs(2), conn.rx.recv())
        .await
        .expect("timeout")
        .expect("channel closed");

    match resp {
        ServerMessage::WorkspaceCreated { name, .. } => {
            assert_eq!(name, "ipc-test");
        }
        other => panic!("unexpected: {:?}", other),
    }
}

// F-3: 複数クライアントが同時接続してそれぞれ応答を受け取れる
#[tokio::test]
async fn test_ipc_multiple_clients() {
    let session = unique_session();

    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);

    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));

    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    // クライアント 1 接続
    let mut conn1 = ServerConnection::connect(&session).await.unwrap();
    // クライアント 2 接続も即時に成功すること
    let mut conn2 = ServerConnection::connect(&session).await.unwrap();

    // 両方から送信
    conn1
        .tx
        .send(ClientMessage::CreateWorkspace {
            name: Some("client1".to_string()),
        })
        .await
        .unwrap();
    conn2
        .tx
        .send(ClientMessage::CreateWorkspace {
            name: Some("client2".to_string()),
        })
        .await
        .unwrap();

    // それぞれが WorkspaceCreated を受け取る（broadcast）
    let r1 = tokio::time::timeout(Duration::from_secs(2), conn1.rx.recv())
        .await
        .expect("c1 timeout")
        .expect("c1 closed");
    let r2 = tokio::time::timeout(Duration::from_secs(2), conn2.rx.recv())
        .await
        .expect("c2 timeout")
        .expect("c2 closed");

    assert!(
        matches!(r1, ServerMessage::WorkspaceCreated { .. }),
        "client1 should receive WorkspaceCreated"
    );
    assert!(
        matches!(r2, ServerMessage::WorkspaceCreated { .. }),
        "client2 should receive WorkspaceCreated"
    );
}

// F-6: IPC 経由で ListPanes を送ると PanesListed が返る
#[tokio::test]
async fn test_ipc_list_panes_returns_panes_listed() {
    let session = unique_session();
    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);
    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));
    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let mut conn = ServerConnection::connect(&session).await.unwrap();
    conn.tx.send(ClientMessage::ListPanes).await.unwrap();

    let resp = tokio::time::timeout(std::time::Duration::from_secs(2), conn.rx.recv())
        .await
        .expect("timeout")
        .expect("closed");
    assert!(
        matches!(resp, ServerMessage::PanesListed { .. }),
        "expected PanesListed, got {:?}",
        resp
    );
}

// F-7: IPC 経由で Input を送ると対象ペインから Output が返る
#[tokio::test]
async fn test_ipc_send_keys_routes_to_pane() {
    let session = unique_session();
    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);
    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));
    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let mut conn = ServerConnection::connect(&session).await.unwrap();

    // ワークスペース → サーフェス → ペイン を作成
    conn.tx
        .send(ClientMessage::CreateWorkspace { name: None })
        .await
        .unwrap();
    let ws_id = loop {
        if let ServerMessage::WorkspaceCreated { id, .. } = conn.rx.recv().await.unwrap() {
            break id;
        }
    };
    conn.tx
        .send(ClientMessage::CreateSurface { workspace: ws_id })
        .await
        .unwrap();
    let surf_id = loop {
        if let ServerMessage::SurfaceCreated { id, .. } = conn.rx.recv().await.unwrap() {
            break id;
        }
    };
    use yatamux_protocol::types::TermSize;
    conn.tx
        .send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
            working_dir: None,
        })
        .await
        .unwrap();
    let pane_id = loop {
        if let ServerMessage::PaneCreated { id, .. } = conn.rx.recv().await.unwrap() {
            break id;
        }
    };

    // Input を送信 → Output が返ってくることを確認
    conn.tx
        .send(ClientMessage::Input {
            pane: pane_id,
            data: b"echo yatamux\r".to_vec(),
        })
        .await
        .unwrap();

    let got_output = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match conn.rx.recv().await.unwrap() {
                ServerMessage::Output { pane, .. } if pane == pane_id => return true,
                ServerMessage::Error { message } => panic!("server error: {}", message),
                _ => continue,
            }
        }
    })
    .await;
    assert!(
        got_output.is_ok(),
        "should receive Output from pane after Input"
    );
}

// C-32: SubscribePane を送ると対象ペインの Output だけを受け取る
#[tokio::test]
async fn test_ipc_subscribe_pane_filters_output_to_target_pane() {
    let session = unique_session();
    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);
    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));
    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let mut conn = ServerConnection::connect(&session).await.unwrap();

    conn.tx
        .send(ClientMessage::CreateWorkspace { name: None })
        .await
        .unwrap();
    let ws_id = loop {
        if let ServerMessage::WorkspaceCreated { id, .. } = conn.rx.recv().await.unwrap() {
            break id;
        }
    };

    conn.tx
        .send(ClientMessage::CreateSurface { workspace: ws_id })
        .await
        .unwrap();
    let surf_id = loop {
        if let ServerMessage::SurfaceCreated { id, .. } = conn.rx.recv().await.unwrap() {
            break id;
        }
    };

    use yatamux_protocol::types::TermSize;
    conn.tx
        .send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
            working_dir: None,
        })
        .await
        .unwrap();
    let pane1 = loop {
        if let ServerMessage::PaneCreated { id, .. } = conn.rx.recv().await.unwrap() {
            break id;
        }
    };

    conn.tx
        .send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: Some(pane1),
            direction: Some(yatamux_protocol::types::SplitDirection::Vertical),
            size: TermSize { cols: 80, rows: 24 },
            working_dir: None,
        })
        .await
        .unwrap();
    let pane2 = loop {
        if let ServerMessage::PaneCreated { id, .. } = conn.rx.recv().await.unwrap() {
            break id;
        }
    };

    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    while let Ok(Some(_)) =
        tokio::time::timeout(std::time::Duration::from_millis(10), conn.rx.recv()).await
    {}

    conn.tx
        .send(ClientMessage::SubscribePane {
            pane: pane1,
            request_id: None,
        })
        .await
        .unwrap();

    conn.tx
        .send(ClientMessage::Input {
            pane: pane2,
            data: b"echo second\r".to_vec(),
        })
        .await
        .unwrap();

    let pane2_visible = tokio::time::timeout(std::time::Duration::from_millis(700), async {
        loop {
            match conn.rx.recv().await.unwrap() {
                ServerMessage::Output { pane, .. } if pane == pane2 => return true,
                _ => continue,
            }
        }
    })
    .await;
    assert!(
        pane2_visible.is_err(),
        "subscribed client should not receive output from non-target pane"
    );

    conn.tx
        .send(ClientMessage::Input {
            pane: pane1,
            data: b"echo first\r".to_vec(),
        })
        .await
        .unwrap();

    let pane1_visible = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match conn.rx.recv().await.unwrap() {
                ServerMessage::Output { pane, .. } if pane == pane1 => return true,
                ServerMessage::Error { message } => panic!("server error: {}", message),
                _ => continue,
            }
        }
    })
    .await;
    assert!(
        pane1_visible.is_ok(),
        "subscribed client should receive output from target pane"
    );
}

// C-25: 存在しない PaneId に Input を送ると Error が返る
#[tokio::test]
async fn test_ipc_send_keys_to_unknown_pane_returns_error() {
    let session = unique_session();
    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);
    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));
    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let mut conn = ServerConnection::connect(&session).await.unwrap();
    use yatamux_protocol::types::PaneId;
    conn.tx
        .send(ClientMessage::Input {
            pane: PaneId(9999),
            data: b"hello\r".to_vec(),
        })
        .await
        .unwrap();

    let err = tokio::time::timeout(std::time::Duration::from_millis(500), async {
        loop {
            if let ServerMessage::Error { message } = conn.rx.recv().await.unwrap() {
                return message;
            }
        }
    })
    .await
    .expect("expected Error for unknown pane");
    assert!(
        err.contains("pane 9999 not found"),
        "unknown pane should return not found error"
    );
}

// C-42: Handshake 成功 — HandshakeAccepted が返り、protocol_version と capabilities を含む
#[tokio::test]
async fn test_ipc_handshake_returns_accepted() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;
    use yatamux_protocol::{MIN_CLIENT_VERSION, PROTOCOL_VERSION, SERVER_CAPABILITIES};

    let session = unique_session();
    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);
    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));
    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let pipe_name = format!(r"\\.\pipe\yatamux-{}", session);
    let pipe = ClientOptions::new().open(&pipe_name).unwrap();
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut lines = tokio::io::BufReader::new(reader).lines();

    // Handshake 送信
    let hs = serde_json::to_string(&ClientMessage::Handshake {
        protocol_version: PROTOCOL_VERSION,
        capabilities: SERVER_CAPABILITIES.iter().map(|s| s.to_string()).collect(),
    })
    .unwrap();
    writer
        .write_all(format!("{}\n", hs).as_bytes())
        .await
        .unwrap();

    // HandshakeAccepted を受信
    let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .expect("timeout")
        .expect("io error")
        .expect("no line");

    let msg: ServerMessage = serde_json::from_str(&line).unwrap();
    match msg {
        ServerMessage::HandshakeAccepted {
            protocol_version,
            min_client_version,
            capabilities,
        } => {
            assert_eq!(
                protocol_version, PROTOCOL_VERSION,
                "protocol_version mismatch"
            );
            assert_eq!(
                min_client_version, MIN_CLIENT_VERSION,
                "min_client_version mismatch"
            );
            for cap in SERVER_CAPABILITIES {
                assert!(
                    capabilities.contains(&cap.to_string()),
                    "missing capability: {}",
                    cap
                );
            }
        }
        other => panic!("expected HandshakeAccepted, got: {:?}", other),
    }
}

// C-42: バージョン不一致 — 古いクライアントバージョンはエラーで切断される
#[tokio::test]
async fn test_ipc_handshake_old_version_rejected() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;
    use yatamux_protocol::MIN_CLIENT_VERSION;

    let session = unique_session();
    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);
    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));
    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let pipe_name = format!(r"\\.\pipe\yatamux-{}", session);
    let pipe = ClientOptions::new().open(&pipe_name).unwrap();
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut lines = tokio::io::BufReader::new(reader).lines();

    // MIN_CLIENT_VERSION より古いバージョンを送信
    let old_version = MIN_CLIENT_VERSION.saturating_sub(1);
    // MIN_CLIENT_VERSION が 1 なので 0 を送る（MIN が 0 なら skip できないが現在は 1 固定）
    let hs = serde_json::to_string(&ClientMessage::Handshake {
        protocol_version: old_version,
        capabilities: vec![],
    })
    .unwrap();
    writer
        .write_all(format!("{}\n", hs).as_bytes())
        .await
        .unwrap();

    // Error が返り、接続が切断される
    let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .expect("timeout")
        .expect("io error")
        .expect("no line");

    let msg: ServerMessage = serde_json::from_str(&line).unwrap();
    match msg {
        ServerMessage::Error { message } => {
            assert!(
                message.contains("not supported") || message.contains("minimum"),
                "error should mention version mismatch: {}",
                message
            );
        }
        other => panic!("expected Error, got: {:?}", other),
    }

    // 切断確認（次の読み取りは None になる）
    let next = tokio::time::timeout(Duration::from_millis(500), lines.next_line())
        .await
        .expect("timeout waiting for disconnect")
        .expect("io error");
    assert!(
        next.is_none(),
        "connection should be closed after version rejection"
    );
}

// C-42: Handshake なし（レガシー互換）— 旧クライアントでも接続が機能する
#[tokio::test]
async fn test_ipc_legacy_client_without_handshake_still_works() {
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};
    use tokio::net::windows::named_pipe::ClientOptions;

    let session = unique_session();
    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);
    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));
    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });
    tokio::time::sleep(Duration::from_millis(100)).await;

    let pipe_name = format!(r"\\.\pipe\yatamux-{}", session);
    let pipe = ClientOptions::new().open(&pipe_name).unwrap();
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut lines = tokio::io::BufReader::new(reader).lines();

    // Handshake なしで直接 ListPanes を送る（レガシーモード）
    let msg = serde_json::to_string(&ClientMessage::ListPanes).unwrap();
    writer
        .write_all(format!("{}\n", msg).as_bytes())
        .await
        .unwrap();

    // PanesListed が返ること（接続が正常に機能している）
    let line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .expect("timeout")
        .expect("io error")
        .expect("no line");

    let resp: ServerMessage = serde_json::from_str(&line).unwrap();
    assert!(
        matches!(resp, ServerMessage::PanesListed { .. }),
        "legacy client without Handshake should still receive PanesListed, got: {:?}",
        resp
    );
}

// F-5: 不正な JSON を送っても接続が維持される（次のメッセージが処理できる）
// 現状の ipc.rs は warn ログを出して継続するため、切断されないことを確認。
#[tokio::test]
async fn test_ipc_invalid_json_does_not_drop_connection() {
    use tokio::io::AsyncWriteExt;
    use tokio::net::windows::named_pipe::ClientOptions;

    let session = unique_session();

    let (server_cmd_tx, server_event_rx) = mpsc::channel::<ClientMessage>(64);
    let (server_out_tx, server_out_rx) = mpsc::channel::<ServerMessage>(64);

    use yatamux_server::Server;
    let logic = Server::new(server_out_tx);
    tokio::spawn(logic.run(server_event_rx));

    let session_c = session.clone();
    tokio::spawn(async move {
        let _ = run_ipc_server(&session_c, server_cmd_tx, server_out_rx).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let pipe_name = format!(r"\\.\pipe\yatamux-{}", session);
    let mut pipe = ClientOptions::new().open(&pipe_name).unwrap();

    // 不正 JSON を送信
    pipe.write_all(b"this is not valid json\n").await.unwrap();

    // 少し待ってから正常なメッセージを送信
    tokio::time::sleep(Duration::from_millis(100)).await;

    let valid = serde_json::to_string(&ClientMessage::CreateWorkspace {
        name: Some("after-invalid".to_string()),
    })
    .unwrap();
    pipe.write_all(format!("{}\n", valid).as_bytes())
        .await
        .unwrap();

    // ServerConnection で読むのでなく、直接 BufReader で読む
    use tokio::io::AsyncBufReadExt;
    let mut lines = tokio::io::BufReader::new(pipe).lines();

    let resp_line = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
        .await
        .expect("timeout")
        .expect("io error")
        .expect("no line");

    let msg: ServerMessage = serde_json::from_str(&resp_line).unwrap();
    match msg {
        ServerMessage::WorkspaceCreated { name, .. } => {
            assert_eq!(name, "after-invalid");
        }
        other => panic!("unexpected: {:?}", other),
    }
}
