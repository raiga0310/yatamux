## テスト計画: F-6 ペイン境界移動方向の一貫性

### 背景

Pane モードで `<`/`>` または `+`/`-` を押したとき、フォーカスしているペインの位置（first/second）によって
境界の移動方向が逆転して見えるバグ（F-6）を修正する。

`>` キーは「境界を右に動かす（Vertical split の first を拡大）」という絶対方向であるべき。
フォーカスペインが first でも second でも同じ方向に境界が動くことが期待される。

### TC-F6-01: 垂直分割・first フォーカスで `>` → 境界が右に動く

- **前提**: `Split(Vertical, ratio=0.5, Leaf(1), Leaf(2))`、アクティブペイン = 1（first）
- **操作**: `adjust_ratio_for_dir(PaneId(1), +0.05, Vertical)`
- **期待結果**: ratio が `0.55` になる（境界が右に移動）

### TC-F6-02: 垂直分割・second フォーカスで `>` → 境界が右に動く（first が縮小）

- **前提**: `Split(Vertical, ratio=0.5, Leaf(1), Leaf(2))`、アクティブペイン = 2（second）
- **操作**: `adjust_ratio_for_dir(PaneId(2), +0.05, Vertical)`
- **期待結果**: ratio が `0.55` になる（境界が右に移動 = second が縮小）

### TC-F6-03: 垂直分割・first フォーカスで `<` → 境界が左に動く

- **前提**: `Split(Vertical, ratio=0.5, Leaf(1), Leaf(2))`、アクティブペイン = 1（first）
- **操作**: `adjust_ratio_for_dir(PaneId(1), -0.05, Vertical)`
- **期待結果**: ratio が `0.45` になる（境界が左に移動）

### TC-F6-04: 垂直分割・second フォーカスで `<` → 境界が左に動く

- **前提**: `Split(Vertical, ratio=0.5, Leaf(1), Leaf(2))`、アクティブペイン = 2（second）
- **操作**: `adjust_ratio_for_dir(PaneId(2), -0.05, Vertical)`
- **期待結果**: ratio が `0.45` になる（境界が左に移動 = first が縮小）

### TC-F6-05: 水平分割・first フォーカスで `+` → 境界が下に動く

- **前提**: `Split(Horizontal, ratio=0.5, Leaf(1), Leaf(2))`、アクティブペイン = 1（first）
- **操作**: `adjust_ratio_for_dir(PaneId(1), +0.05, Horizontal)`
- **期待結果**: ratio が `0.55` になる（境界が下に移動）

### TC-F6-06: 水平分割・second フォーカスで `+` → 境界が下に動く（first が縮小）

- **前提**: `Split(Horizontal, ratio=0.5, Leaf(1), Leaf(2))`、アクティブペイン = 2（second）
- **操作**: `adjust_ratio_for_dir(PaneId(2), +0.05, Horizontal)`
- **期待結果**: ratio が `0.55` になる（境界が下に移動 = second が縮小）

### TC-F6-07: ネストした Split — 正しいノードを操作

- **前提**: `Split(Vertical, ratio=0.5, Split(Vertical, ratio=0.5, Leaf(1), Leaf(2)), Leaf(3))`
- **操作**: `adjust_ratio_for_dir(PaneId(2), +0.05, Vertical)` （内側の second）
- **期待結果**: 内側 Split の ratio が `0.55` になる。外側 Split の ratio は変化しない

### TC-F6-08: クランプ確認（境界移動でも clamp は維持される）

- **前提**: `Split(Vertical, ratio=0.88, Leaf(1), Leaf(2))`
- **操作**: `adjust_ratio_for_dir(PaneId(2), +0.05, Vertical)`（second フォーカス）
- **期待結果**: ratio が `0.9` にクランプされる
