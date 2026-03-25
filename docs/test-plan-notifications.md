# テスト計画: バックグラウンドペイン通知（F-4a / F-4b）

## F-4a: PTY 終了時の自動通知

### TC-01: PTY が終了すると Notification が送出される
- **前提**: ペインが起動済み
- **操作**: PTY の出力チャネルが閉じる（プロセス終了）
- **期待結果**: `client_notification_tx` に `(pane_id, "Process exited")` が送信される

### TC-02: Notification の body が "Process exited" である
- **前提**: TC-01 と同じ
- **操作**: 通知受信
- **期待結果**: body が `"Process exited"` という文字列

## F-4b: BEL（`\x07`）→ 通知変換

### TC-03: BEL バイト受信で bell フラグが立つ
- **前提**: `VtProcessor` を初期化
- **操作**: `execute(0x07)` を呼ぶ
- **期待結果**: `proc.bell == true`

### TC-04: BEL を含まない入力では bell フラグが立たない
- **前提**: `VtProcessor` を初期化
- **操作**: 通常の ASCII 文字列を feed
- **期待結果**: `proc.bell == false`

### TC-05: BEL が Notification として転送される
- **前提**: ペインが起動済み
- **操作**: PTY 出力に `\x07` が含まれるデータを流す
- **期待結果**: `client_notification_tx` に `(pane_id, "Bell")` が送信される
