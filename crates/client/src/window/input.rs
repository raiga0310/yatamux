use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

/// マウスイベントを VT シーケンスに変換する
///
/// - `btn`: ボタン番号（0=左, 2=右, 32+n=モーション）
/// - `col`, `row`: 1 始まりのセル座標
/// - `release`: ボタン離上イベントか
/// - `sgr`: `?1006h` SGR 拡張モードか
pub(crate) fn mouse_to_vt(
    btn: u8,
    col: u16,
    row: u16,
    release: bool,
    sgr: bool,
) -> Option<Vec<u8>> {
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

/// `WM_KEYDOWN` の仮想キーコードを VT シーケンスに変換する
pub(crate) fn keydown_to_vt(wparam: WPARAM, lparam: LPARAM, app_cursor: bool) -> Option<Vec<u8>> {
    let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
    keydown_to_vt_with_mods(wparam, lparam, ctrl, shift, app_cursor)
}

/// 修飾キー状態を引数で受け取るテスト可能な内部実装
pub(crate) fn keydown_to_vt_with_mods(
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
        VK_PRIOR => b"\x1b[5~",
        VK_NEXT => b"\x1b[6~",
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
            if ctrl && vk >= b'A' as u16 && vk <= b'Z' as u16 {
                return Some(vec![vk as u8 - b'A' + 1]);
            }
            return None;
        }
    };

    Some(seq.to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wvk(vk: VIRTUAL_KEY) -> WPARAM {
        WPARAM(vk.0 as usize)
    }

    fn lp() -> LPARAM {
        LPARAM(0)
    }

    #[test]
    fn test_enter_delegated_to_wm_char() {
        assert_eq!(keydown_to_vt(wvk(VK_RETURN), lp(), false), None);
    }

    #[test]
    fn test_backspace_maps_to_del() {
        assert_eq!(
            keydown_to_vt(wvk(VK_BACK), lp(), false),
            Some(b"\x7f".to_vec())
        );
    }

    #[test]
    fn test_escape_delegated_to_wm_char() {
        assert_eq!(keydown_to_vt(wvk(VK_ESCAPE), lp(), false), None);
    }

    #[test]
    fn test_tab_maps_to_ht() {
        assert_eq!(
            keydown_to_vt(wvk(VK_TAB), lp(), false),
            Some(b"\t".to_vec())
        );
    }

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

    #[test]
    fn test_shift_tab() {
        assert_eq!(
            keydown_to_vt_with_mods(wvk(VK_TAB), lp(), false, true, false),
            Some(b"\x1b[Z".to_vec())
        );
    }

    #[test]
    fn test_unhandled_key_returns_none() {
        assert_eq!(
            keydown_to_vt(WPARAM(VK_SPACE.0 as usize), lp(), false),
            None
        );
    }

    #[test]
    fn test_ctrl_a() {
        assert_eq!(
            keydown_to_vt_with_mods(WPARAM(b'A' as usize), lp(), true, false, false),
            Some(vec![0x01])
        );
    }

    #[test]
    fn test_ctrl_c() {
        assert_eq!(
            keydown_to_vt_with_mods(WPARAM(b'C' as usize), lp(), true, false, false),
            Some(vec![0x03])
        );
    }

    #[test]
    fn test_ctrl_d() {
        assert_eq!(
            keydown_to_vt_with_mods(WPARAM(b'D' as usize), lp(), true, false, false),
            Some(vec![0x04])
        );
    }

    #[test]
    fn test_ctrl_l() {
        assert_eq!(
            keydown_to_vt_with_mods(WPARAM(b'L' as usize), lp(), true, false, false),
            Some(vec![0x0c])
        );
    }

    #[test]
    fn test_ctrl_z() {
        assert_eq!(
            keydown_to_vt_with_mods(WPARAM(b'Z' as usize), lp(), true, false, false),
            Some(vec![0x1a])
        );
    }

    #[test]
    fn test_ctrl_all_letters() {
        for (i, letter) in (b'A'..=b'Z').enumerate() {
            let expected = vec![(i + 1) as u8];
            assert_eq!(
                keydown_to_vt_with_mods(WPARAM(letter as usize), lp(), true, false, false),
                Some(expected),
                "Ctrl+{} should produce \\x{:02x}",
                letter as char,
                i + 1
            );
        }
    }

    #[test]
    fn test_letter_without_ctrl_returns_none() {
        assert_eq!(
            keydown_to_vt_with_mods(WPARAM(b'A' as usize), lp(), false, false, false),
            None
        );
    }
}
