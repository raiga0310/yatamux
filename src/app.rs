//! アプリケーション起動ロジック
//!
//! サーバーとクライアントを同一プロセス内で起動する。
//! GUI ↔ サーバー間は [`tokio::sync::mpsc`] チャネルで直結する（IPC オーバーヘッドなし）。
//!
//! また、外部プロセス（CLI・エージェント等）からペイン操作を受け付けるため、
//! Windows 名前付きパイプ IPC サーバー（`\\.\pipe\yatamux-{session}`）を常時起動する。
//! 外部からの入力は merged チャネルでインプロセスの入力と合流し、
//! サーバー出力はファンアウトタスクが GUI と IPC 両方に配信する。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::mpsc;

use yatamux_client::{run_window, PaneStore};
use yatamux_protocol::types::{PaneId, SplitDirection, TermSize};
use yatamux_protocol::{ClientMessage, ServerMessage};
use yatamux_server::{ipc::run_ipc_server, Server};
use yatamux_terminal::TerminalSink;

use crate::DEFAULT_SESSION;

/// デフォルトのターミナルサイズ
///
/// 実際の表示サイズは起動後の WM_SIZE によって即座に上書きされる。
/// ここでは VT100 標準の 80×24 を使用し、PTY・readline が初期化時に
/// 極端に広い幅を持たないようにする（折り返し描画ずれの防止）。
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

/// アプリを起動する
pub async fn run() -> Result<()> {
    let size = TermSize { cols: DEFAULT_COLS, rows: DEFAULT_ROWS };

    // ── サーバーをインプロセスで起動 ────────────────────────────────────
    let (server_tx, mut server_rx) = mpsc::channel::<ServerMessage>(256);

    // サーバー出力は window と IPC 両方へファンアウトする
    // Server は単一の server_out_tx へ出力 → fan_out タスクが振り分ける
    let (server_out_tx, mut server_out_rx) = mpsc::channel::<ServerMessage>(256);
    let (ipc_out_tx, ipc_out_rx) = mpsc::channel::<ServerMessage>(256);

    // 入力は window と IPC を merged_tx でマージ → server へ
    let (merged_tx, merged_rx) = mpsc::channel::<ClientMessage>(256);
    let client_tx = merged_tx.clone(); // window 用
    let ipc_in_tx = merged_tx.clone(); // IPC 用

    // server_out_rx → server_rx（window 用）と ipc_out_tx（IPC 用）へファンアウト
    tokio::spawn(async move {
        while let Some(msg) = server_out_rx.recv().await {
            let _ = server_tx.send(msg.clone()).await;
            let _ = ipc_out_tx.send(msg).await;
        }
    });

    let server = Server::new(server_out_tx);
    tokio::spawn(server.run(merged_rx));

    // ── IPC サーバー起動（外部 CLI からの接続を受け付ける）────────────────
    tokio::spawn(async move {
        if let Err(e) = run_ipc_server(DEFAULT_SESSION, ipc_in_tx, ipc_out_rx).await {
            tracing::error!("IPC server exited with error: {:#}", e);
        }
    });

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
                                {
                                    let mut store = pane_store2.lock().unwrap();
                                    // 親ペインのクライアント側グリッドもリサイズ
                                    // （分割後は幅/高さが半分になるため）
                                    if let Some(g) = store.grids.get(&parent_id) {
                                        g.lock().unwrap().resize(new_size.cols, new_size.rows);
                                    }
                                    store.grids.insert(new_id, new_grid);
                                    store.layout.split_leaf(parent_id, new_id, direction);
                                    store.active = new_id;
                                }
                                // 親ペインをサーバー側（ConPTY）でもリサイズ
                                let _ = client_tx.send(ClientMessage::Resize {
                                    pane: parent_id,
                                    size: new_size,
                                }).await;
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
