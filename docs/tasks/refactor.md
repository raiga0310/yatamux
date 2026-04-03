# yatamux リファクタリングタスク

> **A-2〜A-8 はすべて完了済み（2026-04-04 時点）。**
> 詳細は `docs/tasks/archive-2026-04-04.md` を参照。
> 新しいリファクタリングタスクはここに追加する。

実装挙動とテスト期待値を変えずに、責務ごとにモジュールを切り分けるための backlog。
2026-03-30 時点の大きいファイルを基準に、分割粒度をあらかじめ決めておく。

## 共通ルール

- 目的は「読みやすさ・変更容易性・責務境界の明確化」であり、機能追加ではない
- 既存の public API と CLI/IPC 挙動は原則維持する
- 分割後も既存テストをそのまま通す
- `mod.rs` / 入口ファイルは薄く保ち、詳細実装を抱え込ませない
- 過分割は避け、強く結合したロジックは同じモジュールに残す

## 優先候補

- `crates/client/src/window.rs`: 4088 行
- `crates/client/src/layout.rs`: 1581 行
- `crates/server/src/session.rs`: 1345 行
- `crates/terminal/src/vt.rs`: 1307 行
- `crates/terminal/src/grid.rs`: 1089 行
- `src/app.rs`: 797 行

### A-2: clone 監査と所有権ライフサイクルのリアーキテクチャ 【優先度: 高】

現状のコードベースには、Rust として妥当な「ハンドルの cheap clone (`Arc` / `mpsc::Sender`)」と、
設計上まだ整理余地のある「状態・文字列・レイアウトの clone」が混在している。
とくに `app.rs` の fan-out / hook / layout 適用まわり、`window.rs` の lock 脱出用 snapshot clone、
`layout.rs` の部分木 clone などは、状態ごとのライフサイクルを明示して再設計した方が見通しが良い。

- **問題意識**
  - `clone` が「共有ハンドルの複製」なのか「所有権設計の逃げ」なのか、箇所ごとの意味が揃っていない
  - lock を早く解放する目的で状態全体を clone しており、責務境界が曖昧になっている箇所がある
  - 一時コマンド文字列やレイアウトノードが必要以上に複製され、ライフサイクルが読み取りづらい
- **狙い**
  - セッション単位、ペイン単位、描画フレーム単位、一時イベント単位で「誰が所有し、どこで消費されるか」を明確化する
  - `clone` を cheap handle clone / snapshot clone / accidental clone に分類し、後者2つを必要最小限に絞る
  - Rust らしく borrow / move / `mem::take` / `Arc<str>` / `Cow<'_, str>` を使い分け、状態遷移を明示する

#### 重点監査対象

- `src/app.rs`: `ServerMessage` fan-out 時の `msg.clone()`、hook コマンド文字列の clone、多段レイアウト適用時の `command.clone()`
- `crates/client/src/window.rs`: `launcher` / `theme_launcher` / `copy_mode` / `layout` の lock 脱出用 clone
- `crates/client/src/layout.rs`: 部分木除去時の `(**first).clone()` / `(**second).clone()`
- `crates/server/src/session.rs`: `title` / `body` / `name` などの文字列 clone と capture 用メタデータ生成
- `crates/server/src/pane.rs`: PTY タスク間の所有権分配と notification payload の生成

#### サブタスク

- [x] `docs/design-ownership-lifecycle.md` を作成し、clone の分類表と各状態の owner / borrower / drop point を整理する
- [x] `rg "\\.clone\\("` ベースで clone 監査を行い、cheap clone と削減対象 clone をリストアップする
- [x] Low リスクの clone 整理（hook 判定、queue 先頭参照、notification body move、未使用 sender フィールド削除）を実装する
- [x] `app/bridge` 境界に `BridgeEvent` / `PaneLaunchPlan` / `LayoutPlan` を導入し、layout-switch の tuple queue を型付き計画へ置き換える
- [x] `src/app.rs` の fan-out / layout-switch 経路を、move 中心で書けるよう再設計する
- [x] `window.rs` の Enter 確定系で `take()` を使い、`save_prompt` / `launcher` / `theme_launcher` の不要 clone を除去する
- [x] `window.rs` の UI state 取得を「必要部分のみ抽出する render snapshot」に寄せ、`launcher` / `theme_launcher` / `save_prompt` 描画時の全体 snapshot clone を減らす
- [x] `layout.rs` の部分木操作を `Clone` 前提ではなく `mem::replace` / `Option::take` ベースに置き換えられるか検証する
- [x] 文字列 payload を `String` clone でばら撒いている箇所を `Arc<str>` / 所有権移動で削減できるか検証する
- [x] `pane.rs` ↔ `session.rs` 間の stringly typed な内部通知を `PaneEvent` enum に置き換える
- [x] `docs/test-plan-typed-pane-events.md` を追加する
- [x] `cargo clippy -p yatamux-server -- -D warnings` と `cargo test -p yatamux-server` を通す
- [x] 変更後に `cargo clippy -- -D warnings` と主要テストを通し、回帰のない ownership 境界に整える

