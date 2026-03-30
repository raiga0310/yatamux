use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use yatamux_client::{LayoutNode, PaneStore};
use yatamux_protocol::types::{PaneId, SplitDirection};

use crate::layout_config::{LayoutConfig, SplitDir};

pub(super) type LayoutName = Arc<str>;

#[derive(Clone)]
pub(super) struct PaneLaunchPlan {
    pub(super) split: SplitDirection,
    pub(super) ratio: f32,
    pub(super) command: Option<Arc<str>>,
}

pub(super) struct LayoutPlan {
    pub(super) first_command: Option<Arc<str>>,
    pub(super) queue: VecDeque<PaneLaunchPlan>,
}

impl LayoutPlan {
    pub(super) fn from_config(config: LayoutConfig) -> Self {
        let first_command = config
            .panes
            .first()
            .and_then(|pane| pane.command.as_deref())
            .map(Arc::<str>::from);
        let queue = config
            .panes
            .into_iter()
            .skip(1)
            .map(|pane| PaneLaunchPlan {
                split: match pane.split {
                    Some(SplitDir::Horizontal) => SplitDirection::Horizontal,
                    _ => SplitDirection::Vertical,
                },
                ratio: pane.ratio,
                command: pane.command.map(Arc::<str>::from),
            })
            .collect();
        Self {
            first_command,
            queue,
        }
    }
}

/// レイアウト切り替えのフェーズ。
pub(super) enum LayoutPhase {
    /// 既存ペインが全て閉じるまで待機
    Closing {
        target: LayoutName,
        remaining: usize,
    },
    /// 最初の新規ペインの PaneCreated を待機
    WaitingFirst { plan: LayoutPlan },
    /// 残りペインを順次作成中
    Applying {
        /// 未送信の起動計画キュー（front が直近に送った CreatePane の設定）
        queue: VecDeque<PaneLaunchPlan>,
        layout: LayoutNode,
        grids: Vec<(PaneId, Arc<Mutex<yatamux_terminal::Grid>>)>,
        /// 直前に作成されたペイン ID（次の split_from に使う）
        prev: PaneId,
        // active は finalize 時に最後の new_id を直接使うため省略可だが型合わせで保持
        #[allow(dead_code)]
        active: PaneId,
    },
}

/// レイアウト切り替え完了時に PaneStore を更新する。
pub(super) fn finalize_layout_switch(
    pane_store: &Arc<Mutex<PaneStore>>,
    layout: LayoutNode,
    grids: Vec<(PaneId, Arc<Mutex<yatamux_terminal::Grid>>)>,
    active: PaneId,
) {
    let mut store = pane_store.lock().unwrap();
    store.grids.clear();
    for (id, grid) in grids {
        store.grids.insert(id, grid);
    }
    store.layout = layout;
    store.active = active;
    store.floating = None;
    store.floating_visible = false;
    store.scroll_offset = 0;
    store.launcher = None;
    store.should_quit = false;
}
