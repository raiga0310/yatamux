//! # cmux-protocol — クライアント／サーバー間メッセージ型
//!
//! cmux-win の内部 IPC プロトコルを定義するクレート。
//! クライアント（Win32 ウィンドウ）とサーバー（PTY 管理）が
//! [`tokio::sync::mpsc`] チャネルまたは Windows 名前付きパイプ経由でやり取りするメッセージを定義する。
//!
//! ## メッセージフロー
//!
//! ```text
//! Client                               Server
//!   │── CreateWorkspace ─────────────────▶│
//!   │◀─ WorkspaceCreated ─────────────────│
//!   │── CreateSurface ───────────────────▶│
//!   │◀─ SurfaceCreated ───────────────────│
//!   │── CreatePane ─────────────────────▶│
//!   │◀─ PaneCreated ──────────────────────│
//!   │── Input / Resize ────────────────▶│
//!   │◀─ Output / TitleChanged ────────────│
//! ```
//!
//! ## 階層モデル
//!
//! tmux との対応関係:
//!
//! | cmux-win | tmux 相当 | 説明 |
//! |----------|-----------|------|
//! | [`WorkspaceId`] | session | 最上位コンテナ |
//! | [`SurfaceId`]   | window  | タブに相当 |
//! | [`PaneId`]      | pane    | 実際の PTY を持つ単位 |

pub mod message;
pub mod types;

pub use message::{ClientMessage, ServerMessage};
pub use types::{PaneId, SplitDirection, SurfaceId, TermSize, WorkspaceId};
