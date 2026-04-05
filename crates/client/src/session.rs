//! セッション永続化
//!
//! ペインレイアウトを TOML にシリアライズし、
//! `%APPDATA%\yatamux\session.toml` に保存・読み込みする。
//!
//! ## 設計
//!
//! `LayoutNode` は `Arc<Mutex<Grid>>` を含むためシリアライズ不可。
//! グリッドを持たない `LayoutNodeDef` / `LayoutSnapshot` を別途定義し、
//! `From<&LayoutNode>` で変換してシリアライズする。

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use yatamux_protocol::types::{PaneId, SplitDirection};

use crate::layout::{LayoutNode, PaneStore};

/// シリアライズ可能なレイアウトノード（グリッドを含まない）
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum LayoutNodeDef {
    Leaf {
        id: PaneId,
        /// 復元時に自動実行するコマンド（layout.toml や layout switch 経由で設定された場合のみ）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<String>,
    },
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<LayoutNodeDef>,
        second: Box<LayoutNodeDef>,
    },
}

/// シリアライズ可能なレイアウトスナップショット
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutSnapshot {
    pub root: LayoutNodeDef,
    pub active: PaneId,
}

impl From<&LayoutNode> for LayoutNodeDef {
    fn from(node: &LayoutNode) -> Self {
        Self::from_with_commands(node, &HashMap::new())
    }
}

impl LayoutNodeDef {
    /// `pane_commands` を参照しながら変換する。各 Leaf にコマンドを埋め込む。
    pub fn from_with_commands(node: &LayoutNode, cmds: &HashMap<PaneId, String>) -> Self {
        match node {
            LayoutNode::Leaf(id) => LayoutNodeDef::Leaf {
                id: *id,
                command: cmds.get(id).cloned(),
            },
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => LayoutNodeDef::Split {
                direction: *direction,
                ratio: *ratio,
                first: Box::new(Self::from_with_commands(first, cmds)),
                second: Box::new(Self::from_with_commands(second, cmds)),
            },
        }
    }
}

/// `PaneStore` の現在状態を `session.toml` に保存する。
///
/// `WM_CLOSE` および `SaveAndQuit` の両方から呼ばれる共通関数。
/// `pane_commands` を Leaf ノードに埋め込んで保存するため、次回起動時に復元できる。
pub fn save_session(store: &PaneStore, path: &std::path::Path) {
    let snap = LayoutSnapshot {
        root: LayoutNodeDef::from_with_commands(&store.layout, &store.pane_commands),
        active: store.active,
    };
    if let Err(e) = snap.save(path) {
        tracing::warn!("セッション保存に失敗: {}", e);
    }
}

impl LayoutSnapshot {
    /// TOML 文字列として直列化する
    pub fn to_toml(&self) -> Result<String, toml::ser::Error> {
        toml::to_string(self)
    }

    /// TOML 文字列から復元する
    pub fn from_toml(s: &str) -> Result<Self, toml::de::Error> {
        toml::from_str(s)
    }

    /// ファイルに保存する
    pub fn save(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let toml = self.to_toml().map_err(std::io::Error::other)?;
        std::fs::write(path, toml)
    }

    /// ファイルから読み込む
    pub fn load(path: &std::path::Path) -> std::io::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        Self::from_toml(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    /// `%APPDATA%\yatamux\session.toml` のパスを返す
    pub fn default_path() -> std::path::PathBuf {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        std::path::PathBuf::from(base)
            .join("yatamux")
            .join("session.toml")
    }
}

// ── テスト ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use yatamux_protocol::types::{PaneId, SplitDirection};

    // TC-01: Leaf ノードの TOML ラウンドトリップ
    #[test]
    fn test_layout_leaf_roundtrip() {
        let node = LayoutNodeDef::Leaf { id: PaneId(1), command: None };
        let toml = toml::to_string(&node).unwrap();
        let restored: LayoutNodeDef = toml::from_str(&toml).unwrap();
        assert_eq!(node, restored);
    }

