## テスト計画: capture-pane CLI コマンド (C-13)

### TC-C13-01: CapturePane メッセージが正しくシリアライズ/デシリアライズされる
- **前提**: -
- **操作**: `ClientMessage::CapturePane { pane: PaneId(1), lines: 50 }` を JSON にシリアライズして再度デシリアライズ
- **期待結果**: フィールドが正確に復元される

### TC-C13-02: PaneContent メッセージが正しくシリアライズ/デシリアライズされる
- **前提**: -
- **操作**: `ServerMessage::PaneContent { pane: PaneId(2), content: "hello\nworld".to_string() }` を JSON シリアライズ → デシリアライズ
- **期待結果**: フィールドが正確に復元される

### TC-C13-03: CapturePane のデフォルト値が正しい（手動確認）
- **前提**: yatamux GUI が起動済み
- **操作**: `yatamux capture-pane` を引数なしで実行
- **期待結果**: デフォルト target=0、lines=100 で動作し、アクティブペインの内容を表示する

### TC-C13-04: 存在しないペインへの CapturePane は空のコンテンツを返す（手動確認）
- **前提**: yatamux GUI が起動済み
- **操作**: `yatamux capture-pane --target 9999` を実行
- **期待結果**: 空文字列または何も出力されない

### TC-C13-05: lines=0 の場合は空コンテンツが返る
- **前提**: yatamux GUI が起動済み
- **操作**: `yatamux capture-pane --lines 0` を実行
- **期待結果**: 空文字列が出力される

### TC-C13-06: capture-pane の出力が実際のペイン内容に一致する（手動確認）
- **前提**: ペインで `echo hello` を実行済み
- **操作**: `yatamux capture-pane --lines 10` を実行
- **期待結果**: "hello" を含む行が出力される
