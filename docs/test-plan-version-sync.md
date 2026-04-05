## テスト計画: workspace 版数同期と CI bump

root crate と member crates の版数を別々に持つと、release 表示や crate metadata が食い違う。
workspace で一元管理し、CI の bump も同じ経路を更新する。

### TC-VS-01: root crate と member crates が同じ workspace version を参照する
- **種別**: 自動テスト
- **前提**: `Cargo.toml` と `crates/*/Cargo.toml` が読める
- **操作**: ルートと各 member manifest を読み、`package.version.workspace = true` になっていることを確認する
- **期待結果**: 個別 crate が古い固定版数を持たず、workspace version を参照している

### TC-VS-02: 現在の workspace version で全体がビルドできる
- **種別**: 自動テスト
- **前提**: workspace manifests 更新後
- **操作**: `cargo test` を実行する
- **期待結果**: lockfile と package metadata が整合し、全テストが通る

### TC-VS-03: CI bump workflow が workspace version を更新する
- **種別**: 実装確認
- **前提**: `.github/workflows/bump-version.yml` が読める
- **操作**: bump 手順を確認する
- **期待結果**: root の `package.version` だけでなく、workspace version を更新する形になっている

### TC-VS-04: bump 後の commit 対象に member crate 追従分が自然に含まれる
- **種別**: 実装確認
- **前提**: member crates が workspace version 参照に切り替わっている
- **操作**: bump workflow の `git add` 対象を確認する
- **期待結果**: `Cargo.toml` と `Cargo.lock` の更新だけで member crates も同じ版数へ追従する
