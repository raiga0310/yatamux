# テスト計画: スクロールバック表示（F-5）

## Grid スクロールバックバッファ

### TC-01: フルスクリーンスクロール時に行がスクロールバックに保存される
- **前提**: 3 行グリッド、row 0 に "A" を書き込む
- **操作**: `scroll_up(1)`（scroll_top==0、scroll_bottom==rows-1）
- **期待結果**: `scrollback_len() == 1`、`scrollback_row(0)` に "A" の行が入っている

### TC-02: スクロール領域が全画面でない場合はスクロールバックに保存しない
- **前提**: 3 行グリッド、`set_scroll_region(2, 3)`（1 始まり）後 row 1 に "A"
- **操作**: `scroll_up(1)`
- **期待結果**: `scrollback_len() == 0`

### TC-03: スクロールバックの上限（5000 行）を超えたら古い行を捨てる
- **前提**: 3 行グリッド
- **操作**: `scroll_up(1)` を 5001 回実行
- **期待結果**: `scrollback_len() == 5000`

### TC-04: 複数行スクロール時は複数行がスクロールバックに保存される
- **前提**: 3 行グリッド、各行に "A"/"B"/"C"
- **操作**: `scroll_up(2)`
- **期待結果**: `scrollback_len() == 2`。row 0 = "A"、row 1 = "B" の順で格納

### TC-05: オルタネートスクリーン中はスクロールバックに保存しない
- **前提**: `enter_alternate_screen()` 後
- **操作**: `scroll_up(1)`
- **期待結果**: `scrollback_len() == 0`

## クライアント側スクロールオフセット（レイアウト）

### TC-06: scroll_offset が 0 のとき get_display_offset は 0 を返す
- **前提**: `PaneStore` の `scroll_offset == 0`
- **期待結果**: `scroll_offset == 0`

### TC-07: scroll_offset はスクロールバック行数を超えない
- **前提**: scrollback_len() == 5
- **操作**: `scroll_offset` を 10 にセット
- **期待結果**: `scroll_offset.min(scrollback_len()) == 5` でクランプされる

## C-7: 効率的なスクロールバックバッファ

### TC-08: ScrollbackBuffer の上限が 50,000 行である（ユニットテスト）
- **操作**: `Grid::SCROLLBACK_MAX`
- **期待結果**: `50_000`

### TC-09: ScrollbackBuffer::as_text が末尾の空白を除去して改行で連結する（ユニットテスト）
- **前提**: "foo  ", "bar" を push した ScrollbackBuffer
- **操作**: `buf.as_text()`
- **期待結果**: `"foo\nbar"`

### TC-10: full_content_text() がスクロールバック + 画面内容を含む（ユニットテスト）
- **前提**: "A" を書いてスクロールアウト後、"B" を書き込んだ Grid（2 行）
- **操作**: `grid.full_content_text()`
- **期待結果**: "A" と "B" が両方含まれる

### TC-11: Pane モード X でスクロールバックが一時ファイル + エディタ起動（手動テスト）
- **操作**: ペインに履歴を蓄積後、`Ctrl+B` → `X`
- **期待結果**: `$EDITOR <tmpfile>` がペインに入力されエディタが開く
