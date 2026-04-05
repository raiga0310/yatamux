## テスト計画: C-27 コマンド完了検知（OSC 133;D）

### TC-01: OSC 133;D を受信すると終了通知が立つ
- **種別**: 自動テスト
- **前提**: `VtProcessor` が初期状態
- **操作**: `ESC ] 133 ; D BEL` を feed する
- **期待結果**: `command_finished == Some(None)` になる

### TC-02: OSC 133;D;{code} を受信すると終了コード付き通知が立つ
- **種別**: 自動テスト
- **前提**: `VtProcessor` が初期状態
- **操作**: `ESC ] 133 ; D ; 7 BEL` を feed する
- **期待結果**: `command_finished == Some(Some(7))` になる

### TC-03: `PaneEvent::CommandFinished(Some(code))` が client 向け `ServerMessage::CommandFinished` に転送される
- **種別**: 自動テスト
- **前提**: notifier 付きのテスト用 `Server` が起動している
- **操作**: `pane_event_tx` に `(PaneId(7), PaneEvent::CommandFinished(Some(42)))` を送る
- **期待結果**: `ServerMessage::CommandFinished { pane: PaneId(7), exit_code: Some(42) }` が返る

### TC-04: `PaneEvent::CommandFinished(None)` が `exit_code: None` として転送される
- **種別**: 自動テスト
- **前提**: notifier 付きのテスト用 `Server` が起動している
- **操作**: `pane_event_tx` に `(PaneId(9), PaneEvent::CommandFinished(None))` を送る
- **期待結果**: `ServerMessage::CommandFinished { pane: PaneId(9), exit_code: None }` が返る

### TC-05: `send-keys --wait-for-prompt` を CLI が受け付ける
- **種別**: 自動テスト
- **前提**: `clap` 引数パーサが利用可能
- **操作**: `yatamux send-keys --pane 1 --wait-for-prompt "echo hi"` を parse する
- **期待結果**: `wait_for_prompt == true` として解釈される

### TC-06: `send-keys --wait-for-prompt` は対象ペインの `CommandFinished` を待機条件として扱う
- **種別**: 実装確認
- **前提**: `cli::send_keys()` の `--wait-for-prompt` 経路が有効
- **操作**: 実装を確認し、待機ループが `ServerMessage::CommandFinished { pane, exit_code }` を監視していることを確認する
- **期待結果**: 対象ペインの `CommandFinished` を受信したときのみ待機解除し、非ゼロ終了コードではその値で終了する
