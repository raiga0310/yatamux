## テスト計画: draw-ops-batched dense-ascii 最適化

### TC-01: dense ASCII 80x24 が行単位の 1 テキストランとして維持される
- **前提**: 80x24 の Grid を同色 ASCII で全埋めする。
- **操作**: `count_draw_ops_batched` を実行する。
- **期待結果**: 各行が 1 ランとして集約され、`DrawOpStats` は行数ぶんの背景描画とテキスト描画のみになる。

### TC-02: dirty 1 行の idle prompt で既存の高速ケースの集計が変わらない
- **前提**: 80x24 の Grid の先頭行だけに prompt を書き、`dirty_rows = {0}` を指定する。
- **操作**: `count_draw_ops_batched` を実行する。
- **期待結果**: 既存の `idle_prompt/80x24-dirty1` 相当の集計結果を維持する。

### TC-03: Box 文字・非 ASCII の個別描画パスが維持される
- **前提**: Box 文字、BMP 非 ASCII、サロゲートペア、ASCII を含む Grid を用意する。
- **操作**: `count_draw_ops_batched` を `dirty_rows` 付きで実行する。
- **期待結果**: ASCII 以外の特殊ケースは個別描画として集計され、ASCII ラン最適化に巻き込まれない。

### TC-04: dense_ascii ベンチで 80x24 の batched 回帰が解消される
- **前提**: `cargo bench -p yatamux-renderer -- dense_ascii` を実行できる環境。
- **操作**: `dense_ascii/80x24` の `baseline` と `batched` を比較する。
- **期待結果**: `batched` が `baseline` より遅い状態を解消し、少なくとも回帰前より改善している。

### TC-05: 暗黙デフォルト色と明示デフォルト色が同一テキストランとして集約される
- **前提**: `CellStyle::default()` と `fg/bg` に明示的なデフォルト色を入れたスタイルを交互に並べた ASCII Grid を用意する。
- **操作**: `count_draw_ops_batched` を実行する。
- **期待結果**: `None` と `Some(DEFAULT_*)` が同じ実効色として扱われ、1 行 1 テキストランの `DrawOpStats` を維持する。
