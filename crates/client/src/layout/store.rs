use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use yatamux_protocol::types::PaneId;
use yatamux_protocol::CiRunInfo;
use yatamux_terminal::Grid;

use super::{LauncherState, LayoutNode, PaneRect, ThemeLauncherState};

/// 点滅 1 回あたりの WM_TIMER ティック数（16ms × 16 ≈ 256ms per flip）
pub const ALERT_TICK_DIVISOR: u8 = 16;
/// 初期フリップ回数（ON/OFF ペア × 5 = 10 → 約 2.5 秒）
/// 偶数からスタートし even=ON, odd=OFF なので最初は ON で始まる。
pub const ALERT_FLIP_COUNT: u8 = 10;

/// アプリ内入力欄の編集状態。
///
/// 現在はレイアウト保存プロンプトで使用するが、履歴・カーソル移動・
/// Emacs 風編集キーを持つ行編集モデルとして他の入力 UI にも流用できる。
#[derive(Clone, Debug)]
pub struct PromptState {
    pub text: String,
    pub cursor: usize,
    history_index: Option<usize>,
    history_draft: Option<String>,
}

impl PromptState {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            cursor: 0,
            history_index: None,
            history_draft: None,
        }
    }

    pub fn insert_char(&mut self, ch: char) {
        self.detach_history();
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    pub fn insert_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.detach_history();
        self.text.insert_str(self.cursor, s);
        self.cursor += s.len();
    }

    pub fn move_start(&mut self) {
        self.cursor = 0;
    }

    pub fn move_end(&mut self) {
        self.cursor = self.text.len();
    }

    pub fn move_left(&mut self) {
        self.cursor = prev_char_boundary(&self.text, self.cursor);
    }

    pub fn move_right(&mut self) {
        self.cursor = next_char_boundary(&self.text, self.cursor);
    }

    pub fn backspace(&mut self) -> bool {
        if self.cursor == 0 {
            return false;
        }
        self.detach_history();
        let start = prev_char_boundary(&self.text, self.cursor);
        self.text.drain(start..self.cursor);
        self.cursor = start;
        true
    }

    pub fn kill_to_end(&mut self) -> Option<String> {
        if self.cursor >= self.text.len() {
            return None;
        }
        self.detach_history();
        Some(self.text.drain(self.cursor..).collect())
    }

    pub fn kill_to_start(&mut self) -> Option<String> {
        if self.cursor == 0 {
            return None;
        }
        self.detach_history();
        let killed: String = self.text.drain(..self.cursor).collect();
        self.cursor = 0;
        Some(killed)
    }

    pub fn kill_prev_word(&mut self) -> Option<String> {
        if self.cursor == 0 {
            return None;
        }
        self.detach_history();

        let mut start = self.cursor;
        while start > 0 {
            let prev = prev_char_boundary(&self.text, start);
            let ch = self.text[prev..start].chars().next().unwrap_or('\0');
            if !ch.is_whitespace() {
                break;
            }
            start = prev;
        }
        while start > 0 {
            let prev = prev_char_boundary(&self.text, start);
            let ch = self.text[prev..start].chars().next().unwrap_or('\0');
            if ch.is_whitespace() {
                break;
            }
            start = prev;
        }
        if start == self.cursor {
            return None;
        }
        let killed: String = self.text.drain(start..self.cursor).collect();
        self.cursor = start;
        Some(killed)
    }

    pub fn history_prev(&mut self, history: &[String]) -> bool {
        if history.is_empty() {
            return false;
        }
        match self.history_index {
            None => {
                self.history_draft = Some(self.text.clone());
                self.history_index = Some(history.len() - 1);
            }
            Some(0) => return false,
            Some(idx) => self.history_index = Some(idx - 1),
        }
        self.text = history[self.history_index.unwrap()].clone();
        self.move_end();
        true
    }

    pub fn history_next(&mut self, history: &[String]) -> bool {
        let Some(idx) = self.history_index else {
            return false;
        };

        if idx + 1 < history.len() {
            self.history_index = Some(idx + 1);
            self.text = history[idx + 1].clone();
        } else {
            self.history_index = None;
            self.text = self.history_draft.take().unwrap_or_default();
        }
        self.move_end();
        true
    }

    fn detach_history(&mut self) {
        self.history_index = None;
        self.history_draft = None;
    }
}

