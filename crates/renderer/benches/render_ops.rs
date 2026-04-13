//! GDI 描画操作カウント ベンチマーク
//!
//! 実行:
//!   cargo bench -p yatamux-renderer
//!
//! HTML レポート:
//!   target/criterion/render_ops/*/report/index.html

use std::collections::HashSet;

use criterion::{criterion_group, criterion_main, BenchmarkId, Criterion};
use yatamux_renderer::draw_ops::{count_draw_ops, count_draw_ops_batched, DrawOpStats};
use yatamux_terminal::cell::{CellStyle, Color};
use yatamux_terminal::{CjkWidthConfig, Grid};

// ── グリッド構築ヘルパー ────────────────────────────────────────────────────

fn make_grid(cols: u16, rows: u16) -> Grid {
    Grid::new(cols, rows, CjkWidthConfig::default())
}

fn ascii_style(fg: Color, bg: Color) -> CellStyle {
    CellStyle {
        fg: Some(fg),
        bg: Some(bg),
        ..Default::default()
    }
}

/// S-1: idle_prompt — ほぼ空白、1 行目にプロンプトだけ
///
/// アイドル時のベースライン。大半のセルが Blank なので ExtTextOutW は少ないはず。
fn grid_idle_prompt(cols: u16, rows: u16) -> Grid {
    let mut grid = make_grid(cols, rows);
    let fg = Color {
        r: 0xCD,
        g: 0xD6,
        b: 0xF4,
    };
    let bg = Color {
        r: 0x1E,
        g: 0x1E,
        b: 0x2E,
    };
    let style = ascii_style(fg, bg);
    // 先頭行に "$ " のみ書き込む
    for ch in "$ ".chars() {
        grid.write_char(&ch.to_string(), style);
    }
    grid
}

/// S-2: dense_ascii — 全セル同色テキスト
///
/// `ls -la` のような密な ASCII 出力。色変化なしで ExtTextOutW が最大になるケース。
fn grid_dense_ascii(cols: u16, rows: u16) -> Grid {
    let mut grid = make_grid(cols, rows);
    let fg = Color {
        r: 0xCD,
        g: 0xD6,
        b: 0xF4,
    };
    let bg = Color {
        r: 0x1E,
        g: 0x1E,
        b: 0x2E,
    };
    let style = ascii_style(fg, bg);
    for _ in 0..rows {
        for c in 0..cols {
            let ch = (b'a' + (c % 26) as u8) as char;
            grid.write_char(&ch.to_string(), style);
        }
        grid.carriage_return();
        grid.line_feed();
    }
    grid
}

/// S-3: multicolor — セルごとに fg が変わる（最悪 SetTextColor ケース）
///
/// `cargo build` のカラー出力に近い。色切り替えが全セルで発生する。
fn grid_multicolor(cols: u16, rows: u16) -> Grid {
    let mut grid = make_grid(cols, rows);
    let bg = Color {
        r: 0x1E,
        g: 0x1E,
        b: 0x2E,
    };
    // ANSI 16 色の fg を順番に使う
    let fg_palette: &[(u8, u8, u8)] = &[
        (0xF3, 0x8B, 0xA8), // red
        (0xA6, 0xE3, 0xA1), // green
        (0xF9, 0xE2, 0xAF), // yellow
        (0x89, 0xB4, 0xFA), // blue
        (0xF5, 0xC2, 0xE7), // magenta
        (0x94, 0xE2, 0xD5), // cyan
        (0xCD, 0xD6, 0xF4), // white
        (0x58, 0x5B, 0x70), // bright black
    ];
    for row in 0..rows {
        for col in 0..cols {
            let (r, g, b) = fg_palette[((row * cols + col) as usize) % fg_palette.len()];
            let style = ascii_style(Color { r, g, b }, bg);
            let ch = (b'a' + (col % 26) as u8) as char;
            grid.write_char(&ch.to_string(), style);
        }
        grid.carriage_return();
        grid.line_feed();
    }
    grid
}

/// S-4: vim_style — 行ごとに bg が交互に変わる全画面アプリ近似
///
/// vim のステータスバーや htop のように行全体が異なる背景色を持つパターン。
fn grid_vim_style(cols: u16, rows: u16) -> Grid {
    let mut grid = make_grid(cols, rows);
    let fg = Color {
        r: 0xCD,
        g: 0xD6,
        b: 0xF4,
    };
    let bg_normal = Color {
        r: 0x1E,
        g: 0x1E,
        b: 0x2E,
    };
    let bg_highlight = Color {
        r: 0x31,
        g: 0x32,
        b: 0x44,
    };
    for row in 0..rows {
        let bg = if row % 2 == 0 {
            bg_normal
        } else {
            bg_highlight
        };
        let style = ascii_style(fg, bg);
        for col in 0..cols {
            let ch = (b'a' + (col % 26) as u8) as char;
            grid.write_char(&ch.to_string(), style);
        }
        grid.carriage_return();
        grid.line_feed();
    }
    grid
}

