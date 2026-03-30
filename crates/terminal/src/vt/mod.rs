//! VT シーケンスパーサー → Grid への適用
//!
//! `vte` クレートのステートマシンをラップし、
//! VT エスケープシーケンスを Grid 操作に変換する。

mod color;
mod csi;
mod esc;
mod osc;
mod sgr;

use crate::cell::CellStyle;
use crate::grid::Grid;
use vte::{Params, Parser, Perform};

use self::csi::dispatch_csi;
use self::esc::dispatch_esc;
use self::osc::dispatch_osc;

/// VT シーケンスを受け取って Grid を更新するプロセッサ
pub struct VtProcessor<'a> {
    pub grid: &'a mut Grid,
    current_style: CellStyle,
    /// OSC タイトル（OSC 2）
    pub title: Option<String>,
    /// OSC 通知本文（OSC 9/99/777）
    pub notification: Option<String>,
    /// OSC 52 クリップボードデータ（base64 デコード済みバイト列）
    pub clipboard_data: Option<Vec<u8>>,
    /// OSC 133;D — シェルコマンド終了通知（Some: 終了した、None: 未検出）
    /// exit_code は D;{code} から抽出（省略時は None）
    pub command_finished: Option<Option<i32>>,
    /// BEL（0x07）受信フラグ
    pub bell: bool,
}

impl<'a> VtProcessor<'a> {
    pub fn new(grid: &'a mut Grid) -> Self {
        Self {
            grid,
            current_style: CellStyle::default(),
            title: None,
            notification: None,
            clipboard_data: None,
            command_finished: None,
            bell: false,
        }
    }
}

/// BiDi 制御文字かどうかを判定する。
///
/// これらの文字はカーソルを進めず、セルにも書き込まない（幅0として扱う）。
///
/// - U+200E LEFT-TO-RIGHT MARK
/// - U+200F RIGHT-TO-LEFT MARK
/// - U+202A–U+202E (LRE, RLE, PDF, LRO, RLO)
/// - U+2066–U+2069 (LRI, RLI, FSI, PDI)
/// - U+061C ARABIC LETTER MARK
#[inline]
fn is_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{200E}'  // LEFT-TO-RIGHT MARK
        | '\u{200F}'  // RIGHT-TO-LEFT MARK
        | '\u{202A}'  // LEFT-TO-RIGHT EMBEDDING
        | '\u{202B}'  // RIGHT-TO-LEFT EMBEDDING
        | '\u{202C}'  // POP DIRECTIONAL FORMATTING
        | '\u{202D}'  // LEFT-TO-RIGHT OVERRIDE
        | '\u{202E}'  // RIGHT-TO-LEFT OVERRIDE
        | '\u{2066}'  // LEFT-TO-RIGHT ISOLATE
        | '\u{2067}'  // RIGHT-TO-LEFT ISOLATE
        | '\u{2068}'  // FIRST STRONG ISOLATE
        | '\u{2069}'  // POP DIRECTIONAL ISOLATE
        | '\u{061C}' // ARABIC LETTER MARK
    )
}

impl<'a> Perform for VtProcessor<'a> {
    fn print(&mut self, c: char) {
        use unicode_width::UnicodeWidthChar;

        match c {
            // VS-16 (U+FE0F): 絵文字表示セレクタ → 前セルを 2 セル幅に拡張
            '\u{FE0F}' => self.grid.apply_vs16(),
            // VS-15 (U+FE0E): テキスト表示セレクタ → 前セルにテキスト付加（幅変更なし）
            '\u{FE0E}' => self.grid.combine_with_last_cell(c),
            // ZWJ (U+200D): ゼロ幅結合子 → 前セルに付加（次の文字が結合される）
            '\u{200D}' => self.grid.combine_with_last_cell(c),
            // BiDi 制御文字 → カーソルを進めず、セルにも書き込まない（幅0扱い）
            c if is_bidi_control(c) => {}
            _ => {
                let w = UnicodeWidthChar::width(c).unwrap_or(0);
                if w == 0 {
                    // 一般的な結合文字 → 前セルに付加
                    self.grid.combine_with_last_cell(c);
                } else if self.grid.last_grapheme_ends_with_zwj() {
                    // 前セルが ZWJ で終わっている → この文字を結合（ZWJ シーケンス完成）
                    self.grid.combine_with_last_cell(c);
                } else {
                    self.grid.write_char(&c.to_string(), self.current_style);
                }
            }
        }
    }

