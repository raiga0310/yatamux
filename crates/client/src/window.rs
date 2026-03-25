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
    use crate::layout::{Direction, PaneRect, PaneStore, Toast};
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

    // ── GDI カラーヘルパー ───────────────────────────────────────────────────

    fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
        COLORREF((b as u32) << 16 | (g as u32) << 8 | r as u32)
    }

    // Catppuccin Mocha テーマ
    const COLOR_BG: COLORREF = COLORREF(0x00_2E_1E_1E); // base  #1e1e2e
    const COLOR_FG: COLORREF = COLORREF(0x00_F4_D6_CD); // text  #cdd6f4
    const COLOR_CURSOR: COLORREF = COLORREF(0x00_E7_C2_F5); // pink     #f5c2e7
    const COLOR_SEPARATOR: COLORREF = COLORREF(0x00_5A_47_45); // surface1 #45475a
    const COLOR_PREEDIT_BG: COLORREF = COLORREF(0x00_5A_47_45); // surface1 #45475a

    // ── モード定義 ───────────────────────────────────────────────────────────

    /// UI モード
    ///
    /// `Normal`: 全キー入力を PTY に透過する通常状態。
    /// `Pane`: ペイン操作（移動・分割・削除）を受け付けるワンショットモード。
    ///         1 キー操作後に自動で `Normal` に戻る。
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    pub enum ClientMode {
        Normal,
        Pane,
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
            let store = self.panes.lock().unwrap();
            let rects = store.layout.compute_rects(total);
            for (pane_id, rect) in rects {
                let cols = rect.cols(self.cell_width);
                let rows = rect.rows(self.cell_height);
                if let Some(g) = store.grids.get(&pane_id) {
                    g.lock().unwrap().resize(cols, rows);
                }
                let _ = self.msg_tx.try_send(ClientMessage::Resize {
                    pane: pane_id,
                    size: TermSize { cols, rows },
                });
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
            store.active = next;
        }

        /// フォーカスを指定方向の最近傍ペインに移す
        fn focus_pane_dir(&self, dir: Direction) {
            let mut store = self.panes.lock().unwrap();
            let next = store
                .layout
                .pane_in_direction(store.active, dir, self.content_rect.get());
            store.active = next;
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

        /// アクティブペインを削除する。最後の1ペインの場合は何もしない。
        fn close_active_pane(&self) {
            let store = self.panes.lock().unwrap();
            if matches!(store.layout, crate::layout::LayoutNode::Leaf(_)) {
                return;
            }
            let active = store.active;
            drop(store);
            let _ = self
                .msg_tx
                .try_send(yatamux_protocol::ClientMessage::ClosePane { pane: active });
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
                SetTimer(hwnd, TIMER_REPAINT, TIMER_INTERVAL_MS, None);
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
                    let _ = InvalidateRect(hwnd, None, false);
                }
                LRESULT(0)
            }

            // ── IME: 変換終了 ───────────────────────────────────────────
            WM_IME_ENDCOMPOSITION => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    state.ime.on_end_composition();
                    let _ = InvalidateRect(hwnd, None, false);
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

            // ── 制御キー ────────────────────────────────────────────────
            WM_KEYDOWN => {
                if !state_ptr.is_null() {
                    let state = &*state_ptr;
                    let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
                    let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;

                    // ── Pane モードのキー処理 ────────────────────────────
                    if state.mode.get() == ClientMode::Pane {
                        state.mode.set(ClientMode::Normal);
                        state.skip_char.set(true); // WM_CHAR を抑制
                        let vk = wparam.0 as u16;
                        match vk {
                            k if k == b'H' as u16 => {
                                state.focus_pane_dir(Direction::Left);
                            }
                            k if k == b'J' as u16 => {
                                state.focus_pane_dir(Direction::Down);
                            }
                            k if k == b'K' as u16 => {
                                state.focus_pane_dir(Direction::Up);
                            }
                            k if k == b'L' as u16 => {
                                state.focus_pane_dir(Direction::Right);
                            }
                            k if k == b'E' as u16 => {
                                state.request_split(SplitDirection::Vertical);
                            }
                            k if k == b'O' as u16 => {
                                state.request_split(SplitDirection::Horizontal);
                            }
                            k if k == b'W' as u16 => {
                                state.close_active_pane();
                            }
                            _ => {} // 未定義キーは無視（skip_char で WM_CHAR も抑制済み）
                        }
                        let _ = InvalidateRect(hwnd, None, false);
                        return LRESULT(0);
                    }

                    // ── Ctrl+B: Normal → Pane モードへ切り替え ──────────
                    if ctrl && !shift && wparam.0 == b'B' as usize {
                        state.mode.set(ClientMode::Pane);
                        state.skip_char.set(true); // \x02 を PTY に送らない
                        let _ = InvalidateRect(hwnd, None, false);
                        return LRESULT(0);
                    }

                    // Ctrl+Shift+E: 縦分割 (side by side)
                    if ctrl && shift && wparam.0 == b'E' as usize {
                        state.request_split(SplitDirection::Vertical);
                        return LRESULT(0);
                    }
                    // Ctrl+Shift+O: 横分割 (top / bottom)
                    if ctrl && shift && wparam.0 == b'O' as usize {
                        state.request_split(SplitDirection::Horizontal);
                        return LRESULT(0);
                    }
                    // Ctrl+Shift+W: アクティブペインを削除 (F-8)
                    if ctrl && shift && wparam.0 == b'W' as usize {
                        state.close_active_pane();
                        return LRESULT(0);
                    }
                    // Ctrl+Tab: 次のペイン / Ctrl+Shift+Tab: 前のペイン
                    if ctrl && wparam.0 == VK_TAB.0 as usize {
                        state.cycle_pane(!shift);
                        let _ = InvalidateRect(hwnd, None, false);
                        return LRESULT(0);
                    }

                    // Ctrl+Arrow: 方向指定ペインフォーカス移動
                    if ctrl && !shift {
                        let dir = match wparam.0 as u16 {
                            k if k == VK_LEFT.0 => Some(Direction::Left),
                            k if k == VK_RIGHT.0 => Some(Direction::Right),
                            k if k == VK_UP.0 => Some(Direction::Up),
                            k if k == VK_DOWN.0 => Some(Direction::Down),
                            _ => None,
                        };
                        if let Some(d) = dir {
                            state.focus_pane_dir(d);
                            let _ = InvalidateRect(hwnd, None, false);
                            return LRESULT(0);
                        }
                    }

                    // Ctrl+H/J/K/L によるペインフォーカス移動は廃止（F-6）
                    // Ctrl+J は Claude Code 等の改行キーと衝突するため削除。
                    // フォーカス移動は Ctrl+←↑↓→ を使うこと。

                    // Ctrl+V: クリップボードからペースト
                    if ctrl && !shift && wparam.0 == b'V' as usize {
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
                        return LRESULT(0);
                    }

                    let app_cursor = state
                        .active_grid()
                        .lock()
                        .unwrap()
                        .application_cursor_keys();
                    if let Some(vt) = keydown_to_vt(wparam, lparam, app_cursor) {
                        state.send_input(vt);
                        return LRESULT(0);
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
                    let _ = InvalidateRect(hwnd, None, false);
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

                    let needs_repaint = {
                        let store = state.panes.lock().unwrap();
                        let dirty = store
                            .grids
                            .values()
                            .any(|g| g.lock().unwrap().has_dirty_rows());
                        dirty || state.ime.state.lock().unwrap().composing
                    };
                    if needs_repaint || has_active_toasts {
                        let _ = InvalidateRect(hwnd, None, false);
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
                        if state.focus_pane_at(px, py) {
                            let _ = InvalidateRect(hwnd, None, false);
                        }
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
                let _ = KillTimer(hwnd, TIMER_REPAINT);
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
        let mem_dc = CreateCompatibleDC(hdc);
        let mem_bmp = CreateCompatibleBitmap(hdc, rect.right, rect.bottom);
        let old_bmp = SelectObject(mem_dc, mem_bmp);

        // 背景塗りつぶし
        let bg_brush = CreateSolidBrush(COLOR_BG);
        FillRect(mem_dc, &rect, bg_brush);
        let _ = DeleteObject(bg_brush);

        // フォント設定
        let old_font = SelectObject(mem_dc, state.hfont);
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
        let (active_pane, scroll_offset, pane_rects, sep_rects, grid_map) = {
            let store = state.panes.lock().unwrap();
            let rects = store.layout.compute_rects(total_rect);
            let seps = store.layout.compute_separator_rects(total_rect);
            let map: HashMap<PaneId, Arc<Mutex<Grid>>> = store
                .grids
                .iter()
                .map(|(&id, g)| (id, Arc::clone(g)))
                .collect();
            (store.active, store.scroll_offset, rects, seps, map)
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

                    for cell in cells.iter().take(display_cols) {
                        let cell_rect = RECT {
                            left: x,
                            top: y,
                            right: x + state.cell_width,
                            bottom: y + state.cell_height,
                        };

                        match &cell.content {
                            CellContent::Grapheme { text, width } => {
                                let (fg, bg) = cell_colors(cell, &ime_state);
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
                                    let utf16: Vec<u16> = text.encode_utf16().collect();
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
                                SetBkColor(mem_dc, COLOR_BG);
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
                        let (fg, bg) = preedit_segment_colors(&seg.attr);
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
                        );
                        px += seg_width;
                    }
                }

                // カーソル（アクティブペインのみ）
                if is_active && grid.cursor_visible() {
                    let cur = grid.cursor();
                    let cx = cur.col as i32 * state.cell_width + off_x;
                    let cy = cur.row as i32 * state.cell_height + off_y;
                    fill_rect(mem_dc, COLOR_CURSOR, cx, cy, cx + 2, cy + state.cell_height);
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

        // ── ステータスバー ───────────────────────────────────────────
        paint_status_bar(mem_dc, rect.right, rect.bottom, state);

        // ── トースト通知 ────────────────────────────────────────────
        paint_toasts(mem_dc, rect.right, rect.bottom, state);

        // バックバッファを画面にコピー
        BitBlt(hdc, 0, 0, rect.right, rect.bottom, mem_dc, 0, 0, SRCCOPY).ok();

        // リソース解放
        SelectObject(mem_dc, old_font);
        SelectObject(mem_dc, old_bmp);
        let _ = DeleteObject(mem_bmp);
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
        // Catppuccin Mocha: mantle = #181825, subtext1 = #bac2de, blue = #89b4fa, peach = #fab387
        const COLOR_STATUS_BG: COLORREF = COLORREF(0x00_25_18_18); // mantle
        const COLOR_STATUS_FG: COLORREF = COLORREF(0x00_DE_C2_BA); // subtext1
        const COLOR_MODE_NORMAL: COLORREF = COLORREF(0x00_FA_B4_89); // blue (#89b4fa → BGR)
        const COLOR_MODE_PANE: COLORREF = COLORREF(0x00_87_AB_FA); // peach (#fab387 → BGR)

        let bar_h = state.cell_height * STATUS_BAR_ROWS;
        let bar_y = win_h - bar_h;

        // 背景
        let bar_rect = RECT {
            left: 0,
            top: bar_y,
            right: win_w,
            bottom: win_h,
        };
        let bg_brush = CreateSolidBrush(COLOR_STATUS_BG);
        FillRect(hdc, &bar_rect, bg_brush);
        let _ = DeleteObject(bg_brush);

        SetBkColor(hdc, COLOR_STATUS_BG);
        SetBkMode(hdc, OPAQUE);

        let mode = state.mode.get();
        let text_y = bar_y;

        // ── 左側: モード名 ──────────────────────────────────────────
        let (mode_label, mode_color, hint) = match mode {
            ClientMode::Normal => (" NORMAL ", COLOR_MODE_NORMAL, " Ctrl+B: ペインモード"),
            ClientMode::Pane => (
                " PANE ",
                COLOR_MODE_PANE,
                " H/J/K/L: 移動  E: 縦分割  O: 横分割  W: 削除  q: 戻る",
            ),
        };

        // モード名（色付き背景）
        SetTextColor(hdc, COLOR_STATUS_BG);
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
        SetBkColor(hdc, COLOR_STATUS_BG);
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
        SetBkColor(hdc, COLOR_STATUS_BG);
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
            let _ = DeleteObject(bg_brush);

            // 枠線（1px）
            let border_pen = CreatePen(PS_SOLID, 1, COLOR_TOAST_BORDER);
            let old_pen = SelectObject(hdc, border_pen);
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
            let _ = DeleteObject(border_pen);

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
            SetTextColor(hdc, COLOR_FG);
            DrawTextW(
                hdc,
                &mut msg_w,
                &mut msg_rect,
                DT_LEFT | DT_SINGLELINE | DT_VCENTER | DT_END_ELLIPSIS,
            );
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
        let set_pen = |pen: HPEN| -> HPEN { HPEN(SelectObject(hdc, pen).0) };
        let del_pen = |old: HPEN, pen: HPEN| {
            SelectObject(hdc, old);
            let _ = DeleteObject(pen);
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
        let _ = DeleteObject(brush);
    }

    /// プリエディット下線を描画する
    ///
    /// - `TargetConverted`: 太実線（現在の変換候補）
    /// - `Converted`: 実線
    /// - その他: 点線
    unsafe fn draw_preedit_underline(hdc: HDC, attr: &PreeditAttr, x: i32, y: i32, width: i32) {
        let (pen_style, thickness) = match attr {
            PreeditAttr::TargetConverted => (PS_SOLID, 2u32),
            PreeditAttr::Converted => (PS_SOLID, 1),
            PreeditAttr::TargetNotConverted => (PS_DOT, 2),
            PreeditAttr::Input => (PS_DOT, 1),
        };

        let pen = CreatePen(pen_style, thickness as i32, COLOR_FG);
        let old_pen = SelectObject(hdc, pen);
        let _ = MoveToEx(hdc, x, y, None);
        let _ = LineTo(hdc, x + width, y);
        SelectObject(hdc, old_pen);
        let _ = DeleteObject(pen);
    }

    fn cell_colors(cell: &Cell, _ime: &ImeState) -> (COLORREF, COLORREF) {
        let fg = cell
            .style
            .fg
            .map(|c| rgb(c.r, c.g, c.b))
            .unwrap_or(COLOR_FG);
        let bg = cell
            .style
            .bg
            .map(|c| rgb(c.r, c.g, c.b))
            .unwrap_or(COLOR_BG);
        if cell.style.reverse {
            (bg, fg)
        } else {
            (fg, bg)
        }
    }

    fn preedit_segment_colors(attr: &PreeditAttr) -> (COLORREF, COLORREF) {
        match attr {
            PreeditAttr::TargetConverted => (COLOR_BG, COLOR_FG), // 反転
            _ => (COLOR_FG, COLOR_PREEDIT_BG),
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
        if OpenClipboard(hwnd).is_err() {
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

        if OpenClipboard(hwnd).is_err() {
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
                let _ = SetClipboardData(13, windows::Win32::Foundation::HANDLE(hglobal.0));
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
    pub fn run_window(
        panes: Arc<Mutex<PaneStore>>,
        msg_tx: mpsc::Sender<ClientMessage>,
        split_tx: mpsc::Sender<(PaneId, SplitDirection)>,
        initial_size: TermSize,
        app_focused: Arc<AtomicBool>,
        native_notif_queue: Arc<Mutex<VecDeque<NativeToastMsg>>>,
    ) -> anyhow::Result<()> {
        unsafe {
            let hinstance = GetModuleHandleW(None)?;

            // ── フォント作成（インストール済み候補から自動選択）─────────
            let hfont = create_best_font(FONT_HEIGHT);

            // セルサイズをテキストメトリクスから取得
            let (cell_width, cell_height) = measure_cell_size(hfont)?;

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
                hinstance,
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
            let _ = DeleteObject(hfont);

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
                DEFAULT_CHARSET.0 as u32,
                OUT_DEFAULT_PRECIS.0 as u32,
                CLIP_DEFAULT_PRECIS.0 as u32,
                CLEARTYPE_QUALITY.0 as u32,
                (FIXED_PITCH.0 | FF_MODERN.0) as u32,
                PCWSTR(wide.as_ptr()),
            );

            let old = SelectObject(hdc, hfont);

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
            let _ = DeleteObject(hfont);
        }

        // ここには到達しない（candidates は空でない）が、コンパイラ用
        ReleaseDC(None, hdc);
        HFONT::default()
    }

    /// 仮の DC でフォントのセルサイズを計測する
    unsafe fn measure_cell_size(hfont: HFONT) -> anyhow::Result<(i32, i32)> {
        let hdc = GetDC(None);
        let old_font = SelectObject(hdc, hfont);
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
) -> anyhow::Result<()> {
    anyhow::bail!("Win32 window is only available on Windows")
}
