use super::LayoutNode;

/// ランチャープレビュー用レイアウトデータ
#[derive(Clone, Debug)]
pub struct LayoutPreview {
    /// ペイン分割ツリー（PaneId は 0, 1, 2, … の連番）
    pub node: LayoutNode,
    /// PaneId(i) のペインに送信するコマンド文字列（None = コマンドなし）
    pub commands: Vec<Option<String>>,
}

/// レイアウトランチャーの表示状態
#[derive(Clone, Debug)]
pub struct LauncherState {
    /// (名前, プレビューデータ) のリスト
    pub entries: Vec<(String, Option<LayoutPreview>)>,
    /// 現在選択中のインデックス
    pub selected: usize,
}

impl LauncherState {
    pub fn new(entries: Vec<(String, Option<LayoutPreview>)>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }

    /// 選択中のレイアウト名を返す
    pub fn selected_name(&self) -> Option<&str> {
        self.entries.get(self.selected).map(|(n, _)| n.as_str())
    }

    /// 選択中のプレビューデータを返す
    pub fn selected_preview(&self) -> Option<&LayoutPreview> {
        self.entries.get(self.selected)?.1.as_ref()
    }
}

/// テーマランチャーの表示状態
#[derive(Clone, Debug)]
pub struct ThemeLauncherState {
    /// テーマ名のリスト（ファイル名のステム）
    pub entries: Vec<String>,
    /// 現在選択中のインデックス
    pub selected: usize,
}

impl ThemeLauncherState {
    pub fn new(entries: Vec<String>) -> Self {
        Self {
            entries,
            selected: 0,
        }
    }

    pub fn selected_name(&self) -> Option<&str> {
        self.entries.get(self.selected).map(String::as_str)
    }
}
