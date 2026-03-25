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
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::mpsc;

use yatamux_client::{
    run_window, FocusAwareBackend, LayoutNode, LayoutNodeDef, LayoutSnapshot, NotificationBackend,
    PaneStore,
};
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
    let size = TermSize {
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    };

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
    client_tx
        .send(ClientMessage::CreateWorkspace { name: None })
        .await?;
    let ws_id = wait_for!(server_rx, ServerMessage::WorkspaceCreated { id, .. } => id)?;

    client_tx
        .send(ClientMessage::CreateSurface { workspace: ws_id })
        .await?;
    let surf_id = wait_for!(server_rx, ServerMessage::SurfaceCreated { id, .. } => id)?;

    client_tx
        .send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: None,
            direction: None,
            size,
        })
        .await?;
    let pane_id = wait_for!(server_rx, ServerMessage::PaneCreated { id, .. } => id)?;

    tracing::info!("Pane {:?} created, opening window", pane_id);

    // ── セッション復元 or 初期ペインのみ ────────────────────────────────
    let session_path = LayoutSnapshot::default_path();
    let (layout, sinks_vec, active_pane) = if let Ok(snap) = LayoutSnapshot::load(&session_path) {
        tracing::info!("セッションを復元します");
        let mut old_to_new: HashMap<PaneId, PaneId> = HashMap::new();
        let (layout, sinks_vec) = restore_node(
            &snap.root,
            pane_id,
            surf_id,
            size,
            &client_tx,
            &mut server_rx,
            &mut old_to_new,
        )
        .await?;
        let active = old_to_new.get(&snap.active).copied().unwrap_or(pane_id);
        (layout, sinks_vec, active)
    } else {
        let sink = TerminalSink::new(size.cols, size.rows);
        (LayoutNode::Leaf(pane_id), vec![(pane_id, sink)], pane_id)
    };

    let mut sinks: HashMap<PaneId, TerminalSink> = HashMap::new();
    let mut all_grids: HashMap<PaneId, Arc<Mutex<yatamux_terminal::Grid>>> = HashMap::new();
    for (id, sink) in sinks_vec {
        all_grids.insert(id, Arc::clone(&sink.grid));
        sinks.insert(id, sink);
    }

    let pane_store = {
        let mut store = PaneStore::new(pane_id, all_grids[&pane_id].clone());
        store.layout = layout;
        store.grids = all_grids;
        store.active = active_pane;
        Arc::new(Mutex::new(store))
    };

    // ── 通知バックエンド（フォーカス状態に応じて切り替え）────────────────
    let app_focused = Arc::new(AtomicBool::new(true));
    let (notif_backend, native_notif_queue) =
        FocusAwareBackend::new(Arc::clone(&app_focused), Arc::clone(&pane_store));
    let notif_backend: Arc<dyn NotificationBackend> = Arc::new(notif_backend);

    // ── 入力・リサイズ チャネル（Window → Server）───────────────────────
    let (msg_tx, mut msg_rx) = mpsc::channel::<ClientMessage>(64);
    let client_tx2 = client_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = msg_rx.recv().await {
            let _ = client_tx2.send(msg).await;
        }
    });

    // ── ペイン分割要求チャネル（Window → この tokio タスク）────────────
    let (split_tx, mut split_rx) = mpsc::channel::<(PaneId, SplitDirection)>(8);

    // ── フローティングペイン要求チャネル（Window → この tokio タスク）──
    let (float_tx, mut float_rx) = mpsc::channel::<()>(4);

    // ── サーバー出力 + ペイン分割ハンドラ ───────────────────────────────
    let pane_store2 = Arc::clone(&pane_store);
    let notif_backend2 = Arc::clone(&notif_backend);
    tokio::spawn(async move {
        let notif_backend = notif_backend2;
        // 分割リクエスト待ちキュー (parent_id, direction, new_size)
        let mut pending: VecDeque<(PaneId, SplitDirection, TermSize)> = VecDeque::new();
        // 次の PaneCreated がフローティングペイン用かどうか
        let mut pending_float = false;

        loop {
            tokio::select! {
                biased;

                // フローティングペイン要求
                Some(()) = float_rx.recv() => {
                    let floating = pane_store2.lock().unwrap().floating;
                    match floating {
                        None => {
                            // 初回: 新しいペインを作成してフローティングに設定
                            pending_float = true;
                            let _ = client_tx.send(ClientMessage::CreatePane {
                                surface: surf_id,
                                split_from: None,
                                direction: None,
                                size,
                            }).await;
                        }
                        Some(_) => {
                            // 既存フローティングペインの表示/非表示トグル
                            let mut store = pane_store2.lock().unwrap();
                            if store.floating_visible {
                                store.hide_float();
                            } else {
                                store.show_float();
                            }
                        }
                    }
                }

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
                                // feed() は OSC 52 クリップボードデータがあれば Some を返す
                                if let Some(clip) = sink.feed(&data) {
                                    pane_store2.lock().unwrap().pending_clipboard = Some(clip);
                                }
                            }
                        }
                        ServerMessage::PaneCreated { id: new_id, .. } => {
                            if pending_float {
                                // フローティングペイン作成完了
                                pending_float = false;
                                let float_size = TermSize { cols: DEFAULT_COLS, rows: DEFAULT_ROWS };
                                let new_sink = TerminalSink::new(float_size.cols, float_size.rows);
                                let new_grid = Arc::clone(&new_sink.grid);
                                sinks.insert(new_id, new_sink);
                                {
                                    let mut store = pane_store2.lock().unwrap();
                                    store.grids.insert(new_id, new_grid);
                                    // レイアウトツリーには追加しない（フローティング管理）
                                    store.floating = Some(new_id);
                                    store.show_float();
                                }
                            } else if let Some((parent_id, direction, new_size)) = pending.pop_front() {
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
                            let mut store = pane_store2.lock().unwrap();
                            store.grids.remove(&pane);
                            let next = store.layout.remove_pane(pane);
                            if store.active == pane {
                                if let Some(next_id) = next {
                                    store.active = next_id;
                                }
                            }
                        }
                        ServerMessage::Notification { pane, body } => {
                            let active = pane_store2.lock().unwrap().active;
                            if pane != active {
                                notif_backend.notify(pane, body);
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
    });

    // ── Win32 ウィンドウ（spawn_blocking でメッセージループ実行）────────
    tokio::task::spawn_blocking(move || {
        run_window(
            pane_store,
            msg_tx,
            split_tx,
            size,
            app_focused,
            native_notif_queue,
            float_tx,
        )
    })
    .await??;

    Ok(())
}

/// 保存済みレイアウトを再帰的に再構築する
///
/// `def` のツリー構造に従い `CreatePane` を発行し、
/// 新しい `LayoutNode` と各ペインの `TerminalSink` を返す。
/// `old_to_new` に旧ペイン ID → 新ペイン ID のマッピングを蓄積する。
async fn restore_node(
    def: &LayoutNodeDef,
    current_pane: PaneId,
    surf_id: yatamux_protocol::types::SurfaceId,
    size: TermSize,
    client_tx: &mpsc::Sender<ClientMessage>,
    server_rx: &mut mpsc::Receiver<ServerMessage>,
    old_to_new: &mut HashMap<PaneId, PaneId>,
) -> Result<(LayoutNode, Vec<(PaneId, TerminalSink)>)> {
    match def {
        LayoutNodeDef::Leaf { id: old_id } => {
            old_to_new.insert(*old_id, current_pane);
            let sink = TerminalSink::new(size.cols, size.rows);
            Ok((LayoutNode::Leaf(current_pane), vec![(current_pane, sink)]))
        }
        LayoutNodeDef::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            client_tx
                .send(ClientMessage::CreatePane {
                    surface: surf_id,
                    split_from: Some(current_pane),
                    direction: Some(*direction),
                    size,
                })
                .await?;
            let new_pane = wait_for!(server_rx, ServerMessage::PaneCreated { id, .. } => id)?;

            let (first_layout, mut all_sinks) = Box::pin(restore_node(
                first,
                current_pane,
                surf_id,
                size,
                client_tx,
                server_rx,
                old_to_new,
            ))
            .await?;
            let (second_layout, second_sinks) = Box::pin(restore_node(
                second, new_pane, surf_id, size, client_tx, server_rx, old_to_new,
            ))
            .await?;
            all_sinks.extend(second_sinks);

            Ok((
                LayoutNode::Split {
                    direction: *direction,
                    ratio: *ratio,
                    first: Box::new(first_layout),
                    second: Box::new(second_layout),
                },
                all_sinks,
            ))
        }
    }
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
