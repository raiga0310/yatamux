# テスト計画: 起動時セッション復元（C-4）

## 対象

`app.rs` の `restore_node()` および起動時の session load 分岐。

## ユニットテスト

### TC-01: セッションファイルがない場合は単一ペインで起動
- **前提**: `%APPDATA%\yatamux\session.toml` が存在しない
- **期待**: `PaneStore.layout` が `LayoutNode::Leaf`、`active == pane_id`

### TC-02: セッションファイルがある場合は復元
- **前提**: 2ペイン水平分割の `LayoutSnapshot` を保存済み
- **期待**: `PaneStore.layout` が `LayoutNode::Split`、`grids` に2エントリ

### TC-03: 旧 active ペインが新ペインに正しくマッピングされる
- **前提**: `snap.active == old_id_2`（2番目のリーフ）
- **期待**: `store.active` == 新しい2番目のペイン ID（DFS順でマッピング）

### TC-04: ネストした3ペインレイアウトを復元できる
- **前提**: `Split(V, Leaf(1), Split(H, Leaf(2), Leaf(3)))` を保存
- **期待**: 同構造のツリーが `PaneStore.layout` に復元される

## 統合テスト（手動）

1. yatamux を起動し、`Ctrl+Shift+E` で水平分割
2. 別コマンドを実行して2ペインが認識できる状態にする
3. ウィンドウを閉じる（WM_CLOSE → `session.toml` 保存）
4. yatamux を再起動 → 2ペインレイアウトが復元されていること
5. 各ペインにキー入力が届くこと
