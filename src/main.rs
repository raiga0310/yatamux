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
mod ci;
mod cli;
mod config;
mod layout_config;
mod update;

/// デフォルトセッション名（IPC パイプ名のサフィックス）
pub const DEFAULT_SESSION: &str = "default";

/// CJK 対応 Windows ターミナルマルチプレクサ
#[derive(Parser)]
#[command(name = "yatamux", version, about)]
struct Cli {
    /// 対象セッション名（IPC パイプ名のサフィックス）
    ///
    /// 省略時は `YATAMUX_SESSION` 環境変数を参照し、それも未設定なら "default" を使用する。
    /// ペイン内から CLI サブコマンドを実行する際に自動で正しいセッションへ接続できる。
    #[arg(long, env = "YATAMUX_SESSION", default_value = DEFAULT_SESSION, global = true, hide = true)]
    session: String,

    /// 起動時に適用するレイアウト名（%APPDATA%\yatamux\layouts\<NAME>.toml）
    #[arg(long, value_name = "NAME")]
    layout: Option<String>,

    /// 内部ヘルパーモード: 指定 PID の終了を待ってバイナリ置換を行う
    ///
    /// 使用法: --apply-update <PID> <NEW_EXE_PATH> [--launch]
    /// このオプションはセルフアップデート処理から内部的に使用される。
    #[arg(long, value_names = ["PID", "NEW_PATH"], num_args = 2, hide = true)]
    apply_update: Option<Vec<String>>,

