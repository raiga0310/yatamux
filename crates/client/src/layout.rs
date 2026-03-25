//! ペインレイアウトツリーとペインストア
//!
//! `Arc<Mutex<PaneStore>>` でウィンドウスレッドと tokio タスクが共有する。

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

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
    /// レイアウトツリー
    pub layout: LayoutNode,
    /// フォーカスされているペイン ID
    pub active: PaneId,
    /// OSC 52 で要求されたクリップボードデータ（Win32 スレッドが取り出して SetClipboardData）
    pub pending_clipboard: Option<Vec<u8>>,
    /// 未処理のトースト通知キュー（tokio → Win32 スレッドへの引き渡し）
    pub pending_toasts: VecDeque<Toast>,
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
        }
    }
}
