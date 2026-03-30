use crate::cell::Cell;
use vte::Params;

use super::sgr::apply_sgr;
use super::VtProcessor;

pub(super) fn dispatch_csi(
    processor: &mut VtProcessor<'_>,
    params: &Params,
    intermediates: &[u8],
    action: char,
) {
    let p: Vec<u16> = params
        .iter()
        .map(|sub| sub.first().copied().unwrap_or(0))
        .collect();

    match action {
        'H' | 'f' => cursor_position(processor, &p),
        'A' => cursor_up(processor, &p),
        'B' => cursor_down(processor, &p),
        'C' => cursor_forward(processor, &p),
        'D' => cursor_backward(processor, &p),
        'J' => erase_display(processor, &p),
        'K' => erase_line(processor, &p),
        'r' if intermediates.is_empty() => set_scroll_region(processor, &p),
        'S' => scroll_up(processor, &p),
        'T' => scroll_down(processor, &p),
        'd' => vertical_position_absolute(processor, &p),
        'G' => horizontal_position_absolute(processor, &p),
        'E' => cursor_next_line(processor, &p),
        'F' => cursor_prev_line(processor, &p),
        '@' => insert_characters(processor, &p),
        'P' => delete_characters(processor, &p),
        'L' => insert_lines(processor, &p),
        'M' => delete_lines(processor, &p),
        'X' => erase_characters(processor, &p),
        's' if intermediates.is_empty() => processor.grid.save_cursor(),
        'u' if intermediates.is_empty() => processor.grid.restore_cursor(),
        'm' => apply_sgr(&mut processor.current_style, &p),
        'h' | 'l' if intermediates.first().copied() == Some(b'?') => {
            set_private_mode(processor, &p, action == 'h')
        }
        _ => {}
    }
}

fn cursor_position(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let row = params.first().copied().unwrap_or(1).saturating_sub(1);
    let col = params.get(1).copied().unwrap_or(1).saturating_sub(1);
    processor.grid.move_cursor(col, row);
}

fn cursor_up(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    let cur = processor.grid.cursor();
    processor
        .grid
        .move_cursor(cur.col, cur.row.saturating_sub(n));
}

fn cursor_down(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    let cur = processor.grid.cursor();
    processor.grid.move_cursor(cur.col, cur.row + n);
}

fn cursor_forward(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    let cur = processor.grid.cursor();
    processor.grid.move_cursor(cur.col + n, cur.row);
}

fn cursor_backward(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    let cur = processor.grid.cursor();
    processor
        .grid
        .move_cursor(cur.col.saturating_sub(n), cur.row);
}

fn erase_display(processor: &mut VtProcessor<'_>, params: &[u16]) {
    match params.first().copied().unwrap_or(0) {
        0 => processor.grid.erase_display_below(),
        1 => processor.grid.erase_display_above(),
        2 | 3 => processor.grid.erase_display_all(),
        _ => {}
    }
}

fn erase_line(processor: &mut VtProcessor<'_>, params: &[u16]) {
    match params.first().copied().unwrap_or(0) {
        0 => processor.grid.erase_line_right(),
        1 => processor.grid.erase_line_left(),
        2 => processor.grid.erase_line_all(),
        _ => {}
    }
}

fn set_scroll_region(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let top = params.first().copied().unwrap_or(1).max(1);
    let bottom = params.get(1).copied().unwrap_or(processor.grid.rows());
    processor.grid.set_scroll_region(top, bottom);
}

fn scroll_up(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    processor.grid.scroll_up(n);
}

fn scroll_down(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    processor.grid.scroll_down(n);
}

fn vertical_position_absolute(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let row = params.first().copied().unwrap_or(1).saturating_sub(1);
    let cur = processor.grid.cursor();
    processor.grid.move_cursor(cur.col, row);
}

fn horizontal_position_absolute(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let col = params.first().copied().unwrap_or(1).saturating_sub(1);
    let cur = processor.grid.cursor();
    processor.grid.move_cursor(col, cur.row);
}

fn cursor_next_line(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    let cur = processor.grid.cursor();
    processor.grid.move_cursor(0, cur.row + n);
}

fn cursor_prev_line(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    let cur = processor.grid.cursor();
    processor.grid.move_cursor(0, cur.row.saturating_sub(n));
}

