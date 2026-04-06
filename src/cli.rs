//! CLI サブコマンド実装
//!
//! 実行中の yatamux セッションに IPC 経由で接続し、
//! `list-panes` / `send-keys` を実行する。

use anyhow::{Context, Result};
use regex::Regex;
use serde::Serialize;
use std::time::Duration;
use yatamux_client::connection::ServerConnection;
use yatamux_protocol::types::{PaneCapture, PaneId, PaneInfo, SplitDirection, SurfaceId};
use yatamux_protocol::{ClientMessage, ServerMessage};

#[derive(Serialize)]
struct CapturePaneJsonOutput {
    pane: PaneId,
    content: String,
    #[serde(flatten)]
    capture: PaneCapture,
}

#[derive(Debug)]
enum WaitCondition {
    Exit,
    Silence(Duration),
    OutputRegex { regex: Regex, lines: usize },
}

struct WaitResult {
    exit_code: Option<i32>,
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

fn join_command(command: &[String]) -> String {
    command.join(" ")
}

fn maybe_exit_with_code(result: WaitResult) {
    if let Some(code) = result.exit_code {
        if code != 0 {
            eprintln!("Command exited with status {}", code);
            std::process::exit(code);
        }
    }
}

async fn wait_for_condition(
    conn: &mut ServerConnection,
    pane_id: u32,
    condition: &WaitCondition,
    timeout: Duration,
) -> Result<WaitResult> {
    let pane = PaneId(pane_id);
    let started = tokio::time::Instant::now();
    let mut last_activity = tokio::time::Instant::now();
    let mut next_capture_at = tokio::time::Instant::now();
    let mut last_exit_code = None;

    loop {
        if started.elapsed() >= timeout {
            anyhow::bail!("timeout waiting for pane {}", pane_id);
        }

        if let WaitCondition::Silence(duration) = condition {
            if last_activity.elapsed() >= *duration {
                return Ok(WaitResult {
                    exit_code: last_exit_code,
                });
            }
        }

        if let WaitCondition::OutputRegex { lines, .. } = condition {
            if tokio::time::Instant::now() >= next_capture_at {
                conn.tx
                    .send(ClientMessage::CapturePane {
                        pane,
                        lines: *lines,
                        plain_text: true,
                    })
                    .await?;
                next_capture_at = tokio::time::Instant::now() + Duration::from_millis(200);
            }
        }

        let mut next_wait = Duration::from_millis(100);
        let timeout_left = timeout.saturating_sub(started.elapsed());
        if timeout_left < next_wait {
            next_wait = timeout_left;
        }
        if let WaitCondition::Silence(duration) = condition {
            let silence_left = duration.saturating_sub(last_activity.elapsed());
            if silence_left < next_wait {
                next_wait = silence_left;
            }
        }
        if let WaitCondition::OutputRegex { .. } = condition {
            let capture_left =
                next_capture_at.saturating_duration_since(tokio::time::Instant::now());
            if capture_left < next_wait {
                next_wait = capture_left;
            }
        }
        if next_wait.is_zero() {
            next_wait = Duration::from_millis(1);
        }

        match tokio::time::timeout(next_wait, conn.rx.recv()).await {
            Ok(Some(ServerMessage::Output {
                pane: output_pane, ..
            })) if output_pane == pane => {
                last_activity = tokio::time::Instant::now();
            }
            Ok(Some(ServerMessage::CommandFinished {
                pane: finished_pane,
                exit_code,
            })) if finished_pane == pane => {
                last_activity = tokio::time::Instant::now();
                last_exit_code = exit_code;
                if matches!(condition, WaitCondition::Exit) {
                    return Ok(WaitResult { exit_code });
                }
            }
            Ok(Some(ServerMessage::PaneClosed { pane: closed_pane })) if closed_pane == pane => {
                return match condition {
                    WaitCondition::OutputRegex { .. } => Err(anyhow::anyhow!(
                        "pane {} closed before regex matched",
                        pane_id
                    )),
                    _ => Ok(WaitResult {
                        exit_code: last_exit_code,
                    }),
                };
            }
            Ok(Some(ServerMessage::PaneContent {
                pane: content_pane,
                content,
                ..
            })) if content_pane == pane => {
                if let WaitCondition::OutputRegex { regex, .. } = condition {
                    if regex.is_match(&content) {
                        return Ok(WaitResult {
                            exit_code: last_exit_code,
                        });
                    }
                }
            }
            Ok(Some(ServerMessage::Error { message })) => {
                return Err(anyhow::anyhow!("{}", message));
            }
            Ok(Some(_)) => {}
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

async fn request_panes(conn: &mut ServerConnection) -> Result<Vec<PaneInfo>> {
    conn.tx.send(ClientMessage::ListPanes).await?;

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::PanesListed { panes }) => return Ok(panes),
                Some(ServerMessage::Error { message }) => {
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

fn pane_exists(panes: &[PaneInfo], pane_id: u32) -> bool {
    panes.iter().any(|p| p.id == PaneId(pane_id))
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
            "{:<6} {:<8} {:<6} {:<6} title",
            "pane", "surface", "cols", "rows"
        );
        println!("{}", "-".repeat(40));
        for p in &panes {
            println!(
                "{:<6} {:<8} {:<6} {:<6} {}",
                p.id.0, p.surface.0, p.cols, p.rows, p.title
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
    pane_id: u32,
    lines: usize,
    plain_text: bool,
    json: bool,
) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;

    conn.tx
        .send(ClientMessage::CapturePane {
            pane: PaneId(pane_id),
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
                Some(ServerMessage::Error { message }) => {
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

/// `yatamux split-pane --target <id> --direction <v|h> --dir <path>` — ペインを分割する
///
/// 指定ペインを分割して新しいペインを作成する。
/// `--dir` で作業ディレクトリを指定できる。
pub async fn split_pane(
    session: &str,
    pane_id: u32,
    direction: SplitDirection,
    working_dir: Option<String>,
) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;

    // まずペイン一覧を取得してサーフェス ID を取得する
    let panes = request_panes(&mut conn).await?;

    // 対象ペインを探す（C-25: 見つからない場合はエラー終了）
    let target_pane = if pane_id == 0 {
        panes.first()
    } else {
        panes.iter().find(|p| p.id == PaneId(pane_id))
    };
    if pane_id != 0 && target_pane.is_none() {
        eprintln!("Error: pane {} not found", pane_id);
        std::process::exit(1);
    }

    let surface = target_pane.map(|p| p.surface).unwrap_or(SurfaceId(1));

    // split_from には実際に存在するペイン ID を使う
    // デフォルトの --target 0 は存在しないため、フォールバック後の ID を使わないと
    // split_pane_tree がツリー内で対象 Leaf を見つけられず、新ペインがツリーに入らない
    let split_from_id = target_pane.map(|p| p.id).unwrap_or(PaneId(pane_id));

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
                Some(ServerMessage::Error { message }) => {
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
    pane_id: u32,
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

    if !pane_exists(&panes, pane_id) {
        eprintln!("Error: pane {} not found", pane_id);
        std::process::exit(1);
    }

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
            pane: PaneId(pane_id),
            data,
        })
        .await?;

    // --wait-for-prompt: 対象ペインの CommandFinished を受信するまで待機（C-27）
    if wait_for_prompt {
        tokio::time::timeout(std::time::Duration::from_secs(60), async {
            loop {
                match conn.rx.recv().await {
                    Some(ServerMessage::CommandFinished { pane, exit_code })
                        if pane == PaneId(pane_id) =>
                    {
                        if let Some(code) = exit_code {
                            if code != 0 {
                                eprintln!("Command exited with status {}", code);
                                std::process::exit(code);
                            }
                        }
                        return;
                    }
                    Some(ServerMessage::Error { message }) => {
                        eprintln!("Error: {}", message);
                        std::process::exit(1);
                    }
                    Some(_) => continue,
                    None => return,
                }
            }
        })
        .await
        .context("timeout waiting for command to finish (60s)")?;
    }

    Ok(())
}

/// `yatamux wait-pane --pane <id> ...` — 指定ペインが条件を満たすまで待機する
pub async fn wait_pane(session: &str, pane_id: u32, options: WaitOptions) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    if !pane_exists(&panes, pane_id) {
        eprintln!("Error: pane {} not found", pane_id);
        std::process::exit(1);
    }

    let condition = build_wait_condition(
        options.wait_for,
        options.output_regex.as_deref(),
        options.silence_ms,
        options.lines,
    )?;
    let result = wait_for_condition(
        &mut conn,
        pane_id,
        &condition,
        Duration::from_secs(options.timeout_secs),
    )
    .await?;
    maybe_exit_with_code(result);
    Ok(())
}

/// `yatamux exec --pane <id> -- <command>` — コマンド送信と待機をまとめて行う
pub async fn exec_command(
    session: &str,
    pane_id: u32,
    command: Vec<String>,
    raw: bool,
    options: WaitOptions,
) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    if !pane_exists(&panes, pane_id) {
        eprintln!("Error: pane {} not found", pane_id);
        std::process::exit(1);
    }

    let condition = build_wait_condition(
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

    conn.tx
        .send(ClientMessage::Input {
            pane: PaneId(pane_id),
            data,
        })
        .await?;

    let result = wait_for_condition(
        &mut conn,
        pane_id,
        &condition,
        Duration::from_secs(options.timeout_secs),
    )
    .await?;
    maybe_exit_with_code(result);
    Ok(())
}

/// `yatamux interrupt-pane --pane <id>` — 指定ペインに Ctrl+C を送信する
pub async fn interrupt_pane(session: &str, pane_id: u32) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    if !pane_exists(&panes, pane_id) {
        eprintln!("Error: pane {} not found", pane_id);
        std::process::exit(1);
    }

    conn.tx
        .send(ClientMessage::InterruptPane {
            pane: PaneId(pane_id),
        })
        .await?;
    println!("Interrupted pane {}", pane_id);
    Ok(())
}

/// `yatamux close-pane --pane <id>` — 指定ペインを閉じる
pub async fn close_pane(session: &str, pane_id: u32) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;
    let panes = request_panes(&mut conn).await?;
    if !pane_exists(&panes, pane_id) {
        eprintln!("Error: pane {} not found", pane_id);
        std::process::exit(1);
    }

    conn.tx
        .send(ClientMessage::ClosePane {
            pane: PaneId(pane_id),
        })
        .await?;

    tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::PaneClosed { pane }) if pane == PaneId(pane_id) => {
                    return Ok(())
                }
                Some(ServerMessage::Error { message }) => {
                    return Err(anyhow::anyhow!("{}", message))
                }
                Some(_) => continue,
                None => {
                    return Err(anyhow::anyhow!(
                        "server closed connection before sending PaneClosed"
                    ))
                }
            }
        }
    })
    .await
    .context("timeout waiting for pane to close")??;

    println!("Closed pane {}", pane_id);
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
        download_and_verify_release_binary, fetch_latest_release, need_update, plan_update_paths,
        release_api_url,
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

    eprintln!("バイナリをダウンロード中: {}", release.asset_url);
    let binary = download_and_verify_release_binary(&client, &release).await?;
    eprintln!("チェックサム OK");

    // <exe>.new に保存
    let exe = std::env::current_exe().context("現在の実行ファイルのパスが取得できません")?;
    let (new_path, _) = plan_update_paths(&exe);
    std::fs::write(&new_path, &binary)
        .with_context(|| format!("バイナリの書き込みに失敗: {}", new_path.display()))?;
    eprintln!("バイナリを書き込みました: {}", new_path.display());

    // IPC 経由で SaveAndQuit を送信（yatamux GUI が起動中の場合）
    match yatamux_client::connection::ServerConnection::connect(session).await {
        Ok(conn) => {
            // GUI の PID を取得（バイナリ置換前に GUI の終了を待つため）
            let gui_pid = conn.server_pid;
            eprintln!(
                "yatamux インスタンス（PID {}）に SaveAndQuit を送信中...",
                gui_pid
            );
            let _ = conn
                .tx
                .send(yatamux_protocol::ClientMessage::SaveAndQuit)
                .await;

            // --apply-update ヘルパーを起動（GUI PID 待機 + 置換後に新インスタンスを起動）
            eprintln!("アップデートヘルパーを起動中...");
            std::process::Command::new(&exe)
                .args([
                    "--apply-update",
                    &gui_pid.to_string(),
                    &new_path.to_string_lossy(),
                    "--launch",
                ])
                .spawn()
                .context("アップデートヘルパーの起動に失敗")?;
            eprintln!("アップデートヘルパーを起動しました。このプロセスを終了します。");
        }
        Err(err) => {
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
            std::process::Command::new(&exe)
                .args([
                    "--apply-update",
                    &self_pid.to_string(),
                    &new_path.to_string_lossy(),
                    // --launch なし: 新ウィンドウは開かない
                ])
                .spawn()
                .context("アップデートヘルパーの起動に失敗")?;
            eprintln!("ヘルパーを起動しました。このプロセスを終了します。");
        }
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

#[cfg(test)]
mod tests {
    use super::{build_wait_condition, join_command, unescape, WaitCondition};
    use std::time::Duration;

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
    fn join_command_with_spaces() {
        assert_eq!(
            join_command(&["cargo".to_string(), "test".to_string(), "-q".to_string()]),
            "cargo test -q"
        );
    }
}
