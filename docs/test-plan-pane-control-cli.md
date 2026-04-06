## テスト計画: pane control CLI

### TC-01: `interrupt-pane` CLI 引数が正しく parse される
- **前提**: `yatamux interrupt-pane --pane <id>` サブコマンドが追加されている
- **操作**: clap で引数を parse する
- **期待結果**: 対象ペイン ID が正しく取得される

### TC-02: `close-pane` CLI 引数が正しく parse される
- **前提**: `yatamux close-pane --pane <id>` サブコマンドが追加されている
- **操作**: clap で引数を parse する
- **期待結果**: 対象ペイン ID が正しく取得される

### TC-03: `InterruptPane` メッセージが JSON 往復できる
- **前提**: `ClientMessage::InterruptPane` が protocol に追加されている
- **操作**: シリアライズしてからデシリアライズする
- **期待結果**: ペイン ID を保ったまま roundtrip する

### TC-04: 存在するペインへ `InterruptPane` を送ると Error にならない
- **前提**: Windows 上で PTY ペインを 1 枚起動できる
- **操作**: `ClientMessage::InterruptPane { pane }` を送信する
- **期待結果**: 直後に `ServerMessage::Error` は返らない

### TC-05: 存在しないペインへ `InterruptPane` を送ると not found Error が返る
- **前提**: server 単体テスト環境
- **操作**: 未作成の `PaneId` に `InterruptPane` を送信する
- **期待結果**: `pane <id> not found` を含む `ServerMessage::Error` が返る

### TC-06: `close-pane` CLI が `PaneClosed` を待って終了できる
- **前提**: 実在するペインが 1 枚ある
- **操作**: `ClientMessage::ClosePane` を送信し、対象ペインの `PaneClosed` を待つ
- **期待結果**: 該当ペイン ID の `PaneClosed` を受け取って正常終了する

### TC-07: `terminate-pane` CLI 引数が正しく parse される
- **前提**: `yatamux terminate-pane --pane <id>` サブコマンドが追加されている
- **操作**: clap で引数を parse する
- **期待結果**: 対象ペイン ID が正しく取得される

### TC-08: `TerminatePane` メッセージが JSON 往復できる
- **前提**: `ClientMessage::TerminatePane` が protocol に追加されている
- **操作**: シリアライズしてからデシリアライズする
- **期待結果**: ペイン ID を保ったまま roundtrip する

### TC-09: 存在しないペインへ `TerminatePane` を送ると not found Error が返る
- **前提**: server 単体テスト環境
- **操作**: 未作成の `PaneId` に `TerminatePane` を送信する
- **期待結果**: `pane <id> not found` を含む `ServerMessage::Error` が返る

### TC-10: 実在するペインへ `TerminatePane` を送ると最終的に `PaneClosed` が返る
- **前提**: Windows 上で PTY ペインを 1 枚起動できる
- **操作**: `ClientMessage::TerminatePane { pane }` を送信する
- **期待結果**: 対象ペイン ID の `PaneClosed` を受け取って終了する
