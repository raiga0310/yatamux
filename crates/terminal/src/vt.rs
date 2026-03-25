//! VT シーケンスパーサー → Grid への適用
//!
//! `vte` クレートのステートマシンをラップし、
//! VT エスケープシーケンスを Grid 操作に変換する。

use crate::cell::{Cell, CellStyle, Color};
use crate::grid::Grid;
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use vte::{Params, Parser, Perform};

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
    /// OSC 133;D — シェルコマンド終了通知
    pub command_finished: bool,
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
            command_finished: false,
            bell: false,
        }
    }
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
            _ => {
                let w = UnicodeWidthChar::width(c).unwrap_or(0);
                if w == 0 {
                    // 一般的な結合文字・BiDi 制御文字 → 前セルに付加
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

    fn csi_dispatch(
        &mut self,
        params: &Params,
        _intermediates: &[u8],
        _ignore: bool,
        action: char,
    ) {
        let p: Vec<u16> = params
            .iter()
            .map(|sub| sub.first().copied().unwrap_or(0))
            .collect();

        match action {
            // CUP / HVP — カーソル位置指定（1-indexed）
            'H' | 'f' => {
                let row = p.first().copied().unwrap_or(1).saturating_sub(1);
                let col = p.get(1).copied().unwrap_or(1).saturating_sub(1);
                self.grid.move_cursor(col, row);
            }
            // CUU — カーソル上移動
            'A' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_cursor(cur.col, cur.row.saturating_sub(n));
            }
            // CUD — カーソル下移動
            'B' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_cursor(cur.col, cur.row + n);
            }
            // CUF — カーソル右移動
            'C' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_cursor(cur.col + n, cur.row);
            }
            // CUB — カーソル左移動
            'D' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_cursor(cur.col.saturating_sub(n), cur.row);
            }
            // ED — 画面消去
            'J' => match p.first().copied().unwrap_or(0) {
                0 => self.grid.erase_display_below(),
                1 => self.grid.erase_display_above(),
                2 | 3 => self.grid.erase_display_all(),
                _ => {}
            },
            // EL — 行消去
            'K' => match p.first().copied().unwrap_or(0) {
                0 => self.grid.erase_line_right(),
                1 => self.grid.erase_line_left(),
                2 => self.grid.erase_line_all(),
                _ => {}
            },
            // DECSTBM — スクロール領域設定 (CSI Pt;Pb r)
            'r' if _intermediates.is_empty() => {
                let top = p.first().copied().unwrap_or(1).max(1);
                let bottom = p.get(1).copied().unwrap_or(self.grid.rows());
                self.grid.set_scroll_region(top, bottom);
            }
            // SU — 上スクロール (CSI Pn S)
            'S' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                self.grid.scroll_up(n);
            }
            // SD — 下スクロール (CSI Pn T)
            'T' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                self.grid.scroll_down(n);
            }
            // VPA — 垂直方向絶対位置 (CSI Pn d)
            'd' => {
                let row = p.first().copied().unwrap_or(1).saturating_sub(1);
                let cur = self.grid.cursor();
                self.grid.move_cursor(cur.col, row);
            }
            // CHA — 水平方向絶対位置 (CSI Pn G)
            'G' => {
                let col = p.first().copied().unwrap_or(1).saturating_sub(1);
                let cur = self.grid.cursor();
                self.grid.move_cursor(col, cur.row);
            }
            // CNL — 次行先頭 (CSI Pn E)
            'E' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_cursor(0, cur.row + n);
            }
            // CPL — 前行先頭 (CSI Pn F)
            'F' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                let cur = self.grid.cursor();
                self.grid.move_cursor(0, cur.row.saturating_sub(n));
            }
            // ICH — 文字挿入 (CSI Pn @)
            '@' => {
                let n = p.first().copied().unwrap_or(1).max(1) as usize;
                let row = self.grid.cursor().row as usize;
                let col = self.grid.cursor().col as usize;
                let cols = self.grid.cols() as usize;
                if col < cols {
                    let shift = n.min(cols - col);
                    if let Some(r) = self.grid.row_mut(row) {
                        let rlen = r.len();
                        r[col..].rotate_right(shift.min(rlen - col));
                        for c in r[col..col + shift].iter_mut() {
                            *c = Cell::blank();
                        }
                    }
                    self.grid.mark_dirty(row);
                }
            }
            // DCH — 文字削除 (CSI Pn P)
            'P' => {
                let n = p.first().copied().unwrap_or(1).max(1) as usize;
                let row = self.grid.cursor().row as usize;
                let col = self.grid.cursor().col as usize;
                let cols = self.grid.cols() as usize;
                if col < cols {
                    let del = n.min(cols - col);
                    if let Some(r) = self.grid.row_mut(row) {
                        let rlen = r.len();
                        r[col..].rotate_left(del.min(rlen - col));
                        for c in r[cols - del..].iter_mut() {
                            *c = Cell::blank();
                        }
                    }
                    self.grid.mark_dirty(row);
                }
            }
            // IL — 行挿入 (CSI Pn L)
            'L' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                // カーソル行から scroll_bottom まで n 行下にシフト
                let saved_top = self.grid.scroll_top();
                let saved_bot = self.grid.scroll_bottom();
                let cur_row = self.grid.cursor().row;
                self.grid.set_scroll_region_raw(cur_row, saved_bot);
                self.grid.scroll_down(n);
                self.grid.set_scroll_region_raw(saved_top, saved_bot);
            }
            // DL — 行削除 (CSI Pn M)
            'M' => {
                let n = p.first().copied().unwrap_or(1).max(1);
                let saved_top = self.grid.scroll_top();
                let saved_bot = self.grid.scroll_bottom();
                let cur_row = self.grid.cursor().row;
                self.grid.set_scroll_region_raw(cur_row, saved_bot);
                self.grid.scroll_up(n);
                self.grid.set_scroll_region_raw(saved_top, saved_bot);
            }
            // ECH — 文字消去 (CSI Pn X)
            'X' => {
                let n = p.first().copied().unwrap_or(1).max(1) as usize;
                let row = self.grid.cursor().row as usize;
                let col = self.grid.cursor().col as usize;
                let cols = self.grid.cols() as usize;
                let end = (col + n).min(cols);
                if let Some(r) = self.grid.row_mut(row) {
                    for c in r[col..end].iter_mut() {
                        *c = Cell::blank();
                    }
                }
                self.grid.mark_dirty(row);
            }
            // カーソル位置保存 (CSI s)
            's' if _intermediates.is_empty() => self.grid.save_cursor(),
            // カーソル位置復元 (CSI u)
            'u' if _intermediates.is_empty() => self.grid.restore_cursor(),
            // SGR — グラフィック属性（色・太字等）
            'm' => self.apply_sgr(&p),
            // DEC 私用モード (? プレフィクス): h=on, l=off
            'h' | 'l' if _intermediates.first().copied() == Some(b'?') => {
                let enable = action == 'h';
                for &param in &p {
                    match param {
                        1 => self.grid.set_application_cursor_keys(enable), // DECCKM
                        7 => self.grid.set_auto_wrap(enable),               // DECAWM
                        25 => self.grid.set_cursor_visible(enable),         // カーソル表示
                        1049 => {
                            // オルタネートスクリーン
                            if enable {
                                self.grid.enter_alternate_screen();
                            } else {
                                self.grid.leave_alternate_screen();
                            }
                        }
                        2004 => self.grid.set_bracketed_paste(enable), // ブラケットペースト
                        // マウス報告モード
                        1000 => self.grid.set_mouse_reporting(if enable { 1 } else { 0 }),
                        1002 => self.grid.set_mouse_reporting(if enable { 2 } else { 0 }),
                        1003 => self.grid.set_mouse_reporting(if enable { 3 } else { 0 }),
                        1006 => self.grid.set_mouse_sgr(enable), // SGR 拡張
                        1015 => self.grid.set_mouse_sgr(enable), // URXVT (SGR と同等扱い)
                        // フォーカスイベント
                        1004 => self.grid.set_focus_events(enable),
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }

    fn osc_dispatch(&mut self, params: &[&[u8]], _bell_terminated: bool) {
        if params.is_empty() {
            return;
        }
        let cmd = std::str::from_utf8(params[0]).unwrap_or("");
        match cmd {
            // OSC 0 / 2: ウィンドウタイトル
            "0" | "2" => {
                if let Some(title_bytes) = params.get(1) {
                    if let Ok(title) = std::str::from_utf8(title_bytes) {
                        self.title = Some(title.to_string());
                    }
                }
            }
            // OSC 9: Growl スタイル通知（iTerm2 互換）
            "9" => {
                if let Some(body) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(body) {
                        self.notification = Some(s.to_string());
                    }
                }
            }
            // OSC 99 / 777: デスクトップ通知
            "99" | "777" => {
                if let Some(body) = params.get(1) {
                    if let Ok(s) = std::str::from_utf8(body) {
                        self.notification = Some(s.to_string());
                    }
                }
            }
            // OSC 133: シェルインテグレーション（FinalTerm 互換）
            // D[;exit_code] — コマンド終了
            "133" => {
                let subcode = params
                    .get(1)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                if subcode == "D" || subcode.starts_with("D;") {
                    self.command_finished = true;
                }
            }
            // OSC 52: クリップボード書き込み
            // 形式: \x1b]52;<kind>;<base64data>\x07
            // kind が "c"（クリップボード）のときのみ処理する
            "52" => {
                // params[1] = kind ("c", "p", "q", "s" 等)
                // params[2] = base64 エンコードされたデータ
                let kind = params
                    .get(1)
                    .and_then(|b| std::str::from_utf8(b).ok())
                    .unwrap_or("");
                if kind == "c" {
                    let b64 = params
                        .get(2)
                        .and_then(|b| std::str::from_utf8(b).ok())
                        .unwrap_or("");
                    // 不正な base64 は無視（panic しない）
                    if let Ok(decoded) = BASE64_STANDARD.decode(b64) {
                        self.clipboard_data = Some(decoded);
                    }
                }
            }
            _ => {}
        }
    }

    fn hook(&mut self, _params: &Params, _intermediates: &[u8], _ignore: bool, _action: char) {}
    fn put(&mut self, _byte: u8) {}
    fn unhook(&mut self) {}
    fn esc_dispatch(&mut self, _intermediates: &[u8], _ignore: bool, byte: u8) {
        match byte {
            b'7' => self.grid.save_cursor(),    // DECSC: カーソル保存
            b'8' => self.grid.restore_cursor(), // DECRC: カーソル復元
            _ => {}
        }
    }
}