fn insert_characters(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1) as usize;
    let row = processor.grid.cursor().row as usize;
    let col = processor.grid.cursor().col as usize;
    let cols = processor.grid.cols() as usize;
    if col < cols {
        let shift = n.min(cols - col);
        if let Some(r) = processor.grid.row_mut(row) {
            let rlen = r.len();
            r[col..].rotate_right(shift.min(rlen - col));
            for c in r[col..col + shift].iter_mut() {
                *c = Cell::blank();
            }
        }
        processor.grid.mark_dirty(row);
    }
}

fn delete_characters(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1) as usize;
    let row = processor.grid.cursor().row as usize;
    let col = processor.grid.cursor().col as usize;
    let cols = processor.grid.cols() as usize;
    if col < cols {
        let del = n.min(cols - col);
        if let Some(r) = processor.grid.row_mut(row) {
            let rlen = r.len();
            r[col..].rotate_left(del.min(rlen - col));
            for c in r[cols - del..].iter_mut() {
                *c = Cell::blank();
            }
        }
        processor.grid.mark_dirty(row);
    }
}

fn insert_lines(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    let saved_top = processor.grid.scroll_top();
    let saved_bottom = processor.grid.scroll_bottom();
    let cur_row = processor.grid.cursor().row;
    processor.grid.set_scroll_region_raw(cur_row, saved_bottom);
    processor.grid.scroll_down(n);
    processor
        .grid
        .set_scroll_region_raw(saved_top, saved_bottom);
}

fn delete_lines(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1);
    let saved_top = processor.grid.scroll_top();
    let saved_bottom = processor.grid.scroll_bottom();
    let cur_row = processor.grid.cursor().row;
    processor.grid.set_scroll_region_raw(cur_row, saved_bottom);
    processor.grid.scroll_up(n);
    processor
        .grid
        .set_scroll_region_raw(saved_top, saved_bottom);
}

fn erase_characters(processor: &mut VtProcessor<'_>, params: &[u16]) {
    let n = params.first().copied().unwrap_or(1).max(1) as usize;
    let row = processor.grid.cursor().row as usize;
    let col = processor.grid.cursor().col as usize;
    let cols = processor.grid.cols() as usize;
    let end = (col + n).min(cols);
    if let Some(r) = processor.grid.row_mut(row) {
        for c in r[col..end].iter_mut() {
            *c = Cell::blank();
        }
    }
    processor.grid.mark_dirty(row);
}

