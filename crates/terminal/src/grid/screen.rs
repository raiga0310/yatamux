use super::{Cell, CursorPos, Grid, GridFlags, MainScreenSnapshot};

impl Grid {
    /// オルタネートスクリーンに切り替える (`CSI ?1049h`)
    ///
    /// メインスクリーンの内容・カーソル・フラグを退避し、
    /// 空のオルタネートバッファを表示する。
    pub fn enter_alternate_screen(&mut self) {
        if self.saved_main.is_some() {
            return;
        }
        self.saved_main = Some(MainScreenSnapshot {
            cells: self.cells.clone(),
            cursor: self.cursor,
            flags: self.flags,
        });
        for row in &mut self.cells {
            row.fill(Cell::blank());
        }
        self.cursor = CursorPos::default();
        self.flags = GridFlags::default();
        self.dirty.fill(true);
    }

    /// メインスクリーンに戻る (`CSI ?1049l`)
    ///
    /// 退避しておいたメインスクリーンを復元する。
    pub fn leave_alternate_screen(&mut self) {
        if let Some(snap) = self.saved_main.take() {
            self.cells = snap.cells;
            self.cursor = snap.cursor;
            self.flags = snap.flags;
            self.dirty.fill(true);
        }
    }

    /// オルタネートスクリーンが有効か
    pub fn is_alternate_screen(&self) -> bool {
        self.saved_main.is_some()
    }

    /// 行頭からカーソル位置までを消去する (`EL 1`)
    pub fn erase_line_left(&mut self) {
        let row = self.cursor.row as usize;
        let col = self.cursor.col as usize;
        for cell in self.cells[row][..=col].iter_mut() {
            *cell = Cell::blank();
        }
        self.dirty[row] = true;
    }

    /// 現在行全体を消去する (`EL 2`)
    pub fn erase_line_all(&mut self) {
        let row = self.cursor.row as usize;
        self.cells[row].fill(Cell::blank());
        self.dirty[row] = true;
    }

    /// 行のクリア（EOL）
    pub fn erase_line_right(&mut self) {
        let row = self.cursor.row as usize;
        let col = self.cursor.col as usize;
        for cell in self.cells[row][col..].iter_mut() {
            *cell = Cell::blank();
        }
        self.dirty[row] = true;
    }

    /// 画面先頭からカーソル位置までを消去する (`ED 1`)
    pub fn erase_display_above(&mut self) {
        let cur_row = self.cursor.row as usize;
        self.erase_line_left();
        for row in 0..cur_row {
            self.cells[row].fill(Cell::blank());
            self.dirty[row] = true;
        }
    }

    /// 画面全体を消去する (`ED 2`) — カーソル位置は変えない
    pub fn erase_display_all(&mut self) {
        for row in &mut self.cells {
            row.fill(Cell::blank());
        }
        self.dirty.fill(true);
    }

    /// 画面のクリア（ED 0: カーソル以降）
    pub fn erase_display_below(&mut self) {
        let cur_row = self.cursor.row as usize;
        self.erase_line_right();
        for row in (cur_row + 1)..self.rows as usize {
            self.cells[row].fill(Cell::blank());
            self.dirty[row] = true;
        }
    }

    /// グリッドをリサイズ（ConPTY リサイズ時に呼び出し）
    pub fn resize(&mut self, new_cols: u16, new_rows: u16) {
        self.cells
            .resize_with(new_rows as usize, || vec![Cell::blank(); new_cols as usize]);
        for row in &mut self.cells {
            row.resize_with(new_cols as usize, Cell::blank);
        }
        self.dirty = vec![true; new_rows as usize];
        self.cols = new_cols;
        self.rows = new_rows;
        self.cursor.col = self.cursor.col.min(new_cols.saturating_sub(1));
        self.cursor.row = self.cursor.row.min(new_rows.saturating_sub(1));
        self.scroll_top = 0;
        self.scroll_bottom = new_rows.saturating_sub(1);
    }

    /// セルを直接書き込み
    pub(super) fn put_cell(&mut self, col: u16, row: u16, cell: Cell) {
        if row < self.rows && col < self.cols {
            self.cells[row as usize][col as usize] = cell;
            self.dirty[row as usize] = true;
        }
    }
}
