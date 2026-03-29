## テスト計画: typed pane internal events (A-2 High)

### TC-A2H-01: `CommandFinished(Some(code))` が `ServerMessage::CommandFinished` に変換される
- **前提**: テスト用 `Server` を notifier 付きで起動する
- **操作**: `PaneEvent::CommandFinished(Some(42))` を送信する
- **期待結果**: `ServerMessage::CommandFinished { pane, exit_code: Some(42) }` が返る

### TC-A2H-02: `CommandFinished(None)` が `ServerMessage::CommandFinished` に変換される
- **前提**: テスト用 `Server` を notifier 付きで起動する
- **操作**: `PaneEvent::CommandFinished(None)` を送信する
- **期待結果**: `ServerMessage::CommandFinished { pane, exit_code: None }` が返る

### TC-A2H-03: `Notification(String)` が `ServerMessage::Notification` に変換される
- **前提**: テスト用 `Server` を notifier 付きで起動する
- **操作**: `PaneEvent::Notification("hello".to_string())` を送信する
- **期待結果**: `ServerMessage::Notification { pane, body: "hello" }` が返る

### TC-A2H-04: `Bell` が `ServerMessage::Notification { body: "Bell" }` に変換される
- **前提**: テスト用 `Server` を notifier 付きで起動する
- **操作**: `PaneEvent::Bell` を送信する
- **期待結果**: `ServerMessage::Notification { pane, body: "Bell" }` が返る

### TC-A2H-05: `ProcessExited` が `Notification` と `PaneClosed` を発火する
- **前提**: テスト用 `Server` を notifier 付きで起動する
- **操作**: `PaneEvent::ProcessExited` を送信する
- **期待結果**: `ServerMessage::Notification { body: "Process exited" }` の後に `ServerMessage::PaneClosed { pane }` が返る
