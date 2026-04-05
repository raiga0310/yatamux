use super::*;

pub(super) unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ClientState;

    match msg {
        WM_CREATE => handle_wm_create(hwnd, lparam),
        WM_PAINT => {
            if state_ptr.is_null() {
                return DefWindowProcW(hwnd, msg, wparam, lparam);
            }
            let state = &*state_ptr;
            paint(hwnd, state);
            LRESULT(0)
        }
        WM_IME_STARTCOMPOSITION => handle_ime_start(state_ptr),
        WM_IME_COMPOSITION => handle_ime_composition(state_ptr, hwnd, lparam),
        WM_IME_ENDCOMPOSITION => handle_ime_end(state_ptr, hwnd, msg, wparam, lparam),
        WM_CHAR => handle_wm_char(state_ptr, hwnd, wparam),
        WM_KEYDOWN => handle_wm_keydown(state_ptr, hwnd, wparam, lparam, msg),
        WM_SIZE => handle_wm_size(state_ptr, lparam),
        WM_MOUSEWHEEL => handle_wm_mousewheel(state_ptr, hwnd, wparam),
        WM_TIMER => handle_wm_timer(state_ptr, hwnd, wparam),
        WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_LBUTTONUP | WM_RBUTTONUP | WM_MOUSEMOVE => {
            handle_mouse_message(state_ptr, hwnd, msg, wparam, lparam)
        }
        WM_ACTIVATEAPP => handle_activate_app(state_ptr, hwnd, msg, wparam, lparam),
        WM_SETFOCUS => handle_focus(state_ptr, hwnd, msg, wparam, lparam, true),
        WM_KILLFOCUS => handle_focus(state_ptr, hwnd, msg, wparam, lparam, false),
        WM_CLOSE => handle_wm_close(state_ptr, hwnd),
        WM_DESTROY => handle_wm_destroy(hwnd),
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}

unsafe fn handle_wm_create(hwnd: HWND, lparam: LPARAM) -> LRESULT {
    let cs = &*(lparam.0 as *const CREATESTRUCTW);
    SetWindowLongPtrW(hwnd, GWLP_USERDATA, cs.lpCreateParams as isize);
    SetTimer(Some(hwnd), TIMER_REPAINT, TIMER_INTERVAL_MS, None);

    let dark: i32 = 1;
    let _ = DwmSetWindowAttribute(
        hwnd,
        DWMWINDOWATTRIBUTE(20), // DWMWA_USE_IMMERSIVE_DARK_MODE
        &dark as *const i32 as *const _,
        std::mem::size_of::<i32>() as u32,
    );
    LRESULT(0)
}

unsafe fn handle_ime_start(state_ptr: *mut ClientState) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        state.ime.on_start_composition();
    }
    LRESULT(0)
}

unsafe fn handle_ime_composition(
    state_ptr: *mut ClientState,
    hwnd: HWND,
    lparam: LPARAM,
) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        state.ime.on_composition(hwnd, lparam.0 as usize);

        let committed = {
            let s = state.ime.state.lock().unwrap();
            s.committed.clone()
        };
        if let Some(text) = committed {
            state.send_input(text.into_bytes());
            state.ime.state.lock().unwrap().committed = None;
        }

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

        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    LRESULT(0)
}

