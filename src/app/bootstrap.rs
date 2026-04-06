use anyhow::Result;
use tokio::sync::mpsc;

use yatamux_protocol::types::{PaneId, SurfaceId, TermSize, WorkspaceId};
use yatamux_protocol::{ClientMessage, ServerMessage};
use yatamux_server::{ipc::run_ipc_server, Server};

pub(crate) struct BootstrapHandles {
    pub(crate) client_tx: mpsc::Sender<ClientMessage>,
    pub(crate) server_rx: mpsc::Receiver<ServerMessage>,
    pub(crate) ipc_out_tx: mpsc::Sender<ServerMessage>,
    pub(crate) surf_id: SurfaceId,
    pub(crate) pane_id: PaneId,
}

pub(crate) async fn bootstrap_runtime(session: &str, size: TermSize) -> Result<BootstrapHandles> {
    let (server_out_tx, server_rx) = mpsc::channel::<ServerMessage>(256);
    let (ipc_out_tx, ipc_out_rx) = mpsc::channel::<ServerMessage>(256);

    let (merged_tx, merged_rx) = mpsc::channel::<ClientMessage>(256);
    let client_tx = merged_tx.clone();
    let ipc_in_tx = merged_tx.clone();

    let server = Server::new(server_out_tx);
    tokio::spawn(server.run(merged_rx));

    let session = session.to_string();
    tokio::spawn(async move {
        if let Err(e) = run_ipc_server(&session, ipc_in_tx, ipc_out_rx).await {
            tracing::error!("IPC server exited with error: {:#}", e);
        }
    });

    let mut server_rx = server_rx;
    let (surf_id, pane_id) = create_initial_surface(size, &client_tx, &mut server_rx).await?;

    Ok(BootstrapHandles {
        client_tx,
        server_rx,
        ipc_out_tx,
        surf_id,
        pane_id,
    })
}

async fn create_initial_surface(
    size: TermSize,
    client_tx: &mpsc::Sender<ClientMessage>,
    server_rx: &mut mpsc::Receiver<ServerMessage>,
) -> Result<(SurfaceId, PaneId)> {
    client_tx
        .send(ClientMessage::CreateWorkspace { name: None })
        .await?;
    let workspace = wait_for_workspace_created(server_rx).await?;

    client_tx
        .send(ClientMessage::CreateSurface { workspace })
        .await?;
    let surface = wait_for_surface_created(server_rx).await?;

    client_tx
        .send(ClientMessage::CreatePane {
            surface,
            split_from: None,
            direction: None,
            size,
            working_dir: None,
        })
        .await?;
    let pane = wait_for_pane_created(server_rx).await?;

    Ok((surface, pane))
}

async fn wait_for_workspace_created(
    server_rx: &mut mpsc::Receiver<ServerMessage>,
) -> Result<WorkspaceId> {
    loop {
        match server_rx.recv().await {
            Some(ServerMessage::WorkspaceCreated { id, .. }) => return Ok(id),
            Some(ServerMessage::Error { message }) => {
                return Err(anyhow::anyhow!("Server error: {}", message));
            }
            Some(_) => continue,
            None => return Err(anyhow::anyhow!("Server channel closed unexpectedly")),
        }
    }
}

async fn wait_for_surface_created(
    server_rx: &mut mpsc::Receiver<ServerMessage>,
) -> Result<SurfaceId> {
    loop {
        match server_rx.recv().await {
            Some(ServerMessage::SurfaceCreated { id, .. }) => return Ok(id),
            Some(ServerMessage::Error { message }) => {
                return Err(anyhow::anyhow!("Server error: {}", message));
            }
            Some(_) => continue,
            None => return Err(anyhow::anyhow!("Server channel closed unexpectedly")),
        }
    }
}

async fn wait_for_pane_created(server_rx: &mut mpsc::Receiver<ServerMessage>) -> Result<PaneId> {
    loop {
        match server_rx.recv().await {
            Some(ServerMessage::PaneCreated { id, .. }) => return Ok(id),
            Some(ServerMessage::Error { message }) => {
                return Err(anyhow::anyhow!("Server error: {}", message));
            }
            Some(_) => continue,
            None => return Err(anyhow::anyhow!("Server channel closed unexpectedly")),
        }
    }
}
