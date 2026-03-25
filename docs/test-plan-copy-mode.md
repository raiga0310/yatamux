## テスト計画: コピーモード (C-12)

### TC-C12-01: CopyState が正しく初期化される
- **前提**: -
- **操作**: `CopyState { cursor: (0, 0), anchor: None }` を作成
- **期待結果**: cursor が (0,0)、anchor が None

### TC-C12-02: カーソル移動が端でクランプされる
- **前提**: cols=80, rows=24 のグリッド
- **操作**: cursor=(0,0) から上/左へ移動しようとする
- **期待結果**: cursor が (0,0) のまま（負の値にならない）

### TC-C12-03: アンカーのセット/アンセットが正しく動作する
- **前提**: `CopyState { cursor: (5, 3), anchor: None }`
- **操作**: anchor に cursor の位置をセット
- **期待結果**: `anchor == Some((5, 3))`; 再度 None に設定可能

### TC-C12-04: Grid::extract_text がASCIIテキストを正しく抽出する
- **前提**: 80x24 グリッドに "hello" を書き込む
- **操作**: `grid.extract_text(0, 0)` を呼ぶ
- **期待結果**: "hello" が返る（末尾の空白は除去）

### TC-C12-05: Grid::extract_text が複数行の選択範囲を処理する
- **前提**: 80x24 グリッドに行0="hello"、行1="world" を書き込む
- **操作**: `grid.extract_text(0, 1)` を呼ぶ
- **期待結果**: "hello\nworld" が返る

### TC-C12-06: Grid::extract_text がCJK全角文字の Continuation セルをスキップする
- **前提**: 80x24 グリッドに CJK 文字（例: "日"）を書き込む
- **操作**: `grid.extract_text(0, 0)` を呼ぶ
- **期待結果**: Continuation セルが含まれず、文字数が正確に返る

### TC-C12-07: Pane モードで V キーを押すと Copy モードに入る（手動確認）
- **前提**: 通常動作中の yatamux
- **操作**: `Ctrl+B` → `V` キー
- **期待結果**: Copy モードになり、ステータスバーに "COPY" と表示される

### TC-C12-08: Copy モードで Esc/q を押すと Normal モードに戻る（手動確認）
- **前提**: Copy モード中
- **操作**: `Esc` または `q` を押す
- **期待結果**: Normal モードに戻り、Copy 状態がクリアされる

### TC-C12-09: v キーでビジュアル選択が開始される（手動確認）
- **前提**: Copy モード中
- **操作**: `v` キーを押す
- **期待結果**: anchor がカーソル位置にセットされ、選択範囲のハイライトが表示される

### TC-C12-10: y/Enter でテキストがクリップボードにコピーされる（手動確認）
- **前提**: Copy モードでビジュアル選択中
- **操作**: `y` または `Enter` を押す
- **期待結果**: 選択テキストが pending_clipboard にセットされ、Normal モードに戻る
