use std::sync::Arc;
use serde::{Deserialize, Serialize};
use crate::types::{PaneId, PaneInfo, SplitDirection, SurfaceId, TermSize, WorkspaceId};

/// `Arc<[u8]>` を `Vec<u8>` と同じワイヤーフォーマットで serde する補助モジュール
mod arc_bytes {
    use std::sync::Arc;
    use serde::{Deserialize, Deserializer, Serializer};

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
}

/// サーバー → クライアント メッセージ
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// ワークスペース作成完了
    WorkspaceCreated { id: WorkspaceId, name: String },

    /// サーフェス作成完了
    SurfaceCreated { id: SurfaceId, workspace: WorkspaceId },

    /// ペイン作成完了
    PaneCreated { id: PaneId, surface: SurfaceId },

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

    /// エラー
    Error { message: String },

    /// ListPanes への応答
    PanesListed { panes: Vec<PaneInfo> },
}
