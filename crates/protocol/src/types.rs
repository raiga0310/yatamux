use serde::{Deserialize, Serialize};

/// ワークスペース ID (tmux の session 相当)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorkspaceId(pub u32);

/// サーフェス ID (ワークスペース内のタブ相当)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SurfaceId(pub u32);

/// ペイン ID (サーフェス内の分割ビュー)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u32);

/// ターミナルサイズ
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TermSize {
    /// 幅（列数）
    pub cols: u16,
    /// 高さ（行数）
    pub rows: u16,
}

/// ペイン分割方向
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SplitDirection {
    /// 水平分割（上下に並べる）
    Horizontal,
    /// 垂直分割（左右に並べる）
    Vertical,
}

/// ペイン情報（`list-panes` レスポンス用）
///
/// フィールドは後方互換拡張を基本方針とし、旧クライアントが知らないフィールドは
/// `#[serde(default)]` で省略または既定値を返す。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneInfo {
    /// ペイン ID
    pub id: PaneId,
    /// 所属サーフェス ID
    pub surface: SurfaceId,
    /// ペインタイトル（OSC 0/2 で設定された文字列）
    pub title: String,
    /// 幅（列数）
    pub cols: u16,
    /// 高さ（行数）
    pub rows: u16,
    /// 論理名（`set-pane-meta --alias` で設定、未設定なら省略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,
    /// 役割ラベル（`set-pane-meta --role` で設定、未設定なら省略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub role: Option<String>,
    /// 現在の作業ディレクトリ（OS プロセスメモリから取得、取得不可なら省略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    /// アクティブなコマンド名（シェルより深い孫プロセス名、なければ省略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    /// `send-keys` 送信後から OSC 133;D 受信までの間 `true`
    #[serde(default)]
    pub busy: bool,
    /// 最後に PTY 出力を受け取った時刻（Unix epoch ミリ秒）、未受信なら省略
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_output_unix_ms: Option<u64>,
    /// GUI でアクティブ（フォーカス）なペインなら `true`
    #[serde(default)]
    pub active: bool,
    /// フローティング表示中なら `true`
    #[serde(default)]
    pub floating: bool,
}

/// `capture-pane` のカーソル情報
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CursorInfo {
    /// カーソル列（0 始まり）
    pub col: u16,
    /// カーソル行（0 始まり、可視画面上の位置）
    pub row: u16,
    /// カーソルが表示状態なら `true`（`DECTCEM` で制御）
    pub visible: bool,
}

/// `capture-pane --json` の構造化レスポンス
///
/// `visible_text` と `scrollback_tail` はそれぞれ改行なしの行文字列の配列。
/// CJK 全角文字は 1 要素内に格納されるが、表示幅は `cols` 基準。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneCapture {
    /// ペインタイトル
    pub title: String,
    /// 幅（列数）
    pub cols: u16,
    /// 高さ（行数）
    pub rows: u16,
    /// リクエストされたスクロールバック行数上限
    pub lines_requested: usize,
    /// 現在のスクロールバック行数（`lines_requested` 以下）
    pub scrollback_len: usize,
    /// カーソル情報
    pub cursor: CursorInfo,
    /// 可視画面の行テキスト（先頭 = 最上行、末尾 = 最下行）
    pub visible_text: Vec<String>,
    /// スクロールバック末尾の行テキスト（先頭 = 最古行、末尾 = 最新行）
    pub scrollback_tail: Vec<String>,
}

/// GitHub Actions CI ランの状態
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiConclusion {
    /// 成功
    Success,
    /// 失敗
    Failure,
    /// キャンセル
    Cancelled,
    /// スキップ
    Skipped,
    /// 不明 / 未対応の値
    Unknown,
}

/// GitHub Actions CI ランの進行状態
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CiRunStatus {
    /// キュー待ち / 開始待ち
    Queued,
    /// 実行中
    InProgress,
    /// 完了（conclusion を参照）
    Completed,
}

/// GitHub Actions の最新ワークフローラン情報
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CiRunInfo {
    /// リポジトリ（`owner/repo` 形式）
    pub repo: String,
    /// ワークフロー名
    pub name: String,
    /// ランの進行状態
    pub status: CiRunStatus,
    /// 完了時の結果（実行中は `None`）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conclusion: Option<CiConclusion>,
    /// ブランチ名（未取得なら省略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub branch: Option<String>,
    /// コミット SHA（先頭 7 文字、未取得なら省略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub head_sha: Option<String>,
    /// GitHub の run URL（未取得なら省略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub html_url: Option<String>,
    /// 最終更新時刻（ISO 8601、未取得なら省略）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub updated_at: Option<String>,
}

