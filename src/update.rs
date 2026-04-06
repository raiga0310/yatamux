//! セルフアップデート機能（C-38）
//!
//! GitHub Releases からバイナリを取得し、SHA256 を検証したあと
//! `--apply-update` ヘルパーモードでバイナリ置換を行う。
//!
//! ## テスト計画
//! `docs/test-plan-self-update.md` を参照。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const DEFAULT_RELEASE_API_URL: &str =
    "https://api.github.com/repos/raiga0310/yatamux/releases/latest";

/// GitHub Releases API から取得したリリース情報
#[derive(Debug, PartialEq)]
pub struct ReleaseInfo {
    /// タグ名（例: `v0.2.0`）
    pub tag_name: String,
    /// `yatamux.exe` アセットのダウンロード URL
    pub asset_url: String,
    /// `checksums.txt` アセットのダウンロード URL
    pub checksum_url: String,
    /// リリース公開日時（ISO 8601、例: `2026-04-05T09:00:00Z`）
    pub published_at: Option<String>,
}

/// バージョン文字列をパースして `(major, minor, patch)` を返す。
///
/// 先頭の `v` は無視する。パース失敗時は `None`。
fn parse_version(v: &str) -> Option<(u32, u32, u32)> {
    let v = v.trim_start_matches('v');
    // pre-release suffix（`-beta.1` など）がある場合は None
    if v.contains('-') {
        return None;
    }
    let mut parts = v.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next()?.parse().ok()?;
    let patch: u32 = parts.next()?.parse().ok()?;
    Some((major, minor, patch))
}

/// `latest` が `current` より新しいバージョンかどうかを返す。
///
/// pre-release バージョン（`-` を含む）は更新対象外とみなす。
pub fn need_update(current: &str, latest: &str) -> bool {
    match (parse_version(current), parse_version(latest)) {
        (Some(c), Some(l)) => l > c,
        _ => false,
    }
}

/// GitHub Releases API の JSON レスポンスをパースする。
///
/// `yatamux.exe` と `checksums.txt` の両アセットが揃っているリリースのみ `Some` を返す。
pub fn parse_release_info(json: &str) -> Option<ReleaseInfo> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let tag_name = v["tag_name"].as_str()?.to_string();
    let published_at = v["published_at"].as_str().map(|s| s.to_string());
    let assets = v["assets"].as_array()?;

    let mut asset_url: Option<String> = None;
    let mut checksum_url: Option<String> = None;

    for asset in assets {
        let name = asset["name"].as_str().unwrap_or("");
        let url = asset["browser_download_url"].as_str().unwrap_or("");
        if name == "yatamux.exe" {
            asset_url = Some(url.to_string());
        } else if name == "checksums.txt" {
            checksum_url = Some(url.to_string());
        }
    }

    Some(ReleaseInfo {
        tag_name,
        asset_url: asset_url?,
        checksum_url: checksum_url?,
        published_at,
    })
}

/// `bytes` の SHA256 が `expected_hex` に一致するか検証する。
pub fn verify_checksum(bytes: &[u8], expected_hex: &str) -> Result<()> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    let computed = format!("{:x}", hasher.finalize());
    if computed.eq_ignore_ascii_case(expected_hex.trim()) {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "チェックサム不一致: 期待値={}, 実際={}",
            expected_hex.trim(),
            computed
        ))
    }
}

/// `checksums.txt` の内容から `filename` に対応するハッシュを抽出する。
///
/// フォーマット: `<sha256>  <filename>` （スペース2つ区切り）
pub fn extract_checksum<'a>(checksums: &'a str, filename: &str) -> Option<&'a str> {
    for line in checksums.lines() {
        let parts: Vec<&str> = line.splitn(2, "  ").collect();
        if parts.len() == 2 && parts[1].trim() == filename {
            return Some(parts[0].trim());
        }
    }
    None
}

