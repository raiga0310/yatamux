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
        match self {
            LayoutNode::Leaf(id) if *id == parent => {
                *self = LayoutNode::Split {
                    direction: dir,
                    ratio: 0.5,
                    first: Box::new(LayoutNode::Leaf(parent)),
                    second: Box::new(LayoutNode::Leaf(child)),
                };
                true
            }
            LayoutNode::Split { first, second, .. } => {
                first.split_leaf(parent, child, dir) || second.split_leaf(parent, child, dir)
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
        match self {
            LayoutNode::Leaf(_) => None,
            LayoutNode::Split { first, second, .. } => {
                // first が削除対象
                if matches!(first.as_ref(), LayoutNode::Leaf(lid) if *lid == id) {
                    let next = second.pane_ids().into_iter().next();
                    *self = (**second).clone();
                    return next;
                }
                // second が削除対象
                if matches!(second.as_ref(), LayoutNode::Leaf(lid) if *lid == id) {
                    let next = first.pane_ids().into_iter().next();
                    *self = (**first).clone();
                    return next;
                }
                // 再帰
                first.remove_pane(id).or_else(|| second.remove_pane(id))
            }
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

/// TOML 文字列からプレビュー用 `LayoutPreview` を構築する
fn build_preview_layout(content: &str) -> Option<LayoutPreview> {
    #[derive(Deserialize)]
    struct PreviewConfig {
        #[serde(default)]
        panes: Vec<PreviewPane>,
    }
    #[derive(Deserialize)]
    struct PreviewPane {
        split: Option<PreviewSplitDir>,
        command: Option<String>,
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
            root.split_leaf(PaneId((i - 1) as u32), PaneId(i as u32), dir);
        }
    }
    Some(LayoutPreview {
        node: root,
        commands,
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
}
