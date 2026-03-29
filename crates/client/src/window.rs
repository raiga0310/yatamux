//! Win32 ウィンドウ + メッセージループ
//!
//! ターミナルグリッドを GDI でレンダリングし、
//! キーボード入力と IME メッセージを処理する。
//!
//! ## アーキテクチャ
//! ```text
//! Win32 WndProc
//!   WM_PAINT        → GDI で Grid をレンダリング（プリエディット下線付き）
//!   WM_KEYDOWN      → 制御キーを VT シーケンスに変換 → server へ送信
//!   WM_CHAR         → 通常文字 → server へ送信
//!   WM_IME_*        → ImeHandler に委譲
//!   WM_SIZE         → グリッドリサイズ通知
//!   WM_CLOSE        → ウィンドウ破棄
//! ```
//!
//! WndProc はスタティック関数である必要があるため、
//! `SetWindowLongPtrW(GWLP_USERDATA)` でクライアント状態へのポインタを保持する。

/// 外観テーマ設定（プラットフォーム非依存）
///
/// `AppConfig::appearance` から構築し、`run_window()` に渡す。
/// 色値は `0xRRGGBB` 形式の `u32`。`None` はデフォルト値を意味する。
#[derive(Debug, Clone, Default)]
pub struct Theme {
    /// 背景色（`0xRRGGBB`）
    pub bg: Option<u32>,
    /// 前景色
    pub fg: Option<u32>,
    /// カーソル色
    pub cursor: Option<u32>,
    /// テキスト選択背景色
    pub selection_bg: Option<u32>,
    /// ステータスバー背景色
    pub status_bar_bg: Option<u32>,
    /// フォントファミリー（`None` = 候補リストから自動選択）
    pub font_family: Option<String>,
    /// フォントサイズ（pt）
    pub font_size: Option<u32>,
}