/// 現在の実行ファイルパスから `.new` と `.bak` のパスを導出する。
///
/// 例: `yatamux.exe` → `yatamux.exe.new`, `yatamux.exe.bak`
pub fn plan_update_paths(exe: &Path) -> (PathBuf, PathBuf) {
    let name = exe.file_name().unwrap_or_default().to_string_lossy();
    let parent = exe.parent().unwrap_or(Path::new("."));
    let new_path = parent.join(format!("{}.new", name));
    let bak_path = parent.join(format!("{}.bak", name));
    (new_path, bak_path)
}

/// アップデート後に起動する新プロセスのコマンドを構築する。
pub fn build_launch_command(exe_path: &Path) -> std::process::Command {
    let mut command = std::process::Command::new(exe_path);
    if let Ok(session) = std::env::var("YATAMUX_SESSION") {
        let session = session.trim();
        if !session.is_empty() {
            command.arg("--session").arg(session);
        }
    }
    command
}

/// 更新確認に使う Releases API URL を返す。
///
/// integration test や手動検証では `YATAMUX_UPDATE_API_URL` で差し替えられる。
pub fn release_api_url() -> String {
    std::env::var("YATAMUX_UPDATE_API_URL").unwrap_or_else(|_| DEFAULT_RELEASE_API_URL.to_string())
}

/// ダウンロード用 HTTP レスポンスのステータスを検証する。
pub fn ensure_download_success(status: reqwest::StatusCode, label: &str, url: &str) -> Result<()> {
    if status.is_success() {
        Ok(())
    } else {
        Err(anyhow::anyhow!(
            "{} のダウンロードに失敗: HTTP {} ({})",
            label,
            status,
            url
        ))
    }
}

/// Releases API から最新リリース情報を取得する。
///
/// 404 は「まだリリースがない」扱いで `Ok(None)` を返す。
pub async fn fetch_latest_release(
    client: &reqwest::Client,
    release_api_url: &str,
) -> Result<Option<ReleaseInfo>> {
    let resp = client
        .get(release_api_url)
        .send()
        .await
        .with_context(|| format!("更新 API への接続に失敗: {}", release_api_url))?;

    if resp.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    if !resp.status().is_success() {
        anyhow::bail!("更新 API エラー: {} ({})", resp.status(), release_api_url);
    }

    let json = resp
        .text()
        .await
        .with_context(|| format!("更新 API レスポンスの読み取りに失敗: {}", release_api_url))?;
    let release = parse_release_info(&json).ok_or_else(|| {
        anyhow::anyhow!("リリース情報のパースに失敗（yatamux.exe が見つかりません）")
    })?;
    Ok(Some(release))
}

/// checksums.txt と `yatamux.exe` をダウンロードして SHA256 を検証する。
pub async fn download_and_verify_release_binary(
    client: &reqwest::Client,
    release: &ReleaseInfo,
) -> Result<Vec<u8>> {
    let checksums_resp = client
        .get(&release.checksum_url)
        .send()
        .await
        .context("checksums.txt のダウンロードに失敗")?;
    ensure_download_success(
        checksums_resp.status(),
        "checksums.txt",
        &release.checksum_url,
    )?;
    let checksums_text = checksums_resp
        .text()
        .await
        .context("checksums.txt の読み取りに失敗")?;

    let binary_resp = client
        .get(&release.asset_url)
        .send()
        .await
        .context("バイナリのダウンロードに失敗")?;
    ensure_download_success(binary_resp.status(), "yatamux.exe", &release.asset_url)?;
    let binary = binary_resp
        .bytes()
        .await
        .context("バイナリの読み取りに失敗")?;

    let expected_hash = extract_checksum(&checksums_text, "yatamux.exe").ok_or_else(|| {
        anyhow::anyhow!("checksums.txt に yatamux.exe のエントリが見つかりません")
    })?;
    verify_checksum(&binary, expected_hash).context("SHA256 チェックサム検証に失敗")?;

    Ok(binary.to_vec())
}

