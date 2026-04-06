## テスト計画: wait-pane / exec

### TC-01: `wait-pane --wait-for exit` の CLI parse
- **前提**: `wait-pane` サブコマンドが追加されている
- **操作**: `yatamux wait-pane --pane 3 --wait-for exit`
- **期待結果**: `pane=3` と `wait_for=exit` が正しく parse される

### TC-02: `wait-pane --output-regex` の CLI parse
- **前提**: `wait-pane` が output regex 条件を受け付ける
- **操作**: `yatamux wait-pane --pane 2 --output-regex passed --lines 300`
- **期待結果**: regex と lines が正しく parse される

### TC-03: `exec -- <command>` の CLI parse
- **前提**: `exec` サブコマンドが追加されている
- **操作**: `yatamux exec --pane 1 --timeout 30 -- cargo test -q`
- **期待結果**: pane / timeout / command ベクタが正しく parse される

### TC-04: silence 待機の内部ロジック
- **前提**: 出力時刻列から silence 判定するヘルパーがある
- **操作**: 直近出力あり / 出力なし / silence duration 超過の各ケースを評価する
- **期待結果**: silence 成立タイミングが仕様どおりになる

### TC-05: output regex 待機の内部ロジック
- **前提**: `PaneContent.content` へ regex を当てるヘルパーがある
- **操作**: 一致 / 不一致 / 無効 regex の各ケースを評価する
- **期待結果**: 一致時のみ成功し、無効 regex はエラーになる

### TC-06: `wait-pane --wait-for exit` が `CommandFinished` で成功する
- **前提**: 対象ペインに対して `CommandFinished` を受け取れる
- **操作**: `wait-pane` 相当ヘルパーを `CommandFinished { exit_code: Some(0) }` で完了させる
- **期待結果**: 正常終了する

### TC-07: `exec` が command を送信して wait helper を使う
- **前提**: `exec` が send-keys 相当の入力送信と待機をまとめて行う
- **操作**: command 文字列から送信バイト列を組み立てて Enter 付きで送る
- **期待結果**: 入力末尾に `\r` が付与され、既定の待機条件が適用される