### A-3: `window.rs` の再モジュール化（Win32 入口を薄く保つ） 【優先度: 高】

`crates/client/src/window.rs` は Win32 の WndProc 入口、描画、入力変換、UI モード別ハンドラ、補助ユーティリティ、
テストまでを一体で抱えており、変更時の読解コストが高い。

- **分割方針**
  - 公開面は `Theme` と `run_window()` を維持し、ファイルの入口は `window/mod.rs` に寄せる
  - Win32 メッセージ境界は 1 箇所に残し、純粋ロジックを外へ逃がす
  - 細かいモードごとにバラし過ぎず、関心ごと単位で 4〜6 モジュール程度に留める

#### サブタスク

- [x] `window/mod.rs` を新設し、公開 API とモジュール束ね役だけを残す
- [x] `keydown_to_vt*` と `mouse_to_vt` を `window/input.rs` へ分離する
- [x] 描画色計算、render snapshot、GDI 描画補助を `window/render.rs` へ分離する
- [x] save prompt / layout launcher / theme launcher / copy mode / pane mode のハンドラ群を `window/modes.rs` または `window/handlers.rs` にまとめる
- [x] WndProc の本体は `window/wndproc.rs` に寄せ、分岐から純粋ロジック呼び出しだけが見える形にする
- [x] 既存ユニットテストの配置を見直し、入力変換テストは `window/input.rs` 側へ近接させる
- [x] `cargo test -p yatamux-client && cargo clippy -p yatamux-client -- -D warnings && cargo fmt --check`

### A-4: `layout.rs` の責務分離（レイアウト木 / UI 状態 / ファイル入出力） 【優先度: 高】

`crates/client/src/layout.rs` は純粋なレイアウト木操作、ランチャー状態、テーマ/レイアウト一覧のファイル入出力、
コピー状態、トースト状態、実行時 `PaneStore` を同居させている。

- **分割方針**
  - 純粋データ構造と IO を分離する
  - ランチャー系の UI 状態は `window` に近い側へ逃がすか、少なくとも独立モジュールに寄せる
  - `PaneStore` はランタイム状態として孤立させ、木操作ロジックとの責務を分ける

#### サブタスク

- [x] `layout/tree.rs` に `PaneRect` / `Direction` / `LayoutNode` と純粋な木操作を移す
- [x] `layout/store.rs` に `PaneStore` とその操作を寄せる
- [x] `layout/launcher.rs` に `LayoutPreview` / `LauncherState` / `ThemeLauncherState` を寄せる
- [x] `layout/catalog.rs` に theme/layout 一覧取得、プレビュー構築、保存処理を移す
- [x] `CopyState` と `Toast` の所属を見直し、`store.rs` か専用モジュールへ整理する
- [x] `load_theme_from_file()` の `crate::window::Theme` 依存を見直し、循環しにくい型境界へ整える
- [x] `cargo test -p yatamux-client && cargo clippy -p yatamux-client -- -D warnings && cargo fmt --check`

### A-5: `session.rs` の分割（モデル / 木操作 / メッセージ処理） 【優先度: 高】

`crates/server/src/session.rs` はサーバー本体状態、ワークスペース/サーフェス/ペイン木モデル、
木操作ヘルパー、クライアントメッセージ処理が 1 ファイルに詰まっている。

- **分割方針**
  - 永続的なデータモデルと、逐次実行されるハンドラを別モジュールに分ける
  - 純粋関数として切れる木操作は `Server` 実装から切り離す
  - `Server::new()` と公開 API だけは入口に残す

#### サブタスク

