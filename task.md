# yatamux タスク一覧

未実装・未解決の問題をここに積む。

---

## バグ

### ~~B-1: 長い入力の折り返し描画が機能しない (issue #3)~~ 🔧 対応中 (branch: fix/line-wrap-rendering)
- 入力文字列がペイン幅を超えても次の行に折り返して表示されない
- **根本原因判明**: DEFAULT_COLS=220 で PTY/readline が初期化されるため、
  WM_SIZE で実際の画面サイズに更新されるまで折り返し列がずれていた
- **対応済み**: DEFAULT_COLS を 80×24（VT100 標準）に変更し WM_SIZE で上書き

### ~~B-2: Ctrl+C がエージェント（Claude 等）に届かない~~ 🔧 対応中 (branch: fix/ctrl-c-signal)
- ペイン内で Claude などの AI エージェントを起動した状態で Ctrl+C を押しても
  プロセスが終了しない
- **根本原因判明**: `write_all` 後の `flush()` 漏れにより 0x03 がバッファに滞留
- **対応済み**: `PtySession::write()` に `flush()` を追加

### ~~B-3: Ctrl+Shift+E/O でペイン分割すると ^E / ^O が入力欄に残る~~ ✅ 対応済み
- `WM_KEYDOWN` で Ctrl+Shift+E/O を捕捉して `split_tx` に送った後、
  `return LRESULT(0)` しているが、`TranslateMessage` / `WM_CHAR` 経由で
  `\x05`(^E) や `\x0f`(^O) が PTY に送られてしまっている可能性
- **対応方針**: `WM_KEYDOWN` でショートカットを消費した場合は `WM_CHAR` を
  スキップするフラグを立てるか、Ctrl+Shift の組み合わせは `WM_CHAR` では
  送信しないよう `WM_CHAR` ハンドラ側でガードする

### ~~B-4: バックグラウンドペインのトースト通知が表示されない~~ ✅ 実装は正常（テスト方法の誤りだった）
- **調査結果**: 実装は全経路正常。バグではなかった。
- **原因**: 検証時に OSC 9 バイト列を `ClientMessage::Input`（キー入力）として送っていたが、
  VT パーサは PTY の**出力側**しか処理しない。
  キー入力は cmd.exe の stdin に届くだけで stdout には出力されない。
- **正しいテスト方法**: ペイン内のプロセスが OSC 9 を stdout に出力する必要がある。
  例: `powershell -c "[Console]::Write([char]27 + ']9;メッセージ' + [char]7)"` を実行させる。

### F-4: バックグラウンドペインの通知が実用的でない
現状、トースト通知が出るのは「アプリが明示的に OSC 9/99/777 を stdout に出力した場合」のみ。
以下の自然な通知経路が未実装または機能しない。

#### ~~F-4a: PTY 終了時の自動通知~~ ✅ 対応済み
- `pane.rs` の出力タスクループ終了後に `client_notification_tx.send((id, "Process exited"))` を送信

#### ~~F-4b: BEL（`\x07`）→ トースト変換~~ ✅ 対応済み
- `VtProcessor` に `pub bell: bool` フィールドを追加
- `execute(0x07)` で `self.bell = true` にセット
- `pane.rs` で `bell` フラグを検出し `Notification { body: "Bell" }` として転送

#### ~~F-4c: OSC 133;D が Windows 環境で実質機能しない~~ ✅ 対応済み（ドキュメント整備）
- README にシェルインテグレーション設定例（bash/PowerShell）を追記
- プロセス終了自動通知（F-4a）の実装により、OSC 133;D を設定しなくても
  プロセス終了時には通知が出るようになった

### B-6: Pane が 1 枚のみの時に C-S-W を押してもアプリが終了しない 【優先度: 高】

`Ctrl+Shift+W` でアクティブペインを閉じるとき、`close_active_pane()` が
`LayoutNode::Leaf` を検出して早期リターンし、何もしない。
C-9 では「最後の 1 ペインが閉じられたらアプリ終了」を定義しているが、
`Ctrl+Shift+W` 経路はその終了フローに到達しない。

- **根本原因**: `close_active_pane()` の `Leaf` ガードが ClosePane メッセージを抑止している。
  `app.rs` の `PaneClosed` ハンドラは `grids.is_empty()` で `should_quit = true` にする実装があるが、
  そもそもメッセージが届かないため発動しない。
- **修正方針**: `close_active_pane()` のガードを削除し、最後の 1 ペインでも `ClosePane` を送信する。
  `app.rs` の既存フローが `should_quit = true` → `WM_TIMER` → `DestroyWindow` で終了させる。

#### サブタスク
- [x] `window.rs`: `close_active_pane()` の Leaf ガードを削除
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

---

### ~~B-7: ペイン分割後にスクロールオフセットが前のペイン幅を維持して画面が見切れる~~ ✅ 対応済み 【優先度: 中】

ペインを分割・削除・リサイズして縦方向に余白が生まれた後も、スクロールオフセットが
変更前のペイン高さを基準に計算されたままになる。
画面が上方向にずれて見切れたように表示される。

- **根本原因**: `scroll_offset` がストア全体の1フィールドで、アクティブペイン変更時・
  リサイズ時にリセット/クランプされていなかった。
- **修正内容**:
  1. `resize_all_panes()` でリサイズ後にアクティブペインの `scrollback_len()` にクランプ
  2. `cycle_pane()` / `focus_pane_dir()` でフォーカス変更時に `scroll_offset = 0` にリセット

#### サブタスク
- [x] `window.rs`: `resize_all_panes()` で scroll_offset を scrollback_len にクランプ
- [x] `window.rs`: `cycle_pane()` / `focus_pane_dir()` でフォーカス変更時にリセット
- [x] `docs/test-plan-scroll-offset.md` にテストケースを列挙
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

---

## 機能改善

### ~~F-9: クリックによるペインフォーカス~~ ✅ 対応済み 【優先度: 高】

現状、ペインフォーカスは `Ctrl+←↑↓→` のキーボード操作のみ。
ペイン領域をマウスクリックしたときに、そのペインにフォーカスを移動できるようにする。

#### サブタスク

- [x] `docs/test-plan-pane-click-focus.md` にテストケースを列挙
- [x] `layout.rs`: `LayoutNode::pane_at_point()` を追加
- [x] `window.rs`: `WM_LBUTTONDOWN` ハンドラで座標 → ペイン特定 → `active_pane` 更新
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

### ~~F-8: ペイン削除~~ ✅ 対応済み 【優先度: 高】

現状、ペインを閉じる手段がない（プロセスが終了しても PTY タスクが残る）。