    /// --apply-update と組み合わせて使用: 置換後に新しいインスタンスを起動する
    #[arg(long, hide = true)]
    launch: bool,

    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// アクティブなペイン一覧を表示
    ListPanes {
        /// JSON 配列形式で出力する（エージェント向け）
        #[arg(long)]
        json: bool,
    },
    /// 指定ペインにキー入力を送信
    ///
    /// エスケープシーケンス: \n=LF, \r=CR, \t=TAB, \\=バックスラッシュ
    ///
    /// 注意: Windows パス（例: C:\Users\name）の \n や \r はエスケープ変換される。
    /// パスをそのまま送る場合は --raw を使用すること。
    ///
    /// 例:
    ///   yatamux send-keys --pane 1 --enter "cargo test"
    ///   yatamux send-keys --pane 1 "echo hello\r"
    ///   yatamux send-keys --pane 1 --raw --enter "C:\Users\raiga\dev"
    #[command(verbatim_doc_comment)]
    SendKeys {
        /// 送信先ペイン ID または alias
        #[arg(long, value_name = "ID|ALIAS")]
        pane: String,
        /// 送信するテキスト（エスケープ変換あり: \n=LF \r=CR \t=TAB \\=バックスラッシュ）
        text: String,
        /// 末尾に CR（Enter）を自動付加する
        #[arg(long)]
        enter: bool,
        /// エスケープ変換を無効化してテキストをそのまま送信（Windows パスなどに使用）
        #[arg(long)]
        raw: bool,
        /// コマンド完了（OSC 133;D）を受信するまで待機してから終了する
        #[arg(long)]
        wait_for_prompt: bool,
    },
    /// 指定ペインが条件を満たすまで待機する
    WaitPane {
        /// 対象ペイン ID または alias
        #[arg(long, value_name = "ID|ALIAS")]
        pane: String,
        /// 待機条件
        #[arg(long, value_enum, default_value = "exit")]
        wait_for: WaitForArg,
        /// 全体タイムアウト秒
        #[arg(long, default_value = "60")]
        timeout: u64,
        /// `wait-for output-regex` で待つ正規表現
        #[arg(long)]
        output_regex: Option<String>,
        /// `wait-for silence` の静穏時間ミリ秒
        #[arg(long, default_value = "1500")]
        silence_ms: u64,
        /// `wait-for output-regex` で確認する capture-pane 行数
        #[arg(long, default_value = "200")]
        lines: usize,
    },
    /// 指定ペインでコマンドを実行し、条件を満たすまで待機する
    Exec {
        /// 対象ペイン ID または alias
        #[arg(long, value_name = "ID|ALIAS")]
        pane: String,
        /// 待機条件
        #[arg(long, value_enum, default_value = "exit")]
        wait_for: WaitForArg,
        /// 全体タイムアウト秒
        #[arg(long, default_value = "60")]
        timeout: u64,
        /// `wait-for output-regex` で待つ正規表現
        #[arg(long)]
        output_regex: Option<String>,
        /// `wait-for silence` の静穏時間ミリ秒
        #[arg(long, default_value = "1500")]
        silence_ms: u64,
        /// `wait-for output-regex` で確認する capture-pane 行数
        #[arg(long, default_value = "200")]
        lines: usize,
        /// エスケープ変換を無効化してそのまま送信する
        #[arg(long)]
        raw: bool,
        /// 実行するコマンド
        #[arg(
            value_name = "COMMAND",
            required = true,
            num_args = 1..,
            trailing_var_arg = true,
            allow_hyphen_values = true
        )]
        command: Vec<String>,
    },
    /// 指定ペインの出力ストリームを購読する
    SubscribePane {
        /// 対象ペイン ID または alias
        #[arg(long, value_name = "ID|ALIAS")]
        pane: String,
        /// JSON Lines でイベントを出力する
        #[arg(long)]
        json: bool,
    },
    /// 指定ペインに Ctrl+C を送信する
    InterruptPane {
        /// 送信先ペイン ID または alias
        #[arg(long, value_name = "ID|ALIAS")]
        pane: String,
    },
    /// 指定ペインの子プロセスを強制終了する
    TerminatePane {
        /// 対象ペイン ID または alias
        #[arg(long, value_name = "ID|ALIAS")]
        pane: String,
    },
    /// 指定ペインを閉じる
    ClosePane {
        /// 対象ペイン ID または alias
        #[arg(long, value_name = "ID|ALIAS")]
        pane: String,
    },
    /// ペインの alias / role メタデータを更新する
    SetPaneMeta {
        /// 対象ペイン ID または alias
        #[arg(long, value_name = "ID|ALIAS")]
        pane: String,
        /// 論理名（alias）
        #[arg(long)]
        alias: Option<String>,
        /// 役割ラベル（role）
        #[arg(long)]
        role: Option<String>,
    },
    /// 指定ペインの内容を表示（スクロールバック末尾 N 行 + 現在画面）
    CapturePane {
        /// 対象ペイン ID または alias
        #[arg(long, default_value = "0")]
        target: String,
        /// 取得する行数
        #[arg(long, default_value = "100")]
        lines: usize,
        /// ANSI エスケープを除去してプレーンテキストで出力する（エージェント向け）
        #[arg(long)]
        plain_text: bool,
        /// 構造化 JSON で出力する（エージェント向け）
        #[arg(long)]
        json: bool,
    },
    /// ペインを分割して新しいペインを作成
    SplitPane {
        /// 作業ディレクトリ
        #[arg(long)]
        dir: Option<String>,
        /// 分割方向 (vertical / horizontal)
        #[arg(long, value_enum, default_value = "vertical")]
        direction: SplitDirectionArg,
        /// 分割元ペイン ID（省略時は 0）
        #[arg(long)]
        target: Option<String>,
    },
    /// レイアウトファイルを管理する（C-22）
    ///
    /// 例:
    ///   yatamux layout list
    ///   yatamux layout delete my-project
    ///   yatamux layout export my-project
    #[command(verbatim_doc_comment, subcommand)]
    Layout(LayoutCommands),

    /// GitHub Releases から最新バイナリを取得してセルフアップデートする（C-38）
    Update,

    /// 設定ファイルを読み込んで %APPDATA%\yatamux\config.toml に適用する
    ///
    /// 指定した TOML ファイルを検証し、yatamux の設定ファイルとして保存する。
    /// 変更はプロセス再起動後に反映される。
    ///
    /// 例:
    ///   yatamux source ~/my-config.toml
    ///   yatamux source C:\Users\raiga\dotfiles\yatamux.toml
    #[command(verbatim_doc_comment)]
    Source {
        /// 読み込む TOML 設定ファイルのパス
        path: String,
    },
}

