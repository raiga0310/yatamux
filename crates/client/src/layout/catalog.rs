use serde::Deserialize;
use yatamux_protocol::types::{PaneId, SplitDirection};

use super::{LayoutNode, LayoutPreview};
use crate::Theme;

/// TOML 文字列からプレビュー用 `LayoutPreview` を構築する
fn build_preview_layout(content: &str) -> Option<LayoutPreview> {
    #[derive(Deserialize)]
    struct PreviewConfig {
        #[serde(default)]
        panes: Vec<PreviewPane>,
    }

    fn default_ratio() -> f32 {
        0.5
    }

    #[derive(Deserialize)]
    struct PreviewPane {
        split: Option<PreviewSplitDir>,
        command: Option<String>,
        #[serde(default = "default_ratio")]
        ratio: f32,
    }

    #[derive(Deserialize, Clone, Copy)]
    #[serde(rename_all = "lowercase")]
    enum PreviewSplitDir {
        Vertical,
        Horizontal,
    }

    let config: PreviewConfig = toml::from_str(content).ok()?;
    if config.panes.is_empty() {
        return None;
    }

    let commands: Vec<Option<String>> = config.panes.iter().map(|p| p.command.clone()).collect();
    let mut root = LayoutNode::Leaf(PaneId(0));
    for (i, pane) in config.panes.iter().enumerate().skip(1) {
        if let Some(split) = pane.split {
            let dir = match split {
                PreviewSplitDir::Vertical => SplitDirection::Vertical,
                PreviewSplitDir::Horizontal => SplitDirection::Horizontal,
            };
            root.split_leaf_with_ratio(PaneId((i - 1) as u32), PaneId(i as u32), dir, pane.ratio);
        }
    }

    Some(LayoutPreview {
        node: root,
        commands,
    })
}

/// `#rrggbb` or `rrggbb` を `0xRRGGBB` u32 に変換するローカルヘルパー
fn parse_hex_u32(s: &str) -> Option<u32> {
    let s = s.trim_start_matches('#');
    if s.len() != 6 {
        return None;
    }
    let r = u8::from_str_radix(&s[0..2], 16).ok()?;
    let g = u8::from_str_radix(&s[2..4], 16).ok()?;
    let b = u8::from_str_radix(&s[4..6], 16).ok()?;
    Some((r as u32) << 16 | (g as u32) << 8 | b as u32)
}

/// `%APPDATA%\yatamux\themes\` にある `.toml` ファイルのベース名一覧を返す（ソート済み）
pub fn list_available_themes() -> Vec<String> {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let dir = std::path::PathBuf::from(base)
        .join("yatamux")
        .join("themes");
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

/// テーマ TOML ファイルを読み込んで `Theme` を返す
///
/// ランタイム切り替えではフォント変更をサポートしないため、
/// `font_family` / `font_size` は常に `None` になる。
pub fn load_theme_from_file(name: &str) -> Option<Theme> {
    #[derive(serde::Deserialize, Default)]
    struct AppSec {
        background: Option<String>,
        foreground: Option<String>,
        cursor: Option<String>,
        selection_bg: Option<String>,
        status_bar_bg: Option<String>,
    }

    #[derive(serde::Deserialize, Default)]
    struct ThemeFile {
        #[serde(default)]
        appearance: AppSec,
    }

    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(base)
        .join("yatamux")
        .join("themes")
        .join(format!("{name}.toml"));
    let content = std::fs::read_to_string(&path).ok()?;
    let file: ThemeFile = toml::from_str(&content).ok()?;
    let ap = file.appearance;
    let parse = |s: &Option<String>| s.as_deref().and_then(parse_hex_u32);

    Some(Theme {
        bg: parse(&ap.background),
        fg: parse(&ap.foreground),
        cursor: parse(&ap.cursor),
        selection_bg: parse(&ap.selection_bg),
        status_bar_bg: parse(&ap.status_bar_bg),
        font_family: None,
        font_size: None,
        alert_border: None,
    })
}

/// `%APPDATA%\yatamux\layouts\` 内の `.toml` ファイルを読み込み、
/// `(名前, プレビューデータ)` のリストをソートして返す
pub fn list_available_layouts() -> Vec<(String, Option<LayoutPreview>)> {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let dir = std::path::PathBuf::from(base)
        .join("yatamux")
        .join("layouts");
    let Ok(dir_entries) = std::fs::read_dir(&dir) else {
        return vec![];
    };

    let mut results: Vec<(String, Option<LayoutPreview>)> = dir_entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let path = e.path();
            if path.extension()?.to_str()? == "toml" {
                let name = path.file_stem()?.to_str()?.to_string();
                let preview = std::fs::read_to_string(&path)
                    .ok()
                    .and_then(|c| build_preview_layout(&c));
                Some((name, preview))
            } else {
                None
            }
        })
        .collect();
    results.sort_by(|a, b| a.0.cmp(&b.0));
    results
}

/// `%APPDATA%\yatamux\layouts\<name>.toml` にレイアウト TOML を書き出す。
pub fn save_layout_file(name: &str, content: &str) -> std::io::Result<()> {
    let base = std::env::var("APPDATA").unwrap_or_else(|_| ".".to_string());
    let dir = std::path::PathBuf::from(base)
        .join("yatamux")
        .join("layouts");
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{name}.toml"));
    std::fs::write(path, content)
}
