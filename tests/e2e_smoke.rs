#![cfg(windows)]

use std::collections::HashSet;
use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};
use yatamux_client::connection::ServerConnection;
use yatamux_client::{LayoutNodeDef, LayoutSnapshot};
use yatamux_protocol::types::{ExecStatus, ExecWaitCondition, PaneId, PaneInfo, SplitDirection};
use yatamux_protocol::{ClientMessage, ServerMessage};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;
const DEFAULT_E2E_TIMEOUT_SECS: u64 = 90;

fn e2e_timeout() -> Duration {
    Duration::from_secs(
        std::env::var("YATAMUX_E2E_TIMEOUT_SECS")
            .ok()
            .and_then(|value| value.parse::<u64>().ok())
            .unwrap_or(DEFAULT_E2E_TIMEOUT_SECS),
    )
}

fn bounded_timeout(timeout: Duration) -> Duration {
    timeout.min(e2e_timeout())
}

fn e2e_lock() -> &'static Mutex<()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(()))
}

fn lock_e2e_tests() -> MutexGuard<'static, ()> {
    e2e_lock().lock().unwrap_or_else(|err| err.into_inner())
}

fn unique_name(prefix: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{}-{}", prefix, std::process::id(), n)
}

fn yatamux_exe() -> PathBuf {
    PathBuf::from(env!("CARGO_BIN_EXE_yatamux"))
}

fn build_command(exe: &Path, appdata: &Path, session: &str) -> Command {
    use std::os::windows::process::CommandExt;

    let mut command = Command::new(exe);
    command
        .arg("--session")
        .arg(session)
        .env("APPDATA", appdata)
        .creation_flags(CREATE_NO_WINDOW);
    command
}

fn wait_until<T, F>(timeout: Duration, interval: Duration, description: &str, mut f: F) -> T
where
    F: FnMut() -> Option<T>,
{
    let deadline = Instant::now() + bounded_timeout(timeout);
    loop {
        if let Some(value) = f() {
            return value;
        }
        if Instant::now() >= deadline {
            panic!("timeout waiting for {}", description);
        }
        sleep(interval);
    }
}

fn output_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn assert_success(output: &Output, context: &str) {
    assert!(
        output.status.success(),
        "{} failed\nstdout:\n{}\nstderr:\n{}",
        context,
        output_text(&output.stdout).trim(),
        output_text(&output.stderr).trim()
    );
}

fn wait_with_output(mut child: Child, timeout: Duration) -> Output {
    let deadline = Instant::now() + bounded_timeout(timeout);
    loop {
        if child.try_wait().expect("poll child").is_some() {
            return child.wait_with_output().expect("collect child output");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!(
                "timeout waiting for child process output after {:?}",
                bounded_timeout(timeout)
            );
        }
        sleep(Duration::from_millis(100));
    }
}

fn parse_json_lines(stdout: &[u8]) -> Vec<Value> {
    output_text(stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("parse JSON line"))
        .collect()
}

#[derive(Debug)]
struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(unique_name(prefix));
        fs::create_dir_all(&path).expect("create temp dir");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct AppProcess {
    child: Child,
}

impl AppProcess {
    fn spawn(exe: &Path, appdata: &Path, session: &str) -> Self {
        let child = build_command(exe, appdata, session)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn yatamux GUI");
        Self { child }
    }

    fn wait_for_exit(&mut self, timeout: Duration) {
        let deadline = Instant::now() + bounded_timeout(timeout);
        loop {
            if self.child.try_wait().expect("poll GUI").is_some() {
                return;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!(
                    "timeout waiting for yatamux GUI to exit after {:?}",
                    bounded_timeout(timeout)
                );
            }
            sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for AppProcess {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
            let _ = self.child.wait();
        }
    }
}

struct AppHarness {
    exe: PathBuf,
    appdata: TempDir,
    session: String,
    app: Option<AppProcess>,
}

impl AppHarness {
    fn new(prefix: &str) -> Self {
        Self::with_exe(prefix, yatamux_exe())
    }

    fn with_exe(prefix: &str, exe: PathBuf) -> Self {
        Self {
            exe,
            appdata: TempDir::new(prefix),
            session: unique_name(prefix),
            app: None,
        }
    }

    fn spawn(&mut self) {
        self.app = Some(AppProcess::spawn(
            &self.exe,
            self.appdata.path(),
            &self.session,
        ));
    }

    fn app_mut(&mut self) -> &mut AppProcess {
        self.app.as_mut().expect("app should be running")
    }

    fn appdata_path(&self) -> &Path {
        self.appdata.path()
    }

    fn session_path(&self) -> PathBuf {
        self.appdata_path().join("yatamux").join("session.toml")
    }

    fn run_cli(&self, args: &[&str]) -> Output {
        let child = build_command(&self.exe, self.appdata.path(), &self.session)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn yatamux CLI");
        wait_with_output(child, e2e_timeout())
    }

    fn run_cli_owned(&self, args: &[String]) -> Output {
        let child = build_command(&self.exe, self.appdata.path(), &self.session)
            .args(args)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn yatamux CLI");
        wait_with_output(child, e2e_timeout())
    }

    fn cli_command(&self, args: &[&str]) -> Command {
        let mut command = build_command(&self.exe, self.appdata.path(), &self.session);
        command.args(args);
        command
    }