impl CiRunInfo {
    /// ステータスバー表示用の短い文字列を返す
    ///
    /// - 実行中: `CI⟳`
    /// - 成功: `CI✓`
    /// - 失敗: `CI✗`
    /// - キャンセル: `CI○`
    /// - その他: `CI?`
    pub fn status_label(&self) -> &'static str {
        match self.status {
            CiRunStatus::Queued | CiRunStatus::InProgress => "CI⟳",
            CiRunStatus::Completed => match self.conclusion {
                Some(CiConclusion::Success) | Some(CiConclusion::Skipped) => "CI✓",
                Some(CiConclusion::Failure) => "CI✗",
                Some(CiConclusion::Cancelled) => "CI○",
                _ => "CI?",
            },
        }
    }
}

/// `exec` リクエストで使う待機条件
///
/// IPC ワイヤー形式は `{ "kind": "exit" }` / `{ "kind": "silence", "silence_ms": 2000 }` /
/// `{ "kind": "output_regex", "pattern": "...", "lines": 300 }` のタグ付き JSON。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ExecWaitCondition {
    /// 子プロセス終了まで待機する
    Exit,
    /// 指定ミリ秒間、PTY 出力が途絶えるまで待機する
    Silence {
        /// 無音と判定するまでの待機時間（ミリ秒）
        silence_ms: u64,
    },
    /// 直近 `lines` 行のテキストが `pattern` にマッチするまで待機する
    OutputRegex {
        /// マッチさせる正規表現（Rust `regex` クレート構文）
        pattern: String,
        /// マッチ対象とするスクロールバック末尾行数
        lines: usize,
    },
}

/// `exec` 実行結果の状態
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExecStatus {
    /// 待機条件が満たされた（正常完了）
    Completed,
    /// タイムアウトに達した
    TimedOut,
    /// 待機中にペインが閉じられた
    PaneClosed,
    /// 内部エラーが発生した
    Error,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pane_info_roundtrip_preserves_active_and_floating() {
        let info = PaneInfo {
            id: PaneId(1),
            surface: SurfaceId(2),
            title: "shell".to_string(),
            cols: 80,
            rows: 24,
            alias: Some("tests".to_string()),
            role: Some("verifier".to_string()),
            cwd: Some("C:\\Users".to_string()),
            command: Some("pwsh".to_string()),
            busy: true,
            last_output_unix_ms: Some(1234),
            active: true,
            floating: true,
        };
        let json = serde_json::to_string(&info).expect("serialize PaneInfo");
        let restored: PaneInfo = serde_json::from_str(&json).expect("deserialize PaneInfo");
        assert_eq!(restored, info);
    }

    #[test]
    fn pane_info_old_json_defaults_active_and_floating_to_false() {
        let json = r#"{
            "id": 1,
            "surface": 2,
            "title": "shell",
            "cols": 80,
            "rows": 24
        }"#;
        let restored: PaneInfo = serde_json::from_str(json).expect("deserialize legacy PaneInfo");
        assert_eq!(restored.alias, None);
        assert_eq!(restored.role, None);
        assert!(!restored.active);
        assert!(!restored.floating);
        assert!(!restored.busy);
        assert_eq!(restored.last_output_unix_ms, None);
    }

    #[test]
    fn exec_wait_condition_roundtrip_preserves_output_regex() {
        let wait = ExecWaitCondition::OutputRegex {
            pattern: "test result: ok".to_string(),
            lines: 300,
        };
        let json = serde_json::to_string(&wait).expect("serialize ExecWaitCondition");
        let restored: ExecWaitCondition =
            serde_json::from_str(&json).expect("deserialize ExecWaitCondition");
        assert_eq!(restored, wait);
    }

    #[test]
    fn exec_status_roundtrip_preserves_timed_out() {
        let status = ExecStatus::TimedOut;
        let json = serde_json::to_string(&status).expect("serialize ExecStatus");
        let restored: ExecStatus = serde_json::from_str(&json).expect("deserialize ExecStatus");
        assert_eq!(restored, status);
    }
}