// ── ベンチマーク定義 ────────────────────────────────────────────────────────

/// 各シナリオの DrawOpStats を標準出力に表示する（--nocapture では見えない点に注意）。
/// `cargo bench -p yatamux-renderer -- --nocapture` で確認可能。
#[allow(dead_code)]
fn print_stats(label: &str, stats: DrawOpStats) {
    println!("[{label}] {stats}");
}

struct RenderBenchCase {
    label: String,
    grid: Grid,
    dirty_rows: Option<HashSet<u16>>,
}

fn bench_idle_prompt(c: &mut Criterion) {
    let mut cases = Vec::new();
    for (cols, rows) in [(80u16, 24u16), (200u16, 50u16)] {
        cases.push(RenderBenchCase {
            label: format!("{cols}x{rows}"),
            grid: grid_idle_prompt(cols, rows),
            dirty_rows: None,
        });
        cases.push(RenderBenchCase {
            label: format!("{cols}x{rows}-dirty1"),
            grid: grid_idle_prompt(cols, rows),
            dirty_rows: Some(HashSet::from([0u16])),
        });
    }
    let mut group = c.benchmark_group("idle_prompt");
    for case in &cases {
        group.bench_with_input(
            BenchmarkId::new("baseline", &case.label),
            case,
            |b, case| b.iter(|| count_draw_ops(&case.grid, None, case.dirty_rows.as_ref())),
        );
        group.bench_with_input(BenchmarkId::new("batched", &case.label), case, |b, case| {
            b.iter(|| count_draw_ops_batched(&case.grid, None, case.dirty_rows.as_ref()))
        });
    }
    group.finish();
}

fn bench_dense_ascii(c: &mut Criterion) {
    let grids = [(80u16, 24u16), (200u16, 50u16)];
    let mut group = c.benchmark_group("dense_ascii");
    for (cols, rows) in grids {
        let grid = grid_dense_ascii(cols, rows);
        group.bench_with_input(
            BenchmarkId::new("baseline", format!("{cols}x{rows}")),
            &grid,
            |b, g| b.iter(|| count_draw_ops(g, None, None)),
        );
        group.bench_with_input(
            BenchmarkId::new("batched", format!("{cols}x{rows}")),
            &grid,
            |b, g| b.iter(|| count_draw_ops_batched(g, None, None)),
        );
    }
    group.finish();
}

fn bench_multicolor(c: &mut Criterion) {
    let grids = [(80u16, 24u16), (200u16, 50u16)];
    let mut group = c.benchmark_group("multicolor");
    for (cols, rows) in grids {
        let grid = grid_multicolor(cols, rows);
        group.bench_with_input(
            BenchmarkId::new("baseline", format!("{cols}x{rows}")),
            &grid,
            |b, g| b.iter(|| count_draw_ops(g, None, None)),
        );
        group.bench_with_input(
            BenchmarkId::new("batched", format!("{cols}x{rows}")),
            &grid,
            |b, g| b.iter(|| count_draw_ops_batched(g, None, None)),
        );
    }
    group.finish();
}

fn bench_vim_style(c: &mut Criterion) {
    let grids = [(80u16, 24u16), (200u16, 50u16)];
    let mut group = c.benchmark_group("vim_style");
    for (cols, rows) in grids {
        let grid = grid_vim_style(cols, rows);
        group.bench_with_input(
            BenchmarkId::new("baseline", format!("{cols}x{rows}")),
            &grid,
            |b, g| b.iter(|| count_draw_ops(g, None, None)),
        );
        group.bench_with_input(
            BenchmarkId::new("batched", format!("{cols}x{rows}")),
            &grid,
            |b, g| b.iter(|| count_draw_ops_batched(g, None, None)),
        );
    }
    group.finish();
}

// DrawOpStats の数値スナップショットは draw_ops.rs の tests::print_op_counts を参照
// `cargo test -p yatamux-renderer -- print_op_counts --nocapture`
criterion_group!(
    benches,
    bench_idle_prompt,
    bench_dense_ascii,
    bench_multicolor,
    bench_vim_style,
);
criterion_main!(benches);
