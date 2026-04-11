//! アプリケーション起動ロジック
//!
//! サーバーとクライアントを同一プロセス内で起動する。
//! GUI ↔ サーバー間は [`tokio::sync::mpsc`] チャネルで直結する（IPC オーバーヘッドなし）。
//!
//! また、外部プロセス（CLI・エージェント等）からペイン操作を受け付けるため、
//! Windows 名前付きパイプ IPC サーバー（`\\.\pipe\yatamux-{session}`）を常時起動する。
//! 外部からの入力は merged チャネルでインプロセスの入力と合流し、
//! サーバー出力はファンアウトタスクが GUI と IPC 両方に配信する。

mod bootstrap;
mod bridge;
mod layout_restore;
mod layout_switch;

use std::collections::HashMap;
use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use tokio::sync::mpsc;

use yatamux_client::{
    run_window, AlertingBackend, FocusAwareBackend, NotificationBackend, PaneStore, Theme,
};
use yatamux_protocol::types::{PaneId, SplitDirection, TermSize};
use yatamux_protocol::ClientMessage;
use yatamux_terminal::TerminalSink;

/// RSS フィードから見出しを取得して ` ◆ ` で連結した文字列を返す
async fn fetch_rss_headlines(url: &str) -> anyhow::Result<String> {
    let body = reqwest::get(url).await?.text().await?;
    let titles: Vec<String> = body
        .split("<title>")
        .skip(2) // 最初の <title> はフィード全体のタイトル
        .filter_map(|s| {
            let end = s.find("</title>")?;
            let raw = &s[..end];
            // CDATA アンラップ
            let inner = raw
                .trim()
                .strip_prefix("<![CDATA[")
                .and_then(|s| s.strip_suffix("]]>"))
                .unwrap_or(raw)
                .trim();
            if inner.is_empty() {
                None
            } else {
                Some(inner.to_string())
            }
        })
        .collect();
    Ok(titles.join("  ◆  "))
}

use crate::app::{
    bootstrap::bootstrap_runtime,
    bridge::{
        spawn_bridge_fanout, spawn_server_bridge, sync_pane_state, BridgeChannels, ServerBridge,
    },
    layout_restore::load_initial_layout,
};
use crate::config::{parse_hex_color, AppConfig, AppearanceConfig};

/// `AppearanceConfig` から `Theme` を構築する
fn build_theme(appearance: &AppearanceConfig) -> Theme {
    let parse = |s: &Option<String>| -> Option<u32> {
        s.as_deref()
            .and_then(parse_hex_color)
            .map(|(r, g, b)| (r as u32) << 16 | (g as u32) << 8 | b as u32)
    };
    Theme {
        bg: parse(&appearance.background),
        fg: parse(&appearance.foreground),
        cursor: parse(&appearance.cursor),
        selection_bg: parse(&appearance.selection_bg),
        status_bar_bg: parse(&appearance.status_bar_bg),
        font_family: appearance.font_family.clone(),
        font_size: appearance.font_size,
        alert_border: parse(&appearance.alert_border),
    }
}

/// デフォルトのターミナルサイズ
///
/// 実際の表示サイズは起動後の WM_SIZE によって即座に上書きされる。
/// ここでは VT100 標準の 80×24 を使用し、PTY・readline が初期化時に
/// 極端に広い幅を持たないようにする（折り返し描画ずれの防止）。
const DEFAULT_COLS: u16 = 80;
const DEFAULT_ROWS: u16 = 24;

#[cfg(test)]
pub(crate) fn appdata_env_lock() -> &'static Mutex<()> {
    use std::sync::OnceLock;

    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

