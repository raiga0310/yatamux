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

use yatamux_terminal::cell::{Cell, CellContent, CellStyle, Color};
use yatamux_terminal::Grid;

/// デフォルトテーマ色（Catppuccin Mocha — `render.rs` の定数と合わせる）
const DEFAULT_FG: (u8, u8, u8) = (0xCD, 0xD6, 0xF4);
const DEFAULT_BG: (u8, u8, u8) = (0x1E, 0x1E, 0x2E);
const DEFAULT_FG_KEY: u32 = pack_rgb(DEFAULT_FG);
const DEFAULT_BG_KEY: u32 = pack_rgb(DEFAULT_BG);

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

#[inline(always)]
fn resolve_color(opt: Option<Color>, default: (u8, u8, u8)) -> (u8, u8, u8) {
    opt.map(|c| (c.r, c.g, c.b)).unwrap_or(default)
}

#[inline(always)]
const fn pack_rgb(rgb: (u8, u8, u8)) -> u32 {
    ((rgb.0 as u32) << 16) | ((rgb.1 as u32) << 8) | (rgb.2 as u32)
}

#[inline(always)]
const fn pack_color(color: Color) -> u32 {
    ((color.r as u32) << 16) | ((color.g as u32) << 8) | (color.b as u32)
}

#[inline(always)]
fn resolve_color_key(opt: Option<Color>, default_key: u32) -> u32 {
    match opt {
        Some(color) => pack_color(color),
        None => default_key,
    }
}

#[inline(always)]
fn resolve_effective_colors(style: &CellStyle) -> (u32, u32) {
    let fg = resolve_color_key(style.fg, DEFAULT_FG_KEY);
    let bg = resolve_color_key(style.bg, DEFAULT_BG_KEY);
    if style.reverse {
        (bg, fg)
    } else {
        (fg, bg)
    }
}

#[inline(always)]
fn is_single_ascii(text: &str) -> bool {
    let bytes = text.as_bytes();
    bytes.len() == 1 && bytes[0].is_ascii()
}