    fn try_list_panes(&self) -> Option<Vec<PaneInfo>> {
        let output = self.run_cli(&["list-panes", "--json"]);
        if !output.status.success() {
            return None;
        }
        serde_json::from_slice(&output.stdout).ok()
    }

    fn list_panes(&self) -> Vec<PaneInfo> {
        let output = self.run_cli(&["list-panes", "--json"]);
        assert_success(&output, "list-panes --json");
        serde_json::from_slice(&output.stdout).expect("parse list-panes JSON")
    }

    fn wait_for_pane_count(&self, expected: usize) -> Vec<PaneInfo> {
        wait_until(
            Duration::from_secs(20),
            Duration::from_millis(150),
            &format!("{} panes", expected),
            || {
                self.try_list_panes()
                    .filter(|panes| panes.len() == expected)
            },
        )
    }

    fn wait_for_alias(&self, alias: &str) -> PaneInfo {
        wait_until(
            Duration::from_secs(20),
            Duration::from_millis(150),
            &format!("alias {}", alias),
            || {
                self.try_list_panes().and_then(|panes| {
                    panes
                        .into_iter()
                        .find(|pane| pane.alias.as_deref() == Some(alias))
                })
            },
        )
    }

    fn wait_for_pane_gone(&self, pane_id: PaneId) {
        wait_until(
            Duration::from_secs(20),
            Duration::from_millis(150),
            &format!("pane {} to disappear", pane_id.0),
            || {
                self.try_list_panes().and_then(|panes| {
                    if panes.iter().all(|pane| pane.id != pane_id) {
                        Some(())
                    } else {
                        None
                    }
                })
            },
        );
    }

    fn wait_for_ipc_unavailable(&self, timeout: Duration) {
        wait_until(
            timeout,
            Duration::from_millis(150),
            "IPC to become unavailable",
            || {
                if self.try_list_panes().is_none() {
                    Some(())
                } else {
                    None
                }
            },
        );
    }

    fn capture_json(&self, selector: &str) -> Value {
        let output = self.run_cli(&["capture-pane", "--target", selector, "--json"]);
        assert_success(&output, "capture-pane --json");
        serde_json::from_slice(&output.stdout).expect("parse capture-pane JSON")
    }

    fn wait_capture_contains(&self, selector: &str, needle: &str, timeout: Duration) -> String {
        wait_until(
            timeout,
            Duration::from_millis(200),
            &format!("capture-pane to contain '{}'", needle),
            || {
                let output = self.run_cli(&[
                    "capture-pane",
                    "--target",
                    selector,
                    "--plain-text",
                    "--lines",
                    "200",
                ]);
                if !output.status.success() {
                    return None;
                }
                let text = output_text(&output.stdout);
                if text.contains(needle) {
                    Some(text)
                } else {
                    None
                }
            },
        )
    }

    fn split_pane(
        &self,
        target: &str,
        direction: SplitDirection,
        working_dir: Option<&Path>,
    ) -> PaneId {
        let before = self.list_panes();
        let before_ids: HashSet<PaneId> = before.iter().map(|pane| pane.id).collect();

        let mut args = vec![
            "split-pane".to_string(),
            "--target".to_string(),
            target.to_string(),
            "--direction".to_string(),
            match direction {
                SplitDirection::Horizontal => "horizontal".to_string(),
                SplitDirection::Vertical => "vertical".to_string(),
            },
        ];
        if let Some(dir) = working_dir {
            args.push("--dir".to_string());
            args.push(dir.display().to_string());
        }

        let output = self.run_cli_owned(&args);
        assert_success(&output, "split-pane");

        wait_until(
            Duration::from_secs(20),
            Duration::from_millis(150),
            "newly split pane",
            || {
                self.try_list_panes().and_then(|panes| {
                    if panes.len() != before.len() + 1 {
                        return None;
                    }
                    panes
                        .into_iter()
                        .find(|pane| !before_ids.contains(&pane.id))
                        .map(|pane| pane.id)
                })
            },
        )
    }

    fn set_pane_meta(&self, selector: &str, alias: Option<&str>, role: Option<&str>) {
        let mut args = vec![
            "set-pane-meta".to_string(),
            "--pane".to_string(),
            selector.to_string(),
        ];
        if let Some(alias) = alias {
            args.push("--alias".to_string());
            args.push(alias.to_string());
        }
        if let Some(role) = role {
            args.push("--role".to_string());
            args.push(role.to_string());
        }

        let output = self.run_cli_owned(&args);
        assert_success(&output, "set-pane-meta");
    }

    fn send_keys_raw(&self, selector: &str, text: &str) {
        let args = vec![
            "send-keys".to_string(),
            "--pane".to_string(),
            selector.to_string(),
            "--raw".to_string(),
            "--enter".to_string(),
            text.to_string(),
        ];
        let output = self.run_cli_owned(&args);
        assert_success(&output, "send-keys --raw --enter");
    }

    fn exec_output_regex(
        &self,
        selector: &str,
        pattern: &str,
        timeout_secs: u64,
        command: Vec<String>,
    ) -> Output {
        let mut args = vec![
            "exec".to_string(),
            "--pane".to_string(),
            selector.to_string(),
            "--wait-for".to_string(),
            "output-regex".to_string(),
            "--output-regex".to_string(),
            pattern.to_string(),
            "--timeout".to_string(),
            timeout_secs.to_string(),
            "--".to_string(),
        ];
        args.extend(command);
        self.run_cli_owned(&args)
    }

