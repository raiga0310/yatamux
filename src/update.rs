//! セルフアップデート機能（C-38）
//!
//! GitHub Releases からバイナリを取得し、SHA256 を検証したあと
//! `--apply-update` ヘルパーモードでバイナリ置換を行う。
//!
//! ## テスト計画
//! `docs/test-plan-self-update.md` を参照。

use anyhow::{Context, Result};
use std::path::{Path, PathBuf};

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
    std::process::Command::new(exe_path)
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

// ── テスト ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::PathBuf;
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
        let addr = listener.local_addr().expect("resolve mock server address");
        let base_url = format!("http://{}", addr);
        let responses = build_responses(&base_url);

        let handle = thread::spawn(move || {
            for response in responses {
                let (mut stream, _) = listener.accept().expect("accept test connection");
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
        let exe = Path::new(r"C:\foo\yatamux.exe");
        let cmd = build_launch_command(exe);
        assert_eq!(cmd.get_program(), exe.as_os_str());
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
}
