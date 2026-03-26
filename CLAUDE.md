# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

> **Note for contributors:** This file contains guidance for [Claude Code](https://claude.ai/code),
> Anthropic's AI coding assistant. You can safely ignore it if you're not using Claude Code.

## コマンド

```powershell
cargo build                          # ビルド
cargo run                            # 実行（デバッグビルド、コンソールウィンドウあり）
$env:RUST_LOG="info"; cargo run      # ログ付き実行
cargo test                           # 全テスト
cargo test -p yatamux-terminal       # クレート単体テスト
cargo test -p yatamux-client         # window.rs のキーマップテスト等
cargo test -- test_name              # 単一テスト
```

### just タスクランナー（[just](https://github.com/casey/just) インストール済みの場合）

```powershell
just run          # デバッグビルドして実行
just run-log      # ログ付き実行
just build        # リリースビルド
just test         # 全テスト
just install      # cargo install --path .（~/.cargo/bin/yatamux.exe へインストール）
just lint         # clippy
just fmt          # rustfmt
```

リリースビルドは `#![cfg_attr(all(not(debug_assertions), not(feature = "cli")), windows_subsystem = "windows")]` によりコンソールウィンドウが消える。`RUST_LOG` が使えるのはデバッグビルドのみ。

## アーキテクチャ概要

シングルプロセス・インプロセス構成。tokio `mpsc` チャネルで Server と Client を直結する。
外部 CLI・エージェント向けに Named Pipe IPC サーバー（`\\.\pipe\yatamux-{session}`）も常時起動する。

```
src/main.rs          エントリポイント。tokio::main。clap で引数解析し AppConfig を読み込んで app::run() へ渡す。
                     サブコマンド（list-panes / send-keys / capture-pane / split-pane）は src/cli.rs へ委譲。
src/app.rs           起動オーケストレーション。
                     ① Server を起動（server_out_tx → fan_out タスク → GUI/IPC へ配信）
                     ② IPC サーバーを起動（外部 CLI 接続受け付け）
                     ③ Workspace → Surface → 初期 Pane を作成
                     ④ セッション復元 / --layout 設定 / 初期ペインのみ の3パスで起動
                     ⑤ tokio::select! ループ（出力ルーティング＋ペイン分割＋フローティング
                        ＋フック発火＋通知ルーティング＋レイアウト切り替え）
                     ⑥ spawn_blocking で Win32 メッセージループを起動
src/cli.rs           IPC 経由 CLI サブコマンド実装（list-panes / send-keys / capture-pane / split-pane）。
src/config.rs        AppConfig / HooksConfig / AppearanceConfig。%APPDATA%\yatamux\config.toml から読み込む。
                     on_pane_created / on_pane_closed フックを cmd.exe /C で非同期発火。
                     `parse_hex_color(s)` で `"#rrggbb"` → `(u8,u8,u8)` 変換。
                     `AppearanceConfig` は `[appearance]` セクション（font_family / font_size /
                     background / foreground / cursor / selection_bg / status_bar_bg）。
src/layout_config.rs 宣言的レイアウト設定。%APPDATA%\yatamux\layouts\<name>.toml から読み込む。
```

### チャネル構成

| チャネル | 型 | 向き |
|---------|-----|------|
| `merged_tx` | `mpsc<ClientMessage>` | GUI / IPC → Server（マージポイント） |
| `server_out_tx` | `mpsc<ServerMessage>` | Server → fan_out タスク |
| `server_rx` (GUI 用) | `mpsc<ServerMessage>` | fan_out → app.rs ループ |
| `ipc_out_rx` | `mpsc<ServerMessage>` | fan_out → IPC サーバー |
| `msg_tx` | `mpsc<ClientMessage>` | Win32 スレッド → merged_tx（Input/Resize） |
| `split_tx` | `mpsc<(PaneId, SplitDirection)>` | Win32 スレッド → app.rs（分割要求） |
| `client_notification_tx` | `mpsc<(PaneId, String)>` | Pane タスク → Server 内部（session.rs 経由で `ServerMessage::Notification` に変換後 fan_out へ） |
| `float_tx` | `mpsc<()>` | Win32 スレッド → app.rs（フローティングペイン表示/生成要求） |

### クレート責務

**`yatamux-terminal`** — ターミナルエミュレーション層（Win32 依存なし）
- `Grid`: 仮想スクリーンバッファ。`dirty: Vec<bool>` で差分描画フラグを管理。オルタネートスクリーンは `saved_main: Option<MainScreenSnapshot>` で実装。
- `VtProcessor`: `vte::Perform` 実装。パース結果を `Grid` メソッド呼び出しに変換。OSC 52 受信時は `clipboard_data: Option<Vec<u8>>` にデコード済みバイト列を格納。
- `TerminalSink`: `Grid + vte::Parser` をまとめたラッパー。`feed(&[u8]) -> Option<Vec<u8>>` で VT バイト列を受け取りグリッドを更新。OSC 52 が含まれていた場合のみ `Some(decoded)` を返す。
- `PtySession`: `portable-pty` ラッパー。ConPTY を起動し PTY 読み書きを管理。`write()` は `write_all` 後に `flush()` を呼ぶ（Ctrl+C 即時到達のため）。`clone_child_killer()` で `Drop` 時の子プロセス kill 用ハンドルを取得できる。
- `CjkWidthConfig`: East Asian Ambiguous 幅の設定。ConPTY のカーソル位置を信用せずこちらで計算する。

**`yatamux-server`** — ペイン・セッション管理
- `Server::run()`: tokio `select!` で `ClientMessage` 受信とペイン出力転送を並行処理。
- 階層: `Workspace` → `Surface`（タブ）→ `PaneTree`（二分木）→ `Pane`
- `Pane::spawn()`: tokio タスクを2つ起動（PTY 読み取り・書き込み）。読み取り側は VT パース後 `Grid` を更新し、生バイト列を `pane_output_tx` へも転送する。`Drop` 時に `child_killer.kill()` を呼んで cmd.exe を終了させる（孤児プロセス防止）。
- `Pane.title` / `Pane.size` は `std::sync::Mutex`（`tokio::sync::Mutex` にすると `handle_client_message` 内でデッドロックするため。`docs/troubleshoot.md` T-01 参照）。
- `PaneTree` は `server/src/session.rs` 内のローカル型（`yatamux-client` の `LayoutNode` とは別物）。
- `ipc.rs`: `run_ipc_server()` が Named Pipe を listen し、JSON 行形式で `ClientMessage` / `ServerMessage` を送受信する。
- `ServerMessage::PaneCreated` には `split_from: Option<PaneId>` と `direction: Option<SplitDirection>` が含まれる。IPC 経由で作成されたペインを GUI レイアウトに反映するため `app.rs` がこれを参照する。

**`yatamux-client`** — Win32 ウィンドウ・レンダリング
- `window.rs`: `WndProc` 実装。`SetWindowLongPtrW(GWLP_USERDATA)` で `ClientState` ポインタを保持。`WM_TIMER` で OSC 52 クリップボードデータ（`pending_clipboard`）を Win32 `SetClipboardData` で書き出す。トースト通知は `paint_toasts()` で右下に描画（最大3件表示、スライドイン・フェードアウトアニメーション付き）。
- `ClientMode` 列挙型（`Normal` / `Pane` / `Copy`）で UI モードを管理。`Ctrl+B` で Pane モードへ遷移。Pane モードでは `E/O`（分割）、`W`（ペイン削除）、`F`（フローティング）、`X`（スクロールバックをエディタで開く）、`<`/`>`（サイズ調整）、`L`（レイアウトランチャー）、`V`（コピーモード）、`q`（Normal に戻る）が有効。Copy モードでは `hjkl`/矢印（カーソル移動）、`v`（選択トグル）、`y`/Enter（ヤンク）、Esc/q（終了）。Normal モードでは `Ctrl+P` でテーマランチャーを開く。
- `Ctrl+F` で `float_tx` 経由のフローティングペイン切り替え。`WM_LBUTTONDOWN` でクリック座標からペインを特定してフォーカス移動。`WM_MOUSEWHEEL` で `scroll_offset` を増減。
- `ClientState`: `Arc<Mutex<PaneStore>>` を中心に持つ。Win32 スレッドと tokio タスクが共有。`active_toasts: Mutex<Vec<Toast>>` でアニメーション中のトーストを管理。テーマは `theme: std::cell::Cell<WinTheme>`（`WinTheme` は `#[derive(Copy,Clone)]`）で保持し、ランタイムに `state.theme.set(new_theme)` で切り替える。フォント変更は再起動が必要。
- `layout.rs`: クライアント側レイアウトツリー（`LayoutNode`）と `PaneStore`。`compute_rects()` でペインのピクセル矩形を計算。`PaneStore` は `pending_clipboard: Option<Vec<u8>>`、`pending_toasts: VecDeque<Toast>`、`scroll_offset: usize`、`floating: Option<PaneId>`、`floating_visible: bool`、`launcher: Option<LauncherState>`、`theme_launcher: Option<ThemeLauncherState>`、`copy_mode: Option<CopyState>` を持つ。`list_available_themes()` / `load_theme_from_file(name)` でテーマ TOML を読み込む。
- `session.rs`: `LayoutSnapshot` を `%APPDATA%\yatamux\session.toml` に保存・読み込みする。`LayoutNodeDef` は serde 可能な `LayoutNode` の鏡像型。
- `ime.rs`: `WM_IME_*` ハンドラと候補ウィンドウ管理。

**`yatamux-protocol`** — メッセージ型定義のみ。ロジックなし。
- `ServerMessage::Output.data` は `Arc<[u8]>` 型（ファンアウト時のコピーレス配信のため）。
- `ServerMessage::Notification { pane, body }`: OSC 9/99/777、BEL（`\x07`）、PTY 終了時などに発火。`app.rs` がバックグラウンドペインからの通知を `pending_toasts` に変換する。

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
Ctrl+Shift+E/O（GUI起点）
  → split_tx.send((active, direction))
  → app.rs の select! ループが受信
  → ClientMessage::CreatePane { split_from: Some(parent) } をサーバーへ送信
  → ServerMessage::PaneCreated { split_from: None, .. } が返る（pending キューで照合）
  → TerminalSink 作成 → pane_store.layout.split_leaf() → grids に追加

yatamux split-pane（IPC CLI起点）
  → IPC 経由で ClientMessage::CreatePane { split_from: Some(id), direction: Some(dir), .. }
  → ServerMessage::PaneCreated { split_from: Some(id), direction: Some(dir), .. } が返る
  → app.rs の PaneCreated ハンドラの else 節でレイアウトに追加（pending には積まれない）
```

## Win32 固有の注意点

- `GetTextFaceW` の戻り値は**ヌル終端を含む**長さ。`.trim_end_matches('\0')` が必須。
- `AdjustWindowRectEx` は windows-rs で `Result<()>` を返す（`BOOL` ではない）。`.map_err(|e| anyhow::anyhow!(...))` でハンドルする。
- `WM_SIZE` では `ClientMessage::Resize` を `msg_tx` 経由で送信してサーバー側 ConPTY にも通知すること（クライアント側 Grid だけリサイズすると ConPTY とずれる）。
- DWM ダークタイトルバーは `DWMWINDOWATTRIBUTE(20)` = `DWMWA_USE_IMMERSIVE_DARK_MODE`（Windows 10 1903 以降）。
- フォント優先順位: HackGen Console NF → HackGen Console → HackGen35 Console NF → HackGen35 Console → HackGen NF → HackGen → Cascadia Mono → Cascadia Code → Consolas → MS Gothic（最終フォールバック）。`config.toml` の `[appearance] font_family` で上書き可能。
- テーマファイルは `%APPDATA%\yatamux\themes\<name>.toml`（`[appearance]` セクション）。`Ctrl+P` でランチャーを開き Enter 適用。フォント変更は再起動が必要（ランタイム変更不可）。
- `windows` crate 0.62 以降、`SelectObject`/`DeleteObject` の引数は `HGDIOBJ` 型が必要（`HBRUSH`/`HPEN`/`HFONT`/`HBITMAP` は `.into()` で変換）。多くの Win32 関数で `HWND`/`HDC` 引数が `Option<HWND>`/`Option<HDC>` に変更された。

## 実装フロー

機能追加・バグ修正を行う際は、以下の順序を必ず守ること。

### 1. テストケースを Markdown に起票

`docs/test-plan-<機能名>.md` を作成し、実装前にテストケースを列挙する。
既存の例: `docs/test-plan-osc52.md`, `docs/test-plan-session.md`, `docs/test-plan-notifications.md`, `docs/test-plan-scrollback.md`, `docs/test-plan-plugin-system.md`

```markdown
## テスト計画: <機能名>

### TC-01: <ケース名>
- **前提**: ...
- **操作**: ...
- **期待結果**: ...
```

### 2. TDD（テスト駆動開発）

1. テストケースに対応する `#[test]` / `#[tokio::test]` を先に書く（Red）
2. テストが通る最小限の実装を書く（Green）
3. リファクタリング（Refactor）

Win32 依存のある `yatamux-client` のテストは `#[cfg(test)]` ブロック内で
`ClientState` を直接構築して検証する（ウィンドウを実際に開かない）。
VT シーケンス関連は `yatamux-terminal` 側で単体テストを書く。

`yatamux-server` の `#[tokio::test]` は必ず `with_timeout(async { ... }).await` でラップすること（デッドロック検出のため 120s で強制 Fail）。

### 3. Clippy・テスト・フォーマットを通す

```powershell
cargo clippy -- -D warnings   # 警告をエラーとして扱う
cargo test                     # 全テスト
cargo fmt --check              # フォーマット確認（just fmt で自動修正）
```

または `just` が使える場合:

```powershell
just lint && just test && just fmt
```

すべてパスしてから PR を出すこと。

## task.md

`task.md` が未実装タスクの一覧。

## docs/troubleshoot.md

このプロジェクトで実際に発生したデッドロック・テスト失敗・文字化け等のトラブルパターンと
解決策をまとめたリファレンス。新機能追加・バグ修正の前に確認すること。

主なトピック:
- **T-01**: `tokio::sync::Mutex` を `handle_client_message` 内で使うと循環デッドロック
- **T-02**: テストに必ず `with_timeout`（120s）を付ける
- **T-03**: 子プロセス孤児化を防ぐため `Drop` impl で `ChildKiller::kill()` を呼ぶ
- **T-04**: Windows の ANSI CP932 文字化けは `activeCodePage = UTF-8` マニフェストで解決
- **T-05**: `select!` スタベーションパターン
