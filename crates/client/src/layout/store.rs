use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use yatamux_protocol::types::PaneId;
use yatamux_terminal::Grid;

use super::{LauncherState, LayoutNode, PaneRect, ThemeLauncherState};

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
    pub save_prompt: Option<String>,
    /// テーマランチャー UI の状態（Some = 表示中）
    pub theme_launcher: Option<ThemeLauncherState>,
    /// ペイン ID → セッション保存時の作業ディレクトリ（SaveAndQuit 時に収集）
    pub pane_cwds: HashMap<PaneId, String>,
    /// ペイン ID → 起動コマンド文字列（レイアウト適用時に記録、C-23）
    ///
    /// レイアウトファイルから適用されたコマンドのみ記録される。
    /// 手動入力したコマンドは含まれない。
    pub pane_commands: HashMap<PaneId, String>,
    /// レイアウト変更フラグ（split / close 後に `true` にする）
    ///
    /// `WM_TIMER` で検出し `content_bb` を破棄して全画面再描画をトリガーする。
    /// 残像（旧ペイン領域の描画残り）を防ぐために使用する。
    pub layout_changed: bool,
    /// マウスホバー中の URL 情報: (pane_id, row, col_start, col_end_exclusive, url)
    ///
    /// `WM_MOUSEMOVE` で更新。描画ループでアンダーラインを引くために参照する。
    /// `None` = ホバー対象なし。
    pub hovered_url: Option<(PaneId, usize, usize, usize, String)>,
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
            theme_launcher: None,
            pane_cwds: HashMap::new(),
            pane_commands: HashMap::new(),
            layout_changed: false,
            hovered_url: None,
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
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use yatamux_protocol::types::PaneId;
    use yatamux_terminal::Grid;

    use super::{CopyState, PaneStore};
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
}
