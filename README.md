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
| **Copy mode** | `V` in Pane mode → Copy mode. `hjkl`/arrows to move cursor, `v` to start selection, `y`/Enter to yank to clipboard |
| **Mouse selection** | Left-drag to select text; releases to clipboard automatically |
| **External IPC** | Named pipe `\\.\pipe\yatamux-<session>` for CLI / agent integration |
| **CLI tools** | `list-panes --json`, `set-pane-meta`, `send-keys --raw/--enter/--wait-for-prompt`, `wait-pane`, `exec`, `subscribe-pane`, `interrupt-pane`, `terminate-pane`, `close-pane`, `capture-pane --plain-text/--json`, `split-pane`, `layout list/export/delete` |
| **Scrollback buffer** | Up to 50,000 lines; mouse-wheel scroll; open in `$EDITOR` via Pane mode `X` |
| **Floating pane** | Overlay pane on top of the tiled layout (`Ctrl+F` to toggle) |
| **Pane mode** | `Ctrl+B` enters Pane mode — status bar shows context-sensitive keybind hints |
| **Theme launcher** | `Ctrl+P` opens a theme picker; runtime color switching without restart |
| **Session persistence** | Layout auto-saved to `%APPDATA%\yatamux\session.toml` on exit, restored on startup |
| **Declarative layouts** | `--layout <name>` loads `%APPDATA%\yatamux\layouts\<name>.toml` for project startup |
| **Layout save/manage** | `S` in Pane mode saves the current layout; `yatamux layout list/export/delete` manages saved layouts |
| **Layout launcher** | `L` in Pane mode opens an in-app layout picker with a live split-diagram preview |
| **Pane resize** | `<` / `>` adjusts vertical splits, `+` / `-` adjusts horizontal splits in 5 % steps |
| **Notifications** | Focused app uses in-window toasts; unfocused app falls back to native Windows balloon notifications |
| **Plugin hooks** | `%APPDATA%\yatamux\config.toml` `[hooks]` — `on_pane_created` / `on_pane_closed` shell commands |
| **Click to focus** | Left-click on any pane to focus it |
| **ZWJ / emoji / BiDi** | ZWJ sequences (👨‍💻), VS-16 widening, Nerd Fonts wide-glyph option, BiDi control chars (zero-width) |
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

**Normal mode:**

| Key | Action |
|-----|--------|
| `Ctrl+Shift+E` | Split pane vertically (left/right) |
| `Ctrl+Shift+O` | Split pane horizontally (top/bottom) |
| `Ctrl+Shift+W` | Close active pane, or exit the app if it is the last pane |
| `Ctrl+→` / `Ctrl+↓` | Focus next pane |
| `Ctrl+←` / `Ctrl+↑` | Focus previous pane |
| `Ctrl+Tab` | Focus next pane |
| `Ctrl+Shift+Tab` | Focus previous pane |
| `Ctrl+F` | Toggle floating pane |
| `Ctrl+P` | Open theme launcher |
| `Ctrl+B` | Enter Pane mode |
| Left click | Focus clicked pane |
| Mouse wheel | Scroll scrollback buffer |

**Pane mode** (entered with `Ctrl+B`, status bar shows hint):

| Key | Action |
|-----|--------|
| `E` | Split vertically |
| `O` | Split horizontally |
| `W` | Close active pane |
| `F` | Toggle floating pane |
| `X` | Open scrollback in `$EDITOR` |
| `S` | Open save-layout prompt |
| `<` / `>` | Adjust vertical split ratio by ±5 % |
| `+` / `-` | Adjust horizontal split ratio by ±5 % |
| `L` | Open layout launcher (pick & apply a saved layout) |
| `V` | Enter Copy mode |
| `q` / `Esc` | Return to Normal mode |

**Copy mode** (entered with `V` in Pane mode):

| Key | Action |
|-----|--------|
| `h` / `←` | Move cursor left |
| `j` / `↓` | Move cursor down |
| `k` / `↑` | Move cursor up |
| `l` / `→` | Move cursor right |
| `v` | Toggle selection (visual mode) |
| `y` / `Enter` | Yank selected text to clipboard, exit Copy mode |
| `q` / `Esc` | Exit Copy mode |

### CLI / Agent Integration

When yatamux is running, you can control it from any shell or AI agent via the named pipe IPC:

