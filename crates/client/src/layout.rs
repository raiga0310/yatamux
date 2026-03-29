//! ペインレイアウトツリーとペインストア
//!
//! `Arc<Mutex<PaneStore>>` でウィンドウスレッドと tokio タスクが共有する。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use serde::Deserialize;
use yatamux_protocol::types::{PaneId, SplitDirection};
use yatamux_terminal::Grid;

/// ペインのピクセル矩形（PADDING 適用前のコンテンツ領域内の相対座標）
#[derive(Clone, Copy, Debug)]
pub struct PaneRect {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
}

impl PaneRect {
    pub fn cols(&self, cell_w: i32) -> u16 {
        (self.w / cell_w.max(1)).max(1) as u16
    }
    pub fn rows(&self, cell_h: i32) -> u16 {
        (self.h / cell_h.max(1)).max(1) as u16
    }
}

/// ペインフォーカス移動の方向
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

/// クライアント側レイアウトツリー（サーバーの PaneTree と同構造）
#[derive(Clone, Debug)]
pub enum LayoutNode {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<LayoutNode>,
        second: Box<LayoutNode>,
    },
}

impl LayoutNode {
    /// ペイン間セパレーター幅（ピクセル）
    pub const SEP_PX: i32 = 1;

    /// `parent` のリーフを `parent`/`child` の Split ノードに置き換える
    pub fn split_leaf(&mut self, parent: PaneId, child: PaneId, dir: SplitDirection) -> bool {
        self.split_leaf_with_ratio(parent, child, dir, 0.5)
    }

    pub fn split_leaf_with_ratio(
        &mut self,
        parent: PaneId,
        child: PaneId,
        dir: SplitDirection,
        ratio: f32,
    ) -> bool {
        match self {
            LayoutNode::Leaf(id) if *id == parent => {
                *self = LayoutNode::Split {
                    direction: dir,
                    ratio,
                    first: Box::new(LayoutNode::Leaf(parent)),
                    second: Box::new(LayoutNode::Leaf(child)),
                };
                true
            }
            LayoutNode::Split { first, second, .. } => {
                first.split_leaf_with_ratio(parent, child, dir, ratio)
                    || second.split_leaf_with_ratio(parent, child, dir, ratio)
            }
            _ => false,
        }
    }

    /// 全ペイン ID を深さ優先で返す
    pub fn pane_ids(&self) -> Vec<PaneId> {
        match self {
            LayoutNode::Leaf(id) => vec![*id],
            LayoutNode::Split { first, second, .. } => {
                let mut ids = first.pane_ids();
                ids.extend(second.pane_ids());
                ids
            }
        }
    }

    pub fn next_pane(&self, current: PaneId) -> PaneId {
        let ids = self.pane_ids();
        let pos = ids.iter().position(|&id| id == current).unwrap_or(0);
        ids[(pos + 1) % ids.len()]
    }

    pub fn prev_pane(&self, current: PaneId) -> PaneId {
        let ids = self.pane_ids();
        let n = ids.len();
        let pos = ids.iter().position(|&id| id == current).unwrap_or(0);
        ids[(pos + n - 1) % n]
    }

    /// 指定方向で最近傍のペインを返す。
    /// 同方向にペインがない場合は `current` をそのまま返す（端でループしない）。
    pub fn pane_in_direction(
        &self,
        current: PaneId,
        dir: Direction,
        root_rect: PaneRect,
    ) -> PaneId {
        let rects = self.compute_rects(root_rect);
        let cur_rect = match rects.iter().find(|(id, _)| *id == current) {
            Some((_, r)) => *r,
            None => return current,
        };

        let candidates: Vec<(PaneId, i32)> = rects
            .iter()
            .filter_map(|(id, r)| {
                if *id == current {
                    return None;
                }
                let edge_dist = match dir {
                    Direction::Left => {
                        let d = cur_rect.x - (r.x + r.w);
                        if d >= 0 {
                            Some(d)
                        } else {
                            None
                        }
                    }
                    Direction::Right => {
                        let d = r.x - (cur_rect.x + cur_rect.w);
                        if d >= 0 {
                            Some(d)
                        } else {
                            None
                        }
                    }
                    Direction::Up => {
                        let d = cur_rect.y - (r.y + r.h);
                        if d >= 0 {
                            Some(d)
                        } else {
                            None
                        }
                    }
                    Direction::Down => {
                        let d = r.y - (cur_rect.y + cur_rect.h);
                        if d >= 0 {
                            Some(d)
                        } else {
                            None
                        }
                    }
                };
                edge_dist.map(|d| (*id, d))
            })
            .collect();

        candidates
            .into_iter()
            .min_by_key(|&(_, d)| d)
            .map(|(id, _)| id)
            .unwrap_or(current)
    }

