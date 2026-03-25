## テスト計画: C-10 ペイン幅調整キーバインド

### TC-C10-01: 単一ペインで adjust_ratio は no-op
- **前提**: `LayoutNode::Leaf(1)` のみのツリー
- **操作**: `adjust_ratio(PaneId(1), 0.05)` を呼ぶ
- **期待結果**: `false` を返す。ツリー構造に変化なし

### TC-C10-02: 垂直分割・first ペインを拡大
- **前提**: `Split(Leaf(1), Leaf(2), ratio=0.5)`
- **操作**: `adjust_ratio(PaneId(1), 0.05)`
- **期待結果**: `true` を返す。ratio が `0.55` になる

### TC-C10-03: 垂直分割・second ペインを拡大
- **前提**: `Split(Leaf(1), Leaf(2), ratio=0.5)`
- **操作**: `adjust_ratio(PaneId(2), 0.05)` （second を拡大 = ratio 減少）
- **期待結果**: `true` を返す。ratio が `0.45` になる

### TC-C10-04: ratio が 0.9 を超えないようクランプ
- **前提**: `Split(Leaf(1), Leaf(2), ratio=0.88)`
- **操作**: `adjust_ratio(PaneId(1), 0.05)` → 0.93 になるはずが…
- **期待結果**: ratio が `0.9` にクランプされる

### TC-C10-05: ratio が 0.1 を下回らないようクランプ
- **前提**: `Split(Leaf(1), Leaf(2), ratio=0.12)`
- **操作**: `adjust_ratio(PaneId(1), -0.05)` → 0.07 になるはずが…
- **期待結果**: ratio が `0.1` にクランプされる

### TC-C10-06: ネストした Split — 内側の Split を操作
- **前提**: `Split(outer, Split(inner, Leaf(1), Leaf(2), ratio=0.5), Leaf(3), ratio=0.5)`
- **操作**: `adjust_ratio(PaneId(1), 0.05)`
- **期待結果**: 内側 Split の ratio が `0.55` になる。外側 Split の ratio は変化しない
