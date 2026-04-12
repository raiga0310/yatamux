//! CLI サブコマンド実装
//!
//! 実行中の yatamux セッションに IPC 経由で接続し、
//! `list-panes` / `send-keys` を実行する。

use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use std::io::{self, Write};
use std::path::Path;
use std::time::Duration;
use yatamux_client::connection::ServerConnection;
use yatamux_protocol::types::{
    ExecStatus, ExecWaitCondition, PaneCapture, PaneId, PaneInfo, SplitDirection, SurfaceId,
};
use yatamux_protocol::{ClientMessage, ServerMessage};

#[derive(Serialize)]
struct CapturePaneJsonOutput {
    pane: PaneId,
    content: String,
    #[serde(flatten)]
    capture: PaneCapture,
}

#[derive(Serialize)]
#[serde(tag = "event", rename_all = "snake_case")]
enum SubscribePaneJsonEvent {
    Output {
        pane: PaneId,
        text: String,
    },
    Notification {
        pane: PaneId,
        body: String,
    },
    CommandFinished {
        pane: PaneId,
        exit_code: Option<i32>,
    },
    PaneClosed {
        pane: PaneId,
    },
    Lagged {
        pane: PaneId,
        message: String,
    },
}

#[derive(Debug)]
enum WaitCondition {
    Exit,
    Silence(Duration),
    OutputRegex { regex: Regex, lines: usize },
}

#[derive(Debug)]
struct WaitResult {
    exit_code: Option<i32>,
}

#[derive(Debug)]
enum PaneWaitKind {
    Condition(WaitCondition),
    PaneClosed,
}

#[derive(Debug)]
struct WaitState {
    last_activity: tokio::time::Instant,
    next_capture_at: tokio::time::Instant,
    last_exit_code: Option<i32>,
}

impl WaitState {
    fn new(now: tokio::time::Instant) -> Self {
        Self {
            last_activity: now,
            next_capture_at: now,
            last_exit_code: None,
        }
    }
}

#[derive(Debug)]
enum WaitDecision {
    Continue,
    Done(WaitResult),
    Error(anyhow::Error),
}

pub struct WaitOptions {
    pub wait_for: crate::WaitForArg,
    pub timeout_secs: u64,
    pub output_regex: Option<String>,
    pub silence_ms: u64,
    pub lines: usize,
}

fn build_wait_condition(
    wait_for: crate::WaitForArg,
    output_regex: Option<&str>,
    silence_ms: u64,
    lines: usize,
) -> Result<WaitCondition> {
    match wait_for {
        crate::WaitForArg::Exit => {
            if output_regex.is_some() {
                anyhow::bail!("--output-regex can only be used with --wait-for output-regex");
            }
            Ok(WaitCondition::Exit)
        }
        crate::WaitForArg::Silence => {
            if output_regex.is_some() {
                anyhow::bail!("--output-regex can only be used with --wait-for output-regex");
            }
            Ok(WaitCondition::Silence(Duration::from_millis(silence_ms)))
        }
        crate::WaitForArg::OutputRegex => {
            let pattern =
                output_regex.ok_or_else(|| anyhow::anyhow!("--output-regex is required"))?;
            let regex = Regex::new(pattern)
                .with_context(|| format!("invalid --output-regex pattern: {}", pattern))?;
            Ok(WaitCondition::OutputRegex { regex, lines })
        }
    }
}

fn build_exec_wait_condition(
    wait_for: crate::WaitForArg,
    output_regex: Option<&str>,
    silence_ms: u64,
    lines: usize,
) -> Result<ExecWaitCondition> {
    match wait_for {
        crate::WaitForArg::Exit => {
            if output_regex.is_some() {
                anyhow::bail!("--output-regex can only be used with --wait-for output-regex");
            }
            Ok(ExecWaitCondition::Exit)
        }
        crate::WaitForArg::Silence => {
            if output_regex.is_some() {
                anyhow::bail!("--output-regex can only be used with --wait-for output-regex");
            }
            Ok(ExecWaitCondition::Silence { silence_ms })
        }
        crate::WaitForArg::OutputRegex => {
            let pattern =
                output_regex.ok_or_else(|| anyhow::anyhow!("--output-regex is required"))?;
            Regex::new(pattern)
                .with_context(|| format!("invalid --output-regex pattern: {}", pattern))?;
            Ok(ExecWaitCondition::OutputRegex {
                pattern: pattern.to_string(),
                lines,
            })
        }
    }
}

fn join_command(command: &[String]) -> String {
    command.join(" ")
}

fn next_request_id(prefix: &str, pane: PaneId) -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default();
    format!(
        "{}-{}-{}-{}",
        prefix,
        std::process::id(),
        pane.0,
        now.as_nanos()
    )
}

fn maybe_exit_with_code(result: WaitResult) {
    if let Some(code) = result.exit_code {
        if code != 0 {
            eprintln!("Command exited with status {}", code);
            std::process::exit(code);
        }
    }
}

fn handle_wait_message(
    kind: &PaneWaitKind,
    pane: PaneId,
    pane_id: u32,
    msg: ServerMessage,
    state: &mut WaitState,
) -> WaitDecision {
    match msg {
        ServerMessage::Output {
            pane: output_pane, ..
        } if output_pane == pane => {
            state.last_activity = tokio::time::Instant::now();
            WaitDecision::Continue
        }
        ServerMessage::CommandFinished {
            pane: finished_pane,
            exit_code,
        } if finished_pane == pane => {
            state.last_activity = tokio::time::Instant::now();
            state.last_exit_code = exit_code;
            match kind {
                PaneWaitKind::Condition(WaitCondition::Exit) => {
                    WaitDecision::Done(WaitResult { exit_code })
                }
                _ => WaitDecision::Continue,
            }
        }
        ServerMessage::PaneClosed { pane: closed_pane } if closed_pane == pane => match kind {
            PaneWaitKind::PaneClosed => WaitDecision::Done(WaitResult {
                exit_code: state.last_exit_code,
            }),
            PaneWaitKind::Condition(WaitCondition::OutputRegex { .. }) => WaitDecision::Error(
                anyhow::anyhow!("pane {} closed before regex matched", pane_id),
            ),
            PaneWaitKind::Condition(_) => WaitDecision::Done(WaitResult {
                exit_code: state.last_exit_code,
            }),
        },
        ServerMessage::PaneContent {
            pane: content_pane,
            content,
            ..
        } if content_pane == pane => {
            if let PaneWaitKind::Condition(WaitCondition::OutputRegex { regex, .. }) = kind {
                if regex.is_match(&content) {
                    return WaitDecision::Done(WaitResult {
                        exit_code: state.last_exit_code,
                    });
                }
            }
            WaitDecision::Continue
        }
        ServerMessage::Error { message, .. } => WaitDecision::Error(anyhow::anyhow!("{}", message)),
        _ => WaitDecision::Continue,
    }
}

