//! CLI サブコマンド実装
//!
//! 実行中の yatamux セッションに IPC 経由で接続し、
//! `list-panes` / `send-keys` を実行する。

use anyhow::{Context, Result};
use yatamux_client::connection::ServerConnection;
use yatamux_protocol::types::PaneId;
use yatamux_protocol::{ClientMessage, ServerMessage};

/// `yatamux list-panes` — 実行中のペイン一覧を標準出力に表示する
pub async fn list_panes(session: &str) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;

    conn.tx.send(ClientMessage::ListPanes).await?;

    let panes = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::PanesListed { panes }) => return Ok(panes),
                Some(_) => continue,
                None => return Err(anyhow::anyhow!("server closed connection before sending PanesListed")),
            }
        }
    })
    .await
    .context("timeout waiting for pane list")??;

    if panes.is_empty() {
        println!("(no panes)");
    } else {
        println!("{:<6} {:<8} {:<6} {:<6} title", "pane", "surface", "cols", "rows");
        println!("{}", "-".repeat(40));
        for p in &panes {
            println!("{:<6} {:<8} {:<6} {:<6} {}", p.id.0, p.surface.0, p.cols, p.rows, p.title);
        }
    }
    Ok(())
}

/// `yatamux send-keys --pane <id> <text>` — 指定ペインにテキストを送信する
///
/// `\n` は LF (0x0A)、`\r` は CR (0x0D) として解釈する。
pub async fn send_keys(session: &str, pane_id: u32, text: &str) -> Result<()> {
    let conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;

    let data = unescape(text);
    conn.tx
        .send(ClientMessage::Input { pane: PaneId(pane_id), data })
        .await?;

    Ok(())
}

/// `\n` → LF、`\r` → CR、`\t` → TAB のエスケープ展開
fn unescape(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.peek() {
                Some('n') => { chars.next(); out.push(b'\n'); }
                Some('r') => { chars.next(); out.push(b'\r'); }
                Some('t') => { chars.next(); out.push(b'\t'); }
                Some('\\') => { chars.next(); out.push(b'\\'); }
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
    use super::unescape;

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
}