    fn exec_output_regex_with_retries(
        &self,
        selector: &str,
        pattern: &str,
        timeout_secs: u64,
        command: Vec<String>,
        attempts: usize,
        context: &str,
    ) -> Output {
        let attempts = attempts.max(1);
        let mut last_output = None;
        for attempt in 0..attempts {
            let output = self.exec_output_regex(selector, pattern, timeout_secs, command.clone());
            if output.status.success() {
                return output;
            }
            last_output = Some(output);
            if attempt + 1 < attempts {
                sleep(Duration::from_millis(250));
            }
        }

        assert_success(
            &last_output.expect("at least one exec attempt should run"),
            context,
        );
        unreachable!("assert_success should panic on failure")
    }

    fn wait_for_shell_ready(&self, selector: &str) {
        let ready_token = unique_name("pane-ready");
        let _ = self.exec_output_regex_with_retries(
            selector,
            &ready_token,
            10,
            vec![
                "cmd".to_string(),
                "/c".to_string(),
                "echo".to_string(),
                ready_token.clone(),
            ],
            5,
            &format!("wait for pane {} shell readiness", selector),
        );
    }

    fn send_keys_and_wait_output_regex_with_retries(
        &self,
        selector: &str,
        attempts: usize,
        timeout_secs: u64,
        build_text: impl Fn(&str) -> String,
        context: &str,
    ) -> String {
        let attempts = attempts.max(1);
        let mut last_output = None;
        for attempt in 0..attempts {
            let token = unique_name("send-keys");
            self.send_keys_raw(selector, &build_text(&token));
            let output = self.run_cli_owned(&[
                "wait-pane".to_string(),
                "--pane".to_string(),
                selector.to_string(),
                "--wait-for".to_string(),
                "output-regex".to_string(),
                "--output-regex".to_string(),
                token.clone(),
                "--timeout".to_string(),
                timeout_secs.to_string(),
            ]);
            if output.status.success() {
                return token;
            }
            last_output = Some(output);
            if attempt + 1 < attempts {
                sleep(Duration::from_millis(250));
            }
        }

        assert_success(
            &last_output.expect("at least one send-keys attempt should run"),
            context,
        );
        unreachable!("assert_success should panic on failure")
    }

    fn block_on<T>(&self, future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build tokio runtime")
            .block_on(future)
    }

    fn send_save_and_quit(&self) {
        self.block_on(async {
            let mut conn = ServerConnection::connect(&self.session)
                .await
                .expect("connect for SaveAndQuit");
            conn.tx
                .send(ClientMessage::SaveAndQuit)
                .await
                .expect("send SaveAndQuit");

            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    match conn.rx.recv().await {
                        Some(ServerMessage::SaveAndQuit) => return,
                        Some(ServerMessage::Error { message }) => {
                            panic!("unexpected SaveAndQuit error: {}", message)
                        }
                        Some(_) => continue,
                        None => panic!("connection closed before SaveAndQuit ack"),
                    }
                }
            })
            .await
            .expect("timeout waiting for SaveAndQuit ack");
        });
    }
}

#[derive(Debug)]
struct SnapshotLeaf {
    command: Option<String>,
    cwd: Option<String>,
    alias: Option<String>,
    role: Option<String>,
}

fn collect_snapshot_leaves(node: &LayoutNodeDef, out: &mut Vec<SnapshotLeaf>) {
    match node {
        LayoutNodeDef::Leaf {
            command,
            cwd,
            alias,
            role,
            ..
        } => out.push(SnapshotLeaf {
            command: command.clone(),
            cwd: cwd.clone(),
            alias: alias.clone(),
            role: role.clone(),
        }),
        LayoutNodeDef::Split { first, second, .. } => {
            collect_snapshot_leaves(first, out);
            collect_snapshot_leaves(second, out);
        }
    }
}

/// 指定ペインへの `ServerMessage::Notification` を IPC ストリームから待つ。
///
/// 他のメッセージはスキップし、`timeout` 以内に到着しなければパニックする。
async fn wait_for_notification(
    conn: &mut ServerConnection,
    target_pane: PaneId,
    timeout: Duration,
) -> String {
    tokio::time::timeout(timeout, async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::Notification { pane, body }) if pane == target_pane => {
                    return body;
                }
                Some(ServerMessage::Error { message }) => {
                    panic!(
                        "unexpected error while waiting for notification: {}",
                        message
                    )
                }
                Some(_) => continue,
                None => panic!("connection closed before Notification arrived"),
            }
        }
    })
    .await
    .expect("timeout waiting for Notification")
}

async fn wait_for_exec_result(
    conn: &mut ServerConnection,
    request_id: &str,
    timeout: Duration,
) -> ServerMessage {
    tokio::time::timeout(timeout, async {
        loop {
            match conn.rx.recv().await {
                Some(
                    ref msg @ ServerMessage::ExecResult {
                        request_id: ref result_id,
                        ..
                    },
                ) if result_id == request_id => return msg.clone(),
                Some(ServerMessage::Error { message }) => {
                    panic!("unexpected exec error: {}", message)
                }
                Some(_) => continue,
                None => panic!("connection closed before ExecResult"),
            }
        }
    })
    .await
    .expect("timeout waiting for ExecResult")
}

struct MockHttpResponse {
    path: &'static str,
    status_line: &'static str,
    content_type: &'static str,
    body: Vec<u8>,
}

struct MockHttpServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<std::thread::JoinHandle<()>>,
}

