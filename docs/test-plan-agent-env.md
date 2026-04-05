## テスト計画: エージェント向け環境変数伝搬

### TC-A6-01: PTY 子プロセスに `YATAMUX=1` が設定される
- **種別**: 自動テスト
- **前提**: Windows で ConPTY を使った PTY 統合テストが実行できる
- **操作**: `cmd.exe /C echo %YATAMUX%` を PTY で起動する
- **期待結果**: 出力に `1` が含まれる

### TC-A6-02: PTY 子プロセスに `TERM_PROGRAM=yatamux` が設定される
- **種別**: 自動テスト
- **前提**: Windows で ConPTY を使った PTY 統合テストが実行できる
- **操作**: `cmd.exe /C echo %TERM_PROGRAM%` を PTY で起動する
- **期待結果**: 出力に `yatamux` が含まれる

### TC-A6-03: PTY 子プロセスに `YATAMUX_SESSION=default` が設定される
- **種別**: 自動テスト
- **前提**: 現行実装はデフォルトセッションのみを対象に IPC サーバーを起動する
- **操作**: `cmd.exe /C echo %YATAMUX_SESSION%` を PTY で起動する
- **期待結果**: 出力に `default` が含まれる