async fn wait_for_pane(
    conn: &mut ServerConnection,
    pane_id: u32,
    kind: &PaneWaitKind,
    timeout: Duration,
) -> Result<WaitResult> {
    let pane = PaneId(pane_id);
    let started = tokio::time::Instant::now();
    let mut state = WaitState::new(started);

    loop {
        if started.elapsed() >= timeout {
            anyhow::bail!("timeout waiting for pane {}", pane_id);
        }

        if let PaneWaitKind::Condition(WaitCondition::Silence(duration)) = kind {
            if state.last_activity.elapsed() >= *duration {
                return Ok(WaitResult {
                    exit_code: state.last_exit_code,
                });
            }
        }

        if let PaneWaitKind::Condition(WaitCondition::OutputRegex { lines, .. }) = kind {
            if tokio::time::Instant::now() >= state.next_capture_at {
                conn.tx
                    .send(ClientMessage::CapturePane {
                        pane,
                        lines: *lines,
                        plain_text: true,
                    })
                    .await?;
                state.next_capture_at = tokio::time::Instant::now() + Duration::from_millis(200);
            }
        }

        let mut next_wait = Duration::from_millis(100);
        let timeout_left = timeout.saturating_sub(started.elapsed());
        if timeout_left < next_wait {
            next_wait = timeout_left;
        }
        if let PaneWaitKind::Condition(WaitCondition::Silence(duration)) = kind {
            let silence_left = duration.saturating_sub(state.last_activity.elapsed());
            if silence_left < next_wait {
                next_wait = silence_left;
            }
        }
        if let PaneWaitKind::Condition(WaitCondition::OutputRegex { .. }) = kind {
            let capture_left = state
                .next_capture_at
                .saturating_duration_since(tokio::time::Instant::now());
            if capture_left < next_wait {
                next_wait = capture_left;
            }
        }
        if next_wait.is_zero() {
            next_wait = Duration::from_millis(1);
        }

        match tokio::time::timeout(next_wait, conn.rx.recv()).await {
            Ok(Some(msg)) => match handle_wait_message(kind, pane, pane_id, msg, &mut state) {
                WaitDecision::Continue => {}
                WaitDecision::Done(result) => return Ok(result),
                WaitDecision::Error(err) => return Err(err),
            },
            Ok(None) => {
                anyhow::bail!(
                    "server closed connection while waiting for pane {}",
                    pane_id
                );
            }
            Err(_) => {}
        }
    }
}

async fn wait_for_exec_result(
    conn: &mut ServerConnection,
    request_id: &str,
    timeout: Duration,
) -> Result<WaitResult> {
    tokio::time::timeout(timeout, async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::ExecResult {
                    request_id: result_id,
                    status,
                    exit_code,
                    message,
                    ..
                }) if result_id == request_id => match status {
                    ExecStatus::Completed => return Ok(WaitResult { exit_code }),
                    ExecStatus::TimedOut | ExecStatus::PaneClosed | ExecStatus::Error => {
                        return Err(anyhow::anyhow!(
                            "{}",
                            message
                                .unwrap_or_else(|| format!("exec request {} failed", request_id))
                        ));
                    }
                },
                Some(ServerMessage::Error { message, .. }) => {
                    return Err(anyhow::anyhow!("{}", message));
                }
                Some(_) => continue,
                None => {
                    return Err(anyhow::anyhow!(
                        "server closed connection before sending ExecResult"
                    ));
                }
            }
        }
    })
    .await
    .context("timeout waiting for exec result")?
}

async fn wait_for_pane_meta_update(
    conn: &mut ServerConnection,
    pane: PaneId,
    alias: Option<&str>,
    role: Option<&str>,
    timeout: Duration,
) -> Result<()> {
    tokio::time::timeout(timeout, async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::PaneMetaUpdated {
                    pane: updated_pane,
                    alias: updated_alias,
                    role: updated_role,
                }) if updated_pane == pane => {
                    if updated_alias.as_deref() == alias && updated_role.as_deref() == role {
                        return Ok(());
                    }
                }
                Some(ServerMessage::Error { message, .. }) => {
                    return Err(anyhow::anyhow!("{}", message));
                }
                Some(_) => continue,
                None => {
                    return Err(anyhow::anyhow!(
                        "server closed connection before sending PaneMetaUpdated"
                    ));
                }
            }
        }
    })
    .await
    .context("timeout waiting for pane metadata update")?
}

async fn wait_for_input_accept(
    conn: &mut ServerConnection,
    pane: PaneId,
    timeout: Duration,
) -> Result<()> {
    tokio::time::timeout(timeout, async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::InputAccepted {
                    pane: accepted_pane,
                }) if accepted_pane == pane => {
                    return Ok(());
                }
                Some(ServerMessage::Error { message, .. }) => {
                    return Err(anyhow::anyhow!("{}", message));
                }
                Some(_) => continue,
                None => {
                    return Err(anyhow::anyhow!(
                        "server closed connection before sending InputAccepted"
                    ));
                }
            }
        }
    })
    .await
    .context("timeout waiting for input acceptance")?
}

async fn request_panes(conn: &mut ServerConnection) -> Result<Vec<PaneInfo>> {
    conn.tx.send(ClientMessage::ListPanes).await?;

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::PanesListed { panes }) => return Ok(panes),
                Some(ServerMessage::Error { message, .. }) => {
                    return Err(anyhow::anyhow!("{}", message))
                }
                Some(_) => continue,
                None => {
                    return Err(anyhow::anyhow!(
                        "server closed connection before sending PanesListed"
                    ))
                }
            }
        }
    })
    .await
    .context("timeout waiting for pane list")?
}

fn find_pane_by_selector<'a>(panes: &'a [PaneInfo], selector: &str) -> Option<&'a PaneInfo> {
    if let Ok(id) = selector.parse::<u32>() {
        panes.iter().find(|p| p.id == PaneId(id))
    } else {
        panes
            .iter()
            .find(|p| p.alias.as_deref() == Some(selector.trim()))
    }
}

fn resolve_existing_pane(panes: &[PaneInfo], selector: &str) -> Result<PaneInfo> {
    find_pane_by_selector(panes, selector)
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("pane '{}' not found", selector))
}

