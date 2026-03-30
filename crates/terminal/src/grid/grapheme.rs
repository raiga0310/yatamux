use crate::cell::{Cell, CellContent};

use super::Grid;

pub(super) fn combine_with_last_cell(grid: &mut Grid, c: char) {
    let row = grid.cursor.row as usize;
    let col = grid.cursor.col;

    for c_idx in (0..col).rev() {
        match &grid.cells[row][c_idx as usize].content {
            CellContent::Grapheme { .. } => {
                if let CellContent::Grapheme { text, .. } =
                    &mut grid.cells[row][c_idx as usize].content
                {
                    text.push(c);
                    grid.dirty[row] = true;
                }
                return;
            }
            CellContent::Continuation => continue,
            CellContent::Blank => return,
        }
    }
}

pub(super) fn apply_vs16(grid: &mut Grid) {
    let row = grid.cursor.row as usize;
    let col = grid.cursor.col;

    let mut target_col: Option<usize> = None;
    for c_idx in (0..col).rev() {
        match &grid.cells[row][c_idx as usize].content {
            CellContent::Grapheme { .. } => {
                target_col = Some(c_idx as usize);
                break;
            }
            CellContent::Continuation => continue,
            CellContent::Blank => break,
        }
    }

    if let Some(col_idx) = target_col {
        let style = grid.cells[row][col_idx].style;
        let needs_widen = matches!(
            &grid.cells[row][col_idx].content,
            CellContent::Grapheme { width: 1, .. }
        );
        if let CellContent::Grapheme { text, width } = &mut grid.cells[row][col_idx].content {
            text.push('\u{FE0F}');
            if needs_widen {
                *width = 2;
            }
        }
        if needs_widen {
            let next_col = col_idx + 1;
            if next_col < grid.cols as usize {
                grid.cells[row][next_col] = Cell::continuation(style);
            }
            let new_cursor = col + 1;
            if new_cursor >= grid.cols {
                grid.cursor.col = grid.cols - 1;
                grid.flags.last_col_flag = true;
            } else {
                grid.cursor.col = new_cursor;
            }
        }
        grid.dirty[row] = true;
    }
}

pub(super) fn last_grapheme_ends_with_zwj(grid: &Grid) -> bool {
    let row = grid.cursor.row as usize;
    let col = grid.cursor.col;

    for c_idx in (0..col).rev() {
        match &grid.cells[row][c_idx as usize].content {
            CellContent::Grapheme { text, .. } => {
                return text.ends_with('\u{200D}');
            }
            CellContent::Continuation => continue,
            CellContent::Blank => return false,
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellStyle;
    use crate::vt::VtProcessor;
    use crate::width::CjkWidthConfig;
    use vte::Perform;

    fn default_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    #[test]
    fn test_combine_with_last_cell_zwj() {
        let mut g = default_grid(10, 3);
        {
            let mut vt = VtProcessor::new(&mut g);
            vt.print('👨');
            vt.print('\u{200D}');
            vt.print('💻');
        }
        match &g.row(0).unwrap()[0].content {
            CellContent::Grapheme { text, width } => {
                assert_eq!(text, "👨\u{200D}💻");
                assert_eq!(*width, 2);
            }
            _ => panic!("col=0 should be a Grapheme"),
        }
        assert_eq!(g.row(0).unwrap()[1].content, CellContent::Continuation);
        assert_eq!(g.row(0).unwrap()[2].content, CellContent::Blank);
    }

    #[test]
    fn test_apply_vs16_widens_cell() {
        let mut g = default_grid(10, 3);
        g.write_char("♀", CellStyle::default());
        g.apply_vs16();
        match &g.row(0).unwrap()[0].content {
            CellContent::Grapheme { text, width } => {
                assert!(text.contains('\u{FE0F}'));
                assert_eq!(*width, 2);
            }
            _ => panic!("col=0 should be Grapheme"),
        }
        assert_eq!(g.row(0).unwrap()[1].content, CellContent::Continuation);
    }

    #[test]
    fn test_combine_vs15_no_width_change() {
        let mut g = default_grid(10, 3);
        g.write_char("♀", CellStyle::default());
        g.combine_with_last_cell('\u{FE0E}');
        match &g.row(0).unwrap()[0].content {
            CellContent::Grapheme { text, width } => {
                assert!(text.ends_with('\u{FE0E}'));
                assert_eq!(*width, 1);
            }
            _ => panic!("col=0 should be Grapheme"),
        }
    }

    #[test]
    fn test_write_char_zwj_string() {
        let mut g = default_grid(10, 3);
        g.write_char("👨\u{200D}💻", CellStyle::default());
        match &g.row(0).unwrap()[0].content {
            CellContent::Grapheme { text, width } => {
                assert_eq!(text, "👨\u{200D}💻");
                assert_eq!(*width, 2);
            }
            _ => panic!("col=0 should be Grapheme"),
        }
        assert_eq!(g.row(0).unwrap()[1].content, CellContent::Continuation);
    }
}
