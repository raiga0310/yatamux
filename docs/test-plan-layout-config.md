## テスト計画: 宣言的レイアウト設定 (C-5)

### TC-01: layout_path — 正しいパスを返す（ユニットテスト）
- **操作**: `LayoutConfig::layout_path("dev")`
- **期待結果**: パスが `yatamux\layouts\dev.toml` を含む

### TC-02: 有効な TOML ファイルをロードできる（ユニットテスト）
- **前提**: 一時ファイルに `[[panes]]\ncommand = "nvim ."` を書き込む
- **操作**: `LayoutConfig::load(&path)`
- **期待結果**: `Ok(config)`, `config.panes[0].command == Some("nvim .")`

### TC-03: 不正な TOML は Err を返す（ユニットテスト）
- **前提**: 一時ファイルに不正な TOML を書き込む
- **操作**: `LayoutConfig::load(&path)`
- **期待結果**: `Err(_)`

### TC-04: 空の panes リストは有効（ユニットテスト）
- **前提**: `[[panes]]` なし（空ファイルまたは `[layout]` のみ）
- **操作**: `LayoutConfig::load(&path)`
- **期待結果**: `Ok(config)`, `config.panes.is_empty() == true`

### TC-05: split フィールドが正しくデシリアライズされる（ユニットテスト）
- **前提**: `split = "vertical"` を含む TOML
- **操作**: `toml::from_str::<PaneConfig>(...)
- **期待結果**: `PaneConfig { split: Some(SplitDir::Vertical), .. }`

### TC-06: `--layout dev` でレイアウト設定が適用される（手動テスト）
- **前提**: `%APPDATA%\yatamux\layouts\dev.toml` に2ペイン分割設定あり
- **操作**: `yatamux --layout dev` を実行
- **期待結果**: 設定どおりに分割されたウィンドウが開く

### TC-07: layout config の command がシェルに送信される（手動テスト）
- **前提**: `command = "echo hello"` を含むレイアウト設定
- **操作**: `yatamux --layout <name>` を実行
- **期待結果**: 対応ペインに `echo hello` の出力が表示される

### TC-08: 存在しないレイアウト名はシングルペインにフォールバック（手動テスト）
- **操作**: `yatamux --layout nonexistent` を実行
- **期待結果**: 警告ログ出力後、通常のシングルペインで起動する

### TC-09: `--layout` なし時はセッション復元が通常どおり動作する（手動テスト）
- **前提**: `session.toml` が存在する
- **操作**: `yatamux`（`--layout` なし）
- **期待結果**: 保存済みレイアウトが復元される