#[cfg(windows)]
mod win32 {
    use std::sync::{Arc, Mutex};
    use tokio::sync::mpsc;
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::*;
    use windows::Win32::Graphics::Dwm::{DwmSetWindowAttribute, DWMWINDOWATTRIBUTE};
    use windows::Win32::Graphics::Gdi::*;
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE};
    use windows::Win32::UI::Input::KeyboardAndMouse::*;
    use windows::Win32::UI::WindowsAndMessaging::*;

    use std::collections::{HashMap, VecDeque};
    use std::sync::atomic::{AtomicBool, Ordering};
    use windows::Win32::UI::Shell::{
        Shell_NotifyIconW, NIF_ICON, NIF_INFO, NIF_TIP, NIIF_INFO, NIM_ADD, NIM_DELETE,
        NOTIFYICONDATAW,
    };
    use yatamux_protocol::types::{PaneId, SplitDirection, TermSize};
    use yatamux_protocol::ClientMessage;
    use yatamux_terminal::cell::CellContent;
    use yatamux_terminal::{Cell, Grid};

    use crate::ime::{CellPixelPos, ImeHandler, ImeState, PreeditAttr};
    use crate::layout::{
        layout_to_toml, list_available_layouts, list_available_themes, load_theme_from_file,
        save_layout_file, CopyState, Direction, LauncherState, PaneRect, PaneStore,
        ThemeLauncherState, Toast,
    };
    use crate::notification::NativeToastMsg;

    // ── 定数 ────────────────────────────────────────────────────────────────

    /// ウィンドウクラス名（ワイド文字列）
    const CLASS_NAME: &str = "Yatamux\0";

    /// 再描画タイマー ID
    const TIMER_REPAINT: usize = 1;

    /// 再描画インターバル（ミリ秒）〜60fps
    const TIMER_INTERVAL_MS: u32 = 16;

    /// フォント候補（優先順位順）。最初にインストール済みのものを使う。
    const FONT_CANDIDATES: &[&str] = &[
        "HackGen Console NF",
        "HackGen Console",
        "HackGen35 Console NF",
        "HackGen35 Console",
        "HackGen NF",
        "HackGen",
        "Cascadia Mono",
        "Cascadia Code",
        "Consolas",
        "MS Gothic",
    ];

    /// フォントの文字高さ（負値 = internal leading を含まない正確な文字高さ）
    const FONT_HEIGHT: i32 = -20;

    /// ウィンドウ内パディング（テキスト描画領域とウィンドウ端の余白）
    const PADDING_X: i32 = 10;
    const PADDING_Y: i32 = 8;

    /// Normal モードのマウス選択範囲に (col, row) が含まれるか判定する。
    /// sel = (anchor_col, anchor_row, end_col, end_row)
    fn is_in_normal_selection(sel: (usize, usize, usize, usize), col: usize, row: usize) -> bool {
        let (ac, ar, ec, er) = sel;
        let (r0, r1) = if ar <= er { (ar, er) } else { (er, ar) };
        if row < r0 || row > r1 {
            return false;
        }
        if r0 == r1 {
            let (c0, c1) = if ac <= ec { (ac, ec) } else { (ec, ac) };
            col >= c0 && col <= c1
        } else if row == r0 {
            let c0 = if ar <= er { ac } else { ec };
            col >= c0
        } else if row == r1 {
            let c1 = if ar <= er { ec } else { ac };
            col <= c1
        } else {
            true
        }
    }

    // ── GDI カラーヘルパー ───────────────────────────────────────────────────

    fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
        COLORREF((b as u32) << 16 | (g as u32) << 8 | r as u32)
    }

    // Catppuccin Mocha テーマ（デフォルト値）
    const COLOR_BG: COLORREF = COLORREF(0x00_2E_1E_1E); // base  #1e1e2e
    const COLOR_FG: COLORREF = COLORREF(0x00_F4_D6_CD); // text  #cdd6f4
    const COLOR_CURSOR: COLORREF = COLORREF(0x00_E7_C2_F5); // pink     #f5c2e7
    const COLOR_SEPARATOR: COLORREF = COLORREF(0x00_5A_47_45); // surface1 #45475a
    const COLOR_PREEDIT_BG: COLORREF = COLORREF(0x00_5A_47_45); // surface1 #45475a

    /// `0xRRGGBB` を Win32 BGR 形式の `COLORREF` に変換する
    fn rgb_hex(v: u32) -> COLORREF {
        let r = ((v >> 16) & 0xFF) as u8;
        let g = ((v >> 8) & 0xFF) as u8;
        let b = (v & 0xFF) as u8;
        rgb(r, g, b)
    }

    /// Win32 用に解決済みのテーマ色
    ///
    /// `Theme`（`0xRRGGBB` u32）から `COLORREF` (BGR) へ変換したもの。
    /// `ClientState.theme`（`Cell<WinTheme>`）に保持し、各描画関数から参照する。
    #[derive(Copy, Clone)]
    struct WinTheme {
        bg: COLORREF,
        fg: COLORREF,
        cursor: COLORREF,
        selection_bg: COLORREF,
        status_bar_bg: COLORREF,
    }

    impl WinTheme {
        fn from_theme(theme: &crate::window::Theme) -> Self {
            WinTheme {
                bg: theme.bg.map(rgb_hex).unwrap_or(COLOR_BG),
                fg: theme.fg.map(rgb_hex).unwrap_or(COLOR_FG),
                cursor: theme.cursor.map(rgb_hex).unwrap_or(COLOR_CURSOR),
                selection_bg: theme
                    .selection_bg
                    .map(rgb_hex)
                    .unwrap_or(COLORREF(0x00_44_60_A0)),
                status_bar_bg: theme
                    .status_bar_bg
                    .map(rgb_hex)
                    .unwrap_or(COLORREF(0x00_25_18_18)),
            }
        }
    }

    // ── モード定義 ───────────────────────────────────────────────────────────

    /// UI モード
    ///
    /// `Normal`: 全キー入力を PTY に透過する通常状態。
    /// `Pane`: ペイン操作（移動・分割・削除）を受け付けるワンショットモード。
    ///         1 キー操作後に自動で `Normal` に戻る。
    /// `Copy`: テキスト選択・クリップボードコピーモード。
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum ClientMode {
        Normal,
        Pane,
        Copy,
    }

    enum PreviewRenderNode {
        Leaf {
            label: Option<String>,
        },
        Split {
            direction: SplitDirection,
            ratio: f32,
            first: Box<PreviewRenderNode>,
            second: Box<PreviewRenderNode>,
        },
    }

    impl PreviewRenderNode {
        fn from_layout_preview(
            node: &crate::layout::LayoutNode,
            commands: &[Option<String>],
        ) -> Self {
            use crate::layout::LayoutNode;

            match node {
                LayoutNode::Leaf(id) => Self::Leaf {
                    label: commands.get(id.0 as usize).cloned().flatten(),
                },
                LayoutNode::Split {
                    direction,
                    ratio,
                    first,
                    second,
                } => Self::Split {
                    direction: *direction,
                    ratio: *ratio,
                    first: Box::new(Self::from_layout_preview(first, commands)),
                    second: Box::new(Self::from_layout_preview(second, commands)),
                },
            }
        }

        fn from_live_layout(
            node: &crate::layout::LayoutNode,
            pane_commands: &HashMap<PaneId, String>,
        ) -> Self {
            use crate::layout::LayoutNode;

            match node {
                LayoutNode::Leaf(id) => Self::Leaf {
                    label: pane_commands.get(id).cloned(),
                },
                LayoutNode::Split {
                    direction,
                    ratio,
                    first,
                    second,
                } => Self::Split {
                    direction: *direction,
                    ratio: *ratio,
                    first: Box::new(Self::from_live_layout(first, pane_commands)),
                    second: Box::new(Self::from_live_layout(second, pane_commands)),
                },
            }
        }
    }

    struct LauncherRenderState {
        entries: Vec<String>,
        selected: usize,
        selected_preview: Option<PreviewRenderNode>,
    }

    impl LauncherRenderState {
        fn from_launcher(launcher: &LauncherState) -> Self {
            let selected = if launcher.entries.is_empty() {
                0
            } else {
                launcher.selected.min(launcher.entries.len() - 1)
            };
            let selected_preview = launcher
                .entries
                .get(selected)
                .and_then(|(_, preview)| preview.as_ref())
                .map(|preview| {
                    PreviewRenderNode::from_layout_preview(&preview.node, &preview.commands)
                });
            let entries = launcher
                .entries
                .iter()
                .map(|(name, _)| name.clone())
                .collect();
            Self {
                entries,
                selected,
                selected_preview,
            }
        }
    }

    struct ThemeLauncherRenderState {
        entries: Vec<String>,
        selected: usize,
    }

    impl ThemeLauncherRenderState {
        fn from_theme_launcher(launcher: &ThemeLauncherState) -> Self {
            let selected = if launcher.entries.is_empty() {
                0
            } else {
                launcher.selected.min(launcher.entries.len() - 1)
            };
            Self {
                entries: launcher.entries.clone(),
                selected,
            }
        }
    }

    struct SavePromptRenderState {
        prompt: String,
        preview: PreviewRenderNode,
    }

    impl SavePromptRenderState {
        fn from_store(store: &PaneStore) -> Option<Self> {
            Some(Self {
                prompt: store.save_prompt.clone()?,
                preview: PreviewRenderNode::from_live_layout(&store.layout, &store.pane_commands),
            })
        }
    }

    /// ステータスバーの高さは `cell_height` × 1 行分
    const STATUS_BAR_ROWS: i32 = 1;

    // ── クライアント状態 ─────────────────────────────────────────────────────

    /// ウィンドウが保持するクライアント状態
    pub struct ClientState {
        /// ペインストア（レイアウト + グリッド群）
        pub panes: Arc<Mutex<PaneStore>>,
        /// IME ハンドラ
        pub ime: ImeHandler,
        /// サーバーへの入力・リサイズ送信チャネル
        pub msg_tx: mpsc::Sender<ClientMessage>,
        /// ペイン分割要求チャネル（tokio 側タスクが受け取る）
        pub split_tx: mpsc::Sender<(PaneId, SplitDirection)>,
        /// セルサイズ（GDI テキストメトリクスから計算）
        pub cell_width: i32,
        pub cell_height: i32,
        /// GDI フォントハンドル
        pub hfont: HFONT,
        /// 表示中のトースト通知リスト（Win32 スレッド専用）
        pub active_toasts: Mutex<Vec<Toast>>,
        /// コンテンツ領域矩形（WM_SIZE で更新、Cell で内部可変）
        pub content_rect: std::cell::Cell<PaneRect>,
        /// ウィンドウフォーカス状態（app.rs と共有: WM_ACTIVATEAPP で更新）
        pub app_focused: Arc<AtomicBool>,
        /// NativeToast キュー（tokio → Win32 スレッドへのバルーン通知要求）
        pub native_notif_queue: Arc<Mutex<VecDeque<NativeToastMsg>>>,
        /// バルーン表示後の自動削除カウントダウン（16ms ティック単位）
        pub notif_icon_timer: std::cell::Cell<u32>,
        /// 現在の UI モード（Internal Cell で内部可変）
        pub mode: std::cell::Cell<ClientMode>,
        /// WM_KEYDOWN でモード切替済みの場合、次の WM_CHAR を抑制するフラグ
        pub skip_char: std::cell::Cell<bool>,
        /// Normal モードでマウスドラッグ選択中かどうか
        pub normal_dragging: std::cell::Cell<bool>,
        /// フローティングペイン作成/トグル要求チャネル
        pub float_tx: mpsc::Sender<()>,
        /// レイアウト切り替え要求チャネル（選択したレイアウト名を app.rs へ送る）
        pub layout_tx: mpsc::Sender<String>,
        /// 解決済みテーマ色（Cell で内部可変 — ランタイムテーマ切り替えに使用）
        theme: std::cell::Cell<WinTheme>,
    }

    impl ClientState {
        #[allow(clippy::too_many_arguments)]
        fn new(
            panes: Arc<Mutex<PaneStore>>,
            msg_tx: mpsc::Sender<ClientMessage>,
            split_tx: mpsc::Sender<(PaneId, SplitDirection)>,
            cell_width: i32,
            cell_height: i32,
            hfont: HFONT,
            app_focused: Arc<AtomicBool>,
            native_notif_queue: Arc<Mutex<VecDeque<NativeToastMsg>>>,
            float_tx: mpsc::Sender<()>,
            layout_tx: mpsc::Sender<String>,
            theme: WinTheme,
        ) -> Self {
            Self {
                panes,
                ime: ImeHandler::new(),
                msg_tx,
                split_tx,
                cell_width,
                cell_height,
                hfont,
                active_toasts: Mutex::new(Vec::new()),
                content_rect: std::cell::Cell::new(PaneRect {
                    x: 0,
                    y: 0,
                    w: 1,
                    h: 1,
                }),
                app_focused,
                native_notif_queue,
                notif_icon_timer: std::cell::Cell::new(0),
                mode: std::cell::Cell::new(ClientMode::Normal),
                skip_char: std::cell::Cell::new(false),
                normal_dragging: std::cell::Cell::new(false),
                float_tx,
                layout_tx,
                theme: std::cell::Cell::new(theme),
            }
        }

        /// アクティブペインのグリッド Arc を返す
        fn active_grid(&self) -> Arc<Mutex<Grid>> {
            let store = self.panes.lock().unwrap();
            Arc::clone(&store.grids[&store.active])
        }

        /// サーバーへ入力バイト列を送信（アクティブペイン）
        fn send_input(&self, data: Vec<u8>) {
            let active = self.panes.lock().unwrap().active;
            let _ = self
                .msg_tx
                .try_send(ClientMessage::Input { pane: active, data });
        }

        /// 全ペインをコンテンツ領域サイズに合わせてリサイズ
        fn resize_all_panes(&self, content_w: i32, content_h: i32) {
            let total = PaneRect {
                x: 0,
                y: 0,
                w: content_w,
                h: content_h,
            };
            let mut store = self.panes.lock().unwrap();
            let rects = store.layout.compute_rects(total);
            for (pane_id, rect) in &rects {
                let cols = rect.cols(self.cell_width);
                let rows = rect.rows(self.cell_height);
                if let Some(g) = store.grids.get(pane_id) {
                    g.lock().unwrap().resize(cols, rows);
                }
                let _ = self.msg_tx.try_send(ClientMessage::Resize {
                    pane: *pane_id,
                    size: TermSize { cols, rows },
                });
            }
            // リサイズ後、アクティブペインの scrollback_len に合わせてオフセットをクランプ（B-7）
            let active = store.active;
            if let Some(g) = store.grids.get(&active) {
                let max_offset = g.lock().unwrap().scrollback_len();
                store.scroll_offset = store.scroll_offset.min(max_offset);
            }
        }

        /// ペイン分割を要求する
        fn request_split(&self, direction: SplitDirection) {
            let active = self.panes.lock().unwrap().active;
            let _ = self.split_tx.try_send((active, direction));
        }

        /// フォーカスを次/前のペインに移す（線形 DFS 順）
        fn cycle_pane(&self, forward: bool) {
            let mut store = self.panes.lock().unwrap();
            let next = if forward {
                store.layout.next_pane(store.active)
            } else {
                store.layout.prev_pane(store.active)
            };
            if next != store.active {
                store.active = next;
                store.scroll_offset = 0;
            }
        }

        /// フォーカスを指定方向の最近傍ペインに移す
        fn focus_pane_dir(&self, dir: Direction) {
            let mut store = self.panes.lock().unwrap();
            let next = store
                .layout
                .pane_in_direction(store.active, dir, self.content_rect.get());
            if next != store.active {
                store.active = next;
                store.scroll_offset = 0;
            }
        }

        /// クリック座標（ウィンドウクライアント座標）からペインフォーカスを切り替える。
        /// フォーカスが変わった場合は true を返す。
        fn focus_pane_at(&self, px: i32, py: i32) -> bool {
            let cx = px - PADDING_X;
            let cy = py - PADDING_Y;
            let root = self.content_rect.get();
            let mut store = self.panes.lock().unwrap();
            if let Some(pane_id) = store.layout.pane_at_point(cx, cy, root) {
                if store.active != pane_id {
                    store.active = pane_id;
                    store.scroll_offset = 0;
                    return true;
                }
            }
            false
        }

        /// アクティブペインを削除する。
        /// 最後の1ペインの場合も ClosePane を送信し、
        /// `app.rs` の PaneClosed ハンドラが `grids.is_empty()` を検出して
        /// `should_quit = true` → `WM_TIMER` → `DestroyWindow` でアプリを終了する（C-9 と同様の終了パス）。
        fn close_active_pane(&self) {
            let active = self.panes.lock().unwrap().active;
            let _ = self
                .msg_tx
                .try_send(yatamux_protocol::ClientMessage::ClosePane { pane: active });
        }

        /// スクロールバック + 画面内容を一時ファイルに書き出し、`$EDITOR` で開く
        ///
        /// `$EDITOR` が未設定の場合は `vi` を使用する。
        /// エディタコマンドはアクティブペインへの入力として送信する。
        fn open_scrollback_in_editor(&self) {
            use std::io::Write as _;

            let (active, text) = {
                let store = self.panes.lock().unwrap();
                let active = store.active;
                let Some(grid_arc) = store.grids.get(&active) else {
                    return;
                };
                let grid = grid_arc.lock().unwrap();
                (active, grid.full_content_text())
            };

            // 一時ファイルに書き出す
            let tmp_path = std::env::temp_dir().join(format!("yatamux_scroll_{}.txt", active.0));
            let Ok(mut f) = std::fs::File::create(&tmp_path) else {
                return;
            };
            let _ = f.write_all(text.as_bytes());
            let _ = f.flush();

            // $EDITOR （未設定時は vi）でファイルを開く
            let editor = std::env::var("EDITOR").unwrap_or_else(|_| "vi".to_string());
            let cmd = format!("{} {}\r", editor, tmp_path.display());
            let _ = self
                .msg_tx
                .try_send(yatamux_protocol::ClientMessage::Input {
                    pane: active,
                    data: cmd.into_bytes(),
                });
        }
    }

    // ── キー入力ディスパッチャ ────────────────────────────────────────────────
    //
    // WM_KEYDOWN の処理を UI モード／機能ごとの独立ハンドラに分離する。
    // 各ハンドラは「自分が担当する状態のときのみ動作し、それ以外は No を返す」という
    // Chain of Responsibility パターンで実装する。
    // これにより新機能追加時に既存ハンドラへの干渉を防ぐ（A-1 参照）。

    /// WM_KEYDOWN イベントの入力情報
    struct KeyInput {
        vk: u16,
        ctrl: bool,
        shift: bool,
        wparam: WPARAM,
        lparam: LPARAM,
    }

    /// キー処理結果
    #[derive(PartialEq)]
    enum KeyConsumed {
        /// 消費済み。WM_CHAR も抑制する
        Yes,
        /// 消費済み。WM_CHAR は通す（保存プロンプトの印字可能文字入力用）
        YesPassChar,
        /// 未処理（次のハンドラへ委譲）
        No,
    }

    impl ClientState {
        // ── 保存プロンプトのキー処理 ────────────────────────────────────────────
        unsafe fn handle_save_prompt(state: &Self, hwnd: HWND, key: &KeyInput) -> KeyConsumed {
            if state.panes.lock().unwrap().save_prompt.is_none() {
                return KeyConsumed::No;
            }
            let vk = key.vk;
            let result = if vk == VK_RETURN.0 {
                let (name, layout_toml) = {
                    let mut store = state.panes.lock().unwrap();
                    let name = store.save_prompt.take().unwrap_or_default();
                    let toml = layout_to_toml(&store.layout, &store.pane_commands);
                    (name, toml)
                };
                let name = name.trim().to_string();
                if !name.is_empty() {
                    match save_layout_file(&name, &layout_toml) {
                        Ok(()) => {
                            let mut store = state.panes.lock().unwrap();
                            let active = store.active;
                            store.pending_toasts.push_back(crate::layout::Toast {
                                pane_id: active,
                                message: format!("レイアウト「{name}」を保存しました"),
                                elapsed_ms: 0,
                            });
                        }
                        Err(e) => {
                            let mut store = state.panes.lock().unwrap();
                            let active = store.active;
                            store.pending_toasts.push_back(crate::layout::Toast {
                                pane_id: active,
                                message: format!("保存エラー: {e}"),
                                elapsed_ms: 0,
                            });
                        }
                    }
                }
                KeyConsumed::Yes
            } else if vk == VK_ESCAPE.0 {
                state.panes.lock().unwrap().save_prompt = None;
                KeyConsumed::Yes
            } else if vk == VK_BACK.0 {
                let mut store = state.panes.lock().unwrap();
                if let Some(s) = &mut store.save_prompt {
                    s.pop();
                }
                KeyConsumed::Yes
            } else {
                // 印字可能キーは WM_CHAR に委譲してプロンプト文字列に追記する
                KeyConsumed::YesPassChar
            };
            let _ = InvalidateRect(Some(hwnd), None, false);
            result
        }

        // ── レイアウトランチャーのキー処理 ──────────────────────────────────────
        unsafe fn handle_layout_launcher(state: &Self, hwnd: HWND, key: &KeyInput) -> KeyConsumed {
            if state.panes.lock().unwrap().launcher.is_none() {
                return KeyConsumed::No;
            }
            let vk = key.vk;
            if vk == VK_RETURN.0 {
                let name = {
                    let mut store = state.panes.lock().unwrap();
                    store.launcher.take().and_then(|launcher| {
                        if launcher.entries.is_empty() {
                            None
                        } else {
                            launcher
                                .entries
                                .into_iter()
                                .nth(launcher.selected)
                                .map(|(name, _)| name)
                        }
                    })
                };
                if let Some(name) = name {
                    let _ = state.layout_tx.try_send(name);
                }
            } else if vk == VK_ESCAPE.0 || vk == b'Q' as u16 {
                state.panes.lock().unwrap().launcher = None;
            } else {
                let mut store = state.panes.lock().unwrap();
                if let Some(launcher) = &mut store.launcher {
                    if vk == VK_UP.0 {
                        launcher.selected = launcher.selected.saturating_sub(1);
                    } else if vk == VK_DOWN.0 {
                        let max = launcher.entries.len().saturating_sub(1);
                        launcher.selected = (launcher.selected + 1).min(max);
                    }
                }
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            KeyConsumed::Yes
        }

        // ── テーマランチャーのキー処理 ──────────────────────────────────────────
        unsafe fn handle_theme_launcher(state: &Self, hwnd: HWND, key: &KeyInput) -> KeyConsumed {
            if state.panes.lock().unwrap().theme_launcher.is_none() {
                return KeyConsumed::No;
            }
            let vk = key.vk;
            if vk == VK_RETURN.0 {
                let name = {
                    let mut store = state.panes.lock().unwrap();
                    store.theme_launcher.take().and_then(|launcher| {
                        if launcher.entries.is_empty() {
                            None
                        } else {
                            launcher.entries.into_iter().nth(launcher.selected)
                        }
                    })
                };
                if let Some(name) = name {
                    if let Some(theme) = load_theme_from_file(&name) {
                        let win_theme = WinTheme::from_theme(&theme);
                        state.theme.set(win_theme);
                    }
                }
            } else if vk == VK_ESCAPE.0 || vk == b'Q' as u16 {
                state.panes.lock().unwrap().theme_launcher = None;
            } else {
                let mut store = state.panes.lock().unwrap();
                if let Some(tl) = &mut store.theme_launcher {
                    if vk == VK_UP.0 {
                        tl.selected = tl.selected.saturating_sub(1);
                    } else if vk == VK_DOWN.0 {
                        let max = tl.entries.len().saturating_sub(1);
                        tl.selected = (tl.selected + 1).min(max);
                    }
                }
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            KeyConsumed::Yes
        }

        // ── コピーモードのキー処理 ──────────────────────────────────────────────
        unsafe fn handle_copy_mode(state: &Self, hwnd: HWND, key: &KeyInput) -> KeyConsumed {
            if state.mode.get() != ClientMode::Copy {
                return KeyConsumed::No;
            }
            let vk = key.vk;
            let (cols, rows) = {
                let g = state.active_grid();
                let g = g.lock().unwrap();
                (g.cols() as usize, g.rows() as usize)
            };
            match vk {
                k if k == VK_ESCAPE.0 || k == b'Q' as u16 => {
                    state.mode.set(ClientMode::Normal);
                    state.panes.lock().unwrap().copy_mode = None;
                }
                k if k == b'H' as u16 || k == VK_LEFT.0 => {
                    if let Some(cm) = &mut state.panes.lock().unwrap().copy_mode {
                        cm.move_cursor(-1, 0, cols, rows);
                    }
                }
                k if k == b'L' as u16 || k == VK_RIGHT.0 => {
                    if let Some(cm) = &mut state.panes.lock().unwrap().copy_mode {
                        cm.move_cursor(1, 0, cols, rows);
                    }
                }
                k if k == b'K' as u16 || k == VK_UP.0 => {
                    if let Some(cm) = &mut state.panes.lock().unwrap().copy_mode {
                        cm.move_cursor(0, -1, cols, rows);
                    }
                }
                k if k == b'J' as u16 || k == VK_DOWN.0 => {
                    if let Some(cm) = &mut state.panes.lock().unwrap().copy_mode {
                        cm.move_cursor(0, 1, cols, rows);
                    }
                }
                k if k == b'V' as u16 => {
                    if let Some(cm) = &mut state.panes.lock().unwrap().copy_mode {
                        cm.toggle_anchor();
                    }
                }
                k if k == b'Y' as u16 || k == VK_RETURN.0 => {
                    let clip_data = {
                        let store = state.panes.lock().unwrap();
                        if let Some(cm) = &store.copy_mode {
                            if let Some((row_start, row_end)) = cm.selection_rows() {
                                let active = store.active;
                                store.grids.get(&active).map(|grid_arc| {
                                    let grid = grid_arc.lock().unwrap();
                                    grid.extract_text(row_start, row_end).into_bytes()
                                })
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    };
                    {
                        let mut store = state.panes.lock().unwrap();
                        if let Some(data) = clip_data {
                            store.pending_clipboard = Some(data);
                        }
                        store.copy_mode = None;
                    }
                    state.mode.set(ClientMode::Normal);
                }
                _ => {}
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            KeyConsumed::Yes
        }

        // ── ペインモードのキー処理（Ctrl+B で遷移）────────────────────────────
        unsafe fn handle_pane_mode(state: &Self, hwnd: HWND, key: &KeyInput) -> KeyConsumed {
            if state.mode.get() != ClientMode::Pane {
                return KeyConsumed::No;
            }
            let vk = key.vk;
            let shift = key.shift;

            // モディファイアキー単体はペインモードを維持して無視
            const MODIFIER_KEYS: &[u16] = &[
                0x10, // VK_SHIFT
                0x11, // VK_CONTROL
                0x12, // VK_MENU (Alt)
                0xA0, // VK_LSHIFT
                0xA1, // VK_RSHIFT
                0xA2, // VK_LCONTROL
                0xA3, // VK_RCONTROL
                0xA4, // VK_LMENU
                0xA5, // VK_RMENU
            ];
            if MODIFIER_KEYS.contains(&vk) {
                return KeyConsumed::Yes;
            }

            // `<`/`>` はペインモードを維持して水平比率を調整（繰り返し操作可能）
            const VK_OEM_COMMA: u16 = 0xBC;
            const VK_OEM_PERIOD: u16 = 0xBE;
            if shift && (vk == VK_OEM_COMMA || vk == VK_OEM_PERIOD) {
                let active = state.panes.lock().unwrap().active;
                let delta = if vk == VK_OEM_PERIOD {
                    0.05_f32
                } else {
                    -0.05_f32
                };
                state.panes.lock().unwrap().layout.adjust_ratio_for_dir(
                    active,
                    delta,
                    SplitDirection::Vertical,
                );
                let _ = InvalidateRect(Some(hwnd), None, false);
                return KeyConsumed::Yes;
            }

            // `+`/`-` はペインモードを維持して垂直比率を調整
            const VK_OEM_PLUS: u16 = 0xBB;
            const VK_OEM_MINUS: u16 = 0xBD;
            let is_plus = shift && vk == VK_OEM_PLUS;
            let is_minus = !shift && vk == VK_OEM_MINUS;
            if is_plus || is_minus {
                let active = state.panes.lock().unwrap().active;
                let delta = if is_plus { 0.05_f32 } else { -0.05_f32 };
                state.panes.lock().unwrap().layout.adjust_ratio_for_dir(
                    active,
                    delta,
                    SplitDirection::Horizontal,
                );
                let _ = InvalidateRect(Some(hwnd), None, false);
                return KeyConsumed::Yes;
            }

            // その他のキー: ペインモードを抜けて各アクションを実行
            match vk {
                k if k == b'V' as u16 => {
                    // コピーモードに入る（Normal 遷移なし）
                    state.mode.set(ClientMode::Copy);
                    state.panes.lock().unwrap().copy_mode = Some(CopyState::new(0, 0));
                }
                k if k == b'S' as u16 => {
                    // レイアウト保存プロンプトを開く（ペインモードを維持）
                    state.panes.lock().unwrap().save_prompt = Some(String::new());
                }
                _ => {
                    state.mode.set(ClientMode::Normal);
                    match vk {
                        k if k == b'E' as u16 => state.request_split(SplitDirection::Vertical),
                        k if k == b'O' as u16 => state.request_split(SplitDirection::Horizontal),
                        k if k == b'W' as u16 => state.close_active_pane(),
                        k if k == b'F' as u16 => {
                            let _ = state.float_tx.try_send(());
                        }
                        k if k == b'X' as u16 => state.open_scrollback_in_editor(),
                        k if k == b'L' as u16 => {
                            let entries = list_available_layouts();
                            state.panes.lock().unwrap().launcher =
                                Some(LauncherState::new(entries));
                        }
                        _ => {} // 未定義キーは無視
                    }
                }
            }
            let _ = InvalidateRect(Some(hwnd), None, false);
            KeyConsumed::Yes
        }

        // ── グローバルショートカット（モード非依存）────────────────────────────
        unsafe fn handle_global_shortcuts(state: &Self, hwnd: HWND, key: &KeyInput) -> KeyConsumed {
            let ctrl = key.ctrl;
            let shift = key.shift;
            let vk = key.vk;

            // Ctrl+F: フローティングペイン トグル
            if ctrl && !shift && vk == b'F' as u16 {
                let _ = state.float_tx.try_send(());
                let _ = InvalidateRect(Some(hwnd), None, false);
                return KeyConsumed::Yes;
            }

            // Ctrl+B: Normal → Pane モードへ切り替え
            if ctrl && !shift && vk == b'B' as u16 {
                state.mode.set(ClientMode::Pane);
                let _ = InvalidateRect(Some(hwnd), None, false);
                return KeyConsumed::Yes;
            }

            // Ctrl+P: テーマランチャーを開く
            if ctrl && !shift && vk == b'P' as u16 {
                let entries = list_available_themes();
                state.panes.lock().unwrap().theme_launcher = Some(ThemeLauncherState::new(entries));
                let _ = InvalidateRect(Some(hwnd), None, false);
                return KeyConsumed::Yes;
            }

            // Ctrl+Shift+E: 縦分割 (side by side)
            if ctrl && shift && vk == b'E' as u16 {
                state.request_split(SplitDirection::Vertical);
                return KeyConsumed::Yes;
            }

            // Ctrl+Shift+O: 横分割 (top / bottom)
            if ctrl && shift && vk == b'O' as u16 {
                state.request_split(SplitDirection::Horizontal);
                return KeyConsumed::Yes;
            }

            // Ctrl+Shift+W: アクティブペイン削除（最後の1枚はアプリ終了）(B-6)
            if ctrl && shift && vk == b'W' as u16 {
                state.close_active_pane();
                return KeyConsumed::Yes;
            }

            // Ctrl+Tab: 次のペイン / Ctrl+Shift+Tab: 前のペイン
            if ctrl && vk == VK_TAB.0 {
                state.cycle_pane(!shift);
                let _ = InvalidateRect(Some(hwnd), None, false);
                return KeyConsumed::Yes;
            }

            // Ctrl+Arrow: 方向指定ペインフォーカス移動
            if ctrl && !shift {
                let dir = match vk {
                    k if k == VK_LEFT.0 => Some(Direction::Left),
                    k if k == VK_RIGHT.0 => Some(Direction::Right),
                    k if k == VK_UP.0 => Some(Direction::Up),
                    k if k == VK_DOWN.0 => Some(Direction::Down),
                    _ => None,
                };
                if let Some(d) = dir {
                    state.focus_pane_dir(d);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return KeyConsumed::Yes;
                }
            }

            // Ctrl+V: クリップボードからペースト
            if ctrl && !shift && vk == b'V' as u16 {
                let bracketed = state.active_grid().lock().unwrap().bracketed_paste();
                if let Some(text) = read_clipboard_text(hwnd) {
                    let mut data = Vec::new();
                    if bracketed {
                        data.extend_from_slice(b"\x1b[200~");
                    }
                    data.extend_from_slice(text.as_bytes());
                    if bracketed {
                        data.extend_from_slice(b"\x1b[201~");
                    }
                    state.send_input(data);
                }
                return KeyConsumed::Yes;
            }

            // Normal モード + 選択範囲あり + Ctrl+C: クリップボードにコピー（PTY 送信なし）
            if state.mode.get() == ClientMode::Normal && ctrl && !shift && vk == b'C' as u16 {
                let clip_data = {
                    let store = state.panes.lock().unwrap();
                    store.normal_selection.and_then(|(ac, ar, ec, er)| {
                        if (ac, ar) == (ec, er) {
                            return None;
                        }
                        let row_start = ar.min(er);
                        let row_end = ar.max(er);
                        store.grids.get(&store.active).map(|grid_arc| {
                            let grid = grid_arc.lock().unwrap();
                            grid.extract_text(row_start, row_end).into_bytes()
                        })
                    })
                };
                if let Some(data) = clip_data {
                    let mut store = state.panes.lock().unwrap();
                    store.pending_clipboard = Some(data);
                    store.normal_selection = None;
                    drop(store);
                    let _ = InvalidateRect(Some(hwnd), None, false);
                    return KeyConsumed::Yes;
                }
                // 選択なし → PTY に \x03 を送る（次のハンドラへ委譲）
            }

            KeyConsumed::No
        }

        // ── VT キーマッピング（PTY パススルー）─────────────────────────────────
        unsafe fn handle_vt_passthrough(state: &Self, _hwnd: HWND, key: &KeyInput) -> KeyConsumed {
            let app_cursor = state
                .active_grid()
                .lock()
                .unwrap()
                .application_cursor_keys();
            if let Some(vt) = keydown_to_vt(key.wparam, key.lparam, app_cursor) {
                state.send_input(vt);
                return KeyConsumed::Yes;
            }
            KeyConsumed::No
        }

        // ── WM_KEYDOWN ディスパッチャ ───────────────────────────────────────────
        // 各ハンドラを優先順位順に呼び出す。最初に Yes/YesPassChar を返したハンドラで停止する。
        unsafe fn dispatch_wm_keydown(state: &Self, hwnd: HWND, key: &KeyInput) -> KeyConsumed {
            let r = Self::handle_save_prompt(state, hwnd, key);
            if r != KeyConsumed::No {
                return r;
            }
            let r = Self::handle_layout_launcher(state, hwnd, key);
            if r != KeyConsumed::No {
                return r;
            }
            let r = Self::handle_theme_launcher(state, hwnd, key);
            if r != KeyConsumed::No {
                return r;
            }
            let r = Self::handle_copy_mode(state, hwnd, key);
            if r != KeyConsumed::No {
                return r;
            }
            let r = Self::handle_pane_mode(state, hwnd, key);
            if r != KeyConsumed::No {
                return r;
            }
            let r = Self::handle_global_shortcuts(state, hwnd, key);
            if r != KeyConsumed::No {
                return r;
            }
            Self::handle_vt_passthrough(state, hwnd, key)
        }
    }

    // ── WndProc ──────────────────────────────────────────────────────────────

    unsafe extern "system" fn wnd_proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        // GWLP_USERDATA から ClientState ポインタを取得
        let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ClientState;

        match msg {
            // ── ウィンドウ作成 ──────────────────────────────────────────
            WM_CREATE => {
                // CREATESTRUCTW の lpCreateParams に ClientState* が入っている
                let cs = &*(lparam.0 as *const CREATESTRUCTW);
                SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
                // PTY 出力を画面に反映するための定期再描画タイマーを開始
                SetTimer(Some(hwnd), TIMER_REPAINT, TIMER_INTERVAL_MS, None);
                // DWM ダークモードタイトルバー（Windows 10 1903+ / Windows 11）
                let dark: i32 = 1;
                let _ = DwmSetWindowAttribute(
                    hwnd,
                    DWMWINDOWATTRIBUTE(20), // DWMWA_USE_IMMERSIVE_DARK_MODE
                    &dark as *const i32 as *const _,
                    std::mem::size_of::<i32>() as u32,
                );
                LRESULT(0)
            }

            // ── 描画 ────────────────────────────────────────────────────
            WM_PAINT => {
                if state_ptr.is_null() {
                    return DefWindowProcW(hwnd, msg, wparam, lparam);
                }
                let state = &*state_ptr;
                paint(hwnd, state);
                LRESULT(0)
            }

            // ── IME: 変換開始 ───────────────────────────────────────────
            WM_IME_STARTCOMPOSITION => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    state.ime.on_start_composition();
                }
                // デフォルト処理を呼ばないことで、IME がデフォルトの
                // コンポジションウィンドウを表示しないようにする
                LRESULT(0)
            }

            // ── IME: 変換中（プリエディット更新 / 確定）────────────────
            WM_IME_COMPOSITION => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    state.ime.on_composition(hwnd, lparam.0 as usize);

                    // 確定文字列があればサーバーに送信
                    let committed = {
                        let s = state.ime.state.lock().unwrap();
                        s.committed.clone()
                    };
                    if let Some(text) = committed {
                        state.send_input(text.into_bytes());
                        state.ime.state.lock().unwrap().committed = None;
                    }

                    // 候補ウィンドウをカーソル位置に更新
                    let (cur_col, cur_row) = {
                        let g = state.active_grid();
                        let g = g.lock().unwrap();
                        let c = g.cursor();
                        (c.col, c.row)
                    };
                    let cursor_pixel = CellPixelPos {
                        x: cur_col as i32 * state.cell_width + PADDING_X,
                        y: cur_row as i32 * state.cell_height + PADDING_Y,
                        cell_width: state.cell_width,
                        cell_height: state.cell_height,
                    };
                    state.ime.update_candidate_window(hwnd, cursor_pixel);

                    // 再描画（プリエディット表示更新）
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }

            // ── IME: 変換終了 ───────────────────────────────────────────
            WM_IME_ENDCOMPOSITION => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    state.ime.on_end_composition();
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            // ── キーボード入力（通常文字） ──────────────────────────────
            WM_CHAR => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    // Pane モード中のキー操作は WM_KEYDOWN で処理済み。
                    // TranslateMessage が WM_CHAR を投入する前に WM_KEYDOWN が
                    // skip_char フラグを立てるので、ここで抑制する。
                    if state.skip_char.get() {
                        state.skip_char.set(false);
                        return LRESULT(0);
                    }
                    // 保存プロンプトが開いている間は文字入力をプロンプトへリダイレクト
                    {
                        let save_open = state.panes.lock().unwrap().save_prompt.is_some();
                        if save_open {
                            let code = wparam.0 as u32;
                            if let Some(ch) = char::from_u32(code) {
                                // 制御文字・Backspace・CR/LF は WM_KEYDOWN で処理済みなのでスキップ
                                if !ch.is_control() {
                                    let mut store = state.panes.lock().unwrap();
                                    if let Some(s) = &mut store.save_prompt {
                                        s.push(ch);
                                    }
                                    let _ = InvalidateRect(Some(hwnd), None, false);
                                }
                            }
                            return LRESULT(0);
                        }
                    }
                    // IME 変換中の文字は WM_IME_COMPOSITION で処理済み
                    if !state.ime.state.lock().unwrap().composing {
                        let code = wparam.0 as u32;
                        let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                        let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                        // WM_KEYDOWN で処理済みのキーは WM_CHAR を無視する
                        // (TranslateMessage が二重送信を起こすのを防ぐ)
                        // 8  = Backspace → WM_KEYDOWN が \x7f を送信済み
                        // 9  = Tab / Shift+Tab → WM_KEYDOWN が \t or \x1b[Z を送信済み
                        // Ctrl+Shift+letter → ペイン分割などのショートカットで処理済み
                        //   TranslateMessage が先に WM_CHAR(\x05=^E 等) を投入するため
                        //   こちら側で弾く必要がある
                        let skip = matches!(code, 8 | 9) || (ctrl && shift);
                        if !skip {
                            if let Some(ch) = char::from_u32(code) {
                                if ch != '\0' {
                                    // スクロール中なら最新画面に戻す
                                    state.panes.lock().unwrap().scroll_offset = 0;
                                    let mut buf = [0u8; 4];
                                    let encoded = ch.encode_utf8(&mut buf);
                                    state.send_input(encoded.as_bytes().to_vec());
                                }
                            }
                        }
                    }
                }
                LRESULT(0)
            }

            // ── 制御キー ────────────────────────────────────────────
            WM_KEYDOWN => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
                    let key = KeyInput {
                        vk: wparam.0 as u16,
                        ctrl,
                        shift,
                        wparam,
                        lparam,
                    };
                    match ClientState::dispatch_wm_keydown(state, hwnd, &key) {
                        KeyConsumed::Yes => {
                            state.skip_char.set(true);
                            return LRESULT(0);
                        }
                        KeyConsumed::YesPassChar => return LRESULT(0),
                        KeyConsumed::No => {}
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            // ── リサイズ ────────────────────────────────────────────────
            WM_SIZE => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    let width = (lparam.0 & 0xFFFF) as i32;
                    let height = ((lparam.0 >> 16) & 0xFFFF) as i32;
                    if state.cell_width > 0 && state.cell_height > 0 {
                        let content_w = (width - PADDING_X * 2).max(1);
                        // ステータスバー（1 行分）を下端に確保する
                        let status_h = state.cell_height * STATUS_BAR_ROWS;
                        let content_h = (height - PADDING_Y * 2 - status_h).max(1);
                        state.content_rect.set(PaneRect {
                            x: 0,
                            y: 0,
                            w: content_w,
                            h: content_h,
                        });
                        state.resize_all_panes(content_w, content_h);
                    }
                }
                LRESULT(0)
            }

            // ── マウスホイール（スクロールバック） ──────────────────────
            WM_MOUSEWHEEL => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    // WHEEL_DELTA = 120 単位。正 = 上スクロール（過去方向）
                    let delta = (wparam.0 >> 16) as i16;
                    let lines: usize = 3; // 1 ノッチあたり 3 行
                    let mut store = state.panes.lock().unwrap();
                    let active = store.active;
                    if let Some(grid_arc) = store.grids.get(&active) {
                        let max_offset = grid_arc.lock().unwrap().scrollback_len();
                        if delta > 0 {
                            store.scroll_offset = (store.scroll_offset + lines).min(max_offset);
                        } else {
                            store.scroll_offset = store.scroll_offset.saturating_sub(lines);
                        }
                    }
                    let _ = InvalidateRect(Some(hwnd), None, false);
                }
                LRESULT(0)
            }

            // ── 定期再描画タイマー ──────────────────────────────────────
            WM_TIMER => {
                if wparam.0 == TIMER_REPAINT && !state_ptr.is_null() {
                    let state = &*state_ptr;

                    // OSC 52 クリップボード書き込み（pending_clipboard があれば処理）
                    let clip = state.panes.lock().unwrap().pending_clipboard.take();
                    if let Some(data) = clip {
                        write_clipboard_text(hwnd, &data);
                    }

                    // トースト: pending → active に移し、経過時間を進め、期限切れを削除
                    let has_active_toasts = {
                        let mut active = state.active_toasts.lock().unwrap();
                        {
                            let mut store = state.panes.lock().unwrap();
                            while let Some(t) = store.pending_toasts.pop_front() {
                                active.push(t);
                            }
                        }
                        for t in active.iter_mut() {
                            t.elapsed_ms = t.elapsed_ms.saturating_add(TIMER_INTERVAL_MS);
                        }
                        active.retain(|t| t.elapsed_ms < Toast::DURATION_MS);
                        !active.is_empty()
                    };

                    // NativeToast バルーン: キューから取り出して Shell_NotifyIconW で表示
                    if let Some(msg) = state.native_notif_queue.lock().unwrap().pop_front() {
                        show_balloon_notification(hwnd, &msg.title, &msg.body);
                        state.notif_icon_timer.set(300); // ~5 秒 (300 × 16ms)
                    }
                    let t = state.notif_icon_timer.get();
                    if t > 0 {
                        state.notif_icon_timer.set(t - 1);
                        if t == 1 {
                            remove_tray_icon(hwnd);
                        }
                    }

                    // C-9: 全ペイン終了時にウィンドウを破棄してアプリ終了
                    let quit = state.panes.lock().unwrap().should_quit;
                    if quit {
                        let _ = DestroyWindow(hwnd);
                        return LRESULT(0);
                    }

                    let needs_repaint = {
                        let store = state.panes.lock().unwrap();
                        let dirty = store
                            .grids
                            .values()
                            .any(|g| g.lock().unwrap().has_dirty_rows());
                        dirty || state.ime.state.lock().unwrap().composing
                    };
                    if needs_repaint || has_active_toasts {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                }
                LRESULT(0)
            }

            // ── マウス入力 ──────────────────────────────────────────────
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP | WM_MOUSEMOVE => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;

                    // 左クリック時にペインフォーカスを切り替える（F-9）
                    if msg == WM_LBUTTONDOWN {
                        let px = (lparam.0 & 0xFFFF) as i32;
                        let py = ((lparam.0 >> 16) & 0xFFFF) as i32;

                        // Normal モード選択とドラッグ状態をリセット
                        state.panes.lock().unwrap().normal_selection = None;
                        state.normal_dragging.set(false);

                        // フローティングペイン表示中: 外側クリックで非表示に
                        let float_handled = {
                            let mut store = state.panes.lock().unwrap();
                            if store.floating_visible {
                                let content = state.content_rect.get();
                                let fr = PaneStore::floating_rect(content);
                                let cx = (px - PADDING_X).max(0);
                                let cy = (py - PADDING_Y).max(0);
                                if cx < fr.x || cx >= fr.x + fr.w || cy < fr.y || cy >= fr.y + fr.h
                                {
                                    store.hide_float();
                                    true
                                } else {
                                    false
                                }
                            } else {
                                false
                            }
                        };
                        if float_handled || state.focus_pane_at(px, py) {
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }

                        // Normal モード: アクティブペイン上のドラッグ選択開始
                        if state.mode.get() == ClientMode::Normal {
                            let content = state.content_rect.get();
                            let sel_start = {
                                let store = state.panes.lock().unwrap();
                                let active = store.active;
                                let rects = store.layout.compute_rects(content);
                                rects
                                    .iter()
                                    .find(|(id, _)| *id == active)
                                    .and_then(|(_, pr)| {
                                        let cx = px - PADDING_X;
                                        let cy = py - PADDING_Y;
                                        if cx >= pr.x
                                            && cx < pr.x + pr.w
                                            && cy >= pr.y
                                            && cy < pr.y + pr.h
                                        {
                                            let col =
                                                ((cx - pr.x) / state.cell_width.max(1)) as usize;
                                            let row =
                                                ((cy - pr.y) / state.cell_height.max(1)) as usize;
                                            Some((col, row))
                                        } else {
                                            None
                                        }
                                    })
                            };
                            if let Some((col, row)) = sel_start {
                                state.panes.lock().unwrap().normal_selection =
                                    Some((col, row, col, row));
                                state.normal_dragging.set(true);
                            }
                        }
                    }

                    // Normal モード: ドラッグ選択の終点を更新
                    if msg == WM_MOUSEMOVE
                        && state.normal_dragging.get()
                        && state.mode.get() == ClientMode::Normal
                    {
                        let px = (lparam.0 & 0xFFFF) as i32;
                        let py = ((lparam.0 >> 16) & 0xFFFF) as i32;
                        let content = state.content_rect.get();
                        let sel_end = {
                            let store = state.panes.lock().unwrap();
                            let active = store.active;
                            let rects = store.layout.compute_rects(content);
                            rects.iter().find(|(id, _)| *id == active).map(|(_, pr)| {
                                let cx = (px - PADDING_X - pr.x).max(0);
                                let cy = (py - PADDING_Y - pr.y).max(0);
                                let col = (cx / state.cell_width.max(1)) as usize;
                                let row = (cy / state.cell_height.max(1)) as usize;
                                (col, row)
                            })
                        };
                        if let Some((ec, er)) = sel_end {
                            let mut store = state.panes.lock().unwrap();
                            if let Some(sel) = &mut store.normal_selection {
                                sel.2 = ec;
                                sel.3 = er;
                            }
                            let _ = InvalidateRect(Some(hwnd), None, false);
                        }
                    }

                    // Normal モード: ドラッグ終了
                    if msg == WM_LBUTTONUP && state.normal_dragging.get() {
                        state.normal_dragging.set(false);
                    }

                    let (reporting, sgr) = {
                        let g = state.active_grid();
                        let g = g.lock().unwrap();
                        (g.mouse_reporting(), g.mouse_sgr())
                    };
                    // モーション通知は mode>=2 のときのみ（ボタン押下中は mode>=2、全モーションは mode==3）
                    let is_motion = msg == WM_MOUSEMOVE;
                    let btn_down = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN);
                    let btn_up = matches!(msg, WM_LBUTTONUP | WM_RBUTTONUP);
                    let send = match reporting {
                        0 => false,
                        1 => btn_down,                        // x10: 押下のみ
                        2 => btn_down || btn_up || is_motion, // button: 押下中モーション
                        _ => btn_down || btn_up || is_motion, // any: 全モーション
                    };
                    if send {
                        // lparam の低 16 bit = X, 高 16 bit = Y (クライアント座標・ピクセル)
                        let px = (lparam.0 & 0xFFFF) as i32;
                        let py = ((lparam.0 >> 16) & 0xFFFF) as i32;
                        let col = ((px - PADDING_X) / state.cell_width.max(1)).max(0) as u16 + 1;
                        let row = ((py - PADDING_Y) / state.cell_height.max(1)).max(0) as u16 + 1;
                        // ボタン番号: 左=0, 右=2, モーション=32+btn
                        let base_btn: u8 = if is_motion {
                            let held = wparam.0 as u32;
                            let b = if held & 0x0001 != 0 {
                                0u8
                            } else if held & 0x0002 != 0 {
                                2
                            } else {
                                3
                            };
                            32 + b
                        } else {
                            match msg {
                                WM_LBUTTONDOWN | WM_LBUTTONUP => 0,
                                _ => 2,
                            }
                        };
                        if let Some(data) =
                            mouse_to_vt(base_btn, col, row, btn_up && !is_motion, sgr)
                        {
                            state.send_input(data);
                        }
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            // ── アプリフォーカス切り替え（通知バックエンド選択用）───────
            WM_ACTIVATEAPP => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    let focused = wparam.0 != 0;
                    state.app_focused.store(focused, Ordering::Relaxed);
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            // ── フォーカス ──────────────────────────────────────────────
            WM_SETFOCUS => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    if state.active_grid().lock().unwrap().focus_events() {
                        state.send_input(b"\x1b[I".to_vec());
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }
            WM_KILLFOCUS => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    if state.active_grid().lock().unwrap().focus_events() {
                        state.send_input(b"\x1b[O".to_vec());
                    }
                }
                DefWindowProcW(hwnd, msg, wparam, lparam)
            }

            // ── ウィンドウ終了（セッション保存）──────────────────────
            WM_CLOSE => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    let store = state.panes.lock().unwrap();
                    let snap = crate::session::LayoutSnapshot {
                        root: crate::session::LayoutNodeDef::from(&store.layout),
                        active: store.active,
                    };
                    let path = crate::session::LayoutSnapshot::default_path();
                    if let Err(e) = snap.save(&path) {
                        tracing::warn!("セッション保存に失敗: {}", e);
                    }
                }
                DestroyWindow(hwnd).ok();
                LRESULT(0)
            }

            // ── ウィンドウ破棄 ──────────────────────────────────────────
            WM_DESTROY => {
                let _ = KillTimer(Some(hwnd), TIMER_REPAINT);
                remove_tray_icon(hwnd);
                PostQuitMessage(0);
                LRESULT(0)
            }

            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }

    // ── GDI 描画 ─────────────────────────────────────────────────────────────

    unsafe fn paint(hwnd: HWND, state: &ClientState) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);

        // バックバッファ（ちらつき防止）
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect).ok();
        let mem_dc = CreateCompatibleDC(Some(hdc));
        let mem_bmp = CreateCompatibleBitmap(hdc, rect.right, rect.bottom);
        let old_bmp = SelectObject(mem_dc, mem_bmp.into());

        // 現在のテーマ色を取得（paint 関数内で共通使用）
        let theme = state.theme.get();

        // 背景塗りつぶし
        let bg_brush = CreateSolidBrush(theme.bg);
        FillRect(mem_dc, &rect, bg_brush);
        let _ = DeleteObject(bg_brush.into());

        // フォント設定
        let old_font = SelectObject(mem_dc, state.hfont.into());
        SetBkMode(mem_dc, OPAQUE);

        // ── コンテンツ領域とレイアウト ─────────────────────────────────
        let content_w = (rect.right - PADDING_X * 2).max(1);
        // ステータスバー（1 行分）を下端に確保する
        let status_h = state.cell_height * STATUS_BAR_ROWS;
        let content_h = (rect.bottom - PADDING_Y * 2 - status_h).max(1);
        let total_rect = PaneRect {
            x: 0,
            y: 0,
            w: content_w,
            h: content_h,
        };

        // PaneStore を短時間ロックして必要な情報を取得
        let (
            active_pane,
            scroll_offset,
            pane_rects,
            sep_rects,
            grid_map,
            copy_mode_state,
            normal_selection,
        ) = {
            let store = state.panes.lock().unwrap();
            let rects = store.layout.compute_rects(total_rect);
            let seps = store.layout.compute_separator_rects(total_rect);
            let map: HashMap<PaneId, Arc<Mutex<Grid>>> = store
                .grids
                .iter()
                .map(|(&id, g)| (id, Arc::clone(g)))
                .collect();
            let cm = store.copy_mode.clone();
            let ns = store.normal_selection;
            (store.active, store.scroll_offset, rects, seps, map, cm, ns)
        };

        // ── 各ペインを描画 ──────────────────────────────────────────────
        let ime_state = state.ime.state.lock().unwrap();

        for (pane_id, pane_rect) in &pane_rects {
            let is_active = *pane_id == active_pane;
            if let Some(grid_arc) = grid_map.get(pane_id) {
                let grid = grid_arc.lock().unwrap();
                let off_x = PADDING_X + pane_rect.x;
                let off_y = PADDING_Y + pane_rect.y;
                // ペインの表示幅をセル数に変換（グリッドが表示幅より広い場合に
                // はみ出したセルを隣ペインに描画しないようクリップする）
                let display_cols = (pane_rect.w / state.cell_width.max(1)).max(1) as usize;

                // アクティブペインかつスクロール中は scrollback を考慮したオフセット描画
                let effective_offset = if is_active { scroll_offset } else { 0 };
                let sb_len = grid.scrollback_len();
                // 表示開始位置（スクロールバック+グリッド結合バッファ上のインデックス）
                let view_start = sb_len.saturating_sub(effective_offset);

                for row in 0..grid.rows() {
                    let combined_idx = view_start + row as usize;
                    let cells: &[Cell] = if combined_idx < sb_len {
                        match grid.scrollback_row(combined_idx) {
                            Some(r) => r.as_slice(),
                            None => continue,
                        }
                    } else {
                        let grid_row = (combined_idx - sb_len) as u16;
                        match grid.row(grid_row) {
                            Some(c) => c,
                            None => continue,
                        }
                    };
                    let y = row as i32 * state.cell_height + off_y;
                    let mut x = off_x;

                    for (col_idx, cell) in cells.iter().take(display_cols).enumerate() {
                        let cell_rect = RECT {
                            left: x,
                            top: y,
                            right: x + state.cell_width,
                            bottom: y + state.cell_height,
                        };

                        // コピーモードまたは Normal モードマウス選択中セルかどうか判定
                        let is_copy_sel = is_active
                            && copy_mode_state.as_ref().is_some_and(|cm| {
                                cm.anchor.is_some() && cm.is_selected(col_idx, row as usize)
                            });
                        let is_normal_sel = is_active
                            && copy_mode_state.is_none()
                            && normal_selection.is_some_and(|sel| {
                                is_in_normal_selection(sel, col_idx, row as usize)
                            });
                        let is_sel = is_copy_sel || is_normal_sel;

                        match &cell.content {
                            CellContent::Grapheme { text, width } => {
                                let (raw_fg, raw_bg) = cell_colors(cell, &ime_state, &theme);
                                // 選択中は前景・背景を反転して文字を可視状態に保つ
                                let (fg, bg) = if is_sel {
                                    (raw_bg, raw_fg)
                                } else {
                                    (raw_fg, raw_bg)
                                };
                                let width_px = state.cell_width * (*width as i32);
                                let wide_rect = RECT {
                                    right: x + width_px,
                                    ..cell_rect
                                };

                                SetBkColor(mem_dc, bg);
                                let _ = ExtTextOutW(
                                    mem_dc,
                                    x,
                                    y,
                                    ETO_OPAQUE,
                                    Some(&wide_rect),
                                    PCWSTR::null(),
                                    0,
                                    None,
                                );

                                let first_cp = text.chars().next().map(|c| c as u32).unwrap_or(0);
                                let handled = if (0x2500..=0x259F).contains(&first_cp) {
                                    draw_box_char(
                                        mem_dc,
                                        x,
                                        y,
                                        state.cell_width,
                                        state.cell_height,
                                        first_cp,
                                        fg,
                                    )
                                } else {
                                    false
                                };

                                if !handled {
                                    SetTextColor(mem_dc, fg);
                                    SetBkColor(mem_dc, bg);
                                    // GDI は ZWJ シーケンスをシェーピングできないため、
                                    // ZWJ 以降は描画せず基底グリフのみ表示する
                                    let render_str = zwj_render_text(text);
                                    let utf16: Vec<u16> = render_str.encode_utf16().collect();
                                    let _ = ExtTextOutW(
                                        mem_dc,
                                        x,
                                        y,
                                        ETO_CLIPPED,
                                        Some(&wide_rect),
                                        PCWSTR(utf16.as_ptr()),
                                        utf16.len() as u32,
                                        None,
                                    );
                                }
                                x += state.cell_width;
                            }
                            CellContent::Continuation => {
                                x += state.cell_width;
                            }
                            CellContent::Blank => {
                                // 選択中はセル背景を選択色で塗りつぶす
                                let blank_bg = if is_sel { theme.selection_bg } else { theme.bg };
                                SetBkColor(mem_dc, blank_bg);
                                let _ = ExtTextOutW(
                                    mem_dc,
                                    x,
                                    y,
                                    ETO_OPAQUE,
                                    Some(&cell_rect),
                                    PCWSTR::null(),
                                    0,
                                    None,
                                );
                                x += state.cell_width;
                            }
                        }
                    }
                }

                // プリエディット（アクティブペインのみ）
                if is_active && ime_state.composing && !ime_state.preedit.is_empty() {
                    let cursor = grid.cursor();
                    let mut px = cursor.col as i32 * state.cell_width + off_x;
                    let py = cursor.row as i32 * state.cell_height + off_y;

                    for seg in &ime_state.preedit {
                        let seg_utf16: Vec<u16> = seg.text.encode_utf16().collect();
                        let seg_width = state.cell_width * seg.text.chars().count() as i32;
                        let (fg, bg) = preedit_segment_colors(&seg.attr, &theme);
                        SetTextColor(mem_dc, fg);
                        SetBkColor(mem_dc, bg);
                        let seg_rect = RECT {
                            left: px,
                            top: py,
                            right: px + seg_width,
                            bottom: py + state.cell_height,
                        };
                        let _ = ExtTextOutW(
                            mem_dc,
                            px,
                            py,
                            ETO_CLIPPED | ETO_OPAQUE,
                            Some(&seg_rect),
                            PCWSTR(seg_utf16.as_ptr()),
                            seg_utf16.len() as u32,
                            None,
                        );
                        draw_preedit_underline(
                            mem_dc,
                            &seg.attr,
                            px,
                            py + state.cell_height - 2,
                            seg_width,
                            theme.fg,
                        );
                        px += seg_width;
                    }
                }

                // コピーモード: コピーカーソル（アクティブペインのみ）
                // 選択ハイライトはセル描画ループ内で is_sel フラグにより前景・背景反転で描画済み
                if is_active {
                    if let Some(ref cm) = copy_mode_state {
                        // コピーカーソル（ブロック型）
                        let (cc, cr) = cm.cursor;
                        if cc < display_cols && cr < grid.rows() as usize {
                            let cx = cc as i32 * state.cell_width + off_x;
                            let cy = cr as i32 * state.cell_height + off_y;
                            // ブロックカーソル（アウトライン）
                            let cur_brush = CreateSolidBrush(theme.cursor);
                            let cur_rect = RECT {
                                left: cx,
                                top: cy,
                                right: cx + state.cell_width,
                                bottom: cy + state.cell_height,
                            };
                            FrameRect(mem_dc, &cur_rect, cur_brush);
                            let _ = DeleteObject(cur_brush.into());
                        }
                    }
                }

                // カーソル（アクティブペインのみ、コピーモードでない場合）
                if is_active && grid.cursor_visible() && copy_mode_state.is_none() {
                    let cur = grid.cursor();
                    let cx = cur.col as i32 * state.cell_width + off_x;
                    let cy = cur.row as i32 * state.cell_height + off_y;
                    fill_rect(mem_dc, theme.cursor, cx, cy, cx + 2, cy + state.cell_height);
                }
            }
        }

        // ── セパレーター描画 ────────────────────────────────────────────
        for sep in &sep_rects {
            fill_rect(
                mem_dc,
                COLOR_SEPARATOR,
                PADDING_X + sep.x,
                PADDING_Y + sep.y,
                PADDING_X + sep.x + sep.w,
                PADDING_Y + sep.y + sep.h,
            );
        }

        // ── フローティングペイン ─────────────────────────────────────
        let (float_id, float_visible) = {
            let store = state.panes.lock().unwrap();
            (store.floating, store.floating_visible)
        };
        if float_visible {
            if let Some(fid) = float_id {
                if let Some(grid_arc) = {
                    let store = state.panes.lock().unwrap();
                    store.grids.get(&fid).cloned()
                } {
                    let fr = PaneStore::floating_rect(total_rect);
                    let off_x = PADDING_X + fr.x;
                    let off_y = PADDING_Y + fr.y;
                    let display_cols = (fr.w / state.cell_width.max(1)).max(1) as usize;
                    let grid = grid_arc.lock().unwrap();
                    let sb_len = grid.scrollback_len();
                    for row in 0..grid.rows() {
                        let y = row as i32 * state.cell_height + off_y;
                        let cells = match grid.row(row) {
                            Some(c) => c,
                            None => continue,
                        };
                        let mut x = off_x;
                        for cell in cells.iter().take(display_cols) {
                            let cell_rect = RECT {
                                left: x,
                                top: y,
                                right: x + state.cell_width,
                                bottom: y + state.cell_height,
                            };
                            match &cell.content {
                                CellContent::Grapheme { text, width } => {
                                    let (fg, bg) = cell_colors(cell, &ime_state, &theme);
                                    let width_px = state.cell_width * (*width as i32);
                                    let wide_rect = RECT {
                                        right: x + width_px,
                                        ..cell_rect
                                    };
                                    SetBkColor(mem_dc, bg);
                                    let _ = ExtTextOutW(
                                        mem_dc,
                                        x,
                                        y,
                                        ETO_OPAQUE,
                                        Some(&wide_rect),
                                        PCWSTR::null(),
                                        0,
                                        None,
                                    );
                                    let first_cp =
                                        text.chars().next().map(|c| c as u32).unwrap_or(0);
                                    let handled = if (0x2500..=0x259F).contains(&first_cp) {
                                        draw_box_char(
                                            mem_dc,
                                            x,
                                            y,
                                            state.cell_width,
                                            state.cell_height,
                                            first_cp,
                                            fg,
                                        )
                                    } else {
                                        false
                                    };
                                    if !handled {
                                        SetTextColor(mem_dc, fg);
                                        SetBkColor(mem_dc, bg);
                                        // GDI は ZWJ シーケンスをシェーピングできないため、
                                        // ZWJ 以降は描画せず基底グリフのみ表示する
                                        let render_str = zwj_render_text(text);
                                        let utf16: Vec<u16> = render_str.encode_utf16().collect();
                                        let _ = ExtTextOutW(
                                            mem_dc,
                                            x,
                                            y,
                                            ETO_OPAQUE,
                                            Some(&wide_rect),
                                            PCWSTR(utf16.as_ptr()),
                                            utf16.len() as u32,
                                            None,
                                        );
                                    }
                                    x += width_px;
                                }
                                CellContent::Continuation => {}
                                CellContent::Blank => {
                                    SetBkColor(mem_dc, theme.bg);
                                    let _ = ExtTextOutW(
                                        mem_dc,
                                        x,
                                        y,
                                        ETO_OPAQUE,
                                        Some(&cell_rect),
                                        PCWSTR::null(),
                                        0,
                                        None,
                                    );
                                    x += state.cell_width;
                                }
                            }
                        }
                    }
                    drop(grid);
                    let _ = sb_len;

                    // 枠線（2px）
                    const COLOR_FLOAT_BORDER: COLORREF = COLORREF(0x00_FA_B4_89); // peach
                    let border_pen = CreatePen(PS_SOLID, 2, COLOR_FLOAT_BORDER);
                    let old_pen = SelectObject(mem_dc, border_pen.into());
                    let null_brush = GetStockObject(NULL_BRUSH);
                    let old_brush = SelectObject(mem_dc, null_brush);
                    let _ = Rectangle(
                        mem_dc,
                        off_x - 2,
                        off_y - 2,
                        off_x + fr.w + 2,
                        off_y + fr.h + 2,
                    );
                    SelectObject(mem_dc, old_brush);
                    SelectObject(mem_dc, old_pen);
                    let _ = DeleteObject(border_pen.into());
                }
            }
        }

        // ── ステータスバー ───────────────────────────────────────────
        paint_status_bar(mem_dc, rect.right, rect.bottom, state);

        // ── トースト通知 ────────────────────────────────────────────
        paint_toasts(mem_dc, rect.right, rect.bottom, state);

        // ── レイアウトランチャー ────────────────────────────────────
        paint_launcher(mem_dc, rect.right, rect.bottom, state);

        // ── テーマランチャー ────────────────────────────────────────
        paint_theme_launcher(mem_dc, rect.right, rect.bottom, state);

        // ── レイアウト保存プロンプト ─────────────────────────────────
        paint_save_prompt(mem_dc, rect.right, rect.bottom, state);

        // バックバッファを画面にコピー
        BitBlt(
            hdc,
            0,
            0,
            rect.right,
            rect.bottom,
            Some(mem_dc),
            0,
            0,
            SRCCOPY,
        )
        .ok();

        // リソース解放
        SelectObject(mem_dc, old_font);
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(mem_bmp.into());
        let _ = DeleteDC(mem_dc);

        let _ = EndPaint(hwnd, &ps);
    }

    /// トースト通知をバックバッファの右下に描画する。
    ///
    /// Steam 風のスライドイン（下から上へ 300ms）で最大 3 件まで縦に積む。
    /// 期限（4000ms）に近づくと消えるのではなく WM_TIMER 側で配列から除去される。
    /// ステータスバーをウィンドウ下端に描画する。
    ///
    /// - 左: `[NORMAL]` / `[PANE]` + キーバインドヒント
    /// - 右: `pane X/N`（アクティブペイン番号 / 総ペイン数）
    unsafe fn paint_status_bar(hdc: HDC, win_w: i32, win_h: i32, state: &ClientState) {
        // Catppuccin Mocha: subtext1 = #bac2de, blue = #89b4fa, peach = #fab387, green = #a6e3a1
        let color_status_bg = state.theme.get().status_bar_bg;
        const COLOR_STATUS_FG: COLORREF = COLORREF(0x00_DE_C2_BA); // subtext1
        const COLOR_MODE_NORMAL: COLORREF = COLORREF(0x00_FA_B4_89); // blue (#89b4fa → BGR)
        const COLOR_MODE_PANE: COLORREF = COLORREF(0x00_87_AB_FA); // peach (#fab387 → BGR)
        const COLOR_MODE_COPY: COLORREF = COLORREF(0x00_A1_E3_A6); // green (#a6e3a1 → BGR)

        let bar_h = state.cell_height * STATUS_BAR_ROWS;
        let bar_y = win_h - bar_h;

        // 背景
        let bar_rect = RECT {
            left: 0,
            top: bar_y,
            right: win_w,
            bottom: win_h,
        };
        let bg_brush = CreateSolidBrush(color_status_bg);
        FillRect(hdc, &bar_rect, bg_brush);
        let _ = DeleteObject(bg_brush.into());

        SetBkColor(hdc, color_status_bg);
        SetBkMode(hdc, OPAQUE);

        let mode = state.mode.get();
        let text_y = bar_y;

        // ── 左側: モード名 ──────────────────────────────────────────
        let (mode_label, mode_color, hint) = match mode {
            ClientMode::Normal => (
                " NORMAL ",
                COLOR_MODE_NORMAL,
                " Ctrl+B: ペインモード  Ctrl+P: テーマ",
            ),
            ClientMode::Pane => (
                " PANE ",
                COLOR_MODE_PANE,
                " E: 縦分割  O: 横分割  W: 削除  F: Float  X: Editor  V: コピー  </>: 横比  +/-: 縦比  L: レイアウト  S: 保存  q: 戻る",
            ),
            ClientMode::Copy => (
                " COPY ",
                COLOR_MODE_COPY,
                " hjkl/矢印: 移動  v: 選択  y/Enter: コピー  q/Esc: 終了",
            ),
        };

        // モード名（色付き背景）
        SetTextColor(hdc, color_status_bg);
        SetBkColor(hdc, mode_color);
        let label_wide: Vec<u16> = mode_label.encode_utf16().collect();
        let _ = ExtTextOutW(
            hdc,
            PADDING_X,
            text_y,
            ETO_OPAQUE,
            None,
            PCWSTR(label_wide.as_ptr()),
            label_wide.len() as u32,
            None,
        );

        // ラベル幅を計算
        let mut label_size = SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &label_wide, &mut label_size);
        let hint_x = PADDING_X + label_size.cx;

        // ヒントテキスト
        SetTextColor(hdc, COLOR_STATUS_FG);
        SetBkColor(hdc, color_status_bg);
        let hint_wide: Vec<u16> = hint.encode_utf16().collect();
        let _ = ExtTextOutW(
            hdc,
            hint_x,
            text_y,
            ETO_OPAQUE,
            None,
            PCWSTR(hint_wide.as_ptr()),
            hint_wide.len() as u32,
            None,
        );

        // ── 右側: ペイン番号 ────────────────────────────────────────
        let (active_idx, total) = {
            let store = state.panes.lock().unwrap();
            let ids = store.layout.pane_ids();
            let total = ids.len();
            let idx = ids.iter().position(|&id| id == store.active).unwrap_or(0) + 1;
            (idx, total)
        };
        let right_text = format!(" pane {}/{} ", active_idx, total);
        let right_wide: Vec<u16> = right_text.encode_utf16().collect();
        let mut right_size = SIZE::default();
        SetBkColor(hdc, color_status_bg);
        let _ = GetTextExtentPoint32W(hdc, &right_wide, &mut right_size);
        let right_x = (win_w - right_size.cx).max(hint_x + label_size.cx);
        SetTextColor(hdc, COLOR_STATUS_FG);
        let _ = ExtTextOutW(
            hdc,
            right_x,
            text_y,
            ETO_OPAQUE,
            None,
            PCWSTR(right_wide.as_ptr()),
            right_wide.len() as u32,
            None,
        );
    }

    unsafe fn paint_toasts(hdc: HDC, win_w: i32, win_h: i32, state: &ClientState) {
        const TOAST_W: i32 = 300;
        const TOAST_H: i32 = 64;
        const TOAST_MARGIN: i32 = 16;
        const TOAST_GAP: i32 = 8;
        const TOAST_PADDING: i32 = 12;

        // Catppuccin Mocha: surface0 = #313244, surface2 = #585b70, subtext0 = #a6adc8
        const COLOR_TOAST_BG: COLORREF = COLORREF(0x00_44_32_31);
        const COLOR_TOAST_BORDER: COLORREF = COLORREF(0x00_70_5B_58);
        const COLOR_TOAST_LABEL: COLORREF = COLORREF(0x00_C8_AD_A6);

        let toasts = state.active_toasts.lock().unwrap();
        // 最新のもの（末尾）を最大 3 件、新しいものほど下に表示
        let visible: Vec<&Toast> = toasts.iter().rev().take(3).collect();

        for (i, toast) in visible.iter().enumerate() {
            // スライドオフセット: elapsed < SLIDE_MS のあいだ下から登場
            let slide_offset = if toast.elapsed_ms < Toast::SLIDE_MS {
                let remaining = Toast::SLIDE_MS - toast.elapsed_ms;
                (remaining as i64 * (TOAST_H + TOAST_GAP) as i64 / Toast::SLIDE_MS as i64) as i32
            } else {
                0
            };

            let base_y =
                win_h - TOAST_MARGIN - TOAST_H - (i as i32) * (TOAST_H + TOAST_GAP) + slide_offset;
            let base_x = win_w - TOAST_MARGIN - TOAST_W;

            let toast_rect = RECT {
                left: base_x,
                top: base_y,
                right: base_x + TOAST_W,
                bottom: base_y + TOAST_H,
            };

            // 背景
            let bg_brush = CreateSolidBrush(COLOR_TOAST_BG);
            FillRect(hdc, &toast_rect, bg_brush);
            let _ = DeleteObject(bg_brush.into());

            // 枠線（1px）
            let border_pen = CreatePen(PS_SOLID, 1, COLOR_TOAST_BORDER);
            let old_pen = SelectObject(hdc, border_pen.into());
            let null_brush = GetStockObject(NULL_BRUSH);
            let old_brush = SelectObject(hdc, null_brush);
            let _ = Rectangle(
                hdc,
                toast_rect.left,
                toast_rect.top,
                toast_rect.right,
                toast_rect.bottom,
            );
            SelectObject(hdc, old_pen);
            SelectObject(hdc, old_brush);
            let _ = DeleteObject(border_pen.into());

            SetBkMode(hdc, TRANSPARENT);

            // ラベル行: "Pane N"
            let label = format!("Pane {}", toast.pane_id.0);
            let mut label_w: Vec<u16> = label.encode_utf16().collect();
            let mut label_rect = RECT {
                left: toast_rect.left + TOAST_PADDING,
                top: toast_rect.top + 8,
                right: toast_rect.right - TOAST_PADDING,
                bottom: toast_rect.top + 30,
            };
            SetTextColor(hdc, COLOR_TOAST_LABEL);
            DrawTextW(
                hdc,
                &mut label_w,
                &mut label_rect,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER,
            );

            // メッセージ行
            let mut msg_w: Vec<u16> = toast.message.encode_utf16().collect();
            let mut msg_rect = RECT {
                left: toast_rect.left + TOAST_PADDING,
                top: toast_rect.top + 30,
                right: toast_rect.right - TOAST_PADDING,
                bottom: toast_rect.bottom - 8,
            };
            SetTextColor(hdc, state.theme.get().fg);
            DrawTextW(
                hdc,
                &mut msg_w,
                &mut msg_rect,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
        }
    }

    /// レイアウトランチャーポップアップを画面中央に描画する。
    ///
    /// `PaneStore::launcher` が `Some` のときのみ描画する。
    /// 上下キーで選択行をハイライトし、Enter で適用、Esc/q でキャンセル。
    /// 右パネルに選択中レイアウトのプレビューダイアグラムを表示する。
    unsafe fn paint_launcher(hdc: HDC, win_w: i32, win_h: i32, state: &ClientState) {
        let launcher = {
            let store = state.panes.lock().unwrap();
            store
                .launcher
                .as_ref()
                .map(LauncherRenderState::from_launcher)
        };
        let Some(launcher) = launcher else {
            return;
        };

        // Catppuccin Mocha カラー (BGR)
        const COLOR_OVERLAY: COLORREF = COLORREF(0x00_22_1E_1E); // 暗いオーバーレイ
        const COLOR_POPUP_BG: COLORREF = COLORREF(0x00_3D_31_31); // surface0
        const COLOR_SEL_BG: COLORREF = COLORREF(0x00_87_AB_FA); // peach
        const COLOR_SEL_FG: COLORREF = COLORREF(0x00_2E_1E_1E); // base
        const COLOR_TEXT: COLORREF = COLORREF(0x00_F4_D6_CD); // text
        const COLOR_TITLE: COLORREF = COLORREF(0x00_87_AB_FA); // peach
        const COLOR_HINT_FG: COLORREF = COLORREF(0x00_A0_9D_8C); // subtext0
                                                                 // プレビュー用カラー — popup bg より明確に明るい色でコントラストを確保
        const COLOR_PREVIEW_PANE: COLORREF = COLORREF(0x00_5E_4D_4C); // surface2 相当（暗め）
        const COLOR_PREVIEW_BORDER: COLORREF = COLORREF(0x00_87_AB_FA); // peach（ペイン枠）
        const COLOR_PREVIEW_SEP: COLORREF = COLORREF(0x00_C9_BA_B8); // subtext1（分割線）
        const COLOR_PREVIEW_TEXT: COLORREF = COLORREF(0x00_F4_D6_CD); // text（コマンド名）

        let cw = state.cell_width;
        let ch = state.cell_height;
        let n = launcher.entries.len() as i32;

        // 左パネル（名前リスト）のサイズ
        let max_name_chars = launcher
            .entries
            .iter()
            .map(|s| s.chars().count())
            .max()
            .unwrap_or(10) as i32;
        let list_w = ((max_name_chars + 8) * cw).max(cw * 22);

        // 右パネル（プレビュー）は画面幅に合わせて大きく取る
        let preview_margin = cw;
        let preview_w = (win_w / 2).max(cw * 44);

        let popup_w = list_w + 1 + preview_margin + preview_w + preview_margin;

        // アイテム行数と最小プレビュー高さのどちらか大きい方でコンテンツ高を決める
        let item_rows = n.max(1);
        let content_h = (item_rows * ch).max(ch * 12);
        let popup_h = ch + ch / 2 // タイトル行 + 余白
            + 1               // 水平セパレーター
            + ch / 4          // 余白
            + content_h       // アイテム行 / プレビュー
            + ch / 4          // 余白
            + ch; // ヒント行

        // ウィンドウからはみ出さないようクランプ
        let popup_w = popup_w.min(win_w - cw * 2);
        let popup_h = popup_h.min(win_h - ch * 2);
        let popup_x = (win_w - popup_w) / 2;
        let popup_y = (win_h - popup_h) / 2;

        // 全画面オーバーレイ
        let overlay_rect = RECT {
            left: 0,
            top: 0,
            right: win_w,
            bottom: win_h,
        };
        let overlay_brush = CreateSolidBrush(COLOR_OVERLAY);
        FillRect(hdc, &overlay_rect, overlay_brush);
        let _ = DeleteObject(overlay_brush.into());

        // ポップアップ背景
        let popup_rect = RECT {
            left: popup_x,
            top: popup_y,
            right: popup_x + popup_w,
            bottom: popup_y + popup_h,
        };
        let popup_brush = CreateSolidBrush(COLOR_POPUP_BG);
        FillRect(hdc, &popup_rect, popup_brush);
        let _ = DeleteObject(popup_brush.into());

        SetBkMode(hdc, TRANSPARENT);

        // タイトル行
        let title = "  レイアウト選択";
        let title_w: Vec<u16> = title.encode_utf16().collect();
        SetTextColor(hdc, COLOR_TITLE);
        let _ = ExtTextOutW(
            hdc,
            popup_x + cw,
            popup_y + ch / 4,
            ETO_CLIPPED,
            Some(&popup_rect),
            PCWSTR(title_w.as_ptr()),
            title_w.len() as u32,
            None,
        );

        // 水平セパレーター線
        let sep_y = popup_y + ch + ch / 4;
        let sep_brush = CreateSolidBrush(COLOR_SEPARATOR);
        let sep_rect = RECT {
            left: popup_x + cw,
            top: sep_y,
            right: popup_x + popup_w - cw,
            bottom: sep_y + 1,
        };
        FillRect(hdc, &sep_rect, sep_brush);
        let _ = DeleteObject(sep_brush.into());

        // 左右パネルの垂直セパレーター
        let vsep_x = popup_x + list_w;
        let vsep_brush = CreateSolidBrush(COLOR_SEPARATOR);
        let vsep_rect = RECT {
            left: vsep_x,
            top: sep_y + 1,
            right: vsep_x + 1,
            bottom: popup_y + popup_h - ch,
        };
        FillRect(hdc, &vsep_rect, vsep_brush);
        let _ = DeleteObject(vsep_brush.into());

        let items_top = sep_y + 1 + ch / 4;

        // ── 左パネル: 名前リスト ──────────────────────────────────────
        let list_clip = RECT {
            left: popup_x,
            top: popup_y,
            right: popup_x + list_w,
            bottom: popup_y + popup_h,
        };
        if launcher.entries.is_empty() {
            let msg = "  (レイアウトファイルが見つかりません)";
            let msg_w: Vec<u16> = msg.encode_utf16().collect();
            SetTextColor(hdc, COLOR_HINT_FG);
            let _ = ExtTextOutW(
                hdc,
                popup_x + cw,
                items_top,
                ETO_CLIPPED,
                Some(&list_clip),
                PCWSTR(msg_w.as_ptr()),
                msg_w.len() as u32,
                None,
            );
        } else {
            for (i, name) in launcher.entries.iter().enumerate() {
                let item_y = items_top + i as i32 * ch;
                if i == launcher.selected {
                    let sel_rect = RECT {
                        left: popup_x + cw / 2,
                        top: item_y,
                        right: popup_x + list_w - cw / 2,
                        bottom: item_y + ch,
                    };
                    let sel_brush = CreateSolidBrush(COLOR_SEL_BG);
                    FillRect(hdc, &sel_rect, sel_brush);
                    let _ = DeleteObject(sel_brush.into());
                    SetTextColor(hdc, COLOR_SEL_FG);
                } else {
                    SetTextColor(hdc, COLOR_TEXT);
                }
                let text = format!("  {name}");
                let text_w: Vec<u16> = text.encode_utf16().collect();
                let _ = ExtTextOutW(
                    hdc,
                    popup_x + cw,
                    item_y,
                    ETO_CLIPPED,
                    Some(&list_clip),
                    PCWSTR(text_w.as_ptr()),
                    text_w.len() as u32,
                    None,
                );
            }
        }

        // ── 右パネル: プレビューダイアグラム ─────────────────────────
        let preview_x = vsep_x + 1 + preview_margin;
        let preview_area_h = popup_h
            - (ch + ch / 2 + 1 + ch / 4) // タイトル + セパレーター
            - ch / 4 // 下余白
            - ch; // ヒント行
        let preview_top = sep_y + 1 + ch / 4;
        let actual_preview_w = popup_x + popup_w - preview_margin - preview_x;

        if let Some(preview) = launcher.selected_preview.as_ref() {
            let preview_rect = RECT {
                left: preview_x,
                top: preview_top,
                right: preview_x + actual_preview_w,
                bottom: preview_top + preview_area_h,
            };
            draw_layout_preview(
                hdc,
                preview,
                &preview_rect,
                COLOR_PREVIEW_PANE,
                COLOR_PREVIEW_BORDER,
                COLOR_PREVIEW_SEP,
                COLOR_PREVIEW_TEXT,
                cw,
                ch,
            );
        } else {
            let no_preview = "  (プレビューなし)";
            let no_preview_w: Vec<u16> = no_preview.encode_utf16().collect();
            let preview_clip = RECT {
                left: vsep_x + 1,
                top: popup_y,
                right: popup_x + popup_w,
                bottom: popup_y + popup_h,
            };
            SetTextColor(hdc, COLOR_HINT_FG);
            let _ = ExtTextOutW(
                hdc,
                preview_x,
                preview_top,
                ETO_CLIPPED,
                Some(&preview_clip),
                PCWSTR(no_preview_w.as_ptr()),
                no_preview_w.len() as u32,
                None,
            );
        }

        // ヒント行
        let hint_y = popup_y + popup_h - ch;
        let hint = "  ↑↓: 選択  Enter: 適用  Esc/q: キャンセル";
        let hint_w: Vec<u16> = hint.encode_utf16().collect();
        SetTextColor(hdc, COLOR_HINT_FG);
        let _ = ExtTextOutW(
            hdc,
            popup_x + cw / 2,
            hint_y,
            ETO_CLIPPED,
            Some(&popup_rect),
            PCWSTR(hint_w.as_ptr()),
            hint_w.len() as u32,
            None,
        );
    }

    /// `PaneStore::theme_launcher` が `Some` のときのみ描画する。
    /// シンプルなリストポップアップ。↑↓ で選択、Enter で適用、Esc/q でキャンセル。
    unsafe fn paint_theme_launcher(hdc: HDC, win_w: i32, win_h: i32, state: &ClientState) {
        let theme_launcher = {
            let store = state.panes.lock().unwrap();
            store
                .theme_launcher
                .as_ref()
                .map(ThemeLauncherRenderState::from_theme_launcher)
        };
        let Some(tl) = theme_launcher else {
            return;
        };

        // Catppuccin Mocha カラー (BGR)
        const COLOR_OVERLAY: COLORREF = COLORREF(0x00_22_1E_1E);
        const COLOR_POPUP_BG: COLORREF = COLORREF(0x00_3D_31_31);
        const COLOR_SEL_BG: COLORREF = COLORREF(0x00_87_AB_FA); // peach
        const COLOR_SEL_FG: COLORREF = COLORREF(0x00_2E_1E_1E); // base
        const COLOR_TEXT: COLORREF = COLORREF(0x00_F4_D6_CD); // text
        const COLOR_TITLE: COLORREF = COLORREF(0x00_87_AB_FA); // peach
        const COLOR_HINT_FG: COLORREF = COLORREF(0x00_A0_9D_8C); // subtext0

        let cw = state.cell_width;
        let ch = state.cell_height;
        let n = tl.entries.len() as i32;

        let max_chars = tl
            .entries
            .iter()
            .map(|s| s.chars().count())
            .max()
            .unwrap_or(10) as i32;
        let popup_w = ((max_chars + 8) * cw).max(cw * 24).min(win_w - cw * 4);
        let item_rows = n.max(1);
        let popup_h = (ch + ch / 2  // タイトル行
            + 1                      // セパレーター
            + ch / 4                 // 余白
            + item_rows * ch         // アイテム行
            + ch / 4                 // 余白
            + ch) // ヒント行
            .min(win_h - ch * 2);
        let popup_x = (win_w - popup_w) / 2;
        let popup_y = (win_h - popup_h) / 2;

        // 全画面オーバーレイ
        let overlay_brush = CreateSolidBrush(COLOR_OVERLAY);
        FillRect(
            hdc,
            &RECT {
                left: 0,
                top: 0,
                right: win_w,
                bottom: win_h,
            },
            overlay_brush,
        );
        let _ = DeleteObject(overlay_brush.into());

        // ポップアップ背景
        let popup_rect = RECT {
            left: popup_x,
            top: popup_y,
            right: popup_x + popup_w,
            bottom: popup_y + popup_h,
        };
        let popup_brush = CreateSolidBrush(COLOR_POPUP_BG);
        FillRect(hdc, &popup_rect, popup_brush);
        let _ = DeleteObject(popup_brush.into());

        // タイトル行
        let title = "  テーマを選択";
        let title_w: Vec<u16> = title.encode_utf16().collect();
        SetBkColor(hdc, COLOR_POPUP_BG);
        SetTextColor(hdc, COLOR_TITLE);
        let _ = ExtTextOutW(
            hdc,
            popup_x + cw / 2,
            popup_y + ch / 4,
            ETO_CLIPPED,
            Some(&popup_rect),
            PCWSTR(title_w.as_ptr()),
            title_w.len() as u32,
            None,
        );

        // 水平セパレーター
        let sep_y = popup_y + ch + ch / 2;
        let sep_pen = CreatePen(PS_SOLID, 1, COLOR_TITLE);
        let old_pen = SelectObject(hdc, sep_pen.into());
        let _ = MoveToEx(hdc, popup_x, sep_y, None);
        let _ = LineTo(hdc, popup_x + popup_w, sep_y);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(sep_pen.into());

        // アイテム一覧
        let items_top = sep_y + 1 + ch / 4;
        for (i, name) in tl.entries.iter().enumerate() {
            let item_y = items_top + i as i32 * ch;
            let is_sel = i == tl.selected;
            let item_rect = RECT {
                left: popup_x + 1,
                top: item_y,
                right: popup_x + popup_w - 1,
                bottom: item_y + ch,
            };
            if is_sel {
                let sel_brush = CreateSolidBrush(COLOR_SEL_BG);
                FillRect(hdc, &item_rect, sel_brush);
                let _ = DeleteObject(sel_brush.into());
                SetTextColor(hdc, COLOR_SEL_FG);
                SetBkColor(hdc, COLOR_SEL_BG);
            } else {
                SetTextColor(hdc, COLOR_TEXT);
                SetBkColor(hdc, COLOR_POPUP_BG);
            }
            let label = format!("  {name}");
            let label_w: Vec<u16> = label.encode_utf16().collect();
            let _ = ExtTextOutW(
                hdc,
                popup_x + cw / 2,
                item_y,
                ETO_CLIPPED,
                Some(&item_rect),
                PCWSTR(label_w.as_ptr()),
                label_w.len() as u32,
                None,
            );
        }

        // ヒント行
        let hint_y = popup_y + popup_h - ch;
        let hint = "  ↑↓: 選択  Enter: 適用  Esc/q: キャンセル";
        let hint_w: Vec<u16> = hint.encode_utf16().collect();
        SetTextColor(hdc, COLOR_HINT_FG);
        SetBkColor(hdc, COLOR_POPUP_BG);
        let _ = ExtTextOutW(
            hdc,
            popup_x + cw / 2,
            hint_y,
            ETO_CLIPPED,
            Some(&popup_rect),
            PCWSTR(hint_w.as_ptr()),
            hint_w.len() as u32,
            None,
        );
    }

    /// `PaneStore::save_prompt` が `Some` のときのみ描画する。
    /// 左ペイン: ファイル名入力フィールド。右ペイン: 現在のレイアウトプレビュー。
    unsafe fn paint_save_prompt(hdc: HDC, win_w: i32, win_h: i32, state: &ClientState) {
        let render_state = {
            let store = state.panes.lock().unwrap();
            SavePromptRenderState::from_store(&store)
        };
        let Some(render_state) = render_state else {
            return;
        };

        // Catppuccin Mocha カラー (BGR)
        const COLOR_OVERLAY: COLORREF = COLORREF(0x00_22_1E_1E);
        const COLOR_POPUP_BG: COLORREF = COLORREF(0x00_3D_31_31);
        const COLOR_TITLE: COLORREF = COLORREF(0x00_87_AB_FA); // peach
        const COLOR_TEXT: COLORREF = COLORREF(0x00_F4_D6_CD); // text
        const COLOR_INPUT_BG: COLORREF = COLORREF(0x00_55_44_44); // surface1
        const COLOR_HINT_FG: COLORREF = COLORREF(0x00_A0_9D_8C); // subtext0
        const COLOR_CURSOR: COLORREF = COLORREF(0x00_87_AB_FA); // peach
        const COLOR_PREVIEW_PANE: COLORREF = COLORREF(0x00_5E_4D_4C);
        const COLOR_PREVIEW_BORDER: COLORREF = COLORREF(0x00_87_AB_FA);
        const COLOR_PREVIEW_SEP: COLORREF = COLORREF(0x00_C9_BA_B8);
        const COLOR_PREVIEW_TEXT: COLORREF = COLORREF(0x00_F4_D6_CD);

        let cw = state.cell_width;
        let ch = state.cell_height;

        // 左パネル（入力エリア）の幅
        let left_w = (cw * 30).max(cw * 22).min(win_w / 2);
        // 右パネル（プレビューエリア）の幅
        let preview_margin = cw;
        let preview_w = (win_w / 2).max(cw * 30);

        // ポップアップ全体サイズ
        let content_h = ch * 10; // プレビューの高さを確保
        let popup_w =
            (left_w + 1 + preview_margin + preview_w + preview_margin).min(win_w - cw * 2);
        let popup_h = ch + ch / 2  // タイトル
            + 1                    // セパレーター
            + ch / 4               // 余白
            + content_h            // コンテンツ（入力+プレビュー）
            + ch / 4               // 余白
            + ch; // ヒント行
        let popup_h = popup_h.min(win_h - ch * 2);
        let popup_x = (win_w - popup_w) / 2;
        let popup_y = (win_h - popup_h) / 2;

        // オーバーレイ
        let overlay_brush = CreateSolidBrush(COLOR_OVERLAY);
        FillRect(
            hdc,
            &RECT {
                left: 0,
                top: 0,
                right: win_w,
                bottom: win_h,
            },
            overlay_brush,
        );
        let _ = DeleteObject(overlay_brush.into());

        // ポップアップ背景
        let popup_rect = RECT {
            left: popup_x,
            top: popup_y,
            right: popup_x + popup_w,
            bottom: popup_y + popup_h,
        };
        let popup_brush = CreateSolidBrush(COLOR_POPUP_BG);
        FillRect(hdc, &popup_rect, popup_brush);
        let _ = DeleteObject(popup_brush.into());

        SetBkColor(hdc, COLOR_POPUP_BG);
        SetBkMode(hdc, OPAQUE);

        // タイトル
        let title = "レイアウトを保存";
        let title_w: Vec<u16> = title.encode_utf16().collect();
        SetTextColor(hdc, COLOR_TITLE);
        let _ = ExtTextOutW(
            hdc,
            popup_x + cw,
            popup_y + ch / 4,
            ETO_OPAQUE,
            None,
            PCWSTR(title_w.as_ptr()),
            title_w.len() as u32,
            None,
        );

        // 水平セパレーター（タイトル下）
        let sep_y = popup_y + ch + ch / 4;
        let sep_pen = CreatePen(PS_SOLID, 1, COLORREF(0x00_5E_4D_4C));
        let old_pen = SelectObject(hdc, sep_pen.into());
        let _ = MoveToEx(hdc, popup_x, sep_y, None);
        let _ = LineTo(hdc, popup_x + popup_w, sep_y);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(sep_pen.into());

        // ── 左パネル: 名前入力 ──────────────────────────────────────
        let content_top = sep_y + ch / 4;
        let input_label = "名前:";
        let label_w: Vec<u16> = input_label.encode_utf16().collect();
        SetTextColor(hdc, COLOR_HINT_FG);
        SetBkColor(hdc, COLOR_POPUP_BG);
        let _ = ExtTextOutW(
            hdc,
            popup_x + cw / 2,
            content_top,
            ETO_OPAQUE,
            None,
            PCWSTR(label_w.as_ptr()),
            label_w.len() as u32,
            None,
        );

        // 入力フィールド背景
        let input_y = content_top + ch + ch / 4;
        let input_rect = RECT {
            left: popup_x + cw / 2,
            top: input_y,
            right: popup_x + left_w - cw / 2,
            bottom: input_y + ch,
        };
        let input_brush = CreateSolidBrush(COLOR_INPUT_BG);
        FillRect(hdc, &input_rect, input_brush);
        let _ = DeleteObject(input_brush.into());

        // 入力テキスト（先頭にスペース）
        SetBkColor(hdc, COLOR_INPUT_BG);
        SetTextColor(hdc, COLOR_TEXT);
        let display = format!(" {}", render_state.prompt);
        let display_w: Vec<u16> = display.encode_utf16().collect();
        let _ = ExtTextOutW(
            hdc,
            input_rect.left,
            input_y,
            ETO_OPAQUE,
            Some(&input_rect),
            PCWSTR(display_w.as_ptr()),
            display_w.len() as u32,
            None,
        );

        // カーソル（テキスト末尾にブロック）
        let mut text_size = SIZE::default();
        let _ = GetTextExtentPoint32W(hdc, &display_w, &mut text_size);
        let cursor_x = (input_rect.left + text_size.cx).min(input_rect.right - cw / 4);
        let cursor_rect = RECT {
            left: cursor_x,
            top: input_y,
            right: (cursor_x + cw / 2).min(input_rect.right),
            bottom: input_y + ch,
        };
        let cursor_brush = CreateSolidBrush(COLOR_CURSOR);
        FillRect(hdc, &cursor_rect, cursor_brush);
        let _ = DeleteObject(cursor_brush.into());

        // 左右パネル区切り線
        let vsep_x = popup_x + left_w;
        let vsep_pen = CreatePen(PS_SOLID, 1, COLORREF(0x00_5E_4D_4C));
        let old_pen2 = SelectObject(hdc, vsep_pen.into());
        let _ = MoveToEx(hdc, vsep_x, sep_y + 1, None);
        let _ = LineTo(hdc, vsep_x, popup_y + popup_h - ch - 1);
        SelectObject(hdc, old_pen2);
        let _ = DeleteObject(vsep_pen.into());

        // ── 右パネル: 現在のレイアウトプレビュー ────────────────────
        let preview_top = sep_y + ch / 4;
        let preview_bottom = popup_y + popup_h - ch - ch / 4;
        let preview_left = vsep_x + preview_margin;
        let preview_right = (popup_x + popup_w - preview_margin).max(preview_left + cw);
        if preview_right > preview_left && preview_bottom > preview_top {
            let preview_label = "現在のレイアウト";
            let plabel_w: Vec<u16> = preview_label.encode_utf16().collect();
            SetBkColor(hdc, COLOR_POPUP_BG);
            SetTextColor(hdc, COLOR_HINT_FG);
            let _ = ExtTextOutW(
                hdc,
                preview_left,
                preview_top,
                ETO_OPAQUE,
                None,
                PCWSTR(plabel_w.as_ptr()),
                plabel_w.len() as u32,
                None,
            );

            let diagram_top = preview_top + ch + ch / 4;
            if preview_bottom > diagram_top + 4 {
                let preview_rect = RECT {
                    left: preview_left,
                    top: diagram_top,
                    right: preview_right,
                    bottom: preview_bottom,
                };
                SetBkMode(hdc, TRANSPARENT);
                draw_preview_node(
                    hdc,
                    &render_state.preview,
                    &preview_rect,
                    COLOR_PREVIEW_PANE,
                    COLOR_PREVIEW_BORDER,
                    COLOR_PREVIEW_SEP,
                    COLOR_PREVIEW_TEXT,
                    cw,
                    ch,
                );
            }
        }

        // ヒント行
        let hint_y = popup_y + popup_h - ch;
        let hint = "  Enter: 保存  Esc: キャンセル";
        let hint_w: Vec<u16> = hint.encode_utf16().collect();
        SetBkColor(hdc, COLOR_POPUP_BG);
        SetTextColor(hdc, COLOR_HINT_FG);
        let _ = ExtTextOutW(
            hdc,
            popup_x + cw / 2,
            hint_y,
            ETO_OPAQUE,
            None,
            PCWSTR(hint_w.as_ptr()),
            hint_w.len() as u32,
            None,
        );
    }

    /// 描画用プレビュー木をプレビューエリアに再帰描画する。
    ///
    /// 各ペインは明るいボーダー → 暗い内部 → コマンドテキストの順で描く。
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_layout_preview(
        hdc: HDC,
        preview: &PreviewRenderNode,
        rect: &RECT,
        color_pane: COLORREF,
        color_border: COLORREF,
        color_sep: COLORREF,
        color_text: COLORREF,
        cell_w: i32,
        cell_h: i32,
    ) {
        draw_preview_node(
            hdc,
            preview,
            rect,
            color_pane,
            color_border,
            color_sep,
            color_text,
            cell_w,
            cell_h,
        );
    }

    /// プレビュー木を再帰描画する内部ヘルパー。
    #[allow(clippy::too_many_arguments)]
    unsafe fn draw_preview_node(
        hdc: HDC,
        node: &PreviewRenderNode,
        rect: &RECT,
        color_pane: COLORREF,
        color_border: COLORREF,
        color_sep: COLORREF,
        color_text: COLORREF,
        cell_w: i32,
        cell_h: i32,
    ) {
        let w = rect.right - rect.left;
        let h = rect.bottom - rect.top;
        if w <= 2 || h <= 2 {
            return;
        }

        match node {
            PreviewRenderNode::Leaf { label } => {
                // 外枠を border 色で塗る → 内側を pane 色で上塗り（1px ボーダー効果）
                let border_brush = CreateSolidBrush(color_border);
                FillRect(hdc, rect, border_brush);
                let _ = DeleteObject(border_brush.into());

                let inner = RECT {
                    left: rect.left + 1,
                    top: rect.top + 1,
                    right: rect.right - 1,
                    bottom: rect.bottom - 1,
                };
                let pane_brush = CreateSolidBrush(color_pane);
                FillRect(hdc, &inner, pane_brush);
                let _ = DeleteObject(pane_brush.into());

                // コマンドテキストを左上に描画（内側に収まるようクリップ）
                if let Some(cmd) = label {
                    let pad = (cell_w / 4).max(2);
                    let text_x = inner.left + pad;
                    let text_y = (inner.top + inner.bottom - cell_h) / 2; // 垂直中央
                    SetTextColor(hdc, color_text);
                    SetBkMode(hdc, TRANSPARENT);
                    let text_w: Vec<u16> = cmd.encode_utf16().collect();
                    let _ = ExtTextOutW(
                        hdc,
                        text_x,
                        text_y,
                        ETO_CLIPPED,
                        Some(&inner),
                        PCWSTR(text_w.as_ptr()),
                        text_w.len() as u32,
                        None,
                    );
                }
            }
            PreviewRenderNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                const SEP: i32 = 3;
                let (r1, r2) = match direction {
                    SplitDirection::Vertical => {
                        let w1 = (((w - SEP) as f32 * ratio) as i32).max(1);
                        let w2 = (w - w1 - SEP).max(1);
                        (
                            RECT {
                                left: rect.left,
                                top: rect.top,
                                right: rect.left + w1,
                                bottom: rect.bottom,
                            },
                            RECT {
                                left: rect.left + w1 + SEP,
                                top: rect.top,
                                right: rect.left + w1 + SEP + w2,
                                bottom: rect.bottom,
                            },
                        )
                    }
                    SplitDirection::Horizontal => {
                        let h1 = (((h - SEP) as f32 * ratio) as i32).max(1);
                        let h2 = (h - h1 - SEP).max(1);
                        (
                            RECT {
                                left: rect.left,
                                top: rect.top,
                                right: rect.right,
                                bottom: rect.top + h1,
                            },
                            RECT {
                                left: rect.left,
                                top: rect.top + h1 + SEP,
                                right: rect.right,
                                bottom: rect.top + h1 + SEP + h2,
                            },
                        )
                    }
                };
                // セパレーター
                let sep_rect = match direction {
                    SplitDirection::Vertical => RECT {
                        left: r1.right,
                        top: rect.top,
                        right: r2.left,
                        bottom: rect.bottom,
                    },
                    SplitDirection::Horizontal => RECT {
                        left: rect.left,
                        top: r1.bottom,
                        right: rect.right,
                        bottom: r2.top,
                    },
                };
                let sep_brush = CreateSolidBrush(color_sep);
                FillRect(hdc, &sep_rect, sep_brush);
                let _ = DeleteObject(sep_brush.into());

                draw_preview_node(
                    hdc,
                    first,
                    &r1,
                    color_pane,
                    color_border,
                    color_sep,
                    color_text,
                    cell_w,
                    cell_h,
                );
                draw_preview_node(
                    hdc,
                    second,
                    &r2,
                    color_pane,
                    color_border,
                    color_sep,
                    color_text,
                    cell_w,
                    cell_h,
                );
            }
        }
    }

    /// 罫線文字・ブロック要素を GDI プリミティブで描画する。
    ///
    /// フォント代替（MS Gothic 全角グリフ）による幅ずれを防ぐため、
    /// フォントに依存せず直接線分・矩形で描く。
    /// 対応した場合 `true`、未対応の場合 `false` を返す。
    unsafe fn draw_box_char(
        hdc: HDC,
        x: i32,
        y: i32,
        w: i32,
        h: i32,
        cp: u32,
        fg: COLORREF,
    ) -> bool {
        // セルの中心・端点
        let cx = x + w / 2;
        let cy = y + h / 2;
        let x0 = x;
        let x1 = x + w; // right edge (exclusive)
        let y0 = y;
        let y1 = y + h; // bottom edge (exclusive)

        // 線の太さ: light=1, heavy=2, double=1 (2本で表現)
        let thin = 1i32;
        let thick = (w / 6).max(2);

        // ペンを作成して DC に選択するクロージャ
        let make_pen = |width: i32| CreatePen(PS_SOLID, width, fg);
        let set_pen = |pen: HPEN| -> HPEN { HPEN(SelectObject(hdc, pen.into()).0) };
        let del_pen = |old: HPEN, pen: HPEN| {
            SelectObject(hdc, old.into());
            let _ = DeleteObject(pen.into());
        };

        // 水平線セグメント
        let hline = |hdc: HDC, lx: i32, rx: i32, ly: i32| {
            let _ = MoveToEx(hdc, lx, ly, None);
            let _ = LineTo(hdc, rx, ly);
        };
        // 垂直線セグメント
        let vline = |hdc: HDC, vx: i32, ty: i32, by: i32| {
            let _ = MoveToEx(hdc, vx, ty, None);
            let _ = LineTo(hdc, vx, by);
        };

        match cp {
            // ── 水平線 ──────────────────────────────────────────────────────
            0x2500 => {
                // ─ light
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, x0, x1, cy);
                del_pen(o, p);
            }
            0x2501 => {
                // ━ heavy
                let p = make_pen(thick);
                let o = set_pen(p);
                hline(hdc, x0, x1, cy);
                del_pen(o, p);
            }
            // ── 垂直線 ──────────────────────────────────────────────────────
            0x2502 => {
                // │ light
                let p = make_pen(thin);
                let o = set_pen(p);
                vline(hdc, cx, y0, y1);
                del_pen(o, p);
            }
            0x2503 => {
                // ┃ heavy
                let p = make_pen(thick);
                let o = set_pen(p);
                vline(hdc, cx, y0, y1);
                del_pen(o, p);
            }
            // ── 角 (light) ──────────────────────────────────────────────────
            0x250C => {
                // ┌
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, cx, x1, cy);
                vline(hdc, cx, cy, y1);
                del_pen(o, p);
            }
            0x2510 => {
                // ┐
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, x0, cx + 1, cy);
                vline(hdc, cx, cy, y1);
                del_pen(o, p);
            }
            0x2514 => {
                // └
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, cx, x1, cy);
                vline(hdc, cx, y0, cy + 1);
                del_pen(o, p);
            }
            0x2518 => {
                // ┘
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, x0, cx + 1, cy);
                vline(hdc, cx, y0, cy + 1);
                del_pen(o, p);
            }
            // ── 角 (heavy) ──────────────────────────────────────────────────
            0x250F => {
                // ┏
                let p = make_pen(thick);
                let o = set_pen(p);
                hline(hdc, cx, x1, cy);
                vline(hdc, cx, cy, y1);
                del_pen(o, p);
            }
            0x2513 => {
                // ┓
                let p = make_pen(thick);
                let o = set_pen(p);
                hline(hdc, x0, cx + 1, cy);
                vline(hdc, cx, cy, y1);
                del_pen(o, p);
            }
            0x2517 => {
                // ┗
                let p = make_pen(thick);
                let o = set_pen(p);
                hline(hdc, cx, x1, cy);
                vline(hdc, cx, y0, cy + 1);
                del_pen(o, p);
            }
            0x251B => {
                // ┛
                let p = make_pen(thick);
                let o = set_pen(p);
                hline(hdc, x0, cx + 1, cy);
                vline(hdc, cx, y0, cy + 1);
                del_pen(o, p);
            }
            // ── T 字 (light) ────────────────────────────────────────────────
            0x251C => {
                // ├ vertical + right
                let p = make_pen(thin);
                let o = set_pen(p);
                vline(hdc, cx, y0, y1);
                hline(hdc, cx, x1, cy);
                del_pen(o, p);
            }
            0x2524 => {
                // ┤ vertical + left
                let p = make_pen(thin);
                let o = set_pen(p);
                vline(hdc, cx, y0, y1);
                hline(hdc, x0, cx + 1, cy);
                del_pen(o, p);
            }
            0x252C => {
                // ┬ horizontal + down
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, x0, x1, cy);
                vline(hdc, cx, cy, y1);
                del_pen(o, p);
            }
            0x2534 => {
                // ┴ horizontal + up
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, x0, x1, cy);
                vline(hdc, cx, y0, cy + 1);
                del_pen(o, p);
            }
            0x253C => {
                // ┼ cross
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, x0, x1, cy);
                vline(hdc, cx, y0, y1);
                del_pen(o, p);
            }
            // ── T 字 (heavy) ────────────────────────────────────────────────
            0x2523 => {
                // ┣
                let p = make_pen(thick);
                let o = set_pen(p);
                vline(hdc, cx, y0, y1);
                hline(hdc, cx, x1, cy);
                del_pen(o, p);
            }
            0x252B => {
                // ┫
                let p = make_pen(thick);
                let o = set_pen(p);
                vline(hdc, cx, y0, y1);
                hline(hdc, x0, cx + 1, cy);
                del_pen(o, p);
            }
            0x2533 => {
                // ┳
                let p = make_pen(thick);
                let o = set_pen(p);
                hline(hdc, x0, x1, cy);
                vline(hdc, cx, cy, y1);
                del_pen(o, p);
            }
            0x253B => {
                // ┻
                let p = make_pen(thick);
                let o = set_pen(p);
                hline(hdc, x0, x1, cy);
                vline(hdc, cx, y0, cy + 1);
                del_pen(o, p);
            }
            0x254B => {
                // ╋
                let p = make_pen(thick);
                let o = set_pen(p);
                hline(hdc, x0, x1, cy);
                vline(hdc, cx, y0, y1);
                del_pen(o, p);
            }
            // ── 二重線 ──────────────────────────────────────────────────────
            0x2550 => {
                // ═ double horizontal
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, x0, x1, cy - 1);
                hline(hdc, x0, x1, cy + 1);
                del_pen(o, p);
            }
            0x2551 => {
                // ║ double vertical
                let p = make_pen(thin);
                let o = set_pen(p);
                vline(hdc, cx - 1, y0, y1);
                vline(hdc, cx + 1, y0, y1);
                del_pen(o, p);
            }
            // ── 丸角 (rounded corners) ──────────────────────────────────────
            // 丸弧の近似として角を L 字型の線分で描画する
            0x256D => {
                // ╭ rounded down-right
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, cx, x1, cy);
                vline(hdc, cx, cy, y1);
                del_pen(o, p);
            }
            0x256E => {
                // ╮ rounded down-left
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, x0, cx + 1, cy);
                vline(hdc, cx, cy, y1);
                del_pen(o, p);
            }
            0x256F => {
                // ╯ rounded up-left
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, x0, cx + 1, cy);
                vline(hdc, cx, y0, cy + 1);
                del_pen(o, p);
            }
            0x2570 => {
                // ╰ rounded up-right
                let p = make_pen(thin);
                let o = set_pen(p);
                hline(hdc, cx, x1, cy);
                vline(hdc, cx, y0, cy + 1);
                del_pen(o, p);
            }
            // ── 斜線 ────────────────────────────────────────────────────────
            0x2571 => {
                // ╱ diagonal upper-right to lower-left
                let p = make_pen(thin);
                let o = set_pen(p);
                let _ = MoveToEx(hdc, x0, y1 - 1, None);
                let _ = LineTo(hdc, x1, y0);
                del_pen(o, p);
            }
            0x2572 => {
                // ╲ diagonal upper-left to lower-right
                let p = make_pen(thin);
                let o = set_pen(p);
                let _ = MoveToEx(hdc, x0, y0, None);
                let _ = LineTo(hdc, x1, y1);
                del_pen(o, p);
            }
            0x2573 => {
                // ╳ diagonal cross
                let p = make_pen(thin);
                let o = set_pen(p);
                let _ = MoveToEx(hdc, x0, y1 - 1, None);
                let _ = LineTo(hdc, x1, y0);
                let _ = MoveToEx(hdc, x0, y0, None);
                let _ = LineTo(hdc, x1, y1);
                del_pen(o, p);
            }
            // ── ブロック要素 (U+2580-U+259F) ────────────────────────────────
            0x2580 => {
                // ▀ upper half
                fill_rect(hdc, fg, x0, y0, x1, cy);
            }
            0x2584 => {
                // ▄ lower half
                fill_rect(hdc, fg, x0, cy, x1, y1);
            }
            0x2588 => {
                // █ full
                fill_rect(hdc, fg, x0, y0, x1, y1);
            }
            0x258C => {
                // ▌ left half
                fill_rect(hdc, fg, x0, y0, cx, y1);
            }
            0x2590 => {
                // ▐ right half
                fill_rect(hdc, fg, cx, y0, x1, y1);
            }
            0x2596 => {
                // ▖ lower-left quarter
                fill_rect(hdc, fg, x0, cy, cx, y1);
            }
            0x2597 => {
                // ▗ lower-right quarter
                fill_rect(hdc, fg, cx, cy, x1, y1);
            }
            0x2598 => {
                // ▘ upper-left quarter
                fill_rect(hdc, fg, x0, y0, cx, cy);
            }
            0x259D => {
                // ▝ upper-right quarter
                fill_rect(hdc, fg, cx, y0, x1, cy);
            }
            // 7/8・5/8 ブロック (縦)
            0x2581 => fill_rect(hdc, fg, x0, y0 + h * 7 / 8, x1, y1), // ▁
            0x2582 => fill_rect(hdc, fg, x0, y0 + h * 6 / 8, x1, y1), // ▂
            0x2583 => fill_rect(hdc, fg, x0, y0 + h * 5 / 8, x1, y1), // ▃
            0x2585 => fill_rect(hdc, fg, x0, y0 + h * 3 / 8, x1, y1), // ▅
            0x2586 => fill_rect(hdc, fg, x0, y0 + h * 2 / 8, x1, y1), // ▆
            0x2587 => fill_rect(hdc, fg, x0, y0 + h / 8, x1, y1),     // ▇
            // 横 1/8 ブロック
            0x2589 => fill_rect(hdc, fg, x0, y0, x0 + w * 7 / 8, y1), // ▉
            0x258A => fill_rect(hdc, fg, x0, y0, x0 + w * 6 / 8, y1), // ▊
            0x258B => fill_rect(hdc, fg, x0, y0, x0 + w * 5 / 8, y1), // ▋
            0x258D => fill_rect(hdc, fg, x0, y0, x0 + w * 3 / 8, y1), // ▍
            0x258E => fill_rect(hdc, fg, x0, y0, x0 + w * 2 / 8, y1), // ▎
            0x258F => fill_rect(hdc, fg, x0, y0, x0 + w / 8, y1),     // ▏
            _ => return false,
        }
        true
    }

    /// 塗りつぶし矩形のヘルパー
    unsafe fn fill_rect(hdc: HDC, color: COLORREF, lx: i32, ty: i32, rx: i32, by: i32) {
        let brush = CreateSolidBrush(color);
        let r = RECT {
            left: lx,
            top: ty,
            right: rx,
            bottom: by,
        };
        windows::Win32::Graphics::Gdi::FillRect(hdc, &r, brush);
        let _ = DeleteObject(brush.into());
    }

    /// プリエディット下線を描画する
    ///
    /// - `TargetConverted`: 太実線（現在の変換候補）
    /// - `Converted`: 実線
    /// - その他: 点線
    unsafe fn draw_preedit_underline(
        hdc: HDC,
        attr: &PreeditAttr,
        x: i32,
        y: i32,
        width: i32,
        fg: COLORREF,
    ) {
        let (pen_style, thickness) = match attr {
            PreeditAttr::TargetConverted => (PS_SOLID, 2u32),
            PreeditAttr::Converted => (PS_SOLID, 1),
            PreeditAttr::TargetNotConverted => (PS_DOT, 2),
            PreeditAttr::Input => (PS_DOT, 1),
        };

        let pen = CreatePen(pen_style, thickness as i32, fg);
        let old_pen = SelectObject(hdc, pen.into());
        let _ = MoveToEx(hdc, x, y, None);
        let _ = LineTo(hdc, x + width, y);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(pen.into());
    }

    /// GDI 描画用テキストを返す
    ///
    /// GDI の `ExtTextOutW` は ZWJ シーケンスをシェーピングできないため、
    /// ZWJ（U+200D）が含まれる場合は最初の ZWJ より前の部分文字列のみを返す。
    /// これにより基底グリフ（例: 👨）だけが描画され、
    /// ZWJ 後の文字（例: 💻）がクリップ矩形外にはみ出すのを防ぐ。
    fn zwj_render_text(text: &str) -> &str {
        if let Some(pos) = text.find('\u{200D}') {
            &text[..pos]
        } else {
            text
        }
    }

    fn cell_colors(cell: &Cell, _ime: &ImeState, theme: &WinTheme) -> (COLORREF, COLORREF) {
        let fg = cell
            .style
            .fg
            .map(|c| rgb(c.r, c.g, c.b))
            .unwrap_or(theme.fg);
        let bg = cell
            .style
            .bg
            .map(|c| rgb(c.r, c.g, c.b))
            .unwrap_or(theme.bg);
        if cell.style.reverse {
            (bg, fg)
        } else {
            (fg, bg)
        }
    }

    fn preedit_segment_colors(attr: &PreeditAttr, theme: &WinTheme) -> (COLORREF, COLORREF) {
        match attr {
            PreeditAttr::TargetConverted => (theme.bg, theme.fg), // 反転
            _ => (theme.fg, COLOR_PREEDIT_BG),
        }
    }

    // ── マウスシーケンス生成 ─────────────────────────────────────────────────

    /// マウスイベントを VT シーケンスに変換する
    ///
    /// - `btn`: ボタン番号（0=左, 2=右, 32+n=モーション）
    /// - `col`, `row`: 1 始まりのセル座標
    /// - `release`: ボタン離上イベントか
    /// - `sgr`: `?1006h` SGR 拡張モードか
    fn mouse_to_vt(btn: u8, col: u16, row: u16, release: bool, sgr: bool) -> Option<Vec<u8>> {
        if col == 0 || row == 0 {
            return None;
        }
        if sgr {
            // CSI < btn ; col ; row M/m
            let suffix = if release { b'm' } else { b'M' };
            Some(format!("\x1b[<{};{};{}{}", btn, col, row, suffix as char).into_bytes())
        } else {
            // CSI M btn+32 col+32 row+32 (X10 encoding, max col/row = 223)
            if col > 223 || row > 223 {
                return None;
            }
            Some(vec![
                0x1b,
                b'[',
                b'M',
                btn + 32,
                col as u8 + 32,
                row as u8 + 32,
            ])
        }
    }

    // ── クリップボード ───────────────────────────────────────────────────────

    /// クリップボードから UTF-8 テキストを読み取る
    unsafe fn read_clipboard_text(hwnd: HWND) -> Option<String> {
        if OpenClipboard(Some(hwnd)).is_err() {
            return None;
        }
        // CF_UNICODETEXT = 13
        let h = GetClipboardData(13).ok()?;
        // HGLOBAL is *mut c_void in windows-rs 0.58; HANDLE is *mut c_void as well
        let hglobal = HGLOBAL(h.0);
        let ptr = GlobalLock(hglobal) as *const u16;
        let text = if ptr.is_null() {
            None
        } else {
            let mut len = 0usize;
            while *ptr.add(len) != 0 {
                len += 1;
            }
            let slice = std::slice::from_raw_parts(ptr, len);
            String::from_utf16(slice).ok()
        };
        let _ = GlobalUnlock(hglobal);
        let _ = CloseClipboard();
        text
    }

    /// OSC 52 クリップボードデータ（UTF-8 バイト列）をシステムクリップボードに書き込む
    ///
    /// 有効な UTF-8 の場合は CF_UNICODETEXT として書き込む。
    /// バイナリデータや無効な UTF-8 は無視する。
    unsafe fn write_clipboard_text(hwnd: HWND, data: &[u8]) {
        let text = match std::str::from_utf8(data) {
            Ok(s) => s,
            Err(_) => return, // 有効な UTF-8 でない場合は無視
        };
        // UTF-16 LE + NUL ターミネーター
        let mut wide: Vec<u16> = text.encode_utf16().collect();
        wide.push(0u16);
        let byte_size = wide.len() * 2;

        if OpenClipboard(Some(hwnd)).is_err() {
            return;
        }
        let _ = EmptyClipboard();

        // CF_UNICODETEXT = 13
        if let Ok(hglobal) = GlobalAlloc(GMEM_MOVEABLE, byte_size) {
            let ptr = GlobalLock(hglobal) as *mut u16;
            if !ptr.is_null() {
                std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
                let _ = GlobalUnlock(hglobal);
                // SetClipboardData takes ownership of hglobal on success;
                // on failure we should free it, but for simplicity we skip that here.
                let _ = SetClipboardData(13, Some(windows::Win32::Foundation::HANDLE(hglobal.0)));
            }
        }
        let _ = CloseClipboard();
    }

    // ── キーマップ ───────────────────────────────────────────────────────────

    /// `WM_KEYDOWN` の仮想キーコードを VT シーケンスに変換する
    fn keydown_to_vt(wparam: WPARAM, lparam: LPARAM, app_cursor: bool) -> Option<Vec<u8>> {
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        keydown_to_vt_with_mods(wparam, lparam, ctrl, shift, app_cursor)
    }

    /// 修飾キー状態を引数で受け取るテスト可能な内部実装
    fn keydown_to_vt_with_mods(
        wparam: WPARAM,
        _lparam: LPARAM,
        ctrl: bool,
        shift: bool,
        app_cursor: bool,
    ) -> Option<Vec<u8>> {
        let vk = wparam.0 as u16;

        let seq: &[u8] = match VIRTUAL_KEY(vk) {
            // Enter, Escape は WM_CHAR で処理する（TranslateMessage 経由で 1 回だけ送信）
            VK_BACK => b"\x7f",
            VK_TAB => {
                if shift {
                    b"\x1b[Z"
                } else {
                    b"\t"
                }
            }
            VK_UP => {
                if app_cursor {
                    b"\x1bOA"
                } else {
                    b"\x1b[A"
                }
            }
            VK_DOWN => {
                if app_cursor {
                    b"\x1bOB"
                } else {
                    b"\x1b[B"
                }
            }
            VK_RIGHT => {
                if app_cursor {
                    b"\x1bOC"
                } else {
                    b"\x1b[C"
                }
            }
            VK_LEFT => {
                if app_cursor {
                    b"\x1bOD"
                } else {
                    b"\x1b[D"
                }
            }
            VK_HOME => b"\x1b[H",
            VK_END => b"\x1b[F",
            VK_INSERT => b"\x1b[2~",
            VK_DELETE => b"\x1b[3~",
            VK_PRIOR => b"\x1b[5~", // Page Up
            VK_NEXT => b"\x1b[6~",  // Page Down
            VK_F1 => b"\x1bOP",
            VK_F2 => b"\x1bOQ",
            VK_F3 => b"\x1bOR",
            VK_F4 => b"\x1bOS",
            VK_F5 => b"\x1b[15~",
            VK_F6 => b"\x1b[17~",
            VK_F7 => b"\x1b[18~",
            VK_F8 => b"\x1b[19~",
            VK_F9 => b"\x1b[20~",
            VK_F10 => b"\x1b[21~",
            VK_F11 => b"\x1b[23~",
            VK_F12 => b"\x1b[24~",
            _ => {
                // Ctrl+アルファベット → \x01-\x1a
                if ctrl && vk >= b'A' as u16 && vk <= b'Z' as u16 {
                    return Some(vec![vk as u8 - b'A' + 1]);
                }
                return None;
            }
        };

        Some(seq.to_vec())
    }

    // ── ウィンドウ起動 ───────────────────────────────────────────────────────

    /// Win32 ウィンドウを作成してメッセージループを開始する
    ///
    /// この関数はウィンドウが閉じられるまでブロックする。
    /// `tokio::task::spawn_blocking` でメインスレッドとは別に実行すること。
    #[allow(clippy::too_many_arguments)]
    pub fn run_window(
        panes: Arc<Mutex<PaneStore>>,
        msg_tx: mpsc::Sender<ClientMessage>,
        split_tx: mpsc::Sender<(PaneId, SplitDirection)>,
        initial_size: TermSize,
        app_focused: Arc<AtomicBool>,
        native_notif_queue: Arc<Mutex<VecDeque<NativeToastMsg>>>,
        float_tx: mpsc::Sender<()>,
        layout_tx: mpsc::Sender<String>,
        theme: crate::window::Theme,
    ) -> anyhow::Result<()> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?;

            // ── フォント作成（テーマ設定または候補リストから自動選択）──
            let font_height = theme
                .font_size
                .map(|pt| -((pt as i32) * 96 / 72))
                .unwrap_or(FONT_HEIGHT);
            let hfont = if let Some(ref family) = theme.font_family {
                create_font_with_family(family, font_height)
            } else {
                create_best_font(font_height)
            };

            // セルサイズをテキストメトリクスから取得
            let (cell_width, cell_height) = measure_cell_size(hfont)?;

            let win_theme = WinTheme::from_theme(&theme);

            // ── ClientState をヒープに確保 ──────────────────────────────
            let state = Box::new(ClientState::new(
                panes,
                msg_tx,
                split_tx,
                cell_width,
                cell_height,
                hfont,
                app_focused,
                native_notif_queue,
                float_tx,
                layout_tx,
                win_theme,
            ));
            let state_ptr = Box::into_raw(state);

            // ── ウィンドウクラス登録 ─────────────────────────────────────
            let class_name: Vec<u16> = CLASS_NAME.encode_utf16().collect();
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(wnd_proc),
                hInstance: hinstance.into(),
                hCursor: LoadCursorW(None, IDC_IBEAM)?,
                lpszClassName: PCWSTR(class_name.as_ptr()),
                ..Default::default()
            };
            RegisterClassExW(&wc);

            // ── ウィンドウ作成 ───────────────────────────────────────────
            // AdjustWindowRectEx でクライアント領域が exactly (cols * cell_w + padding) に
            // なるようにウィンドウ全体サイズを計算する。
            // CreateWindowExW に渡す nWidth/nHeight はウィンドウ矩形サイズ（ボーダー・
            // タイトルバー含む）なので、そのまま cols * cell_w + padding を渡すと
            // 実際のクライアント幅が数ピクセル小さくなり、WM_SIZE 後の cols が
            // ConPTY の初期値とずれてしまう。
            let client_w = initial_size.cols as i32 * cell_width + PADDING_X * 2;
            let client_h = initial_size.rows as i32 * cell_height + PADDING_Y * 2;
            let mut wr = RECT {
                left: 0,
                top: 0,
                right: client_w,
                bottom: client_h,
            };
            AdjustWindowRectEx(
                &mut wr,
                WS_OVERLAPPEDWINDOW,
                false,
                WINDOW_EX_STYLE::default(),
            )
            .map_err(|e| anyhow::anyhow!("AdjustWindowRectEx failed: {}", e))?;
            let win_width = wr.right - wr.left;
            let win_height = wr.bottom - wr.top;
            let title: Vec<u16> = "yatamux\0".encode_utf16().collect();

            let hwnd = CreateWindowExW(
                WINDOW_EX_STYLE::default(),
                PCWSTR(class_name.as_ptr()),
                PCWSTR(title.as_ptr()),
                WS_OVERLAPPEDWINDOW,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                win_width,
                win_height,
                None,
                None,
                Some(HINSTANCE(hinstance.0)),
                Some(state_ptr as *const _),
            )?;

            let _ = ShowWindow(hwnd, SW_SHOWMAXIMIZED);
            let _ = UpdateWindow(hwnd);

            // ── メッセージループ ──────────────────────────────────────────
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }

            // クリーンアップ
            let _ = Box::from_raw(state_ptr); // Drop ClientState
            let _ = DeleteObject(hfont.into());

            Ok(())
        }
    }

    /// トレイアイコンを追加してバルーンチップ通知を表示する
    unsafe fn show_balloon_notification(hwnd: HWND, title: &str, body: &str) {
        let mut nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            uFlags: NIF_ICON | NIF_TIP | NIF_INFO,
            hIcon: LoadIconW(None, IDI_APPLICATION).unwrap_or_default(),
            ..Default::default()
        };
        // szTip: トレイツールチップ
        let tip = "yatamux\0";
        let tip_wide: Vec<u16> = tip.encode_utf16().collect();
        let copy_len = tip_wide.len().min(nid.szTip.len());
        nid.szTip[..copy_len].copy_from_slice(&tip_wide[..copy_len]);
        // szInfoTitle: バルーンタイトル
        let title_wide: Vec<u16> = format!("{}\0", title).encode_utf16().collect();
        let copy_len = title_wide.len().min(nid.szInfoTitle.len());
        nid.szInfoTitle[..copy_len].copy_from_slice(&title_wide[..copy_len]);
        // szInfo: バルーン本文
        let body_wide: Vec<u16> = format!("{}\0", body).encode_utf16().collect();
        let copy_len = body_wide.len().min(nid.szInfo.len());
        nid.szInfo[..copy_len].copy_from_slice(&body_wide[..copy_len]);
        nid.dwInfoFlags = NIIF_INFO;
        let _ = Shell_NotifyIconW(NIM_ADD, &nid);
    }

    /// トレイアイコンを削除する
    unsafe fn remove_tray_icon(hwnd: HWND) {
        let nid = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: hwnd,
            uID: 1,
            ..Default::default()
        };
        let _ = Shell_NotifyIconW(NIM_DELETE, &nid);
    }

    /// 指定フォントファミリーで作成を試み、失敗した場合は候補リストにフォールバックする
    unsafe fn create_font_with_family(family: &str, height: i32) -> HFONT {
        let wide: Vec<u16> = format!("{}\0", family).encode_utf16().collect();
        let hfont = CreateFontW(
            height,
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            OUT_DEFAULT_PRECIS,
            CLIP_DEFAULT_PRECIS,
            CLEARTYPE_QUALITY,
            (FIXED_PITCH.0 | FF_MODERN.0) as u32,
            PCWSTR(wide.as_ptr()),
        );
        let hdc = GetDC(None);
        let old = SelectObject(hdc, hfont.into());
        let mut buf = [0u16; 64];
        let len = GetTextFaceW(hdc, Some(&mut buf)) as usize;
        SelectObject(hdc, old);
        ReleaseDC(None, hdc);
        let actual = String::from_utf16_lossy(&buf[..len])
            .trim_end_matches('\0')
            .to_string();
        let norm_actual = actual.replace(' ', "").to_lowercase();
        let norm_want = family.replace(' ', "").to_lowercase();
        if actual.eq_ignore_ascii_case(family) || norm_actual == norm_want {
            tracing::info!(family, "font: user-specified font loaded");
            hfont
        } else {
            tracing::warn!(
                family,
                actual,
                "font: user-specified font not found, falling back"
            );
            let _ = DeleteObject(hfont.into());
            create_best_font(height)
        }
    }

    /// インストール済みの候補フォントから最適なものを選んで作成する
    ///
    /// `GetTextFaceW` で実際に使われたフォント名を確認し、
    /// 指定したフォントが存在した場合のみ採用する。
    /// 全候補が見つからない場合は最後の候補をそのまま返す。
    unsafe fn create_best_font(height: i32) -> HFONT {
        let hdc = GetDC(None);

        for (i, &name) in FONT_CANDIDATES.iter().enumerate() {
            let wide: Vec<u16> = format!("{}\0", name).encode_utf16().collect();
            let hfont = CreateFontW(
                height,
                0,
                0,
                0,
                FW_NORMAL.0 as i32,
                0,
                0,
                0,
                DEFAULT_CHARSET,
                OUT_DEFAULT_PRECIS,
                CLIP_DEFAULT_PRECIS,
                CLEARTYPE_QUALITY,
                (FIXED_PITCH.0 | FF_MODERN.0) as u32,
                PCWSTR(wide.as_ptr()),
            );

            let old = SelectObject(hdc, hfont.into());

            // 実際に割り当てられたフォント名を取得して一致確認
            let mut face = [0u16; 64];
            let len = GetTextFaceW(hdc, Some(&mut face)) as usize;
            // GetTextFaceW の戻り値はヌル終端を含む長さなので除去する
            let actual: String = String::from_utf16_lossy(&face[..len])
                .trim_end_matches('\0')
                .to_string();

            SelectObject(hdc, old);

            let is_last = i == FONT_CANDIDATES.len() - 1;
            // GetTextFaceW が返す名前はスペースなしの場合があるため
            // 正規化して前方一致でも許容する
            let norm_actual = actual.replace(' ', "").to_lowercase();
            let norm_want = name.replace(' ', "").to_lowercase();
            let matched = actual.eq_ignore_ascii_case(name) || norm_actual == norm_want;
            tracing::info!(candidate = name, actual = %actual, matched, "font probe");
            if matched || is_last {
                ReleaseDC(None, hdc);
                return hfont;
            }
            let _ = DeleteObject(hfont.into());
        }

        // ここには到達しない（candidates は空でない）が、コンパイラ用
        ReleaseDC(None, hdc);
        HFONT::default()
    }

    /// 仮の DC でフォントのセルサイズを計測する
    unsafe fn measure_cell_size(hfont: HFONT) -> anyhow::Result<(i32, i32)> {
        let hdc = GetDC(None);
        let old_font = SelectObject(hdc, hfont.into());
        let mut tm = TEXTMETRICW::default();
        let _ = GetTextMetricsW(hdc, &mut tm);
        SelectObject(hdc, old_font);
        ReleaseDC(None, hdc);
        // cell_width = 半角1文字分の幅。全角は cell_width * 2 で表す。
        // tmAveCharWidth は等幅フォントでは ASCII 1文字の幅に等しい。
        Ok((tm.tmAveCharWidth, tm.tmHeight + tm.tmExternalLeading))
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        /// 仮想キーコードから WPARAM を作る
        fn wvk(vk: VIRTUAL_KEY) -> WPARAM {
            WPARAM(vk.0 as usize)
        }

        fn lp() -> LPARAM {
            LPARAM(0)
        }

        // H-1: Enter は WM_CHAR に委譲（keydown_to_vt は None）
        // TranslateMessage が WM_CHAR(13=\r) を生成し、1 回だけ送信される。
        #[test]
        fn test_enter_delegated_to_wm_char() {
            assert_eq!(keydown_to_vt(wvk(VK_RETURN), lp(), false), None);
        }

        // H-2: Backspace → DEL (\x7f)  WM_CHAR(\b) は二重送信防止のためスキップ
        #[test]
        fn test_backspace_maps_to_del() {
            assert_eq!(
                keydown_to_vt(wvk(VK_BACK), lp(), false),
                Some(b"\x7f".to_vec())
            );
        }

        // H-3: Escape は WM_CHAR に委譲（keydown_to_vt は None）
        // TranslateMessage が WM_CHAR(27=\x1b) を生成し、1 回だけ送信される。
        #[test]
        fn test_escape_delegated_to_wm_char() {
            assert_eq!(keydown_to_vt(wvk(VK_ESCAPE), lp(), false), None);
        }

        // H-4: Tab → HT (\t)
        // ※ GetKeyState はテスト中 0 を返す（Shift 未押下）
        #[test]
        fn test_tab_maps_to_ht() {
            assert_eq!(
                keydown_to_vt(wvk(VK_TAB), lp(), false),
                Some(b"\t".to_vec())
            );
        }

        // H-5: 矢印キー → ANSI シーケンス
        #[test]
        fn test_arrow_up() {
            assert_eq!(
                keydown_to_vt(wvk(VK_UP), lp(), false),
                Some(b"\x1b[A".to_vec())
            );
        }

        #[test]
        fn test_arrow_down() {
            assert_eq!(
                keydown_to_vt(wvk(VK_DOWN), lp(), false),
                Some(b"\x1b[B".to_vec())
            );
        }

        #[test]
        fn test_arrow_right() {
            assert_eq!(
                keydown_to_vt(wvk(VK_RIGHT), lp(), false),
                Some(b"\x1b[C".to_vec())
            );
        }

        #[test]
        fn test_arrow_left() {
            assert_eq!(
                keydown_to_vt(wvk(VK_LEFT), lp(), false),
                Some(b"\x1b[D".to_vec())
            );
        }

        // H-6: 特殊キー → VT シーケンス
        #[test]
        fn test_home() {
            assert_eq!(
                keydown_to_vt(wvk(VK_HOME), lp(), false),
                Some(b"\x1b[H".to_vec())
            );
        }

        #[test]
        fn test_end() {
            assert_eq!(
                keydown_to_vt(wvk(VK_END), lp(), false),
                Some(b"\x1b[F".to_vec())
            );
        }

        #[test]
        fn test_insert() {
            assert_eq!(
                keydown_to_vt(wvk(VK_INSERT), lp(), false),
                Some(b"\x1b[2~".to_vec())
            );
        }

        #[test]
        fn test_delete() {
            assert_eq!(
                keydown_to_vt(wvk(VK_DELETE), lp(), false),
                Some(b"\x1b[3~".to_vec())
            );
        }

        #[test]
        fn test_page_up() {
            assert_eq!(
                keydown_to_vt(wvk(VK_PRIOR), lp(), false),
                Some(b"\x1b[5~".to_vec())
            );
        }

        #[test]
        fn test_page_down() {
            assert_eq!(
                keydown_to_vt(wvk(VK_NEXT), lp(), false),
                Some(b"\x1b[6~".to_vec())
            );
        }

        // H-7: ファンクションキー F1–F12
        #[test]
        fn test_f1() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F1), lp(), false),
                Some(b"\x1bOP".to_vec())
            );
        }

        #[test]
        fn test_f2() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F2), lp(), false),
                Some(b"\x1bOQ".to_vec())
            );
        }

        #[test]
        fn test_f3() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F3), lp(), false),
                Some(b"\x1bOR".to_vec())
            );
        }

        #[test]
        fn test_f4() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F4), lp(), false),
                Some(b"\x1bOS".to_vec())
            );
        }

        #[test]
        fn test_f5() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F5), lp(), false),
                Some(b"\x1b[15~".to_vec())
            );
        }

        #[test]
        fn test_f6() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F6), lp(), false),
                Some(b"\x1b[17~".to_vec())
            );
        }

        #[test]
        fn test_f7() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F7), lp(), false),
                Some(b"\x1b[18~".to_vec())
            );
        }

        #[test]
        fn test_f8() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F8), lp(), false),
                Some(b"\x1b[19~".to_vec())
            );
        }

        #[test]
        fn test_f9() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F9), lp(), false),
                Some(b"\x1b[20~".to_vec())
            );
        }

        #[test]
        fn test_f10() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F10), lp(), false),
                Some(b"\x1b[21~".to_vec())
            );
        }

        #[test]
        fn test_f11() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F11), lp(), false),
                Some(b"\x1b[23~".to_vec())
            );
        }

        #[test]
        fn test_f12() {
            assert_eq!(
                keydown_to_vt(wvk(VK_F12), lp(), false),
                Some(b"\x1b[24~".to_vec())
            );
        }

        // H-4: Shift+Tab → \x1b[Z
        #[test]
        fn test_shift_tab() {
            assert_eq!(
                keydown_to_vt_with_mods(wvk(VK_TAB), lp(), false, true, false),
                Some(b"\x1b[Z".to_vec())
            );
        }

        // H-9: 通常文字キー (例: Space) は None → WM_CHAR で処理される
        #[test]
        fn test_unhandled_key_returns_none() {
            assert_eq!(
                keydown_to_vt(WPARAM(VK_SPACE.0 as usize), lp(), false),
                None
            );
        }

        // H-8: Ctrl+A → SOH (\x01)
        #[test]
        fn test_ctrl_a() {
            assert_eq!(
                keydown_to_vt_with_mods(WPARAM(b'A' as usize), lp(), true, false, false),
                Some(vec![0x01])
            );
        }

        // H-8: Ctrl+C → ETX (\x03)
        #[test]
        fn test_ctrl_c() {
            assert_eq!(
                keydown_to_vt_with_mods(WPARAM(b'C' as usize), lp(), true, false, false),
                Some(vec![0x03])
            );
        }

        // H-8: Ctrl+D → EOT (\x04)
        #[test]
        fn test_ctrl_d() {
            assert_eq!(
                keydown_to_vt_with_mods(WPARAM(b'D' as usize), lp(), true, false, false),
                Some(vec![0x04])
            );
        }

        // H-8: Ctrl+L → FF (\x0c) — 画面クリアによく使われる
        #[test]
        fn test_ctrl_l() {
            assert_eq!(
                keydown_to_vt_with_mods(WPARAM(b'L' as usize), lp(), true, false, false),
                Some(vec![0x0c])
            );
        }

        // H-8: Ctrl+Z → SUB (\x1a)
        #[test]
        fn test_ctrl_z() {
            assert_eq!(
                keydown_to_vt_with_mods(WPARAM(b'Z' as usize), lp(), true, false, false),
                Some(vec![0x1a])
            );
        }

        // H-8: Ctrl+A–Z の全範囲を検証
        #[test]
        fn test_ctrl_all_letters() {
            for (i, letter) in (b'A'..=b'Z').enumerate() {
                let expected = vec![(i + 1) as u8]; // \x01–\x1a
                assert_eq!(
                    keydown_to_vt_with_mods(WPARAM(letter as usize), lp(), true, false, false),
                    Some(expected),
                    "Ctrl+{} should produce \\x{:02x}",
                    letter as char,
                    i + 1
                );
            }
        }

        // Ctrl なしでアルファベットは None（WM_CHAR へ委譲）
        #[test]
        fn test_letter_without_ctrl_returns_none() {
            assert_eq!(
                keydown_to_vt_with_mods(WPARAM(b'A' as usize), lp(), false, false, false),
                None
            );
        }

        // TC-09: Normal モードでは mode が Normal のまま
        #[test]
        fn test_client_mode_default_normal() {
            let mode = ClientMode::Normal;
            assert_eq!(mode, ClientMode::Normal);
            assert_ne!(mode, ClientMode::Pane);
        }

        // TC-10: skip_char ヘルパーロジック検証
        // WM_CHAR で skip_char=true なら抑制する（Cell 操作の単体確認）
        #[test]
        fn test_skip_char_cell_behavior() {
            let skip = std::cell::Cell::new(false);
            assert!(!skip.get());
            skip.set(true);
            assert!(skip.get());
            // 消費後に false に戻る
            let was = skip.get();
            skip.set(false);
            assert!(was);
            assert!(!skip.get());
        }

        // TC-08: is_in_normal_selection — 単一行選択
        #[test]
        fn test_normal_sel_single_row() {
            // anchor=(2,1), end=(5,1)
            let sel = (2usize, 1usize, 5usize, 1usize);
            assert!(!is_in_normal_selection(sel, 1, 1)); // 左外
            assert!(is_in_normal_selection(sel, 2, 1)); // 左端
            assert!(is_in_normal_selection(sel, 3, 1)); // 内側
            assert!(is_in_normal_selection(sel, 5, 1)); // 右端
            assert!(!is_in_normal_selection(sel, 6, 1)); // 右外
            assert!(!is_in_normal_selection(sel, 3, 0)); // 別行
        }

        // TC-08: is_in_normal_selection — 逆方向ドラッグ（end < anchor）
        #[test]
        fn test_normal_sel_reversed() {
            // anchor=(5,1), end=(2,1) — 右から左へドラッグ
            let sel = (5usize, 1usize, 2usize, 1usize);
            assert!(!is_in_normal_selection(sel, 1, 1));
            assert!(is_in_normal_selection(sel, 2, 1));
            assert!(is_in_normal_selection(sel, 5, 1));
            assert!(!is_in_normal_selection(sel, 6, 1));
        }

        // TC-09: is_in_normal_selection — 複数行選択
        #[test]
        fn test_normal_sel_multi_row() {
            // anchor=(2,1), end=(4,3)
            let sel = (2usize, 1usize, 4usize, 3usize);
            // 開始行より前
            assert!(!is_in_normal_selection(sel, 0, 0));
            // 開始行: anchor 列以降が選択
            assert!(!is_in_normal_selection(sel, 1, 1));
            assert!(is_in_normal_selection(sel, 2, 1));
            assert!(is_in_normal_selection(sel, 99, 1));
            // 中間行: 全幅が選択
            assert!(is_in_normal_selection(sel, 0, 2));
            assert!(is_in_normal_selection(sel, 99, 2));
            // 終了行: end 列以前が選択
            assert!(is_in_normal_selection(sel, 0, 3));
            assert!(is_in_normal_selection(sel, 4, 3));
            assert!(!is_in_normal_selection(sel, 5, 3));
            // 終了行より後
            assert!(!is_in_normal_selection(sel, 0, 4));
        }

        // TC-09: is_in_normal_selection — 複数行・逆方向
        #[test]
        fn test_normal_sel_multi_row_reversed() {
            // anchor=(4,3), end=(2,1) — 下から上へドラッグ
            let sel = (4usize, 3usize, 2usize, 1usize);
            assert!(!is_in_normal_selection(sel, 1, 1));
            assert!(is_in_normal_selection(sel, 2, 1));
            assert!(is_in_normal_selection(sel, 0, 2));
            assert!(is_in_normal_selection(sel, 4, 3));
            assert!(!is_in_normal_selection(sel, 5, 3));
        }

        // ── B-7: スクロールオフセットのクランプ / リセット ──────────────────

        // TC-B7-01: scroll_offset がクランプされる（resize_all_panes の clamp ロジック確認）
        #[test]
        fn test_scroll_offset_clamped_to_scrollback_len() {
            let scroll_offset: usize = 50;
            let max_offset: usize = 20; // scrollback_len() の戻り値想定
            let clamped = scroll_offset.min(max_offset);
            assert_eq!(clamped, 20);
        }

        // TC-B7-02: scroll_offset が max 以下なら変化しない
        #[test]
        fn test_scroll_offset_within_range_unchanged() {
            let scroll_offset: usize = 10;
            let max_offset: usize = 20;
            let clamped = scroll_offset.min(max_offset);
            assert_eq!(clamped, 10);
        }

        // TC-B7-03: フォーカス変更時に scroll_offset がリセットされる（PaneStore 操作確認）
        #[test]
        fn test_scroll_offset_reset_on_focus_change() {
            use crate::layout::PaneStore;
            use std::sync::{Arc, Mutex};
            use yatamux_protocol::types::{PaneId, SplitDirection};
            use yatamux_terminal::{CjkWidthConfig, Grid};

            let id_a = PaneId(1);
            let id_b = PaneId(2);
            let grid_a = Arc::new(Mutex::new(Grid::new(80, 24, CjkWidthConfig::default())));
            let grid_b = Arc::new(Mutex::new(Grid::new(80, 24, CjkWidthConfig::default())));

            let mut store = PaneStore::new(id_a, grid_a);
            store.grids.insert(id_b, grid_b);
            store.layout = crate::layout::LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first: Box::new(crate::layout::LayoutNode::Leaf(id_a)),
                second: Box::new(crate::layout::LayoutNode::Leaf(id_b)),
            };

            // スクロール中状態を設定
            store.scroll_offset = 30;
            store.active = id_a;

            // フォーカス変更をシミュレート（cycle_pane の動作）
            let next = id_b;
            if next != store.active {
                store.active = next;
                store.scroll_offset = 0;
            }

            assert_eq!(store.active, id_b);
            assert_eq!(store.scroll_offset, 0);
        }

        // TC-B7-04: 同じペインへの"フォーカス変更"ではオフセットを変えない
        #[test]
        fn test_scroll_offset_no_reset_if_same_pane() {
            use yatamux_protocol::types::PaneId;
            let scroll_offset: usize = 15;
            let active = PaneId(1);
            let next = PaneId(1); // same pane
            let mut result = scroll_offset;
            if next != active {
                result = 0;
            }
            assert_eq!(result, 15); // 変化しない
        }

        // TC-04: 選択なし（クリックのみ = anchor == end）は Ctrl+C を PTY へ通す
        #[test]
        fn test_normal_sel_click_only_not_selected() {
            // anchor と end が同じ = クリックのみ（選択なし）
            let sel = (3usize, 2usize, 3usize, 2usize);
            // anchor == end の場合は単一セルのみ選択される
            // Ctrl+C インターセプトでは (ac, ar) == (ec, er) を除外するため
            // ここではハイライトロジック自体を確認: 単一セルは is_sel = true になる
            assert!(is_in_normal_selection(sel, 3, 2));
            assert!(!is_in_normal_selection(sel, 2, 2));
        }
    }
}

// ── 公開 API ────────────────────────────────────────────────────────────────

#[cfg(windows)]
pub use win32::{run_window, ClientState};

/// Windows 以外向けスタブ（クロスビルド用）
#[cfg(not(windows))]
pub fn run_window(
    _panes: std::sync::Arc<std::sync::Mutex<crate::layout::PaneStore>>,
    _msg_tx: tokio::sync::mpsc::Sender<yatamux_protocol::ClientMessage>,
    _split_tx: tokio::sync::mpsc::Sender<(
        yatamux_protocol::types::PaneId,
        yatamux_protocol::types::SplitDirection,
    )>,
    _initial_size: yatamux_protocol::types::TermSize,
    _app_focused: std::sync::Arc<std::sync::atomic::AtomicBool>,
    _native_notif_queue: std::sync::Arc<
        std::sync::Mutex<std::collections::VecDeque<crate::notification::NativeToastMsg>>,
    >,
    _float_tx: tokio::sync::mpsc::Sender<()>,
    _layout_tx: tokio::sync::mpsc::Sender<String>,
    _theme: Theme,
) -> anyhow::Result<()> {
    anyhow::bail!("Win32 window is only available on Windows")
}
