## テスト計画: Pane モードでの縦方向（垂直分割）比率調整 (C-18)

### 前提確認: adjust_ratio の方向無依存性

`adjust_ratio()` は `SplitDirection` を区別せず、アクティブペインを含む最近傍 Split ノードの ratio を調整する。`SplitDirection::Horizontal`（上下分割）に対しても正しく動作することを確認する。

### TC-01: Horizontal Split の first ペインを拡大

- **前提**: `Horizontal(ratio=0.5, first=A, second=B)` のレイアウト。
- **操作**: Pane モードで `+` を押す（アクティブ = A）。
- **期待結果**: ratio が 0.55 になる（上ペインが広がる）。

### TC-02: Horizontal Split の second ペインを拡大

- **前提**: `Horizontal(ratio=0.5, first=A, second=B)` のレイアウト。
- **操作**: Pane モードで `+` を押す（アクティブ = B）。
- **期待結果**: ratio が 0.45 になる（下ペインが広がる）。

### TC-03: `-` キーで縮小

- **前提**: `Horizontal(ratio=0.5, first=A, second=B)` のレイアウト。
- **操作**: Pane モードで `-` を押す（アクティブ = A）。
- **期待結果**: ratio が 0.45 になる。

### TC-04: Pane モードを維持して連続調整可能

- **前提**: Horizontal 分割のレイアウト。
- **操作**: Pane モードで `+` を複数回押す。
- **期待結果**: Pane モードが維持され、押すたびに 5% ずつ ratio が変化する。

### TC-05: 比率のクランプ（上限・下限）

- **前提**: `ratio=0.88` の Horizontal 分割。
- **操作**: Pane モードで `+` を押す。
- **期待結果**: ratio が 0.9 でクランプされる（0.93 にはならない）。

### TC-06: ネスト構造で内側 Split を調整

- **前提**: `Horizontal(ratio=0.5, first=Vertical(A, B), second=C)` のレイアウト。
  アクティブ = A。
- **操作**: Pane モードで `+` を押す。
- **期待結果**: 内側の Vertical ratio が変化し、外側の Horizontal ratio は変わらない。

### TC-07: ステータスバーに `+/-: 縦比` が表示される

- **前提**: Pane モードに移行した状態。
- **操作**: ステータスバーを確認する。
- **期待結果**: `+/-: 縦比` の文字列がヒントとして表示される。

### TC-08: 既存の `<`/`>` は引き続き動作する

- **前提**: Vertical 分割のレイアウト。
- **操作**: Pane モードで `<` / `>` を押す。
- **期待結果**: 従来通り ratio が ±5% 調整される。

---

### ユニットテスト（自動）

#### TC-C18-01: Horizontal Split — first 拡大

```
adjust_ratio(PaneId(1), +0.05) on Horizontal(0.5, A=1, B=2)
→ ratio == 0.55
```

#### TC-C18-02: Horizontal Split — second 拡大

```
adjust_ratio(PaneId(2), +0.05) on Horizontal(0.5, A=1, B=2)
→ ratio == 0.45
```

#### TC-C18-03: Horizontal Split — クランプ

```
adjust_ratio(PaneId(1), +0.05) on Horizontal(0.88, A=1, B=2)
→ ratio == 0.9
```