- `Ctrl+Shift+W` でアクティブペインを削除できるようにする
- ペイン削除時にレイアウトツリーを再構成し、兄弟ペインが空いた領域を埋める
- 最後の1ペインは削除不可（アプリ終了に誘導）
- PTY プロセスも合わせて終了させる

#### サブタスク

- [x] `docs/test-plan-pane-close.md` にテストケースを列挙
- [x] `ClientMessage::ClosePane` はプロトコルに既存
- [x] サーバー側: `ClosePane` 処理は既存（`panes.remove` + `PaneClosed` 送信）
- [x] `layout.rs`: `LayoutNode::remove_pane()` を追加（サブツリー除去 + フォーカス候補返却）
- [x] `app.rs`: `PaneClosed` ハンドラで `layout.remove_pane` を呼び `active` を更新
- [x] `window.rs`: `Ctrl+Shift+W` で `ClientMessage::ClosePane` を送信
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

### ~~F-7: 通知バックエンドの仮想チャネル化とフォーカス連動切り替え~~ ✅ 対応済み 【優先度: 中】

現状のトースト通知は yatamux 内部描画（`paint_toasts()`）のみ。
yatamux がバックグラウンドのとき通知が見えない問題を、通知バックエンドを抽象化して解決する。

#### 方針

- 通知の「送信口」を `NotificationBackend` トレイトとして切り出し、実装から分離する
- バックエンド実装を2種類用意する：
  - **InternalToast**: 既存の yatamux 内描画トースト
  - **NativeToast**: Windows のシステム通知（WinRT `ToastNotificationManager` または Win32 バルーンチップ）
- yatamux がフォーカスを持つ場合 → `InternalToast`、失っている場合 → `NativeToast` へ自動切り替え
- `app.rs` は `NotificationBackend` トレイトオブジェクト経由でのみ通知を送る（実装詳細を持たない）

#### サブタスク

- [x] **Step-1（設計）**: `docs/design-notification-backend.md` に設計方針・トレイト定義案・フォーカス検知方法を記述
- [x] **Step-2（テスト計画）**: `docs/test-plan-notification-backend.md` を作成し TC を列挙
- [x] **Step-3（トレイト定義）**: `yatamux-client` に `NotificationBackend` トレイトを追加
- [x] **Step-4（InternalToast 移植）**: 既存 `paint_toasts()` 経路を `InternalToast` 実装に切り替え
- [x] **Step-5（NativeToast 実装）**: WinRT or バルーンチップで OS ネイティブ通知を送出
- [x] **Step-6（フォーカス切り替え）**: `WM_ACTIVATEAPP` でフォーカス状態を検知し、バックエンドを動的に切り替え
- [x] **Step-7（テスト・lint）**: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

### ~~F-1: 起動時のウィンドウ位置・サイズが不適切~~ ✅ 対応済み (branch: fix/startup-window-position)
- `SW_SHOW` → `SW_SHOWMAXIMIZED` に変更し起動時最大化