impl MockHttpServer {
    fn base_url(&self) -> &str {
        &self.base_url
    }
}

impl Drop for MockHttpServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(handle) = self.handle.take() {
            let _ = handle.join();
        }
    }
}

fn spawn_mock_http_server<F>(build_responses: F) -> MockHttpServer
where
    F: FnOnce(&str) -> Vec<MockHttpResponse>,
{
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock HTTP server");
    listener
        .set_nonblocking(true)
        .expect("set mock HTTP listener nonblocking");
    let addr = listener.local_addr().expect("resolve mock HTTP address");
    let base_url = format!("http://{}", addr);
    let responses = build_responses(&base_url);
    let stop = Arc::new(AtomicBool::new(false));
    let stop_flag = Arc::clone(&stop);

    let handle = std::thread::spawn(move || {
        for response in responses {
            let deadline = Instant::now() + e2e_timeout();
            let (mut stream, _) = loop {
                if stop_flag.load(Ordering::SeqCst) {
                    return;
                }
                match listener.accept() {
                    Ok(stream) => break stream,
                    Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(
                            Instant::now() < deadline,
                            "timeout waiting for mock HTTP request for {}",
                            response.path
                        );
                        sleep(Duration::from_millis(50));
                    }
                    Err(err) => panic!("accept mock HTTP connection: {}", err),
                }
            };
            let mut buf = [0u8; 4096];
            let size = stream.read(&mut buf).expect("read HTTP request");
            let request = String::from_utf8_lossy(&buf[..size]);
            let request_line = request.lines().next().unwrap_or_default().to_string();
            assert!(
                request_line.starts_with(&format!("GET {} ", response.path)),
                "unexpected request line: {}",
                request_line
            );

            let headers = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nContent-Type: {}\r\nConnection: close\r\n\r\n",
                response.status_line,
                response.body.len(),
                response.content_type
            );
            stream
                .write_all(headers.as_bytes())
                .expect("write mock HTTP headers");
            stream
                .write_all(&response.body)
                .expect("write mock HTTP body");
        }
    });

    MockHttpServer {
        base_url,
        stop,
        handle: Some(handle),
    }
}

#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_startup_list_panes_and_capture_pane_smoke() {
    let _guard = lock_e2e_tests();
    let mut harness = AppHarness::new("yatamux-e2e");
    harness.spawn();

    let panes = harness.wait_for_pane_count(1);
    let pane_id = panes[0].id;
    let capture = harness.capture_json(&pane_id.0.to_string());

    assert_eq!(
        capture.get("pane").and_then(Value::as_u64),
        Some(u64::from(pane_id.0))
    );
    assert!(
        capture.get("content").is_some(),
        "capture should include content"
    );
}

#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_split_send_keys_wait_and_exec_flow() {
    let _guard = lock_e2e_tests();
    let mut harness = AppHarness::new("yatamux-e2e-flow");
    harness.spawn();

    let root = harness.wait_for_pane_count(1)[0].id;
    let workdir = harness.appdata_path().join("tc02-workdir");
    fs::create_dir_all(&workdir).expect("create tc02 workdir");

    let worker = harness.split_pane(
        &root.0.to_string(),
        SplitDirection::Vertical,
        Some(&workdir),
    );
    harness.set_pane_meta(&worker.0.to_string(), Some("worker"), Some("verifier"));

    let worker_info = harness.wait_for_alias("worker");
    assert_eq!(worker_info.role.as_deref(), Some("verifier"));
    harness.wait_for_shell_ready("worker");

    let send_token = harness.send_keys_and_wait_output_regex_with_retries(
        "worker",
        5,
        20,
        |token| format!("echo {}", token),
        "wait-pane for send-keys token",
    );
    let captured = harness.wait_capture_contains("worker", &send_token, Duration::from_secs(10));
    assert!(captured.contains(&send_token));

    let exec_token = "E2E_TC02_EXEC";
    let exec_output = harness.run_cli_owned(&[
        "exec".to_string(),
        "--pane".to_string(),
        "worker".to_string(),
        "--wait-for".to_string(),
        "output-regex".to_string(),
        "--output-regex".to_string(),
        exec_token.to_string(),
        "--timeout".to_string(),
        "20".to_string(),
        "--".to_string(),
        "cmd".to_string(),
        "/c".to_string(),
        "echo".to_string(),
        exec_token.to_string(),
    ]);
    assert_success(&exec_output, "exec end-to-end");
    let captured = harness.wait_capture_contains("worker", exec_token, Duration::from_secs(10));
    assert!(captured.contains(&send_token));
    assert!(captured.contains(exec_token));
}