impl Default for PromptState {
    fn default() -> Self {
        Self::new()
    }
}

fn prev_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor == 0 {
        return 0;
    }
    text[..cursor]
        .char_indices()
        .next_back()
        .map(|(idx, _)| idx)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    cursor
        + text[cursor..]
            .chars()
            .next()
            .map(|ch| ch.len_utf8())
            .unwrap_or(0)
}

/// コピーモードのカーソルと選択状態
///
/// `cursor` はスクリーン座標（col, row）の 0-based インデックス。
/// `anchor` が `Some` の場合はビジュアル選択が有効。
#[derive(Clone, Debug)]
pub struct CopyState {
    /// カーソル位置 (col, row)（スクリーン座標、0-based）
    pub cursor: (usize, usize),
    /// 選択アンカー — None = カーソルのみ、Some = ビジュアル選択中
    pub anchor: Option<(usize, usize)>,
}

impl CopyState {
    /// 指定位置でコピーモードを初期化する
    pub fn new(col: usize, row: usize) -> Self {
        Self {
            cursor: (col, row),
            anchor: None,
        }
    }

    /// カーソルを指定方向に移動する（cols/rows でクランプ）
    pub fn move_cursor(&mut self, dcol: isize, drow: isize, cols: usize, rows: usize) {
        let new_col = (self.cursor.0 as isize + dcol)
            .max(0)
            .min(cols.saturating_sub(1) as isize) as usize;
        let new_row = (self.cursor.1 as isize + drow)
            .max(0)
            .min(rows.saturating_sub(1) as isize) as usize;
        self.cursor = (new_col, new_row);
    }

    /// ビジュアル選択のアンカーをカーソル位置に設定する（トグル）
    pub fn toggle_anchor(&mut self) {
        if self.anchor.is_some() {
            self.anchor = None;
        } else {
            self.anchor = Some(self.cursor);
        }
    }

    /// 選択範囲の (row_start, row_end) を返す（None = 選択なし）
    pub fn selection_rows(&self) -> Option<(usize, usize)> {
        let anchor = self.anchor?;
        let (_, ar) = anchor;
        let (_, cr) = self.cursor;
        Some((ar.min(cr), ar.max(cr)))
    }

    /// セル (col, row) が現在の選択範囲内かどうかを判定する
    pub fn is_selected(&self, col: usize, row: usize) -> bool {
        let anchor = match self.anchor {
            Some(a) => a,
            None => return false,
        };
        let (ac, ar) = anchor;
        let (cc, cr) = self.cursor;

        let (row_min, row_max) = (ar.min(cr), ar.max(cr));
        if row < row_min || row > row_max {
            return false;
        }

        if row_min == row_max {
            let (col_min, col_max) = (ac.min(cc), ac.max(cc));
            return col >= col_min && col <= col_max;
        }

        if row == row_min {
            let start_col = if ar < cr { ac } else { cc };
            return col >= start_col;
        }
        if row == row_max {
            let end_col = if ar < cr { cc } else { ac };
            return col <= end_col;
        }
        true
    }
}

/// Win32 スレッドが表示するトースト通知
#[derive(Clone, Debug)]
pub struct Toast {
    /// 発生元ペイン ID
    pub pane_id: PaneId,
    /// 通知メッセージ
    pub message: String,
    /// 生成からの経過ミリ秒
    pub elapsed_ms: u32,
}

impl Toast {
    /// トースト全体の表示時間（ms）
    pub const DURATION_MS: u32 = 4000;
    /// スライドインにかける時間（ms）
    pub const SLIDE_MS: u32 = 300;
}

