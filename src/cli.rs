//! CLI サブコマンド実装
//!
//! 実行中の yatamux セッションに IPC 経由で接続し、
//! `list-panes` / `send-keys` を実行する。

use anyhow::{Context, Result};
use yatamux_client::connection::ServerConnection;
use yatamux_protocol::types::{PaneId, SplitDirection, SurfaceId};
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
                None => {
                    return Err(anyhow::anyhow!(
                        "server closed connection before sending PanesListed"
                    ))
                }
            }
        }
    })
    .await
    .context("timeout waiting for pane list")??;

    if panes.is_empty() {
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

/// `yatamux capture-pane --target <id> --lines <n>` — ペインの内容を表示する
///
/// スクロールバック末尾 N 行 + 現在画面の内容を標準出力に表示する。
pub async fn capture_pane(session: &str, pane_id: u32, lines: usize) -> Result<()> {
    let mut conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;

    conn.tx
        .send(ClientMessage::CapturePane {
            pane: PaneId(pane_id),
            lines,
        })
        .await?;

    let content = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::PaneContent { content, .. }) => return Ok(content),
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

    print!("{}", content);
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
    conn.tx.send(ClientMessage::ListPanes).await?;
    let panes = tokio::time::timeout(std::time::Duration::from_secs(5), async {
        loop {
            match conn.rx.recv().await {
                Some(ServerMessage::PanesListed { panes }) => return Ok(panes),
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
    .context("timeout waiting for pane list")??;

    // 対象ペインを探す。見つからない場合は最初のペインにフォールバック
    let target_pane = panes
        .iter()
        .find(|p| p.id == PaneId(pane_id))
        .or_else(|| panes.first());

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

/// `yatamux send-keys --pane <id> [--enter] [--raw] <text>` — 指定ペインにテキストを送信する
///
/// - `--enter`: 末尾に CR (0x0D) を自動付加する。コマンド実行に使用。
/// - `--raw`: エスケープ変換を無効化してテキストをそのまま送信する。Windows パスに使用。
/// - デフォルト（オプションなし）: `\n`=LF、`\r`=CR、`\t`=TAB、`\\`=バックスラッシュ に変換。
pub async fn send_keys(
    session: &str,
    pane_id: u32,
    text: &str,
    enter: bool,
    raw: bool,
) -> Result<()> {
    let conn = ServerConnection::connect(session)
        .await
        .context("yatamux is not running (could not connect to IPC pipe)")?;

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
