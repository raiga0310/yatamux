use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Gdi::InvalidateRect;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use yatamux_protocol::types::SplitDirection;

use super::super::input::keydown_to_vt;
use super::super::render::WinTheme;
use super::{read_clipboard_text, ClientMode, ClientState};
use crate::layout::{
    layout_to_toml, list_available_layouts, list_available_themes, load_theme_from_file,
    save_layout_file, CopyState, Direction, LauncherState, PromptState, ThemeLauncherState,
};

pub(super) struct KeyInput {
    pub(super) vk: u16,
    pub(super) ctrl: bool,
    pub(super) shift: bool,
    pub(super) wparam: WPARAM,
    pub(super) lparam: LPARAM,
}

#[derive(PartialEq)]
pub(super) enum KeyConsumed {
    Yes,
    YesPassChar,
    No,
}

impl ClientState {
    pub(super) unsafe fn handle_save_prompt(
        state: &Self,
        hwnd: HWND,
        key: &KeyInput,
    ) -> KeyConsumed {
        if state.panes.lock().unwrap().save_prompt.is_none() {
            return KeyConsumed::No;
        }
        let vk = key.vk;
        let result = if vk == VK_RETURN.0 {
            let (name, layout_toml) = {
                let mut store = state.panes.lock().unwrap();
                let name = store.save_prompt.take().unwrap_or_default().text;
                let toml = layout_to_toml(&store.layout, &store.pane_commands);
                (name, toml)
            };
            let name = name.trim().to_string();
            if !name.is_empty() {
                state
                    .panes
                    .lock()
                    .unwrap()
                    .push_save_prompt_history(name.clone());
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
        } else if key.ctrl {
            let mut store = state.panes.lock().unwrap();
            let yank = if vk == b'Y' as u16 {
                Some(store.save_prompt_yank.clone())
            } else {
                None
            };
            let mut new_yank = None;
            let result = {
                let prompt = match &mut store.save_prompt {
                    Some(prompt) => prompt,
                    None => return KeyConsumed::No,
                };
                match vk {
                    k if k == b'A' as u16 => {
                        prompt.move_start();
                        KeyConsumed::Yes
                    }
                    k if k == b'E' as u16 => {
                        prompt.move_end();
                        KeyConsumed::Yes
                    }
                    k if k == b'B' as u16 => {
                        prompt.move_left();
                        KeyConsumed::Yes
                    }
                    k if k == b'F' as u16 => {
                        prompt.move_right();
                        KeyConsumed::Yes
                    }
                    k if k == b'K' as u16 => {
                        new_yank = prompt.kill_to_end();
                        KeyConsumed::Yes
                    }
                    k if k == b'U' as u16 => {
                        new_yank = prompt.kill_to_start();
                        KeyConsumed::Yes
                    }
                    k if k == b'W' as u16 => {
                        new_yank = prompt.kill_prev_word();
                        KeyConsumed::Yes
                    }
                    k if k == b'Y' as u16 => {
                        if let Some(yank) = &yank {
                            prompt.insert_str(yank);
                        }
                        KeyConsumed::Yes
                    }
                    _ => KeyConsumed::YesPassChar,
                }
            };
            if let Some(new_yank) = new_yank {
                store.save_prompt_yank = new_yank;
            }
            result
        } else if vk == VK_BACK.0 {
            let mut store = state.panes.lock().unwrap();
            if let Some(prompt) = &mut store.save_prompt {
                prompt.backspace();
            }
            KeyConsumed::Yes
        } else if vk == VK_LEFT.0 {
            if let Some(prompt) = &mut state.panes.lock().unwrap().save_prompt {
                prompt.move_left();
            }
            KeyConsumed::Yes
        } else if vk == VK_RIGHT.0 {
            if let Some(prompt) = &mut state.panes.lock().unwrap().save_prompt {
                prompt.move_right();
            }
            KeyConsumed::Yes
        } else if vk == VK_UP.0 {
            let mut store = state.panes.lock().unwrap();
            let history = store.save_prompt_history.clone();
            if let Some(prompt) = &mut store.save_prompt {
                let _ = prompt.history_prev(&history);
            }
            KeyConsumed::Yes
        } else if vk == VK_DOWN.0 {
            let mut store = state.panes.lock().unwrap();
            let history = store.save_prompt_history.clone();
            if let Some(prompt) = &mut store.save_prompt {
                let _ = prompt.history_next(&history);
            }
            KeyConsumed::Yes
        } else {
            KeyConsumed::YesPassChar
        };
        let _ = InvalidateRect(Some(hwnd), None, false);
        result
    }

    pub(super) unsafe fn handle_layout_launcher(
        state: &Self,
        hwnd: HWND,
        key: &KeyInput,
    ) -> KeyConsumed {
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

    pub(super) unsafe fn handle_theme_launcher(
        state: &Self,
        hwnd: HWND,
        key: &KeyInput,
    ) -> KeyConsumed {
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

    pub(super) unsafe fn handle_copy_mode(state: &Self, hwnd: HWND, key: &KeyInput) -> KeyConsumed {
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

    pub(super) unsafe fn handle_pane_mode(state: &Self, hwnd: HWND, key: &KeyInput) -> KeyConsumed {
        if state.mode.get() != ClientMode::Pane {
            return KeyConsumed::No;
        }
        let vk = key.vk;
        let shift = key.shift;

        const MODIFIER_KEYS: &[u16] = &[0x10, 0x11, 0x12, 0xA0, 0xA1, 0xA2, 0xA3, 0xA4, 0xA5];
        if MODIFIER_KEYS.contains(&vk) {
            return KeyConsumed::Yes;
        }

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

        match vk {
            k if k == b'V' as u16 => {
                state.mode.set(ClientMode::Copy);
                state.panes.lock().unwrap().copy_mode = Some(CopyState::new(0, 0));
            }
            k if k == b'S' as u16 => {
                state.panes.lock().unwrap().save_prompt = Some(PromptState::new());
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
                        state.panes.lock().unwrap().launcher = Some(LauncherState::new(entries));
                    }
                    _ => {}
                }
            }
        }
        let _ = InvalidateRect(Some(hwnd), None, false);
        KeyConsumed::Yes
    }

    pub(super) unsafe fn handle_global_shortcuts(
        state: &Self,
        hwnd: HWND,
        key: &KeyInput,
    ) -> KeyConsumed {
        let ctrl = key.ctrl;
        let shift = key.shift;
        let vk = key.vk;

        if ctrl && !shift && vk == b'F' as u16 {
            let _ = state.float_tx.try_send(());
            let _ = InvalidateRect(Some(hwnd), None, false);
            return KeyConsumed::Yes;
        }
        if ctrl && !shift && vk == b'B' as u16 {
            state.mode.set(ClientMode::Pane);
            let _ = InvalidateRect(Some(hwnd), None, false);
            return KeyConsumed::Yes;
        }
        if ctrl && !shift && vk == b'P' as u16 {
            let entries = list_available_themes();
            state.panes.lock().unwrap().theme_launcher = Some(ThemeLauncherState::new(entries));
            let _ = InvalidateRect(Some(hwnd), None, false);
            return KeyConsumed::Yes;
        }
        if ctrl && shift && vk == b'E' as u16 {
            state.request_split(SplitDirection::Vertical);
            return KeyConsumed::Yes;
        }
        if ctrl && shift && vk == b'O' as u16 {
            state.request_split(SplitDirection::Horizontal);
            return KeyConsumed::Yes;
        }
        if ctrl && shift && vk == b'W' as u16 {
            state.close_active_pane();
            return KeyConsumed::Yes;
        }
        if ctrl && vk == VK_TAB.0 {
            state.cycle_pane(!shift);
            let _ = InvalidateRect(Some(hwnd), None, false);
            return KeyConsumed::Yes;
        }
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
        }

        KeyConsumed::No
    }

    pub(super) unsafe fn handle_vt_passthrough(
        state: &Self,
        _hwnd: HWND,
        key: &KeyInput,
    ) -> KeyConsumed {
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

    pub(super) unsafe fn dispatch_wm_keydown(
        state: &Self,
        hwnd: HWND,
        key: &KeyInput,
    ) -> KeyConsumed {
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