/// クライアント側のペイン状態（ウィンドウスレッドと tokio タスクで共有）
pub struct PaneStore {
    /// ペイン ID → グリッドの Arc
    pub grids: HashMap<PaneId, Arc<Mutex<Grid>>>,
    /// レイアウトツリー（フローティングペインは含まない）
    pub layout: LayoutNode,
    /// フォーカスされているペイン ID
    pub active: PaneId,
    /// OSC 52 で要求されたクリップボードデータ（Win32 スレッドが取り出して SetClipboardData）
    pub pending_clipboard: Option<Vec<u8>>,
    /// 未処理のトースト通知キュー（tokio → Win32 スレッドへの引き渡し）
    pub pending_toasts: VecDeque<Toast>,
    /// アクティブペインのスクロールオフセット（0 = 最新画面、正値 = 過去方向）
    pub scroll_offset: usize,
    /// フローティングペイン ID（None = 未作成）
    pub floating: Option<PaneId>,
    /// フローティングペインを表示中かどうか
    pub floating_visible: bool,
    /// フローティング表示前のアクティブペイン（非表示時の復帰用）
    pub pre_float_active: Option<PaneId>,
    /// true のとき Win32 タイマーがウィンドウを破棄してアプリを終了する（C-9）
    pub should_quit: bool,
    /// レイアウトランチャー UI の状態（Some = 表示中）
    pub launcher: Option<LauncherState>,
    /// コピーモードの状態（Some = コピーモード中）
    pub copy_mode: Option<CopyState>,
    /// Normal モードのマウス選択状態（anchor_col, anchor_row, end_col, end_row）
    pub normal_selection: Option<(usize, usize, usize, usize)>,
    /// レイアウト保存プロンプトの入力バッファ（Some = プロンプト表示中）
    pub save_prompt: Option<PromptState>,
    /// 保存プロンプトの入力履歴（新しいものが末尾）
    pub save_prompt_history: Vec<String>,
    /// `Ctrl+K/U/W` で削除したテキストを保持する yank バッファ
    pub save_prompt_yank: String,
    /// テーマランチャー UI の状態（Some = 表示中）
    pub theme_launcher: Option<ThemeLauncherState>,
    /// ペイン ID → 起動コマンド文字列（レイアウト適用時に記録、C-23）
    ///
    /// レイアウトファイルから適用されたコマンドのみ記録される。
    /// 手動入力したコマンドは含まれない。
    pub pane_commands: HashMap<PaneId, String>,
    /// ペイン ID → セッション保存時の作業ディレクトリ（SaveAndQuit 時に収集）
    pub pane_cwds: HashMap<PaneId, String>,
    /// ペイン ID → 論理名（alias）
    pub pane_aliases: HashMap<PaneId, String>,
    /// ペイン ID → 役割ラベル（role）
    pub pane_roles: HashMap<PaneId, String>,
    /// レイアウト変更フラグ（split / close 後に `true` にする）
    ///
    /// `WM_TIMER` で検出し `content_bb` を破棄して全画面再描画をトリガーする。
    /// 残像（旧ペイン領域の描画残り）を防ぐために使用する。
    pub layout_changed: bool,
    /// 通知アラート中のペイン → 残りフリップ数（0 = 非アラート）
    ///
    /// 偶数カウント = ボーダー ON、奇数カウント = ボーダー OFF。
    /// `WM_TIMER` 毎に `tick_alert()` を呼び、`ALERT_TICK_DIVISOR` ティックごとに
    /// カウントをデクリメントする。0 になったエントリは除去される。
    pub alerting_panes: HashMap<PaneId, u8>,
    /// 点滅タイマー用サブカウンタ（`ALERT_TICK_DIVISOR` で wrap して使用）
    pub alert_tick: u8,
    /// マウスホバー中の URL 情報: (pane_id, row, col_start, col_end_exclusive, url)
    ///
    /// `WM_MOUSEMOVE` で更新。描画ループでアンダーラインを引くために参照する。
    /// `None` = ホバー対象なし。
    pub hovered_url: Option<(PaneId, usize, usize, usize, String)>,
    /// ニュースティッカー本文（RSS から取得した見出しを区切り文字で連結）
    /// 空文字列 = 未取得またはティッカー無効
    pub news_text: String,
    /// CI ステータス（GitHub Actions ポーラーが定期更新）
    /// `None` = CI 設定なし、または未取得
    pub ci_status: Arc<Mutex<Option<CiRunInfo>>>,
}

