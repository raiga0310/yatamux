#![cfg(windows)]

use std::fs;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;
use sha2::{Digest, Sha256};

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

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

fn output_text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn parse_json_lines(bytes: &[u8]) -> Vec<Value> {
    output_text(bytes)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str::<Value>(line).expect("parse JSON line"))
        .collect()
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
    let deadline = Instant::now() + timeout;
    loop {
        if child.try_wait().expect("poll child").is_some() {
            return child.wait_with_output().expect("collect child output");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("timeout waiting for child process output");
        }
        sleep(Duration::from_millis(100));
    }
}

fn run_cli(exe: &Path, appdata: &Path, session: &str, args: &[&str], timeout: Duration) -> Output {
    let child = build_command(exe, appdata, session)
        .args(args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn CLI process");
    wait_with_output(child, timeout)
}

fn wait_until<T, F>(timeout: Duration, interval: Duration, description: &str, mut f: F) -> T
where
    F: FnMut() -> Option<T>,
{
    let deadline = Instant::now() + timeout;
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
        let deadline = Instant::now() + timeout;
        loop {
            if self.child.try_wait().expect("poll GUI").is_some() {
                return;
            }
            if Instant::now() >= deadline {
                let _ = self.child.kill();
                let _ = self.child.wait();
                panic!("timeout waiting for yatamux GUI to exit");
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
            let deadline = Instant::now() + Duration::from_secs(20);
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
fn apply_update_helper_mode_writes_probe_and_exits() {
    let appdata = TempDir::new("yatamux-helper-probe-appdata");
    let probe_dir = TempDir::new("yatamux-helper-probe");
    let probe_path = probe_dir.path().join("helper-probe.json");
    let new_path = probe_dir.path().join("yatamux.exe.new");
    fs::write(&new_path, b"staged binary").expect("write staged binary placeholder");

    let session = unique_name("helper-probe");
    let child = build_command(&yatamux_exe(), appdata.path(), &session)
        .arg("--apply-update")
        .arg("0")
        .arg(&new_path)
        .arg("--launch")
        .env("YATAMUX_UPDATE_HELPER_PROBE_FILE", &probe_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn helper mode");
    let output = wait_with_output(child, Duration::from_secs(10));
    assert_success(&output, "helper mode probe");

    let probe = wait_until(
        Duration::from_secs(5),
        Duration::from_millis(50),
        "helper probe file",
        || fs::read(&probe_path).ok(),
    );
    let json: Value = serde_json::from_slice(&probe).expect("parse probe JSON");
    assert_eq!(json.get("pid").and_then(Value::as_u64), Some(0));
    assert_eq!(
        json.get("new_path").and_then(Value::as_str),
        Some(new_path.to_string_lossy().as_ref())
    );
    assert_eq!(json.get("launch").and_then(Value::as_bool), Some(true));
    assert_eq!(
        json.get("session").and_then(Value::as_str),
        Some(session.as_str())
    );
}

#[test]
fn update_fallback_spawns_helper_and_returns() {
    let exe_dir = TempDir::new("yatamux-update-helper-exe");
    let copied_exe = exe_dir.path().join("yatamux.exe");
    fs::copy(yatamux_exe(), &copied_exe).expect("copy yatamux.exe");

    let appdata = TempDir::new("yatamux-update-helper-appdata");
    let probe_path = appdata.path().join("helper-probe.json");
    let binary = fs::read(&copied_exe).expect("read copied exe");
    let checksum = format!("{:x}", Sha256::digest(&binary));
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
                body: binary,
            },
        ]
    });

    let session = unique_name("update-fallback");
    let child = build_command(&copied_exe, appdata.path(), &session)
        .arg("update")
        .env(
            "YATAMUX_UPDATE_API_URL",
            format!("{}/latest", mock_server.base_url()),
        )
        .env("YATAMUX_UPDATE_HELPER_PROBE_FILE", &probe_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn yatamux update");
    let output = wait_with_output(child, Duration::from_secs(20));
    assert_success(&output, "yatamux update fallback helper spawn");

    let probe = wait_until(
        Duration::from_secs(5),
        Duration::from_millis(50),
        "fallback helper probe file",
        || fs::read(&probe_path).ok(),
    );
    let json: Value = serde_json::from_slice(&probe).expect("parse fallback helper probe JSON");
    assert_eq!(json.get("launch").and_then(Value::as_bool), Some(false));
    assert_eq!(
        json.get("session").and_then(Value::as_str),
        Some(session.as_str())
    );
    assert!(
        json.get("pid")
            .and_then(Value::as_u64)
            .is_some_and(|pid| pid > 0),
        "fallback helper should receive a real wait PID"
    );
    assert!(
        copied_exe.with_extension("exe.new").exists(),
        "update should leave staged .new behind until the helper applies it"
    );
    assert!(
        !copied_exe.with_extension("exe.bak").exists(),
        "probe helper should skip replacement work"
    );
}

#[test]
fn update_gui_path_spawns_detached_helper_and_returns() {
    let exe_dir = TempDir::new("yatamux-update-gui-exe");
    let copied_exe = exe_dir.path().join("yatamux.exe");
    fs::copy(yatamux_exe(), &copied_exe).expect("copy yatamux.exe");

    let appdata = TempDir::new("yatamux-update-gui-appdata");
    let probe_path = appdata.path().join("helper-probe.json");
    let binary = fs::read(&copied_exe).expect("read copied exe");
    let checksum = format!("{:x}", Sha256::digest(&binary));
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
                body: binary,
            },
        ]
    });

    let session = unique_name("update-gui");
    let mut app = AppProcess::spawn(&copied_exe, appdata.path(), &session);
    let list = wait_until(
        Duration::from_secs(10),
        Duration::from_millis(150),
        "GUI IPC startup",
        || {
            let output = run_cli(
                &copied_exe,
                appdata.path(),
                &session,
                &["list-panes", "--json"],
                Duration::from_secs(5),
            );
            output.status.success().then_some(output)
        },
    );
    assert_success(&list, "list-panes after GUI spawn");

    let update_child = build_command(&copied_exe, appdata.path(), &session)
        .arg("update")
        .env(
            "YATAMUX_UPDATE_API_URL",
            format!("{}/latest", mock_server.base_url()),
        )
        .env("YATAMUX_UPDATE_HELPER_PROBE_FILE", &probe_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn yatamux update GUI path");
    let update = wait_with_output(update_child, Duration::from_secs(20));
    assert_success(&update, "yatamux update GUI path");

    let probe = wait_until(
        Duration::from_secs(5),
        Duration::from_millis(50),
        "GUI helper probe file",
        || fs::read(&probe_path).ok(),
    );
    let json: Value = serde_json::from_slice(&probe).expect("parse GUI helper probe JSON");
    assert_eq!(json.get("launch").and_then(Value::as_bool), Some(true));
    assert_eq!(
        json.get("session").and_then(Value::as_str),
        Some(session.as_str())
    );

    app.wait_for_exit(Duration::from_secs(20));

    let output = run_cli(
        &copied_exe,
        appdata.path(),
        &session,
        &["list-panes", "--json"],
        Duration::from_secs(5),
    );
    assert!(
        !output.status.success(),
        "GUI session should be gone after SaveAndQuit"
    );

    assert!(
        copied_exe.with_extension("exe.new").exists(),
        "GUI-path probe helper should leave staged .new behind"
    );
    assert!(
        !copied_exe.with_extension("exe.bak").exists(),
        "probe helper should skip replacement work"
    );

    drop(mock_server);
}

