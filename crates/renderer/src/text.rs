//! テキストモードレンダラー（開発・デバッグ用）
//!
//! Grid の内容を ANSI エスケープシーケンスとして stdout に出力する。
//! 実際の GPU レンダリングが完成するまでのプレースホルダ。

use yatamux_terminal::cell::CellContent;
use yatamux_terminal::Grid;

/// グリッドをターミナルに出力（デバッグ用）
pub fn render_to_stdout(grid: &Grid) {
    for row in 0..grid.rows() {
        let cells = match grid.row(row) {
            Some(c) => c,
            None => continue,
        };
        let mut line = String::new();
        for cell in cells {
            match &cell.content {
                CellContent::Grapheme { text, .. } => line.push_str(text),
                CellContent::Blank => line.push(' '),
                CellContent::Continuation => {} // 全角文字の右半分は描画しない
            }
        }
        println!("{}", line.trim_end());
    }
}
