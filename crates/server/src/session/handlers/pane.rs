use anyhow::{Context, Result};
use tracing::info;
use yatamux_protocol::types::{PaneId, SplitDirection, SurfaceId, TermSize};
use yatamux_protocol::ServerMessage;

use super::super::tree::split_pane_tree;
use super::*;

impl Server {
    pub(super) async fn handle_create_pane(
        &mut self,
        surface: SurfaceId,
        size: TermSize,
        split_from: Option<PaneId>,
        direction: Option<SplitDirection>,
        working_dir: Option<String>,
    ) -> Result<()> {
        let id = PaneId(self.next_pane_id);
        self.next_pane_id += 1;
        info!("Creating pane {:?} in surface {:?}", id, surface);

        let pane = Pane::spawn(
            id,
            size,
            self.width_config.clone(),
            self.pane_output_tx.clone(),
            self.pane_event_tx.clone(),
            working_dir,
        )?;
        self.panes.insert(id, pane);

        if let Some(s) = self.surfaces.get_mut(&surface) {
            match (split_from, direction, s.pane_tree.take()) {
                (Some(parent_id), Some(dir), Some(tree)) => {
                    s.pane_tree = Some(split_pane_tree(tree, parent_id, id, dir));
                }
                (_, _, existing) => {
                    s.pane_tree = Some(existing.unwrap_or(PaneTree::Leaf(id)));
                    if s.pane_tree
                        .as_ref()
                        .is_none_or(|t| matches!(t, PaneTree::Leaf(_)))
                    {
                        s.pane_tree = Some(PaneTree::Leaf(id));
                    }
                }
            }
            s.active_pane = Some(id);
        }

        self.client_tx
            .send(ServerMessage::PaneCreated {
                id,
                surface,
                split_from,
                direction,
            })
            .await
            .context("Failed to send PaneCreated")?;
        Ok(())
    }

    pub(super) async fn handle_input(&mut self, pane: PaneId, data: Vec<u8>) -> Result<()> {
        if let Some(p) = self.panes.get(&pane) {
            p.mark_busy(true);
            p.send_input(data).await?;
        } else {
            self.send_pane_not_found_error(pane).await?;
        }
        Ok(())
    }

    pub(super) async fn handle_resize(&mut self, pane: PaneId, size: TermSize) -> Result<()> {
        if let Some(p) = self.panes.get(&pane) {
            p.resize(size).await?;
        }
        Ok(())
    }

    pub(super) async fn handle_close_pane(&mut self, pane: PaneId) -> Result<()> {
        self.panes.remove(&pane);
        self.client_tx
            .send(ServerMessage::PaneClosed { pane })
            .await?;
        Ok(())
    }

    pub(super) async fn handle_interrupt_pane(&mut self, pane: PaneId) -> Result<()> {
        if let Some(p) = self.panes.get(&pane) {
            p.mark_busy(true);
            p.send_input(vec![0x03]).await?;
        } else {
            self.send_pane_not_found_error(pane).await?;
        }
        Ok(())
    }

    pub(super) async fn handle_detach(&mut self) -> Result<()> {
        info!("Client detached, server continues running");
        Ok(())
    }

    pub(super) async fn handle_request_screen(&mut self, _pane: PaneId) -> Result<()> {
        Ok(())
    }
}
