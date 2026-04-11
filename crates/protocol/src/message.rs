use crate::types::{
    CiRunInfo, ExecStatus, ExecWaitCondition, PaneCapture, PaneId, PaneInfo, SplitDirection,
    SurfaceId, TermSize, WorkspaceId,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;

/// `Arc<[u8]>` を `Vec<u8>` と同じワイヤーフォーマットで serde する補助モジュール
mod arc_bytes {
    use serde::{Deserialize, Deserializer, Serializer};
    use std::sync::Arc;

    pub fn serialize<S: Serializer>(data: &Arc<[u8]>, s: S) -> Result<S::Ok, S::Error> {
        s.collect_seq(data.iter())
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Arc<[u8]>, D::Error> {
        let v = Vec::<u8>::deserialize(d)?;
        Ok(Arc::from(v))
    }
}

/// クライアント → サーバー メッセージ
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// 新しいワークスペースを作成
    CreateWorkspace { name: Option<String> },

    /// ワークスペース内にサーフェス（タブ）を作成
    CreateSurface { workspace: WorkspaceId },

    /// サーフェスにペインを作成（初回 or 分割）
    CreatePane {
        surface: SurfaceId,
        split_from: Option<PaneId>,
        direction: Option<SplitDirection>,
        size: TermSize,
        /// 作業ディレクトリ（None の場合はサーバープロセスの CWD を引き継ぐ）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        working_dir: Option<String>,
    },

    /// ペインにキー入力を送信
    Input { pane: PaneId, data: Vec<u8> },

    /// コマンド送信と完了待機を 1 回の要求にまとめる
    Exec {
        request_id: String,
        pane: PaneId,
        data: Vec<u8>,
        wait: ExecWaitCondition,
        timeout_ms: u64,
    },

    /// IPC クライアントに対して、指定ペインのストリームイベント購読を開始する
    ///
    /// 既存の in-process サーバー処理では no-op で、IPC 層が解釈する。
    SubscribePane { pane: PaneId },

    /// IPC クライアントに対して、指定ペインのストリームイベント購読を解除する
    ///
    /// 既存の in-process サーバー処理では no-op で、IPC 層が解釈する。
    UnsubscribePane { pane: PaneId },

    /// ペインをリサイズ
    Resize { pane: PaneId, size: TermSize },

    /// ペインを閉じる
    ClosePane { pane: PaneId },

    /// ペインに Ctrl+C を送って割り込む
    InterruptPane { pane: PaneId },

    /// ペインの子プロセスを強制終了する
    TerminatePane { pane: PaneId },

    /// ペインの alias / role メタデータを更新する
    SetPaneMeta {
        pane: PaneId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },

    /// GUI 側の active / floating 状態を server に同期する
    SyncPaneState {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        active_pane: Option<PaneId>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        floating_pane: Option<PaneId>,
    },

    /// スクリーンダンプを要求（接続時の初期状態取得）
    RequestScreen { pane: PaneId },

    /// セッションをデタッチ（サーバーは継続）
    Detach,

    /// 全ペインの情報一覧を要求
    ListPanes,

    /// ペインの内容（スクロールバック末尾 N 行 + 現在画面）を要求
    CapturePane {
        pane: PaneId,
        lines: usize,
        /// true のとき ANSI エスケープを除去してプレーンテキストで返す（デフォルト: false）
        #[serde(default)]
        plain_text: bool,
    },

    /// セッションを保存してから終了する（セルフアップデート用）
    SaveAndQuit,

    /// 全ペインで実際に動いている子プロセス名を問い合わせる
    QueryAllPaneProcesses,

    /// IPC 接続確立直後に送るプロトコルハンドシェイク要求
    ///
    /// クライアントは接続後すぐにこのメッセージを送信する。
    /// 旧サーバーは未知メッセージとして warn して継続（後方互換）。
    Handshake {
        /// クライアントのプロトコルバージョン
        protocol_version: u32,
        /// クライアントがサポートする capabilities
        #[serde(default)]
        capabilities: Vec<String>,
    },

    /// 現在の CI ステータスを問い合わせる
    ///
    /// サーバーは最後に取得した `CiRunInfo` を `ServerMessage::CiStatus` で返す。
    /// CI 設定がない場合は `ServerMessage::CiStatus { info: None }` が返る。
    QueryCiStatus,
}

/// サーバー → クライアント メッセージ
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// ワークスペース作成完了
    WorkspaceCreated { id: WorkspaceId, name: String },

    /// サーフェス作成完了
    SurfaceCreated {
        id: SurfaceId,
        workspace: WorkspaceId,
    },

    /// ペイン作成完了
    PaneCreated {
        id: PaneId,
        surface: SurfaceId,
        /// 分割元ペイン ID（IPC 経由の CreatePane で設定される）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        split_from: Option<PaneId>,
        /// 分割方向（IPC 経由の CreatePane で設定される）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        direction: Option<SplitDirection>,
    },

    /// ペインからの出力データ（VT シーケンス）
    Output {
        pane: PaneId,
        #[serde(with = "arc_bytes")]
        data: Arc<[u8]>,
    },

    /// ペインのタイトル変更（OSC 2）
    TitleChanged { pane: PaneId, title: String },

    /// OSC 通知（9/99/777）
    Notification { pane: PaneId, body: String },

    /// OSC 52 クリップボード書き込み要求
    ClipboardWrite { pane: PaneId, data: Vec<u8> },

    /// ペインが終了
    PaneClosed { pane: PaneId },

    /// 入力が対象ペインのコマンドチャネルへ受理された
    InputAccepted { pane: PaneId },

    /// OSC 133;D — シェルコマンド終了通知（`send-keys --wait-for-prompt` で利用）
    CommandFinished {
        pane: PaneId,
        /// シェルが報告した終了コード（D;{code} 形式の場合のみ Some）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
    },

    /// Exec への応答
    ExecResult {
        request_id: String,
        pane: PaneId,
        status: ExecStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },

    /// エラー
    Error { message: String },

    /// ListPanes への応答
    PanesListed { panes: Vec<PaneInfo> },

    /// CapturePane への応答
    PaneContent {
        pane: PaneId,
        content: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        capture: Option<PaneCapture>,
    },

    /// SaveAndQuit の通知（IPC 経由で SaveAndQuit を受信したときにブリッジへ転送）
    SaveAndQuit,

    /// ペインの alias / role が更新された
    PaneMetaUpdated {
        pane: PaneId,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        alias: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        role: Option<String>,
    },

    /// QueryAllPaneProcesses への応答。各ペインで動いているコマンド名と作業ディレクトリ（None = 不明）
    ///
    /// JSON シリアライズで HashMap のキーは文字列になるため、
    /// ペイン ID の raw 値（u32）を文字列キーとして使う。
    AllPaneProcesses {
        commands: HashMap<String, Option<String>>,
        /// 各ペインの現在の作業ディレクトリ（None = 取得不可）
        #[serde(default)]
        cwds: HashMap<String, Option<String>>,
    },

    /// ClientMessage::Handshake への応答
    ///
    /// サーバーのプロトコルバージョンと capabilities を伝える。
    HandshakeAccepted {
        /// サーバーのプロトコルバージョン
        protocol_version: u32,
        /// サーバーがサポートする minimum client version
        min_client_version: u32,
        /// サーバーの capabilities
        #[serde(default)]
        capabilities: Vec<String>,
    },

    /// CI ステータス通知
    ///
    /// - `QueryCiStatus` への応答として送られる
    /// - CI ポーラーがステータスを更新するたびにブロードキャストされる
    ///
    /// `info` が `None` の場合は CI 設定なし、またはまだ取得前。
    CiStatus {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        info: Option<CiRunInfo>,
    },
}

