use std::collections::HashMap;

use anyhow::Result;
use tokio::sync::mpsc;

use yatamux_client::{LayoutNode, LayoutNodeDef, LayoutSnapshot};
use yatamux_protocol::types::{PaneId, SplitDirection, SurfaceId, TermSize};
use yatamux_protocol::{ClientMessage, ServerMessage};
use yatamux_terminal::TerminalSink;

use crate::layout_config::{LayoutConfig, SplitDir};

/// ペインコマンドのマップ（PaneId → コマンド文字列）
type PaneCommands = HashMap<PaneId, String>;

/// 起動時のレイアウトを決定し、初期ペイン群を構築する。
///
/// 返り値の `PaneCommands` は各ペインで実行するコマンド。
/// 呼び出し側が `PaneStore.pane_commands` に設定すること。
pub(super) async fn load_initial_layout(
    layout_name: Option<String>,
    pane_id: PaneId,
    surf_id: SurfaceId,
    size: TermSize,
    client_tx: &mpsc::Sender<ClientMessage>,
    server_rx: &mut mpsc::Receiver<ServerMessage>,
) -> Result<(
    LayoutNode,
    Vec<(PaneId, TerminalSink)>,
    PaneId,
    PaneCommands,
)> {
    let session_path = LayoutSnapshot::default_path();
    if let Some(name) = layout_name {
        let config_path = LayoutConfig::layout_path(&name);
        match LayoutConfig::load(&config_path) {
            Ok(config) => {
                tracing::info!("レイアウト設定を適用します: {}", config_path.display());
                apply_layout_config(config, pane_id, surf_id, size, client_tx, server_rx).await
            }
            Err(e) => {
                tracing::warn!(
                    "レイアウト設定の読み込みに失敗しました（{}）: {:#}",
                    config_path.display(),
                    e
                );
                let sink = TerminalSink::new(size.cols, size.rows);
                Ok((
                    LayoutNode::Leaf(pane_id),
                    vec![(pane_id, sink)],
                    pane_id,
                    PaneCommands::new(),
                ))
            }
        }
    } else if let Ok(snapshot) = LayoutSnapshot::load(&session_path) {
        tracing::info!("セッションを復元します");
        let mut old_to_new: HashMap<PaneId, PaneId> = HashMap::new();
        let mut pane_commands = PaneCommands::new();
        let (layout, sinks) = restore_node(
            &snapshot.root,
            pane_id,
            surf_id,
            size,
            client_tx,
            server_rx,
            &mut old_to_new,
            &mut pane_commands,
        )
        .await?;
        let active = old_to_new.get(&snapshot.active).copied().unwrap_or(pane_id);
        Ok((layout, sinks, active, pane_commands))
    } else {
        let sink = TerminalSink::new(size.cols, size.rows);
        Ok((
            LayoutNode::Leaf(pane_id),
            vec![(pane_id, sink)],
            pane_id,
            PaneCommands::new(),
        ))
    }
}

