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
//!
//! [appearance]
//! # フォントファミリー（省略時: インストール済み候補から自動選択）
//! font_family = "HackGen Console NF"
//! # フォントサイズ（pt、省略時: 15pt 相当）
//! font_size = 14
//! # 背景色（省略時: Catppuccin Mocha base #1e1e2e）
//! background = "#1e1e2e"
//! # 前景色（省略時: Catppuccin Mocha text #cdd6f4）
//! foreground = "#cdd6f4"
//! # カーソル色（省略時: Catppuccin Mocha pink #f5c2e7）
//! cursor = "#f5c2e7"
//! # テキスト選択背景色
//! selection_bg = "#585b70"
//! # ステータスバー背景色（省略時: Catppuccin Mocha mantle #181825）
//! status_bar_bg = "#181825"
//! ```

use serde::Deserialize;

/// アプリケーション全体設定
#[derive(Debug, Default, Deserialize)]
pub struct AppConfig {
    /// イベントフック設定
    #[serde(default)]
    pub hooks: HooksConfig,
    /// 外観設定
    #[serde(default)]
    pub appearance: AppearanceConfig,
    /// ステータスバー設定
    #[serde(default)]
    pub status_bar: StatusBarConfig,
}

/// ステータスバー設定
///
/// ```toml
/// [status_bar]
/// news_rss = "https://www.sankei.com/rss/news/flash/home-flash.xml"
/// news_interval_secs = 120
/// news_scroll_px_per_tick = 2
/// ```
#[derive(Debug, Deserialize)]
pub struct StatusBarConfig {
    /// ニュースティッカーに使う RSS フィード URL（省略時: ティッカーなし）
    pub news_rss: Option<String>,
    /// RSS 再取得間隔（秒、デフォルト: 120）
    #[serde(default = "StatusBarConfig::default_interval")]
    pub news_interval_secs: u64,
    /// WM_TIMER 1 ティック（≈16ms）あたりのスクロール量（px、デフォルト: 2）
    #[serde(default = "StatusBarConfig::default_scroll_px")]
    pub news_scroll_px_per_tick: i32,
}

impl StatusBarConfig {
    fn default_interval() -> u64 { 120 }
    fn default_scroll_px() -> i32 { 2 }
}

impl Default for StatusBarConfig {
    fn default() -> Self {
        Self {
            news_rss: None,
            news_interval_secs: Self::default_interval(),
            news_scroll_px_per_tick: Self::default_scroll_px(),
        }
    }
}

/// 外観設定（フォント・カラーテーマ）
#[derive(Debug, Default, Deserialize)]
pub struct AppearanceConfig {
    /// フォントファミリー（省略時: 候補リストから自動選択）
    pub font_family: Option<String>,
    /// フォントサイズ（pt、省略時: 15pt 相当）
    pub font_size: Option<u32>,
    /// 背景色（`"#rrggbb"` 形式）
    pub background: Option<String>,
    /// 前景色（`"#rrggbb"` 形式）
    pub foreground: Option<String>,
    /// カーソル色（`"#rrggbb"` 形式）
    pub cursor: Option<String>,
    /// テキスト選択背景色（`"#rrggbb"` 形式）
    pub selection_bg: Option<String>,
    /// ステータスバー背景色（`"#rrggbb"` 形式）
    pub status_bar_bg: Option<String>,
}

/// `"#rrggbb"` 形式の文字列を `(r, g, b)` に変換する。
/// `#` プレフィックスは省略可能。パース失敗時は `None` を返す。
pub fn parse_hex_color(s: &str) -> Option<(u8, u8, u8)> {
    let s = s.trim_start_matches('#');
    if s.len() == 6 {
        let r = u8::from_str_radix(&s[0..2], 16).ok()?;
        let g = u8::from_str_radix(&s[2..4], 16).ok()?;
        let b = u8::from_str_radix(&s[4..6], 16).ok()?;
        Some((r, g, b))
    } else {
        None
    }
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
    /// フックコマンドが有効かどうかを判定する。
    ///
    /// `None`、空文字列、空白のみの文字列は無効扱いにする。
    pub fn is_enabled(command: &Option<String>) -> bool {
        command
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
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

    // TC-C21-01: AppearanceConfig を含む config.toml を読み込める
    #[test]
    fn test_load_appearance_config() {
        let dir = std::env::temp_dir().join("yatamux_appearance_test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            "[appearance]\nfont_family = \"HackGen Console NF\"\nfont_size = 14\nbackground = \"#1e1e2e\"\n",
        )
        .unwrap();

        let config = AppConfig::load(&path).unwrap();
        assert_eq!(
            config.appearance.font_family.as_deref(),
            Some("HackGen Console NF")
        );
        assert_eq!(config.appearance.font_size, Some(14));
        assert_eq!(config.appearance.background.as_deref(), Some("#1e1e2e"));

        let _ = std::fs::remove_dir_all(&dir);
    }

    // TC-C21-02: parse_hex_color が # 付きで正しくパースする
    #[test]
    fn test_parse_hex_color_with_hash() {
        assert_eq!(parse_hex_color("#1e1e2e"), Some((0x1e, 0x1e, 0x2e)));
        assert_eq!(parse_hex_color("#cdd6f4"), Some((0xcd, 0xd6, 0xf4)));
    }

    // TC-C21-03: parse_hex_color が # なしでもパースする
    #[test]
    fn test_parse_hex_color_without_hash() {
        assert_eq!(parse_hex_color("f5c2e7"), Some((0xf5, 0xc2, 0xe7)));
    }

    // TC-C21-04: parse_hex_color が不正な文字列で None を返す
    #[test]
    fn test_parse_hex_color_invalid() {
        assert_eq!(parse_hex_color("gggggg"), None);
        assert_eq!(parse_hex_color("#fff"), None);
        assert_eq!(parse_hex_color(""), None);
    }

    // TC-C21-05: [appearance] がない場合はデフォルト値を返す
    #[test]
    fn test_appearance_defaults_when_absent() {
        let config: AppConfig = toml::from_str("").unwrap();
        assert!(config.appearance.font_family.is_none());
        assert!(config.appearance.background.is_none());
    }
}