```powershell
# List all panes as JSON
yatamux list-panes --json

# Send a command and wait for OSC 133;D command completion
yatamux send-keys --pane 1 --enter --wait-for-prompt "cargo test"

# Wait until a pane becomes quiet for 2 seconds
yatamux wait-pane --pane 1 --wait-for silence --silence-ms 2000

# Run a command and wait for a regex to appear in capture-pane output
yatamux exec --pane 1 --wait-for output-regex --output-regex "test result: ok" -- cargo test

# Stream live output updates from a pane as JSON Lines
yatamux subscribe-pane --pane tests --json

# Assign an alias / role and then use the alias instead of the numeric pane ID
yatamux set-pane-meta --pane 1 --alias tests --role verifier
yatamux send-keys --pane tests --enter "cargo test -q"

# Interrupt a running job with Ctrl+C
yatamux interrupt-pane --pane 1

# Force-terminate the pane process and wait for PaneClosed
yatamux terminate-pane --pane 1

# Close a pane explicitly
yatamux close-pane --pane 2

# Capture pane output as plain text
yatamux capture-pane --target 1 --lines 200 --plain-text

# Capture pane output with structured metadata
yatamux capture-pane --target 1 --lines 200 --json

# Split a pane, optionally in a different working directory
yatamux split-pane --target 1 --direction vertical --dir C:\projects\other-repo

# Manage saved layouts
yatamux layout list
yatamux layout export work
yatamux layout delete work
```

These commands connect to the running `yatamux` instance via `\\.\pipe\yatamux-default`.

`list-panes --json` includes server-side pane metadata that is useful before sending input:

- `cwd`: current working directory when it can be discovered from the pane process
- `command`: active child command when one is running outside the shell
- `busy`: coarse job-running flag that flips true after input and false on command-finished notification
- `last_output_unix_ms`: last observed pane output time in Unix epoch milliseconds
- `active`: whether the GUI currently considers the pane focused
- `floating`: whether the pane is currently shown as the floating overlay

`wait-pane` supports three conditions:

- `--wait-for exit`: wait for `CommandFinished` or `PaneClosed`
- `--wait-for silence --silence-ms <ms>`: wait until no new pane output is observed for the given duration
- `--wait-for output-regex --output-regex <pattern>`: poll `capture-pane --plain-text` and match the regex against captured content

`exec` sends the given command with an automatic Enter and uses the same wait conditions as `wait-pane`, but it now runs as a single IPC request with `request_id` correlation. `send-keys --wait-for-prompt`, `close-pane`, and `terminate-pane` also use the same internal pane wait substrate, so timeout and event handling stay aligned.

`subscribe-pane` provides a live event stream without `capture-pane` polling. The default mode writes raw output chunks to stdout. `--json` switches to JSON Lines events such as `output`, `notification`, `command_finished`, `pane_closed`, and `lagged`.

`--pane` / `--target` accept either a numeric pane ID or an alias set by `set-pane-meta`. `list-panes --json` also includes optional `alias` / `role` fields so agents can pick panes by logical name instead of ephemeral IDs.

### Claude Code Orchestration

The repository includes a Claude Code oriented orchestration bundle under `integrations/claude-code/`.

- `integrations/claude-code/SKILL.md`: the core pane orchestration workflow
- `integrations/claude-code/scripts/new-worker-pane.ps1`: create a pane, assign alias / role, and optionally bootstrap it
- `integrations/claude-code/scripts/watch-pane.ps1`: stream live output with `subscribe-pane` or take a snapshot with `capture-pane`

Example flow:

```powershell
pwsh -File integrations/claude-code/scripts/new-worker-pane.ps1 `
  -Alias tests `
  -Role verifier `
  -Dir C:\src\repo `
  -BootstrapCommand "claude --continue"

yatamux send-keys --pane tests --enter --raw "Run the focused test suite and summarize failures."

pwsh -File integrations/claude-code/scripts/watch-pane.ps1 -Pane tests -Json
```

This keeps the main pane clean while the worker runs in a labeled pane that can be monitored, interrupted, or closed independently.

The pane control commands are intentionally distinct:

- `interrupt-pane`: send `Ctrl+C` and leave the pane open
- `terminate-pane`: force-kill the pane process and wait for `PaneClosed`
- `close-pane`: close the pane itself; if a process is still attached, it is torn down with the pane

`capture-pane --plain-text` keeps the legacy text dump behavior for scripts and copy/paste. `capture-pane --json` returns the same `content` plus structured metadata:

```json
{
  "pane": 1,
  "content": "prompt\ncurrent screen",
  "title": "pwsh",
  "cols": 80,
  "rows": 24,
  "lines_requested": 200,
  "scrollback_len": 512,
  "cursor": { "col": 12, "row": 23, "visible": true },
  "visible_text": ["current screen"],
  "scrollback_tail": ["prompt"]
}
```