fn set_private_mode(processor: &mut VtProcessor<'_>, params: &[u16], enable: bool) {
    for &param in params {
        match param {
            1 => processor.grid.set_application_cursor_keys(enable),
            7 => processor.grid.set_auto_wrap(enable),
            25 => processor.grid.set_cursor_visible(enable),
            1049 => {
                if enable {
                    processor.grid.enter_alternate_screen();
                } else {
                    processor.grid.leave_alternate_screen();
                }
            }
            2004 => processor.grid.set_bracketed_paste(enable),
            1000 => processor
                .grid
                .set_mouse_reporting(if enable { 1 } else { 0 }),
            1002 => processor
                .grid
                .set_mouse_reporting(if enable { 2 } else { 0 }),
            1003 => processor
                .grid
                .set_mouse_reporting(if enable { 3 } else { 0 }),
            1006 | 1015 => processor.grid.set_mouse_sgr(enable),
            1004 => processor.grid.set_focus_events(enable),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::cell::CellContent;
    use crate::grid::Grid;
    use crate::vt::{feed_bytes, VtProcessor};
    use crate::width::CjkWidthConfig;
    use vte::Parser;

    fn make_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    fn feed(grid: &mut Grid, data: &[u8]) {
        let mut parser = Parser::new();
        let mut processor = VtProcessor::new(grid);
        feed_bytes(&mut parser, &mut processor, data);
    }

    #[test]
    fn test_vt_cursor_up() {
        let mut grid = make_grid(80, 24);
        grid.move_cursor(0, 5);
        feed(&mut grid, b"\x1b[3A");
        assert_eq!(grid.cursor().row, 2);
    }

    #[test]
    fn test_vt_cursor_down() {
        let mut grid = make_grid(80, 24);
        grid.move_cursor(0, 2);
        feed(&mut grid, b"\x1b[3B");
        assert_eq!(grid.cursor().row, 5);
    }

    #[test]
    fn test_vt_cursor_forward() {
        let mut grid = make_grid(80, 24);
        grid.move_cursor(0, 0);
        feed(&mut grid, b"\x1b[5C");
        assert_eq!(grid.cursor().col, 5);
    }

    #[test]
    fn test_vt_cursor_backward() {
        let mut grid = make_grid(80, 24);
        grid.move_cursor(10, 0);
        feed(&mut grid, b"\x1b[3D");
        assert_eq!(grid.cursor().col, 7);
    }

    #[test]
    fn test_vt_cursor_position() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[5;10H");
        assert_eq!(grid.cursor().row, 4);
        assert_eq!(grid.cursor().col, 9);
    }

    #[test]
    fn test_vt_cursor_position_default() {
        let mut grid = make_grid(80, 24);
        grid.move_cursor(10, 10);
        feed(&mut grid, b"\x1b[H");
        assert_eq!(grid.cursor().row, 0);
        assert_eq!(grid.cursor().col, 0);
    }

    #[test]
    fn test_vt_cursor_up_clamps_at_zero() {
        let mut grid = make_grid(80, 24);
        grid.move_cursor(0, 2);
        feed(&mut grid, b"\x1b[10A");
        assert_eq!(grid.cursor().row, 0);
    }

    #[test]
    fn test_vt_erase_line_right() {
        let mut grid = make_grid(10, 5);
        feed(&mut grid, b"ABCDE");
        grid.move_cursor(2, 0);
        feed(&mut grid, b"\x1b[K");
        assert!(matches!(
            grid.row(0).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
        assert!(matches!(
            grid.row(0).unwrap()[1].content,
            CellContent::Grapheme { .. }
        ));
        assert_eq!(grid.row(0).unwrap()[2].content, CellContent::Blank);
        assert_eq!(grid.row(0).unwrap()[3].content, CellContent::Blank);
    }

    #[test]
    fn test_vt_erase_display_full() {
        let mut grid = make_grid(10, 3);
        feed(&mut grid, b"Hello");
        feed(&mut grid, b"\x1b[2J");
        for col in 0..10usize {
            assert_eq!(grid.row(0).unwrap()[col].content, CellContent::Blank);
        }
    }

    #[test]
    fn test_vt_erase_display_below() {
        let mut grid = make_grid(10, 3);
        feed(&mut grid, b"AAAAA");
        grid.move_cursor(0, 1);
        feed(&mut grid, b"BBBBB");
        grid.move_cursor(0, 1);
        feed(&mut grid, b"\x1b[J");
        assert!(matches!(
            grid.row(0).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
        assert_eq!(grid.row(1).unwrap()[0].content, CellContent::Blank);
    }

    #[test]
    fn test_vt_decawm_off_no_wrap() {
        let mut grid = make_grid(5, 3);
        feed(&mut grid, b"\x1b[?7l");
        for _ in 0..10 {
            feed(&mut grid, b"X");
        }
        assert_eq!(grid.cursor().row, 0);
    }

    #[test]
    fn test_vt_decawm_on_restores_wrap() {
        let mut grid = make_grid(5, 3);
        feed(&mut grid, b"\x1b[?7l");
        feed(&mut grid, b"\x1b[?7h");
        for _ in 0..6 {
            feed(&mut grid, b"X");
        }
        assert_eq!(grid.cursor().row, 1);
    }

    #[test]
    fn test_vt_enter_alternate_screen() {
        let mut grid = make_grid(10, 3);
        feed(&mut grid, b"Hello");
        feed(&mut grid, b"\x1b[?1049h");
        assert!(grid.is_alternate_screen());
        assert_eq!(grid.row(0).unwrap()[0].content, CellContent::Blank);
        assert_eq!(grid.cursor().col, 0);
        assert_eq!(grid.cursor().row, 0);
    }

    #[test]
    fn test_vt_leave_alternate_screen_restores_main() {
        let mut grid = make_grid(10, 3);
        feed(&mut grid, b"Hello");
        feed(&mut grid, b"\x1b[?1049h");
        feed(&mut grid, b"Alt content");
        feed(&mut grid, b"\x1b[?1049l");
        assert!(!grid.is_alternate_screen());
        assert!(matches!(
            grid.row(0).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
    }

    #[test]
    fn test_vt_erase_line_left() {
        let mut grid = make_grid(10, 3);
        feed(&mut grid, b"ABCDE");
        grid.move_cursor(2, 0);
        feed(&mut grid, b"\x1b[1K");
        assert_eq!(grid.row(0).unwrap()[0].content, CellContent::Blank);
        assert_eq!(grid.row(0).unwrap()[1].content, CellContent::Blank);
        assert_eq!(grid.row(0).unwrap()[2].content, CellContent::Blank);
        assert!(matches!(
            grid.row(0).unwrap()[3].content,
            CellContent::Grapheme { .. }
        ));
    }

    #[test]
    fn test_vt_erase_line_all() {
        let mut grid = make_grid(10, 3);
        feed(&mut grid, b"ABCDE");
        grid.move_cursor(2, 0);
        feed(&mut grid, b"\x1b[2K");
        for col in 0..5usize {
            assert_eq!(grid.row(0).unwrap()[col].content, CellContent::Blank);
        }
    }

    #[test]
    fn test_vt_erase_display_above() {
        let mut grid = make_grid(10, 3);
        feed(&mut grid, b"Row0\r\nRow1\r\nRow2");
        grid.move_cursor(2, 1);
        feed(&mut grid, b"\x1b[1J");
        assert_eq!(grid.row(0).unwrap()[0].content, CellContent::Blank);
        assert_eq!(grid.row(1).unwrap()[0].content, CellContent::Blank);
        assert_eq!(grid.row(1).unwrap()[2].content, CellContent::Blank);
        assert!(matches!(
            grid.row(2).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
    }

    #[test]
    fn test_vt_erase_display_all_keeps_cursor() {
        let mut grid = make_grid(10, 3);
        feed(&mut grid, b"Hello");
        grid.move_cursor(5, 1);
        feed(&mut grid, b"\x1b[2J");
        assert_eq!(grid.cursor().col, 5);
        assert_eq!(grid.cursor().row, 1);
        assert_eq!(grid.row(0).unwrap()[0].content, CellContent::Blank);
    }

    #[test]
    fn test_vt_cursor_save_restore_csi() {
        let mut grid = make_grid(80, 24);
        grid.move_cursor(7, 3);
        feed(&mut grid, b"\x1b[s");
        grid.move_cursor(0, 0);
        feed(&mut grid, b"\x1b[u");
        assert_eq!(grid.cursor().col, 7);
        assert_eq!(grid.cursor().row, 3);
    }

    #[test]
    fn test_vt_cursor_visibility() {
        let mut grid = make_grid(80, 24);
        assert!(grid.cursor_visible());
        feed(&mut grid, b"\x1b[?25l");
        assert!(!grid.cursor_visible());
        feed(&mut grid, b"\x1b[?25h");
        assert!(grid.cursor_visible());
    }

    #[test]
    fn test_decckm_enable() {
        let mut grid = make_grid(80, 24);
        assert!(!grid.application_cursor_keys());
        feed(&mut grid, b"\x1b[?1h");
        assert!(grid.application_cursor_keys());
    }

    #[test]
    fn test_decckm_disable() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[?1h");
        feed(&mut grid, b"\x1b[?1l");
        assert!(!grid.application_cursor_keys());
    }

    #[test]
    fn test_bracketed_paste_enable() {
        let mut grid = make_grid(80, 24);
        assert!(!grid.bracketed_paste());
        feed(&mut grid, b"\x1b[?2004h");
        assert!(grid.bracketed_paste());
    }

    #[test]
    fn test_bracketed_paste_disable() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[?2004h");
        feed(&mut grid, b"\x1b[?2004l");
        assert!(!grid.bracketed_paste());
    }

    #[test]
    fn test_decstbm_sets_region() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[1;22r");
        assert_eq!(grid.scroll_top(), 0);
        assert_eq!(grid.scroll_bottom(), 21);
    }

    #[test]
    fn test_decstbm_reset() {
        let mut grid = make_grid(80, 24);
        feed(&mut grid, b"\x1b[1;22r");
        feed(&mut grid, b"\x1b[r");
        assert_eq!(grid.scroll_top(), 0);
        assert_eq!(grid.scroll_bottom(), 23);
    }

    #[test]
    fn test_decstbm_scroll_stays_in_region() {
        let mut grid = make_grid(10, 5);
        feed(&mut grid, b"\x1b[1;4r");
        feed(&mut grid, b"\x1b[5;1H");
        feed(&mut grid, b"STATUS");
        feed(&mut grid, b"\x1b[4;1H");
        for _ in 0..8 {
            feed(&mut grid, b"\n");
        }
        let row4 = grid.row(4).unwrap();
        assert!(matches!(row4[0].content, CellContent::Grapheme { ref text, .. } if text == "S"));
    }

    #[test]
    fn test_su_scrolls_region() {
        let mut grid = make_grid(10, 5);
        feed(&mut grid, b"\x1b[1;4r");
        feed(&mut grid, b"\x1b[1;1H");
        feed(&mut grid, b"FIRST");
        feed(&mut grid, b"\x1b[1S");
        let row0 = grid.row(0).unwrap();
        assert!(matches!(row0[0].content, CellContent::Blank));
    }
}
