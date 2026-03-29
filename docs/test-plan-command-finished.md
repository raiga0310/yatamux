## テスト計画: C-27 コマンド完了検知（OSC 133;D）

### TC-01: OSC 133;D を受信すると終了通知が立つ
- **前提**: `VtProcessor` が初期状態
- **操作**: `ESC ] 133 ; D BEL` を feed する
- **期待結果**: `command_finished == Some(None)` になる

### TC-02: OSC 133;D;{code} を受信すると終了コード付き通知が立つ
- **前提**: `VtProcessor` が初期状態
- **操作**: `ESC ] 133 ; D ; 7 BEL` を feed する
- **期待結果**: `command_finished == Some(Some(7))` になる

### TC-03: サーバーが `__cmd_finished__:` 通知を `CommandFinished` に変換する
- **前提**: `Server::run()` が動作中
- **操作**: `pane_notification_tx` に `("__cmd_finished__:42")` を送る
- **期待結果**: `ServerMessage::CommandFinished { pane, exit_code: Some(42) }` が client 側へ流れる

### TC-04: 終了コード省略時は `CommandFinished { exit_code: None }` になる
- **前提**: `Server::run()` が動作中
- **操作**: `pane_notification_tx` に `("__cmd_finished__:")` を送る
- **期待結果**: `ServerMessage::CommandFinished { pane, exit_code: None }` が client 側へ流れる

### TC-05: `send-keys --wait-for-prompt` を CLI が受け付ける
- **前提**: `clap` 引数パーサが利用可能
- **操作**: `yatamux send-keys --pane 1 --wait-for-prompt "echo hi"` を parse する
- **期待結果**: `wait_for_prompt == true` として解釈される

### TC-06: `send-keys --wait-for-prompt` の待機配線がコンパイル可能
- **前提**: `src/main.rs` から `cli::send_keys()` を呼び出す経路が有効
- **操作**: `cargo check`
- **期待結果**: `wait_for_prompt` 引数不足のコンパイルエラーが発生しない
