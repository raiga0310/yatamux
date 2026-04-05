//! ターミナルグリッド行からの URL 検出ユーティリティ。
//!
//! `https://` / `http://` で始まるURLをセル配列からスキャンし、
//! 指定された列インデックスがURL範囲内かを判定する。

use yatamux_terminal::cell::CellContent;
use yatamux_terminal::Cell;

/// URL として認識するスキーム
const URL_PREFIXES: &[&str] = &["https://", "http://"];

/// URL の終端と見なす文字
fn is_url_terminator(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t' | '"' | '\'' | '<' | '>' | '`' | '[' | ']' | '(' | ')' | '\x00'..='\x1f'
    )
}

/// セル配列の行テキストを再構成し、指定列 `target_col` にある URL を返す。
///
/// 戻り値は `(col_start, col_end_exclusive, url_string)` のタプル。
/// `col_start..col_end_exclusive` のセル範囲がURL。
///
/// 全角文字（`width == 2`）は 2 セルを消費するが、テキスト中では 1 文字として扱う。
/// このためセルインデックスと文字インデックスはズレることがある。
/// URL 範囲はセルインデックスで返す。
pub fn find_url_at_col(cells: &[Cell], target_col: usize) -> Option<(usize, usize, String)> {
    // セル配列から (cell_col, char) のペアを構築
    // Continuation セルはスキップ（先行する Grapheme の一部）
    let mut col_chars: Vec<(usize, char)> = Vec::with_capacity(cells.len());
    for (ci, cell) in cells.iter().enumerate() {
        if let CellContent::Grapheme { text, .. } = &cell.content {
            if let Some(c) = text.chars().next() {
                col_chars.push((ci, c));
            }
        }
    }

    // col_chars からプレーンテキストを作り、URL を探す
    // URL の開始セル列と終了セル列（exclusive）を追跡する
    let text: String = col_chars.iter().map(|(_, c)| *c).collect();

    for prefix in URL_PREFIXES {
        let mut search_start = 0;
        while let Some(rel) = text[search_start..].find(prefix) {
            let url_char_start = search_start + rel;

            // URL 終端を探す（URL 終端文字 or テキスト末尾）
            let url_char_end = text[url_char_start..]
                .char_indices()
                .find(|(_, c)| is_url_terminator(*c))
                .map(|(i, _)| url_char_start + i)
                .unwrap_or(text.len());

            // 最低限の長さ（スキームより長い必要がある）
            if url_char_end > url_char_start + prefix.len() {
                // セル列インデックスに変換
                let cell_col_start = col_chars[url_char_start].0;
                let cell_col_end = if url_char_end < col_chars.len() {
                    col_chars[url_char_end].0
                } else {
                    // 末尾は最後のセルの次
                    cells
                        .iter()
                        .enumerate()
                        .rev()
                        .find(|(_, c)| !matches!(c.content, CellContent::Blank))
                        .map(|(i, c)| {
                            i + if let CellContent::Grapheme { width, .. } = &c.content {
                                *width as usize
                            } else {
                                1
                            }
                        })
                        .unwrap_or(cells.len())
                };

                if target_col >= cell_col_start && target_col < cell_col_end {
                    let url = text[url_char_start..url_char_end].to_string();
                    // 末尾の句読点を除去（URLの一部でないことが多い）
                    let url = url.trim_end_matches(['.', ',', ';', ':']);
                    if !url.is_empty() {
                        return Some((cell_col_start, cell_col_end, url.to_string()));
                    }
                }
            }
            search_start = url_char_start + 1;
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use yatamux_terminal::cell::CellStyle;
    use yatamux_terminal::Cell;

    fn make_cells(text: &str) -> Vec<Cell> {
        text.chars()
            .map(|c| Cell {
                content: CellContent::Grapheme {
                    text: c.to_string(),
                    width: 1,
                },
                style: CellStyle::default(),
            })
            .collect()
    }

    #[test]
    fn test_url_detected_at_col() {
        // "see https://example.com for details"
        let s = "see https://example.com for details";
        let cells = make_cells(s);
        let result = find_url_at_col(&cells, 10); // 'e' in example
        assert!(result.is_some());
        let (start, _, url) = result.unwrap();
        assert_eq!(start, 4); // "https://" starts at col 4
        assert_eq!(url, "https://example.com");
    }

    #[test]
    fn test_no_url_at_plain_text() {
        let cells = make_cells("hello world");
        assert!(find_url_at_col(&cells, 3).is_none());
    }

    #[test]
    fn test_url_end_trimmed_trailing_punct() {
        let cells = make_cells("visit https://example.com.");
        let result = find_url_at_col(&cells, 10);
        assert!(result.is_some());
        let (_, _, url) = result.unwrap();
        assert_eq!(url, "https://example.com");
    }

    #[test]
    fn test_col_outside_url_returns_none() {
        // "abc https://x.com def"
        let cells = make_cells("abc https://x.com def");
        // col 0 は "abc" の 'a' — URL 外
        assert!(find_url_at_col(&cells, 0).is_none());
        // col 19 は " " — URL 外
        assert!(find_url_at_col(&cells, 19).is_none());
    }
}
