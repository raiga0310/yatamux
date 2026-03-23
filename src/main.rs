//! # yatamux — CJK 対応 Windows ターミナルマルチプレクサ
//!
//! ## 背景
//!
//! [ghostty] や [cmux] をはじめとする、エージェント（AI コーディングアシスタント）向けに
//! 設計されたモダンなターミナルアプリケーションの多くは **macOS / Linux 向け** に開発されている。
//! Windows 移植版（WSL 経由の利用を含む）も存在するが、以下の課題が残っていた:
//!
//! - **CJK 文字幅の不正確な計算**: 漢字・かな・ハングルが 1 セル幅として扱われ、カーソルがずれる
//! - **IME（日本語入力）の未対応・不完全対応**: プリエディット文字列の表示が崩れる
//! - **半角カタカナ濁点 (U+FF9E / U+FF9F) の扱い**: 結合マークと誤認識され幅計算が狂う
//! - **罫線文字のフォント依存**: neovim 等のボックスボーダー UI がフォントによっては描画崩れを起こす
//!
//! yatamux はこれらの問題を Windows ネイティブの実装で解決するために作られた。
//! ConPTY / Win32 GDI / IMM32 をすべてネイティブに利用し、CJK 環境での動作を第一に設計している。
//!
//! [ghostty]: https://ghostty.org/
//! [cmux]: https://github.com/nicowillis/cmux
//!
//! ## 主な特徴
//!
//! | 機能 | 説明 |
//! |------|------|
//! | **CJK 幅計算** | UAX #11 + 独自オーバーライドテーブル。ConPTY のカーソル位置は使用しない |
//! | **IME 対応** | WM_IME_COMPOSITION でプリエディット表示、確定文字列を UTF-8 で PTY に送信 |
//! | **ペイン分割** | バイナリツリーレイアウト。`Ctrl+Shift+E`（縦）/ `Ctrl+Shift+O`（横） |
//! | **罫線文字** | U+2500–259F を GDI プリミティブで直接描画（フォント依存なし） |
//! | **フォント優先順位** | HackGen Console NF → HackGen Console → Cascadia → Consolas |
//! | **カラーテーマ** | Catppuccin Mocha（背景 `#1e1e2e`、前景 `#cdd6f4`、カーソル `#f5c2e7`） |
//! | **動作確認済み** | vim、lazygit、claude code |
//!
//! ## アーキテクチャ
//!
//! ```text
//! yatamux (bin)
//! ├── yatamux-server   PTY 管理・ペイン生成（ConPTY ラッパー、セッション木）
//! ├── yatamux-client   Win32 ウィンドウ・GDI レンダリング・IME ハンドラ・レイアウト計算
//! ├── yatamux-protocol クライアント ↔ サーバー メッセージ型（ClientMessage / ServerMessage）
//! ├── yatamux-terminal VT パーサ・グリッド・CJK 幅テーブル・PTY セッション
//! └── yatamux-renderer テキストモードデバッグレンダラー（フェーズ 2 で GPU 化予定）
//! ```
//!
//! GUI とサーバーは同一プロセス内で動作し、[`tokio::sync::mpsc`] チャネルで直結する
//! （GUI ↔ サーバー間に IPC のオーバーヘッドはない）。
//!
//! 加えて、外部プロセス（`list-panes` / `send-keys` CLI やエージェント）からペインを操作できるよう、
//! GUI 起動時に Windows 名前付きパイプ IPC サーバー（`\\.\pipe\yatamux-{session}`）を常時起動する。
//! 外部クライアントからの入力は GUI の入力と merged チャネルで合流し、
//! サーバー出力はファンアウトタスクが GUI と IPC クライアント両方へ配信する。
//!
//! ## スレッド構成
//!
//! ```text
//! tokio ランタイム
//! ├── Server::run()                    PTY 管理・セッション処理
//! ├── Pane（ペインごと）               PTY 読み取り・書き込みタスク
//! ├── 出力ファンアウト                 server_out → GUI + IPC に配信
//! ├── IPC サーバー                     \\.\pipe\yatamux-{session} を常時待ち受け
//! └── 出力ルーター + 分割ハンドラ      select! ループ
//!
//! spawn_blocking
//! └── Win32 メッセージループ           GDI 描画・キー入力・IME 処理
//! ```
//!
//! Win32 メッセージループはブロッキング API のため `spawn_blocking` で tokio から切り離す。
//! 共有状態は `Arc<Mutex<PaneStore>>` のみ。
//!
//! ## 既知の制限
//!
//! - ペイン分割比は 50:50 固定（ドラッグリサイズ未実装）
//! - スクロールバック未実装
//! - Windows 10 1903 (Build 18362) 以降が必要（ConPTY API の要件）
//!
//! ## 起動
//!
//! ダブルクリックまたはスタートメニューから起動する GUI アプリ。
//! コンソールウィンドウは表示せず、独自の Win32 ウィンドウを開く。
//! リリースビルドでは `windows_subsystem = "windows"` によりコンソールを持たない。

#![cfg_attr(
    all(not(debug_assertions), not(feature = "cli")),
    windows_subsystem = "windows"
)]

use anyhow::{bail, Result};

mod app;
mod cli;

/// デフォルトセッション名（IPC パイプ名のサフィックス）
pub const DEFAULT_SESSION: &str = "default";

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(debug_assertions)]
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let args: Vec<String> = std::env::args().collect();

    match args.get(1).map(String::as_str) {
        Some("list-panes") => cli::list_panes(DEFAULT_SESSION).await,
        Some("send-keys") => {
            let pane_pos = args.iter().position(|a| a == "--pane");
            let pane_id = pane_pos
                .and_then(|i| args.get(i + 1))
                .and_then(|s| s.parse::<u32>().ok());
            // text は --pane <id> の直後の引数
            let text = pane_pos.and_then(|i| args.get(i + 2)).cloned();
            match (pane_id, text) {
                (Some(id), Some(t)) => {
                    cli::send_keys(DEFAULT_SESSION, id, &t).await
                }
                _ => {
                    eprintln!("Usage: yatamux send-keys --pane <id> <text>");
                    bail!("missing arguments");
                }
            }
        }
        Some(other) => {
            eprintln!("Unknown subcommand: {other}");
            eprintln!("Usage: yatamux [list-panes | send-keys --pane <id> <text>]");
            bail!("unknown subcommand");
        }
        None => app::run().await,
    }
}
