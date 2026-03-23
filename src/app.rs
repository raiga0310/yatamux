//! アプリケーション起動ロジック
//!
//! サーバーとクライアントを同一プロセス内で起動する。
//! 名前付きパイプ IPC を経由せず、mpsc チャネルで直結する。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::mpsc;

use yatamux_client::{run_window, PaneStore};
use yatamux_protocol::types::{PaneId, SplitDirection, TermSize};
use yatamux_protocol::{ClientMessage, ServerMessage};
use yatamux_server::Server;
use yatamux_terminal::TerminalSink;

/// デフォルトのターミナルサイズ
const DEFAULT_COLS: u16 = 220;
const DEFAULT_ROWS: u16 = 50;

/// アプリを起動する
pub async fn run() -> Result<()> {
    let size = TermSize { cols: DEFAULT_COLS, rows: DEFAULT_ROWS };

    // ── サーバーをインプロセスで起動 ────────────────────────────────────
    let (client_tx, client_rx) = mpsc::channel::<ClientMessage>(256);
    let (server_tx, mut server_rx) = mpsc::channel::<ServerMessage>(256);

    let server = Server::new(server_tx);
    tokio::spawn(server.run(client_rx));

    // ── ワークスペース → サーフェス → 初期ペイン 作成 ───────────────────
    client_tx.send(ClientMessage::CreateWorkspace { name: None }).await?;
    let ws_id = wait_for!(server_rx, ServerMessage::WorkspaceCreated { id, .. } => id)?;

    client_tx.send(ClientMessage::CreateSurface { workspace: ws_id }).await?;
    let surf_id = wait_for!(server_rx, ServerMessage::SurfaceCreated { id, .. } => id)?;

    client_tx.send(ClientMessage::CreatePane {
        surface: surf_id,
        split_from: None,
        direction: None,
        size,
    }).await?;
    let pane_id = wait_for!(server_rx, ServerMessage::PaneCreated { id, .. } => id)?;

    tracing::info!("Pane {:?} created, opening window", pane_id);

    // ── 初期ペインの TerminalSink とペインストアを作成 ───────────────────
    let mut sinks: HashMap<PaneId, TerminalSink> = HashMap::new();
    {
        let sink = TerminalSink::new(size.cols, size.rows);
        sinks.insert(pane_id, sink);
    }
    let initial_grid = Arc::clone(&sinks[&pane_id].grid);
    let pane_store = Arc::new(Mutex::new(PaneStore::new(pane_id, initial_grid)));

    // ── 入力・リサイズ チャネル（Window → Server）───────────────────────
    let (msg_tx, mut msg_rx) = mpsc::channel::<ClientMessage>(64);
    let client_tx2 = client_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = msg_rx.recv().await {
            let _ = client_tx2.send(msg).await;
        }
    });

    // ── ペイン分割要求チャネル（Window → この tokio タスク）────────────
    let (split_tx, mut split_rx) =
        mpsc::channel::<(PaneId, SplitDirection)>(8);

    // ── サーバー出力 + ペイン分割ハンドラ ───────────────────────────────
    let pane_store2 = Arc::clone(&pane_store);
    tokio::spawn(async move {
        // 分割リクエスト待ちキュー (parent_id, direction, new_size)
        let mut pending: VecDeque<(PaneId, SplitDirection, TermSize)> = VecDeque::new();

        loop {
            tokio::select! {
                biased;

                // ペイン分割要求
                Some((parent_id, direction)) = split_rx.recv() => {
                    // 親グリッドの現在サイズから新ペインのサイズを計算
                    let new_size = {
                        let store = pane_store2.lock().unwrap();
                        if let Some(g) = store.grids.get(&parent_id) {
                            let g = g.lock().unwrap();
                            match direction {
                                SplitDirection::Vertical =>
                                    TermSize { cols: (g.cols() / 2).max(1), rows: g.rows() },
                                SplitDirection::Horizontal =>
                                    TermSize { cols: g.cols(), rows: (g.rows() / 2).max(1) },
                            }
                        } else {
                            TermSize { cols: DEFAULT_COLS / 2, rows: DEFAULT_ROWS }
                        }
                    };
                    pending.push_back((parent_id, direction, new_size));
                    let _ = client_tx.send(ClientMessage::CreatePane {
                        surface: surf_id,
                        split_from: Some(parent_id),
                        direction: Some(direction),
                        size: new_size,
                    }).await;
                }

                // サーバーからのメッセージ
                Some(msg) = server_rx.recv() => {
                    match msg {
                        ServerMessage::Output { pane, data } => {
                            if let Some(sink) = sinks.get_mut(&pane) {
                                sink.feed(&data);
                            }
                        }
                        ServerMessage::PaneCreated { id: new_id, .. } => {
                            if let Some((parent_id, direction, new_size)) = pending.pop_front() {
                                let new_sink = TerminalSink::new(new_size.cols, new_size.rows);
                                let new_grid = Arc::clone(&new_sink.grid);
                                sinks.insert(new_id, new_sink);
                                let mut store = pane_store2.lock().unwrap();
                                store.grids.insert(new_id, new_grid);
                                store.layout.split_leaf(parent_id, new_id, direction);
                                store.active = new_id;
                            }
                        }
                        ServerMessage::PaneClosed { pane } => {
                            sinks.remove(&pane);
                            pane_store2.lock().unwrap().grids.remove(&pane);
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // ── Win32 ウィンドウ（spawn_blocking でメッセージループ実行）────────
    tokio::task::spawn_blocking(move || {
        run_window(pane_store, msg_tx, split_tx, size)
    })
    .await??;

    Ok(())
}

/// サーバーからの特定メッセージを待つマクロ
macro_rules! wait_for {
    ($rx:expr, $pat:pat => $val:expr) => {{
        loop {
            match $rx.recv().await {
                Some($pat) => break Ok($val),
                Some(ServerMessage::Error { message }) => {
                    break Err(anyhow::anyhow!("Server error: {}", message))
                }
                Some(_) => continue,
                None => break Err(anyhow::anyhow!("Server channel closed unexpectedly")),
            }
        }
    }};
}
use wait_for;
