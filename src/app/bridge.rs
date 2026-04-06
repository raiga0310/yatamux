use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use yatamux_client::{LayoutNode, NotificationBackend, PaneStore};
use yatamux_protocol::types::{PaneId, SplitDirection, SurfaceId, TermSize};
use yatamux_protocol::{ClientMessage, ServerMessage};
use yatamux_terminal::TerminalSink;

use crate::app::layout_switch::{
    finalize_layout_switch, LayoutName, LayoutPhase, LayoutPlan, PaneLaunchPlan,
};
use crate::config::HooksConfig;
use crate::layout_config::LayoutConfig;

use super::{DEFAULT_COLS, DEFAULT_ROWS};

pub(super) struct ServerBridge {
    pub(super) server_rx: mpsc::Receiver<BridgeEvent>,
    pub(super) client_tx: mpsc::Sender<ClientMessage>,
    pub(super) surf_id: SurfaceId,
    pub(super) size: TermSize,
    pub(super) pane_store: Arc<Mutex<PaneStore>>,
    pub(super) notif_backend: Arc<dyn NotificationBackend>,
    pub(super) hooks: HooksConfig,
    pub(super) sinks: HashMap<PaneId, TerminalSink>,
}

pub(super) struct BridgeChannels {
    pub(super) split_rx: mpsc::Receiver<(PaneId, SplitDirection)>,
    pub(super) float_rx: mpsc::Receiver<()>,
    pub(super) layout_rx: mpsc::Receiver<String>,
}

pub(super) enum BridgeEvent {
    PaneOutput {
        pane: PaneId,
        data: Arc<[u8]>,
    },
    PaneCreated {
        id: PaneId,
        split_from: Option<PaneId>,
        direction: Option<SplitDirection>,
    },
    PaneClosed {
        pane: PaneId,
    },
    UserNotification {
        pane: PaneId,
        body: String,
    },
    CommandFinished {
        pane: PaneId,
        exit_code: Option<i32>,
    },
    PaneMetaUpdated {
        pane: PaneId,
        alias: Option<String>,
        role: Option<String>,
    },
    SaveAndQuit,
    AllPaneProcesses {
        commands: std::collections::HashMap<PaneId, Option<String>>,
        cwds: std::collections::HashMap<PaneId, Option<String>>,
    },
}

impl BridgeEvent {
    fn from_server_message(message: &ServerMessage) -> Option<Self> {
        match message {
            ServerMessage::Output { pane, data } => Some(Self::PaneOutput {
                pane: *pane,
                data: Arc::clone(data),
            }),
            ServerMessage::PaneCreated {
                id,
                split_from,
                direction,
                ..
            } => Some(Self::PaneCreated {
                id: *id,
                split_from: *split_from,
                direction: *direction,
            }),
            ServerMessage::PaneClosed { pane } => Some(Self::PaneClosed { pane: *pane }),
            ServerMessage::Notification { pane, body } => Some(Self::UserNotification {
                pane: *pane,
                body: body.clone(),
            }),
            ServerMessage::CommandFinished { pane, exit_code } => Some(Self::CommandFinished {
                pane: *pane,
                exit_code: *exit_code,
            }),
            ServerMessage::PaneMetaUpdated { pane, alias, role } => Some(Self::PaneMetaUpdated {
                pane: *pane,
                alias: alias.clone(),
                role: role.clone(),
            }),
            ServerMessage::SaveAndQuit => Some(Self::SaveAndQuit),
            ServerMessage::AllPaneProcesses { commands, cwds } => {
                let parse_map = |m: &std::collections::HashMap<String, Option<String>>| {
                    m.iter()
                        .filter_map(|(k, v)| k.parse::<u32>().ok().map(|n| (PaneId(n), v.clone())))
                        .collect()
                };
                Some(Self::AllPaneProcesses {
                    commands: parse_map(commands),
                    cwds: parse_map(cwds),
                })
            }
            _ => None,
        }
    }
}

