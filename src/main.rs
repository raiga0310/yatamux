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

use anyhow::Result;
use clap::{Parser, Subcommand};

/// 親プロセスのコンソール（PowerShell 等）にアタッチし、
/// `println!` / clap の出力が届くよう stdout/stderr を CONOUT$ にリダイレクトする。
///
/// リリースビルドは `windows_subsystem = "windows"` により stdout が無効なため、
/// `--help` / `--version` などの表示前にこの関数を呼ぶ必要がある。
#[cfg(windows)]
fn attach_parent_console() {
    use windows::Win32::Storage::FileSystem::{
        CreateFileW, FILE_FLAGS_AND_ATTRIBUTES, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows::Win32::System::Console::{
        AttachConsole, SetStdHandle, ATTACH_PARENT_PROCESS, STD_ERROR_HANDLE, STD_OUTPUT_HANDLE,
    };
    unsafe {
        // 親プロセス（PowerShell 等）のコンソールにアタッチ
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
            // CONOUT$ への書き込みハンドルを取得し stdout/stderr に設定する。
            // AttachConsole だけでは GUI サブシステムプロセスの GetStdHandle が
            // NULL のままのため SetStdHandle で上書きが必要。
            if let Ok(h) = CreateFileW(
                windows::core::w!("CONOUT$"),
                0x4000_0000, // GENERIC_WRITE
                FILE_SHARE_WRITE,
                None,
                OPEN_EXISTING,
                FILE_FLAGS_AND_ATTRIBUTES(0),
                None,
            ) {
                let _ = SetStdHandle(STD_OUTPUT_HANDLE, h);
                let _ = SetStdHandle(STD_ERROR_HANDLE, h);
            }
        }
    }
}

mod app;
mod cli;
mod config;
mod layout_config;

/// デフォルトセッション名（IPC パイプ名のサフィックス）
pub const DEFAULT_SESSION: &str = "default";

/// CJK 対応 Windows ターミナルマルチプレクサ
#[derive(Parser)]
#[command(name = "yatamux", version, about)]
struct Cli {
    /// 起動時に適用するレイアウト名（%APPDATA%\yatamux\layouts\<NAME>.toml）
    #[arg(long, value_name = "NAME")]
    layout: Option<String>,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// アクティブなペイン一覧を表示
    ListPanes,
    /// 指定ペインにキー入力を送信
    SendKeys {
        /// 送信先ペイン ID
        #[arg(long, value_name = "ID")]
        pane: u32,
        /// 送信するテキスト
        text: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    #[cfg(debug_assertions)]
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    #[cfg(windows)]
    {
        if std::env::args().count() > 1 {
            // CLI 引数あり: 親コンソール（PowerShell 等）にアタッチして出力を有効化。
            // `cli` フィーチャービルド（コンソールサブシステム）では既に stdout 有効だが
            // 親コンソールに明示的に繋ぐことで出力先を統一する。
            attach_parent_console();
        } else {
            // 引数なし = GUI 起動。`cli` フィーチャービルドはコンソールサブシステムなので
            // 起動時にコンソールウィンドウが開く。FreeConsole() で即座に解放する。
            // GUI サブシステムビルドではコンソールがないため no-op になる。
            unsafe {
                use windows::Win32::System::Console::FreeConsole;
                let _ = FreeConsole();
            }
        }
    }

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::ListPanes) => cli::list_panes(DEFAULT_SESSION).await,
        Some(Commands::SendKeys { pane, text }) => {
            cli::send_keys(DEFAULT_SESSION, pane, &text).await
        }
        None => {
            let app_config =
                config::AppConfig::load(&config::AppConfig::default_path()).unwrap_or_default();
            app::run(cli.layout, app_config).await
        }
    }
}