/// アプリを起動する
///
/// `layout_name` が `Some` の場合、`%APPDATA%\yatamux\layouts\<name>.toml` を読み込み
/// 宣言的レイアウトで起動する。`None` の場合はセッション復元を試みる。
pub async fn run(
    session_name: String,
    layout_name: Option<String>,
    app_config: AppConfig,
) -> Result<()> {
    std::env::set_var("YATAMUX_SESSION", &session_name);

    let size = TermSize {
        cols: DEFAULT_COLS,
        rows: DEFAULT_ROWS,
    };
    let bootstrap = bootstrap_runtime(&session_name, size).await?;
    let client_tx = bootstrap.client_tx;
    let mut server_rx = bootstrap.server_rx;
    let ipc_out_tx = bootstrap.ipc_out_tx;
    let surf_id = bootstrap.surf_id;
    let pane_id = bootstrap.pane_id;

    tracing::info!("Pane {:?} created, opening window", pane_id);

    let (
        layout,
        sinks_vec,
        active_pane,
        initial_pane_commands,
        initial_pane_aliases,
        initial_pane_roles,
    ) = load_initial_layout(
        layout_name,
        pane_id,
        surf_id,
        size,
        &client_tx,
        &mut server_rx,
    )
    .await?;

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
        store.pane_commands = initial_pane_commands;
        store.pane_aliases = initial_pane_aliases;
        store.pane_roles = initial_pane_roles;
        Arc::new(Mutex::new(store))
    };

    // ── 通知バックエンド（フォーカス状態に応じて切り替え + ボーダーアラート）──
    let app_focused = Arc::new(AtomicBool::new(true));
    let (focus_backend, native_notif_queue) =
        FocusAwareBackend::new(Arc::clone(&app_focused), Arc::clone(&pane_store));
    let notif_backend: Arc<dyn NotificationBackend> =
        Arc::new(AlertingBackend::new(Arc::clone(&pane_store), focus_backend));

    // ── 入力・リサイズ チャネル（Window → Server）───────────────────────
    let (msg_tx, mut msg_rx) = mpsc::channel::<ClientMessage>(64);
    let client_tx2 = client_tx.clone();
    tokio::spawn(async move {
        while let Some(msg) = msg_rx.recv().await {
            let _ = client_tx2.send(msg).await;
        }
    });

    // ── ペイン分割要求チャネル（Window → この tokio タスク）────────────
    let (split_tx, split_rx) = mpsc::channel::<(PaneId, SplitDirection)>(8);

    // ── フローティングペイン要求チャネル（Window → この tokio タスク）──
    let (float_tx, float_rx) = mpsc::channel::<()>(4);

    // ── レイアウトランチャー切り替えチャネル（Window → この tokio タスク）
    let (layout_tx, layout_rx) = mpsc::channel::<String>(4);

    let hooks = app_config.hooks;
    let theme = build_theme(&app_config.appearance);
    let bridge_rx = spawn_bridge_fanout(server_rx, ipc_out_tx);
    let state_sync_tx = client_tx.clone();
    spawn_server_bridge(
        ServerBridge {
            server_rx: bridge_rx,
            client_tx,
            surf_id,
            size,
            pane_store: Arc::clone(&pane_store),
            notif_backend: Arc::clone(&notif_backend),
            hooks,
            sinks,
        },
        BridgeChannels {
            split_rx,
            float_rx,
            layout_rx,
        },
    );
    sync_pane_state(&state_sync_tx, &pane_store).await;

    let news_scroll_px_per_tick = app_config.status_bar.news_scroll_px_per_tick;

    // ── ニュースティッカー取得タスク ────────────────────────────────────
    if let Some(rss_url) = app_config.status_bar.news_rss.clone() {
        let interval_secs = app_config.status_bar.news_interval_secs;
        let store_for_news = Arc::clone(&pane_store);
        tokio::spawn(async move {
            loop {
                if let Ok(text) = fetch_rss_headlines(&rss_url).await {
                    store_for_news.lock().unwrap().news_text = text;
                }
                tokio::time::sleep(tokio::time::Duration::from_secs(interval_secs)).await;
            }
        });
    }

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
            layout_tx,
            theme,
            news_scroll_px_per_tick,
            env!("CARGO_PKG_VERSION"),
        )
    })
    .await??;

    Ok(())
}