fn resolve_pane_for_request(panes: &[PaneInfo], selector: &str) -> Result<PaneId> {
    if let Ok(id) = selector.parse::<u32>() {
        Ok(PaneId(id))
    } else {
        resolve_existing_pane(panes, selector).map(|pane| pane.id)
    }
}

fn normalize_meta_value(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let trimmed = value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    })
}

fn subscription_event_from_message(
    pane: PaneId,
    msg: ServerMessage,
) -> Result<Option<SubscribePaneJsonEvent>> {
    match msg {
        ServerMessage::Output {
            pane: output_pane,
            data,
        } if output_pane == pane => Ok(Some(SubscribePaneJsonEvent::Output {
            pane: output_pane,
            text: String::from_utf8_lossy(&data).into_owned(),
        })),
        ServerMessage::Notification {
            pane: notify_pane,
            body,
        } if notify_pane == pane => Ok(Some(SubscribePaneJsonEvent::Notification {
            pane: notify_pane,
            body,
        })),
        ServerMessage::CommandFinished {
            pane: finished_pane,
            exit_code,
        } if finished_pane == pane => Ok(Some(SubscribePaneJsonEvent::CommandFinished {
            pane: finished_pane,
            exit_code,
        })),
        ServerMessage::PaneClosed { pane: closed_pane } if closed_pane == pane => {
            Ok(Some(SubscribePaneJsonEvent::PaneClosed {
                pane: closed_pane,
            }))
        }
        ServerMessage::Error { message, .. } if message.contains("subscription lagged by") => {
            Ok(Some(SubscribePaneJsonEvent::Lagged { pane, message }))
        }
        ServerMessage::Error { message, .. } => Err(anyhow::anyhow!("{}", message)),
        _ => Ok(None),
    }
}

fn is_running_inside_yatamux() -> bool {
    matches!(std::env::var("YATAMUX").as_deref(), Ok("1"))
}

fn cleanup_staged_update(new_path: &Path) {
    if new_path.exists() {
        if let Err(err) = std::fs::remove_file(new_path) {
            eprintln!(
                "警告: staging 済みバイナリの削除に失敗しました: {} ({:#})",
                new_path.display(),
                err
            );
        }
    }
}

fn build_apply_update_command(
    exe: &Path,
    wait_pid: u32,
    new_path: &Path,
    launch: bool,
) -> std::process::Command {
    #[cfg(windows)]
    use std::os::windows::process::CommandExt;

    #[cfg(windows)]
    const DETACHED_PROCESS: u32 = 0x0000_0008;
    #[cfg(windows)]
    const CREATE_NEW_PROCESS_GROUP: u32 = 0x0000_0200;
    #[cfg(windows)]
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;

    let mut command = std::process::Command::new(exe);
    if let Ok(session) = std::env::var("YATAMUX_SESSION") {
        let session = session.trim();
        if !session.is_empty() {
            command.arg("--session").arg(session);
        }
    }
    command.args([
        "--apply-update",
        &wait_pid.to_string(),
        &new_path.to_string_lossy(),
    ]);
    if launch {
        command.arg("--launch");
    }
    // Keep the helper detached from the caller's stdio so `yatamux update`
    // can finish promptly even when the helper relaunches a new GUI process.
    command
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null());
    #[cfg(windows)]
    command.creation_flags(DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_NO_WINDOW);
    command
}

fn spawn_apply_update_helper(
    exe: &Path,
    wait_pid: u32,
    new_path: &Path,
    launch: bool,
) -> Result<()> {
    build_apply_update_command(exe, wait_pid, new_path, launch)
        .spawn()
        .context("アップデートヘルパーの起動に失敗")?;
    Ok(())
}

/// `yatamux list-panes [--json]` — 実行中のペイン一覧を標準出力に表示する
///
/// `--json` を付けると JSON 配列形式で出力する（C-24）。
pub async fn list_panes(session: &str, json: bool) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&panes)?);
    } else if panes.is_empty() {
        println!("(no panes)");
    } else {
        println!(
            "{:<6} {:<8} {:<14} {:<14} {:<6} {:<6} title",
            "pane", "surface", "alias", "role", "cols", "rows"
        );
        println!("{}", "-".repeat(72));
        for p in &panes {
            println!(
                "{:<6} {:<8} {:<14} {:<14} {:<6} {:<6} {}",
                p.id.0,
                p.surface.0,
                p.alias.as_deref().unwrap_or("-"),
                p.role.as_deref().unwrap_or("-"),
                p.cols,
                p.rows,
                p.title
            );
        }
    }
    Ok(())
}

/// `yatamux capture-pane --target <id> --lines <n> [--plain-text]` — ペインの内容を表示する
///
/// スクロールバック末尾 N 行 + 現在画面の内容を標準出力に表示する。
/// `--plain-text` を付けると ANSI エスケープを除去したプレーンテキストで出力する（C-26）。
pub async fn capture_pane(
    session: &str,
    selector: &str,
    lines: usize,
    plain_text: bool,
    json: bool,
) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    let pane_id = resolve_pane_for_request(&panes, selector)?;

    conn.tx
        .send(ClientMessage::CapturePane {
            pane: pane_id,
            lines,
            plain_text,
        })
        .await?;

    let response = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::PaneContent {
                    pane,
                    content,
                    capture,
                }) => return Ok((pane, content, capture)),
                Some(ServerMessage::Error { message, .. }) => {
                    return Err(anyhow::anyhow!("{}", message))
                }
                Some(_) => continue,
                None => {
                    return Err(anyhow::anyhow!(
                        "server closed connection before sending PaneContent"
                    ))
                }
            }
        }
    })
    .await
    .context("timeout waiting for pane content")??;

    let (pane, content, capture) = response;
    if json {
        let capture = capture.context("server did not provide capture metadata")?;
        let output = CapturePaneJsonOutput {
            pane,
            content,
            capture,
        };
        println!("{}", serde_json::to_string_pretty(&output)?);
    } else {
        print!("{}", content);
    }
    Ok(())
}