impl<'a> VtProcessor<'a> {
    fn apply_sgr(&mut self, params: &[u16]) {
        let mut i = 0;
        while i < params.len() {
            match params[i] {
                // ── リセット ────────────────────────────────────────────
                0 => self.current_style = CellStyle::default(),
                // ── 属性オン ────────────────────────────────────────────
                1 => self.current_style.bold = true,
                3 => self.current_style.italic = true,
                4 => self.current_style.underline = true,
                5 => self.current_style.blink = true,
                7 => self.current_style.reverse = true,
                9 => self.current_style.strikethrough = true,
                // ── 属性オフ ────────────────────────────────────────────
                22 => self.current_style.bold = false,
                23 => self.current_style.italic = false,
                24 => self.current_style.underline = false,
                25 => self.current_style.blink = false,
                27 => self.current_style.reverse = false,
                29 => self.current_style.strikethrough = false,
                // ── ANSI 基本色 fg (30-37) ───────────────────────────────
                30..=37 => self.current_style.fg = Some(ansi16(params[i] as u8 - 30)),
                39 => self.current_style.fg = None, // デフォルト fg
                // ── ANSI 基本色 bg (40-47) ───────────────────────────────
                40..=47 => self.current_style.bg = Some(ansi16(params[i] as u8 - 40)),
                49 => self.current_style.bg = None, // デフォルト bg
                // ── 明るい fg (90-97) ────────────────────────────────────
                90..=97 => self.current_style.fg = Some(ansi16(params[i] as u8 - 90 + 8)),
                // ── 明るい bg (100-107) ──────────────────────────────────
                100..=107 => self.current_style.bg = Some(ansi16(params[i] as u8 - 100 + 8)),
                // ── 拡張色 fg: 38;5;N (256色) / 38;2;R;G;B (RGB) ────────
                38 => {
                    if let Some(c) = parse_extended_color(params, &mut i) {
                        self.current_style.fg = Some(c);
                    }
                }
                // ── 拡張色 bg: 48;5;N (256色) / 48;2;R;G;B (RGB) ────────
                48 => {
                    if let Some(c) = parse_extended_color(params, &mut i) {
                        self.current_style.bg = Some(c);
                    }
                }
                _ => {}
            }
            i += 1;
        }
    }
}

