## テスト計画: subscribe-pane / output stream

### TC-01: `subscribe-pane` の CLI parse
- **前提**: `subscribe-pane` サブコマンドが追加されている
- **操作**: `yatamux subscribe-pane --pane tests --json`
- **期待結果**: `pane=tests` と `json=true` が正しく parse される

### TC-02: stream event の JSON 変換
- **前提**: `ServerMessage::Output` / `Notification` / `CommandFinished` / `PaneClosed` を JSON Lines に変換する helper がある
- **操作**: 対象 pane の各イベントを helper に渡す
- **期待結果**: `event` タグ付き JSON へ変換され、他 pane のイベントは無視される

### TC-03: lagged 通知を stream event として扱う
- **前提**: IPC broadcast lag の drop policy を定義している
- **操作**: `subscription lagged by N messages` 相当の Error を stream helper に渡す
- **期待結果**: `lagged` イベントとして扱われ、購読が途切れず利用者に見える

### TC-04: IPC 側の購読フィルタ
- **前提**: pane 単位の subscribe / unsubscribe が IPC 層で解釈される
- **操作**: subscription set あり / なしで `ServerMessage::Output` や `PanesListed` の転送可否を評価する
- **期待結果**: 未購読時は従来どおり broadcast、購読時は対象 pane の stream event だけが forward される

### TC-05: `SubscribePane` 購読中は対象 pane の Output だけを受け取る
- **前提**: Windows named pipe IPC の統合テストを実行できる
- **操作**: 2 ペインを作成し、片方だけ購読したクライアントから両方へ Input を送る
- **期待結果**: 購読対象 pane の `Output` は受信し、非対象 pane の `Output` は受信しない