pub(super) fn spawn_bridge_fanout(
    mut server_rx: mpsc::Receiver<ServerMessage>,
    ipc_out_tx: mpsc::Sender<ServerMessage>,
) -> mpsc::Receiver<BridgeEvent> {
    let (bridge_tx, bridge_rx) = mpsc::channel::<BridgeEvent>(256);
    tokio::spawn(async move {
        while let Some(message) = server_rx.recv().await {
            let bridge_event = BridgeEvent::from_server_message(&message);
            let _ = ipc_out_tx.send(message).await;
            if let Some(event) = bridge_event {
                let _ = bridge_tx.send(event).await;
            }
        }
    });
    bridge_rx
}

pub(super) fn fire_hook(command: &Option<String>, pane: PaneId) {
    if !HooksConfig::is_enabled(command) {
        return;
    }

    let command = command
        .as_deref()
        .expect("enabled hook should contain a command")
        .to_owned();
    let pane_id = pane.0.to_string();
    let session_name =
        std::env::var("YATAMUX_SESSION").unwrap_or_else(|_| crate::DEFAULT_SESSION.to_string());
    tokio::spawn(async move {
        let _ = tokio::process::Command::new("cmd")
            .args(["/C", &command])
            .env("YATAMUX_PANE_ID", pane_id)
            .env("YATAMUX_SESSION", session_name)
            .spawn();
    });
}

fn load_layout_plan(name: &str) -> Option<LayoutPlan> {
    let config_path = LayoutConfig::layout_path(name);
    match LayoutConfig::load(&config_path) {
        Ok(config) => Some(LayoutPlan::from_config(config)),
        Err(e) => {
            tracing::warn!(
                "レイアウト読み込み失敗（{}）: {:#}",
                config_path.display(),
                e
            );
            None
        }
    }
}

pub(super) fn notify_if_inactive(
    pane_store: &Arc<Mutex<PaneStore>>,
    notif_backend: &Arc<dyn NotificationBackend>,
    pane: PaneId,
    body: String,
) {
    let active = pane_store.lock().unwrap().active;
    if pane != active {
        notif_backend.notify(pane, body);
    }
}

pub(super) async fn sync_pane_state(
    client_tx: &mpsc::Sender<ClientMessage>,
    pane_store: &Arc<Mutex<PaneStore>>,
) {
    let (active_pane, floating_pane) = {
        let store = pane_store.lock().unwrap();
        let active = store
            .grids
            .contains_key(&store.active)
            .then_some(store.active);
        let floating = if store.floating_visible {
            store.floating.filter(|pane| store.grids.contains_key(pane))
        } else {
            None
        };
        (active, floating)
    };
    let _ = client_tx
        .send(ClientMessage::SyncPaneState {
            active_pane,
            floating_pane,
        })
        .await;
}

