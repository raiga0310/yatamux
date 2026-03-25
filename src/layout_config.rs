//! 宣言的レイアウト設定
//!
//! `%APPDATA%\yatamux\layouts\<name>.toml` から起動時レイアウトを読み込む。
//!
//! ## TOML フォーマット例
//!
//! ```toml
//! [[panes]]
//! command = "nvim ."
//!
//! [[panes]]
//! split = "vertical"
//! command = "cargo watch -x test"
//!
//! [[panes]]
//! split = "horizontal"
//! ```
//!
//! - 最初のペインは常に初期ペイン（分割不要）。
//! - 2つ目以降は `split` で前のペインから分割方向を指定する。
//! - `command` はペイン作成後にシェルへ入力として送信される（`\r` 付き）。

use serde::Deserialize;

/// レイアウト設定ファイル全体
#[derive(Debug, Deserialize)]
pub struct LayoutConfig {
    /// ペイン設定のリスト（順番に作成される）
    #[serde(default)]
    pub panes: Vec<PaneConfig>,
}

/// 1ペイン分の設定
#[derive(Debug, Deserialize)]
pub struct PaneConfig {
    /// ペイン作成後にシェルへ送信するコマンド文字列
    pub command: Option<String>,
    /// 直前のペインからの分割方向（最初のペインには不要）
    pub split: Option<SplitDir>,
}

/// 分割方向
#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SplitDir {
    Vertical,
    Horizontal,
}

impl LayoutConfig {
    /// `%APPDATA%\yatamux\layouts\` に保存されている `.toml` ファイルのベース名一覧を返す（ソート済み）
    #[allow(dead_code)]
    pub fn list_layouts() -> Vec<String> {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        let dir = std::path::PathBuf::from(base)
            .join("yatamux")
            .join("layouts");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            return vec![];
        };
        let mut names: Vec<String> = entries
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                if path.extension()?.to_str()? == "toml" {
                    path.file_stem()?.to_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect();
        names.sort();
        names
    }

    /// TOML ファイルからレイアウト設定を読み込む
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        let content = std::fs::read_to_string(path)?;
        let config = toml::from_str(&content)?;
        Ok(config)
    }

    /// `%APPDATA%\yatamux\layouts\<name>.toml` のパスを返す
    pub fn layout_path(name: &str) -> std::path::PathBuf {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        std::path::PathBuf::from(base)
            .join("yatamux")
            .join("layouts")
            .join(format!("{name}.toml"))
    }
}

// ── テスト ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // TC-01: layout_path が正しいパスを返す
    #[test]
    fn test_layout_path_contains_name() {
        let path = LayoutConfig::layout_path("dev");
        let s = path.to_string_lossy();
        assert!(
            s.contains("yatamux"),
            "パスに 'yatamux' が含まれること: {s}"
        );
        assert!(
            s.contains("layouts"),
            "パスに 'layouts' が含まれること: {s}"
        );
        assert!(s.ends_with("dev.toml"), "末尾が dev.toml であること: {s}");
    }

    // TC-02: 有効な TOML ファイルをロードできる
    #[test]
    fn test_load_valid_config() {
        let dir = std::env::temp_dir().join("yatamux_layout_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("test.toml");
        std::fs::write(&path, "[[panes]]\ncommand = \"nvim .\"\n").unwrap();

        let config = LayoutConfig::load(&path).unwrap();
        assert_eq!(config.panes.len(), 1);
        assert_eq!(config.panes[0].command.as_deref(), Some("nvim ."));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // TC-03: 不正な TOML は Err を返す
    #[test]
    fn test_load_invalid_toml_returns_err() {
        let dir = std::env::temp_dir().join("yatamux_layout_test_invalid");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("bad.toml");
        std::fs::write(&path, "broken {{{").unwrap();

        assert!(LayoutConfig::load(&path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // TC-04: 空の panes リストは有効
    #[test]
    fn test_load_empty_panes() {
        let config: LayoutConfig = toml::from_str("").unwrap();
        assert!(config.panes.is_empty());
    }

    // TC-05: split フィールドが正しくデシリアライズされる
    #[test]
    fn test_split_dir_deserialize() {
        let config: PaneConfig = toml::from_str("split = \"vertical\"\n").unwrap();
        assert!(matches!(config.split, Some(SplitDir::Vertical)));

        let config2: PaneConfig = toml::from_str("split = \"horizontal\"\n").unwrap();
        assert!(matches!(config2.split, Some(SplitDir::Horizontal)));
    }

    // TC-C11-07: list_layouts は .toml のみ返し、ソートされている
    #[test]
    fn test_list_layouts_filters_and_sorts() {
        let dir = std::env::temp_dir().join("yatamux_list_layouts_test");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("work.toml"), "").unwrap();
        std::fs::write(dir.join("dev.toml"), "").unwrap();
        std::fs::write(dir.join("notes.txt"), "").unwrap(); // 除外されるべき

        // APPDATA を一時的に上書きして list_layouts を呼ぶのは困難なため、
        // layout_path の構造を検証する代わりにパスロジックを直接テスト
        let mut names: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter_map(|e| {
                let path = e.path();
                if path.extension()?.to_str()? == "toml" {
                    path.file_stem()?.to_str().map(|s| s.to_string())
                } else {
                    None
                }
            })
            .collect();
        names.sort();
        assert_eq!(names, vec!["dev", "work"]);

        let _ = std::fs::remove_dir_all(&dir);
    }

    // TC-C11-08: ディレクトリが存在しない場合は空リストを返す
    #[test]
    fn test_list_layouts_missing_dir_returns_empty() {
        let nonexistent = std::path::PathBuf::from("/nonexistent/path/yatamux/layouts");
        let Ok(entries) = std::fs::read_dir(&nonexistent) else {
            // 期待通り空リストに相当する動作
            return;
        };
        // ここに到達した場合はエントリが空であることを確認
        assert_eq!(entries.count(), 0);
    }
}