#[test]
fn update_gui_path_relaunches_with_same_session_and_appdata() {
    let exe_dir = TempDir::new("yatamux-update-relaunch-exe");
    let copied_exe = exe_dir.path().join("yatamux.exe");
    fs::copy(yatamux_exe(), &copied_exe).expect("copy yatamux.exe");

    let appdata = TempDir::new("yatamux-update-relaunch-appdata");
    let startup_probe_path = appdata.path().join("startup-probe.jsonl");
    let binary = fs::read(&copied_exe).expect("read copied exe");
    let checksum = format!("{:x}", Sha256::digest(&binary));
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
                body: binary,
            },
        ]
    });

    let session = unique_name("update-relaunch");
    let mut app = AppProcess::spawn(&copied_exe, appdata.path(), &session);
    let list = wait_until(
        Duration::from_secs(10),
        Duration::from_millis(150),
        "GUI IPC startup",
        || {
            let output = run_cli(
                &copied_exe,
                appdata.path(),
                &session,
                &["list-panes", "--json"],
                Duration::from_secs(5),
            );
            output.status.success().then_some(output)
        },
    );
    assert_success(&list, "list-panes after GUI spawn");

    let update_child = build_command(&copied_exe, appdata.path(), &session)
        .arg("update")
        .env(
            "YATAMUX_UPDATE_API_URL",
            format!("{}/latest", mock_server.base_url()),
        )
        .env("YATAMUX_STARTUP_PROBE_FILE", &startup_probe_path)
        .env("YATAMUX_STARTUP_PROBE_EXIT", "1")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn yatamux update relaunch path");
    let update = wait_with_output(update_child, Duration::from_secs(20));
    assert_success(&update, "yatamux update relaunch path");

    app.wait_for_exit(Duration::from_secs(20));

    let relaunch_probe = wait_until(
        Duration::from_secs(20),
        Duration::from_millis(100),
        "relaunch startup probe entry",
        || {
            let lines = parse_json_lines(&fs::read(&startup_probe_path).ok()?);
            lines.into_iter().find(|entry| {
                entry.get("command").is_some_and(Value::is_null)
                    && entry.get("apply_update").and_then(Value::as_bool) == Some(false)
                    && entry.get("session").and_then(Value::as_str) == Some(session.as_str())
                    && entry.get("appdata").and_then(Value::as_str)
                        == Some(appdata.path().to_string_lossy().as_ref())
            })
        },
    );

    assert_eq!(
        relaunch_probe.get("exe").and_then(Value::as_str),
        Some(copied_exe.to_string_lossy().as_ref())
    );
    assert!(
        copied_exe.exists(),
        "updated exe should remain in place after relaunch"
    );
    assert!(
        copied_exe.with_extension("exe.bak").exists(),
        "old exe should be backed up as .bak after relaunch"
    );
    assert!(
        !copied_exe.with_extension("exe.new").exists(),
        "staged .new should be consumed after relaunch"
    );

    drop(mock_server);
}
