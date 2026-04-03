//! GDI 描画操作カウンタ
//!
//! `window/mod.rs` の `paint()` が発行する Win32 GDI 呼び出し回数を
//! Win32 なしで推定する。criterion ベンチマークから呼ばれる。
//!
//! ## カウント対象
//!
//! | フィールド | 対応する GDI 呼び出し |
//! |-----------|----------------------|
//! | `ext_text_out_calls` | `ExtTextOutW`（背景塗り + テキスト描画） |
//! | `set_bk_color_calls` | `SetBkColor`（背景色が前セルから変わったとき） |
//! | `set_text_color_calls` | `SetTextColor`（前景色が前セルから変わったとき） |
//!
//! ## 計数ロジック
//!
//! `paint()` のセルループを忠実に模倣する:
//! - `Blank`     → `ExtTextOutW` × 1（ETO_OPAQUE で背景塗り）
//! - `Grapheme`  → `ExtTextOutW` × 2（背景塗り + テキスト描画）
//! - `Continuation` → 0（スキップ）
//!
//! 色変化は前セルの (fg, bg) と比較してカウントする。
//! `SetBkColor` は Blank/Grapheme どちらでも背景色が変わると発生する。
//! `SetTextColor` は Grapheme のみ（Blank はテキスト描画がないため）。

use yatamux_terminal::cell::{CellContent, Color};
use yatamux_terminal::Grid;

/// デフォルトテーマ色（Catppuccin Mocha — `render.rs` の定数と合わせる）
const DEFAULT_FG: (u8, u8, u8) = (0xCD, 0xD6, 0xF4);
const DEFAULT_BG: (u8, u8, u8) = (0x1E, 0x1E, 0x2E);

/// 1フレームの描画操作カウント
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct DrawOpStats {
    /// `ExtTextOutW` の総呼び出し回数（背景塗り＋テキスト描画の合計）
    pub ext_text_out_calls: u32,
    /// `SetBkColor` 呼び出し回数（背景色が変わった回数）
    pub set_bk_color_calls: u32,
    /// `SetTextColor` 呼び出し回数（前景色が変わった回数）
    pub set_text_color_calls: u32,
    /// 描画した行数
    pub rows_rendered: u32,
    /// 処理したセル数（Continuation 除く）
    pub cells_processed: u32,
}

impl DrawOpStats {
    /// GDI 呼び出し総数（ExtTextOut + SetBkColor + SetTextColor）
    pub fn total_gdi_calls(&self) -> u32 {
        self.ext_text_out_calls + self.set_bk_color_calls + self.set_text_color_calls
    }
}

impl std::fmt::Display for DrawOpStats {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "GDI calls: {} total  (ExtTextOut={}, SetBkColor={}, SetTextColor={})  rows={} cells={}",
            self.total_gdi_calls(),
            self.ext_text_out_calls,
            self.set_bk_color_calls,
            self.set_text_color_calls,
            self.rows_rendered,
            self.cells_processed,
        )
    }
}

fn resolve_color(opt: Option<Color>, default: (u8, u8, u8)) -> (u8, u8, u8) {
    opt.map(|c| (c.r, c.g, c.b)).unwrap_or(default)
}

/// グリッド 1 枚分の描画操作数を推定する（ランバッチング**なし**）。
///
/// `cols` はクリップ幅（`display_cols` 相当）。`None` の場合は `grid.cols()` を使う。
/// `dirty_rows` が `None` のとき全行を描画対象とする（改善前相当）。
/// `Some(set)` のとき set に含まれる行のみ描画する（改善後相当）。
pub fn count_draw_ops(
    grid: &Grid,
    cols: Option<usize>,
    dirty_rows: Option<&std::collections::HashSet<u16>>,
) -> DrawOpStats {
    let display_cols = cols.unwrap_or(grid.cols() as usize);
    let mut stats = DrawOpStats::default();

    // 前セルの色（DC ステート追跡用）。None = まだ SetBkColor/SetTextColor を呼んでいない
    let mut cur_bg: Option<(u8, u8, u8)> = None;
    let mut cur_fg: Option<(u8, u8, u8)> = None;

    for row in 0..grid.rows() {
        if let Some(dr) = dirty_rows {
            if !dr.contains(&row) {
                continue;
            }
        }
        let cells = match grid.row(row) {
            Some(c) => c,
            None => continue,
        };
        stats.rows_rendered += 1;

        for cell in cells.iter().take(display_cols) {
            match &cell.content {
                CellContent::Continuation => {
                    // x += cell_width のみ、GDI 呼び出しなし
                }
                CellContent::Blank => {
                    stats.cells_processed += 1;
                    let bg = resolve_color(cell.style.bg, DEFAULT_BG);

                    if cur_bg != Some(bg) {
                        stats.set_bk_color_calls += 1;
                        cur_bg = Some(bg);
                    }
                    // ETO_OPAQUE で背景塗り 1 回
                    stats.ext_text_out_calls += 1;
                }
                CellContent::Grapheme { .. } => {
                    stats.cells_processed += 1;

                    let (raw_fg, raw_bg) = {
                        let fg = resolve_color(cell.style.fg, DEFAULT_FG);
                        let bg = resolve_color(cell.style.bg, DEFAULT_BG);
                        if cell.style.reverse { (bg, fg) } else { (fg, bg) }
                    };

                    // 背景色変化 → SetBkColor
                    if cur_bg != Some(raw_bg) {
                        stats.set_bk_color_calls += 1;
                        cur_bg = Some(raw_bg);
                    }
                    // ETO_OPAQUE で背景塗り（wide_rect）
                    stats.ext_text_out_calls += 1;

                    // 前景色変化 → SetTextColor
                    if cur_fg != Some(raw_fg) {
                        stats.set_text_color_calls += 1;
                        cur_fg = Some(raw_fg);
                    }
                    // ETO_CLIPPED でテキスト描画
                    stats.ext_text_out_calls += 1;
                }
            }
        }
    }

    stats
}