    // TC-02: Split ノード（1段）のラウンドトリップ
    #[test]
    fn test_layout_split_roundtrip() {
        let node = LayoutNodeDef::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNodeDef::Leaf { id: PaneId(1), command: None }),
            second: Box::new(LayoutNodeDef::Leaf { id: PaneId(2), command: None }),
        };
        let toml = toml::to_string(&node).unwrap();
        let restored: LayoutNodeDef = toml::from_str(&toml).unwrap();
        assert_eq!(node, restored);
    }

    // TC-03: 3段ネストレイアウトのラウンドトリップ
    #[test]
    fn test_layout_nested_roundtrip() {
        let node = LayoutNodeDef::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNodeDef::Leaf { id: PaneId(1), command: None }),
            second: Box::new(LayoutNodeDef::Split {
                direction: SplitDirection::Horizontal,
                ratio: 0.6,
                first: Box::new(LayoutNodeDef::Leaf { id: PaneId(2), command: None }),
                second: Box::new(LayoutNodeDef::Leaf { id: PaneId(3), command: None }),
            }),
        };
        let toml = toml::to_string(&node).unwrap();
        let restored: LayoutNodeDef = toml::from_str(&toml).unwrap();
        assert_eq!(node, restored);
    }

    // TC-04: LayoutSnapshot 全体のラウンドトリップ
    #[test]
    fn test_snapshot_roundtrip() {
        let snap = LayoutSnapshot {
            root: LayoutNodeDef::Split {
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNodeDef::Leaf { id: PaneId(1), command: None }),
                second: Box::new(LayoutNodeDef::Leaf { id: PaneId(2), command: None }),
            },
            active: PaneId(2),
        };
        let toml = snap.to_toml().unwrap();
        let restored = LayoutSnapshot::from_toml(&toml).unwrap();
        assert_eq!(snap, restored);
    }

    // TC-05: 不正な TOML は Err を返す
    #[test]
    fn test_snapshot_invalid_toml_returns_err() {
        let result = LayoutSnapshot::from_toml("broken toml {{{");
        assert!(result.is_err());
    }

    // TC-06: LayoutNode::Leaf → LayoutNodeDef::Leaf 変換
    #[test]
    fn test_layout_node_to_def_leaf() {
        let node = LayoutNode::Leaf(PaneId(5));
        let def = LayoutNodeDef::from(&node);
        assert_eq!(def, LayoutNodeDef::Leaf { id: PaneId(5), command: None });
    }

    // TC-07: LayoutNode::Split → LayoutNodeDef::Split 変換
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
            LayoutNodeDef::Split {
                direction, ratio, ..
            } => {
                assert_eq!(direction, SplitDirection::Vertical);
                assert!((ratio - 0.4).abs() < f32::EPSILON);
            }
            _ => panic!("Expected Split variant"),
        }
    }

    // TC-07: save_session 関数のテスト（PaneStore → session.toml）
    #[test]
    fn test_save_session_writes_file() {
        use crate::layout::PaneStore;
        use std::sync::{Arc, Mutex};
        use yatamux_protocol::types::PaneId;
        use yatamux_terminal::CjkWidthConfig;

        // ダミーの PaneStore を作成
        let grid = Arc::new(Mutex::new(yatamux_terminal::Grid::new(
            80,
            24,
            CjkWidthConfig::default(),
        )));
        let store = PaneStore::new(PaneId(1), grid);

        let dir = std::env::temp_dir().join("yatamux_test_save_session");
        let path = dir.join("session.toml");
        save_session(&store, &path);

        // ファイルが作成されていること
        assert!(path.exists(), "session.toml が作成されること");
        let loaded = LayoutSnapshot::load(&path).expect("読み込みに成功すること");
        assert_eq!(loaded.active, PaneId(1));
        let _ = std::fs::remove_dir_all(dir);
    }

    // TC-08: save → load ファイルラウンドトリップ
    #[test]
    fn test_snapshot_file_roundtrip() {
        let snap = LayoutSnapshot {
            root: LayoutNodeDef::Leaf { id: PaneId(1), command: None },
            active: PaneId(1),
        };
        let dir = std::env::temp_dir().join("yatamux_test");
        let path = dir.join("session.toml");
        snap.save(&path).unwrap();
        let loaded = LayoutSnapshot::load(&path).unwrap();
        assert_eq!(snap, loaded);
        let _ = std::fs::remove_dir_all(dir);
    }

    // TC-09: default_path が %APPDATA%\yatamux\session.toml を返す
    #[test]
    fn test_default_path_contains_yatamux() {
        let path = LayoutSnapshot::default_path();
        let s = path.to_string_lossy();
        assert!(
            s.contains("yatamux"),
            "パスに 'yatamux' が含まれること: {}",
            s
        );
        assert!(
            s.ends_with("session.toml"),
            "末尾が session.toml であること: {}",
            s
        );
    }
}
