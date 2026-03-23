//! # yatamux-terminal — VT パーサ・グリッド・CJK 幅計算・PTY セッション
//!
//! ターミナルエミュレーションのコアロジックを担うクレート。
//! 外部の TUI ライブラリは使用せず、VT シーケンスのパースから
//! グリッド状態管理まですべて自前で実装している。
//!
//! ## モジュール構成
//!
//! | モジュール | 役割 |
//! |-----------|------|
//! | [`grid`]  | ターミナルグリッド（セル配列・カーソル・スクロール・代替画面） |
//! | [`vt`]    | VT シーケンスプロセッサ（[`vte`] クレートのステートマシン上に実装） |
//! | [`cell`]  | セル型（内容・スタイル・CJK 全角セルの Continuation マーク） |
//! | [`width`] | CJK 文字幅計算（UAX #11 + 独自オーバーライドテーブル） |
//! | [`pty`]   | ConPTY ラッパー（[`portable-pty`] による PTY 起動・読み書き） |
//!
//! ## CJK 幅計算の設計方針
//!
//! ConPTY が報告するカーソル位置は CJK 文字幅を正確に反映しない場合がある。
//! このクレートでは ConPTY のカーソル位置を信用せず、
//! [`CjkWidthConfig`] によるソフトウェア計算を **権威的なソース** として使用する。
//!
//! 特別に対処しているケース:
//! - 半角カタカナ濁点 U+FF9E / U+FF9F: `unicode-width` は 0 を返すが、実際は 1 セル
//! - East Asian Ambiguous 文字（罫線 U+2500–257F 等）: [`AmbiguousWidth`] で設定可能
//! - VS16 (U+FE0F) を含むグラフィームクラスタ: 絵文字扱いで 2 セル
//! - NFD 韓国語: `unicode-normalization` で NFC に変換してから幅を計算
//!
//! ## 対応 VT シーケンス
//!
//! vim、lazygit、claude code での動作に必要なシーケンスをカバーする:
//!
//! - **SGR** (m): 標準 16 色・256 色・TrueColor・bold/italic/underline/reverse/dim
//! - **カーソル移動**: CUP (H/f)、CUU/CUD/CUF/CUB (A/B/C/D)
//! - **消去**: ED (J)、EL (K)
//! - **スクロール**: DECSTBM (r)、SU/SD (S/T)
//! - **モード**: DECAWM、代替画面 (1049h/l)、ブラケットペースト、フォーカスイベント、マウス報告
//! - **OSC**: タイトル変更 (0/2)、デスクトップ通知 (9/99/777)

pub mod cell;
pub mod grid;
pub mod pty;
pub mod vt;
pub mod width;

pub use cell::{Cell, CellStyle, Color};
pub use grid::Grid;
pub use pty::PtySession;
pub use width::CjkWidthConfig;

use std::sync::{Arc, Mutex};

/// グリッドと VT パーサーをまとめたシンク。
///
/// クライアント側でサーバーからの出力データを受け取り、
/// ローカルグリッドを更新する際に使用する。
pub struct TerminalSink {
    pub grid: Arc<Mutex<Grid>>,
    parser: vte::Parser,
}

impl TerminalSink {
    pub fn new(cols: u16, rows: u16) -> Self {
        let grid = Arc::new(Mutex::new(Grid::new(cols, rows, CjkWidthConfig::default())));
        Self {
            grid,
            parser: vte::Parser::new(),
        }
    }

    /// VT バイト列をパースしてグリッドに適用する
    pub fn feed(&mut self, data: &[u8]) {
        let mut g = self.grid.lock().unwrap();
        let mut proc = vt::VtProcessor::new(&mut g);
        vt::feed_bytes(&mut self.parser, &mut proc, data);
    }
}