### ~~F-2: バイナリの実行が不便~~ ✅ 対応済み (branch: feat/install-convenience)
- `justfile` 追加 + Cargo.toml メタデータ追加
- `just install` で `%USERPROFILE%\.cargo\bin\` にインストール可能

### ~~F-3: ペイン分割・フォーカス移動のキーバインドを改善したい~~ ✅ 対応済み
- ~~フォーカス移動を `Ctrl+←↑↓→` および `Ctrl+H/J/K/L` に対応~~ ✅ 対応済み
  - Left/Up/H/K → 前のペイン、Right/Down/L/J → 次のペイン
- ~~方向を考慮したレイアウトツリー走査~~ ✅ 対応済み
  - `LayoutNode::pane_in_direction()` を追加（pixel rect 距離ベース）
  - `Ctrl+←↑↓→` が方向指定の最近傍ペインに移動するように変更
- 分割ショートカット: Ctrl+Shift+E/O のままで運用

### ~~F-5: スクロールバック表示~~ ✅ 対応済み
- `Grid` に `scrollback: VecDeque<Vec<Cell>>` を追加（上限 5000 行）
- フルスクリーンスクロール時のみスクロールバックに保存（サブ領域スクロールは除外）
- オルタネートスクリーン中は保存しない
- `PaneStore` に `scroll_offset: usize` を追加
- `WM_MOUSEWHEEL` で 1 ノッチ 3 行スクロール
- `WM_CHAR` 入力でオフセットを 0 にリセット（自動的に最新画面に戻る）
- `paint()` でオフセット分 scrollback 行を上から描画
- **関連**: C-7（大規模バッファの高効率化）は別途対応

### ~~F-6: Ctrl+J のキーバインド競合（Claude Code との衝突）~~ ✅ 対応済み
- `window.rs` の `Ctrl+H/J/K/L` によるペインフォーカス移動を全廃
- フォーカス移動は `Ctrl+←↑↓→` のみに統一
- 将来的に C-1 のモードベース UI が入ったら再導入を検討

---

## 快適性向上（中〜長期）

以下は次世代ターミナルマルチプレクサとしての競争力強化に向けた機能提案。
優先度順に並べている。

### ~~C-1: モードベース UI とキーバインドヒント表示~~ ✅ 対応済み 【優先度: 高】
- Zellij のように画面下部にコンテキスト依存のキーバインド一覧を表示する
- 導入するモード例：ペイン操作モード、タブ操作モード、リサイズモード、コピーモード
- 各モードで有効なコマンドをリアルタイム表示することで学習コストを大幅に削減
- **実装**: `ClientMode` 列挙型（`Normal` / `Pane`）、`Ctrl+B` でペインモード遷移、ステータスバーにヒント表示

### ~~C-2: OSC 52 対応とクリップボード統合~~ ✅ 部分対応済み 【優先度: 高】
- OSC 52 エスケープシーケンスを処理し、SSH 越しでもローカルクリップボードへコピー可能にする
- マウス選択テキストの自動コピー、キーボードのみによるコピーモード（vi風テキスト選択）
- **参照実装**: WezTerm、Alacritty の OSC 52 実装
- **サブタスク**:
  - [x] `vt.rs` の `osc_dispatch` に OSC 52 パース処理を追加（base64 デコード込み）
  - [x] `VtProcessor` からクリップボードデータをコールバック/フィールドで取り出す（`clipboard_data: Option<Vec<u8>>`）
  - [x] Win32 レイヤーで `SetClipboardData` を呼び出しシステムクリップボードに書き込む（`WM_TIMER` で `pending_clipboard` を処理）
  - [x] テスト: `vt.rs` 内ユニットテストで確認済み

### ~~C-3: フローティングペイン~~ ✅ 対応済み（スタックペインは未実装） 【優先度: 中】
- フローティングペイン: 既存タイルレイアウトの上に重なるオーバーレイ式ペイン
  - 一時的なコマンド実行・ログ確認などに使用。Ctrl+F などで表示/非表示トグル
- スタックペイン: 同一領域に複数ペインを垂直スタックして管理
  - 限られた画面でバッファを多数保持する際に有効
- **設計メモ**: `PaneTree` の `LayoutNode` に `Float` / `Stack` バリアントを追加

### ~~C-4: セッション永続化と自動復元~~ ✅ 対応済み 【優先度: 中】
- システム再起動後もペインのレイアウト・カレントディレクトリ・実行コマンドを復元
- tmux-resurrect 相当の機能を内蔵で提供（外部プラグイン不要）
- 保存フォーマット: TOML で `%APPDATA%\yatamux\session.toml` に書き出し
- **実装方針**: Serde によるレイアウトツリーのシリアライズ。プロセス自体の復元は
  `USERPROFILE` 等の環境変数からシェルを再起動する形にとどめる
- **自動保存**: 定期的（例: 30秒ごと）または終了時に自動保存
- **サブタスク**:
  - [x] `LayoutNode` に `#[derive(Serialize, Deserialize)]` を追加
  - [x] `LayoutSnapshot` 型（グリッドなし・シリアライズ可能）を定義
  - [x] `%APPDATA%\yatamux\` への保存・読み込み実装（`session.rs`）
  - [x] 終了時自動保存フック（`WM_CLOSE` で `snap.save()` を呼び出し）
  - [x] テスト: `session.rs` 内 TC-01〜TC-09 で確認済み
  - [x] 起動時セッション復元（`app.rs` の `restore_node()` で `LayoutSnapshot::load()` → ペインを再生成して `PaneStore` に反映）

### ~~C-5: 宣言的レイアウトとプロジェクト起動定義~~ ✅ 対応済み 【優先度: 中】
- プロジェクトごとのレイアウトを設定ファイルで定義し、一括起動できる機能
  （tmuxinator / Zellij の layout YAML 相当）
- 例: エディタ、テストランナー、ログ監視の3ペインを一発起動
- **フォーマット案**: TOML で `~/.config/yatamux/layouts/myproject.toml` に配置
  ```toml
  [[panes]]
  command = "nvim ."
  [[panes]]
  command = "cargo watch -x test"
  split = "horizontal"
  ```

### ~~C-6: 高度な Unicode / 絵文字対応~~ ✅ 対応済み 【優先度: 中】
- 24ビット True Color の完全サポート確認
- ゼロ幅結合子（ZWJ）を使った絵文字（👨‍💻 等）の正確な幅計算
- Nerd Fonts グリフ（U+E000–U+F8FF）の幅を2セルとして扱うオプション
- 双方向テキスト（BiDi）への基本対応
- **サブタスク**:
  - [x] ZWJ シーケンス（U+200D 連結）の幅を1セルとして扱う（`combine_with_last_cell()` / `last_grapheme_ends_with_zwj()`）
  - [x] Variation Selector（VS-15/VS-16）による幅切り替え対応（`apply_vs16()`）
  - [x] Nerd Fonts グリフ（U+E000–U+F8FF）を2セル幅に扱う `nerd_fonts_wide` オプションを追加
  - [x] BiDi 基本対応（RTL マーカーを幅0で処理）（`vt.rs` の `is_bidi_control()` で制御文字を幅0扱い）

### ~~C-7: 効率的なスクロールバックバッファ~~ ✅ 対応済み 【優先度: 低】
- 数万行のスクロールバック履歴を保持してもメモリ・パフォーマンスが劣化しない設計
- スクロールバック内の外部エディタ起動（`$EDITOR` での編集）
- **実装**: `ropey` の代わりに `VecDeque<Vec<Cell>>` をラップした `ScrollbackBuffer` 型を新設
- **サブタスク**:
  - [x] `ScrollbackBuffer` 型を新設（`VecDeque` ベース、上限超過時に最古行を自動破棄）
  - [x] スクロールバック行数上限を 50,000 行に引き上げ（旧: 5,000 行）
  - [x] 外部エディタ起動（`EDITOR` 環境変数 → 一時ファイルへ書き出し → ペインへ送信、Pane モード `X` キー）

### ~~C-8: プラグイン / 拡張システムの設計~~ ✅ 対応済み 【優先度: 低】
- Candidate B（シェルフック + 既存 IPC）を採用
- `%APPDATA%\yatamux\config.toml` の `[hooks]` セクションで設定
- `on_pane_created` / `on_pane_closed` フックを `cmd.exe /C` で非同期発火
- 環境変数: `YATAMUX_PANE_ID`, `YATAMUX_SESSION`

### ~~C-9: シェル終了時にペインを自動削除~~ ✅ 対応済み 【優先度: 高】
- ペイン内のシェルで `exit` などを実行して PTY プロセスが終了したとき、そのペインを自動的に削除する
- 最後の 1 枚のペインが閉じたときはアプリ終了
- **実装**: `session.rs` で "Process exited" 通知受信時に自動 `ClosePane`。`app.rs` で grids 空になったら `should_quit = true` をセット。`window.rs` の `WM_TIMER` で `DestroyWindow`。

### ~~C-10: ペイン幅調整キーバインド~~ ✅ 対応済み 【優先度: 中】
- ペインモード中に `<` / `>` キーで分割比率（ratio）を 5% 単位で増減
- **実装**: `LayoutNode::adjust_ratio(pane_id, delta)` を追加。`<`/`>` はペインモードを維持して繰り返し操作可能

### ~~C-11: アプリ内レイアウトランチャー / 動的切り替えUI~~ ✅ 対応済み 【優先度: 中】
現状、C-5の宣言的レイアウトは起動時の `--layout` オプションでのみ適用可能であり、Yatamux起動後に別のプロジェクトのレイアウトへ切り替える手段がない。
ペインモード（C-1）等から呼び出せる、対話的なレイアウト選択UIを実装する。

- **概要**: `%APPDATA%\yatamux\layouts\` に保存されている設定ファイル（TOML）を一覧表示し、十字キーで選択して即座に適用（画面の再構成）を行う。
- **UI案**: 画面中央にフローティングメニュー（C-3の仕組みを応用）を描画し、利用可能なレイアウト名の一覧を表示。
- **状態管理の注意点**: 動的にレイアウトを適用する場合、既存のペイン（プロセス）をどう扱うか（すべて破棄するか、新しいウィンドウ/タブとして開くか）のポリシー決定が必要。初期段階では「現在のペインをすべて閉じてから適用する（破棄確認あり）」または「新しいタブ/ワークスペースとして展開する（将来のタブ機能拡張を見据える）」のどちらかを採用する。

#### サブタスク
- [x] `layout_config.rs` に `list_layouts()` 関数を追加（`.toml` のみ返し、ソート済み）
- [x] クライアント側に `LauncherState` と中央ポップアップ描画 UI を実装（`layout.rs` / `window.rs`）
- [x] `list_available_layouts()` でレイアウト一覧を取得しランチャーに表示
- [x] ペインモード中のキーバインド（`L`）でランチャーを起動、上下キーで選択・Enter で適用
- [x] `docs/test-plan-layout-launcher.md` 作成済み

### ~~F-10: CLIヘルプ (`--help`) とコマンドライン引数解析の実装~~ ✅ 対応済み 【優先度: 高】

現状、`yatamux --help` や `--version` といったターミナルアプリケーションとして基本的なCLIオプションが提供されていない。
ユーザーが利用可能な起動オプション（`--layout` など）をターミナル上で簡単に確認できるようにする。

- **実装方針**: `clap` クレートの derive 機能を利用した宣言的な引数定義。`--help` / `--version` は自動生成。

#### サブタスク
- [x] `Cargo.toml` に `clap` クレートを追加（derive機能を利用すると宣言的で管理しやすい）
- [x] `src/main.rs` の `Cli` 構造体で引数を定義し、`clap::Parser` で解析
- [x] サポートする引数の定義: `--help` (自動生成), `--version` (自動生成), `--layout <NAME>`
- [x] `list-panes` / `send-keys` も `#[command(subcommand)]` で整理

