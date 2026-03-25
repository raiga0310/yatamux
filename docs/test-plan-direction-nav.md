# テスト計画: 方向指定ペインフォーカス移動（F-3）

## `LayoutNode::pane_in_direction`

### TC-01: 水平分割で Right → 右ペインへ移動
- **前提**: `Split(Vertical, Leaf(1), Leaf(2))`, root_rect = (0,0,200,100)
- **操作**: `pane_in_direction(PaneId(1), Right, root_rect)`
- **期待**: `PaneId(2)`

### TC-02: 水平分割で Left → 左ペインへ移動
- **操作**: `pane_in_direction(PaneId(2), Left, root_rect)`
- **期待**: `PaneId(1)`

### TC-03: 端のペインで移動先なし → 自ペインを返す
- **操作**: `pane_in_direction(PaneId(1), Left, root_rect)` (Leaf(1) は左端)
- **期待**: `PaneId(1)` (変化なし)

### TC-04: 垂直分割で Down → 下ペインへ移動
- **前提**: `Split(Horizontal, Leaf(1), Leaf(2))`, root_rect = (0,0,100,200)
- **操作**: `pane_in_direction(PaneId(1), Down, root_rect)`
- **期待**: `PaneId(2)`

### TC-05: 3ペインレイアウトで最近傍を選ぶ
```
+---+---+
| 1 | 2 |
+---+---+
|   3   |
+-------+
```
- **操作**: `pane_in_direction(PaneId(1), Down, root_rect)`
- **期待**: `PaneId(3)` (PaneId(2) でも可だが直下の 3 が優先)

### TC-06: 単一ペインでは常に自ペインを返す
- **前提**: `Leaf(1)`
- **操作**: `pane_in_direction(PaneId(1), Right, root_rect)`
- **期待**: `PaneId(1)`