- `content`: scrollback tail plus the current visible screen, joined as plain text
- `scrollback_tail`: only the scrollback portion
- `visible_text`: one string per currently visible row

### Toast Notifications

yatamux uses a focus-aware notification backend:

- When the app is focused, notifications are rendered as in-window toast popups.
- When the app is unfocused, notifications are forwarded to native Windows balloon notifications.

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

- Windows 10 1903+ required (ConPTY)

### Roadmap

- [ ] Remote monitoring via WebSocket bridge (read-only preview from browser / mobile)
- [x] Claude Code integration skill (prompt scaffolding + wrapper scripts for AI orchestration)
- [x] Copy mode — keyboard text selection + clipboard yank (`V` in Pane mode)
- [x] `capture-pane` CLI — AI-readable pane content dump
- [x] `split-pane --dir` CLI — open a new pane in any working directory
- [x] Auto-close pane when shell exits
- [x] Pane resize by keyboard (`<` / `>` in Pane mode, ±5 % ratio)
- [x] In-app layout launcher (`L` in Pane mode, with split-diagram preview)
- [x] Scrollback buffer (50,000 lines, mouse-wheel scroll, open in `$EDITOR`)
- [x] Session persistence (`%APPDATA%\yatamux\session.toml`)
- [x] Floating pane (`Ctrl+F`)
- [x] Pane mode with status bar hints
- [x] Declarative layout (`--layout <name>`)
- [x] Plugin hooks (`config.toml` `[hooks]`)

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
| **コピーモード** | ペインモード `V` でコピーモードへ。`hjkl`/矢印でカーソル移動、`v` で選択開始、`y`/Enter でヤンク |
| **マウス選択** | 左ドラッグでテキスト選択。離した瞬間にクリップボードへコピー |
| **外部 IPC** | `\\.\pipe\yatamux-<session>` で CLI・エージェントからの操作を受け付け |
| **CLI ツール** | `list-panes --json`、`set-pane-meta`、`send-keys --raw/--enter/--wait-for-prompt`、`wait-pane`、`exec`、`subscribe-pane`、`interrupt-pane`、`terminate-pane`、`close-pane`、`capture-pane --plain-text/--json`、`split-pane`、`layout list/export/delete` |
| **スクロールバック** | 最大 50,000 行。マウスホイールでスクロール。ペインモード `X` で `$EDITOR` 起動 |
| **フローティングペイン** | タイルレイアウトの上に重なるオーバーレイペイン（`Ctrl+F` でトグル） |
| **ペインモード** | `Ctrl+B` でペインモードへ。ステータスバーにキーバインドヒントを表示 |
| **テーマランチャー** | `Ctrl+P` でテーマ選択 UI を開き、色テーマを即時切り替え |
| **セッション永続化** | 終了時に `%APPDATA%\yatamux\session.toml` へ自動保存、起動時に復元 |
| **宣言的レイアウト** | `--layout <name>` で `%APPDATA%\yatamux\layouts\<name>.toml` をプロジェクト起動に活用 |
| **レイアウト保存/管理** | ペインモード `S` で現在構成を保存。`yatamux layout list/export/delete` で管理 |
| **レイアウトランチャー** | ペインモード `L` でアプリ内レイアウト選択 UI を表示（分割図プレビュー付き） |
| **ペインリサイズ** | ペインモード `<` / `>` で縦分割比、`+` / `-` で横分割比を 5 % 単位で増減 |
| **通知バックエンド** | フォーカス中はアプリ内トースト、非フォーカス時は Windows ネイティブ通知へ切り替え |
| **プラグインフック** | `config.toml` の `[hooks]` で `on_pane_created` / `on_pane_closed` シェルコマンドを設定 |
| **クリックフォーカス** | ペイン領域を左クリックしてフォーカス移動 |
| **ZWJ / 絵文字 / BiDi** | ZWJ シーケンス（👨‍💻）、VS-16 幅拡張、Nerd Fonts ワイドグリフ、BiDi 制御文字（幅0扱い） |
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

**ノーマルモード:**

| キー | 動作 |
|------|------|
| `Ctrl+Shift+E` | 縦分割（左右） |
| `Ctrl+Shift+O` | 横分割（上下） |
| `Ctrl+Shift+W` | アクティブペインを閉じる。最後の1枚ならアプリ終了 |
| `Ctrl+→` / `Ctrl+↓` | 次のペインにフォーカス |
| `Ctrl+←` / `Ctrl+↑` | 前のペインにフォーカス |
| `Ctrl+Tab` | 次のペインにフォーカス |
| `Ctrl+Shift+Tab` | 前のペインにフォーカス |
| `Ctrl+F` | フローティングペインのトグル |
| `Ctrl+P` | テーマランチャーを開く |
| `Ctrl+B` | ペインモードへ移行 |
| 左クリック | クリックしたペインにフォーカス |
| マウスホイール | スクロールバックをスクロール |