### ~~C-12: コピーモードとテキスト範囲選択UIの実装~~ ✅ 対応済み 【優先度: 高】
ターミナル上の出力をマウスドラッグ、またはキーボード操作で範囲選択し、システムクリップボードにコピーできるようにする。
C-2（クリップボード統合）を土台として、実際にユーザーが画面上の文字を選択・視認できるUIを構築する。

- **概要**: tmuxの `prefix + [` のようなコピーモード（スクロールバックバッファの閲覧とテキスト選択）と、マウスによる直感的なドラッグ選択をサポートする。
- **操作性**: Neovimの操作感を取り入れ、キーボード操作時は `v` で選択開始（ビジュアルモード）、`y` または `Enter` でヤンク（コピー）して通常モードに戻るフローとする。
- **描画**: 選択中のテキスト領域は、背景色と文字色を反転させるなどしてハイライト表示する。

#### サブタスク
- [x] `CopyState`（カーソル位置・選択開始座標を保持）を `PaneStore` に追加
- [x] マウスイベント（`WM_LBUTTONDOWN`, `WM_MOUSEMOVE`, `WM_LBUTTONUP`）でドラッグ始点・終点トラッキングを実装
- [x] キーボード操作（hjkl / 矢印キー）によるカーソル移動と `v`（選択開始）、`y`/`Enter`（コピー実行）のキーバインド配線
- [x] `paint()` でセルごとに fg/bg を反転して選択ハイライトを描画（FillRect による文字隠し問題を修正済み）
- [x] `Grid::extract_text(row_start, row_end)` でテキスト抽出（Continuation セルスキップ、CJK 対応）
- [x] `docs/test-plan-copy-mode.md` 作成済み

### ~~C-13: 画面キャプチャCLI（`capture-pane`）とAI向け出力~~ ✅ 対応済み 【優先度: 高】
Zenn記事の `cmux read-screen` 相当の機能。Claude CodeなどのAIが、別ペインで動かしているサブエージェントの実行結果やエラーをCLI経由で自律的に読み取れるようにする。

- **概要**: `yatamux capture-pane --target <ID> --lines <n>` のようなCLIコマンドを実装し、指定ペインの画面バッファやスクロールバック履歴をプレーンテキストとして標準出力にダンプする。
- **連携**: C-12（コピーモード）で実装したテキスト抽出ロジックを利用し、不要な空白やANSIエスケープシーケンスを整理してAIがパースしやすい形で出力する。

#### サブタスク
- [x] IPCプロトコルに `ClientMessage::CapturePane { pane_id, lines }` と `ServerMessage::PaneContent { content }` を追加
- [x] サーバー側で `Grid` および `scrollback` から指定行数のテキストを抽出し返送するロジックを実装
- [x] `src/cli.rs` に `capture-pane` サブコマンドを追加し、標準出力に結果を流す処理を実装
- [x] `docs/test-plan-capture-pane.md` 作成済み

### ~~C-14: 作業ディレクトリ指定でのペイン分割CLI（クロスリポジトリ対応）~~ ✅ 対応済み 【優先度: 高】
AIが現在のカレントディレクトリの制約を超え、別リポジトリのタスクを「別ペイン・別セッション」で即座に立ち上げるための機能。

- **概要**: `yatamux split-pane --dir <PATH>` のように、ペイン作成時に作業ディレクトリを明示的に指定できるCLIオプションを追加する。
- **背景**: 記事にある「複数プロジェクトをまたぐ作業がつらい」という課題の解決策。Claude CodeがプロジェクトAに居ながら、プロジェクトB用のサブエージェントを別ペインにスムーズに展開できるようにする。

#### サブタスク
- [x] `ClientMessage::CreatePane` のペイロードに `working_dir: Option<String>` を追加（`protocol/src/message.rs`）
- [x] `pty.rs` の `PtySession::spawn()` で `working_dir` が指定された場合そのディレクトリでシェルを起動するよう変更（存在確認つき）
- [x] `src/cli.rs` に `split-pane` サブコマンドを追加（`--target`, `--direction`, `--dir` オプション）
- [x] `docs/test-plan-split-pane-dir.md` 作成済み

### ~~B-5: `split-pane` CLI で作ったペインが GUI に表示されない~~ ✅ 対応済み 【優先度: 高】

`yatamux split-pane` で作成したペインがサーバー側（`list-panes` で確認可）には存在するが、
GUI のレイアウトツリー（`PaneStore`）に反映されず、画面に表示されない。
ステータスバーのペイン数カウントも更新されない。

#### 根本原因

`app.rs` の `ServerMessage::PaneCreated` ハンドラは以下の優先順位で処理を分岐する：

1. `layout_switch.is_some()` → レイアウト切り替えフロー
2. `pending_float` → フローティングペイン
3. `pending.pop_front().is_some()` → **GUI（キーボードショートカット）起点の分割** → `PaneStore` を更新
4. **それ以外 → 何もしない** ← IPC 経由の `CreatePane` はここに落ちる

