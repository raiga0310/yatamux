//! ターミナルグリッド（仮想スクリーンバッファ）
//!
//! tmux の Grid / zellij の Grid を参考にした CJK 対応実装。
//! 全角文字の行境界折り返し（DECAWM + LCF）を正しく処理する。

mod grapheme;
mod screen;
mod scrollback;
mod state;
mod text;

use crate::cell::{Cell, CellStyle};
use crate::width::CjkWidthConfig;
pub use scrollback::ScrollbackBuffer;
pub use text::{normalize_nfc, row_cells_to_text};

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
    /// スクロールバックバッファ（画面外に出た行。インデックス 0 が最古）
    scrollback: ScrollbackBuffer,
}

impl Grid {
    /// スクロールバックバッファの最大行数
    pub const SCROLLBACK_MAX: usize = 50_000;
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
            scrollback: ScrollbackBuffer::new(Self::SCROLLBACK_MAX),
        }
    }

    pub fn cols(&self) -> u16 {
        self.cols
    }
    pub fn rows(&self) -> u16 {
        self.rows
    }
    pub fn cursor(&self) -> CursorPos {
        self.cursor
    }

    /// スクロールバック + 現在の画面全体をプレーンテキストとして返す
    ///
    /// コピー・外部エディタ起動などに使用する。各行末尾の空白は除去される。
    pub fn full_content_text(&self) -> String {
        text::full_content_text(self)
    }

    /// 指定した行範囲（0-based, inclusive）をプレーンテキストとして抽出する。
    ///
    /// - 各行の末尾空白を除去する
    /// - Continuation セル（CJK 全角文字の右半分）はスキップする
    /// - 行は `\n` で連結する
    /// - `row_start` > `row_end` の場合は空文字列を返す
    /// - 範囲外はグリッドの行数にクランプされる
    pub fn extract_text(&self, row_start: usize, row_end: usize) -> String {
        text::extract_text(self, row_start, row_end)
    }

    /// DECAWM 自動折り返しモードを設定する
    ///
    /// `\x1b[?7h` (on) / `\x1b[?7l` (off) に対応。
    pub fn set_auto_wrap(&mut self, enabled: bool) {
        self.flags.auto_wrap = enabled;
    }

    /// カーソル直前のグラフィームセルに文字を結合する
    ///
    /// ZWJ 結合絵文字や VS-15/VS-16 など、前のセルに付加するコードポイントに使用する。
    /// 前にグラフィームセルが見つからない場合は何もしない。
    pub fn combine_with_last_cell(&mut self, c: char) {
        grapheme::combine_with_last_cell(self, c)
    }

    /// VS-16 (絵文字表示セレクタ) をカーソル直前セルに適用する
    ///
    /// 前セルが width=1 の場合は width=2 に拡張し、直後に Continuation セルを挿入する。
    pub fn apply_vs16(&mut self) {
        grapheme::apply_vs16(self)
    }

    /// カーソル直前のグラフィームセルのテキストが ZWJ で終わるか調べる
    pub fn last_grapheme_ends_with_zwj(&self) -> bool {
        grapheme::last_grapheme_ends_with_zwj(self)
    }

    /// 文字をカーソル位置に書き込む
    ///
    /// 全角文字が行末にはみ出す場合は早期折り返しを行う（DECAWM + LCF）。
    pub fn write_char(&mut self, grapheme: &str, style: CellStyle) {
        // str_width で書記素クラスタ全体の幅を計算（VS-16, ZWJ 対応）
        let width = match self.width_config.str_width(grapheme) {
            0 => return, // 幅 0（結合文字など）→ skip
            w => (w as u8).min(2),
        };

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
    /// フルスクリーンスクロール（scroll_top==0 かつ scroll_bottom==rows-1）かつ
    /// メインスクリーン表示中のとき、画面外に出た行をスクロールバックに保存する。
    pub fn scroll_up(&mut self, n: u16) {
        let top = self.scroll_top as usize;
        let bot = self.scroll_bottom as usize;
        let region_len = bot + 1 - top;
        let n = (n as usize).min(region_len);

        // フルスクリーンかつメインスクリーンのときのみスクロールバックに保存
        let is_full_screen = self.scroll_top == 0
            && self.scroll_bottom == self.rows.saturating_sub(1)
            && self.saved_main.is_none();
        if is_full_screen {
            for i in 0..n {
                let row = self.cells[i].clone();
                self.scrollback.push(row);
            }
        }

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
        assert!(matches!(
            g.row(0).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
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
        assert!(matches!(
            g.row(0).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
        // row 1 の B は消える
        assert_eq!(g.row(1).unwrap()[0].content, CellContent::Blank);
    }

    // B-6: スタイル（bold, reverse）がセルに格納される
    #[test]
    fn test_cell_style_bold_reverse() {
        let mut g = default_grid(80, 24);
        let style = CellStyle {
            bold: true,
            reverse: true,
            ..CellStyle::default()
        };
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
        assert_eq!(
            g.cursor().row,
            1,
            "DECAWM on: 5 文字目以降は row 1 に折り返す"
        );
    }
}
