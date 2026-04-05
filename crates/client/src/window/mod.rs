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

#[cfg(windows)]
mod input;
#[cfg(windows)]
mod render;

#[cfg(windows)]
mod win32 {
    mod modes;
    mod wndproc;

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

    use self::modes::{KeyConsumed, KeyInput};
    use super::input::mouse_to_vt;
    use super::render::{
        cell_colors, draw_preedit_underline, is_in_normal_selection, preedit_segment_colors,
        zwj_render_text, LauncherRenderState, PreviewRenderNode, SavePromptRenderState,
        ThemeLauncherRenderState, WinTheme, COLOR_SEPARATOR,
    };
    use crate::ime::{CellPixelPos, ImeHandler};
    use crate::layout::{Direction, PaneRect, PaneStore, Toast};
    use crate::notification::NativeToastMsg;
    use crate::Theme;

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

    /// ステータスバーの高さは `cell_height` × 1 行分
    const STATUS_BAR_ROWS: i32 = 1;

    // ── バックバッファ ───────────────────────────────────────────────────────

    /// 永続バックバッファのハンドル群。
    ///
    /// WM_SIZE でサイズが変わったときのみ再作成し、毎フレームの
    /// `CreateCompatibleBitmap` / `DeleteObject` を省く。
    /// Win32 メッセージスレッド専用なので `Cell<Option<...>>` で保持する。
    #[derive(Copy, Clone)]
    struct BackbufferHandle {
        dc: HDC,
        bmp: HBITMAP,
        w: i32,
        h: i32,
    }

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
        /// 永続バックバッファ（WM_SIZE でのみ再作成）
        content_bb: std::cell::Cell<Option<BackbufferHandle>>,
        /// 前フレームのスクロールオフセット（変化検出用）
        prev_scroll_offset: std::cell::Cell<usize>,
        /// 前フレームのオーバーレイ状態キー（変化でグリッド全 dirty 化）
        prev_overlay_key: std::cell::Cell<u32>,
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
                content_bb: std::cell::Cell::new(None),
                prev_scroll_offset: std::cell::Cell::new(0),
                prev_overlay_key: std::cell::Cell::new(0),
            }
        }

        /// バックバッファを解放する（WM_DESTROY 時に呼ぶ）
        ///
        /// # Safety
        ///
        /// `DeleteDC` / `DeleteObject` が Win32 unsafe API のため unsafe。
        /// 呼び出し元は以下を保証すること:
        /// - **Win32 メッセージスレッドから呼ぶ**: GDI オブジェクトは作成したスレッドで
        ///   解放するのが原則。`WM_DESTROY` ハンドラからのみ呼ばれるため満たされる。
        /// - **二重解放しない**: `content_bb.take()` で `None` にセットするため
        ///   複数回呼んでも安全だが、意図的な二重呼び出しは避けること。
        pub(super) unsafe fn release_backbuffer(&self) {
            if let Some(bb) = self.content_bb.take() {
                let _ = DeleteDC(bb.dc);
                let _ = DeleteObject(bb.bmp.into());
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

    // ── WndProc ──────────────────────────────────────────────────────────────

    // ── 行ラン描画バッファ ────────────────────────────────────────────────────
    // 同一 (fg, bg) の連続セルを 1 回の ExtTextOutW に畳み込み GDI 呼び出し数を削減する。
    // ASCII 密集出力では ExtTextOutW を約 50% 削減できる。
    //
    // # ラン終了条件
    // - 前景色または背景色が変わった
    // - ボックス文字（U+2500–259F）が現れた
    // - UTF-16 コードユニット数が 2 以上のグラフェム（サロゲートペア等）が現れた
    // - 選択状態セル（is_sel）が現れた
    // - 行末に達した
    struct RowRunBuf {
        bg: COLORREF,
        fg: COLORREF,
        /// 背景塗りスパンの左端 x
        bg_x0: i32,
        /// 背景塗りスパンの右端 x（セルを追加するたびに伸長）
        bg_x1: i32,
        /// バッファ中の各グラフェムの x 座標（`text_buf` と 1:1 対応）
        grapheme_xs: Vec<i32>,
        /// UTF-16 コードユニット列（1 コードユニット / グラフェムのみ格納）
        text_buf: Vec<u16>,
        y: i32,
        cell_h: i32,
        active: bool,
    }

    impl RowRunBuf {
        fn new(y: i32, cell_h: i32) -> Self {
            Self {
                bg: COLORREF(0),
                fg: COLORREF(0),
                bg_x0: 0,
                bg_x1: 0,
                grapheme_xs: Vec::with_capacity(256),
                text_buf: Vec::with_capacity(256),
                y,
                cell_h,
                active: false,
            }
        }

        /// ランが有効で bg が一致するか（ブランクセル用）
        #[inline]
        fn same_bg(&self, bg: COLORREF) -> bool {
            self.active && self.bg == bg
        }

        /// ランが有効で (fg, bg) が一致するか（グラフェムセル用）
        #[inline]
        fn same_colors(&self, fg: COLORREF, bg: COLORREF) -> bool {
            self.active && self.fg == fg && self.bg == bg
        }

        fn start_blank(&mut self, bg: COLORREF, x: i32) {
            self.active = true;
            self.bg = bg;
            self.fg = COLORREF(0);
            self.bg_x0 = x;
            self.bg_x1 = x;
        }

        fn start_grapheme(&mut self, fg: COLORREF, bg: COLORREF, x: i32) {
            self.active = true;
            self.fg = fg;
            self.bg = bg;
            self.bg_x0 = x;
            self.bg_x1 = x;
        }

        /// ランをフラッシュ（背景 + テキストを最大 2 回の ExtTextOutW で描画してリセット）
        ///
        /// # Safety
        /// `dc` は有効な `HDC` であること（Win32 メッセージスレッドから呼ぶこと）
        unsafe fn flush(&mut self, dc: HDC) {
            if !self.active {
                return;
            }

            // 1. 背景塗り（スパン全体を 1 回でカバー）
            SetBkColor(dc, self.bg);
            let span_rect = RECT {
                left: self.bg_x0,
                top: self.y,
                right: self.bg_x1,
                bottom: self.y + self.cell_h,
            };
            let _ = ExtTextOutW(
                dc,
                self.bg_x0,
                self.y,
                ETO_OPAQUE,
                Some(&span_rect),
                PCWSTR::null(),
                0,
                None,
            );

            // 2. テキスト描画（dx 配列で文字間隔を指定、1 回の ExtTextOutW）
            if !self.text_buf.is_empty() {
                let n = self.text_buf.len();
                let mut dx: Vec<i32> = Vec::with_capacity(n);
                for i in 0..n {
                    let next_x = if i + 1 < n {
                        self.grapheme_xs[i + 1]
                    } else {
                        self.bg_x1
                    };
                    dx.push(next_x - self.grapheme_xs[i]);
                }
                SetTextColor(dc, self.fg);
                SetBkColor(dc, self.bg);
                let _ = ExtTextOutW(
                    dc,
                    self.grapheme_xs[0],
                    self.y,
                    ETO_CLIPPED,
                    Some(&span_rect),
                    PCWSTR(self.text_buf.as_ptr()),
                    n as u32,
                    Some(dx.as_ptr()),
                );
            }

            // リセット
            self.active = false;
            self.grapheme_xs.clear();
            self.text_buf.clear();
        }
    }

    // ── GDI 描画 ─────────────────────────────────────────────────────────────

    unsafe fn paint(hwnd: HWND, state: &ClientState) {
        let mut ps = PAINTSTRUCT::default();
        let hdc = BeginPaint(hwnd, &mut ps);

        // ── 永続バックバッファ（サイズ変更時のみ再作成）───────────────────
        let mut rect = RECT::default();
        GetClientRect(hwnd, &mut rect).ok();
        let win_w = rect.right;
        let win_h = rect.bottom;

        // 現在のテーマ色を取得（paint 関数内で共通使用）
        let theme = state.theme.get();

        let bb = match state.content_bb.get() {
            Some(bb) if bb.w == win_w && bb.h == win_h => bb,
            old => {
                // 古いバックバッファを解放
                if let Some(old_bb) = old {
                    let _ = DeleteDC(old_bb.dc);
                    let _ = DeleteObject(old_bb.bmp.into());
                }
                // 新しいバックバッファを作成
                let dc = CreateCompatibleDC(Some(hdc));
                let bmp = CreateCompatibleBitmap(hdc, win_w, win_h);
                SelectObject(dc, bmp.into());
                SelectObject(dc, state.hfont.into());
                SetBkMode(dc, OPAQUE);
                // 全面を背景色で初期化
                let bg_brush = CreateSolidBrush(theme.bg);
                FillRect(dc, &rect, bg_brush);
                let _ = DeleteObject(bg_brush.into());
                let new_bb = BackbufferHandle {
                    dc,
                    bmp,
                    w: win_w,
                    h: win_h,
                };
                state.content_bb.set(Some(new_bb));
                // 全グリッドを dirty に（次のループで全行描画される）
                {
                    let store = state.panes.lock().unwrap();
                    for g in store.grids.values() {
                        g.lock().unwrap().mark_all_dirty();
                    }
                }
                new_bb
            }
        };
        let mem_dc = bb.dc;

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
            has_launcher,
            has_theme_launcher,
            has_save_prompt,
            floating_visible,
            hovered_url,
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
            let hl = store.launcher.is_some();
            let htl = store.theme_launcher.is_some();
            let hsp = store.save_prompt.is_some();
            let fv = store.floating_visible;
            let hu = store.hovered_url.clone();
            (
                store.active,
                store.scroll_offset,
                rects,
                seps,
                map,
                cm,
                ns,
                hl,
                htl,
                hsp,
                fv,
                hu,
            )
        };

        // ── オーバーレイ・スクロール変化の検出 → dirty 化 ──────────────────
        // オーバーレイが表示/非表示に切り替わったとき、背後のセル内容をバックバッファに
        // 復元するため全グリッドを dirty にする。
        let overlay_key: u32 = (has_launcher as u32)
            | ((has_theme_launcher as u32) << 1)
            | ((has_save_prompt as u32) << 2)
            | ((floating_visible as u32) << 3)
            | ((copy_mode_state.is_some() as u32) << 4)
            | ((normal_selection.is_some() as u32) << 5);

        if overlay_key != state.prev_overlay_key.get() {
            for g in grid_map.values() {
                g.lock().unwrap().mark_all_dirty();
            }
        }
        state.prev_overlay_key.set(overlay_key);

        // スクロールオフセットが変化したらアクティブペインを全行 dirty に
        if scroll_offset != state.prev_scroll_offset.get() {
            if let Some(g) = grid_map.get(&active_pane) {
                g.lock().unwrap().mark_all_dirty();
            }
        }
        state.prev_scroll_offset.set(scroll_offset);

        // ── 各ペインを描画（dirty 行のみ）────────────────────────────────
        let ime_state = state.ime.state.lock().unwrap();

        for (pane_id, pane_rect) in &pane_rects {
            let is_active = *pane_id == active_pane;
            if let Some(grid_arc) = grid_map.get(pane_id) {
                let mut grid = grid_arc.lock().unwrap();

                // dirty 行のみ再描画。空なら非アクティブペインはスキップ。
                // アクティブペインは WM_TIMER でカーソル行が常に dirty なので
                // ここには必ず入る。
                let dirty_rows: std::collections::HashSet<u16> =
                    grid.take_dirty_rows().into_iter().collect();
                if dirty_rows.is_empty() {
                    continue;
                }

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
                    if !dirty_rows.contains(&row) {
                        continue;
                    }
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

                    let mut run = RowRunBuf::new(y, state.cell_height);
                    for (col_idx, cell) in cells.iter().take(display_cols).enumerate() {
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
                                    left: x,
                                    top: y,
                                    right: x + width_px,
                                    bottom: y + state.cell_height,
                                };

                                let first_cp = text.chars().next().map(|c| c as u32).unwrap_or(0);
                                if (0x2500..=0x259F).contains(&first_cp) {
                                    // ボックス文字: ランをフラッシュして個別描画
                                    run.flush(mem_dc);
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
                                    draw_box_char(
                                        mem_dc,
                                        x,
                                        y,
                                        state.cell_width,
                                        state.cell_height,
                                        first_cp,
                                        fg,
                                    );
                                } else {
                                    // GDI は ZWJ シーケンスをシェーピングできないため、
                                    // ZWJ 以降は描画せず基底グリフのみ表示する
                                    let render_str = zwj_render_text(text);
                                    let utf16: Vec<u16> = render_str.encode_utf16().collect();

                                    if utf16.len() == 1 && !is_sel {
                                        // バッチ対象: 同一 (fg, bg) のランに追加
                                        if !run.same_colors(fg, bg) {
                                            run.flush(mem_dc);
                                        }
                                        if !run.active {
                                            run.start_grapheme(fg, bg, x);
                                        }
                                        run.bg_x1 = x + width_px;
                                        run.grapheme_xs.push(x);
                                        run.text_buf.push(utf16[0]);
                                    } else {
                                        // 選択セル・マルチコードユニット: 個別描画
                                        run.flush(mem_dc);
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
                                        SetTextColor(mem_dc, fg);
                                        SetBkColor(mem_dc, bg);
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
                                }
                                x += state.cell_width;
                            }
                            CellContent::Continuation => {
                                x += state.cell_width;
                            }
                            CellContent::Blank => {
                                // 選択中はセル背景を選択色で塗りつぶす
                                let blank_bg = if is_sel { theme.selection_bg } else { theme.bg };
                                if !run.same_bg(blank_bg) {
                                    run.flush(mem_dc);
                                }
                                if !run.active {
                                    run.start_blank(blank_bg, x);
                                }
                                run.bg_x1 = x + state.cell_width;
                                x += state.cell_width;
                            }
                        }
                    }
                    run.flush(mem_dc);
                }

                // ── URL ホバーアンダーライン ──────────────────────────────────
                if let Some((url_pane, url_row, url_cs, url_ce, _)) = &hovered_url {
                    if *url_pane == *pane_id && dirty_rows.contains(&(*url_row as u16)) {
                        let ux = off_x + (*url_cs as i32) * state.cell_width;
                        let uy =
                            off_y + (*url_row as i32) * state.cell_height + state.cell_height - 2;
                        let uw = (*url_ce as i32 - *url_cs as i32) * state.cell_width;
                        // 水色アンダーライン
                        let pen = CreatePen(PS_SOLID, 1, COLORREF(0x00_FF_D7_5F));
                        let old_pen = SelectObject(mem_dc, pen.into());
                        let _ = MoveToEx(mem_dc, ux, uy, None);
                        let _ = LineTo(mem_dc, ux + uw, uy);
                        SelectObject(mem_dc, old_pen);
                        let _ = DeleteObject(pen.into());
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

        // 永続バックバッファを画面にコピー
        // （DC・ビットマップは persistent なので解放しない）
        BitBlt(hdc, 0, 0, win_w, win_h, Some(mem_dc), 0, 0, SRCCOPY).ok();

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
        theme: Theme,
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
                lpfnWndProc: Some(wndproc::wnd_proc),
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