/// `yatamux layout` のサブコマンド
#[derive(Subcommand)]
enum LayoutCommands {
    /// 保存済みレイアウトの一覧を表示
    List,
    /// レイアウトを削除
    Delete {
        /// 削除するレイアウト名
        name: String,
    },
    /// レイアウトの内容を標準出力に出力
    Export {
        /// エクスポートするレイアウト名
        name: String,
    },
}

/// CLI 用の分割方向
#[derive(clap::ValueEnum, Clone, Debug)]
enum SplitDirectionArg {
    Vertical,
    Horizontal,
}

#[derive(clap::ValueEnum, Clone, Copy, Debug, PartialEq, Eq)]
enum WaitForArg {
    Exit,
    Silence,
    OutputRegex,
}

fn command_name(command: &Commands) -> &'static str {
    match command {
        Commands::ListPanes { .. } => "list-panes",
        Commands::SendKeys { .. } => "send-keys",
        Commands::WaitPane { .. } => "wait-pane",
        Commands::Exec { .. } => "exec",
        Commands::SubscribePane { .. } => "subscribe-pane",
        Commands::InterruptPane { .. } => "interrupt-pane",
        Commands::TerminatePane { .. } => "terminate-pane",
        Commands::ClosePane { .. } => "close-pane",
        Commands::SetPaneMeta { .. } => "set-pane-meta",
        Commands::CapturePane { .. } => "capture-pane",
        Commands::SplitPane { .. } => "split-pane",
        Commands::Layout(_) => "layout",
        Commands::Update => "update",
        Commands::Source { .. } => "source",
    }
}

fn is_truthy_env(name: &str) -> bool {
    std::env::var(name).ok().is_some_and(|value| {
        let value = value.trim();
        !value.is_empty() && value != "0" && !value.eq_ignore_ascii_case("false")
    })
}

