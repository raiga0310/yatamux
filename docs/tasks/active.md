# yatamux アクティブタスク

未完了の機能・バグ・ドキュメントタスクをここで管理する。
完了済みの履歴は `docs/tasks/archive-2026-03-30.md`, `docs/tasks/archive-2026-04-04.md` を参照。

## CI / インフラ

### CI-1: GitHub Actions の Node.js 20 → 24 移行 【優先度: 低・期限あり】

GitHub Actions ランナー上の Node.js 20 が 2026-09-16 に削除される。

- **期限**: 2026-06-02 にデフォルトが Node.js 24 に切り替わる（強制移行は 2026-09-16）
- **対象ファイル**: `.github/workflows/` 内のすべてのワークフロー
- **現在使用中のアクション**（要バージョン確認）:
  - `actions/checkout@v4` → Node.js 24 対応版に更新
  - `actions/cache@v4` → 同上
  - `dtolnay/rust-toolchain@stable` → Node.js 非依存のため影響なし
  - `softprops/action-gh-release@v2` → 要確認
- **対応方針**:
  - 各アクションの最新版リリースノートを確認し、Node.js 24 対応済みバージョンへ `@vX` を更新する
  - または `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24=true` を env に設定して早期検証する

#### サブタスク

- [ ] 使用中アクションの最新バージョンを調査し、Node.js 24 対応済みか確認する
- [ ] ワークフローファイルのアクションバージョンを更新する
- [ ] CI が正常に通ることを確認する

## バグ / 挙動

### ~~F-4: バックグラウンドペインの通知が実用的でない~~ ✅ 対応済み

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

### ~~F-5: ペイン分割・リサイズ時の描画崩れ~~ ✅ 対応済み 【優先度: 高】

ペインを分割したとき、またはペインをリサイズしたときに描画が正しく更新されない。

- **症状**:
  - ペイン分割直後に旧ペイン領域が残像として残る
  - リサイズ後にセル・カーソル・罫線がずれる
- **調査ポイント**:
  - `compute_rects()` → `InvalidateRect` のタイミングが split/resize 後に呼ばれているか
  - `Grid::resize()` で `dirty` フラグが全行セットされているか
  - ConPTY への `Resize` メッセージと GUI 側 `Grid` のサイズが一致しているか（`WM_SIZE` → `ClientMessage::Resize` フロー）
  - 分割後に新旧ペイン両方の `dirty` がクリアされずに残っていないか

#### サブタスク

- [x] `docs/test-plan-pane-render-split-resize.md` を作成してテストケースを列挙する
- [x] `Grid::resize()` で dirty フラグが全行セットされることを単体テストで確認する
- [x] split/resize 後の `InvalidateRect` 呼び出しパスをトレースして抜け漏れを修正する
- [x] 修正後に Clippy・全テスト・rustfmt を通す

### ~~F-7: セッション復元時に作業ディレクトリが HOME になる~~ ✅ 対応済み 【優先度: 高】

セッション保存時のペイン作業ディレクトリ（cwd）が保持されず、復元後は常に HOME で起動する。
`claude --continue` や `codex resume --last` など、ディレクトリ依存のコマンドが正しく動かない。

- **原因**: `session.toml` に cwd を保存する仕組みがない。`LayoutNodeDef` に cwd フィールドがなく、`CommandBuilder` にも作業ディレクトリを渡していない。
- **取得方法（Windows）**: `child_pid` で追跡している ConPTY 直接子プロセス（cmd.exe）の cwd を Windows プロセスメモリ API（`NtQueryInformationProcess` + `ReadProcessMemory`）で読み取る。
- **保存**: `LayoutNodeDef::Leaf` に `cwd: Option<String>` を追加し、`LayoutSnapshot` に含める。
- **復元**: `ClientMessage::CreatePane.working_dir` に cwd を渡す → `PtySession::spawn` で `CommandBuilder::cwd()` に設定。

#### サブタスク