pub(super) fn spawn_server_bridge(bridge: ServerBridge, channels: BridgeChannels) {
    tokio::spawn(async move {
        let ServerBridge {
            mut server_rx,
            client_tx,
            surf_id,
            size,
            pane_store,
            notif_backend,
            hooks,
            mut sinks,
        } = bridge;
        let BridgeChannels {
            mut split_rx,
            mut float_rx,
            mut layout_rx,
        } = channels;
        let mut pending: VecDeque<(PaneId, SplitDirection, TermSize)> = VecDeque::new();
        let mut pending_float = false;
        let mut layout_switch: Option<LayoutPhase> = None;
        // SaveAndQuit 後に AllPaneProcesses を待っているか
        let mut waiting_for_processes = false;
        // SaveAndQuit のタイムアウト用デッドライン
        let mut processes_deadline: Option<tokio::time::Instant> = None;

        loop {
            tokio::select! {
                biased;

                // SaveAndQuit 後の AllPaneProcesses タイムアウト処理
                _ = async {
                    if let Some(dl) = processes_deadline {
                        tokio::time::sleep_until(dl).await;
                    } else {
                        // deadline がなければ永遠に pending（他の arm が先に動く）
                        std::future::pending::<()>().await;
                    }
                }, if waiting_for_processes => {
                    waiting_for_processes = false;
                    processes_deadline = None;
                    let path = yatamux_client::session::LayoutSnapshot::default_path();
                    {
                        let store = pane_store.lock().unwrap();
                        yatamux_client::session::save_session(&store, &path);
                    }
                    pane_store.lock().unwrap().should_quit = true;
                }

                Some(()) = float_rx.recv() => {
                    let floating = pane_store.lock().unwrap().floating;
                    match floating {
                        None => {
                            pending_float = true;
                            let _ = client_tx.send(ClientMessage::CreatePane {
                                surface: surf_id,
                                split_from: None,
                                direction: None,
                                size,
                                working_dir: None,
                            }).await;
                        }
                        Some(_) => {
                            {
                                let mut store = pane_store.lock().unwrap();
                                if store.floating_visible {
                                    store.hide_float();
                                } else {
                                    store.show_float();
                                }
                            }
                            sync_pane_state(&client_tx, &pane_store).await;
                        }
                    }
                }

                Some(name) = layout_rx.recv() => {
                    pending.clear();
                    pending_float = false;

                    let pane_ids: Vec<PaneId> = {
                        let store = pane_store.lock().unwrap();
                        let mut ids: Vec<PaneId> = store.grids.keys().cloned().collect();
                        if let Some(float_id) = store.floating {
                            if !ids.contains(&float_id) {
                                ids.push(float_id);
                            }
                        }
                        ids
                    };
                    let remaining = pane_ids.len();
                    for id in pane_ids {
                        let _ = client_tx.send(ClientMessage::ClosePane { pane: id }).await;
                    }
                    if remaining == 0 {
                        if let Some(plan) = load_layout_plan(&name) {
                            let _ = client_tx.send(ClientMessage::CreatePane {
                                surface: surf_id,
                                split_from: None,
                                direction: None,
                                size,
                                working_dir: None,
                            }).await;
                            layout_switch = Some(LayoutPhase::WaitingFirst { plan });
                        }
                    } else {
                        layout_switch = Some(LayoutPhase::Closing {
                            target: LayoutName::from(name),
                            remaining,
                        });
                    }
                }

                Some((parent_id, direction)) = split_rx.recv() => {
                    let new_size = {
                        let store = pane_store.lock().unwrap();
                        if let Some(g) = store.grids.get(&parent_id) {
                            let g = g.lock().unwrap();
                            match direction {
                                SplitDirection::Vertical =>
                                    TermSize { cols: (g.cols() / 2).max(1), rows: g.rows() },
                                SplitDirection::Horizontal =>
                                    TermSize { cols: g.cols(), rows: (g.rows() / 2).max(1) },
                            }
                        } else {
                            TermSize { cols: DEFAULT_COLS / 2, rows: DEFAULT_ROWS }
                        }
                    };
                    pending.push_back((parent_id, direction, new_size));
                    let _ = client_tx.send(ClientMessage::CreatePane {
                        surface: surf_id,
                        split_from: Some(parent_id),
                        direction: Some(direction),
                        size: new_size,
                        working_dir: None,
                    }).await;
                }

                Some(message) = server_rx.recv() => {
                    match message {
                        BridgeEvent::PaneOutput { pane, data } => {
                            if let Some(sink) = sinks.get_mut(&pane) {
                                if let Some(clip) = sink.feed(&data) {
                                    pane_store.lock().unwrap().pending_clipboard = Some(clip);
                                }
                            }
                        }
                        BridgeEvent::PaneCreated {
                            id: new_id,
                            split_from: ipc_split_from,
                            direction: ipc_direction,
                        } => {
                            fire_hook(&hooks.on_pane_created, new_id);
                            if let Some(phase) = layout_switch.take() {
                                layout_switch = handle_layout_switch_pane_created(
                                    phase,
                                    &pane_store,
                                    &mut sinks,
                                    &client_tx,
                                    surf_id,
                                    size,
                                    new_id,
                                ).await;
                            } else if pending_float {
                                pending_float = false;
                                let float_size = TermSize { cols: DEFAULT_COLS, rows: DEFAULT_ROWS };
                                let new_sink = TerminalSink::new(float_size.cols, float_size.rows);
                                let new_grid = Arc::clone(&new_sink.grid);
                                sinks.insert(new_id, new_sink);
                                let mut store = pane_store.lock().unwrap();
                                store.grids.insert(new_id, new_grid);
                                store.floating = Some(new_id);
                                store.show_float();
                            } else if let Some((parent_id, direction, new_size)) = pending.pop_front() {
                                let new_sink = TerminalSink::new(new_size.cols, new_size.rows);
                                let new_grid = Arc::clone(&new_sink.grid);
                                sinks.insert(new_id, new_sink);
                                {
                                    let mut store = pane_store.lock().unwrap();
                                    if let Some(g) = store.grids.get(&parent_id) {
                                        g.lock().unwrap().resize(new_size.cols, new_size.rows);
                                    }
                                    store.grids.insert(new_id, new_grid);
                                    store.layout.split_leaf(parent_id, new_id, direction);
                                    store.active = new_id;
                                    store.layout_changed = true;
                                }
                                let _ = client_tx.send(ClientMessage::Resize {
                                    pane: parent_id,
                                    size: new_size,
                                }).await;
                            } else if let (Some(parent_id), Some(direction)) =
                                (ipc_split_from, ipc_direction)
                            {
                                let new_sink = TerminalSink::new(size.cols, size.rows);
                                let new_grid = Arc::clone(&new_sink.grid);
                                sinks.insert(new_id, new_sink);
                                let mut store = pane_store.lock().unwrap();
                                store.grids.insert(new_id, new_grid);
                                store.layout.split_leaf(parent_id, new_id, direction);
                                store.active = new_id;
                                store.layout_changed = true;
                            }
                            sync_pane_state(&client_tx, &pane_store).await;
                        }
                        BridgeEvent::PaneClosed { pane } => {
                            fire_hook(&hooks.on_pane_closed, pane);
                            sinks.remove(&pane);
                            {
                                let mut store = pane_store.lock().unwrap();
                                store.grids.remove(&pane);
                                store.pane_aliases.remove(&pane);
                                store.pane_roles.remove(&pane);
                                if store.floating == Some(pane) {
                                    store.floating = None;
                                    store.floating_visible = false;
                                }
                                if layout_switch.is_none() {
                                    let next = store.layout.remove_pane(pane);
                                    if store.active == pane {
                                        if let Some(next_id) = next {
                                            store.active = next_id;
                                        }
                                    }
                                    if store.grids.is_empty() {
                                        store.should_quit = true;
                                    } else {
                                        store.layout_changed = true;
                                    }
                                }
                            }
                            if let Some(LayoutPhase::Closing { remaining, .. }) = &mut layout_switch {
                                *remaining -= 1;
                                if *remaining == 0 {
                                    if let Some(LayoutPhase::Closing { target, .. }) = layout_switch.take() {
                                        if let Some(plan) = load_layout_plan(target.as_ref()) {
                                            let _ = client_tx.send(ClientMessage::CreatePane {
                                                surface: surf_id,
                                                split_from: None,
                                                direction: None,
                                                size,
                                                working_dir: None,
                                            }).await;
                                            layout_switch = Some(LayoutPhase::WaitingFirst { plan });
                                        }
                                    }
                                }
                            }
                            sync_pane_state(&client_tx, &pane_store).await;
                        }
                        BridgeEvent::UserNotification { pane, body } => {
                            notify_if_inactive(&pane_store, &notif_backend, pane, body);
                        }
                        BridgeEvent::CommandFinished { pane, exit_code } => {
                            let body = match exit_code {
                                Some(code) => format!("Command finished (exit {})", code),
                                None => "Command finished".to_string(),
                            };
                            notify_if_inactive(&pane_store, &notif_backend, pane, body);
                        }
                        BridgeEvent::PaneMetaUpdated { pane, alias, role } => {
                            let mut store = pane_store.lock().unwrap();
                            if let Some(alias) = alias {
                                store.pane_aliases.insert(pane, alias);
                            } else {
                                store.pane_aliases.remove(&pane);
                            }
                            if let Some(role) = role {
                                store.pane_roles.insert(pane, role);
                            } else {
                                store.pane_roles.remove(&pane);
                            }
                        }
                        BridgeEvent::SaveAndQuit => {
                            // プロセス名クエリを送信して AllPaneProcesses を待つ
                            let _ = client_tx
                                .send(ClientMessage::QueryAllPaneProcesses)
                                .await;
                            waiting_for_processes = true;
                            processes_deadline = Some(
                                tokio::time::Instant::now()
                                    + std::time::Duration::from_secs(5),
                            );
                        }
                        BridgeEvent::AllPaneProcesses { commands, cwds } => {
                            if waiting_for_processes {
                                waiting_for_processes = false;
                                processes_deadline = None;
                                // pane_commands / pane_cwds を補完
                                {
                                    let mut store = pane_store.lock().unwrap();
                                    for (pane_id, cmd_opt) in commands {
                                        if let Some(cmd) = cmd_opt {
                                            store
                                                .pane_commands
                                                .entry(pane_id)
                                                .or_insert(cmd);
                                        }
                                    }
                                    for (pane_id, cwd_opt) in cwds {
                                        if let Some(cwd) = cwd_opt {
                                            store.pane_cwds.insert(pane_id, cwd);
                                        }
                                    }
                                }
                                let path =
                                    yatamux_client::session::LayoutSnapshot::default_path();
                                {
                                    let store = pane_store.lock().unwrap();
                                    yatamux_client::session::save_session(&store, &path);
                                }
                                pane_store.lock().unwrap().should_quit = true;
                            }
                        }
                    }
                }
            }
        }
    });
}