fn maybe_write_startup_probe(cli: &Cli) -> Result<bool> {
    use anyhow::Context;
    use std::io::Write;

    let Ok(probe_path) = std::env::var("YATAMUX_STARTUP_PROBE_FILE") else {
        return Ok(false);
    };

    let payload = serde_json::json!({
        "pid": std::process::id(),
        "session": cli.session,
        "appdata": std::env::var("APPDATA").ok(),
        "exe": std::env::current_exe().ok(),
        "command": cli.command.as_ref().map(command_name),
        "apply_update": cli.apply_update.is_some(),
        "launch": cli.launch,
        "layout": cli.layout.as_deref(),
        "args": std::env::args().collect::<Vec<_>>(),
    });

    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&probe_path)
        .with_context(|| format!("startup probe ファイルを開けませんでした: {}", probe_path))?;
    serde_json::to_writer(&mut file, &payload).with_context(|| {
        format!(
            "startup probe JSON の書き込みに失敗しました: {}",
            probe_path
        )
    })?;
    writeln!(file)
        .with_context(|| format!("startup probe 改行の書き込みに失敗しました: {}", probe_path))?;

    Ok(is_truthy_env("YATAMUX_STARTUP_PROBE_EXIT")
        && cli.command.is_none()
        && cli.apply_update.is_none())
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
            // yatamux ペイン内（YATAMUX=1）では ConPTY が stdout を用意しているため
            // attach_parent_console() は不要（CONOUT$ にリダイレクトすると逆に出力が消える）。
            // ペイン外（PowerShell / CMD 直起動）の場合のみ親コンソールにアタッチする。
            if std::env::var_os("YATAMUX").is_none() {
                attach_parent_console();
            }
            // UTF-8 コードページに切り替えて日本語文字化けを防ぐ。
            unsafe {
                use windows::Win32::System::Console::SetConsoleOutputCP;
                let _ = SetConsoleOutputCP(65001);
            }
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

    // --version / -V を Cli::parse() より前にインターセプトして明示フラッシュ。
    // clap はバージョン表示後 process::exit(0) するが GUI サブシステムバイナリでは
    // stdout バッファが flush される前に終了することがあるため自前で処理する。
    {
        use std::io::Write;
        let args: Vec<String> = std::env::args().collect();
        if args.iter().any(|a| a == "--version" || a == "-V") {
            println!("yatamux {}", env!("CARGO_PKG_VERSION"));
            let _ = std::io::stdout().flush();
            return Ok(());
        }
    }

    let cli = Cli::parse();
    std::env::set_var("YATAMUX_SESSION", &cli.session);
    if maybe_write_startup_probe(&cli)? {
        return Ok(());
    }

    // --apply-update <pid> <new_path> [--launch] モード
    if let Some(args) = cli.apply_update {
        let pid: u32 = args[0]
            .parse()
            .map_err(|_| anyhow::anyhow!("無効な PID: {}", args[0]))?;
        let new_path = std::path::PathBuf::from(&args[1]);
        return apply_update(pid, &new_path, cli.launch).await;
    }

    match cli.command {
        Some(Commands::ListPanes { json }) => cli::list_panes(&cli.session, json).await,
        Some(Commands::SendKeys {
            pane,
            text,
            enter,
            raw,
            wait_for_prompt,
        }) => cli::send_keys(&cli.session, &pane, &text, enter, raw, wait_for_prompt).await,
        Some(Commands::WaitPane {
            pane,
            wait_for,
            timeout,
            output_regex,
            silence_ms,
            lines,
        }) => {
            cli::wait_pane(
                &cli.session,
                &pane,
                cli::WaitOptions {
                    wait_for,
                    timeout_secs: timeout,
                    output_regex,
                    silence_ms,
                    lines,
                },
            )
            .await
        }
        Some(Commands::Exec {
            pane,
            wait_for,
            timeout,
            output_regex,
            silence_ms,
            lines,
            raw,
            command,
        }) => {
            cli::exec_command(
                &cli.session,
                &pane,
                command,
                raw,
                cli::WaitOptions {
                    wait_for,
                    timeout_secs: timeout,
                    output_regex,
                    silence_ms,
                    lines,
                },
            )
            .await
        }
        Some(Commands::SubscribePane { pane, json }) => {
            cli::subscribe_pane(&cli.session, &pane, json).await
        }
        Some(Commands::InterruptPane { pane }) => cli::interrupt_pane(&cli.session, &pane).await,
        Some(Commands::TerminatePane { pane }) => cli::terminate_pane(&cli.session, &pane).await,
        Some(Commands::ClosePane { pane }) => cli::close_pane(&cli.session, &pane).await,
        Some(Commands::SetPaneMeta { pane, alias, role }) => {
            cli::set_pane_meta(&cli.session, &pane, alias, role).await
        }
        Some(Commands::CapturePane {
            target,
            lines,
            plain_text,
            json,
        }) => cli::capture_pane(&cli.session, &target, lines, plain_text, json).await,
        Some(Commands::SplitPane {
            dir,
            direction,
            target,
        }) => {
            let split_dir = match direction {
                SplitDirectionArg::Vertical => yatamux_protocol::types::SplitDirection::Vertical,
                SplitDirectionArg::Horizontal => {
                    yatamux_protocol::types::SplitDirection::Horizontal
                }
            };
            cli::split_pane(&cli.session, target.as_deref(), split_dir, dir).await
        }
        Some(Commands::Layout(sub)) => match sub {
            LayoutCommands::List => cli::layout_list().await,
            LayoutCommands::Delete { name } => cli::layout_delete(&name).await,
            LayoutCommands::Export { name } => cli::layout_export(&name).await,
        },
        Some(Commands::Update) => cli::update(&cli.session).await,
        Some(Commands::Source { path }) => cli::source_config(&path).await,
        None => {
            let app_config =
                config::AppConfig::load(&config::AppConfig::default_path()).unwrap_or_default();
            app::run(cli.session, cli.layout, app_config).await
        }
    }
}