/// `yatamux subscribe-pane --pane <id>` — 指定ペインの出力ストリームを購読する
pub async fn subscribe_pane(session: &str, selector: &str, json: bool) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    let pane = resolve_existing_pane(&panes, selector)?;

    conn.tx
        .send(ClientMessage::SubscribePane {
            pane: pane.id,
            request_id: None,
        })
        .await?;

    loop {
        let msg = conn.rx.recv().await.ok_or_else(|| {
            anyhow::anyhow!(
                "server closed connection while streaming pane {}",
                pane.id.0
            )
        })?;

        let Some(event) = subscription_event_from_message(pane.id, msg)? else {
            continue;
        };

        if json {
            println!("{}", serde_json::to_string(&event)?);
            if matches!(event, SubscribePaneJsonEvent::PaneClosed { .. }) {
                return Ok(());
            }
            continue;
        }

        match event {
            SubscribePaneJsonEvent::Output { text, .. } => {
                let mut stdout = io::stdout();
                stdout.write_all(text.as_bytes())?;
                stdout.flush()?;
            }
            SubscribePaneJsonEvent::Notification { body, .. } => {
                eprintln!("notification: {}", body);
            }
            SubscribePaneJsonEvent::CommandFinished { exit_code, .. } => match exit_code {
                Some(code) => eprintln!("command finished: {}", code),
                None => eprintln!("command finished"),
            },
            SubscribePaneJsonEvent::PaneClosed { pane } => {
                eprintln!("pane {} closed", pane.0);
                return Ok(());
            }
            SubscribePaneJsonEvent::Lagged { message, .. } => {
                eprintln!("{}", message);
            }
        }
    }
}

/// `yatamux split-pane --target <id> --direction <v|h> --dir <path>` — ペインを分割する
///
/// 指定ペインを分割して新しいペインを作成する。
/// `--dir` で作業ディレクトリを指定できる。
pub async fn split_pane(
    session: &str,
    target_selector: Option<&str>,
    direction: SplitDirection,
    working_dir: Option<String>,
) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;

    // まずペイン一覧を取得してサーフェス ID を取得する
    let panes = request_panes(&mut conn).await?;

    // 対象ペインを探す（C-25: 見つからない場合はエラー終了）
    let target_pane = match target_selector {
        None | Some("0") => panes.first(),
        Some(selector) => Some(
            find_pane_by_selector(&panes, selector)
                .ok_or_else(|| anyhow::anyhow!("pane '{}' not found", selector))?,
        ),
    };

    let surface = target_pane.map(|p| p.surface).unwrap_or(SurfaceId(1));

    // split_from には実際に存在するペイン ID を使う
    // デフォルトの --target 0 は存在しないため、フォールバック後の ID を使わないと
    // split_pane_tree がツリー内で対象 Leaf を見つけられず、新ペインがツリーに入らない
    let split_from_id = target_pane.map(|p| p.id).unwrap_or(PaneId(0));

    let size = target_pane
        .map(|p| yatamux_protocol::types::TermSize {
            cols: p.cols,
            rows: p.rows,
        })
        .unwrap_or(yatamux_protocol::types::TermSize { cols: 80, rows: 24 });

    conn.tx
        .send(ClientMessage::CreatePane {
            surface,
            split_from: Some(split_from_id),
            direction: Some(direction),
            size,
            working_dir,
        })
        .await?;

    let new_pane = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::PaneCreated { id, .. }) => return Ok(id),
                Some(ServerMessage::Error { message, .. }) => {
                    return Err(anyhow::anyhow!("{}", message))
                }
                Some(_) => continue,
                None => {
                    return Err(anyhow::anyhow!(
                        "server closed connection before sending PaneCreated"
                    ))
                }
            }
        }
    })
    .await
    .context("timeout waiting for pane creation")??;

    println!("Created pane {}", new_pane.0);
    Ok(())
}

/// `yatamux send-keys --pane <id> [--enter] [--raw] [--wait-for-prompt] <text>` — 指定ペインにテキストを送信する
///
/// - `--enter`: 末尾に CR (0x0D) を自動付加する。コマンド実行に使用。
/// - `--raw`: エスケープ変換を無効化してテキストをそのまま送信する。Windows パスに使用。
/// - `--wait-for-prompt`: コマンド完了（OSC 133;D）を受信するまで待機してから終了する（C-27）。
/// - デフォルト（オプションなし）: `\n`=LF、`\r`=CR、`\t`=TAB、`\\`=バックスラッシュ に変換。
pub async fn send_keys(
    session: &str,
    selector: &str,
    text: &str,
    enter: bool,
    raw: bool,
    wait_for_prompt: bool,
) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;

    // ペイン存在チェック（C-25: 存在しないペインへの操作でエラー終了）
    let panes = request_panes(&mut conn).await?;

    let pane = resolve_existing_pane(&panes, selector)?;

    let mut data = if raw {
        text.as_bytes().to_vec()
    } else {
        unescape(text)
    };
    if enter {
        data.push(b'\r');
    }
    conn.tx
        .send(ClientMessage::Input {
            pane: pane.id,
            data,
        })
        .await?;
    wait_for_input_accept(&mut conn, pane.id, Duration::from_secs(5)).await?;

    // --wait-for-prompt: 対象ペインの CommandFinished を受信するまで待機（C-27）
    if wait_for_prompt {
        let result = wait_for_pane(
            &mut conn,
            pane.id.0,
            &PaneWaitKind::Condition(WaitCondition::Exit),
            Duration::from_secs(60),
        )
        .await
        .context("timeout waiting for command to finish (60s)")?;
        maybe_exit_with_code(result);
    }

    Ok(())
}

/// `yatamux wait-pane --pane <id> ...` — 指定ペインが条件を満たすまで待機する
pub async fn wait_pane(session: &str, selector: &str, options: WaitOptions) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    let pane = resolve_existing_pane(&panes, selector)?;

    let condition = build_wait_condition(
        options.wait_for,
        options.output_regex.as_deref(),
        options.silence_ms,
        options.lines,
    )?;
    let result = wait_for_pane(
        &mut conn,
        pane.id.0,
        &PaneWaitKind::Condition(condition),
        Duration::from_secs(options.timeout_secs),
    )
    .await?;
    maybe_exit_with_code(result);
    Ok(())
}

/// `yatamux exec --pane <id> -- <command>` — コマンド送信と待機をまとめて行う
pub async fn exec_command(
    session: &str,
    selector: &str,
    command: Vec<String>,
    raw: bool,
    options: WaitOptions,
) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    let pane = resolve_existing_pane(&panes, selector)?;

    let wait = build_exec_wait_condition(
        options.wait_for,
        options.output_regex.as_deref(),
        options.silence_ms,
        options.lines,
    )?;
    let command_text = join_command(&command);
    let mut data = if raw {
        command_text.into_bytes()
    } else {
        unescape(&command_text)
    };
    data.push(b'\r');
    let request_id = next_request_id("exec", pane.id);

    conn.tx
        .send(ClientMessage::Exec {
            request_id: request_id.clone(),
            pane: pane.id,
            data,
            wait,
            timeout_ms: options.timeout_secs.saturating_mul(1000),
        })
        .await?;

    let result = wait_for_exec_result(
        &mut conn,
        &request_id,
        Duration::from_secs(options.timeout_secs.saturating_add(5)),
    )
    .await?;
    maybe_exit_with_code(result);
    Ok(())
}