/// `<exe>.new` の staging パスを用意する。
///
/// 以前の更新失敗で残っていた `.new` はここで削除してから再利用する。
pub fn prepare_staged_binary_path(exe: &Path) -> Result<PathBuf> {
    let (new_path, _) = plan_update_paths(exe);
    if new_path.exists() {
        std::fs::remove_file(&new_path)
            .with_context(|| format!("古い .new の削除に失敗: {}", new_path.display()))?;
    }
    Ok(new_path)
}

/// 検証済みバイナリを `<exe>.new` へ書き込む。
pub fn write_staged_binary(new_path: &Path, binary: &[u8]) -> Result<()> {
    std::fs::write(new_path, binary)
        .with_context(|| format!("バイナリの書き込みに失敗: {}", new_path.display()))
}

/// checksums とバイナリを取得・検証して `<exe>.new` へ staging する。
pub async fn download_and_stage_release_binary(
    client: &reqwest::Client,
    release: &ReleaseInfo,
    exe: &Path,
) -> Result<PathBuf> {
    let new_path = prepare_staged_binary_path(exe)?;
    let binary = download_and_verify_release_binary(client, release).await?;
    write_staged_binary(&new_path, &binary)?;
    Ok(new_path)
}

/// `<exe>` を `<exe>.bak` へ退避し、`<new_path>` を `<exe>` に置き換える。
pub fn replace_executable(exe: &Path, new_path: &Path) -> Result<PathBuf> {
    let (_, bak_path) = plan_update_paths(exe);

    if bak_path.exists() {
        std::fs::remove_file(&bak_path)
            .with_context(|| format!("古い .bak の削除に失敗: {}", bak_path.display()))?;
    }

    std::fs::rename(exe, &bak_path).with_context(|| {
        format!(
            "exe → .bak リネームに失敗: {} → {}",
            exe.display(),
            bak_path.display()
        )
    })?;

    std::fs::rename(new_path, exe).with_context(|| {
        format!(
            ".new → exe リネームに失敗: {} → {}",
            new_path.display(),
            exe.display()
        )
    })?;

    Ok(bak_path)
}