    fn execute(&mut self, byte: u8) {
        match byte {
            // CR
            b'\r' => self.grid.carriage_return(),
            // LF / VT / FF
            b'\n' | 0x0B | 0x0C => self.grid.line_feed(),
            // BS
            0x08 => {
                let cur = self.grid.cursor();
                if cur.col > 0 {
                    self.grid.move_cursor(cur.col - 1, cur.row);
                }
            }
            // BEL — 通知フラグを立てる
            0x07 => {
                self.bell = true;
            }
            _ => {}
        }
    }

    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        dispatch_csi(self, params, intermediates, action);
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        dispatch_osc(
            &mut self.title,
            &mut self.notification,
            &mut self.clipboard_data,
            &mut self.command_finished,
            params,
        );
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn esc_dispatch(&mut self, intermediates: &[u8], _ignore: bool, byte: u8) {
        dispatch_esc(self, intermediates, byte);
    }
}

/// バイト列を VT パースして Grid に適用
pub fn feed_bytes(parser: &mut Parser, processor: &mut VtProcessor<'_>, data: &[u8]) {
    parser.advance(processor, data);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::grid::Grid;
    use crate::width::CjkWidthConfig;

    fn make_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    /// グリッドに VT バイト列を適用するヘルパー
    fn feed(g: &mut Grid, data: &[u8]) {
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(g);
        feed_bytes(&mut parser, &mut proc, data);
    }

    // CR (0x0D) でカーソルが列 0 に戻る
    #[test]
    fn test_vt_carriage_return() {
        let mut g = make_grid(80, 24);
        g.move_cursor(10, 0);
        feed(&mut g, b"\r");
        assert_eq!(g.cursor().col, 0);
        assert_eq!(g.cursor().row, 0);
    }

    // LF (0x0A) でカーソルが 1 行下に進む
    #[test]
    fn test_vt_line_feed() {
        let mut g = make_grid(80, 24);
        g.move_cursor(0, 0);
        feed(&mut g, b"\n");
        assert_eq!(g.cursor().row, 1);
    }

    // CR+LF でカーソルが次行の先頭へ
    #[test]
    fn test_vt_crlf() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"Hello\r\nWorld");
        assert_eq!(g.cursor().row, 1);
        assert_eq!(g.cursor().col, 5);
    }

    // BS (0x08) でカーソルが 1 セル左に移動
    #[test]
    fn test_vt_backspace() {
        let mut g = make_grid(80, 24);
        g.move_cursor(5, 0);
        feed(&mut g, b"\x08"); // BS
        assert_eq!(g.cursor().col, 4);
    }

    // BS でカーソルが col=0 のとき移動しない
    #[test]
    fn test_vt_backspace_at_col_zero() {
        let mut g = make_grid(80, 24);
        g.move_cursor(0, 0);
        feed(&mut g, b"\x08");
        assert_eq!(g.cursor().col, 0);
    }

    // TC-03: BEL バイト受信で bell フラグが立つ
    #[test]
    fn test_vt_bell_sets_flag() {
        let mut g = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(&mut g);
        feed_bytes(&mut parser, &mut proc, b"\x07");
        assert!(proc.bell);
    }

    // TC-04: BEL を含まない入力では bell フラグが立たない
    #[test]
    fn test_vt_no_bell_without_bel_byte() {
        let mut g = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(&mut g);
        feed_bytes(&mut parser, &mut proc, b"hello world");
        assert!(!proc.bell);
    }

    // ── BiDi 制御文字テスト (C-6) ─────────────────────────────────────────

    /// BiDi 制御文字入りの文字列をフィードしてカーソル位置を確認するヘルパー
    fn feed_str(g: &mut Grid, s: &str) {
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(g);
        feed_bytes(&mut parser, &mut proc, s.as_bytes());
    }

    // TC-C6-01: RIGHT-TO-LEFT MARK (U+200F) はカーソルを進めない
    #[test]
    fn test_bidi_rtl_mark_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{200F}B");
        assert_eq!(g.cursor().col, 2, "U+200F should not advance cursor");
    }

    // TC-C6-02: LEFT-TO-RIGHT MARK (U+200E) はカーソルを進めない
    #[test]
    fn test_bidi_ltr_mark_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{200E}B");
        assert_eq!(g.cursor().col, 2, "U+200E should not advance cursor");
    }

    // TC-C6-03: LRE (U+202A) はカーソルを進めない
    #[test]
    fn test_bidi_lre_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{202A}B");
        assert_eq!(g.cursor().col, 2, "U+202A should not advance cursor");
    }

    // TC-C6-04: RLE (U+202B) はカーソルを進めない
    #[test]
    fn test_bidi_rle_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{202B}B");
        assert_eq!(g.cursor().col, 2, "U+202B should not advance cursor");
    }

    // TC-C6-05: PDF (U+202C) はカーソルを進めない
    #[test]
    fn test_bidi_pdf_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{202C}B");
        assert_eq!(g.cursor().col, 2, "U+202C should not advance cursor");
    }

    // TC-C6-06: LRO (U+202D) はカーソルを進めない
    #[test]
    fn test_bidi_lro_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{202D}B");
        assert_eq!(g.cursor().col, 2, "U+202D should not advance cursor");
    }

    // TC-C6-07: RLO (U+202E) はカーソルを進めない
    #[test]
    fn test_bidi_rlo_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{202E}B");
        assert_eq!(g.cursor().col, 2, "U+202E should not advance cursor");
    }

    // TC-C6-08: LRI (U+2066) はカーソルを進めない
    #[test]
    fn test_bidi_lri_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{2066}B");
        assert_eq!(g.cursor().col, 2, "U+2066 should not advance cursor");
    }

    // TC-C6-09: RLI (U+2067) はカーソルを進めない
    #[test]
    fn test_bidi_rli_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{2067}B");
        assert_eq!(g.cursor().col, 2, "U+2067 should not advance cursor");
    }

    // TC-C6-10: FSI (U+2068) はカーソルを進めない
    #[test]
    fn test_bidi_fsi_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{2068}B");
        assert_eq!(g.cursor().col, 2, "U+2068 should not advance cursor");
    }

    // TC-C6-11: PDI (U+2069) はカーソルを進めない
    #[test]
    fn test_bidi_pdi_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{2069}B");
        assert_eq!(g.cursor().col, 2, "U+2069 should not advance cursor");
    }

    // TC-C6-12: ARABIC LETTER MARK (U+061C) はカーソルを進めない
    #[test]
    fn test_bidi_arabic_letter_mark_zero_width() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "A\u{061C}B");
        assert_eq!(g.cursor().col, 2, "U+061C should not advance cursor");
    }

    // TC-C6-13: BiDi 制御文字のみの文字列はカーソルを動かさない
    #[test]
    fn test_bidi_only_controls_no_cursor_movement() {
        let mut g = make_grid(80, 24);
        feed_str(&mut g, "\u{200F}\u{200E}\u{202A}");
        assert_eq!(
            g.cursor().col,
            0,
            "Only BiDi controls should not move cursor"
        );
        assert_eq!(
            g.cursor().row,
            0,
            "Only BiDi controls should not move cursor row"
        );
    }
}
