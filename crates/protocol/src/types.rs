use serde::{Deserialize, Serialize};

/// ワークスペース ID (tmux の session 相当)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub u32);

/// サーフェス ID (ワークスペース内のタブ相当)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SurfaceId(pub u32);

/// ペイン ID (サーフェス内の分割ビュー)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u32);

/// ターミナルサイズ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    pub cols: u16,
    pub rows: u16,
}

/// ペイン分割方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    Horizontal,
    Vertical,
}

/// ペイン情報（list-panes レスポンス用）
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    pub id: PaneId,
    pub surface: SurfaceId,
    pub title: String,
    pub cols: u16,
    pub rows: u16,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    #[serde(default)]
    pub busy: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_unix_ms: Option<u64>,
    #[serde(default)]
    pub active: bool,
    #[serde(default)]
    pub floating: bool,
}

/// capture-pane のカーソル情報
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorInfo {
    pub col: u16,
    pub row: u16,
    pub visible: bool,
}

/// capture-pane の構造化メタデータ
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneCapture {
    pub title: String,
    pub cols: u16,
    pub rows: u16,
    pub lines_requested: usize,
    pub scrollback_len: usize,
    pub cursor: CursorInfo,
    pub visible_text: Vec<String>,
    pub scrollback_tail: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_info_roundtrip_preserves_active_and_floating() {
        let info = PaneInfo {
            id: PaneId(1),
            surface: SurfaceId(2),
            title: "shell".to_string(),
            cols: 80,
            rows: 24,
            alias: Some("tests".to_string()),
            role: Some("verifier".to_string()),
            cwd: Some("C:\\Users".to_string()),
            command: Some("pwsh".to_string()),
            busy: true,
            last_output_unix_ms: Some(1234),
            active: true,
            floating: true,
        };
        let json = serde_json::to_string(&info).expect("serialize PaneInfo");
        let restored: PaneInfo = serde_json::from_str(&json).expect("deserialize PaneInfo");
        assert_eq!(restored, info);
    }

    #[test]
    fn pane_info_old_json_defaults_active_and_floating_to_false() {
        let json = r#"{
            "id": 1,
            "surface": 2,
            "title": "shell",
            "cols": 80,
            "rows": 24
        }"#;
        let restored: PaneInfo = serde_json::from_str(json).expect("deserialize legacy PaneInfo");
        assert_eq!(restored.alias, None);
        assert_eq!(restored.role, None);
        assert!(!restored.active);
        assert!(!restored.floating);
        assert!(!restored.busy);
        assert_eq!(restored.last_output_unix_ms, None);
    }
}