async fn handle_layout_switch_pane_created(
    phase: LayoutPhase,
    pane_store: &Arc<Mutex<PaneStore>>,
    sinks: &mut HashMap<PaneId, TerminalSink>,
    client_tx: &mpsc::Sender<ClientMessage>,
    surf_id: SurfaceId,
    size: TermSize,
    new_id: PaneId,
) -> Option<LayoutPhase> {
    match phase {
        LayoutPhase::WaitingFirst { plan } => {
            let new_sink = TerminalSink::new(size.cols, size.rows);
            let new_grid = Arc::clone(&new_sink.grid);
            sinks.insert(new_id, new_sink);

            if let Some(cmd) = plan.first_command {
                pane_store
                    .lock()
                    .unwrap()
                    .pane_commands
                    .insert(new_id, cmd.to_string());
                send_command_input(client_tx, new_id, cmd.as_bytes().to_vec()).await;
            }

            let layout = LayoutNode::Leaf(new_id);
            let grids = vec![(new_id, new_grid)];
            let queue = plan.queue;

            if queue.is_empty() {
                finalize_layout_switch(pane_store, layout, grids, new_id);
                None
            } else {
                request_next_layout_pane(client_tx, surf_id, size, new_id, &queue).await;
                Some(LayoutPhase::Applying {
                    queue,
                    layout,
                    grids,
                    prev: new_id,
                    active: new_id,
                })
            }
        }
        LayoutPhase::Applying {
            mut queue,
            mut layout,
            mut grids,
            prev,
            active,
        } => {
            let launch = queue.pop_front().expect("queue should be non-empty");
            let new_sink = TerminalSink::new(size.cols, size.rows);
            let new_grid = Arc::clone(&new_sink.grid);
            sinks.insert(new_id, new_sink);
            grids.push((new_id, new_grid));
            layout.split_leaf_with_ratio(prev, new_id, launch.split, launch.ratio);

            if let Some(command) = launch.command {
                pane_store
                    .lock()
                    .unwrap()
                    .pane_commands
                    .insert(new_id, command.to_string());
                send_command_input(client_tx, new_id, command.as_bytes().to_vec()).await;
            }

            if queue.is_empty() {
                finalize_layout_switch(pane_store, layout, grids, new_id);
                None
            } else {
                request_next_layout_pane(client_tx, surf_id, size, new_id, &queue).await;
                Some(LayoutPhase::Applying {
                    queue,
                    layout,
                    grids,
                    prev: new_id,
                    active,
                })
            }
        }
        other => Some(other),
    }
}