`pending` はキーボードショートカット（`split_tx`）経由でのみ積まれる。
IPC クライアントが `CreatePane` を送っても `pending` に積まれないため、
`PaneCreated` が届いても `PaneStore.grids`・`layout` が更新されず GUI に反映されない。

#### 修正方針

`PaneCreated` ハンドラの else 節（現在は何もしない）で、
IPC 起点のペインとして GUI レイアウトに追加する処理を入れる。

- `PaneStore.grids` に新しい `Grid` を追加
- `layout.split_leaf(active, new_id, direction)` でレイアウトツリーに追加
  - ただし IPC 側は `split_from` ペイン ID と `direction` を持っているので、
    `PaneCreated` に `split_from` と `direction` を含める必要がある
- `active` を新しいペイン ID に更新

#### 必要な変更

- `ServerMessage::PaneCreated` に `split_from: Option<PaneId>` と `direction: Option<SplitDirection>` を追加（`protocol/src/message.rs`）
- サーバー側で `PaneCreated` 送信時にこれらを設定する（`session.rs` または `server/src/lib.rs`）
- `app.rs` の else 節でレイアウト更新処理を実装

#### サブタスク
- [x] `ServerMessage::PaneCreated` に `split_from` / `direction` を追加（`protocol/src/message.rs`）
- [x] サーバー側で `PaneCreated` 送信時に `split_from` / `direction` を設定（`session.rs`）
- [x] `app.rs` の `PaneCreated` ハンドラに IPC 起点ペイン追加処理を実装（else 節）

---

### ~~I-1: `send-keys` の使い勝手改善 — エージェントが `--help` 一読で成功できるように~~ ✅ 対応済み 【優先度: 高】

`send-keys` を初めて使うエージェント（Claude Code 等）が `--help` を読んだだけで一発成功できない問題が複数ある。

#### 現状の問題

1. **`\r` が CR に変換されるという仕様が `--help` に記載されていない**
   - Enter を送るには `\r` を文字列末尾に付ける必要があるが、全く記述がない
   - エージェントは自然に `"echo hello"` と送り、コマンドが実行されないことに気づかない

2. **Windows パスに `\r` が含まれると意図せず Enter として解釈される**
   - 例: `"dir C:\Users\raiga\dev"` → `unescape()` が `\r` を CR に変換してしまい、
     `dir C:\Users` + Enter + `aiga\dev` という2コマンドになる
   - `--help` にこの危険な副作用の警告がない

3. **エスケープシーケンス仕様が `--help` に書かれていない**
   - `\r`=CR、`\n`=LF、`\t`=TAB、`\\`=バックスラッシュ の変換ルールが不明

#### 理想状態

`yatamux send-keys --help` を読んだエージェントが、何も試行錯誤せずに初回で正しいコマンドを送れる。

#### 解決策の候補（実装時に選択）

- **A: `--enter` フラグを追加**（推奨）
  - `--enter` を付けると末尾に CR を自動付加する
  - `\r` のエスケープ変換はそのまま残す（明示的に使いたい場合向け）
  - `--help` の Examples に `send-keys --pane 2 --enter "echo hello"` と明示
- **B: パス中の `\r` 問題を回避するため `--raw` モードを追加**
  - `--raw` を付けるとエスケープ変換を一切しない（リテラル送信）
  - `--enter` と組み合わせて `--raw --enter` で「パスをそのまま送って Enter」が実現できる
- **C: `--help` の改善のみ（最小対応）**
  - エスケープ仕様と使用例（Windows パスの注意）を `--help` に追記するだけ

#### サブタスク
- [x] `--enter` フラグ追加（末尾に CR を自動付加）
- [x] `--raw` フラグ追加（エスケープ変換なしでリテラル送信。Windows パス対応）
- [x] `--help` の doc コメントにエスケープ仕様・注意点・使用例を追記

### C-19: 現在のペイン構成を名前付きレイアウトファイルとして保存 【優先度: 中】

C-4（セッション永続化）は起動時復元用の `session.toml` への自動保存であり、後から名前を付けて再利用する仕組みがない。
C-5（宣言的レイアウト）は手書きの TOML 定義を読み込むが、現在の状態をファイルに書き出す機能がない。
これらを組み合わせ、「今の画面構成を名前付きレイアウトファイルに書き出す」コマンドを追加する。

- **概要**: Pane モードのキーバインド（`S`）で、現在の `PaneStore` のレイアウトツリーを `%APPDATA%\yatamux\layouts\<NAME>.toml` に書き出す。保存後はランチャー（C-11）から選択して再適用可能。
- **保存対象**: 各ペインの `command`、`working_dir`、分割方向、比率（ratio）。グリッドの内容（テキスト）は含まない。
- **フォーマット**: C-5 の `[[panes]]` TOML 形式と互換を保つ（読み書き両対応）。

#### サブタスク
- [x] `docs/test-plan-save-layout.md` にテストケースを列挙
- [x] `layout.rs`: `layout_to_toml(node)` + `save_layout_file(name, content)` で現在のレイアウトツリーを `[[panes]]` TOML に変換して書き出し
- [x] `window.rs`: Pane モード中 `S` キーで名前入力プロンプトを表示 → Enter で保存、トースト通知
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

---

### ~~C-21: 外観設定（テーマ・フォント・カラーのファイル管理）~~ ✅ 対応済み

現状、配色やフォントはハードコードされており、ユーザーが変更する手段がない。
`%APPDATA%\yatamux\config.toml` に `[appearance]` セクションを追加し、
フォントファミリー・フォントサイズ・カラーテーマ（背景色・前景色・選択色・UI アクセントカラー等）を設定できるようにする。

- **概要**: 現在 `window.rs` にハードコードされている Catppuccin Mocha 相当の色定数と、フォント選択ロジックを `AppConfig` から読み込む形に変更する。
- **フォーマット案**:
  ```toml
  [appearance]
  font_family = "HackGen Console NF"
  font_size = 14
  background = "#1e1e2e"
  foreground = "#cdd6f4"
  selection_bg = "#585b70"
  status_bar_bg = "#181825"
  ```
- **プリセット**: `catppuccin-mocha`（デフォルト）、`catppuccin-latte`（ライト）、`solarized-dark` 等のテーマ名指定もサポートする方向で検討。

