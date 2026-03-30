use yatamux_protocol::types::{CursorInfo, PaneCapture, PaneId, PaneInfo, SurfaceId};

use crate::pane::Pane;

pub(super) async fn build_capture_response(pane: &Pane, lines: usize) -> (String, PaneCapture) {
    let grid = pane.grid.lock().await;
    let visible_text = visible_text(&grid);
    let scrollback_tail = scrollback_tail(&grid, lines);
    let content = captured_content(&grid, lines);
    let title = pane.title.lock().unwrap().to_string();
    let capture = PaneCapture {
        title,
        cols: grid.cols(),
        rows: grid.rows(),
        lines_requested: lines,
        scrollback_len: grid.scrollback_len(),
        cursor: CursorInfo {
            col: grid.cursor().col,
            row: grid.cursor().row,
            visible: grid.cursor_visible(),
        },
        visible_text: if lines == 0 { Vec::new() } else { visible_text },
        scrollback_tail,
    };
    (content, capture)
}

pub(super) fn pane_info(id: PaneId, surface: SurfaceId, pane: &Pane) -> PaneInfo {
    let size = pane.size.lock().unwrap();
    let (cols, rows) = (size.cols, size.rows);
    drop(size);
    let title = pane.title.lock().unwrap().to_string();

    PaneInfo {
        id,
        surface,
        title,
        cols,
        rows,
    }
}

fn visible_text(grid: &yatamux_terminal::Grid) -> Vec<String> {
    (0..grid.rows())
        .filter_map(|row| grid.row(row))
        .map(yatamux_terminal::grid::row_cells_to_text)
        .collect()
}

fn scrollback_tail(grid: &yatamux_terminal::Grid, lines: usize) -> Vec<String> {
    if lines == 0 {
        return Vec::new();
    }

    let tail_rows = lines.saturating_sub(grid.rows() as usize);
    let start = grid.scrollback_len().saturating_sub(tail_rows);
    (start..grid.scrollback_len())
        .filter_map(|idx| grid.scrollback_row(idx))
        .map(|row| yatamux_terminal::grid::row_cells_to_text(row))
        .collect()
}

fn captured_content(grid: &yatamux_terminal::Grid, lines: usize) -> String {
    if lines == 0 {
        return String::new();
    }

    let scrollback_len = grid.scrollback_len();
    let rows = grid.rows() as usize;
    let total_rows = scrollback_len + rows;
    let skip = total_rows.saturating_sub(lines);

    let mut parts: Vec<String> = Vec::new();
    for idx in skip..scrollback_len {
        if let Some(row) = grid.scrollback_row(idx) {
            parts.push(yatamux_terminal::grid::row_cells_to_text(row));
        }
    }

    let screen_skip = skip.saturating_sub(scrollback_len);
    for row in screen_skip..rows {
        if let Some(cells) = grid.row(row as u16) {
            parts.push(yatamux_terminal::grid::row_cells_to_text(cells));
        }
    }

    parts.join("\n")
}
