//! セルフアップデート機能（C-38）
//!
//! GitHub Releases からバイナリを取得し、SHA256 を検証したあと
//! `--apply-update` ヘルパーモードでバイナリ置換を行う。
//!
//! ## テスト計画
//! `docs/test-plan-self-update.md` を参照。

use anyhow::Result;
use std::path::{Path, PathBuf};

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

// ── テスト ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

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
}
