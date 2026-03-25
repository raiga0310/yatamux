## テスト計画: プラグイン / 拡張システム (C-8)

### TC-01: 有効な config.toml を読み込める（ユニットテスト）
- **操作**: `[hooks]\non_pane_created = "echo hi"` を含む TOML を `AppConfig::load()`
- **期待結果**: `config.hooks.on_pane_created == Some("echo hi")`

### TC-02: config.toml が存在しない場合はデフォルト設定を返す（ユニットテスト）
- **操作**: 存在しないパスに `AppConfig::load()`
- **期待結果**: `Ok(AppConfig::default())` — フックはすべて `None`

### TC-03: 不正な TOML は Err を返す（ユニットテスト）
- **操作**: 不正な TOML ファイルを `AppConfig::load()`
- **期待結果**: `Err(_)`

### TC-04: default_path が %APPDATA%\yatamux\config.toml を返す（ユニットテスト）
- **操作**: `AppConfig::default_path()`
- **期待結果**: パスに `yatamux` と `config.toml` が含まれる

### TC-05: on_pane_created フックがペイン作成時に発火する（手動テスト）
- **前提**: `on_pane_created = "echo %YATAMUX_PANE_ID% >> C:\tmp\hook.log"` を設定
- **操作**: `Ctrl+Shift+E` で新規ペイン作成
- **期待結果**: `C:\tmp\hook.log` にペイン ID が書き込まれる

### TC-06: on_pane_closed フックがペイン終了時に発火する（手動テスト）
- **前提**: `on_pane_closed = "echo closed >> C:\tmp\hook.log"` を設定
- **操作**: `Ctrl+Shift+W` でペインを閉じる
- **期待結果**: `C:\tmp\hook.log` に "closed" が書き込まれる

### TC-07: 空文字列フックは実行されない（手動テスト）
- **前提**: `on_pane_created = ""`
- **操作**: ペイン作成
- **期待結果**: 外部プロセスが起動しない（タスクマネージャーで確認）

### TC-08: YATAMUX_SESSION 環境変数がセッション名を含む（手動テスト）
- **操作**: `on_pane_created = "echo %YATAMUX_SESSION%"` でペイン作成
- **期待結果**: `default` が出力される