// ── テスト ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // TC-06: SaveAndQuit の serde ラウンドトリップ
    #[test]
    fn test_save_and_quit_client_message_roundtrip() {
        let msg = ClientMessage::SaveAndQuit;
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ClientMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        assert!(matches!(restored, ClientMessage::SaveAndQuit));
    }

    #[test]
    fn test_save_and_quit_server_message_roundtrip() {
        let msg = ServerMessage::SaveAndQuit;
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ServerMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        assert!(matches!(restored, ServerMessage::SaveAndQuit));
    }

    #[test]
    fn test_interrupt_pane_roundtrip() {
        let msg = ClientMessage::InterruptPane {
            pane: crate::types::PaneId(7),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ClientMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ClientMessage::InterruptPane { pane } => {
                assert_eq!(pane, crate::types::PaneId(7));
            }
            _ => panic!("期待する variant でない"),
        }
    }

    #[test]
    fn test_terminate_pane_roundtrip() {
        let msg = ClientMessage::TerminatePane {
            pane: crate::types::PaneId(8),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ClientMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ClientMessage::TerminatePane { pane } => {
                assert_eq!(pane, crate::types::PaneId(8));
            }
            _ => panic!("期待する variant でない"),
        }
    }

    #[test]
    fn test_exec_client_message_roundtrip() {
        let msg = ClientMessage::Exec {
            request_id: "req-1".to_string(),
            pane: crate::types::PaneId(5),
            data: b"cargo test\r".to_vec(),
            wait: crate::types::ExecWaitCondition::OutputRegex {
                pattern: "test result: ok".to_string(),
                lines: 200,
            },
            timeout_ms: 30_000,
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ClientMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ClientMessage::Exec {
                request_id,
                pane,
                wait,
                timeout_ms,
                ..
            } => {
                assert_eq!(request_id, "req-1");
                assert_eq!(pane, crate::types::PaneId(5));
                assert_eq!(timeout_ms, 30_000);
                assert_eq!(
                    wait,
                    crate::types::ExecWaitCondition::OutputRegex {
                        pattern: "test result: ok".to_string(),
                        lines: 200,
                    }
                );
            }
            _ => panic!("期待する variant でない"),
        }
    }

    #[test]
    fn test_sync_pane_state_roundtrip() {
        let msg = ClientMessage::SyncPaneState {
            active_pane: Some(crate::types::PaneId(3)),
            floating_pane: Some(crate::types::PaneId(9)),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ClientMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ClientMessage::SyncPaneState {
                active_pane,
                floating_pane,
            } => {
                assert_eq!(active_pane, Some(crate::types::PaneId(3)));
                assert_eq!(floating_pane, Some(crate::types::PaneId(9)));
            }
            _ => panic!("期待する variant でない"),
        }
    }

    // QueryAllPaneProcesses のラウンドトリップ
    #[test]
    fn test_query_all_pane_processes_roundtrip() {
        let msg = ClientMessage::QueryAllPaneProcesses;
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ClientMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        assert!(matches!(restored, ClientMessage::QueryAllPaneProcesses));

        let mut commands = HashMap::new();
        commands.insert("1".to_string(), Some("claude".to_string()));
        commands.insert("2".to_string(), None);
        let msg = ServerMessage::AllPaneProcesses {
            commands,
            cwds: HashMap::new(),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ServerMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ServerMessage::AllPaneProcesses { commands, .. } => {
                assert_eq!(commands.get("1"), Some(&Some("claude".to_string())));
                assert_eq!(commands.get("2"), Some(&None));
            }
            _ => panic!("期待する variant でない"),
        }
    }

    #[test]
    fn test_subscribe_pane_roundtrip() {
        let msg = ClientMessage::SubscribePane {
            pane: crate::types::PaneId(12),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ClientMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ClientMessage::SubscribePane { pane } => {
                assert_eq!(pane, crate::types::PaneId(12));
            }
            _ => panic!("期待する variant でない"),
        }
    }

    #[test]
    fn test_unsubscribe_pane_roundtrip() {
        let msg = ClientMessage::UnsubscribePane {
            pane: crate::types::PaneId(13),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ClientMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ClientMessage::UnsubscribePane { pane } => {
                assert_eq!(pane, crate::types::PaneId(13));
            }
            _ => panic!("期待する variant でない"),
        }
    }

    #[test]
    fn test_set_pane_meta_roundtrip() {
        let msg = ClientMessage::SetPaneMeta {
            pane: crate::types::PaneId(5),
            alias: Some("tests".to_string()),
            role: Some("verifier".to_string()),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ClientMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ClientMessage::SetPaneMeta { pane, alias, role } => {
                assert_eq!(pane, crate::types::PaneId(5));
                assert_eq!(alias.as_deref(), Some("tests"));
                assert_eq!(role.as_deref(), Some("verifier"));
            }
            _ => panic!("期待する variant でない"),
        }
    }

    #[test]
    fn test_pane_meta_updated_roundtrip() {
        let msg = ServerMessage::PaneMetaUpdated {
            pane: crate::types::PaneId(6),
            alias: Some("server".to_string()),
            role: Some("worker".to_string()),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ServerMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ServerMessage::PaneMetaUpdated { pane, alias, role } => {
                assert_eq!(pane, crate::types::PaneId(6));
                assert_eq!(alias.as_deref(), Some("server"));
                assert_eq!(role.as_deref(), Some("worker"));
            }
            _ => panic!("期待する variant でない"),
        }
    }

    #[test]
    fn test_input_accepted_roundtrip() {
        let msg = ServerMessage::InputAccepted {
            pane: crate::types::PaneId(7),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ServerMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ServerMessage::InputAccepted { pane } => {
                assert_eq!(pane, crate::types::PaneId(7));
            }
            _ => panic!("期待する variant でない"),
        }
    }

    #[test]
    fn test_exec_result_server_message_roundtrip() {
        let msg = ServerMessage::ExecResult {
            request_id: "req-2".to_string(),
            pane: crate::types::PaneId(6),
            status: crate::types::ExecStatus::TimedOut,
            exit_code: Some(124),
            message: Some("timeout waiting for pane 6".to_string()),
        };
        let json = serde_json::to_string(&msg).expect("シリアライズに成功すること");
        let restored: ServerMessage =
            serde_json::from_str(&json).expect("デシリアライズに成功すること");
        match restored {
            ServerMessage::ExecResult {
                request_id,
                pane,
                status,
                exit_code,
                message,
            } => {
                assert_eq!(request_id, "req-2");
                assert_eq!(pane, crate::types::PaneId(6));
                assert_eq!(status, crate::types::ExecStatus::TimedOut);
                assert_eq!(exit_code, Some(124));
                assert_eq!(message.as_deref(), Some("timeout waiting for pane 6"));
            }
            _ => panic!("期待する variant でない"),
        }
    }
}
