//! # yatamux-server — PTY 管理・セッションサーバー
//!
//! ターミナルの PTY ライフサイクルとセッション状態を管理するクレート。
//!
//! ## 責務
//!
//! - **セッション管理** ([`session`]): Workspace → Surface → Pane の階層ツリーを保持する
//! - **PTY 管理** ([`pane`]): ConPTY の起動・入力書き込み・出力読み取り・リサイズを非同期で処理する
//! - **IPC サーバー** ([`ipc`]): Windows 名前付きパイプ経由の外部クライアント接続（オプション）
//!
//! ## インプロセス動作
//!
//! yatamux のメインバイナリでは、サーバーとクライアントは同一プロセス内で動作する。
//! [`Server`] は [`tokio::sync::mpsc`] チャネルで直結されており、
//! 名前付きパイプ IPC のオーバーヘッドなしに動作する。
//!
//! ```text
//! app::run()
//!   ├── Server::new(server_tx) → Server::run(client_rx)  ← tokio タスク
//!   └── run_window(...)                                    ← spawn_blocking
//! ```
//!
//! ## ペインツリー
//!
//! ペインは [`session::PaneTree`] バイナリツリーで管理される。
//! 分割操作は葉ノードを `Split` ノードに置き換えることで行われる:
//!
//! ```text
//! Leaf(A)  ─[Ctrl+Shift+E]→  Split { Vertical, 0.5, Leaf(A), Leaf(B) }
//! ```

pub mod pane;
pub mod session;
pub mod ipc;

pub use session::Server;
