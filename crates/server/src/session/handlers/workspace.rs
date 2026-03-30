use std::sync::Arc;

use anyhow::{Context, Result};
use tracing::info;
use yatamux_protocol::types::WorkspaceId;
use yatamux_protocol::ServerMessage;

use super::*;

impl Server {
    pub(super) async fn handle_create_workspace(&mut self, name: Option<String>) -> Result<()> {
        let id = WorkspaceId(self.next_workspace_id);
        self.next_workspace_id += 1;
        let name = Arc::<str>::from(name.unwrap_or_else(|| format!("workspace-{}", id.0)));
        info!("Creating workspace {:?} '{}'", id, name);
        self.workspaces.insert(
            id,
            Workspace {
                id,
                name: Arc::clone(&name),
                surfaces: Vec::new(),
                active_surface: None,
            },
        );
        self.client_tx
            .send(ServerMessage::WorkspaceCreated {
                id,
                name: name.to_string(),
            })
            .await
            .context("Failed to send WorkspaceCreated")?;
        Ok(())
    }

    pub(super) async fn handle_create_surface(&mut self, workspace: WorkspaceId) -> Result<()> {
        let id = yatamux_protocol::types::SurfaceId(self.next_surface_id);
        self.next_surface_id += 1;
        info!("Creating surface {:?} in workspace {:?}", id, workspace);
        if let Some(ws) = self.workspaces.get_mut(&workspace) {
            ws.surfaces.push(id);
            if ws.active_surface.is_none() {
                ws.active_surface = Some(id);
            }
        }
        self.surfaces.insert(
            id,
            Surface {
                id,
                workspace,
                pane_tree: None,
                active_pane: None,
            },
        );
        self.client_tx
            .send(ServerMessage::SurfaceCreated { id, workspace })
            .await
            .context("Failed to send SurfaceCreated")?;
        Ok(())
    }
}
