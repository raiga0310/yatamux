# yatamux

**CJK-first terminal multiplexer for Windows, built with Rust + Win32 GDI + ConPTY.**

[English](#english) · [日本語](#日本語)

---

## English

### What is this?

yatamux is a Windows-native terminal multiplexer designed for CJK (Chinese/Japanese/Korean) users and AI coding workflows (e.g. Claude Code, lazygit).

Modern agent-oriented terminal apps like [ghostty](https://github.com/ghostty-org/ghostty) and [cmux-windows](https://github.com/mkurman/cmux-windows) are primarily built for **macOS / Linux**, or their Windows ports have notable gaps:

- **CJK character width miscalculation** — kanji/kana/hangul are treated as 1-cell wide, causing cursor misalignment
- **Incomplete or broken IME support** — preedit strings overlap or corrupt the display
- **Half-width voiced katakana (U+FF9E/FF9F)** — misidentified as zero-width combining marks
- **Box-drawing character rendering** — font-dependent glyphs cause misaligned borders in neovim and similar UIs

yatamux addresses these on Windows by using native APIs directly: ConPTY for PTY, Win32 GDI for rendering, and IMM32 for Japanese input.

### Features

| Feature | Detail |
|---------|--------|
| **CJK width calculation** | UAX #11 compliant + custom override table. Does not trust ConPTY cursor position. |
| **IME support** | WM_IME_COMPOSITION preedit display, committed text sent as UTF-8 to PTY |
| **Pane splitting** | Binary tree layout — `Ctrl+Shift+E` (vertical) / `Ctrl+Shift+O` (horizontal) |
| **Box-drawing characters** | U+2500–259F rendered via GDI primitives (font-independent) |
| **Font auto-selection** | HackGen Console NF → HackGen Console → Cascadia Mono/Code → MS Gothic (fallback) |
| **Color theme** | Catppuccin Mocha (bg `#1e1e2e`, fg `#cdd6f4`, cursor `#f5c2e7`) |
| **60 fps rendering** | GDI double-buffered, dirty-line differential repaint |
| **Dark title bar** | DWM `DWMWA_USE_IMMERSIVE_DARK_MODE` |
| **OSC 52 clipboard** | Cross-SSH clipboard write via `\x1b]52;c;<base64>\x07` escape sequence |
| **External IPC** | Named pipe `\\.\pipe\yatamux-<session>` for CLI / agent integration |
| **Tested with** | vim, lazygit, Claude Code |

### Requirements

- Windows 10 version 1903 (Build 18362) or later (required for ConPTY API)
- Windows 11 recommended
- [Rust toolchain](https://rustup.rs/) (stable, MSVC target)

### Install

```powershell
cargo install --path .
```

This builds in release mode and installs `yatamux.exe` to `~/.cargo/bin` (already on `PATH` after `rustup` setup).

### Build (without installing)

```powershell
git clone https://github.com/raiga0310/yatamux
cd yatamux
cargo build --release
```

The release binary is at `target/release/yatamux.exe`.
Double-click to launch — no console window appears in release builds.

For debug builds with logging:

```powershell
$env:RUST_LOG="info"; cargo run
```

### Keybindings

| Key | Action |
|-----|--------|
| `Ctrl+Shift+E` | Split pane vertically (left/right) |
| `Ctrl+Shift+O` | Split pane horizontally (top/bottom) |
| `Ctrl+→` / `Ctrl+↓` | Focus next pane |
| `Ctrl+←` / `Ctrl+↑` | Focus previous pane |
| `Ctrl+Tab` | Focus next pane |
| `Ctrl+Shift+Tab` | Focus previous pane |

### Toast Notifications

yatamux shows Steam-style toast notifications in the bottom-right corner when a background pane has something to report.

**Triggers (background panes only):**

| Trigger | How to enable |
|---------|---------------|
| `BEL` (`\x07`) output | Always works — many CLI tools (e.g. `make`, test runners) emit BEL on completion |
| OSC 9: `\x1b]9;message\x07` | Application emits this explicitly |
| OSC 133;D (shell integration) | Configure your shell to emit `\x1b]133;D\x07` after each command (see below) |
| Process exit | Automatic — fires when the process running in the pane exits |

**Shell integration setup (OSC 133;D):**

For **bash / Git Bash / WSL bash**, add to `~/.bashrc`:
```bash
PROMPT_COMMAND='printf "\x1b]133;D\x07"'
```

For **PowerShell**, add to `$PROFILE`:
```powershell
function prompt {
    [Console]::Write("`e]133;D`a")
    "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
}
```

### Architecture

```
yatamux (bin)
├── yatamux-server    PTY lifecycle management, session tree (Workspace → Surface → Pane)
├── yatamux-client    Win32 window, GDI rendering, IME handler, layout calculation
├── yatamux-protocol  Shared message types (ClientMessage / ServerMessage)
├── yatamux-terminal  VT parser, grid state machine, CJK width table, ConPTY wrapper
└── yatamux-renderer  Debug text renderer (Phase 2: GPU via wgpu, planned)
```

Server and client run **in-process** connected by `tokio::sync::mpsc` channels.
An IPC server (`\\.\pipe\yatamux-<session>`) also starts automatically for external CLI / agent access.

### Known Limitations

- Pane split ratio is fixed at 50:50 (drag-to-resize not yet implemented)
- No scrollback buffer
- Windows 10 1903+ required (ConPTY)

### Roadmap

- [ ] Pane resize by keyboard (`Alt+Shift+←→↑↓`) or mouse drag
- [ ] Scrollback buffer
- [x] Session persistence (`%APPDATA%\yatamux\session.toml`)

### License

MIT — see [LICENSE](LICENSE)

---

## 日本語

### これは何か

yatamux は、Windows ネイティブな CJK 対応ターミナルマルチプレクサです。
AI コーディングワークフロー（Claude Code・lazygit など）での利用を想定して設計されています。

[ghostty](https://github.com/ghostty-org/ghostty) や [cmux-windows](https://github.com/mkurman/cmux-windows) などの、
エージェント向けに設計されたモダンなターミナルアプリは **macOS / Linux 向け** に開発されており、
Windows 移植版では以下の課題がありました:

- **CJK 文字幅の不正確な計算** — 漢字・かな・ハングルが 1 セル幅として扱われ、カーソルがずれる
- **IME（日本語入力）の不完全対応** — プリエディット文字列の表示が崩れる
- **半角カタカナ濁点 (U+FF9E/FF9F)** — 結合マークと誤認識され幅計算が狂う
- **罫線文字のフォント依存** — neovim などのボックス UI がフォントによっては崩れる

yatamux は ConPTY・Win32 GDI・IMM32 を直接使い、Windows ネイティブにこれらを解決します。

### 主な機能

| 機能 | 説明 |
|------|------|
| **CJK 幅計算** | UAX #11 準拠 + 独自オーバーライドテーブル。ConPTY のカーソル位置は使用しない |
| **IME 対応** | WM_IME_COMPOSITION でプリエディット表示、確定文字列を UTF-8 で PTY に送信 |
| **ペイン分割** | バイナリツリーレイアウト。`Ctrl+Shift+E`（縦）/ `Ctrl+Shift+O`（横） |
| **罫線文字** | U+2500–259F を GDI プリミティブで直接描画（フォント非依存） |
| **フォント自動選択** | HackGen Console NF → HackGen Console → Cascadia Mono/Code → MS Gothic（フォールバック） |
| **カラーテーマ** | Catppuccin Mocha（背景 `#1e1e2e`、前景 `#cdd6f4`、カーソル `#f5c2e7`） |
| **60fps 描画** | GDI ダブルバッファ、ダーティライン差分再描画 |
| **ダークタイトルバー** | DWM `DWMWA_USE_IMMERSIVE_DARK_MODE` |
| **OSC 52 クリップボード** | `\x1b]52;c;<base64>\x07` による SSH 越しクリップボード書き込み |
| **外部 IPC** | `\\.\pipe\yatamux-<session>` で CLI・エージェントからの操作を受け付け |
| **動作確認済み** | vim、lazygit、Claude Code |

### 動作要件

- Windows 10 バージョン 1903 (Build 18362) 以降（ConPTY API の要件）
- Windows 11 推奨
- [Rust ツールチェーン](https://rustup.rs/)（stable、MSVC ターゲット）

### インストール

```powershell
cargo install --path .
```

リリースビルドで `~/.cargo/bin/yatamux.exe` にインストールされます（`rustup` セットアップ済みなら PATH に含まれています）。

### ビルド（インストールせずに試す場合）

```powershell
git clone https://github.com/raiga0310/yatamux
cd yatamux
cargo build --release
```

リリースバイナリは `target/release/yatamux.exe` に生成されます。
ダブルクリックで起動できます（リリースビルドではコンソールウィンドウは表示されません）。

ログ付きで実行する場合（デバッグビルド）:

```powershell
$env:RUST_LOG="info"; cargo run
```

### キーバインド

| キー | 動作 |
|------|------|
| `Ctrl+Shift+E` | 縦分割（左右） |
| `Ctrl+Shift+O` | 横分割（上下） |
| `Ctrl+→` / `Ctrl+↓` | 次のペインにフォーカス |
| `Ctrl+←` / `Ctrl+↑` | 前のペインにフォーカス |
| `Ctrl+Tab` | 次のペインにフォーカス |
| `Ctrl+Shift+Tab` | 前のペインにフォーカス |

### トースト通知

バックグラウンドペインで何か通知すべきことがあると、画面右下に Steam 風のトースト通知が表示されます。

**通知が出るトリガー（バックグラウンドペインのみ）:**

| トリガー | 有効にする方法 |
|---------|--------------|
| `BEL`（`\x07`）出力 | 常に有効。`make` やテストランナーなど多くの CLI が完了時に BEL を出す |
| OSC 9: `\x1b]9;メッセージ\x07` | アプリが明示的に出力する |
| OSC 133;D（シェルインテグレーション） | シェルにコマンド終了後のシーケンス出力を設定する（下記参照） |
| プロセス終了 | 自動。ペイン内のプロセスが終了すると通知が出る |

**シェルインテグレーション設定（OSC 133;D）:**

**bash / Git Bash / WSL bash** の場合、`~/.bashrc` に追加:
```bash
PROMPT_COMMAND='printf "\x1b]133;D\x07"'
```

**PowerShell** の場合、`$PROFILE` に追加:
```powershell
function prompt {
    [Console]::Write("`e]133;D`a")
    "PS $($executionContext.SessionState.Path.CurrentLocation)$('>' * ($nestedPromptLevel + 1)) "
}
```

### アーキテクチャ

```
yatamux (bin)
├── yatamux-server    PTY 管理・セッションツリー（Workspace → Surface → Pane）
├── yatamux-client    Win32 ウィンドウ・GDI レンダリング・IME ハンドラ・レイアウト計算
├── yatamux-protocol  共有メッセージ型（ClientMessage / ServerMessage）
├── yatamux-terminal  VT パーサ・グリッド・CJK 幅テーブル・ConPTY ラッパー
└── yatamux-renderer  デバッグ用テキストレンダラー（フェーズ 2: wgpu GPU 化予定）
```

サーバーとクライアントは **同一プロセス内** で `tokio::sync::mpsc` チャネルにより直結しています。
起動時に IPC サーバー（`\\.\pipe\yatamux-<session>`）も常時起動し、外部 CLI・エージェントからの操作を受け付けます。

### 既知の制限

- ペイン分割比は 50:50 固定（ドラッグリサイズ未実装）
- スクロールバック未実装
- Windows 10 1903 以降が必要（ConPTY）

### ロードマップ

- [ ] ペインリサイズ（`Alt+Shift+←→↑↓` またはマウスドラッグ）
- [ ] スクロールバックバッファ
- [x] セッション永続化（`%APPDATA%\yatamux\session.toml`）

### ライセンス

MIT — [LICENSE](LICENSE) を参照
