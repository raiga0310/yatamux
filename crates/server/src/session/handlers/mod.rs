mod pane;
mod query;
pub(crate) mod support;
mod workspace;

use anyhow::{Context, Result};
use yatamux_protocol::types::PaneId;
use yatamux_protocol::{ClientMessage, ServerMessage};

use super::*;

impl Server {
    pub(super) async fn handle_client_message(&mut self, msg: ClientMessage) -> Result<()> {
        match msg {
            ClientMessage::CreateWorkspace { name } => self.handle_create_workspace(name).await,
            ClientMessage::CreateSurface { workspace } => {
                self.handle_create_surface(workspace).await
            }
            ClientMessage::CreatePane {
                surface,
                size,
                split_from,
                direction,
                working_dir,
            } => {
                self.handle_create_pane(surface, size, split_from, direction, working_dir)
                    .await
            }
            ClientMessage::Input { pane, data } => self.handle_input(pane, data).await,
            ClientMessage::Exec {
                request_id,
                pane,
                data,
                wait,
                timeout_ms,
            } => {
                self.handle_exec_request(request_id, pane, data, wait, timeout_ms)
                    .await
            }
            ClientMessage::Resize { pane, size } => self.handle_resize(pane, size).await,
            ClientMessage::ClosePane { pane, .. } => self.handle_close_pane(pane).await,
            ClientMessage::InterruptPane { pane, .. } => self.handle_interrupt_pane(pane).await,
            ClientMessage::TerminatePane { pane, .. } => self.handle_terminate_pane(pane).await,
            ClientMessage::SubscribePane { .. } | ClientMessage::UnsubscribePane { .. } => Ok(()),
            ClientMessage::SetPaneMeta { pane, alias, role } => {
                self.handle_set_pane_meta(pane, alias, role).await
            }
            ClientMessage::SyncPaneState {
                active_pane,
                floating_pane,
            } => {
                self.handle_sync_pane_state(active_pane, floating_pane)
                    .await
            }
            ClientMessage::Detach => self.handle_detach().await,
            ClientMessage::RequestScreen { pane } => self.handle_request_screen(pane).await,
            ClientMessage::CapturePane {
                pane,
                lines,
                plain_text,
            } => self.handle_capture_pane(pane, lines, plain_text).await,
            ClientMessage::ListPanes => self.handle_list_panes().await,
            ClientMessage::SaveAndQuit => {
                // SaveAndQuit はブリッジへ転送して GUI 側でセッション保存 + 終了する
                let _ = self.client_tx.send(ServerMessage::SaveAndQuit).await;
                Ok(())
            }
            ClientMessage::QueryAllPaneProcesses => self.handle_query_all_pane_processes().await,
            // Handshake は IPC 層で処理済みのため、ここには到達しない
            ClientMessage::Handshake { .. } => Ok(()),
            // QueryCiStatus は app.rs の CI ポーラーが処理する（broadcast 経由）
            // サーバー側では何もしない
            ClientMessage::QueryCiStatus => Ok(()),
        }
    }

    async fn send_pane_not_found_error(&mut self, pane: PaneId) -> Result<()> {
        self.client_tx
            .send(ServerMessage::Error {
                message: format!("pane {} not found", pane.0),
                request_id: None,
            })
            .await
            .context("Failed to send Error")?;
        Ok(())
    }
}
