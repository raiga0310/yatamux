## テスト計画: 高度な Unicode / 絵文字対応 (C-6)

### TC-01: ZWJ 結合絵文字の str_width は 2（ユニットテスト）
- **操作**: `cfg.str_width("👨\u{200D}💻")`
- **期待結果**: `2`（ZWJ シーケンス全体が 2 セル幅）

### TC-02: VS-16 (U+FE0F) を含むグラフィームは 2 セル（ユニットテスト）
- **操作**: `cfg.str_width("♀\u{FE0F}")`
- **期待結果**: `2`

### TC-03: VS-15 (U+FE0E) を含むグラフィームは 1 セル（ユニットテスト）
- **操作**: `cfg.str_width("♀\u{FE0E}")`
- **期待結果**: `1`（テキスト表示 = 狭幅）

### TC-04: ZWJ 後の文字は新セルを作らず前セルに結合される（ユニットテスト）
- **前提**: Grid にカーソルを col=0 で初期化
- **操作**: VtProcessor で "👨\u{200D}💻" を 1 文字ずつ feed
- **期待結果**: col=0 のセルが `Grapheme { text: "👨\u{200D}💻", width: 2 }`, col=2 は Blank

### TC-05: VS-16 受信後に前セルの幅が 2 になる（ユニットテスト）
- **前提**: Grid の col=0 に `Grapheme { text: "♀", width: 1 }` を書き込む
- **操作**: `grid.apply_vs16()`（カーソルは col=1）
- **期待結果**: col=0 が `width: 2`, col=1 が `Continuation`

### TC-06: VS-15 は前セルにテキストとして付加される（ユニットテスト）
- **前提**: Grid の col=0 に `Grapheme { text: "♀", width: 1 }`
- **操作**: `grid.combine_with_last_cell('\u{FE0E}')`
- **期待結果**: col=0 の text が `"♀\u{FE0E}"`, width=1 のまま

### TC-07: BiDi 制御文字（U+200E, U+202A）は幅 0（ユニットテスト）
- **操作**: `cfg.char_width('\u{200E}')`, `cfg.char_width('\u{202A}')`
- **期待結果**: `0`

### TC-08: Nerd Fonts グリフ（U+E000）が nerd_fonts_wide=true で幅 2（ユニットテスト）
- **前提**: `CjkWidthConfig { nerd_fonts_wide: true, .. }`
- **操作**: `cfg.char_width('\u{E000}')`
- **期待結果**: `2`

### TC-09: Nerd Fonts グリフが nerd_fonts_wide=false（デフォルト）で幅 1（ユニットテスト）
- **操作**: `cfg.char_width('\u{E001}')`
- **期待結果**: `1`

### TC-10: write_char が str_width を使って ZWJ 文字列を 2 セルで書く（ユニットテスト）
- **操作**: `grid.write_char("👨\u{200D}💻", style)`
- **期待結果**: col=0 が `width: 2`, col=1 が Continuation

### TC-11: VtProcessor で ZWJ 絵文字を表示する（手動テスト）
- **操作**: ペイン内で `echo 👨‍💻` を実行
- **期待結果**: 2 セル幅の ZWJ 絵文字として表示される（文字化けなし）
