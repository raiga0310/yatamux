//! # yatamux-protocol — クライアント／サーバー間メッセージ型
//!
//! yatamux の内部 IPC プロトコルを定義するクレート。
//! クライアント（Win32 ウィンドウ）とサーバー（PTY 管理）が
//! [`tokio::sync::mpsc`] チャネルまたは Windows 名前付きパイプ経由でやり取りするメッセージを定義する。
//!
//! ## メッセージフロー
//!
//! ```text
//! Client                               Server
//!   │── CreateWorkspace ─────────────────▶│
//!   │◀─ WorkspaceCreated ─────────────────│
//!   │── CreateSurface ───────────────────▶│
//!   │◀─ SurfaceCreated ───────────────────│
//!   │── CreatePane ─────────────────────▶│
//!   │◀─ PaneCreated ──────────────────────│
//!   │── Input / Resize ────────────────▶│
//!   │◀─ Output / TitleChanged ────────────│
//! ```
//!
//! ## 階層モデル
//!
//! tmux との対応関係:
//!
//! | yatamux | tmux 相当 | 説明 |
//! |----------|-----------|------|
//! | [`WorkspaceId`] | session | 最上位コンテナ |
//! | [`SurfaceId`]   | window  | タブに相当 |
//! | [`PaneId`]      | pane    | 実際の PTY を持つ単位 |

pub mod message;
pub mod types;

pub use message::{ClientMessage, ServerMessage};
pub use types::{PaneId, PaneInfo, SplitDirection, SurfaceId, TermSize, WorkspaceId};

#[cfg(test)]
mod tests {
    use super::*;

    // P-1: ListPanes が {"type":"list_panes"} にシリアライズされる
    #[test]
    fn list_panes_message_serializes() {
        let msg = ClientMessage::ListPanes;
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"list_panes"}"#);
    }

    // P-2: PanesListed が正しくデシリアライズされる
    #[test]
    fn panes_listed_message_deserializes() {
        let json = r#"{"type":"panes_listed","panes":[{"id":1,"surface":1,"title":"bash","cols":80,"rows":24}]}"#;
        let msg: ServerMessage = serde_json::from_str(json).unwrap();
        match msg {
            ServerMessage::PanesListed { panes } => {
                assert_eq!(panes.len(), 1);
                assert_eq!(panes[0].title, "bash");
                assert_eq!(panes[0].cols, 80);
                assert_eq!(panes[0].rows, 24);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // P-3: PaneInfo のフィールドが保持される
    #[test]
    fn pane_info_has_required_fields() {
        let info = PaneInfo {
            id: PaneId(3),
            surface: SurfaceId(1),
            title: "nvim".to_string(),
            cols: 120,
            rows: 40,
        };
        assert_eq!(info.id, PaneId(3));
        assert_eq!(info.cols, 120);
        assert_eq!(info.rows, 40);
        assert_eq!(info.title, "nvim");
    }

    // TC-C13-01: CapturePane メッセージが正しくシリアライズ/デシリアライズされる
    #[test]
    fn capture_pane_message_roundtrip() {
        let msg = ClientMessage::CapturePane {
            pane: PaneId(1),
            lines: 50,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::CapturePane { pane, lines } => {
                assert_eq!(pane, PaneId(1));
                assert_eq!(lines, 50);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // TC-C13-02: PaneContent メッセージが正しくシリアライズ/デシリアライズされる
    #[test]
    fn pane_content_message_roundtrip() {
        let msg = ServerMessage::PaneContent {
            pane: PaneId(2),
            content: "hello\nworld".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::PaneContent { pane, content } => {
                assert_eq!(pane, PaneId(2));
                assert_eq!(content, "hello\nworld");
            }
            other => panic!("unexpected: {:?}", other),
        }
    }
}