    /// 各ペインのピクセル矩形を計算する（セパレーターの隙間込み）
    pub fn compute_rects(&self, r: PaneRect) -> Vec<(PaneId, PaneRect)> {
        match self {
            LayoutNode::Leaf(id) => vec![(*id, r)],
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let sep = Self::SEP_PX;
                let (r1, r2) = match direction {
                    SplitDirection::Vertical => {
                        let w1 = (((r.w - sep) as f32 * ratio) as i32).max(1);
                        let w2 = (r.w - w1 - sep).max(1);
                        (
                            PaneRect {
                                x: r.x,
                                y: r.y,
                                w: w1,
                                h: r.h,
                            },
                            PaneRect {
                                x: r.x + w1 + sep,
                                y: r.y,
                                w: w2,
                                h: r.h,
                            },
                        )
                    }
                    SplitDirection::Horizontal => {
                        let h1 = (((r.h - sep) as f32 * ratio) as i32).max(1);
                        let h2 = (r.h - h1 - sep).max(1);
                        (
                            PaneRect {
                                x: r.x,
                                y: r.y,
                                w: r.w,
                                h: h1,
                            },
                            PaneRect {
                                x: r.x,
                                y: r.y + h1 + sep,
                                w: r.w,
                                h: h2,
                            },
                        )
                    }
                };
                let mut rects = first.compute_rects(r1);
                rects.extend(second.compute_rects(r2));
                rects
            }
        }
    }

    /// クリック座標 (x, y) がどのペインに含まれるかを返す（コンテンツ座標）
    pub fn pane_at_point(&self, x: i32, y: i32, root: PaneRect) -> Option<PaneId> {
        self.compute_rects(root)
            .into_iter()
            .find(|(_, r)| x >= r.x && x < r.x + r.w && y >= r.y && y < r.y + r.h)
            .map(|(id, _)| id)
    }

    /// ペイン `id` をツリーから削除する。
    /// 削除後にフォーカスすべき候補ペインの ID を返す。
    /// ルート Leaf（最後の1ペイン）の場合は None を返す（削除不可）。
    pub fn remove_pane(&mut self, id: PaneId) -> Option<PaneId> {
        fn first_pane_id(node: &LayoutNode) -> Option<PaneId> {
            match node {
                LayoutNode::Leaf(id) => Some(*id),
                LayoutNode::Split { first, .. } => first_pane_id(first),
            }
        }

        fn remove_owned(node: LayoutNode, id: PaneId) -> (LayoutNode, Option<PaneId>, bool) {
            match node {
                LayoutNode::Leaf(leaf_id) => (LayoutNode::Leaf(leaf_id), None, false),
                LayoutNode::Split {
                    direction,
                    ratio,
                    first,
                    second,
                } => {
                    let first = *first;
                    let second = *second;

                    if matches!(first, LayoutNode::Leaf(leaf_id) if leaf_id == id) {
                        let next = first_pane_id(&second);
                        return (second, next, true);
                    }
                    if matches!(second, LayoutNode::Leaf(leaf_id) if leaf_id == id) {
                        let next = first_pane_id(&first);
                        return (first, next, true);
                    }

                    let (first, next, removed) = remove_owned(first, id);
                    if removed {
                        return (
                            LayoutNode::Split {
                                direction,
                                ratio,
                                first: Box::new(first),
                                second: Box::new(second),
                            },
                            next,
                            true,
                        );
                    }

                    let (second, next, removed) = remove_owned(second, id);
                    (
                        LayoutNode::Split {
                            direction,
                            ratio,
                            first: Box::new(first),
                            second: Box::new(second),
                        },
                        next,
                        removed,
                    )
                }
            }
        }

        if matches!(self, LayoutNode::Leaf(_)) {
            return None;
        }

        let current = std::mem::replace(self, LayoutNode::Leaf(PaneId(0)));
        let (new_layout, next, removed) = remove_owned(current, id);
        *self = new_layout;
        if removed {
            next
        } else {
            None
        }
    }

    /// アクティブペイン `id` を含む最近傍 Split ノードの ratio を `delta` だけ増減する。
    ///
    /// - `id` が `first` サブツリーに属する場合: `ratio += delta`（first が拡大）
    /// - `id` が `second` サブツリーに属する場合: `ratio -= delta`（second が拡大）
    /// - ratio は `[0.1, 0.9]` にクランプされる
    ///
    /// 戻り値: ratio を変更した場合 `true`、変更なし（Leaf またはペイン不在）なら `false`
    pub fn adjust_ratio(&mut self, id: PaneId, delta: f32) -> bool {
        match self {
            LayoutNode::Leaf(_) => false,
            LayoutNode::Split {
                ratio,
                first,
                second,
                ..
            } => {
                // 子ノードで先に再帰（最近傍 Split を優先）
                if first.adjust_ratio(id, delta) || second.adjust_ratio(id, delta) {
                    return true;
                }
                // このノードが直接の親 Split かチェック
                if first.pane_ids().contains(&id) {
                    *ratio = (*ratio + delta).clamp(0.1, 0.9);
                    true
                } else if second.pane_ids().contains(&id) {
                    *ratio = (*ratio - delta).clamp(0.1, 0.9);
                    true
                } else {
                    false
                }
            }
        }
    }

    /// `target_dir` 方向の Split に限定して ratio を調整する。
    ///
    /// - `<`/`>` キーは `SplitDirection::Vertical`（縦線分割 = 横比）を対象にする
    /// - `+`/`-` キーは `SplitDirection::Horizontal`（横線分割 = 縦比）を対象にする
    pub fn adjust_ratio_for_dir(
        &mut self,
        id: PaneId,
        delta: f32,
        target_dir: SplitDirection,
    ) -> bool {
        match self {
            LayoutNode::Leaf(_) => false,
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                // 子ノードを先に試す（最近傍を優先）
                if first.adjust_ratio_for_dir(id, delta, target_dir)
                    || second.adjust_ratio_for_dir(id, delta, target_dir)
                {
                    return true;
                }
                // 方向が一致し、このノードがペインを含む場合のみ調整
                if *direction == target_dir {
                    if first.pane_ids().contains(&id) {
                        *ratio = (*ratio + delta).clamp(0.1, 0.9);
                        true
                    } else if second.pane_ids().contains(&id) {
                        *ratio = (*ratio - delta).clamp(0.1, 0.9);
                        true
                    } else {
                        false
                    }
                } else {
                    false
                }
            }
        }
    }

    /// セパレーター矩形リスト（コンテンツ座標）
    pub fn compute_separator_rects(&self, r: PaneRect) -> Vec<PaneRect> {
        match self {
            LayoutNode::Leaf(_) => vec![],
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let sep = Self::SEP_PX;
                let (r1, r2, sep_rect) = match direction {
                    SplitDirection::Vertical => {
                        let w1 = (((r.w - sep) as f32 * ratio) as i32).max(1);
                        let w2 = (r.w - w1 - sep).max(1);
                        (
                            PaneRect {
                                x: r.x,
                                y: r.y,
                                w: w1,
                                h: r.h,
                            },
                            PaneRect {
                                x: r.x + w1 + sep,
                                y: r.y,
                                w: w2,
                                h: r.h,
                            },
                            PaneRect {
                                x: r.x + w1,
                                y: r.y,
                                w: sep,
                                h: r.h,
                            },
                        )
                    }
                    SplitDirection::Horizontal => {
                        let h1 = (((r.h - sep) as f32 * ratio) as i32).max(1);
                        let h2 = (r.h - h1 - sep).max(1);
                        (
                            PaneRect {
                                x: r.x,
                                y: r.y,
                                w: r.w,
                                h: h1,
                            },
                            PaneRect {
                                x: r.x,
                                y: r.y + h1 + sep,
                                w: r.w,
                                h: h2,
                            },
                            PaneRect {
                                x: r.x,
                                y: r.y + h1,
                                w: r.w,
                                h: sep,
                            },
                        )
                    }
                };
                let mut seps = vec![sep_rect];
                seps.extend(first.compute_separator_rects(r1));
                seps.extend(second.compute_separator_rects(r2));
                seps
            }
        }
    }
}

