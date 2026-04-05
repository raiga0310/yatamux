## テスト計画: ペイン削除（F-8）

### TC-01: 単一ペインで Ctrl+Shift+W → `ClosePane` 送信後にアプリ終了
- **種別**: 手動確認
- **前提**: ペインが 1 枚のみ
- **操作**: `Ctrl+Shift+W` を押す
- **期待結果**: `close_active_pane()` が `ClosePane` を送信し、`PaneClosed` ハンドラで `grids.is_empty()` が真になって `should_quit = true` となり、`WM_TIMER` 経由で `DestroyWindow` されてアプリが終了する

### TC-02: 2 ペインでアクティブペインを削除 → 残りペインにフォーカス
- **種別**: 手動確認
- **前提**: 左右 2 ペインに垂直分割され、左がアクティブ
- **操作**: `Ctrl+Shift+W` を押す
- **期待結果**: 左ペインが削除され、レイアウトが単一 `Leaf(右ペイン)` になり、active が右ペインに切り替わる

### TC-03: 3 ペインで中間ペインを削除 → レイアウト再構成
- **種別**: 手動確認
- **前提**: `Split(A, Split(B, C))` の構成で `B` がアクティブ
- **操作**: `Ctrl+Shift+W` を押す
- **期待結果**: レイアウトが `Split(A, C)` に再構成され、active は残った隣接ペインへ移る

### TC-04: ペイン削除後の再描画
- **種別**: 手動確認
- **前提**: 任意の 2 ペイン構成
- **操作**: `Ctrl+Shift+W` でペイン削除
- **期待結果**: 削除されたペインのグリッドが消え、残ペインが拡大して再描画される

### TC-05: `LayoutNode::remove_pane` — 単一 `Leaf` では `None` を返す
- **種別**: 自動テスト
- **操作**: `LayoutNode::Leaf(PaneId(1)).remove_pane(PaneId(1))` を呼び出す
- **期待結果**: `None` を返す

### TC-06: `LayoutNode::remove_pane` — 垂直分割の `first` を削除
- **種別**: 自動テスト
- **前提**: `Split(Leaf(1), Leaf(2))`
- **操作**: `remove_pane(PaneId(1))`
- **期待結果**: 戻り値 `Some(PaneId(2))`、ツリーが `Leaf(2)` になる

### TC-07: `LayoutNode::remove_pane` — 垂直分割の `second` を削除
- **種別**: 自動テスト
- **前提**: `Split(Leaf(1), Leaf(2))`
- **操作**: `remove_pane(PaneId(2))`
- **期待結果**: 戻り値 `Some(PaneId(1))`、ツリーが `Leaf(1)` になる

### TC-08: `LayoutNode::remove_pane` — ネストしたツリーで削除
- **種別**: 自動テスト
- **前提**: `Split(Leaf(1), Split(Leaf(2), Leaf(3)))`
- **操作**: `remove_pane(PaneId(2))`
- **期待結果**: 戻り値 `Some(PaneId(3))`、ツリーが `Split(Leaf(1), Leaf(3))` になる

### TC-09: `WM_CHAR` で Ctrl+Shift+W が PTY に送信されない
- **種別**: 実装確認
- **前提**: `WM_KEYDOWN` で Ctrl+Shift+W が消費される
- **操作**: `handle_wm_keydown` / `handle_wm_char` の抑制経路を確認する
- **期待結果**: `Ctrl+Shift+W` は GUI ショートカットとして消費され、`^W` が PTY に送信されない
