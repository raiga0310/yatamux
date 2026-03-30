//! サーバーセッション管理
//!
//! Workspace → Surface → Pane の階層を管理する。
//! cmux のワークフローモデルに対応。

mod handlers;
mod model;
mod tree;

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::mpsc;

use yatamux_protocol::types::{PaneId, SurfaceId, WorkspaceId};
use yatamux_protocol::{ClientMessage, ServerMessage};
use yatamux_terminal::CjkWidthConfig;

use crate::pane::{Pane, PaneEvent};
pub use model::{PaneTree, Surface, Workspace};

/// サーバー本体
pub struct Server {
    workspaces: HashMap<WorkspaceId, Workspace>,
    surfaces: HashMap<SurfaceId, Surface>,
    panes: HashMap<PaneId, Pane>,
    next_workspace_id: u32,
    next_surface_id: u32,
    next_pane_id: u32,
    width_config: CjkWidthConfig,
    /// クライアントへの出力チャネル（IPC 層が設定）
    client_tx: mpsc::Sender<ServerMessage>,
    /// ペインからの出力を受け取るチャネル
    pane_output_rx: mpsc::Receiver<(PaneId, Arc<[u8]>)>,
    pane_output_tx: mpsc::Sender<(PaneId, Arc<[u8]>)>,
    /// ペインからの内部イベントを受け取るチャネル
    pane_event_rx: mpsc::Receiver<(PaneId, PaneEvent)>,
    pane_event_tx: mpsc::Sender<(PaneId, PaneEvent)>,
}

impl Server {
    pub fn new(client_tx: mpsc::Sender<ServerMessage>) -> Self {
        let (pane_output_tx, pane_output_rx) = mpsc::channel(1024);
        let (pane_event_tx, pane_event_rx) = mpsc::channel(256);
        Self {
            workspaces: HashMap::new(),
            surfaces: HashMap::new(),
            panes: HashMap::new(),
            next_workspace_id: 1,
            next_surface_id: 1,
            next_pane_id: 1,
            width_config: CjkWidthConfig::default(),
            client_tx,
            pane_output_rx,
            pane_output_tx,
            pane_event_rx,
            pane_event_tx,
        }
    }

    /// イベントループを開始する
    pub async fn run(mut self, mut client_rx: mpsc::Receiver<ClientMessage>) {
        loop {
            tokio::select! {
                // クライアントからのメッセージ処理
                Some(msg) = client_rx.recv() => {
                    if let Err(e) = self.handle_client_message(msg).await {
                        let _ = self.client_tx.send(ServerMessage::Error {
                            message: e.to_string(),
                        }).await;
                    }
                }
                // ペインからの出力転送
                Some((pane_id, data)) = self.pane_output_rx.recv() => {
                    let _ = self.client_tx.send(ServerMessage::Output {
                        pane: pane_id,
                        data,
                    }).await;
                }
                // ペインからの内部イベント転送
                Some((pane_id, event)) = self.pane_event_rx.recv() => {
                    self.forward_pane_event(pane_id, event).await;
                }
            }
        }
    }

    async fn send_notification(&mut self, pane: PaneId, body: String) {
        let _ = self
            .client_tx
            .send(ServerMessage::Notification { pane, body })
            .await;
    }

    async fn send_pane_closed(&mut self, pane: PaneId) {
        let _ = self
            .client_tx
            .send(ServerMessage::PaneClosed { pane })
            .await;
    }

    async fn send_command_finished(&mut self, pane: PaneId, exit_code: Option<i32>) {
        let _ = self
            .client_tx
            .send(ServerMessage::CommandFinished { pane, exit_code })
            .await;
    }

