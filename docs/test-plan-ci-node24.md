## テスト計画: GitHub Actions Node.js 24 移行

### TC-01: `release.yml` が Node.js 24 対応アクションを使う
- **前提**: GitHub Actions の JavaScript action runtime を Node.js 24 にそろえたい
- **操作**: `.github/workflows/release.yml` の `uses:` を確認する
- **期待結果**: `actions/checkout` と `actions/cache` が Node.js 24 対応 major に更新されている

### TC-02: `bump-version.yml` が Node.js 24 対応アクションを使う
- **前提**: バージョンバンプ workflow でも checkout action を使っている
- **操作**: `.github/workflows/bump-version.yml` の `uses:` を確認する
- **期待結果**: `actions/checkout` が Node.js 24 対応 major に更新されている

### TC-03: Node.js 20 前提の action 参照が残っていない
- **前提**: 既存 workflow から Node.js 20 runtime action を減らしたい
- **操作**: `.github/workflows/` 配下を確認する
- **期待結果**: `actions/checkout@v4` と `actions/cache@v4` が残っていない

### TC-04: GitHub hosted runner 前提の互換性整理
- **前提**: `actions/checkout@v5` / `actions/cache@v5` は Actions Runner の最小バージョン要件がある
- **操作**: 公式リリース / Marketplace の要件を確認する
- **期待結果**: `windows-latest` / `ubuntu-latest` の hosted runner 利用では追加対応不要と判断できる