#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_exec_protocol_reports_request_ids_and_statuses() {
    let _guard = lock_e2e_tests();
    let mut harness = AppHarness::new("yatamux-e2e-exec");
    harness.spawn();

    let root = harness.wait_for_pane_count(1)[0].id;
    let aux = harness.split_pane(&root.0.to_string(), SplitDirection::Vertical, None);

    harness.block_on(async {
        let mut conn = ServerConnection::connect(&harness.session)
            .await
            .expect("connect for exec tests");

        let request_ok = unique_name("exec-ok");
        conn.tx
            .send(ClientMessage::Exec {
                request_id: request_ok.clone(),
                pane: root,
                data: b"cmd /c echo E2E_TC03_OK\r".to_vec(),
                wait: ExecWaitCondition::OutputRegex {
                    pattern: "E2E_TC03_OK".to_string(),
                    lines: 120,
                },
                timeout_ms: 10_000,
            })
            .await
            .expect("send successful exec");
        match wait_for_exec_result(&mut conn, &request_ok, Duration::from_secs(10)).await {
            ServerMessage::ExecResult {
                request_id,
                pane,
                status,
                ..
            } => {
                assert_eq!(request_id, request_ok);
                assert_eq!(pane, root);
                assert_eq!(status, ExecStatus::Completed);
            }
            other => panic!("expected ExecResult, got {:?}", other),
        }

        let request_timeout = unique_name("exec-timeout");
        conn.tx
            .send(ClientMessage::Exec {
                request_id: request_timeout.clone(),
                pane: root,
                data: b"cmd /c echo E2E_TC03_TIMEOUT\r".to_vec(),
                wait: ExecWaitCondition::OutputRegex {
                    pattern: "NEVER_MATCH_TC03".to_string(),
                    lines: 120,
                },
                timeout_ms: 300,
            })
            .await
            .expect("send timeout exec");
        match wait_for_exec_result(&mut conn, &request_timeout, Duration::from_secs(10)).await {
            ServerMessage::ExecResult {
                request_id,
                pane,
                status,
                message,
                ..
            } => {
                assert_eq!(request_id, request_timeout);
                assert_eq!(pane, root);
                assert_eq!(status, ExecStatus::TimedOut);
                assert!(message
                    .as_deref()
                    .is_some_and(|text| text.contains("timeout waiting for pane")));
            }
            other => panic!("expected timed out ExecResult, got {:?}", other),
        }

        let request_closed = unique_name("exec-closed");
        conn.tx
            .send(ClientMessage::Exec {
                request_id: request_closed.clone(),
                pane: aux,
                data: b"cmd /c echo E2E_TC03_CLOSE\r".to_vec(),
                wait: ExecWaitCondition::OutputRegex {
                    pattern: "NEVER_MATCH_TC03_CLOSE".to_string(),
                    lines: 120,
                },
                timeout_ms: 5_000,
            })
            .await
            .expect("send pane-close exec");
        conn.tx
            .send(ClientMessage::ClosePane { pane: aux })
            .await
            .expect("close exec pane");
        match wait_for_exec_result(&mut conn, &request_closed, Duration::from_secs(10)).await {
            ServerMessage::ExecResult {
                request_id,
                pane,
                status,
                message,
                ..
            } => {
                assert_eq!(request_id, request_closed);
                assert_eq!(pane, aux);
                assert_eq!(status, ExecStatus::PaneClosed);
                assert!(message
                    .as_deref()
                    .is_some_and(|text| text.contains("closed before regex matched")));
            }
            other => panic!("expected pane-closed ExecResult, got {:?}", other),
        }
    });
}

#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_subscribe_interrupt_close_and_terminate_flow() {
    let _guard = lock_e2e_tests();
    let mut harness = AppHarness::new("yatamux-e2e-stream");
    harness.spawn();

    let root = harness.wait_for_pane_count(1)[0].id;
    let stream = harness.split_pane(&root.0.to_string(), SplitDirection::Vertical, None);
    harness.set_pane_meta(&stream.0.to_string(), Some("stream"), Some("observer"));
    let stream_info = harness.wait_for_alias("stream");
    assert_eq!(stream_info.role.as_deref(), Some("observer"));
    harness.wait_for_shell_ready("stream");

    let mut subscriber = harness.cli_command(&["subscribe-pane", "--pane", "stream", "--json"]);
    let subscriber = subscriber
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn subscribe-pane");

    let stream_token = "E2E_TC04_STREAM";
    let stream_output = harness.exec_output_regex_with_retries(
        "stream",
        stream_token,
        20,
        vec![
            "cmd".to_string(),
            "/c".to_string(),
            "echo".to_string(),
            stream_token.to_string(),
        ],
        5,
        "exec stream token",
    );
    assert_success(&stream_output, "exec stream token");
    let stream_capture =
        harness.wait_capture_contains("stream", stream_token, Duration::from_secs(20));
    assert!(stream_capture.contains(stream_token));

    let close_output = harness.run_cli(&["close-pane", "--pane", "stream"]);
    assert_success(&close_output, "close-pane");

    let subscriber_output = wait_with_output(subscriber, Duration::from_secs(10));
    assert_success(&subscriber_output, "subscribe-pane --json");
    let events = parse_json_lines(&subscriber_output.stdout);
    assert!(events.iter().any(|event| {
        event.get("event").and_then(Value::as_str) == Some("output")
            && event
                .get("text")
                .and_then(Value::as_str)
                .is_some_and(|text| text.contains(stream_token))
    }));
    assert!(events.iter().any(|event| {
        event.get("event").and_then(Value::as_str) == Some("pane_closed")
            && event.get("pane").and_then(Value::as_u64) == Some(u64::from(stream.0))
    }));

    let control = harness.split_pane(&root.0.to_string(), SplitDirection::Vertical, None);
    harness.set_pane_meta(&control.0.to_string(), Some("control"), Some("runner"));
    harness.wait_for_shell_ready("control");

    let interrupt_start = "E2E_TC04_INTERRUPT_START";
    let control_output = harness.exec_output_regex_with_retries(
        "control",
        interrupt_start,
        20,
        vec![
            "cmd".to_string(),
            "/c".to_string(),
            format!("echo {} & ping 127.0.0.1 -t", interrupt_start),
        ],
        5,
        "start interruptable loop",
    );
    assert_success(&control_output, "start interruptable loop");

    let interrupt_output = harness.run_cli(&["interrupt-pane", "--pane", "control"]);
    assert_success(&interrupt_output, "interrupt-pane");
    let silence_output = harness.run_cli(&[
        "wait-pane",
        "--pane",
        "control",
        "--wait-for",
        "silence",
        "--silence-ms",
        "1000",
        "--timeout",
        "20",
    ]);
    assert_success(&silence_output, "wait-pane silence after interrupt");
    assert!(harness.list_panes().iter().any(|pane| pane.id == control));

    let terminate = harness.split_pane(&root.0.to_string(), SplitDirection::Vertical, None);
    harness.set_pane_meta(&terminate.0.to_string(), Some("terminate"), Some("runner"));
    let terminate_info = harness.wait_for_alias("terminate");
    assert_eq!(terminate_info.id, terminate);

    let terminate_output = harness.run_cli(&["terminate-pane", "--pane", "terminate"]);
    assert_success(&terminate_output, "terminate-pane");
    harness.wait_for_pane_gone(terminate);
}