- [x] `docs/test-plan-session-cwd.md` を作成してテストケースを列挙する
- [x] `crates/terminal/src/process.rs` に `find_process_cwd(parent_pid: u32) -> Option<String>` を実装する（Windows のみ；非 Windows は `None` スタブ）
- [x] `SaveAndQuit` フロー（`bridge.rs`）で cwd を収集し `pane_commands` と同様に補完する
- [x] `crates/client/src/session.rs` の `LayoutNodeDef::Leaf` に `cwd: Option<String>` を追加し保存・読み込みに含める
- [x] `layout_restore.rs` で復元時に cwd を `CreatePane.working_dir` に設定する
- [x] 修正後に Clippy・全テスト・rustfmt を通す

### ~~F-6: ペインによってリサイズの方向が反転して見える~~ ✅ 対応済み

`adjust_ratio_for_dir` 内で `in_second` フラグによる delta 符号反転を削除し、
常に `ratio += delta` に統一。`crates/client/src/layout/tree.rs:368` 参照。

### ~~F-8: ステータスバーの CPU/RAM 表示が 1 桁でちらつく~~ ✅ 対応済み 【優先度: 中】

ステータスバー上の CPU / RAM 使用率が 1 桁のとき、表示幅が縮んで更新ごとに見た目が揺れる。

- **症状**:
  - `CPU 9%` / `RAM 8%` のような 1 桁表示で文字列幅が変わり、定期更新のたびにステータスバーがちらついて見える
- **原因候補**:
  - 可変桁数のパーセンテージ文字列をそのまま描画しており、再描画時のレイアウト幅が毎回変動している
- **対応方針**:
  - `09%` のように 2 桁固定で 0-padding し、CPU / RAM の表示幅を安定させる

#### サブタスク

- [x] `docs/test-plan-status-bar.md` に CPU / RAM 使用率が 1 桁のときも表示幅が固定されるケースを追記する
- [x] ステータスバーの CPU / RAM 表示フォーマットを 2 桁固定に変更し、0-padding する
- [x] 更新時の再描画で表示揺れが抑制されることを確認する

## 機能


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
- [x] `src/cli.rs` に最小の `exec` サブコマンドを追加し、timeout / wait condition / exit code 伝搬の基本を実装する
- [x] 既存 `send-keys --wait-for-prompt` との責務分担を README に整理する
- [ ] `exec` を protocol レベル request / result に引き上げ、CLI ローカル実装から卒業させる

### C-31: ペイン状態メタデータ取得強化（cwd / busy / active / floating / last_update） 【優先度: 高】

現状の `list-panes --json` は `id / surface / title / cols / rows` のみで、
Agent が「どのペインに何を送るべきか」を安全に判断するには情報が足りない。

- **概要**: ペイン一覧や個別参照で、作業ディレクトリ、実行中コマンド、busy/idle、active、floating、
  最終更新時刻などのメタデータを取得できるようにする。
- **狙い**: 誤ったペインへの指示送信を減らし、Agent が現在の作業状況を自律判定できるようにする。

#### サブタスク

- [x] `PaneInfo` を後方互換な拡張として広げ、`list-panes --json` で `cwd / command / busy / last_output_unix_ms` を返す
- [x] サーバー側で cwd / 実行中コマンド / busy / 最終出力時刻の保持方法を実装する
- [x] `list-panes --json` の出力は既存フィールドを保ったまま JSON 拡張で互換維持する方針に決める
- [x] README に「Agent が pane 選択前に確認すべき情報」を記載する
- [ ] `active / floating` を GUI 状態と同期して返せるようにする

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

- [x] `ClientMessage` に `InterruptPane` を追加し、明示的な割り込み API を入れる
- [ ] ConPTY / 子プロセス kill の扱いを整理し、graceful と force の差を設計する
- [x] CLI サブコマンドとして `interrupt-pane` / `close-pane` の UX を実装する
- [x] 誤爆を減らすため、`list-panes --json` のメタデータ確認と併用できる形にする

