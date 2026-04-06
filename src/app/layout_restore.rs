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
type PaneAliases = HashMap<PaneId, String>;
type PaneRoles = HashMap<PaneId, String>;

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
    PaneAliases,
    PaneRoles,
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
                    PaneAliases::new(),
                    PaneRoles::new(),
                ))
            }
        }
    } else if let Ok(snapshot) = LayoutSnapshot::load(&session_path) {
        tracing::info!("セッションを復元します");
        let mut old_to_new: HashMap<PaneId, PaneId> = HashMap::new();
        let mut pane_commands = PaneCommands::new();
        let mut pane_aliases = PaneAliases::new();
        let mut pane_roles = PaneRoles::new();
        let (layout, sinks) = restore_node(
            &snapshot.root,
            pane_id,
            surf_id,
            size,
            client_tx,
            server_rx,
            &mut old_to_new,
            &mut pane_commands,
            &mut pane_aliases,
            &mut pane_roles,
        )
        .await?;
        let active = old_to_new.get(&snapshot.active).copied().unwrap_or(pane_id);
        Ok((
            layout,
            sinks,
            active,
            pane_commands,
            pane_aliases,
            pane_roles,
        ))
    } else {
        let sink = TerminalSink::new(size.cols, size.rows);
        Ok((
            LayoutNode::Leaf(pane_id),
            vec![(pane_id, sink)],
            pane_id,
            PaneCommands::new(),
            PaneAliases::new(),
            PaneRoles::new(),
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
    PaneAliases,
    PaneRoles,
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

    Ok((
        layout,
        sinks,
        active,
        pane_commands,
        PaneAliases::new(),
        PaneRoles::new(),
    ))
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
    pane_aliases: &mut PaneAliases,
    pane_roles: &mut PaneRoles,
) -> Result<(LayoutNode, Vec<(PaneId, TerminalSink)>)> {
    match def {
        LayoutNodeDef::Leaf {
            id: old_id,
            command,
            cwd,
            alias,
            role,
        } => {
            old_to_new.insert(*old_id, current_pane);
            let sink = TerminalSink::new(size.cols, size.rows);
            // cwd が記録されていれば先に cd してから command を実行する
            if let Some(dir) = cwd {
                // Windows: `cd /d "<path>"` でドライブをまたいで移動（引用符でスペース対応）
                send_command_input(client_tx, current_pane, Some(&format!("cd /d \"{}\"", dir)))
                    .await;
            }
            if let Some(cmd) = command {
                pane_commands.insert(current_pane, cmd.clone());
                send_command_input(client_tx, current_pane, Some(cmd.as_str())).await;
            }
            if let Some(alias) = alias {
                pane_aliases.insert(current_pane, alias.clone());
            }
            if let Some(role) = role {
                pane_roles.insert(current_pane, role.clone());
            }
            if alias.is_some() || role.is_some() {
                let _ = client_tx
                    .send(ClientMessage::SetPaneMeta {
                        pane: current_pane,
                        alias: alias.clone(),
                        role: role.clone(),
                    })
                    .await;
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
                pane_aliases,
                pane_roles,
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
                pane_aliases,
                pane_roles,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct AppDataOverride {
        previous: Option<OsString>,
        root: PathBuf,
    }

    impl AppDataOverride {
        fn new(prefix: &str) -> Self {
            let unique = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos();
            let root = std::env::temp_dir().join(format!(
                "yatamux-{}-{}-{}",
                prefix,
                std::process::id(),
                unique
            ));
            std::fs::create_dir_all(&root).expect("create temp appdata dir");
            let previous = std::env::var_os("APPDATA");
            unsafe {
                std::env::set_var("APPDATA", &root);
            }
            Self { previous, root }
        }
    }

    impl Drop for AppDataOverride {
        fn drop(&mut self) {
            match &self.previous {
                Some(previous) => unsafe {
                    std::env::set_var("APPDATA", previous);
                },
                None => unsafe {
                    std::env::remove_var("APPDATA");
                },
            }
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn load_initial_layout_restores_session_snapshot_and_metadata() {
        let _guard = crate::app::appdata_env_lock()
            .lock()
            .expect("lock APPDATA env");
        let _appdata = AppDataOverride::new("restore-session");

        let session_path = LayoutSnapshot::default_path();
        let snapshot = LayoutSnapshot {
            root: LayoutNodeDef::Split {
                direction: SplitDirection::Vertical,
                ratio: 0.4,
                first: Box::new(LayoutNodeDef::Leaf {
                    id: PaneId(1),
                    command: Some("codex resume --last".to_string()),
                    cwd: Some(r"C:\worktree".to_string()),
                    alias: Some("main".to_string()),
                    role: Some("planner".to_string()),
                }),
                second: Box::new(LayoutNodeDef::Leaf {
                    id: PaneId(2),
                    command: Some("cargo test -q".to_string()),
                    cwd: None,
                    alias: Some("tests".to_string()),
                    role: Some("verifier".to_string()),
                }),
            },
            active: PaneId(2),
        };
        snapshot
            .save(&session_path)
            .expect("save test session snapshot");

        let (client_tx, mut client_rx) = mpsc::channel(16);
        let (server_tx, mut server_rx) = mpsc::channel(4);
        server_tx
            .send(ServerMessage::PaneCreated {
                id: PaneId(99),
                surface: SurfaceId(1),
                split_from: Some(PaneId(10)),
                direction: Some(SplitDirection::Vertical),
            })
            .await
            .expect("queue PaneCreated response");

        let (layout, sinks, active, pane_commands, pane_aliases, pane_roles) = load_initial_layout(
            None,
            PaneId(10),
            SurfaceId(1),
            TermSize { cols: 80, rows: 24 },
            &client_tx,
            &mut server_rx,
        )
        .await
        .expect("restore session snapshot");

        assert_eq!(active, PaneId(99));
        assert_eq!(sinks.len(), 2);
        match layout {
            LayoutNode::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                assert_eq!(direction, SplitDirection::Vertical);
                assert!((ratio - 0.4).abs() < f32::EPSILON);
                assert!(matches!(*first, LayoutNode::Leaf(PaneId(10))));
                assert!(matches!(*second, LayoutNode::Leaf(PaneId(99))));
            }
            other => panic!("expected restored split layout, got {:?}", other),
        }

        assert_eq!(
            pane_commands.get(&PaneId(10)).map(String::as_str),
            Some("codex resume --last")
        );
        assert_eq!(
            pane_commands.get(&PaneId(99)).map(String::as_str),
            Some("cargo test -q")
        );
        assert_eq!(
            pane_aliases.get(&PaneId(10)).map(String::as_str),
            Some("main")
        );
        assert_eq!(
            pane_aliases.get(&PaneId(99)).map(String::as_str),
            Some("tests")
        );
        assert_eq!(
            pane_roles.get(&PaneId(10)).map(String::as_str),
            Some("planner")
        );
        assert_eq!(
            pane_roles.get(&PaneId(99)).map(String::as_str),
            Some("verifier")
        );

        let mut messages = Vec::new();
        while let Ok(message) = client_rx.try_recv() {
            messages.push(message);
        }

        assert!(messages.iter().any(|message| {
            matches!(
                message,
                ClientMessage::CreatePane {
                    surface,
                    split_from: Some(split_from),
                    direction: Some(direction),
                    ..
                } if *surface == SurfaceId(1)
                    && *split_from == PaneId(10)
                    && *direction == SplitDirection::Vertical
            )
        }));
        assert!(messages.iter().any(|message| {
            matches!(
                message,
                ClientMessage::Input { pane, data }
                    if *pane == PaneId(10)
                        && data == &format!("cd /d \"{}\"\r", r"C:\worktree").into_bytes()
            )
        }));
        assert!(messages.iter().any(|message| {
            matches!(
                message,
                ClientMessage::SetPaneMeta {
                    pane,
                    alias,
                    role,
                } if *pane == PaneId(99)
                    && alias.as_deref() == Some("tests")
                    && role.as_deref() == Some("verifier")
            )
        }));
    }
}
