use std::collections::VecDeque;

use crate::cell::Cell;

use super::text::row_to_text;

/// スクロールバックバッファ
///
/// 画面外に押し出された行を FIFO で保持する専用型。
/// `VecDeque` をラップし最大行数管理とテキストダンプを提供する。
pub struct ScrollbackBuffer {
    rows: VecDeque<Vec<Cell>>,
    max_rows: usize,
}

impl ScrollbackBuffer {
    pub fn new(max_rows: usize) -> Self {
        Self {
            rows: VecDeque::new(),
            max_rows,
        }
    }

    /// 行を末尾に追加する。上限を超えた場合は最古行を破棄する。
    pub fn push(&mut self, row: Vec<Cell>) {
        self.rows.push_back(row);
        if self.rows.len() > self.max_rows {
            self.rows.pop_front();
        }
    }

    pub fn len(&self) -> usize {
        self.rows.len()
    }

    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
    }

    pub fn get(&self, idx: usize) -> Option<&Vec<Cell>> {
        self.rows.get(idx)
    }

    /// スクロールバック全行をプレーンテキストとして返す
    ///
    /// 各行の末尾空白を除去し、改行（`\n`）で連結する。
    pub fn as_text(&self) -> String {
        self.rows
            .iter()
            .map(|row| row_to_text(row))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::super::Grid;
    use super::*;
    use crate::cell::{CellContent, CellStyle};
    use crate::width::CjkWidthConfig;

    fn default_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    #[test]
    fn test_scrollback_full_screen_scroll() {
        let mut g = default_grid(10, 3);
        g.write_char("A", CellStyle::default());
        g.scroll_up(1);
        assert_eq!(g.scrollback_len(), 1);
        let row = g.scrollback_row(0).unwrap();
        match &row[0].content {
            CellContent::Grapheme { text, .. } => assert_eq!(text, "A"),
            _ => panic!("expected 'A' in scrollback row 0"),
        }
    }

    #[test]
    fn test_scrollback_subregion_no_save() {
        let mut g = default_grid(10, 3);
        g.set_scroll_region(2, 3);
        g.move_cursor(0, 1);
        g.write_char("A", CellStyle::default());
        g.scroll_up(1);
        assert_eq!(g.scrollback_len(), 0);
    }

    #[test]
    fn test_scrollback_max_lines() {
        let mut g = default_grid(10, 3);
        for _ in 0..=Grid::SCROLLBACK_MAX {
            g.scroll_up(1);
        }
        assert_eq!(g.scrollback_len(), Grid::SCROLLBACK_MAX);
    }

    #[test]
    fn test_scrollback_multi_line_order() {
        let mut g = default_grid(10, 3);
        g.write_char("A", CellStyle::default());
        g.move_cursor(0, 1);
        g.write_char("B", CellStyle::default());
        g.scroll_up(2);
        assert_eq!(g.scrollback_len(), 2);
        match &g.scrollback_row(0).unwrap()[0].content {
            CellContent::Grapheme { text, .. } => assert_eq!(text, "A"),
            _ => panic!("expected 'A' at scrollback[0]"),
        }
        match &g.scrollback_row(1).unwrap()[0].content {
            CellContent::Grapheme { text, .. } => assert_eq!(text, "B"),
            _ => panic!("expected 'B' at scrollback[1]"),
        }
    }

    #[test]
    fn test_scrollback_no_save_in_alternate_screen() {
        let mut g = default_grid(10, 3);
        g.write_char("A", CellStyle::default());
        g.enter_alternate_screen();
        g.scroll_up(1);
        assert_eq!(g.scrollback_len(), 0);
    }

    #[test]
    fn test_scrollback_max_is_50000() {
        assert_eq!(Grid::SCROLLBACK_MAX, 50_000);
    }

    #[test]
    fn test_scrollback_buffer_as_text() {
        let mut buf = ScrollbackBuffer::new(10);
        let style = CellStyle::default();

        let mut row_foo: Vec<Cell> = vec![Cell::blank(); 5];
        row_foo[0] = Cell::from_grapheme("f".into(), 1, style);
        row_foo[1] = Cell::from_grapheme("o".into(), 1, style);
        row_foo[2] = Cell::from_grapheme("o".into(), 1, style);
        buf.push(row_foo);

        let mut row_bar: Vec<Cell> = vec![Cell::blank(); 3];
        row_bar[0] = Cell::from_grapheme("b".into(), 1, style);
        row_bar[1] = Cell::from_grapheme("a".into(), 1, style);
        row_bar[2] = Cell::from_grapheme("r".into(), 1, style);
        buf.push(row_bar);

        let text = buf.as_text();
        assert_eq!(text, "foo\nbar");
    }
}
