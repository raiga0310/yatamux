//! アプリケーション設定
//!
//! `%APPDATA%\yatamux\config.toml` から読み込む全体設定。
//!
//! ## フォーマット例
//!
//! ```toml
//! [hooks]
//! # ペイン作成時に cmd.exe /C で実行するコマンド
//! # 環境変数: YATAMUX_PANE_ID, YATAMUX_SESSION
//! on_pane_created = "echo %YATAMUX_PANE_ID% >> %TEMP%\\yatamux_events.log"
//!
//! # ペイン終了時に実行するコマンド
//! on_pane_closed = ""
//! ```

use serde::Deserialize;

/// アプリケーション全体設定
#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    /// イベントフック設定
    #[serde(default)]
    pub hooks: HooksConfig,
}

/// イベントフック設定
#[derive(Debug, Default, Deserialize)]
pub struct HooksConfig {
    /// ペイン作成時に実行するコマンド（空文字列 / None = 無効）
    pub on_pane_created: Option<String>,
    /// ペイン終了時に実行するコマンド（空文字列 / None = 無効）
    pub on_pane_closed: Option<String>,
}

impl HooksConfig {
    /// コマンドが実行可能かどうかを返す（None または空文字列は無効）
    pub fn is_enabled(cmd: &Option<String>) -> bool {
        cmd.as_deref().is_some_and(|s| !s.is_empty())
    }
}

impl AppConfig {
    /// TOML ファイルから設定を読み込む
    ///
    /// ファイルが存在しない場合はデフォルト設定を返す。
    /// TOML パースエラーの場合は `Err` を返す。
    pub fn load(path: &std::path::Path) -> anyhow::Result<Self> {
        match std::fs::read_to_string(path) {
            Ok(content) => {
                let config = toml::from_str(&content)?;
                Ok(config)
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    /// `%APPDATA%\yatamux\config.toml` のパスを返す
    pub fn default_path() -> std::path::PathBuf {
        let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
        std::path::PathBuf::from(base)
            .join("yatamux")
            .join("config.toml")
    }
}

// ── テスト ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // TC-01: 有効な config.toml を読み込める
    #[test]
    fn test_load_valid_config() {
        let dir = std::env::temp_dir().join("yatamux_config_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "[hooks]\non_pane_created = \"echo hi\"\n").unwrap();

        let config = AppConfig::load(&path).unwrap();
        assert_eq!(config.hooks.on_pane_created.as_deref(), Some("echo hi"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // TC-02: ファイルが存在しない場合はデフォルトを返す
    #[test]
    fn test_load_nonexistent_returns_default() {
        let path = std::env::temp_dir().join("yatamux_no_such_config.toml");
        let _ = std::fs::remove_file(&path); // 念のため削除
        let config = AppConfig::load(&path).unwrap();
        assert!(config.hooks.on_pane_created.is_none());
        assert!(config.hooks.on_pane_closed.is_none());
    }

    // TC-03: 不正な TOML は Err を返す
    #[test]
    fn test_load_invalid_toml_returns_err() {
        let dir = std::env::temp_dir().join("yatamux_config_bad");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(&path, "broken {{{").unwrap();

        assert!(AppConfig::load(&path).is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }

    // TC-04: default_path が正しいパスを返す
    #[test]
    fn test_default_path() {
        let path = AppConfig::default_path();
        let s = path.to_string_lossy();
        assert!(
            s.contains("yatamux"),
            "パスに 'yatamux' が含まれること: {s}"
        );
        assert!(
            s.ends_with("config.toml"),
            "末尾が config.toml であること: {s}"
        );
    }

    // is_enabled のテスト
    #[test]
    fn test_is_enabled_none_is_false() {
        assert!(!HooksConfig::is_enabled(&None));
    }

    #[test]
    fn test_is_enabled_empty_is_false() {
        assert!(!HooksConfig::is_enabled(&Some(String::new())));
    }

    #[test]
    fn test_is_enabled_nonempty_is_true() {
        assert!(HooksConfig::is_enabled(&Some("echo hi".to_string())));
    }
}
