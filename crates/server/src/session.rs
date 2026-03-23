//! サーバーセッション管理
//!
//! Workspace → Surface → Pane の階層を管理する。
//! cmux のワークフローモデルに対応。

use std::collections::HashMap;
use tokio::sync::mpsc;
use anyhow::{Context, Result};
use tracing::info;

use yatamux_protocol::types::{PaneId, SplitDirection, SurfaceId, WorkspaceId};
use yatamux_protocol::{ClientMessage, ServerMessage};
use yatamux_terminal::CjkWidthConfig;

use crate::pane::Pane;

/// ペインの分割ツリーノード（二分木）
pub enum PaneTree {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<PaneTree>,
        second: Box<PaneTree>,
    },
}

/// サーフェス（タブ）
pub struct Surface {
    pub id: SurfaceId,
    pub workspace: WorkspaceId,
    pub pane_tree: Option<PaneTree>,
    pub active_pane: Option<PaneId>,
}

/// ワークスペース（セッション相当）
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub surfaces: Vec<SurfaceId>,
    pub active_surface: Option<SurfaceId>,
}

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
    pane_output_rx: mpsc::Receiver<(PaneId, Vec<u8>)>,
    pane_output_tx: mpsc::Sender<(PaneId, Vec<u8>)>,
}

impl Server {
    pub fn new(client_tx: mpsc::Sender<ServerMessage>) -> Self {
        let (pane_output_tx, pane_output_rx) = mpsc::channel(1024);
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
            }
        }
    }

    async fn handle_client_message(&mut self, msg: ClientMessage) -> Result<()> {
        match msg {
            ClientMessage::CreateWorkspace { name } => {
                let id = WorkspaceId(self.next_workspace_id);
                self.next_workspace_id += 1;
                let name = name.unwrap_or_else(|| format!("workspace-{}", id.0));
                info!("Creating workspace {:?} '{}'", id, name);
                self.workspaces.insert(id, Workspace {
                    id,
                    name: name.clone(),
                    surfaces: Vec::new(),
                    active_surface: None,
                });
                self.client_tx.send(ServerMessage::WorkspaceCreated { id, name }).await
                    .context("Failed to send WorkspaceCreated")?;
            }

            ClientMessage::CreateSurface { workspace } => {
                let id = SurfaceId(self.next_surface_id);
                self.next_surface_id += 1;
                info!("Creating surface {:?} in workspace {:?}", id, workspace);
                if let Some(ws) = self.workspaces.get_mut(&workspace) {
                    ws.surfaces.push(id);
                    if ws.active_surface.is_none() {
                        ws.active_surface = Some(id);
                    }
                }
                self.surfaces.insert(id, Surface {
                    id,
                    workspace,
                    pane_tree: None,
                    active_pane: None,
                });
                self.client_tx.send(ServerMessage::SurfaceCreated { id, workspace }).await
                    .context("Failed to send SurfaceCreated")?;
            }

            ClientMessage::CreatePane { surface, size, split_from, direction } => {
                let id = PaneId(self.next_pane_id);
                self.next_pane_id += 1;
                info!("Creating pane {:?} in surface {:?}", id, surface);

                let pane = Pane::spawn(
                    id,
                    size,
                    self.width_config.clone(),
                    self.pane_output_tx.clone(),
                )?;
                self.panes.insert(id, pane);

                if let Some(s) = self.surfaces.get_mut(&surface) {
                    match (split_from, direction, s.pane_tree.take()) {
                        (Some(parent_id), Some(dir), Some(tree)) => {
                            s.pane_tree = Some(split_pane_tree(tree, parent_id, id, dir));
                        }
                        (_, _, existing) => {
                            s.pane_tree = Some(existing.unwrap_or(PaneTree::Leaf(id)));
                            if s.pane_tree.as_ref().map_or(true, |t| matches!(t, PaneTree::Leaf(_))) {
                                s.pane_tree = Some(PaneTree::Leaf(id));
                            }
                        }
                    }
                    s.active_pane = Some(id);
                }

                self.client_tx.send(ServerMessage::PaneCreated { id, surface }).await
                    .context("Failed to send PaneCreated")?;
            }

            ClientMessage::Input { pane, data } => {
                if let Some(p) = self.panes.get(&pane) {
                    p.send_input(data).await?;
                }
            }

            ClientMessage::Resize { pane, size } => {
                if let Some(p) = self.panes.get(&pane) {
                    p.resize(size).await?;
                }
            }

            ClientMessage::ClosePane { pane } => {
                self.panes.remove(&pane);
                self.client_tx.send(ServerMessage::PaneClosed { pane }).await?;
            }

            ClientMessage::Detach => {
                info!("Client detached, server continues running");
            }

            ClientMessage::RequestScreen { pane: _ } => {
                // TODO: グリッドの現在状態を送信
            }
        }
        Ok(())
    }
}

