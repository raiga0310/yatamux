## テスト計画: ペイン分割・リサイズ時の描画崩れ（F-5）

---

### 設計前提

- `PaneStore.layout_changed: bool` フラグを追加する
- `split_leaf` / `remove_pane` 呼び出し後に `layout_changed = true` にする
- `WM_TIMER` ハンドラで `layout_changed` を検出したら `content_bb` を `None` にセットしてバックバッファを破棄する
- バックバッファが `None` の場合、次の `WM_PAINT` でバックバッファが再作成され全面 `theme.bg` で塗りつぶされ、全グリッドが `mark_all_dirty()` される

---

### unit test

#### TC-01: `layout_changed` フラグが `false` で初期化される
- **前提**: `PaneStore::new(pane_id, grid)` でストアを生成する
- **操作**: 生成後の `layout_changed` を確認する
- **期待結果**: `false`

#### TC-02: `split_leaf` 後に `layout_changed` が `true` になる
- **前提**: 2ペインの `PaneStore` と対応 `LayoutNode` を用意する
- **操作**: `store.layout.split_leaf(parent, child, dir)` → `store.layout_changed = true` のシーケンスを実行する
- **期待結果**: `store.layout_changed == true`

#### TC-03: `layout_changed` を読み出してクリアできる
- **前提**: `layout_changed = true` の `PaneStore`
- **操作**: `store.layout_changed` を読んで `false` にセット
- **期待結果**: 読み出し値が `true`、その後 `layout_changed == false`

#### TC-04: `Grid::resize()` で dirty フラグが全行セットされる（既存動作の回帰確認）
- **前提**: 80×24 の `Grid` を生成し `take_dirty_rows()` で一度クリアする
- **操作**: `grid.resize(40, 24)` を呼ぶ
- **期待結果**: `grid.has_dirty_rows() == true`、`take_dirty_rows()` の長さが `24`

#### TC-05: `TerminalSink::new()` で生成した Grid は全行 dirty
- **前提**: `TerminalSink::new(80, 24)` で生成する
- **操作**: `sink.grid.lock().unwrap().has_dirty_rows()` を確認する
- **期待結果**: `true`（スタートアップ時に残像が出ないことを保証）

---

### 修正対象ファイル

| ファイル | 変更内容 |
|---------|---------|
| `crates/client/src/layout/store.rs` | `PaneStore` に `layout_changed: bool` を追加、`new()` で `false` 初期化 |
| `src/app/bridge.rs` | `split_leaf` / `remove_pane` 呼び出し後に `store.layout_changed = true` をセット |
| `crates/client/src/window/win32/wndproc.rs` | `handle_wm_timer` で `layout_changed` を検出 → `state.content_bb.set(None)` |

---

### CI 実行ポリシー

| テスト種別 | 実行タイミング |
|-----------|--------------|
| unit（TC-01〜05） | 常時（`cargo test`）|
| 視覚確認（残像・ずれ） | PR レビュー時に手動確認 |