### C-34: ペイン別名・ロール付け（alias / role） 【優先度: 中】

Agent 運用では `pane 3` のような数値 ID よりも、`tests` `server` `agent-a` のような論理名で扱えた方が事故が少ない。

- **概要**: ペインに alias / role を付与し、CLI / IPC で ID の代わりに参照できるようにする。
- **狙い**: Agent のプロンプトやスクリプトが、動的に変わる pane ID に依存しないようにする。

#### サブタスク

- [ ] `PaneInfo` に alias / role フィールドを追加する
- [ ] `rename-pane` または `set-pane-meta` 相当の CLI を追加する
- [ ] `send-keys` / `capture-pane` / `exec` などが alias 指定を受け付けるようにする
- [ ] セッション保存・復元時に alias / role を永続化する

### ~~C-35: `capture-pane` の構造化 JSON 出力~~ ✅ 対応済み 【優先度: 高】

現状の `capture-pane --plain-text` は AI 向けとして有用だが、文字列ダンプのみでは
カーソル位置、visible 部分、scrollback、タイトルなどを安定して機械処理しにくい。

- **概要**: `capture-pane --json` を追加し、`visible_text`、`scrollback_tail`、`cursor`、`title`、
  `pane_id`、`active` などを構造化して返す。
- **狙い**: Agent が正規表現や行単位処理に頼りすぎず、安定して pane 状態を解釈できるようにする。

#### サブタスク

- [x] `PaneContent` の JSON 版レスポンス型を設計する
- [x] `docs/test-plan-capture-pane-json.md` を作成する
- [x] 既存 `--plain-text` / 既定出力との住み分けを決める
- [x] visible screen と scrollback の切り分けルールを明文化する
- [x] README とテスト計画に Agent 向け利用例を追加する

### C-36: 待機条件 API の一般化（output regex / silence / exit） 【優先度: 中】

現状の待機は `send-keys --wait-for-prompt` に限定されており、
対話的ツールや独自プロンプトを使うプロセスでは Agent が完了判定しづらい。

- **概要**: `wait-for-output <regex>`、`wait-for-silence <duration>`、`wait-for-exit` など、
  汎用的な待機条件を CLI / IPC に追加する。
- **狙い**: Agent がシェル統合の有無に依存せず、ジョブ完了や安定状態を待てるようにする。

#### サブタスク

- [x] 待機条件ごとのイベントソース（Output / PaneClosed / CommandFinished）を CLI 実装として整理する
- [x] regex マッチは当面 `capture-pane --plain-text` の内容を対象にする方針を決める
- [x] タイムアウト・キャンセルとの組み合わせ仕様を CLI 引数として定義する
- [ ] `exec` / `subscribe` と共有できる内部待機基盤に寄せる

### ~~C-37: エージェント向け環境変数伝搬 + AGENTS.md 整備~~ ✅ 対応済み 【優先度: 中】

Claude Code / Codex が yatamux 上で動作していることを検出し、ペイン操作 CLI の使い方を
自動的に知れるようにする。

- **概要**:
  1. `PtySession::spawn` で `YATAMUX=1` / `TERM_PROGRAM=yatamux` / `YATAMUX_SESSION` を `CommandBuilder` に設定し、
     子プロセス（シェル・エージェント）に伝搬する。
  2. `AGENTS.md` を整備し、Codex が自動読み込みできる yatamux 操作ガイドを記述する。
     （Claude Code は既存の `CLAUDE.md` が対応）
- **狙い**: エージェントが「yatamux 上にいる」を環境変数で検出し、
  `yatamux list-panes` / `send-keys` / `capture-pane` / `split-pane` などを
  プロンプト注入なしに利用できるようにする。

#### サブタスク

- [x] `docs/test-plan-agent-env.md` を作成する
- [x] `crates/terminal/tests/pty_integration.rs` に A-6 テストを追加（env var 伝搬確認）
- [x] `crates/terminal/src/pty.rs` に `YATAMUX` / `TERM_PROGRAM` / `YATAMUX_SESSION` の設定を追加する
- [x] `AGENTS.md` に Codex 向け操作ガイドを記述する

