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

### ~~C-6: 高度な Unicode / 絵文字対応~~ ✅ 部分対応済み 【優先度: 中】
- 24ビット True Color の完全サポート確認
- ゼロ幅結合子（ZWJ）を使った絵文字（👨‍💻 等）の正確な幅計算
- Nerd Fonts グリフ（U+E000–U+F8FF）の幅を2セルとして扱うオプション
- 双方向テキスト（BiDi）への基本対応
- **サブタスク**:
  - [x] ZWJ シーケンス（U+200D 連結）の幅を1セルとして扱う（`combine_with_last_cell()` / `last_grapheme_ends_with_zwj()`）
  - [x] Variation Selector（VS-15/VS-16）による幅切り替え対応（`apply_vs16()`）
  - [x] Nerd Fonts グリフ（U+E000–U+F8FF）を2セル幅に扱う `nerd_fonts_wide` オプションを追加
  - [ ] BiDi 基本対応（RTL マーカーを幅0で処理）

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
- [ ] `layout_config.rs` に、利用可能なレイアウトファイル一覧（ファイル名のリスト）を取得する `list_layouts()` 関数を追加
- [ ] クライアント側に、中央ポップアップでリストを描画し、上下キーで選択するUIコンポーネントを実装
- [ ] レイアウト切り替えコマンドの発火（既存ペインの安全な終了処理と、新しい `LayoutConfig` に基づくペイン生成フローの構築）
- [ ] ペインモード中のキーバインド（例: `L`）でレイアウトランチャーを起動できるように配線
- [ ] `docs/test-plan-layout-launcher.md` を作成し、UIの表示・選択・適用・キャンセルのテストケースを定義

### ~~F-10: CLIヘルプ (`--help`) とコマンドライン引数解析の実装~~ ✅ 対応済み 【優先度: 高】

現状、`yatamux --help` や `--version` といったターミナルアプリケーションとして基本的なCLIオプションが提供されていない。
ユーザーが利用可能な起動オプション（`--layout` など）をターミナル上で簡単に確認できるようにする。

- **実装方針**: `clap` クレートの derive 機能を利用した宣言的な引数定義。`--help` / `--version` は自動生成。

#### サブタスク
- [x] `Cargo.toml` に `clap` クレートを追加（derive機能を利用すると宣言的で管理しやすい）
- [x] `src/main.rs` の `Cli` 構造体で引数を定義し、`clap::Parser` で解析
- [x] サポートする引数の定義: `--help` (自動生成), `--version` (自動生成), `--layout <NAME>`
- [x] `list-panes` / `send-keys` も `#[command(subcommand)]` で整理
