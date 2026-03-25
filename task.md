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

- [ ] **Step-1（設計）**: `docs/design-notification-backend.md` に設計方針・トレイト定義案・フォーカス検知方法を記述
- [ ] **Step-2（テスト計画）**: `docs/test-plan-notification-backend.md` を作成し TC を列挙
- [ ] **Step-3（トレイト定義）**: `yatamux-client` に `NotificationBackend` トレイトを追加
- [ ] **Step-4（InternalToast 移植）**: 既存 `paint_toasts()` 経路を `InternalToast` 実装に切り替え
- [ ] **Step-5（NativeToast 実装）**: WinRT or バルーンチップで OS ネイティブ通知を送出
- [ ] **Step-6（フォーカス切り替え）**: `WM_ACTIVATEAPP` でフォーカス状態を検知し、バックエンドを動的に切り替え
- [ ] **Step-7（テスト・lint）**: `cargo test && cargo clippy -- -D warnings && cargo fmt --check`

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

### C-1: モードベース UI とキーバインドヒント表示 【優先度: 高】
- Zellij のように画面下部にコンテキスト依存のキーバインド一覧を表示する
- 導入するモード例：ペイン操作モード、タブ操作モード、リサイズモード、コピーモード
- 各モードで有効なコマンドをリアルタイム表示することで学習コストを大幅に削減
- **設計メモ**: ステータスバー領域を確保し、`WM_PAINT` の最後にモード情報を描画

### C-2: OSC 52 対応とクリップボード統合 【優先度: 高】
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

### C-4: セッション永続化と自動復元 【優先度: 中】
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

### C-5: 宣言的レイアウトとプロジェクト起動定義 【優先度: 中】
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

### C-6: 高度な Unicode / 絵文字対応 【優先度: 中】
- 24ビット True Color の完全サポート確認
- ゼロ幅結合子（ZWJ）を使った絵文字（👨‍💻 等）の正確な幅計算
- Nerd Fonts グリフ（U+E000–U+F8FF）の幅を2セルとして扱うオプション
- 双方向テキスト（BiDi）への基本対応
- **現状**: `CjkWidthConfig` で East Asian Ambiguous を制御中。ZWJ 結合絵文字は未対応
- **サブタスク**:
  - [ ] ZWJ シーケンス（U+200D 連結）の幅を1セルとして扱う
  - [ ] Variation Selector（VS-15/VS-16）による幅切り替え対応
  - [ ] BiDi 基本対応（RTL マーカーを幅0で処理）

### C-7: 効率的なスクロールバックバッファ 【優先度: 低】
- 数万行のスクロールバック履歴を保持してもメモリ・パフォーマンスが劣化しない設計
- 現状の `Vec<Row>` をより効率的なデータ構造に置き換える検討
  - 候補: Rope 構造（`O(log n)` で挿入・削除）、Ring buffer + ディスク書き出し
- スクロールバック内の外部エディタ起動（`$EDITOR` での編集）
- **サブタスク**:
  - [ ] `ropey` クレートを追加し `ScrollbackBuffer` 型として `Vec<Row>` を置き換え
  - [ ] スクロールバック行数上限（例: 50,000 行）と LRU エビクションの実装
  - [ ] 外部エディタ起動（`EDITOR` 環境変数 → 一時ファイルへ書き出し → 起動）

### ~~C-8: プラグイン / 拡張システムの設計~~ ✅ 対応済み 【優先度: 低】
- Candidate B（シェルフック + 既存 IPC）を採用
- `%APPDATA%\yatamux\config.toml` の `[hooks]` セクションで設定
- `on_pane_created` / `on_pane_closed` フックを `cmd.exe /C` で非同期発火
- 環境変数: `YATAMUX_PANE_ID`, `YATAMUX_SESSION`

### C-9: シェル終了時にペインを自動削除 【優先度: 高】
- ペイン内のシェルで `exit` などを実行して PTY プロセスが終了したとき、そのペインを自動的に削除する
- 現状: PTY 終了時に `ServerMessage::Notification` が発火するが、ペインは残ったまま
- 最後の 1 枚のペインが閉じたときはアプリ終了
- **実装方針**: PTY 読み取りタスクの EOF 時に `ClosePane` を送信 or `PaneExited` メッセージを追加して `app.rs` で処理

### C-10: ペイン幅調整キーバインド 【優先度: 中】
- ペインモード中にキー操作で分割比率（ratio）を変更できるようにする
- 例: `<` / `>` で 5% 単位で増減
- **実装方針**: `LayoutNode` の `ratio` を増減する `adjust_ratio(pane_id, delta)` を `PaneStore` に追加し、`compute_rects()` が参照することで即時再描画
