use yatamux_protocol::types::{PaneId, SplitDirection};

use super::PaneTree;

/// ツリー内の全 PaneId を収集する
pub(crate) fn pane_ids_in_tree(tree: &PaneTree) -> Vec<PaneId> {
    match tree {
        PaneTree::Leaf(id) => vec![*id],
        PaneTree::Split { first, second, .. } => {
            let mut ids = pane_ids_in_tree(first);
            ids.extend(pane_ids_in_tree(second));
            ids
        }
    }
}

/// `parent` の Leaf を `parent`/`child` の Split に置き換えるヘルパー
pub(crate) fn split_pane_tree(
    tree: PaneTree,
    parent: PaneId,
    child: PaneId,
    dir: SplitDirection,
) -> PaneTree {
    match tree {
        PaneTree::Leaf(id) if id == parent => PaneTree::Split {
            direction: dir,
            ratio: 0.5,
            first: Box::new(PaneTree::Leaf(id)),
            second: Box::new(PaneTree::Leaf(child)),
        },
        PaneTree::Split {
            direction,
            ratio,
            first,
            second,
        } => PaneTree::Split {
            direction,
            ratio,
            first: Box::new(split_pane_tree(*first, parent, child, dir)),
            second: Box::new(split_pane_tree(*second, parent, child, dir)),
        },
        other => other,
    }
}
