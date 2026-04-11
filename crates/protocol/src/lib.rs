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

/// 現在のプロトコルバージョン（サーバー側）
pub const PROTOCOL_VERSION: u32 = 1;

/// サーバーが受け入れる最小クライアントバージョン
pub const MIN_CLIENT_VERSION: u32 = 1;

/// サーバーが宣言する capabilities
pub const SERVER_CAPABILITIES: &[&str] = &[
    "subscribe_pane",
    "exec",
    "capture_pane",
    "alias_role",
    "session_save",
];
pub use types::{
    CiConclusion, CiRunInfo, CiRunStatus, CursorInfo, PaneCapture, PaneId, PaneInfo,
    SplitDirection, SurfaceId, TermSize, WorkspaceId,
};

/// ワイヤーフォーマット golden fixture テスト
///
/// 外部ツール（CLI / エージェント / MCP サーバー）との互換性を保つため、
/// 主要メッセージの JSON シリアライズ形式が変わらないことをここで固定する。
/// フィールドの追加は後方互換だが、フィールド名変更・削除・型変更は壊れ変更なので
/// このテストが落ちた場合は `docs/protocol-ipc.md` のバージョンを上げること。
#[cfg(test)]
mod golden {
    use super::*;
    use crate::types::{ExecStatus, ExecWaitCondition, PaneId, SurfaceId, TermSize, WorkspaceId};

    // ── ClientMessage ──────────────────────────────────────────

    #[test]
    fn golden_list_panes() {
        let json = serde_json::to_string(&ClientMessage::ListPanes).unwrap();
        assert_eq!(json, r#"{"type":"list_panes"}"#);
    }

    #[test]
    fn golden_handshake() {
        let msg = ClientMessage::Handshake {
            protocol_version: 1,
            capabilities: vec!["exec".to_string(), "subscribe_pane".to_string()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"handshake","protocol_version":1,"capabilities":["exec","subscribe_pane"]}"#
        );
    }

    #[test]
    fn golden_handshake_no_capabilities() {
        let msg = ClientMessage::Handshake {
            protocol_version: 1,
            capabilities: vec![],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"handshake","protocol_version":1,"capabilities":[]}"#
        );
    }

