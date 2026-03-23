//! ターミナルグリッド（仮想スクリーンバッファ）
//!
//! tmux の Grid / zellij の Grid を参考にした CJK 対応実装。
//! 全角文字の行境界折り返し（DECAWM + LCF）を正しく処理する。

use crate::cell::{Cell, CellStyle};
use crate::width::CjkWidthConfig;

/// カーソル位置
#[derive(Debug, Clone, Copy, Default)]
pub struct CursorPos {
    pub col: u16,
    pub row: u16,
}

/// グリッドフラグ
#[derive(Debug, Clone, Copy)]
struct GridFlags {
    /// 自動折り返しモード（DECAWM）
    auto_wrap: bool,
    /// Last Column Flag — 次の文字入力で折り返し
    last_col_flag: bool,
    /// カーソル表示フラグ（`CSI ?25h/l`）
    cursor_visible: bool,
    /// 保存済みカーソル位置（`ESC 7` / `CSI s`）
    saved_cursor: Option<CursorPos>,
    /// DECCKM: アプリケーションカーソルキーモード（`CSI ?1h/l`）
    application_cursor_keys: bool,
    /// ブラケットペーストモード（`CSI ?2004h/l`）
    bracketed_paste: bool,
    /// マウス報告モード: 0=off, 1=x10(1000), 2=button(1002), 3=any(1003)
    mouse_reporting: u8,
    /// マウス座標報告形式: false=X10, true=SGR拡張(`CSI ?1006h`)
    mouse_sgr: bool,
    /// フォーカスイベント送信 (`CSI ?1004h/l`)
    focus_events: bool,
}

impl Default for GridFlags {
    fn default() -> Self {
        Self {
            auto_wrap: true,
            last_col_flag: false,
            cursor_visible: true,
            saved_cursor: None,
            application_cursor_keys: false,
            bracketed_paste: false,
            mouse_reporting: 0,
            mouse_sgr: false,
            focus_events: false,
        }
    }
}

/// 仮想スクリーンバッファ
pub struct Grid {
    cols: u16,
    rows: u16,
    cells: Vec<Vec<Cell>>,
    cursor: CursorPos,
    flags: GridFlags,
    pub width_config: CjkWidthConfig,
    /// ダーティ行フラグ（差分レンダリング用）
    dirty: Vec<bool>,
    /// オルタネートスクリーン保存領域（`?1049h` で退避、`?1049l` で復元）
    saved_main: Option<MainScreenSnapshot>,
    /// DECSTBM スクロール領域の上端行（0 始まり）
    scroll_top: u16,
    /// DECSTBM スクロール領域の下端行（0 始まり、inclusive）
    scroll_bottom: u16,
}

/// メインスクリーンのスナップショット（オルタネートスクリーン切り替え用）
struct MainScreenSnapshot {
    cells: Vec<Vec<Cell>>,
    cursor: CursorPos,
    flags: GridFlags,
}

impl Grid {
    pub fn new(cols: u16, rows: u16, width_config: CjkWidthConfig) -> Self {
        let cells = vec![vec![Cell::blank(); cols as usize]; rows as usize];
        let dirty = vec![true; rows as usize];
        let scroll_bottom = rows.saturating_sub(1);
        Self {
            cols,
            rows,
            cells,
            cursor: CursorPos::default(),
            flags: GridFlags::default(),
            width_config,
            dirty,
            saved_main: None,
            scroll_top: 0,
            scroll_bottom,
        }
    }

    pub fn cols(&self) -> u16 { self.cols }
    pub fn rows(&self) -> u16 { self.rows }
    pub fn cursor(&self) -> CursorPos { self.cursor }