/// `--apply-update <pid> <new_path> [--launch]` ヘルパーモード
///
/// 1. 指定 PID のプロセス終了を待つ（pid=0 の場合はスキップ）
/// 2. `<exe>` を `<exe>.bak` にリネーム
/// 3. `<new_path>` を `<exe>` にリネーム
/// 4. `--launch` が指定されている場合のみ新しい exe を起動
async fn apply_update(pid: u32, new_path: &std::path::Path, launch: bool) -> anyhow::Result<()> {
    use anyhow::Context;

    let exe = std::env::current_exe().context("現在の実行ファイルのパスが取得できません")?;
    if let Ok(probe_path) = std::env::var("YATAMUX_UPDATE_HELPER_PROBE_FILE") {
        let payload = serde_json::json!({
            "pid": pid,
            "new_path": new_path,
            "launch": launch,
            "session": std::env::var("YATAMUX_SESSION").ok(),
            "exe": exe,
        });
        std::fs::write(&probe_path, serde_json::to_vec_pretty(&payload)?).with_context(|| {
            format!(
                "ヘルパー probe ファイルの書き込みに失敗しました: {}",
                probe_path
            )
        })?;
        return Ok(());
    }

    if pid != 0 {
        eprintln!("PID {} の終了を待機中...", pid);
    }
    if launch {
        eprintln!("PID {} の終了後に新しいインスタンスを起動します。", pid);
    }

    update::apply_staged_update(
        &exe,
        pid,
        new_path,
        launch,
        std::time::Duration::from_secs(30),
    )?;

    if pid != 0 {
        eprintln!("PID {} が終了しました。バイナリを置換しました。", pid);
    }

    if launch {
        eprintln!("バイナリ置換完了。新しいインスタンスを起動しました。");
    } else {
        eprintln!("バイナリ置換完了。");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use toml::Value;

    #[test]
    fn parse_send_keys_wait_for_prompt() {
        let cli = Cli::try_parse_from([
            "yatamux",
            "send-keys",
            "--pane",
            "1",
            "--wait-for-prompt",
            "echo hi",
        ])
        .expect("CLI should parse");

        match cli.command {
            Some(Commands::SendKeys {
                pane,
                text,
                wait_for_prompt,
                ..
            }) => {
                assert_eq!(pane, "1");
                assert_eq!(text, "echo hi");
                assert!(wait_for_prompt);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_capture_pane_json() {
        let cli = Cli::try_parse_from([
            "yatamux",
            "capture-pane",
            "--target",
            "1",
            "--lines",
            "20",
            "--json",
        ])
        .expect("CLI should parse");

        match cli.command {
            Some(Commands::CapturePane {
                target,
                lines,
                json,
                ..
            }) => {
                assert_eq!(target, "1");
                assert_eq!(lines, 20);
                assert!(json);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_interrupt_pane() {
        let cli = Cli::try_parse_from(["yatamux", "interrupt-pane", "--pane", "7"])
            .expect("CLI should parse");

        match cli.command {
            Some(Commands::InterruptPane { pane }) => {
                assert_eq!(pane, "7");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_wait_pane_exit() {
        let cli =
            Cli::try_parse_from(["yatamux", "wait-pane", "--pane", "3", "--wait-for", "exit"])
                .expect("CLI should parse");

        match cli.command {
            Some(Commands::WaitPane { pane, wait_for, .. }) => {
                assert_eq!(pane, "3");
                assert_eq!(wait_for, WaitForArg::Exit);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_wait_pane_output_regex() {
        let cli = Cli::try_parse_from([
            "yatamux",
            "wait-pane",
            "--pane",
            "2",
            "--wait-for",
            "output-regex",
            "--output-regex",
            "passed",
            "--lines",
            "300",
        ])
        .expect("CLI should parse");

        match cli.command {
            Some(Commands::WaitPane {
                pane,
                wait_for,
                output_regex,
                lines,
                ..
            }) => {
                assert_eq!(pane, "2");
                assert_eq!(wait_for, WaitForArg::OutputRegex);
                assert_eq!(output_regex.as_deref(), Some("passed"));
                assert_eq!(lines, 300);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_exec_command() {
        let cli = Cli::try_parse_from([
            "yatamux",
            "exec",
            "--pane",
            "1",
            "--timeout",
            "30",
            "--",
            "cargo",
            "test",
            "-q",
        ])
        .expect("CLI should parse");

        match cli.command {
            Some(Commands::Exec {
                pane,
                timeout,
                command,
                ..
            }) => {
                assert_eq!(pane, "1");
                assert_eq!(timeout, 30);
                assert_eq!(command, vec!["cargo", "test", "-q"]);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_global_session_option() {
        let cli = Cli::try_parse_from(["yatamux", "--session", "e2e-smoke", "list-panes"])
            .expect("CLI should parse");

        assert_eq!(cli.session, "e2e-smoke");
        assert!(matches!(cli.command, Some(Commands::ListPanes { .. })));
    }

    #[test]
    fn parse_close_pane() {
        let cli = Cli::try_parse_from(["yatamux", "close-pane", "--pane", "9"])
            .expect("CLI should parse");

        match cli.command {
            Some(Commands::ClosePane { pane }) => {
                assert_eq!(pane, "9");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_terminate_pane() {
        let cli = Cli::try_parse_from(["yatamux", "terminate-pane", "--pane", "11"])
            .expect("CLI should parse");

        match cli.command {
            Some(Commands::TerminatePane { pane }) => {
                assert_eq!(pane, "11");
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_subscribe_pane() {
        let cli = Cli::try_parse_from(["yatamux", "subscribe-pane", "--pane", "tests", "--json"])
            .expect("CLI should parse");

        match cli.command {
            Some(Commands::SubscribePane { pane, json }) => {
                assert_eq!(pane, "tests");
                assert!(json);
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn parse_set_pane_meta() {
        let cli = Cli::try_parse_from([
            "yatamux",
            "set-pane-meta",
            "--pane",
            "tests",
            "--alias",
            "tests",
            "--role",
            "verifier",
        ])
        .expect("CLI should parse");

        match cli.command {
            Some(Commands::SetPaneMeta { pane, alias, role }) => {
                assert_eq!(pane, "tests");
                assert_eq!(alias.as_deref(), Some("tests"));
                assert_eq!(role.as_deref(), Some("verifier"));
            }
            _ => panic!("unexpected command"),
        }
    }

    #[test]
    fn workspace_and_member_crates_share_workspace_version() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
        let manifests = [
            root.join("Cargo.toml"),
            root.join("crates/client/Cargo.toml"),
            root.join("crates/server/Cargo.toml"),
            root.join("crates/protocol/Cargo.toml"),
            root.join("crates/terminal/Cargo.toml"),
            root.join("crates/renderer/Cargo.toml"),
        ];

        for manifest_path in manifests {
            let text = std::fs::read_to_string(&manifest_path)
                .unwrap_or_else(|e| panic!("failed to read {}: {e}", manifest_path.display()));
            let value: Value = text
                .parse()
                .unwrap_or_else(|e| panic!("failed to parse {}: {e}", manifest_path.display()));
            let package = value
                .get("package")
                .and_then(Value::as_table)
                .unwrap_or_else(|| panic!("missing [package] in {}", manifest_path.display()));
            let version = package
                .get("version")
                .and_then(Value::as_table)
                .unwrap_or_else(|| {
                    panic!(
                        "package.version should be a table in {}",
                        manifest_path.display()
                    )
                });
            assert_eq!(
                version.get("workspace").and_then(Value::as_bool),
                Some(true),
                "{} should use package.version.workspace = true",
                manifest_path.display()
            );
        }
    }
}
