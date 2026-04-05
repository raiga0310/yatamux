use anyhow::{Context, Result};
use std::collections::HashMap;
use yatamux_protocol::types::{PaneId, PaneInfo};
use yatamux_protocol::ServerMessage;

use super::super::tree::pane_ids_in_tree;
use super::support::{build_capture_response, pane_info};
use super::*;

impl Server {
    pub(super) async fn handle_capture_pane(
        &mut self,
        pane: PaneId,
        lines: usize,
        _plain_text: bool,
    ) -> Result<()> {
        if !self.panes.contains_key(&pane) {
            self.send_pane_not_found_error(pane).await?;
            return Ok(());
        }

        let (content, capture) = if let Some(p) = self.panes.get(&pane) {
            let (content, capture) = build_capture_response(p, lines).await;
            (content, Some(capture))
        } else {
            (String::new(), None)
        };

        self.client_tx
            .send(ServerMessage::PaneContent {
                pane,
                content,
                capture,
            })
            .await
            .context("Failed to send PaneContent")?;
        Ok(())
    }

    pub(super) async fn handle_query_all_pane_processes(&mut self) -> Result<()> {
        let mut commands: HashMap<String, Option<String>> = HashMap::new();
        let mut cwds: HashMap<String, Option<String>> = HashMap::new();
        for (&pane_id, pane) in &self.panes {
            let key = pane_id.0.to_string();
            let cmd = pane
                .child_pid
                .and_then(yatamux_terminal::process::find_active_command);
            commands.insert(key.clone(), cmd);
            // child_pid (cmd.exe 等) の cwd を取得する
            let cwd = pane
                .child_pid
                .and_then(yatamux_terminal::process::find_process_cwd);
            cwds.insert(key, cwd);
        }
        self.client_tx
            .send(ServerMessage::AllPaneProcesses { commands, cwds })
            .await
            .context("Failed to send AllPaneProcesses")?;
        Ok(())
    }

    pub(super) async fn handle_list_panes(&mut self) -> Result<()> {
        let mut panes: Vec<PaneInfo> = Vec::new();
        for (surf_id, surface) in &self.surfaces {
            let ids_in_tree = surface
                .pane_tree
                .as_ref()
                .map(pane_ids_in_tree)
                .unwrap_or_default();
            for pane_id in &ids_in_tree {
                if let Some(pane) = self.panes.get(pane_id) {
                    panes.push(pane_info(*pane_id, *surf_id, pane));
                }
            }
        }
        self.client_tx
            .send(ServerMessage::PanesListed { panes })
            .await
            .context("Failed to send PanesListed")?;
        Ok(())
    }
}
