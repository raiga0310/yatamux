use std::sync::Arc;

use yatamux_protocol::types::{PaneId, SplitDirection, SurfaceId, WorkspaceId};

/// ペインの分割ツリーノード（二分木）
pub enum PaneTree {
    Leaf(PaneId),
    Split {
        direction: SplitDirection,
        ratio: f32,
        first: Box<PaneTree>,
        second: Box<PaneTree>,
    },
}

/// サーフェス（タブ）
pub struct Surface {
    pub id: SurfaceId,
    pub workspace: WorkspaceId,
    pub pane_tree: Option<PaneTree>,
    pub active_pane: Option<PaneId>,
}

/// ワークスペース（セッション相当）
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: Arc<str>,
    pub surfaces: Vec<SurfaceId>,
    pub active_surface: Option<SurfaceId>,
}