/// `yatamux interrupt-pane --pane <id>` — 指定ペインに Ctrl+C を送信する
pub async fn interrupt_pane(session: &str, selector: &str) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    let pane = resolve_existing_pane(&panes, selector)?;

    conn.tx
        .send(ClientMessage::InterruptPane {
            pane: pane.id,
            request_id: None,
        })
        .await?;
    println!("Interrupted pane {}", pane.id.0);
    Ok(())
}

/// `yatamux close-pane --pane <id>` — 指定ペインを閉じる
pub async fn close_pane(session: &str, selector: &str) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    let pane = resolve_existing_pane(&panes, selector)?;

    conn.tx
        .send(ClientMessage::ClosePane {
            pane: pane.id,
            request_id: None,
        })
        .await?;

    wait_for_pane_closed(&mut conn, pane.id.0).await?;

    println!("Closed pane {}", pane.id.0);
    Ok(())
}

/// `yatamux terminate-pane --pane <id>` — 指定ペインの子プロセスを強制終了する
pub async fn terminate_pane(session: &str, selector: &str) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    let pane = resolve_existing_pane(&panes, selector)?;

    conn.tx
        .send(ClientMessage::TerminatePane {
            pane: pane.id,
            request_id: None,
        })
        .await?;

    wait_for_pane_closed(&mut conn, pane.id.0).await?;

    println!("Terminated pane {}", pane.id.0);
    Ok(())
}

pub async fn set_pane_meta(
    session: &str,
    selector: &str,
    alias: Option<String>,
    role: Option<String>,
) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    let pane = resolve_existing_pane(&panes, selector)?;

    let alias = normalize_meta_value(alias).or_else(|| pane.alias.clone());
    let role = normalize_meta_value(role).or_else(|| pane.role.clone());
    if alias.is_none() && role.is_none() {
        anyhow::bail!("at least one of --alias or --role is required");
    }

    conn.tx
        .send(ClientMessage::SetPaneMeta {
            pane: pane.id,
            alias: alias.clone(),
            role: role.clone(),
        })
        .await?;
    wait_for_pane_meta_update(
        &mut conn,
        pane.id,
        alias.as_deref(),
        role.as_deref(),
        Duration::from_secs(5),
    )
    .await?;
    println!(
        "Updated pane {} alias={} role={}",
        pane.id.0,
        alias.as_deref().unwrap_or("-"),
        role.as_deref().unwrap_or("-")
    );
    Ok(())
}

async fn wait_for_pane_closed(conn: &mut ServerConnection, pane_id: u32) -> Result<()> {
    let _ = wait_for_pane(
        conn,
        pane_id,
        &PaneWaitKind::PaneClosed,
        Duration::from_secs(5),
    )
    .await
    .context("timeout waiting for pane to close")?;
    Ok(())
}

/// `yatamux layout list` — 保存済みレイアウトの一覧を標準出力に表示する（C-22）
pub async fn layout_list() -> Result<()> {
    let names = crate::layout_config::LayoutConfig::list_layouts();
    if names.is_empty() {
        println!("(no layouts)");
    } else {
        for name in &names {
            println!("{name}");
        }
    }
    Ok(())
}

/// `yatamux layout delete <name>` — レイアウトを削除する（C-22）
pub async fn layout_delete(name: &str) -> Result<()> {
    crate::layout_config::LayoutConfig::delete_layout(name)
        .with_context(|| format!("failed to delete layout '{name}'"))?;
    println!("Deleted layout '{name}'");
    Ok(())
}

/// `yatamux layout export <name>` — レイアウトの内容を標準出力に出力する（C-22）
pub async fn layout_export(name: &str) -> Result<()> {
    let content = crate::layout_config::LayoutConfig::export_layout(name)
        .with_context(|| format!("failed to export layout '{name}'"))?;
    print!("{content}");
    Ok(())
}

/// `yatamux update` — GitHub Releases から最新バイナリを取得してセルフアップデートする（C-38）
///
/// フロー:
/// 1. GitHub API で最新バージョンを確認
/// 2. 最新バージョンが現在より新しい場合のみ続行
/// 3. `yatamux.exe` と `checksums.txt` をダウンロード
/// 4. SHA256 を検証
/// 5. `<exe>.new` に保存
/// 6. IPC 経由で `SaveAndQuit` を送信
/// 7. `--apply-update <pid> <new_path>` ヘルパーを起動して処理を委譲
pub async fn update(session: &str) -> anyhow::Result<()> {
    use crate::update::{
        download_and_stage_release_binary, fetch_latest_release, need_update, release_api_url,
    };

    const CURRENT_VERSION: &str = env!("CARGO_PKG_VERSION");
    let release_api_url = release_api_url();

    eprintln!("現在のバージョン: v{}", CURRENT_VERSION);
    eprintln!("更新 API を確認中: {}", release_api_url);

    // GitHub API で最新リリース情報を取得（async reqwest: tokio ランタイム内で使用）
    let client = reqwest::Client::builder()
        .user_agent(format!("yatamux/{}", CURRENT_VERSION))
        .build()
        .context("HTTP クライアントの初期化に失敗")?;

    let Some(release) = fetch_latest_release(&client, &release_api_url).await? else {
        eprintln!(
            "更新 API に最新リリースが見つかりません: {}",
            release_api_url
        );
        return Ok(());
    };

    // 日時を "2026-04-05T09:12:25Z" → "2026-04-05 09:12 UTC" に整形
    let published = release
        .published_at
        .as_deref()
        .map(|s| s.replace('T', " ").trim_end_matches('Z').trim().to_string() + " UTC")
        .unwrap_or_else(|| "不明".to_string());
    eprintln!("最新バージョン: {} （{}）", release.tag_name, published);

    if !need_update(CURRENT_VERSION, &release.tag_name) {
        eprintln!("すでに最新バージョンです。");
        return Ok(());
    }

    eprintln!("アップデートを開始します...");

    let exe = std::env::current_exe().context("現在の実行ファイルのパスが取得できません")?;
    eprintln!("バイナリをダウンロード中: {}", release.asset_url);
    let new_path = download_and_stage_release_binary(&client, &release, &exe).await?;
    eprintln!("チェックサム OK");
    eprintln!("バイナリを書き込みました: {}", new_path.display());

    // IPC 経由で SaveAndQuit を送信（yatamux GUI が起動中の場合）
    let coordination_result = match yatamux_client::connection::ServerConnection::connect(session)
        .await
    {
        Ok(conn) => {
            // GUI の PID を取得（バイナリ置換前に GUI の終了を待つため）
            let gui_pid = conn.server_pid;
            eprintln!(
                "yatamux インスタンス（PID {}）に SaveAndQuit を送信中...",
                gui_pid
            );
            conn.tx
                .send(yatamux_protocol::ClientMessage::SaveAndQuit)
                .await
                .context("SaveAndQuit の送信に失敗")?;

            // --apply-update ヘルパーを起動（GUI PID 待機 + 置換後に新インスタンスを起動）
            eprintln!("アップデートヘルパーを起動中...");
            spawn_apply_update_helper(&exe, gui_pid, &new_path, true)?;
            eprintln!("アップデートヘルパーを起動しました。このプロセスを終了します。");
            Ok(())
        }
        Err(err) => {
            if is_running_inside_yatamux() {
                Err(anyhow::anyhow!(
                    "セッション '{}' の IPC に接続できませんでした。yatamux ペイン内からの update では SaveAndQuit に失敗したまま自己置換へフォールバックできないため、中断します: {:#}",
                    session,
                    err
                ))
            } else {
                // GUI が起動していない → ヘルパーを起動して自プロセス終了後に rename させる
                // （Windows では実行中の exe を rename できないため self PID を渡して待機）
                eprintln!(
                    "セッション '{}' の IPC に接続できませんでした: {:#}",
                    session, err
                );
                eprintln!(
                    "実行中の yatamux インスタンスが見つかりません。ヘルパーでバイナリを置換します。"
                );
                let self_pid = std::process::id();
                spawn_apply_update_helper(&exe, self_pid, &new_path, false)?;
                eprintln!("ヘルパーを起動しました。このプロセスを終了します。");
                Ok(())
            }
        }
    };

    if let Err(err) = coordination_result {
        cleanup_staged_update(&new_path);
        return Err(err);
    }
    Ok(())
}