#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_save_and_quit_writes_session_snapshot() {
    let _guard = lock_e2e_tests();
    let mut harness = AppHarness::new("yatamux-e2e-save");
    harness.spawn();

    let root = harness.wait_for_pane_count(1)[0].id;
    let worker_dir = harness.appdata_path().join("worker-cwd");
    fs::create_dir_all(&worker_dir).expect("create worker cwd");

    let worker = harness.split_pane(
        &root.0.to_string(),
        SplitDirection::Vertical,
        Some(&worker_dir),
    );
    harness.set_pane_meta(&root.0.to_string(), Some("main"), Some("planner"));
    harness.set_pane_meta(&worker.0.to_string(), Some("worker"), Some("verifier"));
    harness.wait_for_shell_ready("worker");
    let ready_token = "E2E_TC05_READY";
    let worker_output = harness.exec_output_regex_with_retries(
        "worker",
        ready_token,
        20,
        vec![
            "cmd".to_string(),
            "/c".to_string(),
            format!("echo {} & ping 127.0.0.1 -t", ready_token),
        ],
        5,
        "start save-and-quit worker loop",
    );
    assert_success(&worker_output, "start save-and-quit worker loop");
    sleep(Duration::from_millis(500));

    harness.send_save_and_quit();
    harness.app_mut().wait_for_exit(Duration::from_secs(20));

    let snapshot = LayoutSnapshot::load(&harness.session_path()).expect("load session snapshot");
    let mut leaves = Vec::new();
    collect_snapshot_leaves(&snapshot.root, &mut leaves);
    assert_eq!(leaves.len(), 2);

    let main = leaves
        .iter()
        .find(|leaf| leaf.alias.as_deref() == Some("main"))
        .expect("main leaf should be saved");
    assert_eq!(main.role.as_deref(), Some("planner"));

    let worker = leaves
        .iter()
        .find(|leaf| leaf.alias.as_deref() == Some("worker"))
        .expect("worker leaf should be saved");
    assert_eq!(worker.role.as_deref(), Some("verifier"));
    assert_eq!(
        worker.cwd.as_deref(),
        Some(worker_dir.to_string_lossy().as_ref())
    );
    assert!(worker
        .command
        .as_deref()
        .is_some_and(|command| command.to_lowercase().contains("ping")));
}

#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_saved_session_restores_alias_role_cwd_and_active_state() {
    let _guard = lock_e2e_tests();
    let mut harness = AppHarness::new("yatamux-e2e-restore");
    let main_dir = harness.appdata_path().join("restore-main");
    let tests_dir = harness.appdata_path().join("restore-tests");
    fs::create_dir_all(&main_dir).expect("create main restore dir");
    fs::create_dir_all(&tests_dir).expect("create tests restore dir");

    let snapshot = LayoutSnapshot {
        root: LayoutNodeDef::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNodeDef::Leaf {
                id: PaneId(1),
                command: Some("cmd /c echo E2E_TC06_MAIN".to_string()),
                cwd: Some(main_dir.to_string_lossy().into_owned()),
                alias: Some("main".to_string()),
                role: Some("planner".to_string()),
            }),
            second: Box::new(LayoutNodeDef::Leaf {
                id: PaneId(2),
                command: Some("cmd /c echo E2E_TC06_TESTS".to_string()),
                cwd: Some(tests_dir.to_string_lossy().into_owned()),
                alias: Some("tests".to_string()),
                role: Some("verifier".to_string()),
            }),
        },
        active: PaneId(1),
    };
    snapshot
        .save(&harness.session_path())
        .expect("write restore session snapshot");

    harness.spawn();

    let panes = wait_until(
        Duration::from_secs(20),
        Duration::from_millis(200),
        "restored panes with active metadata",
        || {
            let panes = harness.try_list_panes()?;
            if panes.len() != 2 {
                return None;
            }
            let has_main = panes.iter().any(|pane| {
                pane.alias.as_deref() == Some("main")
                    && pane.role.as_deref() == Some("planner")
                    && pane.active
                    && !pane.floating
                    && pane.cwd.as_deref() == Some(main_dir.to_string_lossy().as_ref())
            });
            let has_tests = panes.iter().any(|pane| {
                pane.alias.as_deref() == Some("tests")
                    && pane.role.as_deref() == Some("verifier")
                    && !pane.active
                    && !pane.floating
                    && pane.cwd.as_deref() == Some(tests_dir.to_string_lossy().as_ref())
            });
            if has_main && has_tests {
                Some(panes)
            } else {
                None
            }
        },
    );

    let main = panes
        .iter()
        .find(|pane| pane.alias.as_deref() == Some("main"))
        .expect("restored main pane");
    let tests = panes
        .iter()
        .find(|pane| pane.alias.as_deref() == Some("tests"))
        .expect("restored tests pane");
    assert!(main.active);
    assert!(!tests.active);
    assert!(!main.floating);
    assert!(!tests.floating);

    let main_capture =
        harness.wait_capture_contains("main", "E2E_TC06_MAIN", Duration::from_secs(10));
    assert!(main_capture.contains("E2E_TC06_MAIN"));
    let tests_capture =
        harness.wait_capture_contains("tests", "E2E_TC06_TESTS", Duration::from_secs(10));
    assert!(tests_capture.contains("E2E_TC06_TESTS"));
}

