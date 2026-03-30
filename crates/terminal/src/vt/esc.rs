use super::VtProcessor;

pub(super) fn dispatch_esc(processor: &mut VtProcessor<'_>, intermediates: &[u8], byte: u8) {
    if !intermediates.is_empty() {
        return;
    }

    match byte {
        b'7' => processor.grid.save_cursor(),
        b'8' => processor.grid.restore_cursor(),
        _ => {}
    }
}

#[cfg(test)]
mod tests {
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
    fn test_vt_cursor_save_restore_esc() {
        let mut grid = make_grid(80, 24);
        grid.move_cursor(10, 5);
        feed(&mut grid, b"\x1b7");
        grid.move_cursor(0, 0);
        feed(&mut grid, b"\x1b8");
        assert_eq!(grid.cursor().col, 10);
        assert_eq!(grid.cursor().row, 5);
    }
}