/// ANSI 16色パレット（xterm 標準）
fn ansi16(index: u8) -> Color {
    const P: [(u8, u8, u8); 16] = [
        (0, 0, 0),       // 0  Black
        (128, 0, 0),     // 1  Red
        (0, 128, 0),     // 2  Green
        (128, 128, 0),   // 3  Yellow
        (0, 0, 128),     // 4  Blue
        (128, 0, 128),   // 5  Magenta
        (0, 128, 128),   // 6  Cyan
        (192, 192, 192), // 7  White
        (128, 128, 128), // 8  Bright Black (Gray)
        (255, 0, 0),     // 9  Bright Red
        (0, 255, 0),     // 10 Bright Green
        (255, 255, 0),   // 11 Bright Yellow
        (0, 0, 255),     // 12 Bright Blue
        (255, 0, 255),   // 13 Bright Magenta
        (0, 255, 255),   // 14 Bright Cyan
        (255, 255, 255), // 15 Bright White
    ];
    let (r, g, b) = P[index as usize % 16];
    Color { r, g, b }
}

/// xterm 256色パレット
fn color256(n: u8) -> Color {
    if n < 16 {
        ansi16(n)
    } else if n < 232 {
        // 6×6×6 カラーキューブ
        let i = n - 16;
        let cube = |v: u8| if v == 0 { 0u8 } else { 55 + 40 * v };
        Color {
            r: cube((i / 36) % 6),
            g: cube((i / 6) % 6),
            b: cube(i % 6),
        }
    } else {
        // グレースケール
        let v = 8 + 10 * (n - 232);
        Color { r: v, g: v, b: v }
    }
}