/// グリッド 1 枚分の描画操作数を推定する（ランバッチング**あり**、B 案）。
///
/// 同一 (fg, bg) の連続セルをまとめて 1 スパンとして処理する。
/// - Blank セルは bg 一致で延長（テキスト描画なし）
/// - Grapheme セルは (fg, bg) 一致・ASCII 1 コードユニットのみでラン延長
/// - ランごとに ExtTextOutW × 1（背景）+ 最大 × 1（テキスト）
///
/// ボックス文字・サロゲートペア・ZWJ・選択セルはランを分断して個別描画（各 2 コール）。
pub fn count_draw_ops_batched(
    grid: &Grid,
    cols: Option<usize>,
    dirty_rows: Option<&std::collections::HashSet<u16>>,
) -> DrawOpStats {
    let display_cols = cols.unwrap_or(grid.cols() as usize);
    let mut stats = DrawOpStats::default();

    // ラン状態
    #[derive(Default, Clone, Copy, PartialEq)]
    enum RunKind { #[default] None, Blank, Text }

    struct Run {
        kind: RunKind,
        bg: (u8, u8, u8),
        fg: (u8, u8, u8),
        has_text: bool,
    }
    let mut run: Option<Run> = None;

    // ランフラッシュ: ExtTextOutW × 1（背景）+ 条件付き × 1（テキスト）
    // ※ SetBkColor / SetTextColor はランごとに各 1 回としてカウント（worst case 近似）
    let flush = |run: &mut Option<Run>, stats: &mut DrawOpStats| {
        if let Some(r) = run.take() {
            stats.set_bk_color_calls += 1;
            stats.ext_text_out_calls += 1; // 背景 ETO_OPAQUE
            if r.has_text {
                stats.set_text_color_calls += 1;
                stats.ext_text_out_calls += 1; // テキスト ETO_CLIPPED
            }
        }
    };

    for row in 0..grid.rows() {
        if let Some(dr) = dirty_rows {
            if !dr.contains(&row) {
                continue;
            }
        }
        let cells = match grid.row(row) {
            Some(c) => c,
            None => continue,
        };
        stats.rows_rendered += 1;

        for cell in cells.iter().take(display_cols) {
            match &cell.content {
                CellContent::Continuation => {}
                CellContent::Blank => {
                    stats.cells_processed += 1;
                    let bg = resolve_color(cell.style.bg, DEFAULT_BG);
                    if let Some(ref r) = run {
                        if r.kind == RunKind::Blank && r.bg == bg {
                            // 延長（フラッシュなし）
                            continue;
                        }
                    }
                    flush(&mut run, &mut stats);
                    run = Some(Run { kind: RunKind::Blank, bg, fg: DEFAULT_FG, has_text: false });
                }
                CellContent::Grapheme { text, .. } => {
                    stats.cells_processed += 1;
                    let (raw_fg, raw_bg) = {
                        let fg = resolve_color(cell.style.fg, DEFAULT_FG);
                        let bg = resolve_color(cell.style.bg, DEFAULT_BG);
                        if cell.style.reverse { (bg, fg) } else { (fg, bg) }
                    };

                    let first_cp = text.chars().next().map(|c| c as u32).unwrap_or(0);
                    let is_box = (0x2500..=0x259F).contains(&first_cp);
                    let utf16_len = text.encode_utf16().count();

                    if is_box || utf16_len != 1 {
                        // 個別描画（ランを分断）
                        flush(&mut run, &mut stats);
                        // 背景 + テキスト各 1 回
                        stats.set_bk_color_calls += 1;
                        stats.ext_text_out_calls += 1;
                        if !is_box {
                            stats.set_text_color_calls += 1;
                            stats.ext_text_out_calls += 1;
                        }
                    } else {
                        // ランに追加できるか
                        let can_extend = run.as_ref().is_some_and(|r| {
                            r.kind == RunKind::Text && r.fg == raw_fg && r.bg == raw_bg
                        });
                        if !can_extend {
                            flush(&mut run, &mut stats);
                            run = Some(Run {
                                kind: RunKind::Text,
                                bg: raw_bg,
                                fg: raw_fg,
                                has_text: true,
                            });
                        }
                        // 延長（bg スパンにテキストを追加するだけ）
                    }
                }
            }
        }
        // 行末でランをフラッシュ
        flush(&mut run, &mut stats);
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use yatamux_terminal::cell::CellStyle;
    use yatamux_terminal::CjkWidthConfig;

    fn ascii_style(fg: Color, bg: Color) -> CellStyle {
        CellStyle { fg: Some(fg), bg: Some(bg), ..Default::default() }
    }

    fn make_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    /// 各シナリオの DrawOpStats をビフォーアフターで表示する。
    /// `cargo test -p yatamux-renderer -- print_op_counts --nocapture`
    #[test]
    fn print_op_counts() {
        use super::count_draw_ops_batched;

        let fg = Color { r: 0xCD, g: 0xD6, b: 0xF4 };
        let bg = Color { r: 0x1E, g: 0x1E, b: 0x2E };
        let style = ascii_style(fg, bg);

        macro_rules! show {
            ($label:expr, $g:expr, $dr:expr) => {{
                let b = count_draw_ops(&$g, None, $dr);
                let a = count_draw_ops_batched(&$g, None, $dr);
                let save = if b.total_gdi_calls() > 0 {
                    let ratio = a.total_gdi_calls() * 100 / b.total_gdi_calls();
                    100i64 - ratio as i64
                } else { 0 };
                println!("[{:30}] before={:5}  after={:5}  {:+}%", $label, b.total_gdi_calls(), a.total_gdi_calls(), -save);
            }};
        }

        // S-1: idle_prompt（全行 vs カーソル 1 行のみ dirty）
        let mut g = make_grid(80, 24);
        for ch in "$ ".chars() { g.write_char(&ch.to_string(), style); }
        let one_row: std::collections::HashSet<u16> = [0].into();
        show!("idle_prompt 80x24 ALL ", g, None);
        show!("idle_prompt 80x24 1row", g, Some(&one_row));

        let mut g = make_grid(200, 50);
        for ch in "$ ".chars() { g.write_char(&ch.to_string(), style); }
        show!("idle_prompt 200x50 ALL", g, None);

        // S-2: dense_ascii (同色全埋め)
        for (cols, rows) in [(80u16, 24u16), (200u16, 50u16)] {
            let mut g = make_grid(cols, rows);
            for _ in 0..rows {
                for c in 0..cols {
                    let ch = (b'a' + (c % 26) as u8) as char;
                    g.write_char(&ch.to_string(), style);
                }
                g.carriage_return();
                g.line_feed();
            }
            show!(format!("dense_ascii {cols}x{rows}"), g, None);
        }

        // S-3: multicolor (セルごとに fg が変化)
        let palette: &[(u8,u8,u8)] = &[
            (0xF3,0x8B,0xA8),(0xA6,0xE3,0xA1),(0xF9,0xE2,0xAF),(0x89,0xB4,0xFA),
        ];
        for (cols, rows) in [(80u16, 24u16), (200u16, 50u16)] {
            let mut g = make_grid(cols, rows);
            for row in 0..rows {
                for col in 0..cols {
                    let (r,gv,b) = palette[((row*cols+col) as usize) % palette.len()];
                    let s = ascii_style(Color{r,g:gv,b}, bg);
                    let ch = (b'a' + (col % 26) as u8) as char;
                    g.write_char(&ch.to_string(), s);
                }
                g.carriage_return();
                g.line_feed();
            }
            show!(format!("multicolor  {cols}x{rows}"), g, None);
        }

        // S-4: vim_style (行ごとに bg が交互)
        let bg2 = Color { r: 0x31, g: 0x32, b: 0x44 };
        for (cols, rows) in [(80u16, 24u16), (200u16, 50u16)] {
            let mut g = make_grid(cols, rows);
            for row in 0..rows {
                let s = ascii_style(fg, if row % 2 == 0 { bg } else { bg2 });
                for col in 0..cols {
                    let ch = (b'a' + (col % 26) as u8) as char;
                    g.write_char(&ch.to_string(), s);
                }
                g.carriage_return();
                g.line_feed();
            }
            show!(format!("vim_style   {cols}x{rows}"), g, None);
        }
    }
}
