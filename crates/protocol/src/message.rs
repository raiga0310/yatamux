use crate::types::{
    PaneCapture, PaneId, PaneInfo, SplitDirection, SurfaceId, TermSize, WorkspaceId,
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

    /// ペインをリサイズ
    Resize { pane: PaneId, size: TermSize },

    /// ペインを閉じる
    ClosePane { pane: PaneId },

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

    /// OSC 133;D — シェルコマンド終了通知（`send-keys --wait-for-prompt` で利用）
    CommandFinished {
        pane: PaneId,
        /// シェルが報告した終了コード（D;{code} 形式の場合のみ Some）
        #[serde(default, skip_serializing_if = "Option::is_none")]
        exit_code: Option<i32>,
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
}