    #[test]
    fn golden_exec() {
        let msg = ClientMessage::Exec {
            request_id: "req-1".to_string(),
            pane: PaneId(3),
            data: vec![99, 97, 114, 103, 111, 13],
            wait: ExecWaitCondition::OutputRegex {
                pattern: "ok".to_string(),
                lines: 100,
            },
            timeout_ms: 30000,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        // フォーマットが固定されていること（フィールド名）
        assert!(json.contains(r#""type":"exec""#));
        assert!(json.contains(r#""request_id":"req-1""#));
        assert!(json.contains(r#""pane":3"#));
        assert!(json.contains(r#""timeout_ms":30000"#));
        // デシリアライズでラウンドトリップすること
        assert!(matches!(decoded, ClientMessage::Exec { .. }));
    }

    #[test]
    fn golden_subscribe_pane() {
        let json =
            serde_json::to_string(&ClientMessage::SubscribePane { pane: PaneId(7) }).unwrap();
        assert_eq!(json, r#"{"type":"subscribe_pane","pane":7}"#);
    }

    #[test]
    fn golden_unsubscribe_pane() {
        let json =
            serde_json::to_string(&ClientMessage::UnsubscribePane { pane: PaneId(7) }).unwrap();
        assert_eq!(json, r#"{"type":"unsubscribe_pane","pane":7}"#);
    }

    #[test]
    fn golden_interrupt_pane() {
        let json =
            serde_json::to_string(&ClientMessage::InterruptPane { pane: PaneId(5) }).unwrap();
        assert_eq!(json, r#"{"type":"interrupt_pane","pane":5}"#);
    }

    #[test]
    fn golden_terminate_pane() {
        let json =
            serde_json::to_string(&ClientMessage::TerminatePane { pane: PaneId(5) }).unwrap();
        assert_eq!(json, r#"{"type":"terminate_pane","pane":5}"#);
    }

    #[test]
    fn golden_close_pane() {
        let json = serde_json::to_string(&ClientMessage::ClosePane { pane: PaneId(5) }).unwrap();
        assert_eq!(json, r#"{"type":"close_pane","pane":5}"#);
    }

    #[test]
    fn golden_capture_pane() {
        let json = serde_json::to_string(&ClientMessage::CapturePane {
            pane: PaneId(2),
            lines: 50,
            plain_text: true,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"capture_pane","pane":2,"lines":50,"plain_text":true}"#
        );
    }

    #[test]
    fn golden_capture_pane_default_plain_text() {
        // plain_text: false はデフォルト値（0 扱い）なのでシリアライズに含まれる
        let json = serde_json::to_string(&ClientMessage::CapturePane {
            pane: PaneId(2),
            lines: 50,
            plain_text: false,
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"capture_pane","pane":2,"lines":50,"plain_text":false}"#
        );
    }

    #[test]
    fn golden_set_pane_meta_with_fields() {
        let json = serde_json::to_string(&ClientMessage::SetPaneMeta {
            pane: PaneId(1),
            alias: Some("worker".to_string()),
            role: Some("executor".to_string()),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"set_pane_meta","pane":1,"alias":"worker","role":"executor"}"#
        );
    }

    #[test]
    fn golden_set_pane_meta_no_optional_fields() {
        // None フィールドは skip_serializing_if で省略される
        let json = serde_json::to_string(&ClientMessage::SetPaneMeta {
            pane: PaneId(1),
            alias: None,
            role: None,
        })
        .unwrap();
        assert_eq!(json, r#"{"type":"set_pane_meta","pane":1}"#);
    }

    #[test]
    fn golden_save_and_quit_client() {
        let json = serde_json::to_string(&ClientMessage::SaveAndQuit).unwrap();
        assert_eq!(json, r#"{"type":"save_and_quit"}"#);
    }

    #[test]
    fn golden_create_workspace_no_name() {
        let json = serde_json::to_string(&ClientMessage::CreateWorkspace { name: None }).unwrap();
        assert_eq!(json, r#"{"type":"create_workspace","name":null}"#);
    }

    #[test]
    fn golden_create_pane_without_working_dir() {
        let json = serde_json::to_string(&ClientMessage::CreatePane {
            surface: SurfaceId(1),
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
            working_dir: None,
        })
        .unwrap();
        // working_dir: None は skip_serializing_if で省略
        assert!(json.contains(r#""type":"create_pane""#));
        assert!(
            !json.contains("working_dir"),
            "working_dir=None should be omitted"
        );
    }

    #[test]
    fn golden_create_pane_with_working_dir() {
        let json = serde_json::to_string(&ClientMessage::CreatePane {
            surface: SurfaceId(1),
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
            working_dir: Some("C:/Users/test".to_string()),
        })
        .unwrap();
        assert!(json.contains(r#""working_dir":"C:/Users/test""#));
    }

    // ── ServerMessage ──────────────────────────────────────────

    #[test]
    fn golden_handshake_accepted() {
        let msg = ServerMessage::HandshakeAccepted {
            protocol_version: 1,
            min_client_version: 1,
            capabilities: vec!["exec".to_string(), "subscribe_pane".to_string()],
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"handshake_accepted""#));
        assert!(json.contains(r#""protocol_version":1"#));
        assert!(json.contains(r#""min_client_version":1"#));
        assert!(json.contains(r#""capabilities":"#));
    }

    #[test]
    fn golden_error() {
        let msg = ServerMessage::Error {
            message: "pane 99 not found".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"error","message":"pane 99 not found"}"#);
    }

    #[test]
    fn golden_pane_closed() {
        let json = serde_json::to_string(&ServerMessage::PaneClosed { pane: PaneId(4) }).unwrap();
        assert_eq!(json, r#"{"type":"pane_closed","pane":4}"#);
    }

    #[test]
    fn golden_exec_result_completed() {
        let msg = ServerMessage::ExecResult {
            request_id: "req-1".to_string(),
            pane: PaneId(3),
            status: ExecStatus::Completed,
            exit_code: Some(0),
            message: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""type":"exec_result""#));
        assert!(json.contains(r#""request_id":"req-1""#));
        assert!(json.contains(r#""status":"completed""#));
        assert!(json.contains(r#""exit_code":0"#));
        assert!(
            !json.contains("\"message\""),
            "message=None should be omitted"
        );
    }

    #[test]
    fn golden_exec_result_timed_out() {
        let msg = ServerMessage::ExecResult {
            request_id: "req-2".to_string(),
            pane: PaneId(3),
            status: ExecStatus::TimedOut,
            exit_code: None,
            message: Some("timeout".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains(r#""status":"timed_out""#));
        assert!(
            !json.contains("\"exit_code\""),
            "exit_code=None should be omitted"
        );
        assert!(json.contains(r#""message":"timeout""#));
    }

    #[test]
    fn golden_notification() {
        let json = serde_json::to_string(&ServerMessage::Notification {
            pane: PaneId(1),
            body: "Process exited".to_string(),
        })
        .unwrap();
        assert_eq!(
            json,
            r#"{"type":"notification","pane":1,"body":"Process exited"}"#
        );
    }

    #[test]
    fn golden_pane_meta_updated_with_fields() {
        let msg = ServerMessage::PaneMetaUpdated {
            pane: PaneId(1),
            alias: Some("worker".to_string()),
            role: Some("executor".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"pane_meta_updated","pane":1,"alias":"worker","role":"executor"}"#
        );
    }

    #[test]
    fn golden_pane_meta_updated_no_optional_fields() {
        let msg = ServerMessage::PaneMetaUpdated {
            pane: PaneId(1),
            alias: None,
            role: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"pane_meta_updated","pane":1}"#);
    }

    #[test]
    fn golden_workspace_created() {
        let msg = ServerMessage::WorkspaceCreated {
            id: WorkspaceId(1),
            name: "default".to_string(),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(
            json,
            r#"{"type":"workspace_created","id":1,"name":"default"}"#
        );
    }

    #[test]
    fn golden_surface_created() {
        let msg = ServerMessage::SurfaceCreated {
            id: SurfaceId(1),
            workspace: WorkspaceId(1),
        };
        let json = serde_json::to_string(&msg).unwrap();
        assert_eq!(json, r#"{"type":"surface_created","id":1,"workspace":1}"#);
    }

    #[test]
    fn golden_pane_created_minimal() {
        let msg = ServerMessage::PaneCreated {
            id: PaneId(1),
            surface: SurfaceId(1),
            split_from: None,
            direction: None,
        };
        let json = serde_json::to_string(&msg).unwrap();
        // split_from / direction: None は省略
        assert_eq!(json, r#"{"type":"pane_created","id":1,"surface":1}"#);
    }

    // ── デシリアライズ後方互換性 ──────────────────────────────────

    /// 旧サーバーが知らないフィールドを含む新メッセージを受け取っても
    /// デシリアライズがパニックしないこと（unknown fields は無視）
    #[test]
    fn golden_unknown_field_ignored_on_deserialize() {
        let json = r#"{"type":"list_panes","future_field":"value"}"#;
        // unknown フィールドは serde の #[serde(deny_unknown_fields)] がないため無視される
        let msg: ClientMessage = serde_json::from_str(json).unwrap();
        assert!(matches!(msg, ClientMessage::ListPanes));
    }

    /// 新しい type を受け取っても、パースは失敗するが panic しない
    #[test]
    fn golden_unknown_type_fails_gracefully() {
        let json = r#"{"type":"future_command","data":42}"#;
        let result: Result<ClientMessage, _> = serde_json::from_str(json);
        assert!(result.is_err(), "unknown type should return Err, not panic");
    }
}

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
                assert_eq!(panes[0].cwd, None);
                assert_eq!(panes[0].command, None);
                assert!(!panes[0].busy);
                assert_eq!(panes[0].last_output_unix_ms, None);
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
            cwd: Some("C:/Users/test".to_string()),
            command: Some("cargo".to_string()),
            busy: true,
            last_output_unix_ms: Some(1_744_000_000_000),
            active: true,
            floating: false,
            alias: None,
            role: None,
        };
        assert_eq!(info.id, PaneId(3));
        assert_eq!(info.cols, 120);
        assert_eq!(info.rows, 40);
        assert_eq!(info.title, "nvim");
        assert_eq!(info.cwd.as_deref(), Some("C:/Users/test"));
        assert_eq!(info.command.as_deref(), Some("cargo"));
        assert!(info.busy);
        assert_eq!(info.last_output_unix_ms, Some(1_744_000_000_000));
        assert!(info.active);
        assert!(!info.floating);
    }

    // TC-C13-01: CapturePane メッセージが正しくシリアライズ/デシリアライズされる
    #[test]
    fn capture_pane_message_roundtrip() {
        let msg = ClientMessage::CapturePane {
            pane: PaneId(1),
            lines: 50,
            plain_text: true,
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::CapturePane {
                pane,
                lines,
                plain_text,
            } => {
                assert_eq!(pane, PaneId(1));
                assert_eq!(lines, 50);
                assert!(plain_text);
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
            capture: Some(PaneCapture {
                title: "cmd".to_string(),
                cols: 80,
                rows: 24,
                lines_requested: 20,
                scrollback_len: 10,
                cursor: CursorInfo {
                    col: 3,
                    row: 4,
                    visible: true,
                },
                visible_text: vec!["hello".to_string(), "world".to_string()],
                scrollback_tail: vec!["prompt".to_string()],
            }),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ServerMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ServerMessage::PaneContent {
                pane,
                content,
                capture,
            } => {
                assert_eq!(pane, PaneId(2));
                assert_eq!(content, "hello\nworld");
                let capture = capture.expect("capture metadata should be present");
                assert_eq!(capture.title, "cmd");
                assert_eq!(capture.cols, 80);
                assert_eq!(capture.rows, 24);
                assert_eq!(capture.cursor.col, 3);
                assert_eq!(capture.visible_text, vec!["hello", "world"]);
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    #[test]
    fn pane_capture_roundtrip() {
        let capture = PaneCapture {
            title: "pwsh".to_string(),
            cols: 100,
            rows: 30,
            lines_requested: 50,
            scrollback_len: 12,
            cursor: CursorInfo {
                col: 8,
                row: 3,
                visible: false,
            },
            visible_text: vec!["line1".to_string(), "line2".to_string()],
            scrollback_tail: vec!["prev".to_string()],
        };

        let json = serde_json::to_string(&capture).unwrap();
        let decoded: PaneCapture = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded.title, "pwsh");
        assert_eq!(decoded.cols, 100);
        assert_eq!(decoded.rows, 30);
        assert_eq!(decoded.cursor.col, 8);
        assert_eq!(decoded.visible_text, vec!["line1", "line2"]);
        assert_eq!(decoded.scrollback_tail, vec!["prev"]);
    }

    // TC-C14-01: CreatePane { working_dir: Some(...) } がシリアライズ/デシリアライズされる
    #[test]
    fn create_pane_with_working_dir_roundtrip() {
        let msg = ClientMessage::CreatePane {
            surface: SurfaceId(1),
            split_from: None,
            direction: None,
            size: TermSize { cols: 80, rows: 24 },
            working_dir: Some("C:/Users/test".to_string()),
        };
        let json = serde_json::to_string(&msg).unwrap();
        let decoded: ClientMessage = serde_json::from_str(&json).unwrap();
        match decoded {
            ClientMessage::CreatePane { working_dir, .. } => {
                assert_eq!(working_dir, Some("C:/Users/test".to_string()));
            }
            other => panic!("unexpected: {:?}", other),
        }
    }

    // TC-C14-02: CreatePane { working_dir: None } が後方互換性を保つ
    #[test]
    fn create_pane_without_working_dir_backward_compat() {
        // 旧フォーマット（working_dir フィールドなし）
        let old_json = r#"{"type":"create_pane","surface":1,"split_from":null,"direction":null,"size":{"cols":80,"rows":24}}"#;
        let decoded: ClientMessage = serde_json::from_str(old_json).unwrap();
        match decoded {
            ClientMessage::CreatePane { working_dir, .. } => {
                assert_eq!(
                    working_dir, None,
                    "旧フォーマットでは working_dir が None になること"
                );
            }
            other => panic!("unexpected: {:?}", other),
        }
    }
}
