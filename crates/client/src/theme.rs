/// 外観テーマ設定（プラットフォーム非依存）
///
/// `AppConfig::appearance` から構築し、`run_window()` に渡す。
/// 色値は `0xRRGGBB` 形式の `u32`。`None` はデフォルト値を意味する。
#[derive(Debug, Clone, Default)]
pub struct Theme {
    /// 背景色（`0xRRGGBB`）
    pub bg: Option<u32>,
    /// 前景色
    pub fg: Option<u32>,
    /// カーソル色
    pub cursor: Option<u32>,
    /// テキスト選択背景色
    pub selection_bg: Option<u32>,
    /// ステータスバー背景色
    pub status_bar_bg: Option<u32>,
    /// フォントファミリー（`None` = 候補リストから自動選択）
    pub font_family: Option<String>,
    /// フォントサイズ（pt）
    pub font_size: Option<u32>,
}