/// `parent` の Leaf を `parent`/`child` の Split に置き換えるヘルパー
fn split_pane_tree(tree: PaneTree, parent: PaneId, child: PaneId, dir: SplitDirection) -> PaneTree {
    match tree {
        PaneTree::Leaf(id) if id == parent => PaneTree::Split {
            direction: dir,
            ratio: 0.5,
            first: Box::new(PaneTree::Leaf(id)),
            second: Box::new(PaneTree::Leaf(child)),
        },
        PaneTree::Split { direction, ratio, first, second } => PaneTree::Split {
            direction,
            ratio,
            first: Box::new(split_pane_tree(*first, parent, child, dir)),
            second: Box::new(split_pane_tree(*second, parent, child, dir)),
        },
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yatamux_protocol::{ClientMessage, ServerMessage};
    use yatamux_protocol::types::{SurfaceId, TermSize, WorkspaceId};
    use std::time::Duration;
    use tokio::sync::mpsc;

    /// テスト用サーバーを起動し (client_tx, server_rx) を返す
    fn start_server() -> (mpsc::Sender<ClientMessage>, mpsc::Receiver<ServerMessage>) {
        let (server_msg_tx, server_msg_rx) = mpsc::channel::<ServerMessage>(64);
        let (client_msg_tx, client_msg_rx) = mpsc::channel::<ClientMessage>(64);
        let server = Server::new(server_msg_tx);
        tokio::spawn(server.run(client_msg_rx));
        (client_msg_tx, server_msg_rx)
    }

    /// 1 秒タイムアウト付きで次のメッセージを受信する
    async fn recv_one(rx: &mut mpsc::Receiver<ServerMessage>) -> ServerMessage {
        tokio::time::timeout(Duration::from_secs(1), rx.recv())
            .await
            .expect("timeout waiting for server message")
            .expect("server channel closed")
    }

    // G-1: CreateWorkspace → WorkspaceCreated が返る
    #[tokio::test]
    async fn test_create_workspace() {
        let (tx, mut rx) = start_server();
        tx.send(ClientMessage::CreateWorkspace { name: Some("ws1".to_string()) })
            .await
            .unwrap();
        match recv_one(&mut rx).await {
            ServerMessage::WorkspaceCreated { id, name } => {
                assert_eq!(id, WorkspaceId(1));
                assert_eq!(name, "ws1");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // G-1: 名前なしワークスペースも作成できる（自動命名）
    #[tokio::test]
    async fn test_create_workspace_auto_name() {
        let (tx, mut rx) = start_server();
        tx.send(ClientMessage::CreateWorkspace { name: None }).await.unwrap();
        match recv_one(&mut rx).await {
            ServerMessage::WorkspaceCreated { name, .. } => {
                assert!(!name.is_empty(), "auto name should not be empty");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // G-2: CreateSurface → SurfaceCreated が返り WorkspaceId と紐づく
    #[tokio::test]
    async fn test_create_surface() {
        let (tx, mut rx) = start_server();
        tx.send(ClientMessage::CreateWorkspace { name: None }).await.unwrap();
        let ws_id = match recv_one(&mut rx).await {
            ServerMessage::WorkspaceCreated { id, .. } => id,
            other => panic!("unexpected: {:?}", other),
        };
        tx.send(ClientMessage::CreateSurface { workspace: ws_id }).await.unwrap();
        match recv_one(&mut rx).await {
            ServerMessage::SurfaceCreated { id, workspace } => {
                assert_eq!(id, SurfaceId(1));
                assert_eq!(workspace, ws_id);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // G-3: CreatePane → PaneCreated が返る (Windows のみ: PTY spawn が必要)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_create_pane() {
        let (tx, mut rx) = start_server();
        tx.send(ClientMessage::CreateWorkspace { name: None }).await.unwrap();
        let ws_id = match recv_one(&mut rx).await {
            ServerMessage::WorkspaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreateSurface { workspace: ws_id }).await.unwrap();
        let surf_id = match recv_one(&mut rx).await {
            ServerMessage::SurfaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
        })
        .await
        .unwrap();
        match recv_one(&mut rx).await {
            ServerMessage::PaneCreated { id, surface } => {
                assert_eq!(surface, surf_id);
                assert_eq!(id, yatamux_protocol::types::PaneId(1));
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // G-5: PTY 出力がクライアントに Output メッセージとして届く (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_pane_output_forwarded_to_client() {
        let (tx, mut rx) = start_server();
        tx.send(ClientMessage::CreateWorkspace { name: None }).await.unwrap();
        let ws_id = match recv_one(&mut rx).await {
            ServerMessage::WorkspaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreateSurface { workspace: ws_id }).await.unwrap();
        let surf_id = match recv_one(&mut rx).await {
            ServerMessage::SurfaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
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
    }

    // G-4: Input メッセージがペインに届く（Error が返らない）(Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_input_routed_to_pane_without_error() {
        let (tx, mut rx) = start_server();
        tx.send(ClientMessage::CreateWorkspace { name: None }).await.unwrap();
        let ws_id = match recv_one(&mut rx).await {
            ServerMessage::WorkspaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreateSurface { workspace: ws_id }).await.unwrap();
        let surf_id = match recv_one(&mut rx).await {
            ServerMessage::SurfaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
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
        tx.send(ClientMessage::Input { pane: pane_id, data: b"echo test_input\r".to_vec() })
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
        assert!(got_error.is_err(), "no error should be received after Input");
    }

    // G-6: ClosePane → PaneClosed が返る (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_close_pane() {
        let (tx, mut rx) = start_server();
        tx.send(ClientMessage::CreateWorkspace { name: None }).await.unwrap();
        let ws_id = match recv_one(&mut rx).await {
            ServerMessage::WorkspaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreateSurface { workspace: ws_id }).await.unwrap();
        let surf_id = match recv_one(&mut rx).await {
            ServerMessage::SurfaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
        })
        .await
        .unwrap();
        let pane_id = match recv_one(&mut rx).await {
            ServerMessage::PaneCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::ClosePane { pane: pane_id }).await.unwrap();
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
    }

    // G-7: Resize メッセージでエラーが返らない (Windows のみ)
    #[cfg(windows)]
    #[tokio::test]
    async fn test_resize_pane() {
        let (tx, mut rx) = start_server();
        tx.send(ClientMessage::CreateWorkspace { name: None }).await.unwrap();
        let ws_id = match recv_one(&mut rx).await {
            ServerMessage::WorkspaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreateSurface { workspace: ws_id }).await.unwrap();
        let surf_id = match recv_one(&mut rx).await {
            ServerMessage::SurfaceCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
        })
        .await
        .unwrap();
        let pane_id = match recv_one(&mut rx).await {
            ServerMessage::PaneCreated { id, .. } => id,
            _ => panic!(),
        };
        tx.send(ClientMessage::Resize {
            pane: pane_id,
            size: TermSize { cols: 120, rows: 40 },
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
    }

    // F-4: Detach 後もサーバーが応答する
    #[tokio::test]
    async fn test_server_continues_after_detach() {
        let (tx, mut rx) = start_server();
        tx.send(ClientMessage::Detach).await.unwrap();
        tx.send(ClientMessage::CreateWorkspace { name: Some("after-detach".to_string()) })
            .await
            .unwrap();
        match recv_one(&mut rx).await {
            ServerMessage::WorkspaceCreated { name, .. } => {
                assert_eq!(name, "after-detach");
            }
            other => panic!("unexpected after detach: {:?}", other),
        }
    }
}
