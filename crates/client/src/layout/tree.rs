use yatamux_protocol::types::{PaneId, SplitDirection};

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
                if first.adjust_ratio(id, delta) || second.adjust_ratio(id, delta) {
                    return true;
                }
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
                if first.adjust_ratio_for_dir(id, delta, target_dir)
                    || second.adjust_ratio_for_dir(id, delta, target_dir)
                {
                    return true;
                }
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