#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_self_update_smoke_preserves_session_and_relaunches() {
    let _guard = lock_e2e_tests();
    let exe_dir = TempDir::new("yatamux-e2e-update-exe");
    let copied_exe = exe_dir.path().join("yatamux.exe");
    fs::copy(yatamux_exe(), &copied_exe).expect("copy yatamux.exe for self-update smoke");

    let mut harness = AppHarness::with_exe("yatamux-e2e-update", copied_exe.clone());
    harness.spawn();

    let root = harness.wait_for_pane_count(1)[0].id;
    let startup_probe_path = harness.appdata_path().join("startup-probe.jsonl");
    let worker_dir = harness.appdata_path().join("update-worker");
    fs::create_dir_all(&worker_dir).expect("create update worker dir");
    let worker = harness.split_pane(
        &root.0.to_string(),
        SplitDirection::Vertical,
        Some(&worker_dir),
    );
    harness.set_pane_meta(&worker.0.to_string(), Some("worker"), Some("updater"));
    harness.wait_for_shell_ready("worker");
    harness.send_keys_raw("worker", "ping 127.0.0.1 -t");
    sleep(Duration::from_millis(800));

    let staged_binary = fs::read(&copied_exe).expect("read copied exe");
    let checksum = format!("{:x}", Sha256::digest(&staged_binary));
    let mock_server = spawn_mock_http_server(|base_url| {
        vec![
            MockHttpResponse {
                path: "/latest",
                status_line: "200 OK",
                content_type: "application/json",
                body: format!(
                    r#"{{
                        "tag_name": "v999.0.0",
                        "published_at": "2026-04-06T12:34:56Z",
                        "assets": [
                            {{
                                "name": "yatamux.exe",
                                "browser_download_url": "{base_url}/downloads/yatamux.exe"
                            }},
                            {{
                                "name": "checksums.txt",
                                "browser_download_url": "{base_url}/downloads/checksums.txt"
                            }}
                        ]
                    }}"#
                )
                .into_bytes(),
            },
            MockHttpResponse {
                path: "/downloads/checksums.txt",
                status_line: "200 OK",
                content_type: "text/plain",
                body: format!("{}  yatamux.exe\n", checksum).into_bytes(),
            },
            MockHttpResponse {
                path: "/downloads/yatamux.exe",
                status_line: "200 OK",
                content_type: "application/octet-stream",
                body: staged_binary,
            },
        ]
    });

    let update_child = build_command(&harness.exe, harness.appdata_path(), &harness.session)
        .arg("update")
        .env(
            "YATAMUX_UPDATE_API_URL",
            format!("{}/latest", mock_server.base_url()),
        )
        .env("YATAMUX_STARTUP_PROBE_FILE", &startup_probe_path)
        .env("YATAMUX_STARTUP_PROBE_EXIT", "1")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn yatamux update");
    let update_output = wait_with_output(update_child, e2e_timeout());
    assert_success(&update_output, "yatamux update");

    harness.app_mut().wait_for_exit(Duration::from_secs(30));

    let relaunch_probe = wait_until(
        Duration::from_secs(30),
        Duration::from_millis(100),
        "updated yatamux relaunch startup probe",
        || {
            let lines = parse_json_lines(&fs::read(&startup_probe_path).ok()?);
            lines.into_iter().find(|entry| {
                entry.get("command").is_some_and(Value::is_null)
                    && entry.get("apply_update").and_then(Value::as_bool) == Some(false)
                    && entry.get("session").and_then(Value::as_str)
                        == Some(harness.session.as_str())
                    && entry.get("appdata").and_then(Value::as_str)
                        == Some(harness.appdata_path().to_string_lossy().as_ref())
            })
        },
    );
    assert_eq!(
        relaunch_probe.get("exe").and_then(Value::as_str),
        Some(copied_exe.to_string_lossy().as_ref())
    );
    assert!(
        harness.session_path().exists(),
        "session.toml should be saved"
    );
    assert!(copied_exe.exists(), "updated exe should remain in place");
    assert!(
        copied_exe.with_extension("exe.bak").exists(),
        "old exe should be backed up as .bak"
    );
    assert!(
        !copied_exe.with_extension("exe.new").exists(),
        "staged .new should be consumed"
    );

    harness.wait_for_ipc_unavailable(Duration::from_secs(20));
}

