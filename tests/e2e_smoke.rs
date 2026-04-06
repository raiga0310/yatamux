#![cfg(windows)]

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::thread::sleep;
use std::time::{Duration, Instant};

use serde_json::Value;

const CREATE_NO_WINDOW: u32 = 0x0800_0000;

fn unique_name(prefix: &str) -> String {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::SeqCst);
    format!("{}-{}-{}", prefix, std::process::id(), n)
}

fn yatamux_exe() -> &'static str {
    env!("CARGO_BIN_EXE_yatamux")
}

struct TempAppData {
    path: PathBuf,
}

impl TempAppData {
    fn new(prefix: &str) -> Self {
        let path = std::env::temp_dir().join(unique_name(prefix));
        fs::create_dir_all(&path).expect("create temp APPDATA");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempAppData {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

struct AppProcess {
    child: Child,
}

impl AppProcess {
    fn spawn(appdata: &Path, session: &str) -> Self {
        use std::os::windows::process::CommandExt;

        let child = Command::new(yatamux_exe())
            .arg("--session")
            .arg(session)
            .env("APPDATA", appdata)
            .creation_flags(CREATE_NO_WINDOW)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn yatamux GUI");
        Self { child }
    }

    fn wait_for_exit(&mut self, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        loop {
            match self.child.try_wait().expect("poll child exit") {
                Some(status) => {
                    assert!(status.success(), "yatamux exited with status {status}");
                    return;
                }
                None if Instant::now() >= deadline => {
                    panic!("timeout waiting for yatamux to exit");
                }
                None => sleep(Duration::from_millis(100)),
            }
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

fn run_cli(appdata: &Path, session: &str, args: &[&str]) -> Output {
    use std::os::windows::process::CommandExt;

    Command::new(yatamux_exe())
        .arg("--session")
        .arg(session)
        .args(args)
        .env("APPDATA", appdata)
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .expect("run yatamux CLI")
}

fn wait_for_panes(appdata: &Path, session: &str) -> Vec<Value> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let output = run_cli(appdata, session, &["list-panes", "--json"]);
        if output.status.success() {
            let panes: Vec<Value> =
                serde_json::from_slice(&output.stdout).expect("parse list-panes JSON");
            if !panes.is_empty() {
                return panes;
            }
        }

        if Instant::now() >= deadline {
            let stderr = String::from_utf8_lossy(&output.stderr);
            panic!("timeout waiting for IPC readiness: {}", stderr.trim());
        }
        sleep(Duration::from_millis(150));
    }
}

#[test]
#[ignore = "starts a real yatamux GUI process"]
fn e2e_startup_list_panes_and_capture_pane_smoke() {
    let appdata = TempAppData::new("yatamux-e2e");
    let session = unique_name("e2e-smoke");
    let mut app = AppProcess::spawn(appdata.path(), &session);

    let panes = wait_for_panes(appdata.path(), &session);
    let pane_id = panes[0]
        .get("id")
        .and_then(Value::as_u64)
        .expect("pane id should be present");

    let capture = run_cli(
        appdata.path(),
        &session,
        &["capture-pane", "--target", &pane_id.to_string(), "--json"],
    );
    assert!(
        capture.status.success(),
        "capture-pane failed: {}",
        String::from_utf8_lossy(&capture.stderr).trim()
    );
    let capture_json: Value =
        serde_json::from_slice(&capture.stdout).expect("parse capture-pane JSON");
    assert_eq!(
        capture_json.get("pane").and_then(Value::as_u64),
        Some(pane_id)
    );
    assert!(
        capture_json.get("content").is_some(),
        "capture should include content"
    );

    let close = run_cli(
        appdata.path(),
        &session,
        &["close-pane", "--pane", &pane_id.to_string()],
    );
    assert!(
        close.status.success(),
        "close-pane failed: {}",
        String::from_utf8_lossy(&close.stderr).trim()
    );

    app.wait_for_exit(Duration::from_secs(10));
}