    /// オルタネートスクリーンに切り替える (`CSI ?1049h`)
    ///
    /// メインスクリーンの内容・カーソル・フラグを退避し、
    /// 空のオルタネートバッファを表示する。
    pub fn enter_alternate_screen(&mut self) {
        if self.saved_main.is_some() {
            return; // 二重切り替えは無視
        }
        self.saved_main = Some(MainScreenSnapshot {
            cells: self.cells.clone(),
            cursor: self.cursor,
            flags: self.flags,
        });
        // オルタネートバッファをクリア
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

    /// カーソル位置を保存する (`ESC 7` / `CSI s`)
    pub fn save_cursor(&mut self) {
        // saved_cursor フィールドを GridFlags に追加せず、別フィールドで管理
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
        // 1 始まり → 0 始まりに変換
        let t = top.saturating_sub(1);
        let b = bottom.saturating_sub(1).min(self.rows.saturating_sub(1));
        if t < b {
            self.scroll_top = t;
            self.scroll_bottom = b;
        } else {
            // 無効な引数 → フル画面
            self.scroll_top = 0;
            self.scroll_bottom = self.rows.saturating_sub(1);
        }
        // XTerm 互換: カーソルをホームに移動
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

    /// 行頭からカーソル位置までを消去する (`EL 1`)
    pub fn erase_line_left(&mut self) {
        let row = self.cursor.row as usize;
        let col = self.cursor.col as usize;
        for c in self.cells[row][..=col].iter_mut() {
            *c = Cell::blank();
        }
        self.dirty[row] = true;
    }

    /// 現在行全体を消去する (`EL 2`)
    pub fn erase_line_all(&mut self) {
        let row = self.cursor.row as usize;
        self.cells[row].fill(Cell::blank());
        self.dirty[row] = true;
    }

    /// 画面先頭からカーソル位置までを消去する (`ED 1`)
    pub fn erase_display_above(&mut self) {
        let cur_row = self.cursor.row as usize;
        // 現在行の左側（行頭→カーソル列）
        self.erase_line_left();
        // それより上の行
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

    /// DECAWM 自動折り返しモードを設定する
    ///
    /// `\x1b[?7h` (on) / `\x1b[?7l` (off) に対応。
    pub fn set_auto_wrap(&mut self, enabled: bool) {
        self.flags.auto_wrap = enabled;
    }

    /// グリッドをリサイズ（ConPTY リサイズ時に呼び出し）
    pub fn resize(&mut self, new_cols: u16, new_rows: u16) {
        // 行数調整
        self.cells.resize_with(new_rows as usize, || vec![Cell::blank(); new_cols as usize]);
        // 各行の列数調整
        for row in &mut self.cells {
            row.resize_with(new_cols as usize, Cell::blank);
        }
        self.dirty = vec![true; new_rows as usize];
        self.cols = new_cols;
        self.rows = new_rows;
        // カーソルをバッファ内にクリップ
        self.cursor.col = self.cursor.col.min(new_cols.saturating_sub(1));
        self.cursor.row = self.cursor.row.min(new_rows.saturating_sub(1));
        // スクロール領域をフル画面にリセット
        self.scroll_top = 0;
        self.scroll_bottom = new_rows.saturating_sub(1);
    }

    /// 文字をカーソル位置に書き込む
    ///
    /// 全角文字が行末にはみ出す場合は早期折り返しを行う（DECAWM + LCF）。
    pub fn write_char(&mut self, grapheme: &str, style: CellStyle) {
        let first_char = match grapheme.chars().next() {
            Some(c) => c,
            None => return,
        };
        let width = self.width_config.char_width(first_char);

        if width == 0 {
            // 結合文字：現在セルに付加（簡易実装）
            return;
        }

        // DECAWM: last_col_flag が立っていたら折り返し
        if self.flags.last_col_flag && self.flags.auto_wrap {
            self.carriage_return();
            self.line_feed();
        }
        self.flags.last_col_flag = false;

        // 全角文字が行末をまたぐ場合の早期折り返し
        if width == 2 && self.cursor.col + 2 > self.cols {
            // 残り 1 セルに全角文字は入らない → 行末をスペースで埋めて折り返し
            self.put_cell(self.cursor.col, self.cursor.row, Cell::blank());
            self.carriage_return();
            self.line_feed();
        }

        let col = self.cursor.col;
        let row = self.cursor.row;

        // リーディングセルを書き込む
        let cell = Cell::from_grapheme(grapheme.to_string(), width, style);
        self.put_cell(col, row, cell);

        if width == 2 {
            // トレーリングセル（Continuation）を書き込む
            if (col + 1) < self.cols {
                self.put_cell(col + 1, row, Cell::continuation(style));
            }
        }

        // カーソル前進
        let new_col = col + width as u16;
        if new_col >= self.cols {
            // 行末に達した
            self.cursor.col = self.cols - 1;
            self.flags.last_col_flag = true;
        } else {
            self.cursor.col = new_col;
        }
    }

    /// カーソルを指定位置に移動（CUP / HVP）
    pub fn move_cursor(&mut self, col: u16, row: u16) {
        self.cursor.col = col.min(self.cols.saturating_sub(1));
        self.cursor.row = row.min(self.rows.saturating_sub(1));
        self.flags.last_col_flag = false;
    }

    /// 改行（LF）— DECSTBM スクロール領域を考慮する
    pub fn line_feed(&mut self) {
        if self.cursor.row == self.scroll_bottom {
            // スクロール領域の最下行にいる → 領域内をスクロールしてカーソルは動かない
            self.scroll_up(1);
        } else if self.cursor.row + 1 < self.rows {
            self.cursor.row += 1;
        }
        // カーソルが scroll_bottom より下にいる（領域外で LF）場合は
        // 通常の下方向移動だが画面末尾は超えない
    }

    /// 復帰（CR）
    pub fn carriage_return(&mut self) {
        self.cursor.col = 0;
        self.flags.last_col_flag = false;
    }

    /// スクロール領域を上にスクロール（DECSTBM 対応）
    ///
    /// `scroll_top`–`scroll_bottom` の範囲のみ n 行上にシフトし、
    /// 空いた最下 n 行を空白で埋める。領域外の行は変化しない。
    pub fn scroll_up(&mut self, n: u16) {
        let top = self.scroll_top as usize;
        let bot = self.scroll_bottom as usize;
        let region_len = bot + 1 - top;
        let n = (n as usize).min(region_len);

        self.cells[top..=bot].rotate_left(n);
        for row in self.cells[top..=bot].iter_mut().rev().take(n) {
            row.fill(Cell::blank());
        }
        for i in top..=bot {
            self.dirty[i] = true;
        }
    }

    /// スクロール領域を下にスクロール（`CSI Pn T` / `CSI Pn S` の逆）
    ///
    /// `scroll_top`–`scroll_bottom` の範囲を n 行下にシフトし、
    /// 空いた最上 n 行を空白で埋める。
    pub fn scroll_down(&mut self, n: u16) {
        let top = self.scroll_top as usize;
        let bot = self.scroll_bottom as usize;
        let region_len = bot + 1 - top;
        let n = (n as usize).min(region_len);

        self.cells[top..=bot].rotate_right(n);
        for row in self.cells[top..=bot].iter_mut().take(n) {
            row.fill(Cell::blank());
        }
        for i in top..=bot {
            self.dirty[i] = true;
        }
    }

    /// 行のクリア（EOL）
    pub fn erase_line_right(&mut self) {
        let row = self.cursor.row as usize;
        let col = self.cursor.col as usize;
        for c in self.cells[row][col..].iter_mut() {
            *c = Cell::blank();
        }
        self.dirty[row] = true;
    }

    /// 画面のクリア（ED 0: カーソル以降）
    pub fn erase_display_below(&mut self) {
        let cur_row = self.cursor.row as usize;
        // 現在行の右側
        self.erase_line_right();
        // 以降の行
        for row in (cur_row + 1)..self.rows as usize {
            self.cells[row].fill(Cell::blank());
            self.dirty[row] = true;
        }
    }

    /// セルを直接書き込み
    fn put_cell(&mut self, col: u16, row: u16, cell: Cell) {
        if row < self.rows && col < self.cols {
            self.cells[row as usize][col as usize] = cell;
            self.dirty[row as usize] = true;
        }
    }

    /// 行を参照
    pub fn row(&self, row: u16) -> Option<&[Cell]> {
        self.cells.get(row as usize).map(|r| r.as_slice())
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
        let b = bottom.min(self.rows.saturating_sub(1));
        if top <= b {
            self.scroll_top = top;
            self.scroll_bottom = b;
        }
    }

    /// ダーティ行が 1 行以上あるか（リセットしない）
    pub fn has_dirty_rows(&self) -> bool {
        self.dirty.iter().any(|&d| d)
    }

    /// ダーティ行のインデックスを返しリセット
    pub fn take_dirty_rows(&mut self) -> Vec<u16> {
        self.dirty
            .iter_mut()
            .enumerate()
            .filter_map(|(i, d)| {
                if *d {
                    *d = false;
                    Some(i as u16)
                } else {
                    None
                }
            })
            .collect()
    }
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
    use crate::cell::CellContent;
    use crate::width::CjkWidthConfig;

    fn default_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    #[test]
    fn test_ascii_write() {
        let mut g = default_grid(80, 24);
        g.write_char("A", CellStyle::default());
        assert_eq!(g.cursor().col, 1);
    }

    #[test]
    fn test_wide_char_write() {
        let mut g = default_grid(80, 24);
        g.write_char("漢", CellStyle::default());
        // 全角 = 2 セル進む
        assert_eq!(g.cursor().col, 2);
        // カラム 1 は Continuation
        let cell = &g.row(0).unwrap()[1];
        assert_eq!(cell.content, CellContent::Continuation);
    }

    #[test]
    fn test_wide_char_early_wrap() {
        // 79 列目（0-indexed）に全角文字 → 早期折り返し
        let mut g = default_grid(80, 24);
        g.move_cursor(79, 0);
        g.write_char("漢", CellStyle::default());
        // 折り返されて次の行の col=2 にあるはず
        assert_eq!(g.cursor().row, 1);
        assert_eq!(g.cursor().col, 2);
    }

    #[test]
    fn test_scroll_up() {
        let mut g = default_grid(4, 3);
        g.write_char("A", CellStyle::default());
        g.cursor = CursorPos { col: 0, row: 2 };
        g.scroll_up(1);
        // "A" は row 0 に移動（rotate_left）
        let cell = &g.row(0).unwrap()[0];
        // row 0 はもともと row 1（空行）になっているはず
        assert_eq!(cell.content, CellContent::Blank);
    }

    // B-2: テキストがセルに正しく格納される
    #[test]
    fn test_text_stored_in_cell() {
        let mut g = default_grid(80, 24);
        g.write_char("X", CellStyle::default());
        let cell = &g.row(0).unwrap()[0];
        match &cell.content {
            CellContent::Grapheme { text, width } => {
                assert_eq!(text, "X");
                assert_eq!(*width, 1);
            }
            _ => panic!("expected Grapheme"),
        }
    }

    // B-3: 全角文字がリーディング＋Continuation ペアで格納される (extend)
    #[test]
    fn test_wide_char_leading_and_continuation() {
        let mut g = default_grid(80, 24);
        g.write_char("漢", CellStyle::default());
        let leading = &g.row(0).unwrap()[0];
        let trailing = &g.row(0).unwrap()[1];
        match &leading.content {
            CellContent::Grapheme { text, width } => {
                assert_eq!(text, "漢");
                assert_eq!(*width, 2);
            }
            _ => panic!("expected Grapheme at col 0"),
        }
        assert_eq!(trailing.content, CellContent::Continuation);
    }

    // B-4: スクロール後に古い行が消える
    #[test]
    fn test_scroll_clears_top_row() {
        let mut g = default_grid(10, 3);
        g.write_char("A", CellStyle::default()); // row 0, col 0
        g.scroll_up(1);
        // row 0 はもともと row 1（空行）になっている
        assert_eq!(g.row(0).unwrap()[0].content, CellContent::Blank);
    }

    // B-4: スクロールで内容が 1 行上に移動する
    #[test]
    fn test_scroll_moves_content_up() {
        let mut g = default_grid(10, 3);
        g.move_cursor(0, 1);
        g.write_char("B", CellStyle::default()); // row 1 に 'B'
        g.scroll_up(1);
        // 'B' は row 0 に移動しているはず
        match &g.row(0).unwrap()[0].content {
            CellContent::Grapheme { text, .. } => assert_eq!(text, "B"),
            _ => panic!("expected 'B' at row 0 after scroll"),
        }
    }

    // B-5: EL (erase line right) — カーソル以降が消える
    #[test]
    fn test_erase_line_right_clears_from_cursor() {
        let mut g = default_grid(10, 5);
        g.write_char("A", CellStyle::default()); // col 0
        g.write_char("B", CellStyle::default()); // col 1
        g.write_char("C", CellStyle::default()); // col 2
        g.move_cursor(1, 0); // col 1 に移動
        g.erase_line_right();
        // A は残る
        assert!(matches!(g.row(0).unwrap()[0].content, CellContent::Grapheme { .. }));
        // B, C は消える
        assert_eq!(g.row(0).unwrap()[1].content, CellContent::Blank);
        assert_eq!(g.row(0).unwrap()[2].content, CellContent::Blank);
    }

    // B-5: ED (erase display below) — 現在行以降が消える
    #[test]
    fn test_erase_display_below_clears_subsequent_rows() {
        let mut g = default_grid(10, 3);
        g.write_char("A", CellStyle::default()); // row 0
        g.move_cursor(0, 1);
        g.write_char("B", CellStyle::default()); // row 1
        g.move_cursor(0, 1);
        g.erase_display_below();
        // row 0 の A は残る
        assert!(matches!(g.row(0).unwrap()[0].content, CellContent::Grapheme { .. }));
        // row 1 の B は消える
        assert_eq!(g.row(1).unwrap()[0].content, CellContent::Blank);
    }

    // B-6: スタイル（bold, reverse）がセルに格納される
    #[test]
    fn test_cell_style_bold_reverse() {
        let mut g = default_grid(80, 24);
        let style = CellStyle { bold: true, reverse: true, ..CellStyle::default() };
        g.write_char("Y", style);
        let cell = &g.row(0).unwrap()[0];
        assert!(cell.style.bold);
        assert!(cell.style.reverse);
    }

    // B-6: スタイルがデフォルト状態では全フラグ false
    #[test]
    fn test_cell_style_default_is_plain() {
        let mut g = default_grid(80, 24);
        g.write_char("Z", CellStyle::default());
        let cell = &g.row(0).unwrap()[0];
        assert!(!cell.style.bold);
        assert!(!cell.style.reverse);
        assert!(!cell.style.underline);
    }

    // カーソル位置がグリッド外を指定した場合クランプされる
    #[test]
    fn test_move_cursor_clamps_to_bounds() {
        let mut g = default_grid(10, 5);
        g.move_cursor(100, 100);
        assert_eq!(g.cursor().col, 9);
        assert_eq!(g.cursor().row, 4);
    }

    // resize 後にカーソルが新サイズ内にクリップされる
    #[test]
    fn test_resize_clips_cursor() {
        let mut g = default_grid(80, 24);
        g.move_cursor(79, 23);
        g.resize(40, 12);
        assert!(g.cursor().col <= 39);
        assert!(g.cursor().row <= 11);
    }

    // D-2: DECAWM オフ時は行末で折り返さずカーソルが row 0 に留まる
    #[test]
    fn test_decawm_off_no_wrap() {
        let mut g = default_grid(5, 3);
        g.set_auto_wrap(false);
        for _ in 0..10 {
            g.write_char("X", CellStyle::default());
        }
        assert_eq!(g.cursor().row, 0, "DECAWM off: カーソルは row 0 のまま");
        assert_eq!(g.cursor().col, 4, "カーソルは末尾列に留まる");
    }

    // D-2: DECAWM オン時は通常通り折り返す（デフォルト動作の確認）
    #[test]
    fn test_decawm_on_wraps() {
        let mut g = default_grid(5, 3);
        // デフォルトは auto_wrap=true
        for _ in 0..6 {
            g.write_char("X", CellStyle::default());
        }
        assert_eq!(g.cursor().row, 1, "DECAWM on: 5 文字目以降は row 1 に折り返す");
    }

    // C-9: normalize_nfc は NFD → NFC を変換する
    #[test]
    fn test_normalize_nfc_converts_korean() {
        let nfd = "\u{110B}\u{1161}"; // NFD: ㅇ + ㅏ
        let nfc = normalize_nfc(nfd);
        assert_eq!(nfc, "아"); // NFC: U+C544
        assert_ne!(nfc, nfd);  // 変換されていることを確認
    }
}