// ── C-41: 通知・アラート E2E ─────────────────────────────────────────────────

/// バックグラウンドペインに BEL を送ると ServerMessage::Notification が届くことを確認する。
///
/// 検証内容:
/// - IPC 経由で `ServerMessage::Notification { pane: bg, body: "Bell" }` が到着する
/// - `notify_if_inactive` 経路（bg != active）が正しく通ること
///
/// Win32 アラートボーダーの点滅は視覚的 E2E であり、ここでは IPC 到達を検証する。
#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_bel_on_background_pane_triggers_notification() {
    let _guard = lock_e2e_tests();
    let mut harness = AppHarness::new("yatamux-e2e-alert-bel");
    harness.spawn();

    // 起動待機 — initial pane が active (pane_store.active = root)
    let root = harness.wait_for_pane_count(1)[0].id;

    // IPC 分割で bg ペインを作成。IPC 起点では pane_store.active は変わらないため
    // bg != active となり、BEL が notify_if_inactive を通過する。
    let bg = harness.split_pane(&root.0.to_string(), SplitDirection::Vertical, None);
    harness.wait_for_pane_count(2);
    // bg ペインのシェルが起動するまで待つ（コマンドを受け付けられる状態にする）
    harness.wait_for_shell_ready(&bg.0.to_string());

    harness.block_on(async {
        let mut conn = ServerConnection::connect(&harness.session)
            .await
            .expect("connect for alert BEL test");

        // PowerShell で BEL (0x07) を PTY 出力に書き出す。
        // ClientMessage::Input は PTY stdin に書くため、cmd.exe がコマンドとして実行し
        // PowerShell が [char]7 を stdout に出力 → VtProcessor が BEL を検出する。
        conn.tx
            .send(ClientMessage::Input {
                pane: bg,
                data: b"powershell -nop -c \"[Console]::Write([char]7)\"\r".to_vec(),
            })
            .await
            .expect("send BEL command to bg pane");

        // Notification が IPC ストリームに流れることを確認
        let body = wait_for_notification(&mut conn, bg, Duration::from_secs(10)).await;
        assert_eq!(
            body, "Bell",
            "BEL should produce Notification with body 'Bell', got: {body:?}"
        );
    });
}

/// バックグラウンドペインでプロセスが終了すると Notification が届くことを確認する。
///
/// 検証内容:
/// - `cmd /c exit` を bg ペインで実行し PTY が閉じる
/// - `ServerMessage::Notification { pane: bg, body: "Process exited" }` が IPC 到達する
#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_process_exit_on_background_pane_triggers_notification() {
    let _guard = lock_e2e_tests();
    let mut harness = AppHarness::new("yatamux-e2e-alert-exit");
    harness.spawn();

    let root = harness.wait_for_pane_count(1)[0].id;

    // bg ペイン作成（root が active のまま）
    let bg = harness.split_pane(&root.0.to_string(), SplitDirection::Vertical, None);
    harness.wait_for_pane_count(2);
    // bg ペインのシェルが起動するまで待つ
    harness.wait_for_shell_ready(&bg.0.to_string());

    harness.block_on(async {
        let mut conn = ServerConnection::connect(&harness.session)
            .await
            .expect("connect for alert exit test");

        // bg ペインのシェル（cmd.exe）を終了させる
        conn.tx
            .send(ClientMessage::Input {
                pane: bg,
                data: b"exit\r".to_vec(),
            })
            .await
            .expect("send exit to bg pane");

        // PTY 終了通知が IPC に流れることを確認
        let body = wait_for_notification(&mut conn, bg, Duration::from_secs(15)).await;
        assert_eq!(
            body, "Process exited",
            "PTY exit should produce Notification 'Process exited', got: {body:?}"
        );
    });
}

/// OSC 9 通知文字列を bg ペインに送ると Notification が届くことを確認する。
///
/// 検証内容:
/// - OSC 9 は tmux 互換の即時通知トリガー
/// - `\x1b]9;hello\x07` → `ServerMessage::Notification { body: "hello" }`
#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_osc9_on_background_pane_triggers_notification() {
    let _guard = lock_e2e_tests();
    let mut harness = AppHarness::new("yatamux-e2e-alert-osc9");
    harness.spawn();

    let root = harness.wait_for_pane_count(1)[0].id;
    let bg = harness.split_pane(&root.0.to_string(), SplitDirection::Vertical, None);
    harness.wait_for_pane_count(2);
    // bg ペインのシェルが起動するまで待つ
    harness.wait_for_shell_ready(&bg.0.to_string());

    harness.block_on(async {
        let mut conn = ServerConnection::connect(&harness.session)
            .await
            .expect("connect for OSC 9 test");

        // PowerShell で OSC 9 シーケンス（ESC ] 9 ; <body> BEL）を PTY 出力に書き出す。
        // [string][char]27 = ESC、[string][char]7 = BEL をシングルクォート文字列と結合する。
        conn.tx
            .send(ClientMessage::Input {
                pane: bg,
                data: b"powershell -nop -c \"[Console]::Write([string][char]27+']9;e2e-osc9-test'+[string][char]7)\"\r".to_vec(),
            })
            .await
            .expect("send OSC 9 command to bg pane");

        let body = wait_for_notification(&mut conn, bg, Duration::from_secs(10)).await;
        assert_eq!(
            body, "e2e-osc9-test",
            "OSC 9 should produce Notification with the message body, got: {body:?}"
        );
    });
}