impl PaneStore {
    pub fn new(pane_id: PaneId, grid: Arc<Mutex<Grid>>) -> Self {
        let mut grids = HashMap::new();
        grids.insert(pane_id, grid);
        Self {
            grids,
            layout: LayoutNode::Leaf(pane_id),
            active: pane_id,
            pending_clipboard: None,
            pending_toasts: VecDeque::new(),
            scroll_offset: 0,
            floating: None,
            floating_visible: false,
            pre_float_active: None,
            should_quit: false,
            launcher: None,
            copy_mode: None,
            normal_selection: None,
            save_prompt: None,
            save_prompt_history: Vec::new(),
            save_prompt_yank: String::new(),
            theme_launcher: None,
            pane_commands: HashMap::new(),
            pane_cwds: HashMap::new(),
            pane_aliases: HashMap::new(),
            pane_roles: HashMap::new(),
            layout_changed: false,
            alerting_panes: HashMap::new(),
            alert_tick: 0,
            hovered_url: None,
            news_text: String::new(),
            ci_status: Arc::new(Mutex::new(None)),
        }
    }

    /// フローティングペインをコンテンツ領域の中央 80% に配置した矩形を返す
    pub fn floating_rect(content: PaneRect) -> PaneRect {
        let w = ((content.w as f32 * 0.8) as i32).max(1);
        let h = ((content.h as f32 * 0.8) as i32).max(1);
        PaneRect {
            x: (content.w - w) / 2,
            y: (content.h - h) / 2,
            w,
            h,
        }
    }

    /// フローティングペインを表示してフォーカスを移す
    pub fn show_float(&mut self) {
        if let Some(float_id) = self.floating {
            self.pre_float_active = Some(self.active);
            self.active = float_id;
            self.floating_visible = true;
        }
    }

    /// フローティングペインを非表示にして元のペインにフォーカスを戻す
    pub fn hide_float(&mut self) {
        self.floating_visible = false;
        if let Some(prev) = self.pre_float_active.take() {
            if self.grids.contains_key(&prev) {
                self.active = prev;
            }
        }
    }

    /// 指定ペインの通知アラートを開始する（再トリガーするとフリップ数をリセット）。
    pub fn trigger_alert(&mut self, pane_id: PaneId) {
        self.alerting_panes.insert(pane_id, ALERT_FLIP_COUNT);
    }

    /// 指定ペインのアラートを即座に解除する（アクティブ化時などに使用）。
    pub fn clear_alert(&mut self, pane_id: PaneId) {
        self.alerting_panes.remove(&pane_id);
    }

    /// 指定ペインがアラートの点灯フェーズ（ボーダー表示）かどうかを返す。
    ///
    /// 偶数カウント = ON（最初のフリップは 10 = 偶数 → 即座に ON）。
    pub fn is_alert_on(&self, pane_id: PaneId) -> bool {
        match self.alerting_panes.get(&pane_id) {
            Some(&count) if count > 0 => count % 2 == 0,
            _ => false,
        }
    }

    /// `WM_TIMER` 16ms ティックごとに呼ぶ。
    ///
    /// `ALERT_TICK_DIVISOR` ティックに 1 回フリップカウントをデクリメントし、
    /// 0 になったペインを除去する。アラート中のペインが残っている間は `true` を返す。
    pub fn tick_alert(&mut self) -> bool {
        if self.alerting_panes.is_empty() {
            self.alert_tick = 0;
            return false;
        }
        self.alert_tick = self.alert_tick.wrapping_add(1);
        if self.alert_tick.is_multiple_of(ALERT_TICK_DIVISOR) {
            self.alerting_panes.retain(|_, count| {
                *count = count.saturating_sub(1);
                *count > 0
            });
        }
        !self.alerting_panes.is_empty()
    }