/// `\n` → LF、`\r` → CR、`\t` → TAB のエスケープ展開
fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('n') => {
                    chars.next();
                    out.push(b'\n');
                }
                Some('r') => {
                    chars.next();
                    out.push(b'\r');
                }
                Some('t') => {
                    chars.next();
                    out.push(b'\t');
                }
                Some('\\') => {
                    chars.next();
                    out.push(b'\\');
                }
                _ => out.push(b'\\'),
            }
        } else {
            let mut buf = [0u8; 4];
            out.extend_from_slice(c.encode_utf8(&mut buf).as_bytes());
        }
    }
    out
}

/// 設定ファイルを読み込んで `%APPDATA%\yatamux\config.toml` に適用する。
///
/// - TOML として有効かを検証する（`AppConfig` にデシリアライズ可能か確認）
/// - 検証通過後、`config.toml` にコピーする
/// - 変更はプロセス再起動後に有効になる
pub async fn source_config(path: &str) -> anyhow::Result<()> {
    use crate::config::AppConfig;
    use std::io::Write;

    let src = std::path::Path::new(path);
    if !src.exists() {
        anyhow::bail!("ファイルが見つかりません: {}", path);
    }

    let content = std::fs::read_to_string(src)
        .with_context(|| format!("ファイルを読み込めませんでした: {}", path))?;

    // TOML + AppConfig として検証
    let _: AppConfig = toml::from_str(&content)
        .with_context(|| format!("TOML のパースに失敗しました: {}", path))?;

    let dest = AppConfig::default_path();

    // 親ディレクトリを作成
    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "設定ディレクトリを作成できませんでした: {}",
                parent.display()
            )
        })?;
    }

    // 既存 config.toml があればバックアップ
    if dest.exists() {
        let backup = dest.with_extension("toml.bak");
        std::fs::copy(&dest, &backup)
            .with_context(|| format!("バックアップに失敗しました: {}", backup.display()))?;
        println!("バックアップ: {}", backup.display());
    }

    std::fs::write(&dest, &content)
        .with_context(|| format!("設定ファイルの書き込みに失敗しました: {}", dest.display()))?;

    println!("設定を適用しました: {}", dest.display());
    println!("変更はプロセス再起動後に有効になります。");
    let _ = std::io::stdout().flush();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_apply_update_command, build_exec_wait_condition, build_wait_condition,
        cleanup_staged_update, find_pane_by_selector, handle_wait_message, join_command,
        next_request_id, resolve_existing_pane, resolve_pane_for_request,
        subscription_event_from_message, unescape, wait_for_exec_result, wait_for_input_accept,
        wait_for_pane_meta_update, PaneWaitKind, SubscribePaneJsonEvent, WaitCondition,
        WaitDecision, WaitState,
    };
    use regex::Regex;
    use std::path::Path;
    use std::sync::Arc;
    use std::time::Duration;
    use tokio::sync::mpsc;
    use yatamux_client::connection::ServerConnection;
    use yatamux_protocol::{
        types::{ExecStatus, ExecWaitCondition, PaneId, PaneInfo, SurfaceId},
        ServerMessage,
    };

    #[test]
    fn unescape_newline() {
        assert_eq!(unescape("echo hello\\r"), b"echo hello\r");
    }

    #[test]
    fn unescape_lf() {
        assert_eq!(unescape("line1\\nline2"), b"line1\nline2");
    }

    #[test]
    fn unescape_passthrough() {
        assert_eq!(unescape("abc"), b"abc");
    }

    #[test]
    fn unescape_cjk() {
        assert_eq!(unescape("こんにちは"), "こんにちは".as_bytes());
    }

    #[test]
    fn build_wait_condition_for_silence() {
        let condition = build_wait_condition(crate::WaitForArg::Silence, None, 1500, 200)
            .expect("wait condition should build");
        match condition {
            WaitCondition::Silence(duration) => {
                assert_eq!(duration, Duration::from_millis(1500));
            }
            _ => panic!("unexpected wait condition"),
        }
    }

    #[test]
    fn build_wait_condition_for_output_regex() {
        let condition =
            build_wait_condition(crate::WaitForArg::OutputRegex, Some("passed"), 1500, 300)
                .expect("wait condition should build");
        match condition {
            WaitCondition::OutputRegex { regex, lines } => {
                assert!(regex.is_match("test passed"));
                assert_eq!(lines, 300);
            }
            _ => panic!("unexpected wait condition"),
        }
    }

    #[test]
    fn build_wait_condition_rejects_invalid_regex() {
        let err = build_wait_condition(crate::WaitForArg::OutputRegex, Some("("), 1500, 200)
            .expect_err("invalid regex should fail");
        assert!(err.to_string().contains("invalid --output-regex pattern"));
    }

    #[test]
    fn build_exec_wait_condition_for_output_regex() {
        let wait =
            build_exec_wait_condition(crate::WaitForArg::OutputRegex, Some("passed"), 1500, 200)
                .expect("exec wait should build");
        assert_eq!(
            wait,
            ExecWaitCondition::OutputRegex {
                pattern: "passed".to_string(),
                lines: 200,
            }
        );
    }

    #[test]
    fn build_exec_wait_condition_rejects_invalid_regex() {
        let err = build_exec_wait_condition(crate::WaitForArg::OutputRegex, Some("("), 1500, 200)
            .expect_err("invalid regex should fail");
        assert!(err.to_string().contains("invalid --output-regex pattern"));
    }

    #[test]
    fn next_request_id_includes_prefix() {
        let request_id = next_request_id("exec", PaneId(12));
        assert!(request_id.starts_with("exec-"));
        assert!(request_id.contains("-12-"));
    }

    #[test]
    fn join_command_with_spaces() {
        assert_eq!(
            join_command(&["cargo".to_string(), "test".to_string(), "-q".to_string()]),
            "cargo test -q"
        );
    }

    #[test]
    fn pane_closed_wait_ignores_other_panes_then_matches_target() {
        let mut state = WaitState::new(tokio::time::Instant::now());
        let kind = PaneWaitKind::PaneClosed;

        let other = handle_wait_message(
            &kind,
            PaneId(5),
            5,
            ServerMessage::PaneClosed { pane: PaneId(4) },
            &mut state,
        );
        assert!(matches!(other, WaitDecision::Continue));

        let matched = handle_wait_message(
            &kind,
            PaneId(5),
            5,
            ServerMessage::PaneClosed { pane: PaneId(5) },
            &mut state,
        );
        assert!(matches!(matched, WaitDecision::Done(_)));
    }

    #[test]
    fn exit_wait_finishes_on_command_finished_and_preserves_exit_code() {
        let mut state = WaitState::new(tokio::time::Instant::now());
        let kind = PaneWaitKind::Condition(WaitCondition::Exit);

        let matched = handle_wait_message(
            &kind,
            PaneId(8),
            8,
            ServerMessage::CommandFinished {
                pane: PaneId(8),
                exit_code: Some(2),
            },
            &mut state,
        );
        match matched {
            WaitDecision::Done(result) => assert_eq!(result.exit_code, Some(2)),
            _ => panic!("expected WaitDecision::Done"),
        }
    }

    #[test]
    fn regex_wait_errors_if_target_pane_closes_first() {
        let mut state = WaitState::new(tokio::time::Instant::now());
        let kind = PaneWaitKind::Condition(WaitCondition::OutputRegex {
            regex: Regex::new("passed").expect("regex should compile"),
            lines: 200,
        });

        let matched = handle_wait_message(
            &kind,
            PaneId(9),
            9,
            ServerMessage::PaneClosed { pane: PaneId(9) },
            &mut state,
        );
        match matched {
            WaitDecision::Error(err) => {
                assert!(err.to_string().contains("closed before regex matched"));
            }
            _ => panic!("expected WaitDecision::Error"),
        }
    }

    #[test]
    fn build_apply_update_command_includes_launch_when_requested() {
        let exe = Path::new(r"C:\tmp\yatamux.exe");
        let new_path = Path::new(r"C:\tmp\yatamux.exe.new");
        unsafe {
            std::env::remove_var("YATAMUX_SESSION");
        }
        let command = build_apply_update_command(exe, 1234, new_path, true);

        let program = command.get_program().to_string_lossy().into_owned();
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert!(program.ends_with("yatamux.exe"));
        assert_eq!(
            args,
            vec![
                "--apply-update".to_string(),
                "1234".to_string(),
                r"C:\tmp\yatamux.exe.new".to_string(),
                "--launch".to_string(),
            ]
        );
    }

    #[test]
    fn build_apply_update_command_preserves_session_argument() {
        let exe = Path::new(r"C:\tmp\yatamux.exe");
        let new_path = Path::new(r"C:\tmp\yatamux.exe.new");
        unsafe {
            std::env::set_var("YATAMUX_SESSION", "helper-session");
        }
        let command = build_apply_update_command(exe, 42, new_path, false);
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args,
            vec![
                "--session".to_string(),
                "helper-session".to_string(),
                "--apply-update".to_string(),
                "42".to_string(),
                r"C:\tmp\yatamux.exe.new".to_string(),
            ]
        );

        unsafe {
            std::env::remove_var("YATAMUX_SESSION");
        }
    }

    #[test]
    fn cleanup_staged_update_removes_staged_binary() {
        let dir = std::env::temp_dir().join(format!(
            "yatamux-cli-cleanup-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system time before unix epoch")
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        let new_path = dir.join("yatamux.exe.new");
        std::fs::write(&new_path, b"staged binary").expect("write staged binary");

        cleanup_staged_update(&new_path);

        assert!(!new_path.exists(), "staged binary should be removed");
        let _ = std::fs::remove_dir_all(dir);
    }

    #[tokio::test]
    async fn wait_for_exec_result_ignores_other_request_ids() {
        let (client_tx, _client_rx) = mpsc::channel(4);
        let (server_tx, server_rx) = mpsc::channel(4);
        let mut conn = ServerConnection {
            tx: client_tx,
            rx: server_rx,
            server_pid: 0,
        };

        server_tx
            .send(ServerMessage::ExecResult {
                request_id: "other".to_string(),
                pane: PaneId(3),
                status: ExecStatus::Completed,
                exit_code: Some(9),
                message: None,
            })
            .await
            .expect("send other result");
        server_tx
            .send(ServerMessage::ExecResult {
                request_id: "target".to_string(),
                pane: PaneId(3),
                status: ExecStatus::Completed,
                exit_code: Some(0),
                message: None,
            })
            .await
            .expect("send target result");

        let result = wait_for_exec_result(&mut conn, "target", Duration::from_secs(1))
            .await
            .expect("wait should succeed");
        assert_eq!(result.exit_code, Some(0));
    }

    #[tokio::test]
    async fn wait_for_exec_result_returns_error_message() {
        let (client_tx, _client_rx) = mpsc::channel(4);
        let (server_tx, server_rx) = mpsc::channel(4);
        let mut conn = ServerConnection {
            tx: client_tx,
            rx: server_rx,
            server_pid: 0,
        };

        server_tx
            .send(ServerMessage::ExecResult {
                request_id: "target".to_string(),
                pane: PaneId(4),
                status: ExecStatus::TimedOut,
                exit_code: None,
                message: Some("timeout waiting for pane 4".to_string()),
            })
            .await
            .expect("send timed out result");

        let err = wait_for_exec_result(&mut conn, "target", Duration::from_secs(1))
            .await
            .expect_err("timed out exec should surface as error");
        assert!(err.to_string().contains("timeout waiting for pane 4"));
    }

    #[tokio::test]
    async fn wait_for_pane_meta_update_ignores_unrelated_events() {
        let (client_tx, _client_rx) = mpsc::channel(4);
        let (server_tx, server_rx) = mpsc::channel(4);
        let mut conn = ServerConnection {
            tx: client_tx,
            rx: server_rx,
            server_pid: 0,
        };

        server_tx
            .send(ServerMessage::PaneMetaUpdated {
                pane: PaneId(3),
                alias: Some("other".to_string()),
                role: Some("observer".to_string()),
            })
            .await
            .expect("send other pane metadata");
        server_tx
            .send(ServerMessage::PaneMetaUpdated {
                pane: PaneId(4),
                alias: Some("tests".to_string()),
                role: Some("verifier".to_string()),
            })
            .await
            .expect("send target pane metadata");

        wait_for_pane_meta_update(
            &mut conn,
            PaneId(4),
            Some("tests"),
            Some("verifier"),
            Duration::from_secs(1),
        )
        .await
        .expect("wait should succeed");
    }

    #[tokio::test]
    async fn wait_for_pane_meta_update_surfaces_server_error() {
        let (client_tx, _client_rx) = mpsc::channel(4);
        let (server_tx, server_rx) = mpsc::channel(4);
        let mut conn = ServerConnection {
            tx: client_tx,
            rx: server_rx,
            server_pid: 0,
        };

        server_tx
            .send(ServerMessage::Error {
                message: "pane 8 not found".to_string(),
                request_id: None,
            })
            .await
            .expect("send error");

        let err = wait_for_pane_meta_update(
            &mut conn,
            PaneId(8),
            Some("tests"),
            Some("verifier"),
            Duration::from_secs(1),
        )
        .await
        .expect_err("server error should surface");
        assert!(err.to_string().contains("pane 8 not found"));
    }

    #[tokio::test]
    async fn wait_for_input_accept_ignores_other_panes() {
        let (client_tx, _client_rx) = mpsc::channel(4);
        let (server_tx, server_rx) = mpsc::channel(4);
        let mut conn = ServerConnection {
            tx: client_tx,
            rx: server_rx,
            server_pid: 0,
        };

        server_tx
            .send(ServerMessage::InputAccepted { pane: PaneId(1) })
            .await
            .expect("send other pane ack");
        server_tx
            .send(ServerMessage::InputAccepted { pane: PaneId(2) })
            .await
            .expect("send target pane ack");

        wait_for_input_accept(&mut conn, PaneId(2), Duration::from_secs(1))
            .await
            .expect("wait should succeed");
    }

    #[tokio::test]
    async fn wait_for_input_accept_surfaces_server_error() {
        let (client_tx, _client_rx) = mpsc::channel(4);
        let (server_tx, server_rx) = mpsc::channel(4);
        let mut conn = ServerConnection {
            tx: client_tx,
            rx: server_rx,
            server_pid: 0,
        };

        server_tx
            .send(ServerMessage::Error {
                message: "pane 9 not found".to_string(),
                request_id: None,
            })
            .await
            .expect("send error");

        let err = wait_for_input_accept(&mut conn, PaneId(9), Duration::from_secs(1))
            .await
            .expect_err("server error should surface");
        assert!(err.to_string().contains("pane 9 not found"));
    }

    #[test]
    fn find_pane_by_selector_matches_alias() {
        let panes = vec![PaneInfo {
            id: PaneId(4),
            surface: SurfaceId(1),
            title: "shell".to_string(),
            cols: 80,
            rows: 24,
            alias: Some("tests".to_string()),
            role: Some("verifier".to_string()),
            cwd: None,
            command: None,
            busy: false,
            last_output_unix_ms: None,
            active: false,
            floating: false,
        }];
        let pane = find_pane_by_selector(&panes, "tests").expect("alias should resolve");
        assert_eq!(pane.id, PaneId(4));
    }

    #[test]
    fn resolve_pane_for_request_accepts_numeric_id_without_lookup() {
        let panes = Vec::<PaneInfo>::new();
        let pane_id = resolve_pane_for_request(&panes, "999").expect("numeric id should pass");
        assert_eq!(pane_id, PaneId(999));
    }

    #[test]
    fn resolve_existing_pane_errors_for_unknown_alias() {
        let panes = Vec::<PaneInfo>::new();
        let err = resolve_existing_pane(&panes, "missing").expect_err("missing alias should fail");
        assert!(err.to_string().contains("missing"));
    }

    #[test]
    fn subscription_event_maps_output_to_json_line_event() {
        let event = subscription_event_from_message(
            PaneId(7),
            ServerMessage::Output {
                pane: PaneId(7),
                data: Arc::from(&b"hello\r\n"[..]),
            },
        )
        .expect("stream event conversion should succeed")
        .expect("matching pane output should yield an event");

        match event {
            SubscribePaneJsonEvent::Output { pane, text } => {
                assert_eq!(pane, PaneId(7));
                assert_eq!(text, "hello\r\n");
            }
            _ => panic!("unexpected event"),
        }
    }

    #[test]
    fn subscription_event_ignores_other_pane_messages() {
        let event = subscription_event_from_message(
            PaneId(7),
            ServerMessage::PaneClosed { pane: PaneId(8) },
        )
        .expect("non-matching pane should not error");
        assert!(event.is_none());
    }

    #[test]
    fn subscription_event_treats_lag_error_as_stream_event() {
        let event = subscription_event_from_message(
            PaneId(7),
            ServerMessage::Error {
                message: "subscription lagged by 3 messages; stream output may be incomplete"
                    .to_string(),
                request_id: None,
            },
        )
        .expect("lagged error should be converted")
        .expect("lagged error should produce an event");

        match event {
            SubscribePaneJsonEvent::Lagged { pane, message } => {
                assert_eq!(pane, PaneId(7));
                assert!(message.contains("lagged by 3 messages"));
            }
            _ => panic!("unexpected event"),
        }
    }
}