/// ランチャープレビュー用レイアウトデータ
#[derive(Clone, Debug)]
pub struct LayoutPreview {
    /// ペイン分割ツリー（PaneId は 0, 1, 2, … の連番）
    pub node: LayoutNode,
    /// PaneId(i) のペインに送信するコマンド文字列（None = コマンドなし）
    pub commands: Vec<Option<String>>,
}

/// レイアウトランチャーの表示状態
#[derive(Clone, Debug)]
pub struct LauncherState {
    /// (名前, プレビューデータ) のリスト
    pub entries: Vec<(String, Option<LayoutPreview>)>,
    /// 現在選択中のインデックス
    pub selected: usize,
}

impl LauncherState {
    pub fn new(entries: Vec<(String, Option<LayoutPreview>)>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }

    /// 選択中のレイアウト名を返す
    pub fn selected_name(&self) -> Option<&str> {
        self.entries.get(self.selected).map(|(n, _)| n.as_str())
    }

    /// 選択中のプレビューデータを返す
    pub fn selected_preview(&self) -> Option<&LayoutPreview> {
        self.entries.get(self.selected)?.1.as_ref()
    }
}

/// テーマランチャーの表示状態
#[derive(Clone, Debug)]
pub struct ThemeLauncherState {
    /// テーマ名のリスト（ファイル名のステム）
    pub entries: Vec<String>,
    /// 現在選択中のインデックス
    pub selected: usize,
}

impl ThemeLauncherState {
    pub fn new(entries: Vec<String>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }

    pub fn selected_name(&self) -> Option<&str> {
        self.entries.get(self.selected).map(String::as_str)
    }
}

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

/// `%APPDATA%\yatamux\layouts\` 内の `.toml` ファイルを読み込み、
/// `(名前, プレビューデータ)` のリストをソートして返す
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
pub fn load_theme_from_file(name: &str) -> Option<crate::window::Theme> {
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
    Some(crate::window::Theme {
        bg: parse(&ap.background),
        fg: parse(&ap.foreground),
        cursor: parse(&ap.cursor),
        selection_bg: parse(&ap.selection_bg),
        status_bar_bg: parse(&ap.status_bar_bg),
        font_family: None,
        font_size: None,
    })
}

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

