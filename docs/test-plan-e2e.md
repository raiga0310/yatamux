## テスト計画: Windows E2E

実バイナリとして `yatamux` を起動し、Named Pipe IPC・ConPTY・セッション保存復元まで含めた主要フローを確認する。
unit / integration test では拾いにくい「結線はできるが実プロセス運用で壊れる」退行を検出するための計画。

### TC-01: コールドスタートして IPC で最初のペインを観測できる
- **前提**: 一時ディレクトリを `APPDATA` に使い、専用 session 名で `yatamux` を起動する
- **操作**: `list-panes --json` と `capture-pane --json` を呼ぶ
- **期待結果**: 最初の 1 ペインが取得でき、IPC 接続・ペイン列挙・画面キャプチャが通る

### TC-02: ペイン分割から入力・待機までの基本フローが動く
- **前提**: 起動済みセッションがある
- **操作**: `split-pane` で新規ペインを作成し、`send-keys` または `exec` でコマンドを送り、`wait-pane` で完了を待つ
- **期待結果**: 追加ペインが列挙に現れ、送信したコマンド結果を `capture-pane` で確認できる

### TC-03: `exec` が request / result と終了状態を end-to-end で返す
- **前提**: 起動済みセッションがある
- **操作**: `exec --pane <id> -- <command>` を実行する
- **期待結果**: `request_id` 付きの protocol 経路で実行され、成功時は exit status / timeout / pane close を正しく反映する

### TC-04: 出力購読と制御 API が実プロセス相手に動く
- **前提**: 起動済みセッションがあり、長めに出力するコマンドを流せる
- **操作**: `subscribe-pane` で購読しつつ出力を発生させ、`interrupt-pane` / `close-pane` / `terminate-pane` を試す
- **期待結果**: 新着出力を購読でき、割り込み・終了・クローズの結果が CLI から観測できる

### TC-05: `SaveAndQuit` で `session.toml` が保存される
- **前提**: 複数ペイン、alias / role、cwd / command を持つセッションを作る
- **操作**: `save-and-quit` 相当の終了経路を通す
- **期待結果**: `session.toml` にレイアウト・メタデータ・cwd / command が保存される

### TC-06: 次回起動で前回セッションを復元できる
- **前提**: `session.toml` が保存済み
- **操作**: 新しく `yatamux` を起動し、`list-panes --json` と `capture-pane` を確認する
- **期待結果**: ペイン構造、active / floating、alias / role、復元コマンドが意図通り再現される

### TC-07: self-update の安全な smoke を end-to-end で通す
- **前提**: mock release または staged binary を使い、本物の GitHub Release には依存しない
- **操作**: `yatamux update` を安全なテスト入力で実行する
- **期待結果**: download / verify / `SaveAndQuit` / apply helper のつながりを確認でき、失敗時も既存 exe を壊さない

### TC-08: CI 実行モードを分離できる
- **前提**: E2E テストが Windows runner でのみ安定して動く
- **操作**: 通常 `cargo test`、`#[ignore]`、専用 workflow の候補を比較する
- **期待結果**: ローカルでは `cargo test` から外れた `#[ignore]` とし、CI では専用 `e2e.yml` から `cargo test --test e2e_smoke -- --ignored` を回す
- **補足**: workflow 内の action は full SHA で pin し、直前コメントに `vX.Y.Z` を残して更新元を追えるようにする
- **運用メモ**: `master` の branch protection では `Windows E2E smoke` を required check にしているため、workflow / job 名は安易に変更しない
