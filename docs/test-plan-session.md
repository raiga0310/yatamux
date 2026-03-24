# テスト計画: セッション永続化 (C-4)

## 概要

ペインのレイアウトツリー（`LayoutNode`）を TOML にシリアライズして
`%APPDATA%\yatamux\session.toml` に保存し、起動時に読み込んで復元する。

## 対象ファイル

- `crates/client/src/layout.rs` — `LayoutNode` に serde derive を追加
- `crates/client/src/session.rs` — 新規: `LayoutSnapshot` と保存・読み込み API

---

## 型定義（実装前に確定する）

```rust
/// シリアライズ可能なレイアウトスナップショット
/// (Grid の Arc<Mutex<...>> は含まない)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct LayoutSnapshot {
    pub root: LayoutNodeDef,
    pub active: PaneId,
}

/// シリアライズ可能なレイアウトノード
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNodeDef {
    Leaf { id: PaneId },
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<LayoutNodeDef>,
        second: Box<LayoutNodeDef>,
    },
}
```

---

## テストケース一覧

### TC-01: Leaf ノードの TOML ラウンドトリップ（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | `LayoutNodeDef::Leaf { id: PaneId(1) }` |
| **期待値** | TOML 化 → デシリアライズで元と同一 |

```rust
#[test]
fn test_layout_leaf_roundtrip() {
    let node = LayoutNodeDef::Leaf { id: PaneId(1) };
    let toml = toml::to_string(&node).unwrap();
    let restored: LayoutNodeDef = toml::from_str(&toml).unwrap();
    assert_eq!(node, restored);
}
```

---

### TC-02: Split ノード（1段）のラウンドトリップ（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | Horizontal Split (ratio=0.5) of PaneId(1), PaneId(2) |
| **期待値** | ラウンドトリップで同一 |

```rust
#[test]
fn test_layout_split_roundtrip() {
    let node = LayoutNodeDef::Split {
        direction: SplitDirection::Horizontal,
        ratio: 0.5,
        first: Box::new(LayoutNodeDef::Leaf { id: PaneId(1) }),
        second: Box::new(LayoutNodeDef::Leaf { id: PaneId(2) }),
    };
    let toml = toml::to_string(&node).unwrap();
    let restored: LayoutNodeDef = toml::from_str(&toml).unwrap();
    assert_eq!(node, restored);
}
```

---

### TC-03: 3段ネストレイアウトのラウンドトリップ（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | Vertical Split → left: Leaf(1), right: Horizontal Split → Leaf(2), Leaf(3) |
| **期待値** | ラウンドトリップで同一 |

```rust
#[test]
fn test_layout_nested_roundtrip() {
    let node = LayoutNodeDef::Split {
        direction: SplitDirection::Vertical,
        ratio: 0.5,
        first: Box::new(LayoutNodeDef::Leaf { id: PaneId(1) }),
        second: Box::new(LayoutNodeDef::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.6,
            first: Box::new(LayoutNodeDef::Leaf { id: PaneId(2) }),
            second: Box::new(LayoutNodeDef::Leaf { id: PaneId(3) }),
        }),
    };
    let toml = toml::to_string(&node).unwrap();
    let restored: LayoutNodeDef = toml::from_str(&toml).unwrap();
    assert_eq!(node, restored);
}
```

---

### TC-04: LayoutSnapshot 全体のラウンドトリップ（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | root: Split ノード, active: PaneId(2) |
| **期待値** | ラウンドトリップで同一 |

```rust
#[test]
fn test_snapshot_roundtrip() {
    let snap = LayoutSnapshot {
        root: LayoutNodeDef::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNodeDef::Leaf { id: PaneId(1) }),
            second: Box::new(LayoutNodeDef::Leaf { id: PaneId(2) }),
        },
        active: PaneId(2),
    };
    let toml = toml::to_string(&snap).unwrap();
    let restored: LayoutSnapshot = toml::from_str(&toml).unwrap();
    assert_eq!(snap, restored);
}
```

---

### TC-05: 不正な TOML は Err を返す（エラー系）

| 項目 | 内容 |
|------|------|
| **入力** | `"broken toml {{{"` |
| **期待値** | `toml::from_str::<LayoutSnapshot>(&s).is_err()` |

```rust
#[test]
fn test_snapshot_invalid_toml_returns_err() {
    let result = toml::from_str::<LayoutSnapshot>("broken toml {{{");
    assert!(result.is_err());
}
```

---

### TC-06: LayoutNode → LayoutNodeDef 変換（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | `LayoutNode::Leaf(PaneId(5))` |
| **期待値** | `LayoutNodeDef::Leaf { id: PaneId(5) }` |
| **説明** | `From<&LayoutNode> for LayoutNodeDef` が正しく変換すること |

```rust
#[test]
fn test_layout_node_to_def_leaf() {
    let node = LayoutNode::Leaf(PaneId(5));
    let def = LayoutNodeDef::from(&node);
    assert_eq!(def, LayoutNodeDef::Leaf { id: PaneId(5) });
}
```

---

### TC-07: LayoutNode → LayoutNodeDef 変換（Split）

| 項目 | 内容 |
|------|------|
| **入力** | `LayoutNode::Split { direction: Vertical, ratio: 0.4, ... }` |
| **期待値** | `LayoutNodeDef::Split { direction: Vertical, ratio: 0.4, ... }` |

```rust
#[test]
fn test_layout_node_to_def_split() {
    let node = LayoutNode::Split {
        direction: SplitDirection::Vertical,
        ratio: 0.4,
        first: Box::new(LayoutNode::Leaf(PaneId(1))),
        second: Box::new(LayoutNode::Leaf(PaneId(2))),
    };
    let def = LayoutNodeDef::from(&node);
    match def {
        LayoutNodeDef::Split { direction, ratio, .. } => {
            assert_eq!(direction, SplitDirection::Vertical);
            assert!((ratio - 0.4).abs() < f32::EPSILON);
        }
        _ => panic!("Expected Split variant"),
    }
}
```

---

## 実装チェックリスト

- [ ] `crates/client/src/session.rs` を新規作成
- [ ] `LayoutNodeDef` と `LayoutSnapshot` を定義（serde derive 付き）
- [ ] `From<&LayoutNode> for LayoutNodeDef` を実装
- [ ] TC-01 〜 TC-07 がすべてグリーンになること
- [ ] `LayoutSnapshot::save(path)` → `toml::to_string` → `fs::write`
- [ ] `LayoutSnapshot::load(path)` → `fs::read_to_string` → `toml::from_str`
- [ ] `app.rs` の終了フックで `save()` を呼び出す

## 保存パス仕様

```
Windows: %APPDATA%\yatamux\session.toml
         = C:\Users\<user>\AppData\Roaming\yatamux\session.toml
```

取得方法: `std::env::var("APPDATA").unwrap_or_default()` + `\yatamux\session.toml`