#### サブタスク
- [x] `src/config.rs` に `AppearanceConfig` 構造体を追加（フォント・色設定）
- [x] `window.rs` の色定数を `AppearanceConfig` から読み込む形にリファクタリング
- [x] フォント選択ロジックを `font_family` 設定に対応
- [x] デフォルト値は既存の Catppuccin Mocha 配色を維持
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`
- [x] `%APPDATA%\yatamux\themes\` ディレクトリにテーマ TOML を配置する規約を追加
- [x] Ctrl+P でテーマランチャーを開き、↑↓/Enter/Esc でランタイム切り替え（フォント変更は再起動が必要）
- [x] サンプルテーマ `light.toml`（Catppuccin Latte）を `%APPDATA%\yatamux\themes\` に配置

---

### ~~C-22: レイアウト CLI 管理コマンド（一覧表示・削除・エクスポート）~~ ✅ 対応済み 【優先度: 低】

現状、`%APPDATA%\yatamux\layouts\` にある TOML ファイルを直接操作するしかなく、
CLI から一覧確認・削除・他環境へのエクスポートができない。

- **概要**: `yatamux layout list` / `yatamux layout delete <NAME>` / `yatamux layout export <NAME>` サブコマンドを追加する。
- **用途**: スクリプトやエージェントからレイアウトを管理できるようにする。

#### サブタスク
- [x] `src/cli.rs` に `layout_list()` / `layout_delete()` / `layout_export()` を実装
- [x] `layout_config.rs` に `delete_layout(name)` / `export_layout(name)` を追加
- [x] `main.rs` に `Layout(LayoutCommands)` サブコマンドグループと `LayoutCommands` 列挙型を追加
- [x] テスト 3 件追加（TC-C22-01〜03）
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

---

### ~~C-23: レイアウト保存時にペインの起動コマンドも保存する~~ ✅ 対応済み 【優先度: 中】

現状の `layout_to_toml()` はペイン構成（分割方向・比率）のみを TOML に書き出すが、
保存時に各ペインで起動していたアプリのコマンドが記録されないため、
レイアウト再現時にウィンドウ分割だけ復元されてアプリは手動起動が必要になる。

- **採用方針**: オプション 2（クライアント側で記録）
  レイアウトファイル適用時に送信したコマンドを `PaneStore.pane_commands` に記録し、
  保存時に `layout_to_toml()` が TOML の `command` フィールドとして出力する。
  手動入力のコマンドは対象外（ConPTY から取得が困難なため）。
- **用途**: `cargo watch`・`nvim` 等の開発ツールをレイアウトファイルに保存して再利用できる。

#### サブタスク
- [x] `PaneStore` に `pane_commands: HashMap<PaneId, String>` を追加
- [x] `layout_to_toml(node, commands)` にコマンドマップを渡す形に変更
- [x] `app.rs`: レイアウト適用時（`LayoutPhase::WaitingFirst` / `Applying`）でコマンドを `pane_commands` に記録
- [x] `window.rs`: 保存プロンプト処理で `&store.pane_commands` を `layout_to_toml` に渡す
- [x] `docs/test-plan-save-layout.md` にテストケース追記（TC-C23-01〜04）
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

---

### ~~C-24: `list-panes --json` オプション追加~~ ✅ 対応済み 【優先度: 高】

現状の `list-panes` はテキストテーブル形式のみで出力される。
エージェント（takt / Claude Code 等）がペイン一覧をプログラムからパースできるよう、JSON 形式での出力オプションを追加する。

- **概要**: `yatamux list-panes --json` でペイン情報を JSON 配列として標準出力する。
- **フィールド案**: `id`, `title`, `is_active`, `size` (cols×rows), `working_dir`（C-30 と連動）

#### サブタスク
- [x] `ClientMessage::ListPanes` / `ServerMessage::PaneList` のプロトコル型に JSON 向けフィールドを確認
- [x] `src/cli.rs`: `list-panes` サブコマンドに `--json` フラグを追加
- [x] JSON 出力実装（`serde_json::to_string_pretty` 等）
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

---

### ~~C-25: 全 IPC コマンドのエラー応答徹底~~ ✅ 対応済み 【優先度: 高】

存在しないペイン ID への操作（`send-keys --pane 999` 等）が silently fail しており、
エージェントがエラーを検知できない。失敗時は必ず `ServerMessage::Error { message }` を返す。

- **対象コマンド**: `send-keys`, `capture-pane`, `split-pane`, `select-pane`（C-29）など全 IPC コマンド
- **現状**: 存在しないペインへの操作が無応答または不明なレスポンスで終わる

#### サブタスク
- [x] `yatamux-protocol/src/`: `ServerMessage::Error { message: String }` バリアントを確認（既存）
- [x] `yatamux-server/src/session.rs`: `Input` / `CapturePane` に存在チェックとエラー返却を追加
- [x] `src/cli.rs`: `send-keys` は `ListPanes` でペイン事前検証、各コマンドで `Error` 受信時 `exit(1)`
- [x] `src/cli.rs`: `split-pane` は `--target` が存在しない場合にエラー終了（フォールバック廃止）
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

---

### ~~C-26: `capture-pane` ANSI 剥離オプション（`--plain-text`）追加~~ ✅ 対応済み 【優先度: 高】

`capture-pane` の出力に ANSI エスケープシーケンス・VT バイト列が含まれており、
エージェントが読み取るには前処理が必要。`--plain-text` オプションでプレーンテキストを返す。

- **実装**: `row_cells_to_text()` が既にプレーンテキストを返すため、`--plain-text` フラグは前処理不要。
  フラグ追加でAPIの意図を明示し、将来の色付き出力モード追加に備える。

#### サブタスク
- [x] `ClientMessage::CapturePane` に `plain_text: bool` フィールドを追加（`protocol/src/message.rs`）
- [x] `src/cli.rs`: `capture-pane` サブコマンドに `--plain-text` フラグを追加
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

---

### ~~C-27: コマンド完了検知の仕組み（OSC 133;D 対応）~~ ✅ 対応済み 【優先度: 高】

`send-keys` でコマンドを送信後、そのコマンドが完了したかどうかを知る手段がない。
OSC 133;D（Shell Integration: command_done）を検出し、`ServerMessage::CommandFinished` として発火する。

- **概要**: シェル側に `PROMPT_COMMAND` / `precmd` フックで OSC 133 シーケンスを出力する設定を行うと、
  yatamux がコマンド完了を検知して IPC クライアントへ通知できる。
- **設計案**:
  - `VtProcessor` で `OSC 133;D;{exit_code}` を検出 → `bell` と同様のフラグ or コールバック
  - `ServerMessage::CommandFinished { pane: PaneId, exit_code: Option<i32> }` を追加
  - `send-keys --wait-for-prompt` オプション: 送信後に `CommandFinished` が来るまで待機してから終了
- **設定例（PowerShell）**:
  ```powershell
  function prompt { ... ; Write-Host -NoNewline "`e]133;D`a" }
  ```

#### サブタスク
- [x] `yatamux-terminal/src/vt.rs`: OSC 133;D パースとフラグ設定を追加
- [x] `yatamux-protocol/src/`: `ServerMessage::CommandFinished { pane: PaneId, exit_code: Option<i32> }` を追加
- [x] `yatamux-server/src/`: `CommandFinished` イベントを fan_out へ転送
- [x] `src/cli.rs`: `send-keys` に `--wait-for-prompt` フラグを追加（`CommandFinished` 受信まで待機）
- [x] `docs/test-plan-command-finished.md` 作成
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

---
### C-15: AIオーケストレーション向け Claude Code 統合スキル提供 【優先度: 中】
`using-cmux` 相当。Yatamux本体の機能追加ではなく、Claude Codeに「Yatamuxの操作方法」を教えるためのインターフェースを提供する。

- **概要**: Claude Codeが「ペイン分割 → サブエージェント（別のClaude Code）起動 → `capture-pane` で監視・結果回収」というパターンを自律的に行えるよう、専用の MCP (Model Context Protocol) サーバー、または Claude Code 用のスキル定義を同梱する。

#### サブタスク
- [ ] リポジトリ内に `integrations/claude-code/` などを設け、Yatamux操作用のプロンプトやコマンドのラッパースクリプトを作成
- [ ] AIに対して「別タスクは `yatamux split-pane` で隔離し、`yatamux send-keys` で指示を送り、`yatamux capture-pane` で回収せよ」と教えるシステムプロンプトの設計
- [ ] READMEに「AIサブエージェントの可視化と管理」に関するユースケース・チュートリアルを追記

### C-16: リモート監視用 WebSocket ブリッジ（スマホからの進捗モニタリング） 【優先度: 低】
`cmux-remote` 相当の機能。AIが自動作業している様子を、席を離れてiPhoneや別PCから監視できるようにする。

- **概要**: YatamuxのIPCサーバーに、リモートプレビュー用のWebSocketエンドポイント（読み取り専用）を追加し、ターミナルの描画更新をJSON等で配信する。
- **UI**: 配信されたデータを受信してブラウザ上でレンダリングする、簡易的なWebビューア（xterm.jsベース）を実装する。

#### サブタスク
- [ ] サーバー側で、既存の名前付きパイプ（Windows IPC）とは別に、WebSocketで接続を待ち受けるオプトインの機能を追加
- [ ] セキュリティを考慮し、リモートからは入力（Input）を受け付けない「読み取り専用（Read-only）セッション」の仕組みを導入
- [ ] 外部から状態を確認するための簡易PWA/Webクライアントのプロトタイプ作成

### C-30: 高水準 `exec` API（コマンド実行・終了コード・タイムアウトの一体化） 【優先度: 高】
現状の Agent 連携は `send-keys` + `--wait-for-prompt` が中心で、シェルプロンプトや OSC 133;D に依存している。
AI から見ると「1つのコマンド実行要求」を安全に扱いづらく、タイムアウト・終了コード・相関管理も不足している。

- **概要**: `yatamux exec --pane <id> --timeout <sec> -- <command>` のような高水準 API を追加し、
  入力送信・完了待機・終了コード取得・タイムアウト・失敗時エラー化を1回の要求にまとめる。
- **狙い**: Agent が `send-keys` の細かい流儀を知らなくても、単発ジョブを安全に実行できるようにする。

#### サブタスク
- [ ] `yatamux-protocol` に `Exec` / `ExecResult` 相当のメッセージ設計を追加
- [ ] IPC レベルで request_id を持てるようにし、複数同時実行時も応答を相関できるようにする
- [ ] `src/cli.rs` に `exec` サブコマンドを追加し、timeout / exit code / stderr 相当の扱いを決める
- [ ] 既存 `send-keys --wait-for-prompt` との責務分担を README に整理する

### C-31: ペイン状態メタデータ取得強化（cwd / busy / active / floating / last_update） 【優先度: 高】
現状の `list-panes --json` は `id / surface / title / cols / rows` のみで、
Agent が「どのペインに何を送るべきか」を安全に判断するには情報が足りない。

- **概要**: ペイン一覧や個別参照で、作業ディレクトリ、実行中コマンド、busy/idle、active、floating、
  最終更新時刻などのメタデータを取得できるようにする。
- **狙い**: 誤ったペインへの指示送信を減らし、Agent が現在の作業状況を自律判定できるようにする。

#### サブタスク
- [ ] `PaneInfo` を拡張するか、新しい `PaneState` API を追加する
- [ ] サーバー側で cwd / 実行中コマンド / busy 状態の保持方法を設計する
- [ ] `list-panes --json` の出力互換性ポリシーを決める（拡張 or 別コマンド）
- [ ] README に「Agent が pane 選択前に確認すべき情報」を記載する

### C-32: 出力購読 API（subscribe / diff stream）追加 【優先度: 高】
現状は `capture-pane` によるポーリングが前提で、長時間ジョブ監視や複数ペイン監視では効率が悪い。

- **概要**: 指定ペインの出力更新を IPC 経由で購読できる `subscribe-pane` / event stream を追加する。
  フルダンプではなく差分・新着行ベースで流せるようにする。
- **狙い**: Agent が `capture-pane` の連打なしで進捗監視・異常検知・完了判定を行えるようにする。

#### サブタスク
- [ ] `ServerMessage::Output` をそのまま購読するのか、Agent 向けに整形済みイベントを追加するのか設計する
- [ ] pane 単位の subscribe / unsubscribe を IPC で扱えるようにする
- [ ] 遅延クライアント向けに backlog / drop policy を設計する
- [ ] CLI で扱う場合のストリーム出力形式（JSON Lines など）を決める

### C-33: 明示的な割り込み・キャンセル API（Ctrl+C / terminate / close） 【優先度: 高】
現状でも `Ctrl+C` をキー送信すれば多くのケースは止められるが、
Agent 視点では「割り込み」「強制終了」「ペインを閉じる」が明示的な操作として分かれていた方が安全。

- **概要**: `interrupt-pane`、`terminate-pane`、`close-pane` などの制御 API を CLI / IPC に追加する。
- **狙い**: Agent が失敗したジョブやハングしたジョブを、キー入力に依存せず確実に停止できるようにする。

#### サブタスク
- [ ] `ClientMessage` に割り込み・終了系メッセージを追加する
- [ ] ConPTY / 子プロセス kill の扱いを整理し、graceful と force の差を設計する
- [ ] CLI サブコマンドとして `interrupt-pane` / `close-pane` の UX を定義する
- [ ] 実行中ジョブへの誤爆を減らすため、確認用メタデータ表示との組み合わせを検討する

### C-34: ペイン別名・ロール付け（alias / role） 【優先度: 中】
Agent 運用では `pane 3` のような数値 ID よりも、`tests` `server` `agent-a` のような論理名で扱えた方が事故が少ない。

- **概要**: ペインに alias / role を付与し、CLI / IPC で ID の代わりに参照できるようにする。
- **狙い**: Agent のプロンプトやスクリプトが、動的に変わる pane ID に依存しないようにする。

#### サブタスク
- [ ] `PaneInfo` に alias / role フィールドを追加する
- [ ] `rename-pane` または `set-pane-meta` 相当の CLI を追加する
- [ ] `send-keys` / `capture-pane` / `exec` などが alias 指定を受け付けるようにする
- [ ] セッション保存・復元時に alias / role を永続化する

### C-35: `capture-pane` の構造化 JSON 出力 【優先度: 高】
現状の `capture-pane --plain-text` は AI 向けとして有用だが、文字列ダンプのみでは
カーソル位置、visible 部分、scrollback、タイトルなどを安定して機械処理しにくい。

- **概要**: `capture-pane --json` を追加し、`visible_text`、`scrollback_tail`、`cursor`、`title`、
  `pane_id`、`active` などを構造化して返す。
- **狙い**: Agent が正規表現や行単位処理に頼りすぎず、安定して pane 状態を解釈できるようにする。

#### サブタスク
- [x] `PaneContent` の JSON 版レスポンス型を設計する
- [x] `docs/test-plan-capture-pane-json.md` を作成する
- [ ] 既存 `--plain-text` / 既定出力との住み分けを決める
- [ ] visible screen と scrollback の切り分けルールを明文化する
- [ ] README とテスト計画に Agent 向け利用例を追加する

### C-36: 待機条件 API の一般化（output regex / silence / exit） 【優先度: 中】
現状の待機は `send-keys --wait-for-prompt` に限定されており、
対話的ツールや独自プロンプトを使うプロセスでは Agent が完了判定しづらい。

- **概要**: `wait-for-output <regex>`、`wait-for-silence <duration>`、`wait-for-exit` など、
  汎用的な待機条件を CLI / IPC に追加する。
- **狙い**: Agent がシェル統合の有無に依存せず、ジョブ完了や安定状態を待てるようにする。

#### サブタスク
- [ ] 待機条件ごとのイベントソース（Output / PaneClosed / CommandFinished）を整理する
- [ ] regex マッチの対象範囲（screen のみ / scrollback 含む）を決める
- [ ] タイムアウト・キャンセルとの組み合わせ仕様を定義する
- [ ] `exec` / `subscribe` と共有できる内部待機基盤に寄せる

---

## リファクタリング

### ~~A-1: WM_KEYDOWN キー処理アーキテクチャ刷新（ハンドラ分離）~~ ✅ 対応済み 【優先度: 高】

`window.rs` の `WM_KEYDOWN` ハンドラは 460 行超の単一 if-else チェーンで、
保存プロンプト・ランチャー・コピーモード・ペインモード・グローバルショートカット・VT パススルー
が混在している。新機能追加のたびに既存ハンドラへの干渉リスクが高まる（B-6 の根本原因の一因）。

- **問題点**: 早期 `return` の連鎖でどのハンドラが優先されるか不明瞭。
  `skip_char` フラグが散在しており、設定漏れが WM_CHAR への二重送信を引き起こす。
- **解決策**: 各 UI モード・ショートカットグループを独立した `unsafe fn` に分離し、
  `dispatch_wm_keydown` がこれらを優先順位順に呼び出す Chain of Responsibility パターンを採用。
  `KeyInput` 構造体と `KeyConsumed` 列挙型で処理結果を型安全に伝播させ、
  `skip_char` の設定をディスパッチャが一元管理する。

```
dispatch_wm_keydown
  ├─ handle_save_prompt         (保存プロンプトが開いている間のみ有効)
  ├─ handle_layout_launcher     (レイアウトランチャーが開いている間のみ有効)
  ├─ handle_theme_launcher      (テーマランチャーが開いている間のみ有効)
  ├─ handle_copy_mode           (Copy モード中のみ有効)
  ├─ handle_pane_mode           (Pane モード中のみ有効)
  ├─ handle_global_shortcuts    (常時有効：Ctrl+B/F/P/Shift+E/O/W など)
  └─ handle_vt_passthrough      (VK_ → VT シーケンス変換)