**ペインモード**（`Ctrl+B` で移行、ステータスバーにヒント表示）:

| キー | 動作 |
|------|------|
| `E` | 縦分割 |
| `O` | 横分割 |
| `W` | アクティブペインを閉じる |
| `F` | フローティングペインのトグル |
| `X` | スクロールバックを `$EDITOR` で開く |
| `S` | レイアウト保存プロンプトを開く |
| `<` / `>` | 縦分割比を ±5 % 調整 |
| `+` / `-` | 横分割比を ±5 % 調整 |
| `L` | レイアウトランチャーを開く（保存済みレイアウトを選択・適用） |
| `V` | コピーモードに入る |
| `q` / `Esc` | ノーマルモードに戻る |

**コピーモード**（ペインモードで `V` を押して移行）:

| キー | 動作 |
|------|------|
| `h` / `←` | カーソルを左に移動 |
| `j` / `↓` | カーソルを下に移動 |
| `k` / `↑` | カーソルを上に移動 |
| `l` / `→` | カーソルを右に移動 |
| `v` | 選択トグル（ビジュアルモード） |
| `y` / `Enter` | 選択テキストをクリップボードにコピーしてコピーモードを終了 |
| `q` / `Esc` | コピーモードを終了 |

### CLI / エージェント連携

yatamux 起動中は、任意のシェルや AI エージェントから名前付きパイプ IPC を通じて操作できます:

```powershell
# ペイン一覧を JSON で表示
yatamux list-panes --json

# 指定ペインにコマンドを送信し、OSC 133;D まで待機
yatamux send-keys --pane 1 --enter --wait-for-prompt "cargo test"

# 2 秒間出力が止まるまで待機
yatamux wait-pane --pane 1 --wait-for silence --silence-ms 2000

# コマンドを送って、capture-pane 上で正規表現に一致するまで待機
yatamux exec --pane 1 --wait-for output-regex --output-regex "test result: ok" -- cargo test

# ペインのライブ出力を JSON Lines で購読
yatamux subscribe-pane --pane tests --json

# alias / role を付けてから、数値 ID の代わりに alias で操作
yatamux set-pane-meta --pane 1 --alias tests --role verifier
yatamux send-keys --pane tests --enter "cargo test -q"

# 実行中ジョブへ Ctrl+C を送る
yatamux interrupt-pane --pane 1

# ペインのプロセスを強制終了し、PaneClosed まで待つ
yatamux terminate-pane --pane 1

# ペインを明示的に閉じる
yatamux close-pane --pane 2

# ペインの内容をプレーンテキストで取得
yatamux capture-pane --target 1 --lines 200 --plain-text

# ペインの内容を構造化 JSON で取得
yatamux capture-pane --target 1 --lines 200 --json

# ペインを分割。--dir で別リポジトリの作業ディレクトリを指定可能
yatamux split-pane --target 1 --direction vertical --dir C:\projects\other-repo

# 保存済みレイアウトを管理
yatamux layout list
yatamux layout export work
yatamux layout delete work
```

接続先: `\\.\pipe\yatamux-default`

`list-panes --json` には、入力送信前の判断に使える server 側メタデータも含まれます。

- `cwd`: 取得できた場合の現在作業ディレクトリ
- `command`: シェル以外で実行中の子コマンド名
- `busy`: 入力送信後からコマンド完了通知までを表す粗めの実行中フラグ
- `last_output_unix_ms`: 最後にそのペインから出力を観測した Unix epoch ミリ秒
- `active`: GUI 上で現在フォーカスされているペインかどうか
- `floating`: 現在フローティングオーバーレイとして表示されているペインかどうか

`wait-pane` は次の待機条件に対応します。

- `--wait-for exit`: `CommandFinished` または `PaneClosed` を待つ
- `--wait-for silence --silence-ms <ms>`: 指定時間だけ新しい出力が来ない状態を待つ
- `--wait-for output-regex --output-regex <pattern>`: `capture-pane --plain-text` の内容に正規表現が一致するまで待つ

