# CLAUDE.md

> **Note for contributors:** This file contains guidance for [Claude Code](https://claude.ai/code),
> Anthropic's AI coding assistant. It helps Claude understand the project architecture when
> assisting with development. You can safely ignore it if you're not using Claude Code.

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## コマンド

```powershell
cargo build                          # ビルド
cargo run                            # 実行（デバッグビルド、コンソールウィンドウあり）
$env:RUST_LOG="info"; cargo run      # ログ付き実行
cargo test                           # 全テスト
cargo test -p yatamux-terminal          # クレート単体テスト
cargo test -p yatamux-client            # window.rs のキーマップテスト等
cargo test -- test_name              # 単一テスト
```

リリースビルドは `#![windows_subsystem = "windows"]` によりコンソールウィンドウが消える（デバッグビルドのみコンソールとログが表示される）。

## アーキテクチャ概要

シングルプロセス・インプロセス構成。Named Pipe や IPC は使わず、tokio `mpsc` チャネルで Server と Client を直結する。

```
src/main.rs        エントリポイント。tokio::main。
src/app.rs         起動オーケストレーション。
                   ① Server を起動
                   ② Workspace → Surface → 初期 Pane を作成
                   ③ tokio::select! ループ（出力ルーティング＋ペイン分割処理）
                   ④ spawn_blocking で Win32 メッセージループを起動
```

### チャネル構成

| チャネル | 型 | 向き |
|---------|-----|------|
| `client_tx` | `mpsc<ClientMessage>` | app.rs → Server |
| `server_rx` | `mpsc<ServerMessage>` | Server → app.rs |
| `msg_tx` | `mpsc<ClientMessage>` | Win32スレッド → app.rs → Server（Input/Resize） |
| `split_tx` | `mpsc<(PaneId, SplitDirection)>` | Win32スレッド → app.rs（分割要求） |

### クレート責務

**`yatamux-terminal`** — ターミナルエミュレーション層（Win32 依存なし）
- `Grid`: 仮想スクリーンバッファ。`dirty: Vec<bool>` で差分描画フラグを管理。オルタネートスクリーンは `saved_main: Option<MainScreenSnapshot>` で実装。
- `VtProcessor`: `vte::Perform` 実装。パース結果を `Grid` メソッド呼び出しに変換。
- `TerminalSink`: `Grid + vte::Parser` をまとめたラッパー。`feed(&[u8])` で VT バイト列を受け取りグリッドを更新。
- `PtySession`: `portable-pty` ラッパー。ConPTY を起動し PTY 読み書きを管理。
- `CjkWidthConfig`: East Asian Ambiguous 幅の設定。ConPTY のカーソル位置を信用せずこちらで計算する。

**`yatamux-server`** — ペイン・セッション管理
- `Server::run()`: tokio `select!` で `ClientMessage` 受信とペイン出力転送を並行処理。
- 階層: `Workspace` → `Surface`（タブ）→ `PaneTree`（二分木）→ `Pane`
- `Pane::spawn()`: tokio タスクを2つ起動（PTY 読み取り・書き込み）。読み取り側は VT パース後 `Grid` を更新し、生バイト列を `pane_output_tx` へも転送する。
- `PaneTree` は `session.rs` 内のローカル型（`LayoutNode` とは別物）。

**`yatamux-client`** — Win32 ウィンドウ・レンダリング
- `window.rs`: `WndProc` 実装。`SetWindowLongPtrW(GWLP_USERDATA)` で `ClientState` ポインタを保持。
- `ClientState`: `Arc<Mutex<PaneStore>>` を中心に持つ。Win32 スレッドと tokio タスクが共有。
- `layout.rs`: クライアント側レイアウトツリー（`LayoutNode`）と `PaneStore`。`compute_rects()` でペインのピクセル矩形を計算。
- `ime.rs`: `WM_IME_*` ハンドラと候補ウィンドウ管理。

**`yatamux-protocol`** — メッセージ型定義のみ。ロジックなし。

### レンダリングの仕組み

`WM_TIMER`（16ms）→ `has_dirty_rows()` チェック → `InvalidateRect` → `WM_PAINT` → `paint()`

`paint()` の処理順：
1. `PaneStore` を短時間ロックして `layout.compute_rects()` と `grid` の Arc を取得（すぐにロック解放）
2. 各ペインの `Grid` を個別にロックしてセル描画
3. 罫線文字（U+2500–259F）は `ExtTextOutW` を使わず `MoveToEx`/`LineTo`/`FillRect` で直接描画（フォント依存による幅ずれ回避）
4. セパレーター線を描画
5. `BitBlt` でバックバッファを転送

### CJK 全角文字のセル表現

全角文字は `Grapheme { width: 2 }` セル＋`Continuation` セルのペアで格納。`Continuation` はレンダリング時にスキップする。行末での折り返しは DECAWM + LCF（Last Column Flag）で制御。

### ペイン分割フロー

```
Ctrl+Shift+E/O
  → split_tx.send((active, direction))
  → app.rs の select! ループが受信
  → ClientMessage::CreatePane { split_from: Some(parent) } をサーバーへ送信
  → ServerMessage::PaneCreated が返る
  → TerminalSink 作成 → pane_store.layout.split_leaf() → grids に追加
```

## Win32 固有の注意点

- `GetTextFaceW` の戻り値は**ヌル終端を含む**長さ。`.trim_end_matches('\0')` が必須。
- `WM_SIZE` では `ClientMessage::Resize` を `msg_tx` 経由で送信してサーバー側 ConPTY にも通知すること（クライアント側 Grid だけリサイズすると ConPTY とずれる）。
- DWM ダークタイトルバーは `DWMWINDOWATTRIBUTE(20)` = `DWMWA_USE_IMMERSIVE_DARK_MODE`（Windows 10 1903 以降）。
- リリースビルドは `windows_subsystem = "windows"` のためコンソールが開かない。`RUST_LOG` が使えるのはデバッグビルドのみ。

## task.md

`task.md` が未実装タスクの一覧。
