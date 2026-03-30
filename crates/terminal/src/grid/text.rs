use crate::cell::{Cell, CellContent};

use super::Grid;

/// 1行のセル列をプレーンテキストに変換する（末尾の空白を除去）
///
/// サーバーサイドの `CapturePane` などから直接利用できるよう公開している。
pub fn row_cells_to_text(row: &[Cell]) -> String {
    let line: String = row
        .iter()
        .filter_map(|cell| match &cell.content {
            CellContent::Grapheme { text, .. } => Some(text.as_str()),
            _ => None,
        })
        .collect();
    line.trim_end().to_string()
}

pub(super) fn row_to_text(row: &[Cell]) -> String {
    row_cells_to_text(row)
}

pub(super) fn full_content_text(grid: &Grid) -> String {
    let mut parts: Vec<String> = Vec::new();
    if !grid.scrollback.is_empty() {
        parts.push(grid.scrollback.as_text());
    }
    for row in 0..grid.rows {
        if let Some(cells) = grid.row(row) {
            parts.push(row_to_text(cells));
        }
    }
    parts.join("\n")
}

pub(super) fn extract_text(grid: &Grid, row_start: usize, row_end: usize) -> String {
    if row_start > row_end {
        return String::new();
    }
    let rows = grid.rows as usize;
    let start = row_start.min(rows.saturating_sub(1));
    let end = row_end.min(rows.saturating_sub(1));
    (start..=end)
        .filter_map(|row| grid.row(row as u16))
        .map(row_to_text)
        .collect::<Vec<_>>()
        .join("\n")
}

/// グリッド内容の NFC 正規化（macOS NFD パス等の韓国語対応）
///
/// 要件定義書 §4「Unicode 正規化」参照。
/// NFD 形式の韓国語（Hangul Jamo 分解形）を NFC（音節単位）に変換する。
pub fn normalize_nfc(s: &str) -> String {
    use unicode_normalization::UnicodeNormalization;
    s.nfc().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellStyle;
    use crate::vt::{feed_bytes, VtProcessor};
    use crate::width::CjkWidthConfig;
    use vte::Parser;

    fn default_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    fn feed_str_grid(grid: &mut Grid, s: &str) {
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(grid);
        feed_bytes(&mut parser, &mut proc, s.as_bytes());
    }

    #[test]
    fn test_normalize_nfc_converts_korean() {
        let nfd = "\u{110B}\u{1161}";
        let nfc = normalize_nfc(nfd);
        assert_eq!(nfc, "아");
        assert_ne!(nfc, nfd);
    }

    #[test]
    fn test_full_content_text_includes_scrollback_and_screen() {
        let mut g = default_grid(10, 2);
        g.write_char("A", CellStyle::default());
        g.scroll_up(1);
        g.write_char("B", CellStyle::default());
        let text = g.full_content_text();
        assert!(text.contains('A'));
        assert!(text.contains('B'));
    }

    #[test]
    fn test_extract_text_ascii() {
        let mut g = default_grid(80, 24);
        feed_str_grid(&mut g, "hello");
        let text = g.extract_text(0, 0);
        assert_eq!(text, "hello");
    }

    #[test]
    fn test_extract_text_multi_row() {
        let mut g = default_grid(80, 24);
        feed_str_grid(&mut g, "hello\r\nworld");
        let text = g.extract_text(0, 1);
        assert_eq!(text, "hello\nworld");
    }

    #[test]
    fn test_extract_text_skips_continuation() {
        let mut g = default_grid(80, 24);
        g.write_char("日", CellStyle::default());
        g.write_char("本", CellStyle::default());
        let text = g.extract_text(0, 0);
        assert_eq!(text, "日本");
        assert!(!text.contains('\u{FFFF}'));
    }

    #[test]
    fn test_extract_text_inverted_range() {
        let mut g = default_grid(80, 24);
        feed_str_grid(&mut g, "hello");
        let text = g.extract_text(5, 0);
        assert_eq!(text, "");
    }
}