unsafe fn handle_ime_end(
    state_ptr: *mut ClientState,
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        state.ime.on_end_composition();
        let _ = InvalidateRect(Some(hwnd), None, false);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

pub(super) unsafe fn handle_wm_char(
    state_ptr: *mut ClientState,
    hwnd: HWND,
    wparam: WPARAM,
) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        if state.skip_char.get() {
            state.skip_char.set(false);
            return LRESULT(0);
        }
        {
            let save_open = state.panes.lock().unwrap().save_prompt.is_some();
            if save_open {
                let code = wparam.0 as u32;
                if let Some(ch) = char::from_u32(code) {
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
        if !state.ime.state.lock().unwrap().composing {
            let code = wparam.0 as u32;
            let ctrl = GetKeyState(VK_CONTROL.0 as i32) < 0;
            let shift = GetKeyState(VK_SHIFT.0 as i32) < 0;
            let skip = matches!(code, 8 | 9) || (ctrl && shift);
            if !skip {
                if let Some(ch) = char::from_u32(code) {
                    if ch != '\0' {
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

pub(super) unsafe fn handle_wm_keydown(
    state_ptr: *mut ClientState,
    hwnd: HWND,
    wparam: WPARAM,
    lparam: LPARAM,
    msg: u32,
) -> LRESULT {
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

unsafe fn handle_wm_size(state_ptr: *mut ClientState, lparam: LPARAM) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        let width = (lparam.0 & 0xFFFF) as i32;
        let height = ((lparam.0 >> 16) & 0xFFFF) as i32;
        if state.cell_width > 0 && state.cell_height > 0 {
            let content_w = (width - PADDING_X * 2).max(1);
            let status_h = state.cell_height * STATUS_BAR_ROWS;
            let content_h = (height - PADDING_Y * 2 - status_h).max(1);
            state.content_rect.set(PaneRect {
                x: 0,
                y: 0,
                w: content_w,
                h: content_h,
            });
            state.resize_all_panes(content_w, content_h);
            // バックバッファを無効化（次の WM_PAINT で再作成される）
            state.content_bb.set(None);
        }
    }
    LRESULT(0)
}

unsafe fn handle_wm_mousewheel(state_ptr: *mut ClientState, hwnd: HWND, wparam: WPARAM) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        let delta = (wparam.0 >> 16) as i16;
        let lines: usize = 3;
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

pub(super) unsafe fn handle_wm_timer(
    state_ptr: *mut ClientState,
    hwnd: HWND,
    wparam: WPARAM,
) -> LRESULT {
    if wparam.0 == TIMER_REPAINT && !state_ptr.is_null() {
        let state = &*state_ptr;

        let clip = state.panes.lock().unwrap().pending_clipboard.take();
        if let Some(data) = clip {
            write_clipboard_text(hwnd, &data);
        }

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

        if let Some(msg) = state.native_notif_queue.lock().unwrap().pop_front() {
            show_balloon_notification(hwnd, &msg.title, &msg.body);
            state.notif_icon_timer.set(300);
        }
        let t = state.notif_icon_timer.get();
        if t > 0 {
            state.notif_icon_timer.set(t - 1);
            if t == 1 {
                remove_tray_icon(hwnd);
            }
        }

        let quit = state.panes.lock().unwrap().should_quit;
        if quit {
            let _ = DestroyWindow(hwnd);
            return LRESULT(0);
        }

        // レイアウト変更（split / close）後はバックバッファを破棄して全画面再描画する。
        // これにより旧ペイン領域の残像（端数ピクセルを含む）が消える（F-5）。
        {
            let mut store = state.panes.lock().unwrap();
            if store.layout_changed {
                store.layout_changed = false;
                drop(store);
                state.content_bb.set(None);
            }
        }

        // カーソル行を常に dirty に（永続バックバッファ上のカーソル描画を毎フレーム更新）
        {
            let store = state.panes.lock().unwrap();
            if let Some(g) = store.grids.get(&store.active) {
                let mut grid = g.lock().unwrap();
                if grid.cursor_visible() {
                    let row = grid.cursor().row as usize;
                    grid.mark_dirty(row);
                }
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
            let _ = InvalidateRect(Some(hwnd), None, false);
        }
    }
    LRESULT(0)
}

pub(super) unsafe fn handle_mouse_message(
    state_ptr: *mut ClientState,
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;

        if msg == WM_LBUTTONDOWN {
            let px = (lparam.0 & 0xFFFF) as i32;
            let py = ((lparam.0 >> 16) & 0xFFFF) as i32;

            state.panes.lock().unwrap().normal_selection = None;
            state.normal_dragging.set(false);

            let float_handled = {
                let mut store = state.panes.lock().unwrap();
                if store.floating_visible {
                    let content = state.content_rect.get();
                    let fr = PaneStore::floating_rect(content);
                    let cx = (px - PADDING_X).max(0);
                    let cy = (py - PADDING_Y).max(0);
                    if cx < fr.x || cx >= fr.x + fr.w || cy < fr.y || cy >= fr.y + fr.h {
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
                            if cx >= pr.x && cx < pr.x + pr.w && cy >= pr.y && cy < pr.y + pr.h {
                                let col = ((cx - pr.x) / state.cell_width.max(1)) as usize;
                                let row = ((cy - pr.y) / state.cell_height.max(1)) as usize;
                                Some((col, row))
                            } else {
                                None
                            }
                        })
                };
                if let Some((col, row)) = sel_start {
                    state.panes.lock().unwrap().normal_selection = Some((col, row, col, row));
                    state.normal_dragging.set(true);
                }
            }
        }

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

        if msg == WM_LBUTTONUP && state.normal_dragging.get() {
            state.normal_dragging.set(false);
        }

        let (reporting, sgr) = {
            let g = state.active_grid();
            let g = g.lock().unwrap();
            (g.mouse_reporting(), g.mouse_sgr())
        };
        let is_motion = msg == WM_MOUSEMOVE;
        let btn_down = matches!(msg, WM_LBUTTONDOWN | WM_RBUTTONDOWN);
        let btn_up = matches!(msg, WM_LBUTTONUP | WM_RBUTTONUP);
        let send = match reporting {
            0 => false,
            1 => btn_down,
            2 => btn_down || btn_up || is_motion,
            _ => btn_down || btn_up || is_motion,
        };
        if send {
            let px = (lparam.0 & 0xFFFF) as i32;
            let py = ((lparam.0 >> 16) & 0xFFFF) as i32;
            let col = ((px - PADDING_X) / state.cell_width.max(1)).max(0) as u16 + 1;
            let row = ((py - PADDING_Y) / state.cell_height.max(1)).max(0) as u16 + 1;
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
            if let Some(data) = mouse_to_vt(base_btn, col, row, btn_up && !is_motion, sgr) {
                state.send_input(data);
            }
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn handle_activate_app(
    state_ptr: *mut ClientState,
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        let focused = wparam.0 != 0;
        state.app_focused.store(focused, Ordering::Relaxed);
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn handle_focus(
    state_ptr: *mut ClientState,
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    focused: bool,
) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        if state.active_grid().lock().unwrap().focus_events() {
            let data = if focused { b"\x1b[I" } else { b"\x1b[O" };
            state.send_input(data.to_vec());
        }
    }
    DefWindowProcW(hwnd, msg, wparam, lparam)
}

unsafe fn handle_wm_close(state_ptr: *mut ClientState, hwnd: HWND) -> LRESULT {
    if !state_ptr.is_null() {
        let state = &*state_ptr;
        let store = state.panes.lock().unwrap();
        let path = crate::session::LayoutSnapshot::default_path();
        crate::session::save_session(&store, &path);
    }
    DestroyWindow(hwnd).ok();
    LRESULT(0)
}

unsafe fn handle_wm_destroy(hwnd: HWND) -> LRESULT {
    let _ = KillTimer(Some(hwnd), TIMER_REPAINT);
    remove_tray_icon(hwnd);
    // 永続バックバッファを解放
    // SAFETY: GWLP_USERDATA への生ポインタ逆参照は wndproc.rs 全体で使う既存パターンと同じ。
    // WM_CREATE で SetWindowLongPtrW が完了した後にのみ WM_DESTROY が来るため
    // ポインタは有効。null チェックで未初期化ケースも除外する。
    let state_ptr = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut ClientState;
    if !state_ptr.is_null() {
        (*state_ptr).release_backbuffer();
    }
    PostQuitMessage(0);
    LRESULT(0)
}
