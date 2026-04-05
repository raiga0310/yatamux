## テスト計画: capture-pane CLI コマンド (C-13)

### TC-C13-01: `CapturePane` メッセージが正しくシリアライズ/デシリアライズされる
- **種別**: 自動テスト
- **前提**: -
- **操作**: `ClientMessage::CapturePane { pane: PaneId(1), lines: 50, plain_text: true }` を JSON にシリアライズして再度デシリアライズする
- **期待結果**: `pane`、`lines`、`plain_text` が正確に復元される

### TC-C13-02: `PaneContent` メッセージが正しくシリアライズ/デシリアライズされる
- **種別**: 自動テスト
- **前提**: -
- **操作**: `ServerMessage::PaneContent { pane: PaneId(2), content: "hello\nworld".to_string(), capture: None }` を JSON シリアライズして再度デシリアライズする
- **期待結果**: `pane`、`content`、`capture` が正確に復元される

### TC-C13-03: 存在しないペインへの `CapturePane` は `ServerMessage::Error` を返す
- **種別**: 自動テスト
- **前提**: テスト用 `Server` が起動している
- **操作**: `ClientMessage::CapturePane { pane: PaneId(9999), lines: 100, plain_text: true }` を送る
- **期待結果**: `ServerMessage::Error { message }` が返り、`message` は `pane 9999 not found` を含む

### TC-C13-04: `lines=0` の `CapturePane` は空文字を返し、capture メタデータは保持される
- **種別**: 自動テスト（Windows）
- **前提**: 対象ペインが存在する
- **操作**: `ClientMessage::CapturePane { pane, lines: 0, plain_text: true }` を送る
- **期待結果**: `ServerMessage::PaneContent { content: "", capture: Some(...) }` が返り、`capture.lines_requested == 0` になる

### TC-C13-05: 実在するペインへの `CapturePane` は内容を返す
- **種別**: 自動テスト（Windows）
- **前提**: PTY に初期出力が出ているペインが存在する
- **操作**: `ClientMessage::CapturePane { pane, lines: 100, plain_text: true }` を送る
- **期待結果**: `ServerMessage::PaneContent` が返り、`content` は非空、`capture` も `Some(...)` になる

### TC-C13-06: `yatamux capture-pane --plain-text` が CLI で受け付けられ、ANSI なし出力を要求できる
- **種別**: 実装確認
- **前提**: `capture-pane` CLI が有効
- **操作**: `src/main.rs` と `src/cli.rs` を確認し、`--plain-text` が `ClientMessage::CapturePane.plain_text = true` に配線されていることを確認する
- **期待結果**: プレーンテキスト出力を要求する経路が存在する

### TC-C13-07: `yatamux capture-pane --json` が構造化 JSON 出力経路を使用する
- **種別**: 自動テスト + 実装確認
- **前提**: `capture-pane --json` オプションが有効
- **操作**: `yatamux capture-pane --target 1 --lines 20 --json` を parse し、`src/cli.rs` の JSON 出力経路を確認する
- **期待結果**: `json == true` として解釈され、`PaneContent.capture` を含む整形済み JSON を標準出力に出す

### TC-C13-08: 手動で `capture-pane` の出力内容を確認する
- **種別**: 手動確認
- **前提**: yatamux GUI が起動済みで、対象ペインに可視テキストがある
- **操作**: `yatamux capture-pane --target <ID> --lines 10` と `yatamux capture-pane --target <ID> --lines 10 --plain-text` を実行する
- **期待結果**: 対象ペインの内容が取得でき、`--plain-text` 指定時は AI/CLI 処理向けのプレーンテキストとして読める