#[cold]
fn is_box_drawing(text: &str) -> bool {
    let mut utf16 = text.encode_utf16();
    let first_u16 = utf16.next().unwrap_or_default();
    utf16.next().is_none() && (0x2500..=0x259F).contains(&first_u16)
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
                        if cell.style.reverse {
                            (bg, fg)
                        } else {
                            (fg, bg)
                        }
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

fn count_draw_ops_batched_row(cells: &[Cell], display_cols: usize, stats: &mut DrawOpStats) {
    let cells = &cells[..display_cols.min(cells.len())];
    let mut row_runs: u32 = 0;
    let mut row_text_runs: u32 = 0;
    let mut index = 0;

    stats.rows_rendered += 1;

    while index < cells.len() {
        let cell = &cells[index];
        match &cell.content {
            CellContent::Continuation => {
                index += 1;
            }
            CellContent::Blank => {
                stats.cells_processed += 1;
                let mut raw_bg = cell.style.bg;
                let run_bg = resolve_color_key(raw_bg, DEFAULT_BG_KEY);
                index += 1;

                while index < cells.len() {
                    let next = &cells[index];
                    match &next.content {
                        CellContent::Blank => {
                            if next.style.bg != raw_bg
                                && resolve_color_key(next.style.bg, DEFAULT_BG_KEY) != run_bg
                            {
                                break;
                            }
                            raw_bg = next.style.bg;
                            stats.cells_processed += 1;
                            index += 1;
                        }
                        _ => break,
                    }
                }

                row_runs += 1;
            }
            CellContent::Grapheme { text, .. } => {
                stats.cells_processed += 1;

                if is_single_ascii(text) {
                    let mut raw_fg = cell.style.fg;
                    let mut raw_bg = cell.style.bg;
                    let mut raw_reverse = cell.style.reverse;
                    let (run_fg, run_bg) = resolve_effective_colors(&cell.style);
                    index += 1;

                    while index < cells.len() {
                        let next = &cells[index];
                        let next_text = match &next.content {
                            CellContent::Grapheme { text, .. } => text,
                            _ => break,
                        };
                        if !is_single_ascii(next_text) {
                            break;
                        }
                        if next.style.reverse == raw_reverse
                            && next.style.fg == raw_fg
                            && next.style.bg == raw_bg
                        {
                            stats.cells_processed += 1;
                            index += 1;
                            continue;
                        }

                        let (next_fg, next_bg) = resolve_effective_colors(&next.style);
                        if next_fg != run_fg || next_bg != run_bg {
                            break;
                        }

                        raw_fg = next.style.fg;
                        raw_bg = next.style.bg;
                        raw_reverse = next.style.reverse;
                        stats.cells_processed += 1;
                        index += 1;
                    }

                    row_runs += 1;
                    row_text_runs += 1;
                    continue;
                }

                index += 1;
                row_runs += 1;
                if !is_box_drawing(text) {
                    row_text_runs += 1;
                }
            }
        }
    }

    stats.set_bk_color_calls += row_runs;
    stats.set_text_color_calls += row_text_runs;
    stats.ext_text_out_calls += row_runs + row_text_runs;
}

/// グリッド 1 枚分の描画操作数を推定する（ランバッチング**あり**）。
///
/// 同一 (fg, bg) の連続 ASCII セルをまとめて 1 スパンとして処理する。
/// ランは行をまたがない。行末で `flush_run` して `stats` に加算する。
///
/// - ASCII Grapheme（1 byte < 0x80）のみランを延長する
/// - Blank はランを分断して個別の背景塗りランとして処理する
/// - ボックス文字・非 ASCII は個別描画（各 2 GDI コール）
/// - ランごとに ExtTextOutW × 1（背景）+ テキストランは × 1（テキスト）を加算
pub fn count_draw_ops_batched(
    grid: &Grid,
    cols: Option<usize>,
    dirty_rows: Option<&std::collections::HashSet<u16>>,
) -> DrawOpStats {
    let display_cols = cols.unwrap_or(grid.cols() as usize);
    let mut stats = DrawOpStats::default();

    if let Some(rows) = dirty_rows {
        for &row in rows {
            let Some(cells) = grid.row(row) else {
                continue;
            };
            count_draw_ops_batched_row(cells, display_cols, &mut stats);
        }
    } else {
        for row in 0..grid.rows() {
            let Some(cells) = grid.row(row) else {
                continue;
            };
            count_draw_ops_batched_row(cells, display_cols, &mut stats);
        }
    }

    stats
}

#[cfg(test)]
mod tests {
    use super::*;
    use yatamux_terminal::CjkWidthConfig;

    fn ascii_style(fg: Color, bg: Color) -> CellStyle {
        CellStyle {
            fg: Some(fg),
            bg: Some(bg),
            ..Default::default()
        }
    }

    fn make_grid(cols: u16, rows: u16) -> Grid {
        Grid::new(cols, rows, CjkWidthConfig::default())
    }

    fn default_fg() -> Color {
        Color {
            r: 0xCD,
            g: 0xD6,
            b: 0xF4,
        }
    }

    fn default_bg() -> Color {
        Color {
            r: 0x1E,
            g: 0x1E,
            b: 0x2E,
        }
    }

    #[test]
    fn batched_collapses_dense_ascii_into_one_text_run_per_row() {
        let fg = default_fg();
        let bg = default_bg();
        let style = ascii_style(fg, bg);

        let mut dense = make_grid(12, 4);
        for row in 0..4 {
            for c in 0..12 {
                let ch = (b'a' + (c % 26) as u8) as char;
                dense.write_char(&ch.to_string(), style);
            }
            if row < 3 {
                dense.carriage_return();
                dense.line_feed();
            }
        }

        assert_eq!(
            count_draw_ops_batched(&dense, None, None),
            DrawOpStats {
                ext_text_out_calls: 8,
                set_bk_color_calls: 4,
                set_text_color_calls: 4,
                rows_rendered: 4,
                cells_processed: 48,
            }
        );
    }

    #[test]
    fn batched_collapses_dense_ascii_80x24_into_24_text_runs() {
        let fg = default_fg();
        let bg = default_bg();
        let style = ascii_style(fg, bg);

        let mut dense = make_grid(80, 24);
        for row in 0..24 {
            for c in 0..80 {
                let ch = (b'a' + (c % 26) as u8) as char;
                dense.write_char(&ch.to_string(), style);
            }
            if row < 23 {
                dense.carriage_return();
                dense.line_feed();
            }
        }

        assert_eq!(
            count_draw_ops_batched(&dense, None, None),
            DrawOpStats {
                ext_text_out_calls: 48,
                set_bk_color_calls: 24,
                set_text_color_calls: 24,
                rows_rendered: 24,
                cells_processed: 80 * 24,
            }
        );
    }

    #[test]
    fn batched_merges_implicit_and_explicit_default_colors_into_one_text_run() {
        let explicit_defaults = ascii_style(default_fg(), default_bg());
        let implicit_defaults = CellStyle::default();
        let mut grid = make_grid(4, 1);
        for (index, ch) in ['a', 'b', 'c', 'd'].into_iter().enumerate() {
            let style = if index % 2 == 0 {
                implicit_defaults
            } else {
                explicit_defaults
            };
            grid.write_char(&ch.to_string(), style);
        }

        assert_eq!(
            count_draw_ops_batched(&grid, None, None),
            DrawOpStats {
                ext_text_out_calls: 2,
                set_bk_color_calls: 1,
                set_text_color_calls: 1,
                rows_rendered: 1,
                cells_processed: 4,
            }
        );
    }

    #[test]
    fn batched_splits_multicolor_ascii_runs_on_fg_changes() {
        let bg = default_bg();
        let palette: [Color; 4] = [
            Color {
                r: 0xF3,
                g: 0x8B,
                b: 0xA8,
            },
            Color {
                r: 0xA6,
                g: 0xE3,
                b: 0xA1,
            },
            Color {
                r: 0xF9,
                g: 0xE2,
                b: 0xAF,
            },
            Color {
                r: 0x89,
                g: 0xB4,
                b: 0xFA,
            },
        ];

        let mut grid = make_grid(4, 2);
        for row in 0..2 {
            for col in 0..4 {
                let fg = palette[(col as usize) % palette.len()];
                let ch = (b'a' + (col % 26) as u8) as char;
                grid.write_char(&ch.to_string(), ascii_style(fg, bg));
            }
            if row == 0 {
                grid.carriage_return();
                grid.line_feed();
            }
        }

        assert_eq!(
            count_draw_ops_batched(&grid, None, None),
            DrawOpStats {
                ext_text_out_calls: 16,
                set_bk_color_calls: 8,
                set_text_color_calls: 8,
                rows_rendered: 2,
                cells_processed: 8,
            }
        );
    }

    #[test]
    fn batched_counts_idle_prompt_when_only_prompt_row_is_dirty() {
        let style = ascii_style(default_fg(), default_bg());
        let mut grid = make_grid(10, 3);
        for ch in "$ ".chars() {
            grid.write_char(&ch.to_string(), style);
        }
        let dirty_rows = std::collections::HashSet::from([0u16]);

        assert_eq!(
            count_draw_ops_batched(&grid, None, Some(&dirty_rows)),
            DrawOpStats {
                ext_text_out_calls: 3,
                set_bk_color_calls: 2,
                set_text_color_calls: 1,
                rows_rendered: 1,
                cells_processed: 10,
            }
        );
    }

    #[test]
    fn batched_keeps_box_and_non_ascii_as_individual_draws_with_dirty_rows() {
        let style = ascii_style(default_fg(), default_bg());
        let mut grid = make_grid(6, 2);
        grid.line_feed();
        for ch in ["─", "é", "🙂", "x"] {
            grid.write_char(ch, style);
        }
        let dirty_rows = std::collections::HashSet::from([1u16]);

        assert_eq!(
            count_draw_ops_batched(&grid, None, Some(&dirty_rows)),
            DrawOpStats {
                ext_text_out_calls: 8,
                set_bk_color_calls: 5,
                set_text_color_calls: 3,
                rows_rendered: 1,
                cells_processed: 5,
            }
        );
    }

    /// 各シナリオの DrawOpStats をビフォーアフターで表示する。
    /// `cargo test -p yatamux-renderer -- print_op_counts --nocapture`
    #[test]
    fn print_op_counts() {
        let fg = default_fg();
        let bg = default_bg();
        let style = ascii_style(fg, bg);

        macro_rules! show {
            ($label:expr, $g:expr, $dr:expr) => {{
                let b = count_draw_ops(&$g, None, $dr);
                let a = count_draw_ops_batched(&$g, None, $dr);
                let save = if b.total_gdi_calls() > 0 {
                    let ratio = a.total_gdi_calls() * 100 / b.total_gdi_calls();
                    100i64 - ratio as i64
                } else {
                    0
                };
                println!(
                    "[{:30}] before={:5}  batched={:5}  {:+}%",
                    $label,
                    b.total_gdi_calls(),
                    a.total_gdi_calls(),
                    -save
                );
            }};
        }

        // S-1: idle_prompt（全行 vs カーソル 1 行のみ dirty）
        let mut g = make_grid(80, 24);
        for ch in "$ ".chars() {
            g.write_char(&ch.to_string(), style);
        }
        let one_row: std::collections::HashSet<u16> = [0].into();
        show!("idle_prompt 80x24 ALL ", g, None);
        show!("idle_prompt 80x24 1row", g, Some(&one_row));

        let mut g = make_grid(200, 50);
        for ch in "$ ".chars() {
            g.write_char(&ch.to_string(), style);
        }
        let one_row: std::collections::HashSet<u16> = [0].into();
        show!("idle_prompt 200x50 ALL", g, None);
        show!("idle_prompt 200x50 1row", g, Some(&one_row));

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
        let palette: &[(u8, u8, u8)] = &[
            (0xF3, 0x8B, 0xA8),
            (0xA6, 0xE3, 0xA1),
            (0xF9, 0xE2, 0xAF),
            (0x89, 0xB4, 0xFA),
        ];
        for (cols, rows) in [(80u16, 24u16), (200u16, 50u16)] {
            let mut g = make_grid(cols, rows);
            for row in 0..rows {
                for col in 0..cols {
                    let (r, gv, b) = palette[((row * cols + col) as usize) % palette.len()];
                    let s = ascii_style(Color { r, g: gv, b }, bg);
                    let ch = (b'a' + (col % 26) as u8) as char;
                    g.write_char(&ch.to_string(), s);
                }
                g.carriage_return();
                g.line_feed();
            }
            show!(format!("multicolor  {cols}x{rows}"), g, None);
        }

        // S-4: vim_style (行ごとに bg が交互)
        let bg2 = Color {
            r: 0x31,
            g: 0x32,
            b: 0x44,
        };
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