- [x] `session/model.rs` に `PaneTree` / `Surface` / `Workspace` を移す
- [x] `session/tree.rs` に `pane_ids_in_tree()` と `split_pane_tree()` を含む純粋木操作を移す
- [x] `session/server.rs` または `session/mod.rs` に `Server` 定義と公開 API を残す
- [x] `handle_client_message` 系の分岐を `session/handlers/*.rs` に整理し、pane 操作・問い合わせ系の責務を分ける
- [x] capture / metadata / 通知生成の補助処理をハンドラ本体から切り離す
- [x] `crates/server/tests/ipc_integration.rs` を中心に既存テストの回帰がないことを確認する
- [x] `cargo test -p yatamux-server && cargo clippy -p yatamux-server -- -D warnings && cargo fmt --check`

### A-6: `vt.rs` の分割（CSI / OSC / SGR / 色変換の切り出し） 【優先度: 高】

`crates/terminal/src/vt.rs` は `VtProcessor` の状態、CSI/OSC/ESC の分岐、SGR と色変換ヘルパー、
大量のユニットテストが同居している。

- **分割方針**
  - `VtProcessor` 自体は中心に残し、プロトコル別の解釈ロジックを外へ出す
  - テーブル的な色変換と OSC 解析は独立性が高いので優先して切る
  - パーサ状態の散逸は避け、処理対象 `Grid` への副作用境界は明確に保つ

#### サブタスク

- [x] `terminal/vt/mod.rs` を入口にして `VtProcessor` と `feed_bytes()` を残す
- [x] `terminal/vt/sgr.rs` に `apply_sgr()` と拡張色パースを移す
- [x] `terminal/vt/color.rs` に `ansi16()` / `color256()` を移す
- [x] `terminal/vt/osc.rs` に OSC 9/52/133/777 の解析を移す
- [x] 必要なら `terminal/vt/csi.rs` / `terminal/vt/esc.rs` にディスパッチ補助を分ける
- [x] テストを機能群ごとに近接配置し、OSC/SGR/カーソル移動の失敗時に追いやすくする
- [x] `cargo test -p yatamux-terminal && cargo clippy -p yatamux-terminal -- -D warnings && cargo fmt --check`

### A-7: `grid.rs` の補助責務分離（scrollback / text export / grapheme 補助） 【優先度: 高】

`crates/terminal/src/grid.rs` は `Grid` コアの状態遷移と、scrollback、テキスト抽出、Unicode 正規化、
ZWJ/VS16 を含む書き込み補助までを抱えている。
ただしここは内部結合が強いので、コア更新ロジックまで細かく裂かない。

- **分割方針**
  - `Grid` の主要な状態遷移は 1 箇所に残す
  - 補助型・補助関数・テキスト化ロジックから外へ逃がす
  - 「分けること」よりも「コアの読み筋を短くすること」を優先する

#### サブタスク

- [x] `terminal/grid/scrollback.rs` に `ScrollbackBuffer` を移す
- [x] `terminal/grid/text.rs` に `row_cells_to_text()` / `normalize_nfc()` / `extract_text()` 系を寄せる
- [x] ZWJ / VS16 / 結合文字の補助ロジックを `terminal/grid/grapheme.rs` などへ切り出せるか検証する
- [x] `Grid` 本体にはカーソル移動、スクロール、画面切替、セル書き込みの主経路だけを残す
- [x] ユニットテストを scrollback / text export / grapheme 系に寄せ、責務単位で読めるようにする
- [x] `cargo test -p yatamux-terminal && cargo clippy -p yatamux-terminal -- -D warnings && cargo fmt --check`

### A-8: `app.rs` の起動オーケストレーション分離 【優先度: 中】

`src/app.rs` は起動配線、IPC bridge、レイアウト復元、レイアウト切り替え、fan-out を一括で抱えている。
`A-2` の clone 整理とも関係が深いので、オーケストレーション境界を先に明確にしておきたい。

#### サブタスク

- [x] `app/bootstrap.rs` に初期化とチャネル配線を寄せる
- [x] `app/layout_restore.rs` に保存済みレイアウト復元と宣言的レイアウト適用を寄せる
- [x] `app/layout_switch.rs` に `LayoutSwitchPhase` と切り替え完了処理を寄せる
- [x] IPC server 起動と fan-out の責務を `app/ipc.rs` または `app/bridge.rs` に切り出す
- [x] `run_app()` 相当の上位フローから、起動手順が上から追える形に整理する
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