`exec` はコマンド送信時に自動で Enter を付け、`wait-pane` と同じ待機条件を使えます。内部的には `request_id` 付きの単一 IPC request として処理されるため、複数の `exec` を同時に流しても結果を相関できます。`send-keys --wait-for-prompt`、`close-pane`、`terminate-pane` も同じ内部待機基盤を使うので、タイムアウトやイベント解釈の挙動がそろいます。

`subscribe-pane` は `capture-pane` のポーリングなしでライブ監視できる購読コマンドです。既定では生の出力チャンクを stdout に流し、`--json` を付けると `output` / `notification` / `command_finished` / `pane_closed` / `lagged` の JSON Lines を出力します。

`--pane` / `--target` には数値 ID だけでなく、`set-pane-meta` で付けた alias も使えます。`list-panes --json` には `alias` / `role` も含まれるので、エージェントは変動しやすい pane ID ではなく論理名で対象を選べます。

### Claude Code 連携

`integrations/claude-code/` に、Claude Code が yatamux を worker pane オーケストレーションに使うための素材を同梱しています。

- `integrations/claude-code/SKILL.md`: 基本フローと運用ルール
- `integrations/claude-code/scripts/new-worker-pane.ps1`: 新しい worker pane の作成、alias / role 付与、初期コマンド送信
- `integrations/claude-code/scripts/watch-pane.ps1`: `subscribe-pane` によるライブ監視、または `capture-pane` によるスナップショット取得

例:

```powershell
pwsh -File integrations/claude-code/scripts/new-worker-pane.ps1 `
  -Alias tests `
  -Role verifier `
  -Dir C:\src\repo `
  -BootstrapCommand "claude --continue"

yatamux send-keys --pane tests --enter --raw "対象のテストだけ実行して、失敗時は要因をまとめてください。"

pwsh -File integrations/claude-code/scripts/watch-pane.ps1 -Pane tests -Json
```

この流れなら、メイン pane を汚さずに worker を分離し、進捗の監視・割り込み・終了判断を alias ベースで扱えます。

ペイン制御コマンドの役割分担は次の通りです。

- `interrupt-pane`: `Ctrl+C` を送り、ペインは残す
- `terminate-pane`: ペインのプロセスを強制終了し、`PaneClosed` まで待つ
- `close-pane`: ペイン自体を閉じる。プロセスが残っていればペイン破棄と一緒に停止する

`capture-pane --plain-text` は従来どおりスクリプト向けのプレーンテキストダンプを返します。`capture-pane --json` は同じ `content` に加えて、次のような構造化メタデータを返します。

```json
{
  "pane": 1,
  "content": "prompt\ncurrent screen",
  "title": "pwsh",
  "cols": 80,
  "rows": 24,
  "lines_requested": 200,
  "scrollback_len": 512,
  "cursor": { "col": 12, "row": 23, "visible": true },
  "visible_text": ["current screen"],
  "scrollback_tail": ["prompt"]
}
```

- `content`: スクロールバック末尾と現在画面を連結したプレーンテキスト
- `scrollback_tail`: そのうちスクロールバック側だけの行配列
- `visible_text`: 現在画面の各行を 1 要素ずつ持つ配列

### トースト通知

yatamux の通知はフォーカス状態でバックエンドが切り替わります。

- アプリがフォーカス中なら、画面右下にアプリ内トーストを描画します。
- アプリが非フォーカスなら、Windows ネイティブのバルーン通知へ転送します。

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

- Windows 10 1903 以降が必要（ConPTY）

### ロードマップ

- [ ] WebSocket ブリッジ（ブラウザ / スマホからの読み取り専用リモート監視）
- [x] Claude Code 統合スキル（AI オーケストレーション向けプロンプト定義 + wrapper script）
- [x] コピーモード — キーボードによるテキスト選択 + クリップボードヤンク（ペインモード `V`）
- [x] `capture-pane` CLI — AI が読みやすいペイン内容ダンプ
- [x] `split-pane --dir` CLI — 任意の作業ディレクトリで新ペインを作成
- [x] シェル終了時のペイン自動削除
- [x] ペインリサイズ（ペインモード `<` / `>`、±5 % 比率調整）
- [x] アプリ内レイアウトランチャー（ペインモード `L`、分割図プレビュー付き）
- [x] スクロールバックバッファ（50,000 行、マウスホイール、`$EDITOR` 起動）
- [x] セッション永続化（`%APPDATA%\yatamux\session.toml`）
- [x] フローティングペイン（`Ctrl+F`）
- [x] ペインモード・ステータスバーヒント
- [x] 宣言的レイアウト（`--layout <name>`）
- [x] プラグインフック（`config.toml` `[hooks]`）

### ライセンス

MIT — [LICENSE](LICENSE) を参照
