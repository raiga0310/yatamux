## テスト計画: ペイン削除（F-8）

### TC-01: 単一ペインで Ctrl+Shift+W → 何も起こらない
- **前提**: ペインが1枚のみ
- **操作**: Ctrl+Shift+W
- **期待結果**: ClosePane が送信されない、ウィンドウはそのまま残る

### TC-02: 2ペインでアクティブペインを削除 → 残りペインにフォーカス
- **前提**: 左右2ペインに垂直分割（左がアクティブ）
- **操作**: Ctrl+Shift+W
- **期待結果**: 左ペインが削除、レイアウトが単一 Leaf(右ペイン) になる、active が右ペインに切り替わる

### TC-03: 3ペインで中間ペインを削除 → レイアウト再構成
- **前提**: Split(A, Split(B, C)) の構成、B がアクティブ
- **操作**: Ctrl+Shift+W
- **期待結果**: Split(A, C) になる、active が C か A のいずれかになる

### TC-04: ペイン削除後の再描画
- **前提**: 任意の2ペイン構成
- **操作**: Ctrl+Shift+W でペイン削除
- **期待結果**: 削除されたペインのグリッドが消え、残ペインが画面全体に広がって描画される

### TC-05: LayoutNode::remove_pane — 単体テスト: 単一 Leaf では None を返す
- **操作**: `LayoutNode::Leaf(PaneId(1)).remove_pane(PaneId(1))` を呼び出す
- **期待結果**: None を返す（削除不可）

### TC-06: LayoutNode::remove_pane — 単体テスト: 垂直分割の first を削除
- **前提**: `Split(Leaf(1), Leaf(2))`
- **操作**: `remove_pane(PaneId(1))`
- **期待結果**: 戻り値 Some(PaneId(2))、ツリーが `Leaf(2)` になる

### TC-07: LayoutNode::remove_pane — 単体テスト: 垂直分割の second を削除
- **前提**: `Split(Leaf(1), Leaf(2))`
- **操作**: `remove_pane(PaneId(2))`
- **期待結果**: 戻り値 Some(PaneId(1))、ツリーが `Leaf(1)` になる

### TC-08: LayoutNode::remove_pane — 単体テスト: ネストしたツリーで削除
- **前提**: `Split(Leaf(1), Split(Leaf(2), Leaf(3)))`
- **操作**: `remove_pane(PaneId(2))`
- **期待結果**: 戻り値 Some(PaneId(3))、ツリーが `Split(Leaf(1), Leaf(3))` になる

### TC-09: WM_CHAR で Ctrl+Shift+W がスキップされる
- **前提**: WM_KEYDOWN で Ctrl+Shift+W を捕捉
- **期待結果**: WM_CHAR ハンドラでは `ctrl && shift` の guard により ^W が PTY に送信されない
