## テスト計画: pane state metadata

### TC-01: `PaneInfo` が追加メタデータを保持できる
- **前提**: `PaneInfo` に `cwd` / `command` / `busy` / `last_output_unix_ms` が追加されている
- **操作**: 値ありの `PaneInfo` を JSON にシリアライズしてからデシリアライズする
- **期待結果**: 追加フィールドが欠落せず往復する

### TC-02: 旧 `PanesListed` JSON との後方互換性
- **前提**: 旧形式の `PaneInfo` JSON は `id / surface / title / cols / rows` のみを含む
- **操作**: 旧形式 JSON を `ServerMessage::PanesListed` としてデシリアライズする
- **期待結果**: 追加フィールドは `None` または `false` のデフォルトで復元される

### TC-03: `ListPanes` が `cwd` / `command` / `busy` を返す
- **前提**: Windows 上で server テストから PTY ペインを 1 枚起動できる
- **操作**: `ClientMessage::ListPanes` を送信し `ServerMessage::PanesListed` を受け取る
- **期待結果**: 対象ペインの `cwd` / `command` / `busy` フィールドが JSON へ載る

### TC-04: `busy` が入力送信後に true になり、`CommandFinished` で false に戻る
- **前提**: server が pane ごとの busy 状態を保持している
- **操作**: `Input` を送ってから `PaneEvent::CommandFinished` を注入し、その前後で `ListPanes` を取る
- **期待結果**: 入力後は `busy=true`、完了後は `busy=false`

### TC-05: `last_output_unix_ms` が出力受信で更新される
- **前提**: server が pane 出力時刻を保持している
- **操作**: `PaneCreated` 直後と `Output` 受信後で `ListPanes` を取り比較する
- **期待結果**: 出力後の `last_output_unix_ms` が `Some(...)` になり、0 でない値になる