/// 宣言的レイアウト設定を適用して初期ペインを構築する。
///
/// 返り値の `PaneCommands` は各ペインで実行するコマンド。
pub(super) async fn apply_layout_config(
    config: LayoutConfig,
    first_pane: PaneId,
    surf_id: SurfaceId,
    size: TermSize,
    client_tx: &mpsc::Sender<ClientMessage>,
    server_rx: &mut mpsc::Receiver<ServerMessage>,
) -> Result<(
    LayoutNode,
    Vec<(PaneId, TerminalSink)>,
    PaneId,
    PaneCommands,
)> {
    let mut layout = LayoutNode::Leaf(first_pane);
    let mut sinks: Vec<(PaneId, TerminalSink)> =
        vec![(first_pane, TerminalSink::new(size.cols, size.rows))];
    let mut active = first_pane;
    let mut pane_commands = PaneCommands::new();

    if let Some(pane_cfg) = config.panes.first() {
        if let Some(cmd) = &pane_cfg.command {
            pane_commands.insert(first_pane, cmd.clone());
        }
        send_command_input(client_tx, first_pane, pane_cfg.command.as_deref()).await;
    }

    for pane_cfg in config.panes.iter().skip(1) {
        let direction = match pane_cfg.split {
            Some(SplitDir::Horizontal) => SplitDirection::Horizontal,
            Some(SplitDir::Vertical) | None => SplitDirection::Vertical,
        };
        client_tx
            .send(ClientMessage::CreatePane {
                surface: surf_id,
                split_from: Some(active),
                direction: Some(direction),
                size,
                working_dir: None,
            })
            .await?;
        let new_id = wait_for_pane_created(server_rx).await?;
        sinks.push((new_id, TerminalSink::new(size.cols, size.rows)));
        layout.split_leaf_with_ratio(active, new_id, direction, pane_cfg.ratio);
        active = new_id;

        if let Some(cmd) = &pane_cfg.command {
            pane_commands.insert(new_id, cmd.clone());
        }
        send_command_input(client_tx, new_id, pane_cfg.command.as_deref()).await;
    }

    Ok((layout, sinks, active, pane_commands))
}

/// 保存済みレイアウトを再帰的に再構築する。
///
/// `pane_commands` には復元時に送信したコマンドが（新 PaneId をキーとして）蓄積される。
#[allow(clippy::too_many_arguments)]
pub(super) async fn restore_node(
    def: &LayoutNodeDef,
    current_pane: PaneId,
    surf_id: SurfaceId,
    size: TermSize,
    client_tx: &mpsc::Sender<ClientMessage>,
    server_rx: &mut mpsc::Receiver<ServerMessage>,
    old_to_new: &mut HashMap<PaneId, PaneId>,
    pane_commands: &mut PaneCommands,
) -> Result<(LayoutNode, Vec<(PaneId, TerminalSink)>)> {
    match def {
        LayoutNodeDef::Leaf {
            id: old_id,
            command,
        } => {
            old_to_new.insert(*old_id, current_pane);
            let sink = TerminalSink::new(size.cols, size.rows);
            if let Some(cmd) = command {
                pane_commands.insert(current_pane, cmd.clone());
                send_command_input(client_tx, current_pane, Some(cmd.as_str())).await;
            }
            Ok((LayoutNode::Leaf(current_pane), vec![(current_pane, sink)]))
        }
        LayoutNodeDef::Split {
            direction,
            ratio,
            first,
            second,
        } => {
            client_tx
                .send(ClientMessage::CreatePane {
                    surface: surf_id,
                    split_from: Some(current_pane),
                    direction: Some(*direction),
                    size,
                    working_dir: None,
                })
                .await?;
            let new_pane = wait_for_pane_created(server_rx).await?;

            let (first_layout, mut all_sinks) = Box::pin(restore_node(
                first,
                current_pane,
                surf_id,
                size,
                client_tx,
                server_rx,
                old_to_new,
                pane_commands,
            ))
            .await?;
            let (second_layout, second_sinks) = Box::pin(restore_node(
                second,
                new_pane,
                surf_id,
                size,
                client_tx,
                server_rx,
                old_to_new,
                pane_commands,
            ))
            .await?;
            all_sinks.extend(second_sinks);

            Ok((
                LayoutNode::Split {
                    direction: *direction,
                    ratio: *ratio,
                    first: Box::new(first_layout),
                    second: Box::new(second_layout),
                },
                all_sinks,
            ))
        }
    }
}

async fn send_command_input(
    client_tx: &mpsc::Sender<ClientMessage>,
    pane: PaneId,
    command: Option<&str>,
) {
    if let Some(cmd) = command {
        let mut input = cmd.as_bytes().to_vec();
        input.push(b'\r');
        let _ = client_tx
            .send(ClientMessage::Input { pane, data: input })
            .await;
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