/// `38` / `48` に続く拡張色パラメータを解析してインデックスを進める
fn parse_extended_color(params: &[u16], i: &mut usize) -> Option<Color> {
    match params.get(*i + 1).copied() {
        Some(5) => {
            // ;5;N — 256色
            let n = params.get(*i + 2).copied()? as u8;
            *i += 2;
            Some(color256(n))
        }
        Some(2) => {
            // ;2;R;G;B — RGB
            let r = params.get(*i + 2).copied()? as u8;
            let g = params.get(*i + 3).copied()? as u8;
            let b = params.get(*i + 4).copied()? as u8;
            *i += 4;
            Some(Color { r, g, b })
        }
        _ => None,
    }
}

/// バイト列を VT パースして Grid に適用
pub fn feed_bytes(parser: &mut Parser, processor: &mut VtProcessor<'_>, data: &[u8]) {
    for &byte in data {
        parser.advance(processor, byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::CellContent;
    use crate::grid::Grid;
    use crate::width::CjkWidthConfig;
    use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
    use base64::Engine as _;

    fn make_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    /// グリッドに VT バイト列を適用するヘルパー
    fn feed(g: &mut Grid, data: &[u8]) {
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(g);
        feed_bytes(&mut parser, &mut proc, data);
    }

    // B-1: CUU — カーソル上移動
    #[test]
    fn test_vt_cursor_up() {
        let mut g = make_grid(80, 24);
        g.move_cursor(0, 5);
        feed(&mut g, b"\x1b[3A"); // CUU 3
        assert_eq!(g.cursor().row, 2);
    }

    // B-1: CUD — カーソル下移動
    #[test]
    fn test_vt_cursor_down() {
        let mut g = make_grid(80, 24);
        g.move_cursor(0, 2);
        feed(&mut g, b"\x1b[3B"); // CUD 3
        assert_eq!(g.cursor().row, 5);
    }

    // B-1: CUF — カーソル右移動
    #[test]
    fn test_vt_cursor_forward() {
        let mut g = make_grid(80, 24);
        g.move_cursor(0, 0);
        feed(&mut g, b"\x1b[5C"); // CUF 5
        assert_eq!(g.cursor().col, 5);
    }

    // B-1: CUB — カーソル左移動
    #[test]
    fn test_vt_cursor_backward() {
        let mut g = make_grid(80, 24);
        g.move_cursor(10, 0);
        feed(&mut g, b"\x1b[3D"); // CUB 3
        assert_eq!(g.cursor().col, 7);
    }

    // B-1: CUP — カーソル絶対位置指定（1-indexed）
    #[test]
    fn test_vt_cursor_position() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[5;10H"); // row=5, col=10 (1-indexed)
        assert_eq!(g.cursor().row, 4); // 0-indexed
        assert_eq!(g.cursor().col, 9);
    }

    // B-1: CUP パラメータなし = (1,1) = (0,0) へ移動
    #[test]
    fn test_vt_cursor_position_default() {
        let mut g = make_grid(80, 24);
        g.move_cursor(10, 10);
        feed(&mut g, b"\x1b[H");
        assert_eq!(g.cursor().row, 0);
        assert_eq!(g.cursor().col, 0);
    }

    // B-1: カーソルがグリッド範囲を超えてもクランプされる
    #[test]
    fn test_vt_cursor_up_clamps_at_zero() {
        let mut g = make_grid(80, 24);
        g.move_cursor(0, 2);
        feed(&mut g, b"\x1b[10A"); // 2 行目から 10 上は 0 にクランプ
        assert_eq!(g.cursor().row, 0);
    }

    // B-5: EL 0 — カーソルから行末を消去
    #[test]
    fn test_vt_erase_line_right() {
        let mut g = make_grid(10, 5);
        feed(&mut g, b"ABCDE"); // col 0-4 に書き込み
        g.move_cursor(2, 0);
        feed(&mut g, b"\x1b[K"); // EL 0
                                 // col 0-1 (A, B) は残る
        assert!(matches!(
            g.row(0).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
        assert!(matches!(
            g.row(0).unwrap()[1].content,
            CellContent::Grapheme { .. }
        ));
        // col 2-4 は Blank
        assert_eq!(g.row(0).unwrap()[2].content, CellContent::Blank);
        assert_eq!(g.row(0).unwrap()[3].content, CellContent::Blank);
    }

    // B-5: ED 2 — 全画面消去
    #[test]
    fn test_vt_erase_display_full() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"Hello");
        feed(&mut g, b"\x1b[2J"); // ED 2
        for col in 0..10usize {
            assert_eq!(g.row(0).unwrap()[col].content, CellContent::Blank);
        }
    }

    // B-5: ED 0 — カーソル以降を消去
    #[test]
    fn test_vt_erase_display_below() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"AAAAA"); // row 0
        g.move_cursor(0, 1);
        feed(&mut g, b"BBBBB"); // row 1
        g.move_cursor(0, 1);
        feed(&mut g, b"\x1b[J"); // ED 0 (カーソルから下)
                                 // row 0 は残る
        assert!(matches!(
            g.row(0).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
        // row 1 は消える
        assert_eq!(g.row(1).unwrap()[0].content, CellContent::Blank);
    }

    // B-6: SGR 1 — bold フラグが立つ
    #[test]
    fn test_vt_sgr_bold() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[1mA");
        assert!(g.row(0).unwrap()[0].style.bold);
    }

    // B-6: SGR 7 — reverse フラグが立つ
    #[test]
    fn test_vt_sgr_reverse() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[7mA");
        assert!(g.row(0).unwrap()[0].style.reverse);
    }

    // B-6: SGR 4 — underline フラグが立つ
    #[test]
    fn test_vt_sgr_underline() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[4mA");
        assert!(g.row(0).unwrap()[0].style.underline);
    }

    // B-6: SGR 0 — 全属性リセット
    #[test]
    fn test_vt_sgr_reset() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[1m"); // bold on
        feed(&mut g, b"\x1b[0m"); // reset
        feed(&mut g, b"A");
        assert!(!g.row(0).unwrap()[0].style.bold);
    }

    // B-6: 複数 SGR パラメータを一度に指定できる
    #[test]
    fn test_vt_sgr_multiple_params() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[1;4mA"); // bold + underline
        let cell = &g.row(0).unwrap()[0];
        assert!(cell.style.bold);
        assert!(cell.style.underline);
    }

    // B-6: SGR 9 — strikethrough フラグが立つ
    #[test]
    fn test_vt_sgr_strikethrough() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[9mA");
        assert!(g.row(0).unwrap()[0].style.strikethrough);
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

    // OSC 2: ウィンドウタイトルが取得できる
    #[test]
    fn test_vt_osc_title() {
        let mut g = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(&mut g);
        feed_bytes(&mut parser, &mut proc, b"\x1b]2;My Terminal Title\x07");
        assert_eq!(proc.title.as_deref(), Some("My Terminal Title"));
    }

    // OSC 9: 通知本文が取得できる
    #[test]
    fn test_vt_osc_notification_9() {
        let mut g = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(&mut g);
        feed_bytes(&mut parser, &mut proc, b"\x1b]9;Build complete\x07");
        assert_eq!(proc.notification.as_deref(), Some("Build complete"));
    }

    // D-2: \x1b[?7l で DECAWM オフ → 折り返しなし
    #[test]
    fn test_vt_decawm_off_no_wrap() {
        let mut g = make_grid(5, 3);
        feed(&mut g, b"\x1b[?7l"); // DECAWM off
        for _ in 0..10 {
            feed(&mut g, b"X");
        }
        assert_eq!(g.cursor().row, 0, "DECAWM off: row 0 のまま");
    }

    // D-2: \x1b[?7h で DECAWM オン → 折り返し復帰
    #[test]
    fn test_vt_decawm_on_restores_wrap() {
        let mut g = make_grid(5, 3);
        feed(&mut g, b"\x1b[?7l"); // off
        feed(&mut g, b"\x1b[?7h"); // on
        for _ in 0..6 {
            feed(&mut g, b"X");
        }
        assert_eq!(g.cursor().row, 1, "DECAWM on 復帰後: row 1 に折り返す");
    }

    // OSC 777: 通知本文が取得できる
    #[test]
    fn test_vt_osc_notification_777() {
        let mut g = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(&mut g);
        feed_bytes(&mut parser, &mut proc, b"\x1b]777;notify;Test\x07");
        assert_eq!(proc.notification.as_deref(), Some("notify"));
    }

    // T-01: ?1049h でオルタネートスクリーンに切り替わる
    #[test]
    fn test_vt_enter_alternate_screen() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"Hello"); // メイン画面に書き込み
        feed(&mut g, b"\x1b[?1049h"); // alternate に切り替え
        assert!(g.is_alternate_screen());
        // オルタネート画面はクリアされている
        assert_eq!(g.row(0).unwrap()[0].content, CellContent::Blank);
        assert_eq!(g.cursor().col, 0);
        assert_eq!(g.cursor().row, 0);
    }

    // T-01: ?1049l でメインスクリーンが復元される
    #[test]
    fn test_vt_leave_alternate_screen_restores_main() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"Hello"); // メイン画面に書き込み
        feed(&mut g, b"\x1b[?1049h"); // alternate
        feed(&mut g, b"Alt content");
        feed(&mut g, b"\x1b[?1049l"); // main に戻る
        assert!(!g.is_alternate_screen());
        // メイン画面の "Hello" が復元されている
        assert!(matches!(
            g.row(0).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
    }

    // T-02: EL 1 — 行頭からカーソルまで消去
    #[test]
    fn test_vt_erase_line_left() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"ABCDE");
        g.move_cursor(2, 0); // col 2 にカーソル
        feed(&mut g, b"\x1b[1K"); // EL 1
                                  // col 0,1,2 が Blank に
        assert_eq!(g.row(0).unwrap()[0].content, CellContent::Blank);
        assert_eq!(g.row(0).unwrap()[1].content, CellContent::Blank);
        assert_eq!(g.row(0).unwrap()[2].content, CellContent::Blank);
        // col 3,4 は残る
        assert!(matches!(
            g.row(0).unwrap()[3].content,
            CellContent::Grapheme { .. }
        ));
    }

    // T-02: EL 2 — 行全体を消去
    #[test]
    fn test_vt_erase_line_all() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"ABCDE");
        g.move_cursor(2, 0);
        feed(&mut g, b"\x1b[2K"); // EL 2
        for col in 0..5usize {
            assert_eq!(g.row(0).unwrap()[col].content, CellContent::Blank);
        }
    }

    // T-03: ED 1 — 画面先頭からカーソルまで消去
    #[test]
    fn test_vt_erase_display_above() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"Row0\r\nRow1\r\nRow2");
        g.move_cursor(2, 1); // row 1, col 2 にカーソル
        feed(&mut g, b"\x1b[1J"); // ED 1
                                  // row 0 は全消え
        assert_eq!(g.row(0).unwrap()[0].content, CellContent::Blank);
        // row 1 の col 0-2 も消える
        assert_eq!(g.row(1).unwrap()[0].content, CellContent::Blank);
        assert_eq!(g.row(1).unwrap()[2].content, CellContent::Blank);
        // row 2 は残る
        assert!(matches!(
            g.row(2).unwrap()[0].content,
            CellContent::Grapheme { .. }
        ));
    }

    // T-03: ED 2 — カーソル位置を変えずに全画面消去
    #[test]
    fn test_vt_erase_display_all_keeps_cursor() {
        let mut g = make_grid(10, 3);
        feed(&mut g, b"Hello");
        g.move_cursor(5, 1);
        feed(&mut g, b"\x1b[2J"); // ED 2
        assert_eq!(g.cursor().col, 5, "ED 2 はカーソルを動かさない");
        assert_eq!(g.cursor().row, 1);
        assert_eq!(g.row(0).unwrap()[0].content, CellContent::Blank);
    }

    // T-04: ESC 7 / ESC 8 でカーソル保存・復元
    #[test]
    fn test_vt_cursor_save_restore_esc() {
        let mut g = make_grid(80, 24);
        g.move_cursor(10, 5);
        feed(&mut g, b"\x1b7"); // DECSC: 保存
        g.move_cursor(0, 0);
        feed(&mut g, b"\x1b8"); // DECRC: 復元
        assert_eq!(g.cursor().col, 10);
        assert_eq!(g.cursor().row, 5);
    }

    // T-04: CSI s / CSI u でカーソル保存・復元
    #[test]
    fn test_vt_cursor_save_restore_csi() {
        let mut g = make_grid(80, 24);
        g.move_cursor(7, 3);
        feed(&mut g, b"\x1b[s"); // 保存
        g.move_cursor(0, 0);
        feed(&mut g, b"\x1b[u"); // 復元
        assert_eq!(g.cursor().col, 7);
        assert_eq!(g.cursor().row, 3);
    }

    // T-05: ?25l でカーソル非表示、?25h で表示
    #[test]
    fn test_vt_cursor_visibility() {
        let mut g = make_grid(80, 24);
        assert!(g.cursor_visible(), "初期値は表示");
        feed(&mut g, b"\x1b[?25l");
        assert!(!g.cursor_visible(), "?25l で非表示");
        feed(&mut g, b"\x1b[?25h");
        assert!(g.cursor_visible(), "?25h で表示復帰");
    }

    // ── T-06: SGR 基本色 ──────────────────────────────────────────────────

    // T-06: SGR 31 — 前景色 Red (ANSI 1番)
    #[test]
    fn test_sgr_fg_red() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[31mA");
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 128, g: 0, b: 0 }));
    }

    // T-06: SGR 42 — 背景色 Green (ANSI 2番)
    #[test]
    fn test_sgr_bg_green() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[42mA");
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(style.bg, Some(Color { r: 0, g: 128, b: 0 }));
    }

    // T-06: SGR 39/49 — fg/bg をデフォルトにリセット
    #[test]
    fn test_sgr_fg_bg_reset_to_default() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[31;42m"); // fg=Red, bg=Green
        feed(&mut g, b"\x1b[39;49mA"); // fg/bg デフォルト
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(style.fg, None);
        assert_eq!(style.bg, None);
    }

    // T-06: SGR 90 — 明るい前景色 Bright Red (ANSI 9番)
    #[test]
    fn test_sgr_bright_fg() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[91mA"); // Bright Red
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 255, g: 0, b: 0 }));
    }

    // T-06: SGR 100 — 明るい背景色 Bright Black (ANSI 8番)
    #[test]
    fn test_sgr_bright_bg() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[100mA"); // Bright Black bg
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(
            style.bg,
            Some(Color {
                r: 128,
                g: 128,
                b: 128
            })
        );
    }

    // ── T-07: SGR 256色 ───────────────────────────────────────────────────

    // T-07: 38;5;1 — 前景 256色パレット index 1 (Red)
    #[test]
    fn test_sgr_256_fg() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[38;5;1mA");
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 128, g: 0, b: 0 }));
    }

    // T-07: 48;5;2 — 背景 256色パレット index 2 (Green)
    #[test]
    fn test_sgr_256_bg() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[48;5;2mA");
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(style.bg, Some(Color { r: 0, g: 128, b: 0 }));
    }

    // T-07: 256色キューブ index 16 (最初の非 ANSI 色) = (0,0,0)
    #[test]
    fn test_sgr_256_cube_first() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[38;5;16mA");
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 0, g: 0, b: 0 }));
    }

    // T-07: 256色グレースケール index 232 = (8,8,8)
    #[test]
    fn test_sgr_256_grayscale() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[38;5;232mA");
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(style.fg, Some(Color { r: 8, g: 8, b: 8 }));
    }

    // ── T-08: SGR RGB色 ───────────────────────────────────────────────────

    // T-08: 38;2;255;128;0 — 前景 RGB オレンジ
    #[test]
    fn test_sgr_rgb_fg() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[38;2;255;128;0mA");
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(
            style.fg,
            Some(Color {
                r: 255,
                g: 128,
                b: 0
            })
        );
    }

    // T-08: 48;2;0;0;255 — 背景 RGB 青
    #[test]
    fn test_sgr_rgb_bg() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[48;2;0;0;255mA");
        let style = g.row(0).unwrap()[0].style;
        assert_eq!(style.bg, Some(Color { r: 0, g: 0, b: 255 }));
    }

    // ── T-09: SGR 属性オフ ────────────────────────────────────────────────

    // T-09: SGR 23 — italic オフ
    #[test]
    fn test_sgr_italic_off() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[3m"); // italic on
        feed(&mut g, b"\x1b[23mA"); // italic off
        assert!(!g.row(0).unwrap()[0].style.italic);
    }

    // T-09: SGR 24 — underline オフ
    #[test]
    fn test_sgr_underline_off() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[4m\x1b[24mA");
        assert!(!g.row(0).unwrap()[0].style.underline);
    }

    // T-09: SGR 27 — reverse オフ
    #[test]
    fn test_sgr_reverse_off() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[7m\x1b[27mA");
        assert!(!g.row(0).unwrap()[0].style.reverse);
    }

    // ── T-11: DECCKM カーソルキーモード ──────────────────────────────────

    // T-11: ?1h でアプリケーションモード有効
    #[test]
    fn test_decckm_enable() {
        let mut g = make_grid(80, 24);
        assert!(!g.application_cursor_keys(), "初期値は通常モード");
        feed(&mut g, b"\x1b[?1h");
        assert!(g.application_cursor_keys(), "?1h でアプリケーションモード");
    }

    // T-11: ?1l で通常モードに戻る
    #[test]
    fn test_decckm_disable() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[?1h");
        feed(&mut g, b"\x1b[?1l");
        assert!(!g.application_cursor_keys(), "?1l で通常モードに戻る");
    }

    // ── T-12: ブラケットペーストモード ───────────────────────────────────

    // T-12: ?2004h でブラケットペーストモード有効
    #[test]
    fn test_bracketed_paste_enable() {
        let mut g = make_grid(80, 24);
        assert!(!g.bracketed_paste(), "初期値は無効");
        feed(&mut g, b"\x1b[?2004h");
        assert!(g.bracketed_paste(), "?2004h で有効");
    }

    // T-12: ?2004l でブラケットペーストモード無効
    #[test]
    fn test_bracketed_paste_disable() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[?2004h");
        feed(&mut g, b"\x1b[?2004l");
        assert!(!g.bracketed_paste(), "?2004l で無効に戻る");
    }

    // ── DECSTBM: スクロール領域 ──────────────────────────────────────────

    // DECSTBM: CSI r でスクロール領域が設定される
    #[test]
    fn test_decstbm_sets_region() {
        let mut g = make_grid(80, 24);
        // 上 1–22 行をスクロール領域に設定（最終 2 行をステータスバーに残す）
        feed(&mut g, b"\x1b[1;22r");
        assert_eq!(g.scroll_top(), 0, "scroll_top は 0 始まり");
        assert_eq!(g.scroll_bottom(), 21, "scroll_bottom は 0 始まり 21");
    }

    // DECSTBM: 引数なしでフル画面にリセット
    #[test]
    fn test_decstbm_reset() {
        let mut g = make_grid(80, 24);
        feed(&mut g, b"\x1b[1;22r"); // 部分設定
        feed(&mut g, b"\x1b[r"); // リセット
        assert_eq!(g.scroll_top(), 0);
        assert_eq!(g.scroll_bottom(), 23);
    }

    // DECSTBM: スクロール領域外の行はスクロールされない
    #[test]
    fn test_decstbm_scroll_stays_in_region() {
        let mut g = make_grid(10, 5);
        // 行 0–3 をスクロール領域に、行 4 をステータスバーとして残す
        feed(&mut g, b"\x1b[1;4r");

        // ステータスバー行（row=4）に文字を書く
        feed(&mut g, b"\x1b[5;1H"); // カーソルを row=4 へ
        feed(&mut g, b"STATUS");

        // スクロール領域内で LF を発行し続ける → 行 4 は侵食されないはず
        feed(&mut g, b"\x1b[4;1H"); // スクロール領域最下行 (row=3) へ移動
        for _ in 0..8 {
            feed(&mut g, b"\n");
        }

        // row=4 は "STATUS" のまま
        let row4 = g.row(4).unwrap();
        assert!(
            matches!(row4[0].content, CellContent::Grapheme { ref text, .. } if text == "S"),
            "ステータスバー行はスクロールで上書きされない"
        );
    }

    // DECSTBM: SU (CSI S) でスクロール領域内のみスクロール
    #[test]
    fn test_su_scrolls_region() {
        let mut g = make_grid(10, 5);
        // 1–4 行目（0–3）をスクロール領域に設定
        feed(&mut g, b"\x1b[1;4r");
        // スクロール領域内の先頭行に文字を書く
        feed(&mut g, b"\x1b[1;1H");
        feed(&mut g, b"FIRST");
        // SU 1: 1 行上スクロール → FIRST が消える
        feed(&mut g, b"\x1b[1S");
        let row0 = g.row(0).unwrap();
        assert!(
            matches!(row0[0].content, CellContent::Blank),
            "SU でスクロール後、先頭行は空白になる"
        );
    }

    // ── OSC 52 クリップボード ─────────────────────────────────────────────

    fn osc52_proc(data: &[u8]) -> Option<Vec<u8>> {
        let mut g = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(&mut g);
        feed_bytes(&mut parser, &mut proc, data);
        proc.clipboard_data
    }

    // TC-01: ASCII 文字列を BEL 終端でコピー
    #[test]
    fn test_osc52_ascii_bel() {
        // "hello" の base64 = "aGVsbG8="
        let result = osc52_proc(b"\x1b]52;c;aGVsbG8=\x07");
        assert_eq!(result, Some(b"hello".to_vec()));
    }

    // TC-02: 日本語 UTF-8 のコピー
    #[test]
    fn test_osc52_utf8_japanese() {
        // "こんにちは" の base64
        use std::io::Write;
        let text = "こんにちは".as_bytes();
        let b64 = BASE64_STANDARD.encode(text);
        let seq = format!("\x1b]52;c;{}\x07", b64);
        let result = osc52_proc(seq.as_bytes());
        assert_eq!(result, Some(text.to_vec()));
    }

    // TC-03: ST 終端（\x1b\x5c）のサポート
    #[test]
    fn test_osc52_st_terminator() {
        let result = osc52_proc(b"\x1b]52;c;aGVsbG8=\x1b\\");
        assert_eq!(result, Some(b"hello".to_vec()));
    }

    // TC-04: 空データでクリップボードをクリア
    #[test]
    fn test_osc52_empty_data() {
        let result = osc52_proc(b"\x1b]52;c;\x07");
        assert_eq!(result, Some(b"".to_vec()));
    }

    // TC-05: `c` 以外のクリップボード種別は無視
    #[test]
    fn test_osc52_non_clipboard_type_ignored() {
        let result = osc52_proc(b"\x1b]52;p;aGVsbG8=\x07");
        assert_eq!(result, None);
    }

    // TC-06: 不正な base64 は無視（panic しない）
    #[test]
    fn test_osc52_invalid_base64() {
        let result = osc52_proc(b"\x1b]52;c;!!!invalid!!!\x07");
        assert_eq!(result, None);
    }

    // TC-07: 複数回の OSC 52 で最新値に上書き
    #[test]
    fn test_osc52_overwrite() {
        // "first" = "Zmlyc3Q=", "second" = "c2Vjb25k"
        let mut g = make_grid(80, 24);
        let mut parser = Parser::new();
        let mut proc = VtProcessor::new(&mut g);
        feed_bytes(&mut parser, &mut proc, b"\x1b]52;c;Zmlyc3Q=\x07");
        feed_bytes(&mut parser, &mut proc, b"\x1b]52;c;c2Vjb25k\x07");
        assert_eq!(proc.clipboard_data, Some(b"second".to_vec()));
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
}
