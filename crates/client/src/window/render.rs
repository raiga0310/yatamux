use std::collections::HashMap;

use windows::Win32::Foundation::COLORREF;
use windows::Win32::Graphics::Gdi::*;
use yatamux_protocol::types::{PaneId, SplitDirection};
use yatamux_terminal::Cell;

use crate::ime::{ImeState, PreeditAttr};
use crate::layout::{LauncherState, PaneStore, ThemeLauncherState};
use crate::Theme;

// Catppuccin Mocha テーマ（デフォルト値）
const COLOR_BG: COLORREF = COLORREF(0x00_2E_1E_1E);
const COLOR_FG: COLORREF = COLORREF(0x00_F4_D6_CD);
const COLOR_CURSOR: COLORREF = COLORREF(0x00_E7_C2_F5);
pub(crate) const COLOR_SEPARATOR: COLORREF = COLORREF(0x00_5A_47_45);
const COLOR_PREEDIT_BG: COLORREF = COLORREF(0x00_5A_47_45);

/// Normal モードのマウス選択範囲に (col, row) が含まれるか判定する。
/// sel = (anchor_col, anchor_row, end_col, end_row)
pub(crate) fn is_in_normal_selection(
    sel: (usize, usize, usize, usize),
    col: usize,
    row: usize,
) -> bool {
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

fn rgb(r: u8, g: u8, b: u8) -> COLORREF {
    COLORREF((b as u32) << 16 | (g as u32) << 8 | r as u32)
}

fn rgb_hex(v: u32) -> COLORREF {
    let r = ((v >> 16) & 0xFF) as u8;
    let g = ((v >> 8) & 0xFF) as u8;
    let b = (v & 0xFF) as u8;
    rgb(r, g, b)
}

/// デフォルトのアラートボーダー色（#FF6B6B: 赤橙系）
const COLOR_ALERT_BORDER: COLORREF = COLORREF(0x00_6B_6B_FF);

/// Win32 用に解決済みのテーマ色。
#[derive(Copy, Clone)]
pub(crate) struct WinTheme {
    pub(crate) bg: COLORREF,
    pub(crate) fg: COLORREF,
    pub(crate) cursor: COLORREF,
    pub(crate) selection_bg: COLORREF,
    pub(crate) status_bar_bg: COLORREF,
    /// 通知アラート時のペインボーダー色
    pub(crate) alert_border: COLORREF,
}

impl WinTheme {
    pub(crate) fn from_theme(theme: &Theme) -> Self {
        Self {
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
            alert_border: theme
                .alert_border
                .map(rgb_hex)
                .unwrap_or(COLOR_ALERT_BORDER),
        }
    }
}

pub(crate) enum PreviewRenderNode {
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
    fn from_layout_preview(node: &crate::layout::LayoutNode, commands: &[Option<String>]) -> Self {
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

pub(crate) struct LauncherRenderState {
    pub(crate) entries: Vec<String>,
    pub(crate) selected: usize,
    pub(crate) selected_preview: Option<PreviewRenderNode>,
}

impl LauncherRenderState {
    pub(crate) fn from_launcher(launcher: &LauncherState) -> Self {
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

pub(crate) struct ThemeLauncherRenderState {
    pub(crate) entries: Vec<String>,
    pub(crate) selected: usize,
}

impl ThemeLauncherRenderState {
    pub(crate) fn from_theme_launcher(launcher: &ThemeLauncherState) -> Self {
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

pub(crate) struct SavePromptRenderState {
    pub(crate) prompt: String,
    pub(crate) cursor: usize,
    pub(crate) preview: PreviewRenderNode,
}

impl SavePromptRenderState {
    pub(crate) fn from_store(store: &PaneStore) -> Option<Self> {
        let prompt = store.save_prompt.as_ref()?;
        Some(Self {
            prompt: prompt.text.clone(),
            cursor: prompt.cursor,
            preview: PreviewRenderNode::from_live_layout(&store.layout, &store.pane_commands),
        })
    }
}

/// プリエディット下線を描画する
///
/// - `TargetConverted`: 太実線（現在の変換候補）
/// - `Converted`: 実線
/// - その他: 点線
pub(crate) unsafe fn draw_preedit_underline(
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

/// GDI 描画用テキストを返す。
pub(crate) fn zwj_render_text(text: &str) -> &str {
    if let Some(pos) = text.find('\u{200D}') {
        &text[..pos]
    } else {
        text
    }
}

pub(crate) fn cell_colors(cell: &Cell, _ime: &ImeState, theme: &WinTheme) -> (COLORREF, COLORREF) {
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

pub(crate) fn preedit_segment_colors(attr: &PreeditAttr, theme: &WinTheme) -> (COLORREF, COLORREF) {
    match attr {
        PreeditAttr::TargetConverted => (theme.bg, theme.fg),
        _ => (theme.fg, COLOR_PREEDIT_BG),
    }
}

#[cfg(test)]
mod tests {
    use super::is_in_normal_selection;

    #[test]
    fn test_normal_sel_single_row() {
        let sel = (2usize, 1usize, 5usize, 1usize);
        assert!(!is_in_normal_selection(sel, 1, 1));
        assert!(is_in_normal_selection(sel, 2, 1));
        assert!(is_in_normal_selection(sel, 3, 1));
        assert!(is_in_normal_selection(sel, 5, 1));
        assert!(!is_in_normal_selection(sel, 6, 1));
        assert!(!is_in_normal_selection(sel, 3, 0));
    }

    #[test]
    fn test_normal_sel_reversed() {
        let sel = (5usize, 1usize, 2usize, 1usize);
        assert!(!is_in_normal_selection(sel, 1, 1));
        assert!(is_in_normal_selection(sel, 2, 1));
        assert!(is_in_normal_selection(sel, 5, 1));
        assert!(!is_in_normal_selection(sel, 6, 1));
    }

    #[test]
    fn test_normal_sel_multi_row() {
        let sel = (2usize, 1usize, 4usize, 3usize);
        assert!(!is_in_normal_selection(sel, 0, 0));
        assert!(!is_in_normal_selection(sel, 1, 1));
        assert!(is_in_normal_selection(sel, 2, 1));
        assert!(is_in_normal_selection(sel, 99, 1));
        assert!(is_in_normal_selection(sel, 0, 2));
        assert!(is_in_normal_selection(sel, 99, 2));
        assert!(is_in_normal_selection(sel, 0, 3));
        assert!(is_in_normal_selection(sel, 4, 3));
        assert!(!is_in_normal_selection(sel, 5, 3));
        assert!(!is_in_normal_selection(sel, 0, 4));
    }

    #[test]
    fn test_normal_sel_multi_row_reversed() {
        let sel = (4usize, 3usize, 2usize, 1usize);
        assert!(!is_in_normal_selection(sel, 1, 1));
        assert!(is_in_normal_selection(sel, 2, 1));
        assert!(is_in_normal_selection(sel, 0, 2));
        assert!(is_in_normal_selection(sel, 4, 3));
        assert!(!is_in_normal_selection(sel, 5, 3));
    }
}