async fn request_next_layout_pane(
    client_tx: &mpsc::Sender<ClientMessage>,
    surf_id: SurfaceId,
    size: TermSize,
    split_from: PaneId,
    queue: &VecDeque<PaneLaunchPlan>,
) {
    let next_direction = queue
        .front()
        .map(|launch| launch.split)
        .expect("queue should be non-empty");
    let _ = client_tx
        .send(ClientMessage::CreatePane {
            surface: surf_id,
            split_from: Some(split_from),
            direction: Some(next_direction),
            size,
            working_dir: None,
        })
        .await;
}

async fn send_command_input(
    client_tx: &mpsc::Sender<ClientMessage>,
    pane: PaneId,
    mut data: Vec<u8>,
) {
    data.push(b'\r');
    let _ = client_tx.send(ClientMessage::Input { pane, data }).await;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use std::path::PathBuf;
    use std::sync::Mutex;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tokio::time::{sleep, timeout, Duration};
    use yatamux_terminal::Grid;

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

    struct NoopNotificationBackend;

    impl NotificationBackend for NoopNotificationBackend {
        fn notify(&self, _pane_id: PaneId, _message: String) {}
    }

    fn make_store() -> Arc<Mutex<PaneStore>> {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        Arc::new(Mutex::new(PaneStore::new(PaneId(1), grid)))
    }

    #[tokio::test]
    async fn save_and_quit_bridge_writes_session_snapshot() {
        let _guard = crate::app::appdata_env_lock()
            .lock()
            .expect("lock APPDATA env");
        let _appdata = AppDataOverride::new("save-and-quit");
        let session_path = yatamux_client::session::LayoutSnapshot::default_path();

        let pane_store = make_store();
        {
            let mut store = pane_store.lock().unwrap();
            store.pane_aliases.insert(PaneId(1), "worker".to_string());
            store.pane_roles.insert(PaneId(1), "agent".to_string());
        }

        let (server_tx, server_rx) = mpsc::channel(8);
        let (ipc_out_tx, _ipc_out_rx) = mpsc::channel(8);
        let bridge_rx = spawn_bridge_fanout(server_rx, ipc_out_tx);
        let (client_tx, mut client_rx) = mpsc::channel(8);
        let (_split_tx, split_rx) = mpsc::channel(1);
        let (_float_tx, float_rx) = mpsc::channel(1);
        let (_layout_tx, layout_rx) = mpsc::channel(1);

        spawn_server_bridge(
            ServerBridge {
                server_rx: bridge_rx,
                client_tx,
                surf_id: SurfaceId(1),
                size: TermSize { cols: 80, rows: 24 },
                pane_store: Arc::clone(&pane_store),
                notif_backend: Arc::new(NoopNotificationBackend),
                hooks: HooksConfig::default(),
                sinks: HashMap::new(),
            },
            BridgeChannels {
                split_rx,
                float_rx,
                layout_rx,
            },
        );

        server_tx
            .send(ServerMessage::SaveAndQuit)
            .await
            .expect("send SaveAndQuit");

        let query = timeout(Duration::from_secs(1), client_rx.recv())
            .await
            .expect("timeout waiting for QueryAllPaneProcesses")
            .expect("client channel closed");
        assert!(matches!(query, ClientMessage::QueryAllPaneProcesses));

        let mut commands = HashMap::new();
        commands.insert("1".to_string(), Some("codex".to_string()));
        let mut cwds = HashMap::new();
        cwds.insert("1".to_string(), Some(r"C:\repo".to_string()));
        server_tx
            .send(ServerMessage::AllPaneProcesses { commands, cwds })
            .await
            .expect("send AllPaneProcesses");

        timeout(Duration::from_secs(2), async {
            loop {
                if session_path.exists() && pane_store.lock().unwrap().should_quit {
                    break;
                }
                sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("timeout waiting for saved session snapshot");

        let snapshot = yatamux_client::session::LayoutSnapshot::load(&session_path)
            .expect("load saved session snapshot");
        assert_eq!(snapshot.active, PaneId(1));
        match snapshot.root {
            yatamux_client::session::LayoutNodeDef::Leaf {
                command,
                cwd,
                alias,
                role,
                ..
            } => {
                assert_eq!(command.as_deref(), Some("codex resume --last"));
                assert_eq!(cwd.as_deref(), Some(r"C:\repo"));
                assert_eq!(alias.as_deref(), Some("worker"));
                assert_eq!(role.as_deref(), Some("agent"));
            }
            other => panic!("expected leaf snapshot, got {:?}", other),
        }
    }
}
