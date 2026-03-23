# cmux-win

**CJK-first terminal multiplexer for Windows, built with Rust + Win32 GDI + ConPTY.**

[English](#english) · [日本語](#日本語)

---

## English

### What is this?

cmux-win is a Windows-native terminal multiplexer designed for CJK (Chinese/Japanese/Korean) users and AI coding workflows (e.g. Claude Code, lazygit).

Modern agent-oriented terminal apps like [ghostty](https://github.com/ghostty-org/ghostty) and [cmux-windows](https://github.com/mkurman/cmux-windows) are primarily built for **macOS / Linux**, or their Windows ports have notable gaps:

- **CJK character width miscalculation** — kanji/kana/hangul are treated as 1-cell wide, causing cursor misalignment
- **Incomplete or broken IME support** — preedit strings overlap or corrupt the display
- **Half-width voiced katakana (U+FF9E/FF9F)** — misidentified as zero-width combining marks
- **Box-drawing character rendering** — font-dependent glyphs cause misaligned borders in neovim and similar UIs

cmux-win addresses these on Windows by using native APIs directly: ConPTY for PTY, Win32 GDI for rendering, and IMM32 for Japanese input.

### Features

| Feature | Detail |
|---------|--------|
| **CJK width calculation** | UAX #11 compliant + custom override table. Does not trust ConPTY cursor position. |
| **IME support** | WM_IME_COMPOSITION preedit display, committed text sent as UTF-8 to PTY |
| **Pane splitting** | Binary tree layout — `Ctrl+Shift+E` (vertical) / `Ctrl+Shift+O` (horizontal) |
| **Box-drawing characters** | U+2500–259F rendered via GDI primitives (font-independent) |
| **Font auto-selection** | HackGen Console NF → HackGen Console → Cascadia Mono/Code → Consolas |
| **Color theme** | Catppuccin Mocha (bg `#1e1e2e`, fg `#cdd6f4`, cursor `#f5c2e7`) |
| **60 fps rendering** | GDI double-buffered, dirty-line differential repaint |
| **Dark title bar** | DWM `DWMWA_USE_IMMERSIVE_DARK_MODE` |
| **Tested with** | vim, lazygit, Claude Code |

### Requirements

- Windows 10 version 1903 (Build 18362) or later (required for ConPTY API)
- Windows 11 recommended
- [Rust toolchain](https://rustup.rs/) (stable, MSVC target)

### Build

```powershell
git clone https://github.com/raiga0310/cmux-win
cd cmux-win
cargo build --release
```

The release binary is at `target/release/cmux-win.exe`.
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
| `Ctrl+Tab` | Focus next pane |
| `Ctrl+Shift+Tab` | Focus previous pane |

### Architecture

```
cmux-win (bin)
├── cmux-server    PTY lifecycle management, session tree (Workspace → Surface → Pane)
├── cmux-client    Win32 window, GDI rendering, IME handler, layout calculation
├── cmux-protocol  Shared message types (ClientMessage / ServerMessage)
├── cmux-terminal  VT parser, grid state machine, CJK width table, ConPTY wrapper
└── cmux-renderer  Debug text renderer (Phase 2: GPU via wgpu, planned)
```

Server and client run **in-process** connected by `tokio::sync::mpsc` channels — no named pipe overhead.

### Known Limitations

- Pane split ratio is fixed at 50:50 (drag-to-resize not yet implemented)
- No scrollback buffer
- Windows 10 1903+ required (ConPTY)

### Roadmap

- [ ] Pane resize by keyboard (`Alt+Shift+←→↑↓`) or mouse drag
- [ ] Scrollback buffer
- [ ] Session persistence

### License

MIT — see [LICENSE](LICENSE)

---

## 日本語

### これは何か

cmux-win は、Windows ネイティブな CJK 対応ターミナルマルチプレクサです。
AI コーディングワークフロー（Claude Code・lazygit など）での利用を想定して設計されています。

[ghostty](https://github.com/ghostty-org/ghostty) や [cmux-windows](https://github.com/mkurman/cmux-windows) などの、
エージェント向けに設計されたモダンなターミナルアプリは **macOS / Linux 向け** に開発されており、
Windows 移植版では以下の課題がありました:

- **CJK 文字幅の不正確な計算** — 漢字・かな・ハングルが 1 セル幅として扱われ、カーソルがずれる
- **IME（日本語入力）の不完全対応** — プリエディット文字列の表示が崩れる
- **半角カタカナ濁点 (U+FF9E/FF9F)** — 結合マークと誤認識され幅計算が狂う
- **罫線文字のフォント依存** — neovim などのボックス UI がフォントによっては崩れる

cmux-win は ConPTY・Win32 GDI・IMM32 を直接使い、Windows ネイティブにこれらを解決します。

### 主な機能

| 機能 | 説明 |
|------|------|
| **CJK 幅計算** | UAX #11 準拠 + 独自オーバーライドテーブル。ConPTY のカーソル位置は使用しない |
| **IME 対応** | WM_IME_COMPOSITION でプリエディット表示、確定文字列を UTF-8 で PTY に送信 |
| **ペイン分割** | バイナリツリーレイアウト。`Ctrl+Shift+E`（縦）/ `Ctrl+Shift+O`（横） |
| **罫線文字** | U+2500–259F を GDI プリミティブで直接描画（フォント非依存） |
| **フォント自動選択** | HackGen Console NF → HackGen Console → Cascadia Mono/Code → Consolas |
| **カラーテーマ** | Catppuccin Mocha（背景 `#1e1e2e`、前景 `#cdd6f4`、カーソル `#f5c2e7`） |
| **60fps 描画** | GDI ダブルバッファ、ダーティライン差分再描画 |
| **ダークタイトルバー** | DWM `DWMWA_USE_IMMERSIVE_DARK_MODE` |
| **動作確認済み** | vim、lazygit、Claude Code |

### 動作要件

- Windows 10 バージョン 1903 (Build 18362) 以降（ConPTY API の要件）
- Windows 11 推奨
- [Rust ツールチェーン](https://rustup.rs/)（stable、MSVC ターゲット）

### ビルド

```powershell
git clone https://github.com/raiga0310/cmux-win
cd cmux-win
cargo build --release
```

リリースバイナリは `target/release/cmux-win.exe` に生成されます。
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
| `Ctrl+Tab` | 次のペインにフォーカス |
| `Ctrl+Shift+Tab` | 前のペインにフォーカス |

### アーキテクチャ

```
cmux-win (bin)
├── cmux-server    PTY 管理・セッションツリー（Workspace → Surface → Pane）
├── cmux-client    Win32 ウィンドウ・GDI レンダリング・IME ハンドラ・レイアウト計算
├── cmux-protocol  共有メッセージ型（ClientMessage / ServerMessage）
├── cmux-terminal  VT パーサ・グリッド・CJK 幅テーブル・ConPTY ラッパー
└── cmux-renderer  デバッグ用テキストレンダラー（フェーズ 2: wgpu GPU 化予定）
```

サーバーとクライアントは **同一プロセス内** で `tokio::sync::mpsc` チャネルにより直結しています。
名前付きパイプ IPC のオーバーヘッドはありません。

### 既知の制限

- ペイン分割比は 50:50 固定（ドラッグリサイズ未実装）
- スクロールバック未実装
- Windows 10 1903 以降が必要（ConPTY）

### ロードマップ

- [ ] ペインリサイズ（`Alt+Shift+←→↑↓` またはマウスドラッグ）
- [ ] スクロールバックバッファ
- [ ] セッション永続化

### ライセンス

MIT — [LICENSE](LICENSE) を参照
