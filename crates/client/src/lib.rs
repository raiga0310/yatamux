//! # yatamux-client — Win32 ウィンドウ・GDI レンダリング・IME・レイアウト
//!
//! yatamux の描画とユーザー入力処理を担うクレート。
//! Win32 API を直接使用し、外部の GUI フレームワークには依存しない。
//!
//! ## モジュール構成
//!
//! | モジュール | 役割 |
//! |-----------|------|
//! | [`window`]     | Win32 メッセージループ・GDI ダブルバッファ描画（60fps / 16ms タイマー） |
//! | [`ime`]        | IMM32 ベースの日本語入力処理（プリエディット表示・確定送信） |
//! | [`layout`]     | ペインのピクセル矩形計算（バイナリツリー → `Vec<(PaneId, PaneRect)>`） |
//! | [`connection`] | 名前付きパイプ経由の外部サーバー接続（外部プロセス接続時に使用） |
//!
//! ## GDI レンダリングの設計
//!
//! GPU は使用しない。すべて Win32 GDI で実装している。
//!
//! ```text
//! WM_TIMER (16ms)
//!   └── WM_PAINT
//!         ├── CreateCompatibleDC（バックバッファ）
//!         ├── 背景塗りつぶし（COLOR_BG = #1e1e2e）
//!         ├── セル描画（文字・スタイル・全角幅対応）
//!         ├── 罫線文字（U+2500–259F）を GDI プリミティブで直接描画
//!         ├── IME プリエディットオーバーレイ
//!         ├── カーソル（2px バースタイル縦棒）
//!         └── BitBlt（フロントバッファに転送）
//! ```
//!
//! **罫線文字を GDI プリミティブで描く理由**: フォントによっては全角グリッドで
//! 罫線が 1 セル分ずれ、neovim 等のボックス UI が崩れる。
//! `MoveToEx` / `LineTo` / `FillRect` で自前描画することでフォント非依存にしている。
//!
//! ## IME 処理
//!
//! `WM_IME_STARTCOMPOSITION` でデフォルト IME ウィンドウを抑制し、
//! `WM_IME_COMPOSITION` でプリエディット文字列を取得してカーソル位置に重ねて描画する。
//! 確定文字列は UTF-8 に変換してサーバーの [`ClientMessage::Input`] で送信する。
//!
//! ## フォント選択
//!
//! 起動時にインストール済みフォントから以下の優先順位で自動選択する:
//!
//! 1. HackGen Console NF / HackGen Console
//! 2. HackGen35 Console NF / HackGen35 Console
//! 3. Cascadia Mono / Cascadia Code
//! 4. MS Gothic（最終フォールバック）
//!
//! `GetTextFaceW` の戻り値はヌル終端を含むため、`.trim_end_matches('\0')` で除去してから比較する。

pub mod connection;
pub mod ime;
pub mod layout;
pub mod notification;
pub mod session;
pub mod theme;
pub mod url;
pub mod window;

pub use ime::{CellPixelPos, ImeHandler, ImeState, PreeditAttr, PreeditSegment};
pub use layout::{
    CopyState, Direction, LauncherState, LayoutNode, LayoutPreview, PaneRect, PaneStore,
    PromptState, ThemeLauncherState, Toast,
};
pub use notification::{AlertingBackend, FocusAwareBackend, NotificationBackend};
pub use session::{LayoutNodeDef, LayoutSnapshot};
pub use theme::Theme;
pub use window::run_window;