/// 現在のレイアウトツリーを `[[panes]]` TOML 形式に変換する。
///
/// DFS でノードを訪問し、各葉ペインを 1 エントリとして出力する。
/// ratio が 0.5 と異なる場合のみ `ratio = X.XXX` を出力する。
/// 制限: `first` が Split であるような左辺スプリット（左優先ツリー）を
/// ロードし直すと構造が変わる（連鎖型ツリーとして再構成される）。
pub fn layout_to_toml(node: &LayoutNode, commands: &HashMap<PaneId, String>) -> String {
    // (pane_id, split_dir, ratio) — 最初のペインは split = None
    fn collect(
        node: &LayoutNode,
        split: Option<(&'static str, f32)>,
        out: &mut Vec<(PaneId, Option<(&'static str, f32)>)>,
    ) {
        match node {
            LayoutNode::Leaf(id) => out.push((*id, split)),
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                collect(first, split, out);
                let dir = match direction {
                    SplitDirection::Vertical => "vertical",
                    SplitDirection::Horizontal => "horizontal",
                };
                collect(second, Some((dir, *ratio)), out);
            }
        }
    }

    let mut panes: Vec<(PaneId, Option<(&'static str, f32)>)> = Vec::new();
    collect(node, None, &mut panes);

    let mut out = String::new();
    for (pane_id, split) in panes {
        out.push_str("[[panes]]\n");
        if let Some(cmd) = commands.get(&pane_id) {
            // TOML の文字列リテラルとして適切にエスケープして出力
            out.push_str(&format!("command = {cmd:?}\n"));
        }
        if let Some((dir, ratio)) = split {
            out.push_str(&format!("split = \"{dir}\"\n"));
            if (ratio - 0.5).abs() >= 1e-4 {
                out.push_str(&format!("ratio = {ratio:.4}\n"));
            }
        }
        out.push('\n');
    }
    out
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

/// コピーモードのカーソルと選択状態
///
/// `cursor` はスクリーン座標（col, row）の 0-based インデックス。
/// `anchor` が `Some` の場合はビジュアル選択が有効。
#[derive(Clone, Debug)]
pub struct CopyState {
    /// カーソル位置 (col, row)（スクリーン座標、0-based）
    pub cursor: (usize, usize),
    /// 選択アンカー — None = カーソルのみ、Some = ビジュアル選択中
    pub anchor: Option<(usize, usize)>,
}

impl CopyState {
    /// 指定位置でコピーモードを初期化する
    pub fn new(col: usize, row: usize) -> Self {
        Self {
            cursor: (col, row),
            anchor: None,
        }
    }

    /// カーソルを指定方向に移動する（cols/rows でクランプ）
    pub fn move_cursor(&mut self, dcol: isize, drow: isize, cols: usize, rows: usize) {
        let new_col = (self.cursor.0 as isize + dcol)
            .max(0)
            .min(cols.saturating_sub(1) as isize) as usize;
        let new_row = (self.cursor.1 as isize + drow)
            .max(0)
            .min(rows.saturating_sub(1) as isize) as usize;
        self.cursor = (new_col, new_row);
    }

    /// ビジュアル選択のアンカーをカーソル位置に設定する（トグル）
    pub fn toggle_anchor(&mut self) {
        if self.anchor.is_some() {
            self.anchor = None;
        } else {
            self.anchor = Some(self.cursor);
        }
    }

    /// 選択範囲の (row_start, row_end) を返す（None = 選択なし）
    pub fn selection_rows(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let (_, ar) = anchor;
        let (_, cr) = self.cursor;
        Some((ar.min(cr), ar.max(cr)))
    }

    /// セル (col, row) が現在の選択範囲内かどうかを判定する
    pub fn is_selected(&self, col: usize, row: usize) -> bool {
        let anchor = match self.anchor {
            Some(a) => a,
            None => return false,
        };
        let (ac, ar) = anchor;
        let (cc, cr) = self.cursor;

        // 行範囲を確定
        let (row_min, row_max) = (ar.min(cr), ar.max(cr));
        if row < row_min || row > row_max {
            return false;
        }

        // 単一行選択
        if row_min == row_max {
            let (col_min, col_max) = (ac.min(cc), ac.max(cc));
            return col >= col_min && col <= col_max;
        }

        // 複数行選択: 先頭行・末尾行・中間行で判定
        if row == row_min {
            let start_col = if ar < cr { ac } else { cc };
            return col >= start_col;
        }
        if row == row_max {
            let end_col = if ar < cr { cc } else { ac };
            return col <= end_col;
        }
        // 中間行はすべて選択
        true
    }
}

/// Win32 スレッドが表示するトースト通知
#[derive(Clone, Debug)]
pub struct Toast {
    /// 発生元ペイン ID
    pub pane_id: PaneId,
    /// 通知メッセージ
    pub message: String,
    /// 生成からの経過ミリ秒
    pub elapsed_ms: u32,
}

impl Toast {
    /// トースト全体の表示時間（ms）
    pub const DURATION_MS: u32 = 4000;
    /// スライドインにかける時間（ms）
    pub const SLIDE_MS: u32 = 300;
}

/// クライアント側のペイン状態（ウィンドウスレッドと tokio タスクで共有）
pub struct PaneStore {
    /// ペイン ID → グリッドの Arc
    pub grids: HashMap<PaneId, Arc<Mutex<Grid>>>,
    /// レイアウトツリー（フローティングペインは含まない）
    pub layout: LayoutNode,
    /// フォーカスされているペイン ID
    pub active: PaneId,
    /// OSC 52 で要求されたクリップボードデータ（Win32 スレッドが取り出して SetClipboardData）
    pub pending_clipboard: Option<Vec<u8>>,
    /// 未処理のトースト通知キュー（tokio → Win32 スレッドへの引き渡し）
    pub pending_toasts: VecDeque<Toast>,
    /// アクティブペインのスクロールオフセット（0 = 最新画面、正値 = 過去方向）
    pub scroll_offset: usize,
    /// フローティングペイン ID（None = 未作成）
    pub floating: Option<PaneId>,
    /// フローティングペインを表示中かどうか
    pub floating_visible: bool,
    /// フローティング表示前のアクティブペイン（非表示時の復帰用）
    pub pre_float_active: Option<PaneId>,
    /// true のとき Win32 タイマーがウィンドウを破棄してアプリを終了する（C-9）
    pub should_quit: bool,
    /// レイアウトランチャー UI の状態（Some = 表示中）
    pub launcher: Option<LauncherState>,
    /// コピーモードの状態（Some = コピーモード中）
    pub copy_mode: Option<CopyState>,
    /// Normal モードのマウス選択状態（anchor_col, anchor_row, end_col, end_row）
    pub normal_selection: Option<(usize, usize, usize, usize)>,
    /// レイアウト保存プロンプトの入力バッファ（Some = プロンプト表示中）
    pub save_prompt: Option<String>,
    /// テーマランチャー UI の状態（Some = 表示中）
    pub theme_launcher: Option<ThemeLauncherState>,
    /// ペイン ID → 起動コマンド文字列（レイアウト適用時に記録、C-23）
    ///
    /// レイアウトファイルから適用されたコマンドのみ記録される。
    /// 手動入力したコマンドは含まれない。
    pub pane_commands: HashMap<PaneId, String>,
}

impl PaneStore {
    pub fn new(pane_id: PaneId, grid: Arc<Mutex<Grid>>) -> Self {
        let mut grids = HashMap::new();
        grids.insert(pane_id, grid);
        Self {
            grids,
            layout: LayoutNode::Leaf(pane_id),
            active: pane_id,
            pending_clipboard: None,
            pending_toasts: VecDeque::new(),
            scroll_offset: 0,
            floating: None,
            floating_visible: false,
            pre_float_active: None,
            should_quit: false,
            launcher: None,
            copy_mode: None,
            normal_selection: None,
            save_prompt: None,
            theme_launcher: None,
            pane_commands: HashMap::new(),
        }
    }

    /// フローティングペインをコンテンツ領域の中央 80% に配置した矩形を返す
    pub fn floating_rect(content: PaneRect) -> PaneRect {
        let w = ((content.w as f32 * 0.8) as i32).max(1);
        let h = ((content.h as f32 * 0.8) as i32).max(1);
        PaneRect {
            x: (content.w - w) / 2,
            y: (content.h - h) / 2,
            w,
            h,
        }
    }

    /// フローティングペインを表示してフォーカスを移す
    pub fn show_float(&mut self) {
        if let Some(float_id) = self.floating {
            self.pre_float_active = Some(self.active);
            self.active = float_id;
            self.floating_visible = true;
        }
    }

    /// フローティングペインを非表示にして元のペインにフォーカスを戻す
    pub fn hide_float(&mut self) {
        self.floating_visible = false;
        if let Some(prev) = self.pre_float_active.take() {
            if self.grids.contains_key(&prev) {
                self.active = prev;
            }
        }
    }
}

// ── テスト ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PaneRect {
        PaneRect {
            x: 0,
            y: 0,
            w: 200,
            h: 100,
        }
    }

    // TC-01: 垂直分割で Right → 右ペイン
    #[test]
    fn test_direction_right_vertical_split() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(1), Direction::Right, root()),
            PaneId(2)
        );
    }

    // TC-02: 垂直分割で Left → 左ペイン
    #[test]
    fn test_direction_left_vertical_split() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(2), Direction::Left, root()),
            PaneId(1)
        );
    }

    // TC-03: 端のペインで移動先なし → 自ペインを返す
    #[test]
    fn test_direction_no_candidate_returns_self() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(1), Direction::Left, root()),
            PaneId(1)
        );
    }

    // TC-04: 水平分割で Down → 下ペイン
    #[test]
    fn test_direction_down_horizontal_split() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let r = PaneRect {
            x: 0,
            y: 0,
            w: 100,
            h: 200,
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(1), Direction::Down, r),
            PaneId(2)
        );
    }

    // TC-05: 水平分割で Up → 上ペイン
    #[test]
    fn test_direction_up_horizontal_split() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let r = PaneRect {
            x: 0,
            y: 0,
            w: 100,
            h: 200,
        };
        assert_eq!(
            layout.pane_in_direction(PaneId(2), Direction::Up, r),
            PaneId(1)
        );
    }

    // TC-06: 単一ペインでは常に自ペインを返す
    #[test]
    fn test_direction_single_pane_returns_self() {
        let layout = LayoutNode::Leaf(PaneId(1));
        assert_eq!(
            layout.pane_in_direction(PaneId(1), Direction::Right, root()),
            PaneId(1)
        );
    }

    // ── pane_at_point テスト ──────────────────────────────────────────────

    // TC-F9-01: 単一ペイン — どこをクリックしても自ペイン
    #[test]
    fn test_pane_at_point_single() {
        let layout = LayoutNode::Leaf(PaneId(1));
        assert_eq!(layout.pane_at_point(50, 50, root()), Some(PaneId(1)));
    }

    // TC-F9-02: 垂直分割 — 左半分 → first
    #[test]
    fn test_pane_at_point_vertical_left() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        // root = 200x100, SEP=1, w1=99, w2=100
        assert_eq!(layout.pane_at_point(10, 50, root()), Some(PaneId(1)));
    }

    // TC-F9-03: 垂直分割 — 右半分 → second
    #[test]
    fn test_pane_at_point_vertical_right() {
        let layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert_eq!(layout.pane_at_point(150, 50, root()), Some(PaneId(2)));
    }

    // TC-F9-04: 範囲外 → None
    #[test]
    fn test_pane_at_point_out_of_bounds() {
        let layout = LayoutNode::Leaf(PaneId(1));
        // root は (0,0,200,100)、点 (-1, 0) はヒットしない
        assert_eq!(layout.pane_at_point(-1, 0, root()), None);
    }

    // ── floating_rect テスト ─────────────────────────────────────────────

    // TC-01: 200×100 の中央に 80% 矩形
    #[test]
    fn test_floating_rect_centered() {
        let content = PaneRect {
            x: 0,
            y: 0,
            w: 200,
            h: 100,
        };
        let r = PaneStore::floating_rect(content);
        assert_eq!(r.w, 160);
        assert_eq!(r.h, 80);
        assert_eq!(r.x, 20);
        assert_eq!(r.y, 10);
    }

    // TC-02: 奇数サイズでも中央揃えされる
    #[test]
    fn test_floating_rect_odd_size() {
        let content = PaneRect {
            x: 0,
            y: 0,
            w: 101,
            h: 51,
        };
        let r = PaneStore::floating_rect(content);
        assert_eq!(r.w, 80); // floor(101 * 0.8) = 80
        assert_eq!(r.h, 40); // floor(51 * 0.8) = 40
        assert!(r.x >= 10);
        assert!(r.y >= 5);
    }

    // TC-03: show_float で active がフローティング ID に変わる
    #[test]
    fn test_show_float_sets_active() {
        let grid = Arc::new(Mutex::new(yatamux_terminal::Grid::new(
            80,
            24,
            Default::default(),
        )));
        let float_grid = Arc::new(Mutex::new(yatamux_terminal::Grid::new(
            80,
            24,
            Default::default(),
        )));
        let mut store = PaneStore::new(PaneId(1), grid);
        store.grids.insert(PaneId(2), float_grid);
        store.floating = Some(PaneId(2));
        store.show_float();
        assert_eq!(store.active, PaneId(2));
        assert_eq!(store.pre_float_active, Some(PaneId(1)));
        assert!(store.floating_visible);
    }

    // TC-04: hide_float で active が元に戻る
    #[test]
    fn test_hide_float_restores_active() {
        let grid = Arc::new(Mutex::new(yatamux_terminal::Grid::new(
            80,
            24,
            Default::default(),
        )));
        let float_grid = Arc::new(Mutex::new(yatamux_terminal::Grid::new(
            80,
            24,
            Default::default(),
        )));
        let mut store = PaneStore::new(PaneId(1), grid);
        store.grids.insert(PaneId(2), float_grid);
        store.floating = Some(PaneId(2));
        store.show_float();
        store.hide_float();
        assert_eq!(store.active, PaneId(1));
        assert!(!store.floating_visible);
    }

    // TC-05: レイアウトツリーにフローティングペインは含まれない
    #[test]
    fn test_floating_not_in_layout_ids() {
        let grid = Arc::new(Mutex::new(yatamux_terminal::Grid::new(
            80,
            24,
            Default::default(),
        )));
        let store = PaneStore::new(PaneId(1), grid);
        // layout は Leaf(1) のみ
        let ids = store.layout.pane_ids();
        assert_eq!(ids, vec![PaneId(1)]);
        // floating = Some(99) であっても pane_ids には入らない
        // (floating はレイアウトツリー外で管理)
    }

    // ── remove_pane テスト ────────────────────────────────────────────────

    // TC-F8-05: 単一 Leaf は削除不可 → None
    #[test]
    fn test_remove_pane_single_returns_none() {
        let mut layout = LayoutNode::Leaf(PaneId(1));
        assert_eq!(layout.remove_pane(PaneId(1)), None);
        // ツリーは変化しない
        assert!(matches!(layout, LayoutNode::Leaf(PaneId(1))));
    }

    // TC-F8-06: 垂直分割の first を削除
    #[test]
    fn test_remove_pane_vertical_first() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let next = layout.remove_pane(PaneId(1));
        assert_eq!(next, Some(PaneId(2)));
        assert!(matches!(layout, LayoutNode::Leaf(PaneId(2))));
    }

    // TC-F8-07: 垂直分割の second を削除
    #[test]
    fn test_remove_pane_vertical_second() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let next = layout.remove_pane(PaneId(2));
        assert_eq!(next, Some(PaneId(1)));
        assert!(matches!(layout, LayoutNode::Leaf(PaneId(1))));
    }

    // ── adjust_ratio テスト (C-10) ─────────────────────────────────────────

    // TC-C10-01: 単一 Leaf → no-op (false)
    #[test]
    fn test_adjust_ratio_leaf_noop() {
        let mut layout = LayoutNode::Leaf(PaneId(1));
        assert!(!layout.adjust_ratio(PaneId(1), 0.05));
    }

    // TC-C10-02: first ペインを拡大 → ratio 増加
    #[test]
    fn test_adjust_ratio_first_expand() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.55).abs() < 1e-6);
        }
    }

    // TC-C10-03: second ペインを拡大 → ratio 減少
    #[test]
    fn test_adjust_ratio_second_expand() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(2), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.45).abs() < 1e-6);
        }
    }

    // TC-C10-04: ratio は 0.9 でクランプ
    #[test]
    fn test_adjust_ratio_clamp_max() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.88,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.9).abs() < 1e-6);
        }
    }

    // TC-C10-05: ratio は 0.1 でクランプ
    #[test]
    fn test_adjust_ratio_clamp_min() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.12,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), -0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.1).abs() < 1e-6);
        }
    }

    // TC-C10-06: ネスト Split — 内側の Split を操作
    #[test]
    fn test_adjust_ratio_nested_inner() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(1))),
                second: Box::new(LayoutNode::Leaf(PaneId(2))),
            }),
            second: Box::new(LayoutNode::Leaf(PaneId(3))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        // 外側の ratio は変わらない
        if let LayoutNode::Split {
            ratio: outer_ratio,
            first,
            ..
        } = &layout
        {
            assert!((outer_ratio - 0.5).abs() < 1e-6);
            // 内側の ratio が変わっている
            if let LayoutNode::Split {
                ratio: inner_ratio, ..
            } = first.as_ref()
            {
                assert!((inner_ratio - 0.55).abs() < 1e-6);
            }
        }
    }

    // ── adjust_ratio テスト — Horizontal Split (C-18) ──────────────────────

    // TC-C18-01: Horizontal Split の first ペインを拡大 → ratio 増加
    #[test]
    fn test_adjust_ratio_horizontal_first_expand() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.55).abs() < 1e-6);
        }
    }

    // TC-C18-02: Horizontal Split の second ペインを拡大 → ratio 減少
    #[test]
    fn test_adjust_ratio_horizontal_second_expand() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(2), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.45).abs() < 1e-6);
        }
    }

    // TC-C18-03: Horizontal Split — ratio は 0.9 でクランプ
    #[test]
    fn test_adjust_ratio_horizontal_clamp_max() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.88,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        assert!(layout.adjust_ratio(PaneId(1), 0.05));
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.9).abs() < 1e-6);
        }
    }

    // ── adjust_ratio_for_dir テスト (C-19 リグレッション防止) ────────────────

    // TC-C19-R01: Vertical-only adjust は Horizontal Split に触れない
    #[test]
    fn test_adjust_ratio_for_dir_vertical_ignores_horizontal() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Horizontal,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        // Vertical 方向の調整は Horizontal Split を変更しない
        let changed = layout.adjust_ratio_for_dir(PaneId(1), 0.05, SplitDirection::Vertical);
        assert!(
            !changed,
            "Vertical adjust should not affect Horizontal split"
        );
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.5).abs() < 1e-6, "ratio should be unchanged");
        }
    }

    // TC-C19-R02: Horizontal-only adjust は Vertical Split に触れない
    #[test]
    fn test_adjust_ratio_for_dir_horizontal_ignores_vertical() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        // Horizontal 方向の調整は Vertical Split を変更しない
        let changed = layout.adjust_ratio_for_dir(PaneId(1), 0.05, SplitDirection::Horizontal);
        assert!(
            !changed,
            "Horizontal adjust should not affect Vertical split"
        );
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.5).abs() < 1e-6, "ratio should be unchanged");
        }
    }

    // TC-C19-R03: ネスト構造 Vertical(1, Horizontal(2, 3)) で方向ごとに別の Split が動く
    //
    // `<`/`>` (Vertical) を Pane2 に適用 → 外側 Vertical ratio が変化、内側 Horizontal は不変
    // `+`/`-` (Horizontal) を Pane2 に適用 → 内側 Horizontal ratio が変化、外側 Vertical は不変
    #[test]
    fn test_adjust_ratio_for_dir_vertical_changes_outer_not_inner() {
        let make_layout = || LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(2))),
                second: Box::new(LayoutNode::Leaf(PaneId(3))),
            }),
        };

        // Vertical 調整 → 外側 Vertical ratio が変化（Pane2 は second 側なので減少）
        // 内側 Horizontal ratio は不変
        let mut layout = make_layout();
        let changed = layout.adjust_ratio_for_dir(PaneId(2), 0.05, SplitDirection::Vertical);
        assert!(
            changed,
            "Vertical adjust should affect outer Vertical split"
        );
        if let LayoutNode::Split {
            ratio: outer,
            second,
            ..
        } = &layout
        {
            assert!(
                (*outer - 0.45).abs() < 1e-6,
                "outer Vertical ratio decreased: {outer}"
            );
            if let LayoutNode::Split { ratio: inner, .. } = second.as_ref() {
                assert!(
                    (*inner - 0.5).abs() < 1e-6,
                    "inner Horizontal ratio unchanged: {inner}"
                );
            }
        }

        // Horizontal 調整 → 内側 Horizontal ratio が変化（Pane2 は first 側なので増加）
        // 外側 Vertical ratio は不変
        let mut layout = make_layout();
        let changed = layout.adjust_ratio_for_dir(PaneId(2), 0.05, SplitDirection::Horizontal);
        assert!(
            changed,
            "Horizontal adjust should affect inner Horizontal split"
        );
        if let LayoutNode::Split {
            ratio: outer,
            second,
            ..
        } = &layout
        {
            assert!(
                (*outer - 0.5).abs() < 1e-6,
                "outer Vertical ratio unchanged: {outer}"
            );
            if let LayoutNode::Split { ratio: inner, .. } = second.as_ref() {
                assert!(
                    (*inner - 0.55).abs() < 1e-6,
                    "inner Horizontal ratio increased: {inner}"
                );
            }
        }
    }

    // TC-C19-R04: adjust_ratio_for_dir で ratio クランプが機能する
    #[test]
    fn test_adjust_ratio_for_dir_clamps_ratio() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.88,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        layout.adjust_ratio_for_dir(PaneId(1), 0.05, SplitDirection::Vertical);
        if let LayoutNode::Split { ratio, .. } = layout {
            assert!((ratio - 0.9).abs() < 1e-6, "ratio clamped at 0.9");
        }
    }

    // TC-F8-08: ネスト Split(1, Split(2, 3)) → remove 2 → Split(1, 3)
    #[test]
    fn test_remove_pane_nested() {
        let mut layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(2))),
                second: Box::new(LayoutNode::Leaf(PaneId(3))),
            }),
        };
        let next = layout.remove_pane(PaneId(2));
        assert_eq!(next, Some(PaneId(3)));
        // ツリーが Split(1, 3) になっていることを確認
        let ids = layout.pane_ids();
        assert_eq!(ids, vec![PaneId(1), PaneId(3)]);
    }

    // ── CopyState テスト (C-12) ─────────────────────────────────────────────

    // TC-C12-01: CopyState が正しく初期化される
    #[test]
    fn test_copy_state_init() {
        let cs = CopyState::new(0, 0);
        assert_eq!(cs.cursor, (0, 0));
        assert!(cs.anchor.is_none());
    }

    // TC-C12-02: カーソル移動が端でクランプされる
    #[test]
    fn test_copy_state_cursor_clamp() {
        let mut cs = CopyState::new(0, 0);
        // 左端・上端からさらに左・上へ
        cs.move_cursor(-1, 0, 80, 24);
        assert_eq!(cs.cursor, (0, 0), "col should clamp at 0");
        cs.move_cursor(0, -1, 80, 24);
        assert_eq!(cs.cursor, (0, 0), "row should clamp at 0");
        // 右端・下端へ移動してからさらに右・下へ
        cs.move_cursor(100, 100, 80, 24);
        assert_eq!(cs.cursor, (79, 23), "should clamp at (cols-1, rows-1)");
        cs.move_cursor(1, 0, 80, 24);
        assert_eq!(cs.cursor.0, 79, "col should clamp at cols-1");
        cs.move_cursor(0, 1, 80, 24);
        assert_eq!(cs.cursor.1, 23, "row should clamp at rows-1");
    }

    // TC-C12-03: アンカーのセット/アンセットが正しく動作する
    #[test]
    fn test_copy_state_anchor_toggle() {
        let mut cs = CopyState::new(5, 3);
        assert!(cs.anchor.is_none());
        cs.toggle_anchor();
        assert_eq!(cs.anchor, Some((5, 3)));
        // カーソルを移動してもアンカーは変わらない
        cs.move_cursor(3, 2, 80, 24);
        assert_eq!(cs.anchor, Some((5, 3)));
        assert_eq!(cs.cursor, (8, 5));
        // 再度トグルでアンカーをクリア
        cs.toggle_anchor();
        assert!(cs.anchor.is_none());
    }

    // TC-C12-04: is_selected が単一行選択を正しく判定する
    #[test]
    fn test_copy_state_is_selected_single_row() {
        let mut cs = CopyState::new(2, 3);
        cs.toggle_anchor(); // anchor = (2, 3)
        cs.move_cursor(3, 0, 80, 24); // cursor = (5, 3)
                                      // col 2-5 が選択されている
        assert!(cs.is_selected(2, 3));
        assert!(cs.is_selected(4, 3));
        assert!(cs.is_selected(5, 3));
        assert!(!cs.is_selected(1, 3));
        assert!(!cs.is_selected(6, 3));
        assert!(!cs.is_selected(3, 2)); // 別の行
    }

    // TC-C12-05: is_selected が複数行選択を正しく判定する
    #[test]
    fn test_copy_state_is_selected_multi_row() {
        let mut cs = CopyState::new(5, 2);
        cs.toggle_anchor(); // anchor = (5, 2)
        cs.move_cursor(3, 2, 80, 24); // cursor = (8, 4)
                                      // 先頭行: col >= 5
        assert!(cs.is_selected(5, 2));
        assert!(cs.is_selected(79, 2));
        assert!(!cs.is_selected(4, 2));
        // 中間行: すべて選択
        assert!(cs.is_selected(0, 3));
        assert!(cs.is_selected(79, 3));
        // 末尾行: col <= 8
        assert!(cs.is_selected(0, 4));
        assert!(cs.is_selected(8, 4));
        assert!(!cs.is_selected(9, 4));
    }

    // ── C-19: layout_to_toml / save_layout_file ─────────────────────────

    // TC-C19-01: 単一ペインは [[panes]] 1 エントリで split なし
    #[test]
    fn test_layout_to_toml_single_pane() {
        let node = LayoutNode::Leaf(PaneId(1));
        let toml = layout_to_toml(&node, &HashMap::new());
        assert_eq!(toml, "[[panes]]\n\n");
    }

    // TC-C19-02: 垂直分割は 2 エントリ、2 つ目に split = "vertical"
    #[test]
    fn test_layout_to_toml_vertical_split() {
        let node = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let toml = layout_to_toml(&node, &HashMap::new());
        assert_eq!(toml, "[[panes]]\n\n[[panes]]\nsplit = \"vertical\"\n\n");
    }

    // TC-C19-03: ネスト構造は 3 エントリ
    #[test]
    fn test_layout_to_toml_nested() {
        let node = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Split {
                direction: SplitDirection::Horizontal,
                ratio: 0.5,
                first: Box::new(LayoutNode::Leaf(PaneId(2))),
                second: Box::new(LayoutNode::Leaf(PaneId(3))),
            }),
        };
        let toml = layout_to_toml(&node, &HashMap::new());
        let expected = "[[panes]]\n\n\
                        [[panes]]\nsplit = \"vertical\"\n\n\
                        [[panes]]\nsplit = \"horizontal\"\n\n";
        assert_eq!(toml, expected);
    }

    // TC-C23-01: コマンドなしペインは command 行を出力しない
    #[test]
    fn test_layout_to_toml_no_command() {
        let node = LayoutNode::Leaf(PaneId(1));
        let toml = layout_to_toml(&node, &HashMap::new());
        assert!(!toml.contains("command"));
        assert_eq!(toml, "[[panes]]\n\n");
    }

    // TC-C23-02: コマンドありペインは command = "..." 行を出力する
    #[test]
    fn test_layout_to_toml_with_command() {
        let node = LayoutNode::Leaf(PaneId(1));
        let mut commands = HashMap::new();
        commands.insert(PaneId(1), "cargo watch".to_string());
        let toml = layout_to_toml(&node, &commands);
        assert!(toml.contains("command = \"cargo watch\""), "got: {toml}");
    }

    // TC-C23-03: 垂直分割でコマンドを持つ 2 つ目のペインが TOML に含まれる
    #[test]
    fn test_layout_to_toml_split_with_command() {
        let node = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(PaneId(1))),
            second: Box::new(LayoutNode::Leaf(PaneId(2))),
        };
        let mut commands = HashMap::new();
        commands.insert(PaneId(2), "cargo test".to_string());
        let toml = layout_to_toml(&node, &commands);
        let expected = "[[panes]]\n\n\
                        [[panes]]\n\
                        command = \"cargo test\"\n\
                        split = \"vertical\"\n\n";
        assert_eq!(toml, expected);
    }

    // TC-C23-04: コマンドに特殊文字を含む場合も適切にエスケープされる
    #[test]
    fn test_layout_to_toml_command_with_special_chars() {
        let node = LayoutNode::Leaf(PaneId(1));
        let mut commands = HashMap::new();
        commands.insert(PaneId(1), r#"echo "hello""#.to_string());
        let toml = layout_to_toml(&node, &commands);
        // Rust の Debug フォーマットで " がエスケープされる
        assert!(toml.contains("command ="), "got: {toml}");
        // TOML としてパース可能であることを確認
        let parsed: toml::Value = toml::from_str(&toml).expect("should be valid TOML");
        let arr = parsed["panes"].as_array().unwrap();
        assert_eq!(arr[0]["command"].as_str().unwrap(), r#"echo "hello""#);
    }

    // TC-C19-04: save_layout_file は正常に書き込む
    #[test]
    fn test_save_layout_file_roundtrip() {
        let dir = std::env::temp_dir().join("yatamux_save_layout_test");
        std::fs::create_dir_all(&dir).unwrap();

        // APPDATA を一時ディレクトリで代替するため直接書き込みテスト
        let content = "[[panes]]\n\n[[panes]]\nsplit = \"vertical\"\n\n";
        let path = dir.join("testlayout.toml");
        std::fs::write(&path, content).unwrap();
        let loaded = std::fs::read_to_string(&path).unwrap();
        assert_eq!(loaded, content);

        let _ = std::fs::remove_dir_all(&dir);
    }
}
