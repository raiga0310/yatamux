//! GitHub Actions CI ステータスポーラー
//!
//! `config.toml` の `[ci]` セクションに `repo = "owner/repo"` が設定されているとき、
//! バックグラウンドで GitHub Actions API をポーリングして最新ランの状態を取得する。
//!
//! ## 設計上の分離
//!
//! - **取得側**: [`run_ci_poller`] がバックグラウンド tokio タスクとして動作し、
//!   `Arc<std::sync::Mutex<Option<CiRunInfo>>>` に結果を書き込む。
//! - **表示側**: Win32 スレッドからその Arc を読み込み、ステータスバーに描画する。
//! - **IPC 配信**: ポーラーは更新のたびに `mpsc::Sender<ServerMessage>` 経由で
//!   `ServerMessage::CiStatus` をブロードキャストする。

use anyhow::Result;
use serde::Deserialize;
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;
use tokio::time::Duration;
use tracing::{debug, error, info, warn};
use yatamux_protocol::{CiConclusion, CiRunInfo, CiRunStatus, ServerMessage};

use crate::config::CiConfig;

/// GitHub API の workflow_run オブジェクト（必要フィールドのみ）
#[derive(Debug, Deserialize)]
struct ApiWorkflowRun {
    name: String,
    status: String,
    #[serde(default)]
    conclusion: Option<String>,
    #[serde(default)]
    head_branch: Option<String>,
    head_sha: String,
    html_url: String,
    updated_at: String,
}

/// GitHub API のレスポンス（`GET /repos/{owner}/{repo}/actions/runs`）
#[derive(Debug, Deserialize)]
struct ApiRunsResponse {
    workflow_runs: Vec<ApiWorkflowRun>,
}

fn parse_status(status: &str) -> CiRunStatus {
    match status {
        "queued" | "waiting" | "pending" => CiRunStatus::Queued,
        "in_progress" => CiRunStatus::InProgress,
        _ => CiRunStatus::Completed,
    }
}

fn parse_conclusion(conclusion: Option<&str>) -> Option<CiConclusion> {
    match conclusion? {
        "success" => Some(CiConclusion::Success),
        "failure" => Some(CiConclusion::Failure),
        "cancelled" => Some(CiConclusion::Cancelled),
        "skipped" => Some(CiConclusion::Skipped),
        _ => Some(CiConclusion::Unknown),
    }
}

/// GitHub Actions API から最新 run を取得する
async fn fetch_latest_run(
    client: &reqwest::Client,
    repo: &str,
    branch: Option<&str>,
) -> Result<Option<CiRunInfo>> {
    let mut url = format!(
        "https://api.github.com/repos/{}/actions/runs?per_page=1&exclude_pull_requests=true",
        repo
    );
    if let Some(b) = branch {
        url.push_str(&format!("&branch={}", b));
    }

    debug!("CI: polling {}", url);
    let resp = client
        .get(&url)
        .send()
        .await?
        .error_for_status()?
        .json::<ApiRunsResponse>()
        .await?;

    let Some(run) = resp.workflow_runs.into_iter().next() else {
        return Ok(None);
    };

    Ok(Some(CiRunInfo {
        repo: repo.to_string(),
        name: run.name,
        status: parse_status(&run.status),
        conclusion: parse_conclusion(run.conclusion.as_deref()),
        branch: run.head_branch,
        head_sha: Some(run.head_sha.chars().take(7).collect()),
        html_url: Some(run.html_url),
        updated_at: Some(run.updated_at),
    }))
}

/// CI ポーラーのバックグラウンドタスクを起動する
///
/// - `ci_state`: 取得結果を保持する共有 Arc（Win32 スレッドから参照）
/// - `broadcast_tx`: 更新時に `CiStatus` をブロードキャストするチャネル
///
/// `config.repo` が `None` の場合は何もしない。
pub async fn run_ci_poller(
    config: CiConfig,
    ci_state: Arc<Mutex<Option<CiRunInfo>>>,
    broadcast_tx: mpsc::Sender<ServerMessage>,
) {
    let Some(repo) = config.repo else {
        debug!("CI: no repo configured; poller disabled");
        return;
    };

    let interval = Duration::from_secs(config.poll_interval_secs.max(10));
    info!(
        "CI: starting poller for {} (interval: {}s)",
        repo, config.poll_interval_secs
    );

    let mut builder =
        reqwest::Client::builder().user_agent(format!("yatamux/{}", env!("CARGO_PKG_VERSION")));
    if let Some(token) = &config.token {
        let mut headers = reqwest::header::HeaderMap::new();
        if let Ok(val) = reqwest::header::HeaderValue::from_str(&format!("Bearer {}", token)) {
            headers.insert(reqwest::header::AUTHORIZATION, val);
        }
        builder = builder.default_headers(headers);
    }

    let client = match builder.build() {
        Ok(c) => c,
        Err(e) => {
            error!("CI: failed to build HTTP client: {}", e);
            return;
        }
    };

    loop {
        match fetch_latest_run(&client, &repo, config.branch.as_deref()).await {
            Ok(new_info) => {
                let changed = {
                    let mut state = ci_state.lock().unwrap();
                    if *state != new_info {
                        *state = new_info.clone();
                        true
                    } else {
                        false
                    }
                };
                if changed {
                    let msg = ServerMessage::CiStatus { info: new_info };
                    if broadcast_tx.send(msg).await.is_err() {
                        info!("CI: broadcast channel closed; stopping poller");
                        return;
                    }
                }
            }
            Err(e) => {
                warn!("CI: failed to fetch status for {}: {:#}", repo, e);
            }
        }

        tokio::time::sleep(interval).await;
    }
}