    pub fn push_save_prompt_history(&mut self, entry: String) {
        const HISTORY_LIMIT: usize = 20;

        if entry.is_empty() {
            return;
        }
        if self
            .save_prompt_history
            .last()
            .is_some_and(|last| last == &entry)
        {
            return;
        }
        self.save_prompt_history.push(entry);
        if self.save_prompt_history.len() > HISTORY_LIMIT {
            let drop_count = self.save_prompt_history.len() - HISTORY_LIMIT;
            self.save_prompt_history.drain(..drop_count);
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use yatamux_protocol::types::PaneId;
    use yatamux_terminal::Grid;

    use super::{CopyState, PaneStore, PromptState, ALERT_FLIP_COUNT, ALERT_TICK_DIVISOR};
    use crate::layout::PaneRect;

    // TC-01: layout_changed は false で初期化される
    #[test]
    fn test_layout_changed_initial_false() {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let store = PaneStore::new(PaneId(1), grid);
        assert!(!store.layout_changed);
    }

    // TC-02: layout_changed を true にセットできる
    #[test]
    fn test_layout_changed_set_true() {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let mut store = PaneStore::new(PaneId(1), grid);
        store.layout_changed = true;
        assert!(store.layout_changed);
    }

    // TC-03: layout_changed を true → false にクリアできる
    #[test]
    fn test_layout_changed_clear() {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let mut store = PaneStore::new(PaneId(1), grid);
        store.layout_changed = true;
        assert!(store.layout_changed);
        store.layout_changed = false;
        assert!(!store.layout_changed);
    }

    #[test]
    fn test_prompt_history_roundtrip_restores_draft() {
        let history = vec!["first".to_string(), "second".to_string()];
        let mut prompt = PromptState::new();
        prompt.insert_str("draft");

        assert!(prompt.history_prev(&history));
        assert_eq!(prompt.text, "second");
        assert!(prompt.history_prev(&history));
        assert_eq!(prompt.text, "first");
        assert!(prompt.history_next(&history));
        assert_eq!(prompt.text, "second");
        assert!(prompt.history_next(&history));
        assert_eq!(prompt.text, "draft");
    }

    #[test]
    fn test_prompt_kill_and_yank() {
        let mut prompt = PromptState::new();
        prompt.insert_str("alpha beta");
        prompt.move_start();
        prompt.move_right();
        prompt.move_right();
        let killed = prompt.kill_to_end().unwrap();
        assert_eq!(killed, "pha beta");
        prompt.insert_str(&killed);
        assert_eq!(prompt.text, "alpha beta");

        let killed_word = prompt.kill_prev_word().unwrap();
        assert_eq!(killed_word, "beta");
        prompt.insert_str(&killed_word);
        assert_eq!(prompt.text, "alpha beta");
    }

    #[test]
    fn test_prompt_backspace_respects_utf8_boundaries() {
        let mut prompt = PromptState::new();
        prompt.insert_str("あい");
        assert!(prompt.backspace());
        assert_eq!(prompt.text, "あ");
        assert_eq!(prompt.cursor, "あ".len());
    }

    #[test]
    fn test_floating_rect_centered() {
        let content = PaneRect {
            x: 0,
            y: 0,
            w: 200,
            h: 100,
        };
        let r = PaneStore::floating_rect(content);
        assert_eq!(r.w, 160);
        assert_eq!(r.h, 80);
        assert_eq!(r.x, 20);
        assert_eq!(r.y, 10);
    }

    #[test]
    fn test_floating_rect_odd_size() {
        let content = PaneRect {
            x: 0,
            y: 0,
            w: 101,
            h: 51,
        };
        let r = PaneStore::floating_rect(content);
        assert_eq!(r.w, 80);
        assert_eq!(r.h, 40);
        assert!(r.x >= 10);
        assert!(r.y >= 5);
    }

    #[test]
    fn test_show_float_sets_active() {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let float_grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let mut store = PaneStore::new(PaneId(1), grid);
        store.grids.insert(PaneId(2), float_grid);
        store.floating = Some(PaneId(2));
        store.show_float();
        assert_eq!(store.active, PaneId(2));
        assert_eq!(store.pre_float_active, Some(PaneId(1)));
        assert!(store.floating_visible);
    }

    #[test]
    fn test_hide_float_restores_active() {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let float_grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let mut store = PaneStore::new(PaneId(1), grid);
        store.grids.insert(PaneId(2), float_grid);
        store.floating = Some(PaneId(2));
        store.show_float();
        store.hide_float();
        assert_eq!(store.active, PaneId(1));
        assert!(!store.floating_visible);
    }

    #[test]
    fn test_floating_not_in_layout_ids() {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let store = PaneStore::new(PaneId(1), grid);
        let ids = store.layout.pane_ids();
        assert_eq!(ids, vec![PaneId(1)]);
    }

    // TC-B8: WM_SIZE 相当 — grid.resize() で全行 dirty になる（B-8 回帰確認）
    //
    // `handle_wm_size` は resize_all_panes() を呼び、その中で grid.resize() を呼ぶ。
    // grid.resize() が dirty を全行セットすることで WM_TIMER / InvalidateRect
    // 経由の再描画が確実に走ることを検証する。
    #[test]
    fn test_b8_resize_marks_all_dirty() {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        // 一度 dirty をクリア（ウィンドウが安定している状態を模倣）
        let _ = grid.lock().unwrap().take_dirty_rows();
        assert!(
            !grid.lock().unwrap().has_dirty_rows(),
            "precondition: dirty cleared"
        );

        // WM_SIZE 後の resize_all_panes() が呼ぶ grid.resize() を直接呼ぶ
        grid.lock().unwrap().resize(80, 24); // 同サイズでもリセットされる
        assert!(
            grid.lock().unwrap().has_dirty_rows(),
            "grid.resize() must mark all rows dirty so WM_TIMER → InvalidateRect fires"
        );
    }

    // TC-B9: ペイン比率変更後のリサイズで全グリッドが dirty になる（B-9 回帰確認）
    //
    // adjust_ratio_for_dir() でレイアウト矩形が変わった後、
    // resize_all_panes() 相当の grid.resize() を呼ぶと dirty が立つことを確認する。
    // dirty がなければ paint() の dirty_rows.is_empty() で continue → 描画されないため。
    #[test]
    fn test_b9_ratio_adjust_then_resize_marks_dirty() {
        use crate::layout::{LayoutNode, PaneRect};
        use yatamux_protocol::types::SplitDirection;

        let grid1 = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let grid2 = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        let pane1 = PaneId(1);
        let pane2 = PaneId(2);

        let mut store = PaneStore::new(pane1, grid1.clone());
        store.grids.insert(pane2, grid2.clone());
        store.layout = LayoutNode::Split {
            direction: SplitDirection::Vertical,
            ratio: 0.5,
            first: Box::new(LayoutNode::Leaf(pane1)),
            second: Box::new(LayoutNode::Leaf(pane2)),
        };

        // 安定状態に戻す（dirty クリア）
        let _ = grid1.lock().unwrap().take_dirty_rows();
        let _ = grid2.lock().unwrap().take_dirty_rows();
        assert!(!grid1.lock().unwrap().has_dirty_rows());
        assert!(!grid2.lock().unwrap().has_dirty_rows());

        // 比率変更（< キー相当）
        store
            .layout
            .adjust_ratio_for_dir(pane1, 0.1, SplitDirection::Vertical);

        // 比率変更後の compute_rects → grid.resize（resize_all_panes 相当）
        let total = PaneRect {
            x: 0,
            y: 0,
            w: 1000,
            h: 600,
        };
        let rects = store.layout.compute_rects(total);
        let cell_w = 10;
        let cell_h = 20;
        for (pane_id, rect) in &rects {
            let cols = (rect.w / cell_w).max(1) as u16;
            let rows = (rect.h / cell_h).max(1) as u16;
            if let Some(g) = store.grids.get(pane_id) {
                g.lock().unwrap().resize(cols, rows);
            }
        }

        // 両グリッドとも dirty でなければ paint() が新しい矩形に描き直さない
        assert!(
            grid1.lock().unwrap().has_dirty_rows(),
            "grid1 must be dirty after ratio adjustment + resize"
        );
        assert!(
            grid2.lock().unwrap().has_dirty_rows(),
            "grid2 must be dirty after ratio adjustment + resize"
        );
    }

    #[test]
    fn test_copy_state_init() {
        let cs = CopyState::new(0, 0);
        assert_eq!(cs.cursor, (0, 0));
        assert!(cs.anchor.is_none());
    }

    #[test]
    fn test_copy_state_cursor_clamp() {
        let mut cs = CopyState::new(0, 0);
        cs.move_cursor(-1, 0, 80, 24);
        assert_eq!(cs.cursor, (0, 0));
        cs.move_cursor(0, -1, 80, 24);
        assert_eq!(cs.cursor, (0, 0));
        cs.move_cursor(100, 100, 80, 24);
        assert_eq!(cs.cursor, (79, 23));
        cs.move_cursor(1, 0, 80, 24);
        assert_eq!(cs.cursor.0, 79);
        cs.move_cursor(0, 1, 80, 24);
        assert_eq!(cs.cursor.1, 23);
    }

    #[test]
    fn test_copy_state_anchor_toggle() {
        let mut cs = CopyState::new(5, 3);
        assert!(cs.anchor.is_none());
        cs.toggle_anchor();
        assert_eq!(cs.anchor, Some((5, 3)));
        cs.move_cursor(3, 2, 80, 24);
        assert_eq!(cs.anchor, Some((5, 3)));
        assert_eq!(cs.cursor, (8, 5));
        cs.toggle_anchor();
        assert!(cs.anchor.is_none());
    }

    #[test]
    fn test_copy_state_is_selected_single_row() {
        let mut cs = CopyState::new(2, 3);
        cs.toggle_anchor();
        cs.move_cursor(3, 0, 80, 24);
        assert!(cs.is_selected(2, 3));
        assert!(cs.is_selected(4, 3));
        assert!(cs.is_selected(5, 3));
        assert!(!cs.is_selected(1, 3));
        assert!(!cs.is_selected(6, 3));
        assert!(!cs.is_selected(3, 2));
    }

    #[test]
    fn test_copy_state_is_selected_multi_row() {
        let mut cs = CopyState::new(5, 2);
        cs.toggle_anchor();
        cs.move_cursor(3, 2, 80, 24);
        assert!(cs.is_selected(5, 2));
        assert!(cs.is_selected(79, 2));
        assert!(!cs.is_selected(4, 2));
        assert!(cs.is_selected(0, 3));
        assert!(cs.is_selected(79, 3));
        assert!(cs.is_selected(0, 4));
        assert!(cs.is_selected(8, 4));
        assert!(!cs.is_selected(9, 4));
    }

    fn make_store() -> PaneStore {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        PaneStore::new(PaneId(1), grid)
    }

    // TC-C41-11: 初期状態で alerting_panes は空
    #[test]
    fn test_alert_initial_empty() {
        let store = make_store();
        assert!(store.alerting_panes.is_empty());
        assert!(!store.is_alert_on(PaneId(1)));
    }

    // TC-C41-10: trigger_alert でペインが追加される
    #[test]
    fn test_trigger_alert_inserts_pane() {
        let mut store = make_store();
        store.trigger_alert(PaneId(2));
        assert!(store.alerting_panes.contains_key(&PaneId(2)));
        assert_eq!(store.alerting_panes[&PaneId(2)], ALERT_FLIP_COUNT);
    }

    // TC-C41-12: トリガー直後は is_alert_on が true
    #[test]
    fn test_is_alert_on_after_trigger() {
        let mut store = make_store();
        store.trigger_alert(PaneId(1));
        // ALERT_FLIP_COUNT=10 は偶数 → ON
        assert!(store.is_alert_on(PaneId(1)));
    }

    // TC-C41-13: 登録されていないペインは false
    #[test]
    fn test_is_alert_on_unregistered_false() {
        let store = make_store();
        assert!(!store.is_alert_on(PaneId(99)));
    }

    // TC-C41-14: ALERT_TICK_DIVISOR 回 tick するとフリップ数が 1 減る
    #[test]
    fn test_tick_alert_decrements_flip_count() {
        let mut store = make_store();
        store.trigger_alert(PaneId(1));
        for _ in 0..ALERT_TICK_DIVISOR {
            store.tick_alert();
        }
        assert_eq!(store.alerting_panes[&PaneId(1)], ALERT_FLIP_COUNT - 1);
    }

    // TC-C41-15: tick_alert はアラート中 true を返す
    #[test]
    fn test_tick_alert_returns_true_while_alerting() {
        let mut store = make_store();
        store.trigger_alert(PaneId(1));
        assert!(store.tick_alert());
    }

    // TC-C41-16: 全フリップ後に alerting_panes から除去される
    #[test]
    fn test_tick_alert_removes_pane_after_all_flips() {
        let mut store = make_store();
        store.trigger_alert(PaneId(1));
        let total_ticks = ALERT_TICK_DIVISOR as u32 * ALERT_FLIP_COUNT as u32;
        for _ in 0..total_ticks {
            store.tick_alert();
        }
        assert!(store.alerting_panes.is_empty());
        assert!(!store.is_alert_on(PaneId(1)));
    }

    // TC-C41-17: 全フリップ完了後 tick_alert は false を返す
    #[test]
    fn test_tick_alert_returns_false_after_completion() {
        let mut store = make_store();
        store.trigger_alert(PaneId(1));
        let total_ticks = ALERT_TICK_DIVISOR as u32 * ALERT_FLIP_COUNT as u32;
        for _ in 0..total_ticks {
            store.tick_alert();
        }
        assert!(!store.tick_alert());
    }

    // TC-C41-18: ON/OFF フェーズが交互に切り替わる
    #[test]
    fn test_alert_phase_alternates() {
        let mut store = make_store();
        store.trigger_alert(PaneId(1));
        // 初期: count=10(偶数) → ON
        assert!(store.is_alert_on(PaneId(1)));
        // 1 フリップ後: count=9(奇数) → OFF
        for _ in 0..ALERT_TICK_DIVISOR {
            store.tick_alert();
        }
        assert!(!store.is_alert_on(PaneId(1)));
        // 2 フリップ後: count=8(偶数) → ON
        for _ in 0..ALERT_TICK_DIVISOR {
            store.tick_alert();
        }
        assert!(store.is_alert_on(PaneId(1)));
    }

    // TC-C41-19: clear_alert でアラートが即座に解除される
    #[test]
    fn test_clear_alert_removes_pane() {
        let mut store = make_store();
        store.trigger_alert(PaneId(1));
        store.clear_alert(PaneId(1));
        assert!(store.alerting_panes.is_empty());
        assert!(!store.is_alert_on(PaneId(1)));
    }

    // TC-C41-20: 複数ペインを同時にアラートできる
    #[test]
    fn test_multiple_panes_alerting() {
        let mut store = make_store();
        store.trigger_alert(PaneId(1));
        store.trigger_alert(PaneId(2));
        assert!(store.is_alert_on(PaneId(1)));
        assert!(store.is_alert_on(PaneId(2)));
    }

    // TC-C41-21: clear_alert は存在しないペインにパニックしない
    #[test]
    fn test_clear_alert_nonexistent_no_panic() {
        let mut store = make_store();
        store.clear_alert(PaneId(99)); // パニックしないこと
    }

    // TC-C41-22: trigger_alert 再トリガーでフリップ数がリセットされる
    #[test]
    fn test_retrigger_resets_flip_count() {
        let mut store = make_store();
        store.trigger_alert(PaneId(1));
        // 数フリップ進める
        for _ in 0..(ALERT_TICK_DIVISOR as u32 * 3) {
            store.tick_alert();
        }
        let count_mid = store.alerting_panes[&PaneId(1)];
        assert!(count_mid < ALERT_FLIP_COUNT);
        // 再トリガー
        store.trigger_alert(PaneId(1));
        assert_eq!(store.alerting_panes[&PaneId(1)], ALERT_FLIP_COUNT);
    }
}