    async fn forward_pane_event(&mut self, pane: PaneId, event: PaneEvent) {
        match event {
            PaneEvent::Notification(body) => self.send_notification(pane, body).await,
            PaneEvent::Bell => self.send_notification(pane, "Bell".to_string()).await,
            PaneEvent::ProcessExited => {
                self.send_notification(pane, "Process exited".to_string())
                    .await;
                self.panes.remove(&pane);
                self.send_pane_closed(pane).await;
            }
            PaneEvent::CommandFinished(exit_code) => {
                self.send_command_finished(pane, exit_code).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use yatamux_protocol::types::{SurfaceId, TermSize, WorkspaceId};
    use yatamux_protocol::{ClientMessage, ServerMessage};

    /// テスト用サーバーを起動し (client_tx, server_rx) を返す
    fn start_server() -> (mpsc::Sender<ClientMessage>, mpsc::Receiver<ServerMessage>) {
        let (server_msg_tx, server_msg_rx) = mpsc::channel::<ServerMessage>(64);
        let (client_msg_tx, client_msg_rx) = mpsc::channel::<ClientMessage>(64);
        let server = Server::new(server_msg_tx);
        tokio::spawn(server.run(client_msg_rx));
        (client_msg_tx, server_msg_rx)
    }

    /// テスト用サーバーを起動し、内部通知チャネルも返す。
    fn start_server_with_notifier() -> (
        mpsc::Sender<ClientMessage>,
        mpsc::Receiver<ServerMessage>,
        mpsc::Sender<(PaneId, PaneEvent)>,
    ) {
        let (server_msg_tx, server_msg_rx) = mpsc::channel::<ServerMessage>(64);
        let (client_msg_tx, client_msg_rx) = mpsc::channel::<ClientMessage>(64);
        let server = Server::new(server_msg_tx);
        let notifier = server.pane_event_tx.clone();
        tokio::spawn(server.run(client_msg_rx));
        (client_msg_tx, server_msg_rx, notifier)
    }

    /// 1 秒タイムアウト付きで次のメッセージを受信する
    async fn recv_one(rx: &mut mpsc::Receiver<ServerMessage>) -> ServerMessage {
        tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for server message")
            .expect("server channel closed")
    }

    /// Output / Notification / PaneClosed を読み飛ばし、次の「制御」メッセージを返す。
    ///
    /// PTY が起動直後に出力を流し始めるため、テストで特定のレスポンス
    /// (PaneCreated 等) を待つときは Output が先着する可能性がある。
    /// このヘルパーはそれらを無視して期待するメッセージだけを返す。
    /// 全体タイムアウト 60 秒: それを超えた場合はデッドロック等とみなして panic。
    async fn recv_ctrl(rx: &mut mpsc::Receiver<ServerMessage>) -> ServerMessage {
        tokio::time::timeout(Duration::from_secs(60), async {
            loop {
                match recv_one(rx).await {
                    ServerMessage::Output { .. }
                    | ServerMessage::Notification { .. }
                    | ServerMessage::CommandFinished { .. }
                    | ServerMessage::PaneClosed { .. } => continue,
                    other => return other,
                }
            }
        })
        .await
        .expect("recv_ctrl: timed out after 60s — suspected deadlock or resource exhaustion")
    }

    /// テスト全体を 120 秒でタイムアウトさせるラッパー。
    ///
    /// デッドロック・無限ループ・OS リソース枯渇によるハングを防ぐ。
    /// 非同期デッドロックはコンパイル時に保証できないため、
    /// すべてのテストでこのラッパーを使用すること。
    async fn with_timeout<F: std::future::Future<Output = ()>>(test_fn: F) {
        tokio::time::timeout(Duration::from_secs(120), test_fn)
            .await
            .expect("test timed out after 120s — likely deadlock or resource exhaustion")
    }

    // G-1: CreateWorkspace → WorkspaceCreated が返る
    #[tokio::test]
    async fn test_create_workspace() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace {
                name: Some("ws1".to_string()),
            })
            .await
            .unwrap();
            match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, name } => {
                    assert_eq!(id, WorkspaceId(1));
                    assert_eq!(name, "ws1");
                }
                other => panic!("unexpected: {:?}", other),
            }
        })
        .await;
    }

    // G-1: 名前なしワークスペースも作成できる（自動命名）
    #[tokio::test]
    async fn test_create_workspace_auto_name() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { name, .. } => {
                    assert!(!name.is_empty(), "auto name should not be empty");
                }
                other => panic!("unexpected: {:?}", other),
            }
        })
        .await;
    }

    // G-2: CreateSurface → SurfaceCreated が返り WorkspaceId と紐づく
    #[tokio::test]
    async fn test_create_surface() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                other => panic!("unexpected: {:?}", other),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, workspace } => {
                    assert_eq!(id, SurfaceId(1));
                    assert_eq!(workspace, ws_id);
                }
                other => panic!("unexpected: {:?}", other),
            }
        })
        .await;
    }

    // G-3: CreatePane → PaneCreated が返る (Windows のみ: PTY spawn が必要)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_create_pane() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size: TermSize { cols: 80, rows: 24 },
                working_dir: None,
            })
            .await
            .unwrap();
            match recv_one(&mut rx).await {
                ServerMessage::PaneCreated { id, surface, .. } => {
                    assert_eq!(surface, surf_id);
                    assert_eq!(id, yatamux_protocol::types::PaneId(1));
                }
                other => panic!("unexpected: {:?}", other),
            }
        })
        .await;
    }

    // G-5: PTY 出力がクライアントに Output メッセージとして届く (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_pane_output_forwarded_to_client() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size: TermSize { cols: 80, rows: 24 },
                working_dir: None,
            })
            .await
            .unwrap();
            let pane_id = match recv_one(&mut rx).await {
                ServerMessage::PaneCreated { id, .. } => id,
                _ => panic!(),
            };
            // cmd.exe が起動すると初期プロンプトが出力されるはず
            let output = tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let ServerMessage::Output { pane, data } = recv_one(&mut rx).await {
                        if pane == pane_id && !data.is_empty() {
                            return data;
                        }
                    }
                }
            })
            .await
            .expect("timeout: no output from PTY");
            assert!(!output.is_empty(), "initial PTY output should be non-empty");
        })
        .await;
    }

    // G-4: Input メッセージがペインに届く（Error が返らない）(Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_input_routed_to_pane_without_error() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size: TermSize { cols: 80, rows: 24 },
                working_dir: None,
            })
            .await
            .unwrap();
            let pane_id = match recv_one(&mut rx).await {
                ServerMessage::PaneCreated { id, .. } => id,
                _ => panic!(),
            };
            // 初期出力が来るまで待つ
            tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let ServerMessage::Output { .. } = recv_one(&mut rx).await {
                        break;
                    }
                }
            })
            .await
            .ok();
            // 入力送信
            tx.send(ClientMessage::Input {
                pane: pane_id,
                data: b"echo test_input\r".to_vec(),
            })
            .await
            .unwrap();
            // 500ms 以内に Error が来ないことを確認
            let got_error = tokio::time::timeout(Duration::from_millis(500), async {
                loop {
                    if let ServerMessage::Error { .. } = recv_one(&mut rx).await {
                        return true;
                    }
                }
            })
            .await;
            assert!(
                got_error.is_err(),
                "no error should be received after Input"
            );
        })
        .await;
    }

    // G-6: ClosePane → PaneClosed が返る (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_close_pane() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size: TermSize { cols: 80, rows: 24 },
                working_dir: None,
            })
            .await
            .unwrap();
            let pane_id = match recv_one(&mut rx).await {
                ServerMessage::PaneCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::ClosePane { pane: pane_id })
                .await
                .unwrap();
            let closed = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let ServerMessage::PaneClosed { pane } = recv_one(&mut rx).await {
                        return pane;
                    }
                }
            })
            .await
            .expect("timeout waiting for PaneClosed");
            assert_eq!(closed, pane_id);
        })
        .await;
    }

    // G-7: Resize メッセージでエラーが返らない (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_resize_pane() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size: TermSize { cols: 80, rows: 24 },
                working_dir: None,
            })
            .await
            .unwrap();
            let pane_id = match recv_one(&mut rx).await {
                ServerMessage::PaneCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::Resize {
                pane: pane_id,
                size: TermSize {
                    cols: 120,
                    rows: 40,
                },
            })
            .await
            .unwrap();
            let got_error = tokio::time::timeout(Duration::from_millis(300), async {
                loop {
                    if let ServerMessage::Error { .. } = recv_one(&mut rx).await {
                        return true;
                    }
                }
            })
            .await;
            assert!(got_error.is_err(), "Resize should not produce an error");
        })
        .await;
    }

    #[tokio::test]
    async fn test_command_finished_notification_forwarded_with_exit_code() {
        with_timeout(async {
            let (_tx, mut rx, notifier) = start_server_with_notifier();
            notifier
                .send((PaneId(7), PaneEvent::CommandFinished(Some(42))))
                .await
                .unwrap();

            match recv_one(&mut rx).await {
                ServerMessage::CommandFinished { pane, exit_code } => {
                    assert_eq!(pane, PaneId(7));
                    assert_eq!(exit_code, Some(42));
                }
                other => panic!("unexpected: {:?}", other),
            }
        })
        .await;
    }

    #[tokio::test]
    async fn test_command_finished_notification_forwarded_without_exit_code() {
        with_timeout(async {
            let (_tx, mut rx, notifier) = start_server_with_notifier();
            notifier
                .send((PaneId(9), PaneEvent::CommandFinished(None)))
                .await
                .unwrap();

            match recv_one(&mut rx).await {
                ServerMessage::CommandFinished { pane, exit_code } => {
                    assert_eq!(pane, PaneId(9));
                    assert_eq!(exit_code, None);
                }
                other => panic!("unexpected: {:?}", other),
            }
        })
        .await;
    }

    #[tokio::test]
    async fn test_notification_event_forwarded() {
        with_timeout(async {
            let (_tx, mut rx, notifier) = start_server_with_notifier();
            notifier
                .send((PaneId(3), PaneEvent::Notification("hello".to_string())))
                .await
                .unwrap();

            match recv_one(&mut rx).await {
                ServerMessage::Notification { pane, body } => {
                    assert_eq!(pane, PaneId(3));
                    assert_eq!(body, "hello");
                }
                other => panic!("unexpected: {:?}", other),
            }
        })
        .await;
    }

    #[tokio::test]
    async fn test_bell_event_forwarded_as_notification() {
        with_timeout(async {
            let (_tx, mut rx, notifier) = start_server_with_notifier();
            notifier.send((PaneId(5), PaneEvent::Bell)).await.unwrap();

            match recv_one(&mut rx).await {
                ServerMessage::Notification { pane, body } => {
                    assert_eq!(pane, PaneId(5));
                    assert_eq!(body, "Bell");
                }
                other => panic!("unexpected: {:?}", other),
            }
        })
        .await;
    }

    #[tokio::test]
    async fn test_process_exited_event_closes_pane() {
        with_timeout(async {
            let (_tx, mut rx, notifier) = start_server_with_notifier();
            notifier
                .send((PaneId(11), PaneEvent::ProcessExited))
                .await
                .unwrap();

            match recv_one(&mut rx).await {
                ServerMessage::Notification { pane, body } => {
                    assert_eq!(pane, PaneId(11));
                    assert_eq!(body, "Process exited");
                }
                other => panic!("unexpected first event: {:?}", other),
            }

            match recv_one(&mut rx).await {
                ServerMessage::PaneClosed { pane } => {
                    assert_eq!(pane, PaneId(11));
                }
                other => panic!("unexpected second event: {:?}", other),
            }
        })
        .await;
    }

    // G-8: ListPanes → PanesListed に全ペインが含まれる (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_list_panes_returns_all_panes() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            let size = TermSize { cols: 80, rows: 24 };
            // ペイン 1 作成
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size,
                working_dir: None,
            })
            .await
            .unwrap();
            let pane1_id = match recv_ctrl(&mut rx).await {
                ServerMessage::PaneCreated { id, .. } => id,
                other => panic!("expected PaneCreated, got {:?}", other),
            };
            // ペイン 2 作成（分割）
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: Some(pane1_id),
                direction: Some(yatamux_protocol::types::SplitDirection::Vertical),
                size: TermSize { cols: 40, rows: 24 },
                working_dir: None,
            })
            .await
            .unwrap();
            match recv_ctrl(&mut rx).await {
                ServerMessage::PaneCreated { .. } => {}
                other => panic!("expected PaneCreated, got {:?}", other),
            }
            // ListPanes を送信
            tx.send(ClientMessage::ListPanes).await.unwrap();
            let panes = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let ServerMessage::PanesListed { panes } = recv_one(&mut rx).await {
                        return panes;
                    }
                }
            })
            .await
            .expect("timeout waiting for PanesListed");
            assert_eq!(panes.len(), 2);
            assert!(panes.iter().all(|p| p.surface == surf_id));
        })
        .await;
    }

    // G-9: ペインなしで ListPanes → 空リストが返る
    #[tokio::test]
    async fn test_list_panes_returns_empty_when_no_panes() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::ListPanes).await.unwrap();
            let panes = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let ServerMessage::PanesListed { panes } = recv_one(&mut rx).await {
                        return panes;
                    }
                }
            })
            .await
            .expect("timeout waiting for PanesListed");
            assert!(panes.is_empty());
        })
        .await;
    }

    // G-10: 非アクティブペインへの Input でエラーが返らない (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_send_input_to_inactive_pane() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            let size = TermSize { cols: 80, rows: 24 };
            // ペイン 1 作成
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size,
                working_dir: None,
            })
            .await
            .unwrap();
            let pane1_id = match recv_ctrl(&mut rx).await {
                ServerMessage::PaneCreated { id, .. } => id,
                other => panic!("expected PaneCreated, got {:?}", other),
            };
            // ペイン 2 作成（分割）
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: Some(pane1_id),
                direction: Some(yatamux_protocol::types::SplitDirection::Vertical),
                size: TermSize { cols: 40, rows: 24 },
                working_dir: None,
            })
            .await
            .unwrap();
            let pane2_id = match recv_ctrl(&mut rx).await {
                ServerMessage::PaneCreated { id, .. } => id,
                other => panic!("expected PaneCreated, got {:?}", other),
            };
            let pane_ids = vec![pane1_id, pane2_id];
            // 2 番目のペイン（非アクティブ）に Input を送信
            tx.send(ClientMessage::Input {
                pane: pane_ids[1],
                data: b"echo hello\r".to_vec(),
            })
            .await
            .unwrap();
            // エラーが来ないことを確認
            let got_error = tokio::time::timeout(Duration::from_millis(500), async {
                loop {
                    if let ServerMessage::Error { .. } = recv_one(&mut rx).await {
                        return true;
                    }
                }
            })
            .await;
            assert!(
                got_error.is_err(),
                "Input to inactive pane should not produce an error"
            );
        })
        .await;
    }

    // TC-C13-03: 存在しないペインに CapturePane → Error が返る
    #[tokio::test]
    async fn test_capture_pane_nonexistent_pane_returns_error() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CapturePane {
                pane: PaneId(9999),
                lines: 100,
                plain_text: true,
            })
            .await
            .unwrap();
            let msg = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let ServerMessage::Error { message } = recv_one(&mut rx).await {
                        return message;
                    }
                }
            })
            .await
            .expect("timeout waiting for Error");
            assert!(
                msg.contains("pane 9999 not found"),
                "non-existent pane should return not found error"
            );
        })
        .await;
    }

    // TC-C13-04 / TC-C35-04: lines=0 に CapturePane → 空 content と capture メタデータが返る (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_capture_pane_lines_zero_returns_empty() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size: TermSize { cols: 80, rows: 24 },
                working_dir: None,
            })
            .await
            .unwrap();
            let pane_id = match recv_one(&mut rx).await {
                ServerMessage::PaneCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CapturePane {
                pane: pane_id,
                lines: 0,
                plain_text: true,
            })
            .await
            .unwrap();
            let (msg, capture) = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let ServerMessage::PaneContent {
                        content, capture, ..
                    } = recv_one(&mut rx).await
                    {
                        return (content, capture);
                    }
                }
            })
            .await
            .expect("timeout waiting for PaneContent");
            assert!(msg.is_empty(), "lines=0 should return empty content");
            let capture = capture.expect("capture metadata should be present");
            assert_eq!(capture.lines_requested, 0);
            assert!(capture.visible_text.is_empty());
            assert!(capture.scrollback_tail.is_empty());
            assert!(capture.cols > 0);
            assert!(capture.rows > 0);
        })
        .await;
    }

    // TC-C13-05 / TC-C35-05: 実在するペインに CapturePane → PaneContent と capture が返る (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_capture_pane_returns_pane_content() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size: TermSize { cols: 80, rows: 24 },
                working_dir: None,
            })
            .await
            .unwrap();
            let pane_id = match recv_one(&mut rx).await {
                ServerMessage::PaneCreated { id, .. } => id,
                _ => panic!(),
            };
            // PTY 初期出力が届くまで待機
            tokio::time::timeout(Duration::from_secs(3), async {
                loop {
                    if let ServerMessage::Output { .. } = recv_one(&mut rx).await {
                        break;
                    }
                }
            })
            .await
            .ok();
            // 少し待ってから CapturePane を送信
            tokio::time::sleep(Duration::from_millis(200)).await;
            tx.send(ClientMessage::CapturePane {
                pane: pane_id,
                lines: 100,
                plain_text: true,
            })
            .await
            .unwrap();
            let (content, capture) = tokio::time::timeout(Duration::from_secs(2), async {
                loop {
                    if let ServerMessage::PaneContent {
                        pane,
                        content,
                        capture,
                    } = recv_one(&mut rx).await
                    {
                        if pane == pane_id {
                            return (content, capture);
                        }
                    }
                }
            })
            .await
            .expect("timeout waiting for PaneContent");
            // PaneContent が返ること（空でも空でなくてもよい、エラーでないことを確認）
            // cmd.exe の初期プロンプトが含まれるはずなので空でないことを確認
            assert!(
                !content.is_empty(),
                "existing pane should return non-empty content after PTY output"
            );
            let capture = capture.expect("capture metadata should be present");
            assert_eq!(capture.cols, 80);
            assert_eq!(capture.rows, 24);
            assert_eq!(capture.lines_requested, 100);
            assert_eq!(capture.visible_text.len(), 24);
        })
        .await;
    }

    // TC-C14-03: working_dir 指定でペイン作成が成功する (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_create_pane_with_working_dir_succeeds() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            // C:\Users は Windows に必ず存在するパス
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size: TermSize { cols: 80, rows: 24 },
                working_dir: Some("C:\\Users".to_string()),
            })
            .await
            .unwrap();
            // PaneCreated が返ること（エラーでないこと）を確認
            let result = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match recv_one(&mut rx).await {
                        ServerMessage::PaneCreated { id, .. } => return Ok(id),
                        ServerMessage::Error { message } => return Err(message),
                        _ => continue,
                    }
                }
            })
            .await
            .expect("timeout");
            assert!(
                result.is_ok(),
                "CreatePane with valid working_dir should succeed, got error: {:?}",
                result.err()
            );
        })
        .await;
    }

    // TC-C14-04: 存在しないパスを working_dir に指定すると Error が返る (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_create_pane_with_invalid_working_dir_returns_error() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::CreateWorkspace { name: None })
                .await
                .unwrap();
            let ws_id = match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreateSurface { workspace: ws_id })
                .await
                .unwrap();
            let surf_id = match recv_one(&mut rx).await {
                ServerMessage::SurfaceCreated { id, .. } => id,
                _ => panic!(),
            };
            tx.send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: None,
                direction: None,
                size: TermSize { cols: 80, rows: 24 },
                working_dir: Some("Z:\\nonexistent_path_xyzzy_yatamux_test".to_string()),
            })
            .await
            .unwrap();
            // Error が返ること（PaneCreated が来ないこと）を確認
            let got_error = tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match recv_one(&mut rx).await {
                        ServerMessage::Error { .. } => return true,
                        ServerMessage::PaneCreated { .. } => return false,
                        _ => continue,
                    }
                }
            })
            .await
            .expect("timeout waiting for Error or PaneCreated");
            assert!(
                got_error,
                "CreatePane with non-existent working_dir should return Error"
            );
        })
        .await;
    }

    // F-4: Detach 後もサーバーが応答する
    #[tokio::test]
    async fn test_server_continues_after_detach() {
        with_timeout(async {
            let (tx, mut rx) = start_server();
            tx.send(ClientMessage::Detach).await.unwrap();
            tx.send(ClientMessage::CreateWorkspace {
                name: Some("after-detach".to_string()),
            })
            .await
            .unwrap();
            match recv_one(&mut rx).await {
                ServerMessage::WorkspaceCreated { name, .. } => {
                    assert_eq!(name, "after-detach");
                }
                other => panic!("unexpected after detach: {:?}", other),
            }
        })
        .await;
    }
}