/// 指定 PID の終了を待つ。
///
/// `OpenProcess` に失敗した場合は、すでに終了済みとみなして成功扱いにする。
pub fn wait_for_process_exit(pid: u32, timeout: Duration) -> Result<()> {
    if pid == 0 {
        return Ok(());
    }

    #[cfg(windows)]
    {
        use windows::Win32::Foundation::{CloseHandle, WAIT_FAILED, WAIT_OBJECT_0, WAIT_TIMEOUT};
        use windows::Win32::System::Threading::{
            OpenProcess, WaitForSingleObject, PROCESS_SYNCHRONIZE,
        };

        let timeout_ms = timeout.as_millis().min(u32::MAX as u128) as u32;
        match unsafe { OpenProcess(PROCESS_SYNCHRONIZE, false, pid) } {
            Ok(handle) => {
                let result = unsafe { WaitForSingleObject(handle, timeout_ms) };
                unsafe {
                    let _ = CloseHandle(handle);
                }
                match result {
                    WAIT_OBJECT_0 => Ok(()),
                    WAIT_TIMEOUT => {
                        anyhow::bail!("プロセス {} の終了待機がタイムアウトしました", pid)
                    }
                    WAIT_FAILED => {
                        anyhow::bail!("プロセス {} の終了待機に失敗しました", pid)
                    }
                    other => anyhow::bail!(
                        "プロセス {} の終了待機で予期しない結果を受け取りました: {:?}",
                        pid,
                        other
                    ),
                }
            }
            Err(_) => Ok(()),
        }
    }

    #[cfg(not(windows))]
    {
        let start = std::time::Instant::now();
        while start.elapsed() < timeout {
            if !std::path::Path::new(&format!("/proc/{}", pid)).exists() {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        anyhow::bail!("プロセス {} の終了待機がタイムアウトしました", pid)
    }
}

/// staging 済みバイナリを本体へ反映し、必要なら新インスタンスを起動する。
pub fn apply_staged_update(
    exe: &Path,
    pid: u32,
    new_path: &Path,
    launch: bool,
    wait_timeout: Duration,
) -> Result<()> {
    wait_for_process_exit(pid, wait_timeout)?;
    replace_executable(exe, new_path)?;

    if launch {
        build_launch_command(exe)
            .spawn()
            .context("新しい yatamux の起動に失敗")?;
    }

    Ok(())
}

// ── テスト ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
    use std::process::{Command, Stdio};
    use std::thread;
    use std::time::{SystemTime, UNIX_EPOCH};

    struct MockHttpResponse {
        path: &'static str,
        status_line: &'static str,
        content_type: &'static str,
        body: Vec<u8>,
    }

    fn spawn_mock_http_server<F>(build_responses: F) -> (String, thread::JoinHandle<()>)
    where
        F: FnOnce(&str) -> Vec<MockHttpResponse>,
    {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock http server");
        listener
            .set_nonblocking(true)
            .expect("set mock http listener nonblocking");
        let addr = listener.local_addr().expect("resolve mock server address");
        let base_url = format!("http://{}", addr);
        let responses = build_responses(&base_url);

        let handle = thread::spawn(move || {
            for response in responses {
                let deadline = std::time::Instant::now() + Duration::from_secs(90);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(stream) => break stream,
                        Err(err) if err.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "timeout waiting for mock HTTP request for {}",
                                response.path
                            );
                            thread::sleep(Duration::from_millis(50));
                        }
                        Err(err) => panic!("accept test connection: {}", err),
                    }
                };
                let mut buf = [0u8; 4096];
                let size = stream.read(&mut buf).expect("read request");
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
                stream.write_all(headers.as_bytes()).expect("write headers");
                stream.write_all(&response.body).expect("write body");
            }
        });

        (base_url, handle)
    }

    fn make_temp_test_dir(prefix: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before unix epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "yatamux-{}-{}-{}",
            prefix,
            std::process::id(),
            unique
        ));
        std::fs::create_dir_all(&dir).expect("create temp test dir");
        dir
    }

    // TC-01: バージョン比較ロジック
    #[test]
    fn test_need_update_newer() {
        assert!(need_update("0.1.0", "0.2.0"));
    }

    #[test]
    fn test_need_update_same_version() {
        assert!(!need_update("0.1.0", "0.1.0"));
    }

    #[test]
    fn test_need_update_older_version() {
        assert!(!need_update("0.1.0", "0.0.9"));
    }

    #[test]
    fn test_need_update_prerelease_skipped() {
        // prerelease は更新対象外
        assert!(!need_update("0.1.0", "0.2.0-beta.1"));
    }

    #[test]
    fn test_need_update_with_v_prefix() {
        assert!(need_update("0.1.0", "v0.2.0"));
    }

    #[test]
    fn test_need_update_minor_increment() {
        assert!(need_update("0.1.0", "0.1.1"));
    }

    // TC-02: GitHub Releases JSON パース
    #[test]
    fn test_parse_release_info_normal() {
        let json = r#"{
            "tag_name": "v0.2.0",
            "published_at": "2026-04-05T09:00:00Z",
            "assets": [
                {
                    "name": "yatamux.exe",
                    "browser_download_url": "https://example.com/yatamux.exe"
                },
                {
                    "name": "checksums.txt",
                    "browser_download_url": "https://example.com/checksums.txt"
                }
            ]
        }"#;
        let info = parse_release_info(json).expect("パースに成功すること");
        assert_eq!(info.tag_name, "v0.2.0");
        assert_eq!(info.asset_url, "https://example.com/yatamux.exe");
        assert_eq!(info.checksum_url, "https://example.com/checksums.txt");
        assert_eq!(info.published_at.as_deref(), Some("2026-04-05T09:00:00Z"));
    }

    #[test]
    fn test_parse_release_info_no_assets() {
        let json = r#"{"tag_name": "v0.2.0", "assets": []}"#;
        assert!(parse_release_info(json).is_none());
    }

    #[test]
    fn test_parse_release_info_missing_exe() {
        let json = r#"{
            "tag_name": "v0.2.0",
            "assets": [
                {
                    "name": "checksums.txt",
                    "browser_download_url": "https://example.com/checksums.txt"
                }
            ]
        }"#;
        // yatamux.exe がないので None
        assert!(parse_release_info(json).is_none());
    }

    #[test]
    fn test_parse_release_info_missing_checksums() {
        let json = r#"{
            "tag_name": "v0.2.0",
            "assets": [
                {
                    "name": "yatamux.exe",
                    "browser_download_url": "https://example.com/yatamux.exe"
                }
            ]
        }"#;
        // checksums.txt がないので None
        assert!(parse_release_info(json).is_none());
    }

    // TC-03: SHA256 検証
    #[test]
    fn test_verify_checksum_correct() {
        // SHA256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        let bytes = b"hello";
        let expected = "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824";
        assert!(verify_checksum(bytes, expected).is_ok());
    }

    #[test]
    fn test_verify_checksum_mismatch() {
        let bytes = b"hello";
        let wrong = "0000000000000000000000000000000000000000000000000000000000000000";
        assert!(verify_checksum(bytes, wrong).is_err());
    }

    #[test]
    fn test_verify_checksum_case_insensitive() {
        let bytes = b"hello";
        let upper = "2CF24DBA5FB0A30E26E83B2AC5B9E29E1B161E5C1FA7425E73043362938B9824";
        assert!(verify_checksum(bytes, upper).is_ok());
    }

    // TC-04: ファイルパス導出
    #[test]
    fn test_plan_update_paths() {
        let exe = Path::new(r"C:\foo\yatamux.exe");
        let (new_path, bak_path) = plan_update_paths(exe);
        assert_eq!(new_path, Path::new(r"C:\foo\yatamux.exe.new"));
        assert_eq!(bak_path, Path::new(r"C:\foo\yatamux.exe.bak"));
    }

    // TC-08: 新プロセス起動コマンドの組み立て
    #[test]
    fn test_build_launch_command_exe_path() {
        unsafe {
            std::env::remove_var("YATAMUX_SESSION");
        }
        let exe = Path::new(r"C:\foo\yatamux.exe");
        let cmd = build_launch_command(exe);
        assert_eq!(cmd.get_program(), exe.as_os_str());
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(
            args.is_empty(),
            "launch command should not add args by default"
        );
    }

    #[test]
    fn test_build_launch_command_preserves_session_argument() {
        unsafe {
            std::env::set_var("YATAMUX_SESSION", "e2e-update");
        }

        let exe = Path::new(r"C:\foo\yatamux.exe");
        let cmd = build_launch_command(exe);
        let args: Vec<String> = cmd
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();

        assert_eq!(
            args,
            vec!["--session".to_string(), "e2e-update".to_string()]
        );

        unsafe {
            std::env::remove_var("YATAMUX_SESSION");
        }
    }

    // extract_checksum のテスト
    #[test]
    fn test_extract_checksum_found() {
        let checksums = "abc123  yatamux.exe\ndef456  other.zip\n";
        assert_eq!(extract_checksum(checksums, "yatamux.exe"), Some("abc123"));
    }

    #[test]
    fn test_extract_checksum_not_found() {
        let checksums = "abc123  yatamux.exe\n";
        assert!(extract_checksum(checksums, "missing.exe").is_none());
    }

    #[test]
    fn test_release_api_url_defaults_to_github() {
        unsafe {
            std::env::remove_var("YATAMUX_UPDATE_API_URL");
        }
        assert_eq!(
            release_api_url(),
            "https://api.github.com/repos/raiga0310/yatamux/releases/latest"
        );
    }

    #[test]
    fn test_release_api_url_can_be_overridden() {
        unsafe {
            std::env::set_var("YATAMUX_UPDATE_API_URL", "http://127.0.0.1:18080/latest");
        }
        assert_eq!(release_api_url(), "http://127.0.0.1:18080/latest");
        unsafe {
            std::env::remove_var("YATAMUX_UPDATE_API_URL");
        }
    }

    #[test]
    fn test_ensure_download_success_accepts_success_status() {
        assert!(ensure_download_success(
            reqwest::StatusCode::OK,
            "checksums.txt",
            "https://example.com/checksums.txt"
        )
        .is_ok());
    }

    #[test]
    fn test_ensure_download_success_reports_status_and_url() {
        let err = ensure_download_success(
            reqwest::StatusCode::NOT_FOUND,
            "checksums.txt",
            "https://example.com/checksums.txt",
        )
        .expect_err("404 should fail");
        let message = err.to_string();
        assert!(message.contains("checksums.txt"));
        assert!(message.contains("404 Not Found"));
        assert!(message.contains("https://example.com/checksums.txt"));
    }

    #[tokio::test]
    async fn test_fetch_latest_release_and_download_binary_with_mock_http() {
        let binary = b"mock yatamux binary".to_vec();
        let checksum = format!("{:x}", Sha256::digest(&binary));
        let (base_url, handle) = spawn_mock_http_server(|base_url| {
            vec![
                MockHttpResponse {
                    path: "/latest",
                    status_line: "200 OK",
                    content_type: "application/json",
                    body: format!(
                        r#"{{
                            "tag_name": "v0.1.99",
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
                    body: binary.clone(),
                },
            ]
        });

        let client = reqwest::Client::builder()
            .user_agent("yatamux-test")
            .build()
            .expect("build reqwest client");

        let release = fetch_latest_release(&client, &format!("{}/latest", base_url))
            .await
            .expect("fetch latest release")
            .expect("release should exist");
        assert_eq!(release.tag_name, "v0.1.99");
        assert_eq!(
            release.asset_url,
            format!("{}/downloads/yatamux.exe", base_url)
        );
        assert_eq!(
            release.checksum_url,
            format!("{}/downloads/checksums.txt", base_url)
        );

        let downloaded = download_and_verify_release_binary(&client, &release)
            .await
            .expect("download and verify binary");
        assert_eq!(downloaded, binary);

        handle.join().expect("join mock server thread");
    }

    #[tokio::test]
    async fn test_fetch_latest_release_returns_none_on_404() {
        let (base_url, handle) = spawn_mock_http_server(|_| {
            vec![MockHttpResponse {
                path: "/latest",
                status_line: "404 Not Found",
                content_type: "text/plain",
                body: b"not found".to_vec(),
            }]
        });

        let client = reqwest::Client::builder()
            .user_agent("yatamux-test")
            .build()
            .expect("build reqwest client");

        let release = fetch_latest_release(&client, &format!("{}/latest", base_url))
            .await
            .expect("404 should not be treated as hard error");
        assert!(release.is_none());

        handle.join().expect("join mock server thread");
    }

    #[tokio::test]
    async fn test_download_and_stage_release_binary_removes_stale_new_on_checksum_mismatch() {
        let exe_dir = make_temp_test_dir("update-stage-mismatch");
        let exe = exe_dir.join("yatamux.exe");
        let old_binary = b"existing binary".to_vec();
        let stale_new = exe_dir.join("yatamux.exe.new");
        let binary = b"mock yatamux binary".to_vec();

        std::fs::write(&exe, &old_binary).expect("write existing exe");
        std::fs::write(&stale_new, b"stale staged binary").expect("write stale .new");

        let (base_url, handle) = spawn_mock_http_server(|_base_url| {
            vec![
                MockHttpResponse {
                    path: "/downloads/checksums.txt",
                    status_line: "200 OK",
                    content_type: "text/plain",
                    body: b"deadbeef  yatamux.exe\n".to_vec(),
                },
                MockHttpResponse {
                    path: "/downloads/yatamux.exe",
                    status_line: "200 OK",
                    content_type: "application/octet-stream",
                    body: binary.clone(),
                },
            ]
        });

        let client = reqwest::Client::builder()
            .user_agent("yatamux-test")
            .build()
            .expect("build reqwest client");
        let release = ReleaseInfo {
            tag_name: "v0.1.99".to_string(),
            asset_url: format!("{}/downloads/yatamux.exe", base_url),
            checksum_url: format!("{}/downloads/checksums.txt", base_url),
            published_at: None,
        };

        let err = download_and_stage_release_binary(&client, &release, &exe)
            .await
            .expect_err("checksum mismatch should fail");
        assert!(
            err.to_string().contains("SHA256"),
            "checksum mismatch should mention SHA256: {err:#}"
        );
        assert!(
            !stale_new.exists(),
            ".new should be cleaned up before a failed staging attempt"
        );
        assert_eq!(
            std::fs::read(&exe).expect("read existing exe"),
            old_binary,
            "existing exe should remain unchanged"
        );

        handle.join().expect("join mock server thread");
        std::fs::remove_dir_all(&exe_dir).expect("cleanup temp dir");
    }

    #[test]
    fn test_replace_executable_swaps_new_file_and_overwrites_stale_backup() {
        let temp_dir = make_temp_test_dir("update-replace");
        let exe = temp_dir.join("yatamux.exe");
        let new_path = temp_dir.join("yatamux.exe.new");
        let bak_path = temp_dir.join("yatamux.exe.bak");

        std::fs::write(&exe, b"old binary").expect("write old exe");
        std::fs::write(&new_path, b"new binary").expect("write new exe");
        std::fs::write(&bak_path, b"stale backup").expect("write stale backup");

        let actual_bak = replace_executable(&exe, &new_path).expect("replace executable");
        assert_eq!(actual_bak, bak_path);
        assert_eq!(
            std::fs::read(&exe).expect("read replaced exe"),
            b"new binary"
        );
        assert_eq!(
            std::fs::read(&bak_path).expect("read backup"),
            b"old binary"
        );
        assert!(!new_path.exists(), ".new should be consumed by rename");

        std::fs::remove_dir_all(&temp_dir).expect("cleanup temp dir");
    }

    #[test]
    fn test_apply_staged_update_times_out_before_replacing_binary() {
        let temp_dir = make_temp_test_dir("update-timeout");
        let exe = temp_dir.join("yatamux.exe");
        let new_path = temp_dir.join("yatamux.exe.new");
        let bak_path = temp_dir.join("yatamux.exe.bak");

        std::fs::write(&exe, b"old binary").expect("write old exe");
        std::fs::write(&new_path, b"new binary").expect("write new exe");

        #[cfg(windows)]
        let mut child = Command::new("ping")
            .args(["127.0.0.1", "-n", "6"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn long-running child");

        #[cfg(not(windows))]
        let mut child = Command::new("sleep")
            .arg("2")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn long-running child");

        let err = apply_staged_update(
            &exe,
            child.id(),
            &new_path,
            false,
            Duration::from_millis(100),
        )
        .expect_err("live process should trigger timeout");
        assert!(
            err.to_string().contains("タイムアウト"),
            "timeout should be reported clearly: {err:#}"
        );
        assert_eq!(
            std::fs::read(&exe).expect("read exe after timeout"),
            b"old binary"
        );
        assert_eq!(
            std::fs::read(&new_path).expect("read staged binary after timeout"),
            b"new binary"
        );
        assert!(
            !bak_path.exists(),
            ".bak should not be created when process wait times out"
        );

        let _ = child.kill();
        let _ = child.wait();
        std::fs::remove_dir_all(&temp_dir).expect("cleanup temp dir");
    }
}
