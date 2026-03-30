use super::{Cell, CursorPos, Grid};

impl Grid {
    /// カーソル位置を保存する (`ESC 7` / `CSI s`)
    pub fn save_cursor(&mut self) {
        self.flags.saved_cursor = Some(self.cursor);
    }

    /// 保存したカーソル位置を復元する (`ESC 8` / `CSI u`)
    pub fn restore_cursor(&mut self) {
        if let Some(pos) = self.flags.saved_cursor {
            self.move_cursor(pos.col, pos.row);
        }
    }

    /// カーソルの表示・非表示を設定する (`CSI ?25h/l`)
    pub fn set_cursor_visible(&mut self, visible: bool) {
        self.flags.cursor_visible = visible;
    }

    /// カーソルが表示状態か
    pub fn cursor_visible(&self) -> bool {
        self.flags.cursor_visible
    }

    /// DECCKM アプリケーションカーソルキーモードを設定する (`CSI ?1h/l`)
    pub fn set_application_cursor_keys(&mut self, enable: bool) {
        self.flags.application_cursor_keys = enable;
    }

    /// DECCKM アプリケーションカーソルキーモードが有効か
    pub fn application_cursor_keys(&self) -> bool {
        self.flags.application_cursor_keys
    }

    /// ブラケットペーストモードを設定する (`CSI ?2004h/l`)
    pub fn set_bracketed_paste(&mut self, enable: bool) {
        self.flags.bracketed_paste = enable;
    }

    /// ブラケットペーストモードが有効か
    pub fn bracketed_paste(&self) -> bool {
        self.flags.bracketed_paste
    }

    /// マウス報告モードを設定する (`CSI ?1000h/1002h/1003h/l`)
    pub fn set_mouse_reporting(&mut self, mode: u8) {
        self.flags.mouse_reporting = mode;
    }

    /// 現在のマウス報告モード (0=off, 1=x10, 2=button, 3=any)
    pub fn mouse_reporting(&self) -> u8 {
        self.flags.mouse_reporting
    }

    /// SGR マウス拡張モードを設定する (`CSI ?1006h/l`)
    pub fn set_mouse_sgr(&mut self, enable: bool) {
        self.flags.mouse_sgr = enable;
    }

    /// SGR マウス拡張モードが有効か
    pub fn mouse_sgr(&self) -> bool {
        self.flags.mouse_sgr
    }

    /// フォーカスイベントモードを設定する (`CSI ?1004h/l`)
    pub fn set_focus_events(&mut self, enable: bool) {
        self.flags.focus_events = enable;
    }

    /// フォーカスイベントモードが有効か
    pub fn focus_events(&self) -> bool {
        self.flags.focus_events
    }

    /// DECSTBM スクロール領域を設定する (`CSI Pt;Pb r`)
    ///
    /// `top` と `bottom` は 1 始まり行番号（端末の行番号と同じ）。
    /// 範囲外・逆順の場合はフル画面にリセット。
    /// 設定後はカーソルを左上 (0,0) に移動する（XTerm 互換）。
    pub fn set_scroll_region(&mut self, top: u16, bottom: u16) {
        let t = top.saturating_sub(1);
        let b = bottom.saturating_sub(1).min(self.rows.saturating_sub(1));
        if t < b {
            self.scroll_top = t;
            self.scroll_bottom = b;
        } else {
            self.scroll_top = 0;
            self.scroll_bottom = self.rows.saturating_sub(1);
        }
        self.cursor = CursorPos::default();
        self.flags.last_col_flag = false;
    }

    /// スクロール領域の上端行（0 始まり）
    pub fn scroll_top(&self) -> u16 {
        self.scroll_top
    }

    /// スクロール領域の下端行（0 始まり、inclusive）
    pub fn scroll_bottom(&self) -> u16 {
        self.scroll_bottom
    }

    /// スクロールバックに蓄積された行数
    pub fn scrollback_len(&self) -> usize {
        self.scrollback.len()
    }

    /// スクロールバックの行を取得する（0 が最古、len-1 が最新）
    pub fn scrollback_row(&self, idx: usize) -> Option<&Vec<Cell>> {
        self.scrollback.get(idx)
    }

    /// 行を参照
    pub fn row(&self, row: u16) -> Option<&[Cell]> {
        self.cells.get(row as usize).map(|row| row.as_slice())
    }

    /// 行を可変参照
    pub fn row_mut(&mut self, row: usize) -> Option<&mut Vec<Cell>> {
        self.cells.get_mut(row)
    }

    /// 指定行をダーティとしてマーク
    pub fn mark_dirty(&mut self, row: usize) {
        if row < self.dirty.len() {
            self.dirty[row] = true;
        }
    }

    /// スクロール領域を 0 始まり行番号で直接設定（カーソル移動なし）
    ///
    /// IL/DL 等が一時的に領域を変えて戻すために使用する内部ヘルパー。
    pub fn set_scroll_region_raw(&mut self, top: u16, bottom: u16) {
        let bottom = bottom.min(self.rows.saturating_sub(1));
        if top <= bottom {
            self.scroll_top = top;
            self.scroll_bottom = bottom;
        }
    }

    /// ダーティ行が 1 行以上あるか（リセットしない）
    pub fn has_dirty_rows(&self) -> bool {
        self.dirty.iter().any(|&dirty| dirty)
    }

    /// ダーティ行のインデックスを返しリセット
    pub fn take_dirty_rows(&mut self) -> Vec<u16> {
        self.dirty
            .iter_mut()
            .enumerate()
            .filter_map(|(index, dirty)| {
                if *dirty {
                    *dirty = false;
                    Some(index as u16)
                } else {
                    None
                }
            })
            .collect()
    }
}
