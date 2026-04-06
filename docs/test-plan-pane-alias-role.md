## テスト計画: ペイン alias / role

### TC-01: `PaneInfo` が alias / role を roundtrip できる
- **前提**: `PaneInfo` に `alias` / `role` フィールドが追加されている
- **操作**: alias / role 付きの `PaneInfo` を JSON シリアライズして復元する
- **期待結果**: 既存フィールドを壊さず alias / role も保持される

### TC-02: 旧 JSON では alias / role が `None` になる
- **前提**: 旧 `list-panes --json` 相当の JSON
- **操作**: alias / role を含まない JSON を `PaneInfo` にデシリアライズする
- **期待結果**: 後方互換で読み込めて alias / role は `None`

### TC-03: `set-pane-meta` が CLI で parse できる
- **前提**: `set-pane-meta` サブコマンドが追加されている
- **操作**: `yatamux set-pane-meta --pane tests --alias tests --role verifier`
- **期待結果**: pane selector / alias / role が正しく parse される

### TC-04: pane selector が alias を ID に解決できる
- **前提**: `list-panes` 取得結果に alias 付きペインが含まれる
- **操作**: `send-keys` / `capture-pane` / `exec` 相当の resolver に alias を渡す
- **期待結果**: 対応する `PaneId` を返す

### TC-05: pane selector が numeric ID も従来通り解決できる
- **前提**: 既存の `--pane 3` を壊さない
- **操作**: resolver に `"3"` を渡す
- **期待結果**: `PaneId(3)` として扱われる

### TC-06: server の `list-panes` が alias / role を返す
- **前提**: server に alias / role を持つペインがある
- **操作**: `ListPanes` を送る
- **期待結果**: `PanesListed` の `PaneInfo` に alias / role が含まれる

### TC-07: `set-pane-meta` 後に `list-panes` へ反映される
- **前提**: 既存ペインが 1 つある
- **操作**: `SetPaneMeta` を送ってから `ListPanes` を送る
- **期待結果**: 対象ペインの alias / role が更新されている

### TC-08: session 保存で alias / role が `session.toml` に残る
- **前提**: `PaneStore` に alias / role が入っている
- **操作**: `save_session()` を呼ぶ
- **期待結果**: `LayoutNodeDef::Leaf` に alias / role が保存される

### TC-09: session 復元で alias / role が `CreatePane` に渡る
- **前提**: alias / role 付き `LayoutSnapshot`
- **操作**: `restore_node()` を通して復元する
- **期待結果**: 復元後の新ペインに alias / role が設定される