### C-38: セルフアップデート機能（`yatamux update`） 【優先度: 中】

エージェントが呼び出せる `yatamux update` サブコマンドを追加する。
GitHub Releases からバイナリを取得し、実行中インスタンスのセッションを保持したままアップデートする。

- **フロー**: GitHub Releases API で最新バージョン確認 → バイナリ + checksums.txt ダウンロード → SHA256 検証 → IPC 経由で実行中インスタンスに `SaveAndQuit` 送信 → `--apply-update` ヘルパーモードでバイナリ置換 → 新インスタンス起動 → `session.toml` から自動復元
- **テスト計画**: `docs/test-plan-self-update.md`（Codex と壁打ちして策定）

#### サブタスク

- [x] `WM_CLOSE` 内の保存処理を `save_session(store, path)` に切り出す（`session.rs` に `pub fn save_session` として追加）
- [x] `ClientMessage::SaveAndQuit` を `crates/protocol/src/message.rs` に追加する
- [x] `SaveAndQuit` ハンドラをサーバー・app 側に実装する（`server/handlers/mod.rs` + `app/bridge.rs`）
- [x] `yatamux --apply-update <pid> <new_path>` の内部ヘルパーモードを `src/main.rs` に追加する
- [x] `yatamux update` サブコマンドを `src/cli.rs` に実装する（バージョン確認・ダウンロード・SHA256・IPC 連携）
- [x] `.github/workflows/release.yml` を追加する（タグ push → ビルド → checksums.txt 付き Release 自動作成）
- [x] unit test（TC-01〜08）を実装する（`src/update.rs`・`crates/protocol/src/message.rs`・`crates/client/src/session.rs`）
- [x] mock HTTP を使った TC-09 相当のダウンロード〜チェックサム検証テストを追加する
- [x] `replace_executable()` を共有関数に切り出し、TC-14 の rename / stale `.bak` 置換コアを自動テスト化する
- [ ] IPC 経由 SaveAndQuit → `session.toml` 書き出し（TC-10）を自動化する
- [ ] 新プロセス起動後の session 復元（TC-11）を自動化する
- [ ] checksum 不一致 / quit timeout の end-to-end guard（TC-12, TC-13）を自動化する

### ~~C-39: 入力プロンプト UX 改善（save prompt の履歴 / Emacs キーバインド / ヒント表示）~~ ✅ 対応済み 【優先度: 中】

現時点で実在するアプリ内入力欄はレイアウト保存プロンプト（`save_prompt`）のみなので、
まずここに履歴・Emacs 風編集・ヒント表示を実装した。
`PromptState` を導入し、将来的な他の入力欄にも流用できる行編集モデルへ整理した。

- **実装内容**:
  - 保存プロンプトの入力履歴をセッション内で最大 20 件保持する
  - `Up` / `Down` で履歴を前後移動し、末尾を抜けると編集中の下書きへ戻る
  - `Ctrl+A` / `Ctrl+E` / `Ctrl+B` / `Ctrl+F` / `Ctrl+K` / `Ctrl+U` / `Ctrl+W` / `Ctrl+Y` と `Left` / `Right` / `Backspace` に対応する
  - 保存プロンプト下部に履歴 / Emacs 編集キーのヒントを 2 行で表示する
  - IME 確定文字列はプロンプト表示中にペインへ誤送信せず、入力欄へ反映する

#### サブタスク

- [x] `docs/test-plan-prompt-input-ux.md` を作成し、履歴移動・編集中入力への復帰・Emacs キーバインド・ヒント表示のテストケースを列挙する
- [x] 保存プロンプト向けの履歴モデルを設計し、保存範囲をセッション内・最大件数を 20 件に決める
- [x] 履歴参照中に `Up` / `Down` で前後移動し、編集中入力へ戻れる UI 状態遷移を実装する
- [x] 保存プロンプト編集に Emacs キーバインドを実装し、既存ショートカットと競合しないキーに整理する
- [x] 入力欄に履歴 / Emacs 操作のヒントを表示する
- [x] 修正後に Clippy・関連テスト・rustfmt を通す