```

#### サブタスク
- [x] `KeyInput` 構造体・`KeyConsumed` 列挙型を定義
- [x] 各ハンドラを `unsafe fn handle_*(state, hwnd, key) -> KeyConsumed` として分離
- [x] `dispatch_wm_keydown` を実装
- [x] `WM_KEYDOWN` ブロックをディスパッチャ呼び出しに置き換え
- [x] `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

### A-2: clone 監査と所有権ライフサイクルのリアーキテクチャ 【優先度: 高】

現状のコードベースには、Rust として妥当な「ハンドルの cheap clone (`Arc` / `mpsc::Sender`)」と、
設計上まだ整理余地のある「状態・文字列・レイアウトの clone」が混在している。
とくに `app.rs` の fan-out / hook / layout 適用まわり、`window.rs` の lock 脱出用 snapshot clone、
`layout.rs` の部分木 clone などは、状態ごとのライフサイクルを明示して再設計した方が見通しが良い。

- **問題意識**:
  - `clone` が「共有ハンドルの複製」なのか「所有権設計の逃げ」なのか、箇所ごとの意味が揃っていない
  - lock を早く解放する目的で状態全体を clone しており、責務境界が曖昧になっている箇所がある
  - 一時コマンド文字列やレイアウトノードが必要以上に複製され、ライフサイクルが読み取りづらい
- **狙い**:
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
- [ ] `src/app.rs` の fan-out / layout-switch 経路を、move 中心で書けるよう再設計する
- [ ] `window.rs` の UI state 取得を「必要部分のみ抽出する view struct」に寄せ、全体 snapshot clone を減らす
- [x] `layout.rs` の部分木操作を `Clone` 前提ではなく `mem::replace` / `Option::take` ベースに置き換えられるか検証する
- [ ] 文字列 payload を `String` clone でばら撒いている箇所を `Arc<str>` / 所有権移動で削減できるか検証する
- [x] `pane.rs` ↔ `session.rs` 間の stringly typed な内部通知を `PaneEvent` enum に置き換える
- [x] `docs/test-plan-typed-pane-events.md` を追加する
- [x] `cargo clippy -p yatamux-server -- -D warnings` と `cargo test -p yatamux-server` を通す
- [x] 変更後に `cargo clippy -- -D warnings` と主要テストを通し、回帰のない ownership 境界に整える
