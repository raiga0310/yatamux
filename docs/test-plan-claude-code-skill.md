## テスト計画: Claude Code 統合スキル

### TC-01: スキル定義が pane 分離の基本フローを含む
- **前提**: `integrations/claude-code/SKILL.md` を同梱する
- **操作**: スキル本文を確認する
- **期待結果**: `list-panes` → `split-pane` → `set-pane-meta` → `send-keys` / `exec` → `subscribe-pane` / `capture-pane` → `interrupt-pane` / `terminate-pane` / `close-pane` の流れが示されている

### TC-02: worker pane 作成 wrapper の構文が正しい
- **前提**: `integrations/claude-code/scripts/new-worker-pane.ps1` を追加する
- **操作**: PowerShell でスクリプトを parse する
- **期待結果**: 構文エラーなく読み込める

### TC-03: pane 監視 wrapper の構文が正しい
- **前提**: `integrations/claude-code/scripts/watch-pane.ps1` を追加する
- **操作**: PowerShell でスクリプトを parse する
- **期待結果**: 構文エラーなく読み込める

### TC-04: README に AI サブエージェント運用チュートリアルがある
- **前提**: Claude Code / yatamux 連携の README 追記を行う
- **操作**: README を確認する
- **期待結果**: worker pane の作成、alias / role 付け、監視、回収の一連の例が掲載されている