### B-1: yatamux ペイン内から `yatamux update` が失敗する 【優先度: 高】

yatamux ペインのシェルから `yatamux update` を実行すると exit code 1 で即終了し、アップデートが動かない。

- **症状**: `yatamux update` が exit 1 で終了する再現があり、どこで失敗しているかの切り分けが未完了
- **現状確認（2026-04-06）**:
  - `src/main.rs` は `cli::update(DEFAULT_SESSION)` を呼んでおり、CLI サブコマンドのセッション選択は `YATAMUX_SESSION` には依存していない
  - `crates/terminal/src/pty.rs` では `YATAMUX=1` / `TERM_PROGRAM=yatamux` / `YATAMUX_SESSION` を子プロセスへ伝搬するようになった
- **現時点の見立て**:
  - 旧仮説の「環境変数未設定で IPC パイプ名を特定できない」は実装と合っていない
  - 失敗原因は IPC 接続、GitHub Releases 取得、`SaveAndQuit`、`--apply-update` のいずれかで、利用者向けエラーメッセージ不足も問題になっている可能性が高い

#### サブタスク

- [ ] 2026-04-06 時点の再現手順を取り直し、ペイン内 / 外それぞれの stderr と exit code を採取する
- [x] `src/cli.rs` の `update()` で利用者向けエラーメッセージが欠けている失敗パスを洗い出す
- [ ] IPC 接続 / `SaveAndQuit` / `--apply-update` / ダウンロード検証のどこで落ちるかを切り分ける

## ドキュメント

### ~~D-1: `docs/test-plan-*` と実装済み自動テストの同期~~ ✅ 対応済み 【優先度: 中】

`docs/` 配下のテスト計画には、実装変更後も古い前提のまま残っているケースがある。
README / CLAUDE は今回実装準拠に更新するが、個別のテスト計画書は別途同期しないと
「何を自動テストで担保しているのか」を誤読しやすい。

- **確認したズレ**
  - `docs/test-plan-command-finished.md`
    - 旧 `__cmd_finished__:` 文字列通知を前提にしているが、実装は `PaneEvent::CommandFinished` に移行済み
  - `docs/test-plan-capture-pane.md`
    - `target=0` でアクティブペインを取る前提、存在しない pane で空文字を返す前提が残っている
    - 実装は存在しない pane に対して `Error` を返す
  - `docs/test-plan-pane-close.md`
    - 最後の 1 ペインで `Ctrl+Shift+W` が no-op という旧仕様のまま
    - 実装は `ClosePane` を送り、最後の 1 ペインならアプリ終了経路に入る
  - `docs/test-plan-status-bar.md`
    - Pane モードで `H/J/K/L` フォーカス移動という旧仕様が残っている
    - 現実の Pane モードは `S`, `L`, `V`, `<`/`>`, `+`/`-` など中心で、該当ユニットテスト前提も古い
  - `docs/test-plan-layout-launcher.md`
    - `list_layouts()` を直接呼ぶ想定で書かれているが、実テストは `LayoutConfig::list_layouts` 相当のロジックを別形で検証している

#### サブタスク

- [x] `docs/test-plan-command-finished.md` を typed `PaneEvent` ベースのテスト計画に更新する
- [x] `docs/test-plan-capture-pane.md` を現行エラー挙動と `capture-pane --plain-text/--json` 前提に合わせて更新する
- [x] `docs/test-plan-pane-close.md` を「最後の 1 ペインで終了」に合わせて更新する
- [x] `docs/test-plan-status-bar.md` を現行 Pane モードのキー割り当てと実在ユニットテストに合わせて更新する
- [x] `docs/test-plan-layout-launcher.md` の自動テスト対象表現を現行実装に合わせて整理する
