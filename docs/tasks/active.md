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
- **テスト計画**: `docs/test-plan-ci-node24.md`
- **現状メモ（2026-04-06）**:
  - `Bump Version` は master push 後の run で success を確認済み
  - `Release` workflow は Node 24 対応版へ更新済みだが、更新後定義での実行確認は未採取

#### サブタスク

- [x] 使用中アクションの最新バージョンを調査し、Node.js 24 対応済みか確認する
- [x] ワークフローファイルのアクションバージョンを更新する
- [x] `Bump Version` workflow が正常に通ることを確認する
- [ ] `Release` workflow が更新後定義で正常に通ることを確認する

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


### ~~C-15: AIオーケストレーション向け Claude Code 統合スキル提供~~ ✅ 対応済み 【優先度: 中】

`using-cmux` 相当。Yatamux本体の機能追加ではなく、Claude Codeに「Yatamuxの操作方法」を教えるためのインターフェースを提供する。

- **概要**: Claude Codeが「ペイン分割 → サブエージェント（別のClaude Code）起動 → `capture-pane` で監視・結果回収」というパターンを自律的に行えるよう、専用の MCP (Model Context Protocol) サーバー、または Claude Code 用のスキル定義を同梱する。
- **テスト計画**: `docs/test-plan-claude-code-skill.md`

#### サブタスク

- [x] リポジトリ内に `integrations/claude-code/` などを設け、Yatamux操作用のプロンプトやコマンドのラッパースクリプトを作成
- [x] AIに対して「別タスクは `yatamux split-pane` で隔離し、`yatamux send-keys` で指示を送り、`yatamux capture-pane` / `subscribe-pane` で回収せよ」と教えるシステムプロンプトの設計
- [x] READMEに「AIサブエージェントの可視化と管理」に関するユースケース・チュートリアルを追記

### C-16: リモート監視用 WebSocket ブリッジ（スマホからの進捗モニタリング） 【優先度: 低】

`cmux-remote` 相当の機能。AIが自動作業している様子を、席を離れてiPhoneや別PCから監視できるようにする。

- **概要**: YatamuxのIPCサーバーに、リモートプレビュー用のWebSocketエンドポイント（読み取り専用）を追加し、ターミナルの描画更新をJSON等で配信する。
- **UI**: 配信されたデータを受信してブラウザ上でレンダリングする、簡易的なWebビューア（xterm.jsベース）を実装する。

#### サブタスク

- [ ] サーバー側で、既存の名前付きパイプ（Windows IPC）とは別に、WebSocketで接続を待ち受けるオプトインの機能を追加
- [ ] セキュリティを考慮し、リモートからは入力（Input）を受け付けない「読み取り専用（Read-only）セッション」の仕組みを導入
- [ ] 外部から状態を確認するための簡易PWA/Webクライアントのプロトタイプ作成

### C-41: IPC ハードニング（Named Pipe ACL / 認証 / 入力制限） 【優先度: 高】

`\\.\pipe\yatamux-<session>` は `send-keys` / `exec` / `capture-pane` / `subscribe-pane` を外部から叩ける強力な操作面になっている一方、
現状の IPC には「誰が接続してよいか」「どこまで送ってよいか」を厳密に制御する仕組みがまだ弱い。
ローカル他プロセスからの操作・盗聴・攪乱に耐える前提を明示し、Named Pipe 生成時の ACL、クライアント認証、入力制限、失敗時の監査ログをセットで固める。

- **概要**: Named Pipe のアクセス制御を同一ユーザまたは同一ログオンセッション相当に絞り、CLI / エージェント側とは秘密値またはハンドシェイクで相互確認する。加えて oversized JSON Lines や過剰購読に対する上限を入れ、失敗時は黙って切断せず利用者向けエラーと監査ログを残す。
- **狙い**: IPC を「便利だがローカル誰でも叩ける裏口」にしない。今後の Agent / MCP 連携の土台として、最低限の安全性と運用可能性を確保する。
- **テスト計画**: `docs/test-plan-ipc-hardening.md`

#### サブタスク

- [x] `docs/test-plan-ipc-hardening.md` を作成し、同一ユーザ許可 / 別ユーザ拒否 / 認証失敗 / 互換モードのテストケースを列挙する
- [x] `crates/server/src/ipc.rs` の Named Pipe セキュリティ属性を見直し、少なくとも同一ユーザまたは同一ログオンセッションに限定する
- [x] 既存 CLI 互換を壊しすぎない移行策つきで、サーバー・クライアント間の認証（nonce 付き pipe 名または handshake token）を導入する
- [x] `BufReader.lines()` 前提の JSON Lines 入力に 1 メッセージ上限を設け、oversize メッセージを明示エラーで拒否する
- [x] 遅延購読者や過剰送信クライアントに対する切断 / rate limit / lagged 連携方針を決め、黙った break を減らす（ipc.rs の OK(None)/Err/Closed に debug/info ログを追加）
- [ ] 認証失敗・権限不足・入力制限超過が Windows integration / E2E テストで再現・検証できるようにする

### C-42: IPC プロトコルの handshake / version / capabilities 固定化 【優先度: 高】

`request_id` を持つコマンドは増えてきたが、CLI / Agent 統合を継続的に育てるには
「最初に何を名乗るか」「どの機能を話せるか」「古いクライアントが来たときにどう失敗するか」を protocol として固定する必要がある。

- **概要**: `yatamux-protocol` に初回 handshake、`protocol_version`、`capabilities`、共通エラー表現を導入し、単発要求とストリーミング要求の両方で相関・失敗・非対応機能を機械判定できるようにする。
- **狙い**: CLI / MCP / 将来の WebSocket bridge が capability mismatch を事前検出できるようにし、後方互換を壊す変更を protocol レベルで管理できるようにする。
- **テスト計画**: `docs/test-plan-ipc-protocol.md`

#### サブタスク

- [x] `docs/test-plan-ipc-protocol.md` を作成し、handshake 成功 / version 不一致 / capability 不足 / error envelope のケースを列挙する
- [x] `crates/protocol` に `protocol_version` / `capabilities` を含む handshake request / response を追加する
- [x] `exec` / `wait-pane` / `subscribe-pane` / 制御 API を横断して、`request_id`・成功応答・失敗応答・タイムアウト表現を揃える
- [x] `lagged` 発生時の再同期契約（`capture-pane --json` へのフォールバックを含む）を protocol と CLI 両方で明文化する（`docs/protocol-ipc.md` に記述）
- [x] 旧 CLI ↔ 新 server、新 CLI ↔ 旧 server の互換性テストまたは golden fixture を追加する（`crates/protocol/src/lib.rs` の `mod golden`）
- [x] `docs/protocol-ipc.md` などの形で、外部ツール向けの protocol 要約ドキュメントを整備する

### C-43: `yatamux-mcp` エージェントブリッジの公式化 【優先度: 中】

README には Claude Code 向けのオーケストレーション bundle があるが、
AI エージェントから見ると「どの pane API をどう呼べば安全か」をツール面で固定したインターフェースはまだない。
まずは既存 CLI を背後で使う安全プロキシとして `yatamux-mcp` を用意し、将来的な direct IPC 実装の土台にする。

- **概要**: `integrations/yatamux-mcp/` などに MCP サーバーの試作を追加し、pane 一覧・worker pane 確保・コマンド実行・状態取得・監視・停止系 API を MCP ツールとして公開する。
- **狙い**: Claude Code / Codex / 将来の他エージェントが、README の「儀式」をコピペせずに安全なツール呼び出しとして yatamux を使えるようにする。
- **テスト計画**: `docs/test-plan-yatamux-mcp.md`

#### サブタスク

- [ ] `docs/test-plan-yatamux-mcp.md` を作成し、worker pane 作成 / exec / subscribe / lagged 再同期 / interrupt のケースを列挙する
- [ ] まずは direct IPC ではなく既存 CLI を呼ぶラッパーとして `yatamux-mcp` の最小プロトタイプを作る
- [ ] `list_panes` / `ensure_worker_pane` / `exec` / `capture_pane` / `subscribe_pane` / `interrupt` / `terminate` / `close` を最小ツールセットとして設計する
- [ ] alias / role / working dir / command filter を使った allowlist と監査ログのモデルを定義する
- [ ] `lagged` を受け取ったら `capture-pane --json` で再同期する標準フローをブリッジ側で吸収する
- [ ] 既存 README の Claude Code bundle 記述を、MCP ベースの汎用エージェント統合ドキュメントへ段階的に寄せる

### ~~C-30: 高水準 `exec` API（コマンド実行・終了コード・タイムアウトの一体化）~~ ✅ 対応済み 【優先度: 高】

現状の Agent 連携は `send-keys` + `--wait-for-prompt` が中心で、シェルプロンプトや OSC 133;D に依存している。
AI から見ると「1つのコマンド実行要求」を安全に扱いづらく、タイムアウト・終了コード・相関管理も不足している。

- **概要**: `yatamux exec --pane <id> --timeout <sec> -- <command>` のような高水準 API を追加し、
  入力送信・完了待機・終了コード取得・タイムアウト・失敗時エラー化を1回の要求にまとめる。
- **狙い**: Agent が `send-keys` の細かい流儀を知らなくても、単発ジョブを安全に実行できるようにする。
- **テスト計画**: `docs/test-plan-wait-and-exec.md`

#### サブタスク

- [x] `yatamux-protocol` に `Exec` / `ExecResult` 相当のメッセージ設計を追加
- [x] IPC レベルで request_id を持てるようにし、複数同時実行時も応答を相関できるようにする
- [x] `src/cli.rs` に最小の `exec` サブコマンドを追加し、timeout / wait condition / exit code 伝搬の基本を実装する
- [x] 既存 `send-keys --wait-for-prompt` との責務分担を README に整理する
- [x] `exec` を protocol レベル request / result に引き上げ、CLI ローカル実装から卒業させる

### ~~C-31: ペイン状態メタデータ取得強化（cwd / busy / active / floating / last_update）~~ ✅ 対応済み 【優先度: 高】

現状の `list-panes --json` は `id / surface / title / cols / rows` のみで、
Agent が「どのペインに何を送るべきか」を安全に判断するには情報が足りない。

- **概要**: ペイン一覧や個別参照で、作業ディレクトリ、実行中コマンド、busy/idle、active、floating、
  最終更新時刻などのメタデータを取得できるようにする。
- **狙い**: 誤ったペインへの指示送信を減らし、Agent が現在の作業状況を自律判定できるようにする。

#### サブタスク

- [x] `PaneInfo` を後方互換な拡張として広げ、`list-panes --json` で `cwd / command / busy / last_output_unix_ms / active / floating` を返す
- [x] サーバー側で cwd / 実行中コマンド / busy / 最終出力時刻の保持方法を実装する
- [x] `list-panes --json` の出力は既存フィールドを保ったまま JSON 拡張で互換維持する方針に決める
- [x] README に「Agent が pane 選択前に確認すべき情報」を記載する
- [x] `active / floating` を GUI 状態と同期して返せるようにする

### ~~C-32: 出力購読 API（subscribe / diff stream）追加~~ ✅ 対応済み 【優先度: 高】

現状は `capture-pane` によるポーリングが前提で、長時間ジョブ監視や複数ペイン監視では効率が悪い。

- **概要**: 指定ペインの出力更新を IPC 経由で購読できる `subscribe-pane` / event stream を追加する。
  フルダンプではなく差分・新着行ベースで流せるようにする。
- **狙い**: Agent が `capture-pane` の連打なしで進捗監視・異常検知・完了判定を行えるようにする。
- **テスト計画**: `docs/test-plan-subscribe-pane.md`

#### サブタスク

- [x] `ServerMessage::Output` をベースにしつつ、CLI では JSON Lines の整形イベントへ射影する方針を決める
- [x] pane 単位の subscribe / unsubscribe を IPC で扱えるようにする
- [x] 遅延クライアント向けに backlog / drop policy を設計し、lagged メッセージを stream に流す
- [x] CLI で扱う場合のストリーム出力形式（JSON Lines / raw text）を決める

### ~~C-33: 明示的な割り込み・キャンセル API（Ctrl+C / terminate / close）~~ ✅ 対応済み 【優先度: 高】

現状でも `Ctrl+C` をキー送信すれば多くのケースは止められるが、
Agent 視点では「割り込み」「強制終了」「ペインを閉じる」が明示的な操作として分かれていた方が安全。

- **概要**: `interrupt-pane`、`terminate-pane`、`close-pane` などの制御 API を CLI / IPC に追加する。
- **狙い**: Agent が失敗したジョブやハングしたジョブを、キー入力に依存せず確実に停止できるようにする。

#### サブタスク

- [x] `ClientMessage` に `InterruptPane` を追加し、明示的な割り込み API を入れる
- [x] ConPTY / 子プロセス kill の扱いを整理し、graceful と force の差を README と CLI に反映する
- [x] CLI サブコマンドとして `interrupt-pane` / `terminate-pane` / `close-pane` の UX を実装する
- [x] 誤爆を減らすため、`list-panes --json` のメタデータ確認と併用できる形にする

### ~~C-34: ペイン別名・ロール付け（alias / role）~~ ✅ 対応済み 【優先度: 中】

Agent 運用では `pane 3` のような数値 ID よりも、`tests` `server` `agent-a` のような論理名で扱えた方が事故が少ない。

- **概要**: ペインに alias / role を付与し、CLI / IPC で ID の代わりに参照できるようにする。
- **狙い**: Agent のプロンプトやスクリプトが、動的に変わる pane ID に依存しないようにする。

#### サブタスク

- [x] `PaneInfo` に alias / role フィールドを追加する
- [x] `docs/test-plan-pane-alias-role.md` を作成する
- [x] `rename-pane` または `set-pane-meta` 相当の CLI を追加する
- [x] `send-keys` / `capture-pane` / `exec` などが alias 指定を受け付けるようにする
- [x] セッション保存・復元時に alias / role を永続化する

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

### ~~C-36: 待機条件 API の一般化（output regex / silence / exit）~~ ✅ 対応済み 【優先度: 中】

現状の待機は `send-keys --wait-for-prompt` に限定されており、
対話的ツールや独自プロンプトを使うプロセスでは Agent が完了判定しづらい。

- **概要**: `wait-for-output <regex>`、`wait-for-silence <duration>`、`wait-for-exit` など、
  汎用的な待機条件を CLI / IPC に追加する。
- **狙い**: Agent がシェル統合の有無に依存せず、ジョブ完了や安定状態を待てるようにする。

#### サブタスク

- [x] 待機条件ごとのイベントソース（Output / PaneClosed / CommandFinished）を CLI 実装として整理する
- [x] regex マッチは当面 `capture-pane --plain-text` の内容を対象にする方針を決める
- [x] タイムアウト・キャンセルとの組み合わせ仕様を CLI 引数として定義する
- [x] `exec` / `send-keys --wait-for-prompt` / `close-pane` / `terminate-pane` と共有できる内部待機基盤に寄せる

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

### ~~C-38: セルフアップデート機能（`yatamux update`）~~ ✅ 対応済み 【優先度: 中】

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
- [x] checksum 不一致 / quit timeout の guard（TC-12, TC-13）を helper 境界で自動テスト化する
- [x] app bridge の SaveAndQuit → `session.toml` 書き出し（TC-10）を自動化する
- [x] 起動時 restore path の session 復元（TC-11）を自動化する

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

### ~~C-40: Windows E2E テスト基盤と主要フロー自動化~~ ✅ 対応済み 【優先度: 高】

最近の CLI / IPC / session / update 系機能は unit / integration test が増えてきた一方で、
実際に `yatamux` を起動して Named Pipe IPC・ConPTY・保存復元までつないだ end-to-end coverage はまだ薄い。
個別テストは通るのに実プロセス運用で壊れる回帰を、PR 段階で拾えるようにしたい。

- **概要**: 実プロセスとして `yatamux` を起動し、`list-panes` / `split-pane` / `send-keys` / `wait-pane` / `exec` /
  `subscribe-pane` / `SaveAndQuit` / restore / update smoke までを確認できる Windows E2E harness を整備する。
- **狙い**: `B-1` や `C-38` で見えた「結合点でだけ壊れる」問題を自動化し、今後の CLI / Agent 向け機能追加の安全網にする。
- **テスト計画**: `docs/test-plan-e2e.md`

#### サブタスク

- [x] `docs/test-plan-e2e.md` を作成し、smoke / restore / update 系の対象シナリオを切り分ける
- [x] Windows 実プロセス E2E harness の方針を決める（temp APPDATA、専用 session 名、起動待機、後始末）
- [x] `yatamux` 起動 → `list-panes --json` / `capture-pane --json` の基本 smoke を `tests/e2e_smoke.rs` に追加する
- [x] `split-pane` → `send-keys` / `wait-pane` / `exec` の主要 CLI フローを自動化する
- [x] `subscribe-pane` / `interrupt-pane` / `close-pane` / `terminate-pane` の制御系 smoke を追加する
- [x] `SaveAndQuit` と次回起動時 restore の end-to-end を自動化する
- [x] `update` は mock release / staged binary を使う安全な smoke を追加し、`B-1` の再現採取に流用できる形へ寄せる
- [x] CI での実行方針（通常 test / `#[ignore]` / 専用 workflow）を決め、`e2e.yml` を full SHA pin + バージョンコメント方針で追加する

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
- [x] `SaveAndQuit` 送信失敗と「yatamux ペイン内なのに IPC 接続失敗」の分岐を明示エラー化し、誤った自己置換フォールバックを止める
- [ ] IPC 接続 / `SaveAndQuit` / `--apply-update` / ダウンロード検証のどこで落ちるかを切り分ける

### B-8: `WM_SIZE` 後の再描画要求が遅延し、別イベントまで見た目が更新されない 【修正済み】

ウィンドウサイズ変更直後に描画が更新されず、別の入力やフォーカス変化まで古いフレームが残ることがある。

- **修正済み（2026-04-13）**: PR #133
- **対応内容**:
  - `handle_wm_size` に `hwnd: HWND` パラメータを追加
  - `content_bb.set(None)` の直後に `InvalidateRect(Some(hwnd), None, false)` を追加
  - `WM_SIZE` から即時再描画要求が発行されるようになり、WM_TIMER 依存の遅延が解消

#### サブタスク

- [x] `WM_SIZE` 直後にコンテンツ領域を `InvalidateRect` するか、同等の即時再描画要求を入れる
- [x] `WM_SIZE` 直後にフォーカス変更なしでも画面更新されることを確認するテストケースを追加する（TC-B8 in `layout/store.rs`）

### B-9: ペイン幅が広がった領域が dirty 行なしで描き直されず、旧フレームが残る 【修正済み】

ペイン境界を動かして幅が広がったとき、新たに露出した領域に現在のアプリ描画が出ず、以前のバックバッファ内容が残ることがある。

- **修正済み（2026-04-13）**: PR #133
- **対応内容**:
  - `handle_pane_mode` の `<`/`>` および `+`/`-` 比率調整ブロックで `adjust_ratio_for_dir()` 後に `resize_all_panes(cr.w, cr.h)` と `content_bb.set(None)` を追加
  - `grid.resize()` が全行 dirty をセットし、次の `WM_PAINT` で全ペインが新しい矩形で再描画される

#### サブタスク

- [x] ペイン矩形が変わったフレームは当該ペインを全行 dirty にするか、ペイン矩形全体を背景込みで再描画する
- [x] 境界調整後に広がった領域へ現行フレームが描かれることを確認するテストケースを追加する（TC-B9 in `layout/store.rs`）

### C-41: 通知時のペインボーダーアクセントカラー + 点滅 【優先度: 中】

tmux / cmux のように、バックグラウンドペインで通知が発生したとき OS のトースト通知（Windows Action Center）を送りつつ、
該当ペインの枠線をアクセントカラーに切り替えて数回点滅させる視覚フィードバックを追加する。

- **概要**:
  1. **OS 通知**: 既存の `NativeToast` キュー（非フォーカス時）に加え、フォーカス中でも OS Action Center へ通知を送るオプションを追加する。発火トリガーは既存の BEL / OSC 9/99/777 / PTY 終了通知フローをそのまま流用する。
  2. **ペインボーダー点滅**: 通知を受けたペインの ID を `alerting_panes: HashMap<PaneId, u8>` で管理し、残り点滅回数をカウントダウンする。`WM_TIMER` 16ms ティックでカウントを進め、0 になったら通常色に戻す。点滅の ON/OFF 切り替えは 200〜300ms 間隔（約 4〜5 ティックに 1 回）とする。
  3. **アクセントカラー**: `AppearanceConfig` に `alert_border: Option<String>`（デフォルト `#FF6B6B` = 赤橙系）を追加し、`config.toml` の `[appearance]` セクションで上書き可能にする。
  4. **点滅回数**: デフォルト 5 回（ON/OFF の対で 10 フリップ）。将来的に設定化できるよう `alert_blink_count` をコード上定数で管理する。

- **変更クレート**: `yatamux-client`（`layout.rs`, `window.rs`, `config.rs`, `render/` 周辺）、`yatamux-protocol`（必要に応じて）
- **テスト計画**: `docs/test-plan-pane-alert.md`

#### サブタスク

- [x] `docs/test-plan-pane-alert.md` を作成してテストケースを列挙する
- [x] `AppearanceConfig` に `alert_border: Option<String>` を追加し、`parse_hex_color` でデフォルト `#FF6B6B` にフォールバックする
- [x] `PaneStore` に `alerting_panes: HashMap<PaneId, u8>`（残りフリップ数）と `alert_tick: u8`（点滅タイマー用サブカウンタ）を追加する
- [x] `AlertingBackend<I>` を `notification.rs` に追加し、`notify()` で `trigger_alert` を呼んだ後に内部バックエンドへ委譲する。`app.rs` で `FocusAwareBackend` をラップして接続する
- [x] `WM_TIMER` ハンドラで `clear_alert(active)` を呼び、`tick_alert()` でフリップカウントをデクリメント → 0 になったエントリを削除して `InvalidateRect` を要求する
- [x] `paint()` のセパレーター描画後に `alerting_panes` を参照し、点滅 ON フェーズなら `alert_border` 色で 2px ペインボーダーを描画する
- [ ] フォーカス中でも OS Action Center へ `ShellExecute` / `Windows.UI.Notifications` 相当の API で送信する経路を追加する（既存 `NativeToast` のフォーカス条件を外すか別送信パスを設ける）
- [x] `tests/e2e_smoke.rs` に BEL / ProcessExit / OSC 9 の通知 E2E テストを 3 本追加する（CI `windows-latest` で実機実行）
- [x] 修正後に Clippy・全テスト・rustfmt を通す

## ドキュメント

### D-2: IPC / Agent 運用ドキュメント再整理 【優先度: 中】

README は機能一覧と個別 CLI 例が充実してきた一方で、
運用者やエージェント実装者が知りたい「安全にどう使うか」「lagged 後にどう戻るか」「CJK / IME / 絵文字はどこまで保証されるか」が分散している。
IPC ハードニングと protocol 固定化に合わせて、使い方の一枚岩の説明を追加したい。

#### サブタスク

- [x] README か `docs/agent-operations.md` に、`list-panes --json` → `exec` / `send-keys` → `subscribe-pane --json` → `lagged` → `capture-pane --json` の標準監視フローをまとめる
- [x] Named Pipe IPC の信頼境界、想定するローカル脅威モデル、`C-41` 後の推奨設定を 1 か所に整理する（`docs/agent-operations.md`）
- [x] CJK 幅計算、IME、ZWJ 絵文字など「意図的な制限がある表示領域」を README / docs に明文化する（`docs/agent-operations.md`）
- [x] `config.toml` / `session.toml` / layouts / themes の保存場所と優先順位を、運用・トラブルシュート観点で整理する（`docs/agent-operations.md`）

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

---
## 自動調査による改善候補 (2026-04-16)

## 改善候補

### IMP-01: `list-panes` ごとのプロセスメタデータ再走査が重い
- **ファイル**: `crates/server/src/session/handlers/query.rs`, `crates/server/src/session/handlers/support.rs`, `crates/terminal/src/process.rs`
- **内容**: `handle_list_panes()` が各ペインごとに `find_active_command()` と `find_process_cwd()` を呼んでおり、Toolhelp スナップショット、`OpenProcess`、`ReadProcessMemory` を毎回実行している。`list-panes --json` をエージェントが頻繁に叩く運用では、ペイン数に比例して CPU コストと待ち時間が増える。
- **改善案**: `Pane` 側に `command` / `cwd` のキャッシュを持たせ、出力受信時・一定間隔・SaveAndQuit 前などで更新する。少なくとも `list-panes` 応答はキャッシュ読み出しを基本にして OS 走査をホットパスから外す。
- **優先度**: High

### IMP-02: 描画ホットパスでロック取得と一時アロケーションが多い
- **ファイル**: `crates/client/src/window/mod.rs`, `crates/client/src/window/win32/wndproc.rs`, `crates/client/src/window/win32/modes.rs`
- **内容**: `WM_TIMER` と `paint()` が 16ms 周期で `state.panes.lock()` を何度も取り直し、各フレームで `HashMap<PaneId, Arc<Mutex<Grid>>>` の複製や dirty 行 `Vec` → `HashSet` 変換まで行っている。出力が多いセッションではロック競合とヒープ確保が積み上がりやすい。
- **改善案**: フレーム単位のスナップショット取得を 1 回に寄せ、dirty 行は `HashSet` 化せずソート済み `Vec` やビットセットで扱う。`WM_TIMER` 側も「dirty フラグあり」を軽量に判定できる状態を `ClientState` に持たせる。
- **優先度**: Medium

### IMP-03: フローティングペインだけ毎回フルリペイントになっている
- **ファイル**: `crates/client/src/window/mod.rs`
- **内容**: 通常ペイン描画は dirty 行ベースだが、フローティングペイン描画は可視時に全行・全セルを毎回描き直している。しかも通常ペイン描画とかなり似たロジックが別実装になっており、最適化や不具合修正が二重管理になる。
- **改善案**: フローティングペインも通常ペインと同じ描画ヘルパーに寄せ、dirty 行と共通の run batching を使う。これで描画負荷と重複コードを同時に減らせる。
- **優先度**: Medium

### IMP-04: IPC クライアント接続時の handshake エラーが握りつぶされる
- **ファイル**: `crates/client/src/connection.rs`
- **内容**: 接続直後の handshake 送信失敗を無視しており、`HandshakeAccepted` の内容も検証せずに `Ok(Self)` を返している。認証必須サーバーや将来の version mismatch では、接続成功に見えたあとで遅れて失敗し、原因が利用者に見えにくい。
- **改善案**: `connect()` 内で handshake の往復を完了させ、`ServerMessage::HandshakeAccepted` / `ServerMessage::Error` を明示的に処理してから接続成功にする。認証あり・旧サーバー・version mismatch の自動テストも追加する。
- **優先度**: High

### IMP-05: 不正な JSON リクエストに対してクライアントへエラーを返していない
- **ファイル**: `crates/server/src/ipc.rs`
- **内容**: `serde_json::from_str::<ClientMessage>` が失敗したとき、サーバー側で warn ログを出すだけでクライアントには何も返していない。CLI や外部ツールは「待ち続けるだけ」になり、失敗理由を機械判定できない。
- **改善案**: パース失敗時に `ServerMessage::Error` を返して切断するか、少なくとも明示的なエラー応答を返す。oversize message と同じくプロトコル上の失敗として扱い、異常系テストを追加する。
- **優先度**: Medium

### IMP-06: レイアウト切り替えステートマシンに本番 `expect()` が残っている
- **ファイル**: `src/app/bridge.rs`
- **内容**: `LayoutPhase::Applying` の処理で `queue.pop_front().expect("queue should be non-empty")` と `queue.front().expect(...)` が使われている。キュー管理にズレが出ると GUI 全体が panic で落ちるため、状態遷移バグがそのまま致命傷になる。
- **改善案**: キュー枯渇を recoverable な異常系として扱い、レイアウト切り替えを中断してログ・通知を返す。`LayoutPhase` 遷移の単体テストを追加して invariant を固定する。
- **優先度**: Medium

### IMP-07: テーマローダーが `alert_border` を読み込まず、色パーサも重複している
- **ファイル**: `crates/client/src/layout/catalog.rs`
- **内容**: `Theme` には `alert_border` があるのに、`load_theme_from_file()` の TOML デシリアライズ対象に含まれておらず常に `None` になる。さらに `parse_hex_u32()` が `src/config.rs` の色パースと重複していて、テーマ/設定で挙動が分岐しやすい。モジュール自体に専用テストもない。
- **改善案**: `alert_border` をテーマ読み込みに追加し、色変換と `%APPDATA%` パス構築を共通ヘルパーへ寄せる。`temp APPDATA` を使うユニットテストでテーマ・レイアウト探索を固定する。
- **優先度**: High

### IMP-08: CI ポーラーの自動テストがなく、API 変化に弱い
- **ファイル**: `src/ci.rs`
- **内容**: GitHub API レスポンスの変換、ブランチ付き URL 生成、未知ステータスの扱い、エラー時の挙動が未テストで、ネットワーク境界のロジックがコード読解頼みになっている。ステータスバー表示や `ServerMessage::CiStatus` に直結するわりに安全網が薄い。
- **改善案**: URL 生成とレスポンス変換を純粋関数に切り出し、`src/update.rs` と同様に mock HTTP を使ったテストを追加する。少なくとも success/failure/in_progress/unknown の射影は固定したい。
- **優先度**: Medium

### IMP-09: `yatamux-protocol` の公開 API にフィールド単位の説明が足りない
- **ファイル**: `crates/protocol/src/types.rs`, `crates/protocol/src/message.rs`
- **内容**: このクレートは CLI・エージェント・将来の MCP 実装が直接触る wire contract だが、`PaneInfo`、`PaneCapture`、`ExecWaitCondition`、各 message variant の公開フィールドに「単位」「省略時の意味」「後方互換上の扱い」の説明が十分に付いていない。実装や golden test を読まないと意味が確定しない箇所が残る。
- **改善案**: 公開型・公開フィールドへ rustdoc を追加し、必要なら `#![deny(missing_docs)]` を導入する。`docs/protocol-ipc.md` と rustdoc を同じ契約の一次情報として保つ。
- **優先度**: Medium

---
## 自動調査による改善候補 第2ラウンド (2026-04-16)

### IMP-10: 認証前 IPC クライアントへサーバー出力が配信される
- **ファイル**: `crates/server/src/ipc.rs`
- **内容**: `handle_client()` は `authenticated=false` の間も `broadcast` 側を通常どおり処理しており、`subscriptions.is_empty()` の既定動作で `Output` / `Notification` / `PaneClosed` などを未認証クライアントへ流してしまう。`require_auth=true` でも「送信は拒否するが盗聴はできる」状態が残っている。
- **改善案**: 認証完了までは `HandshakeAccepted` / `Error` 以外を一切書き出さないようにし、未認証接続でペイン出力を観測できないことを integration test で固定する。
- **優先度**: High

### IMP-11: oversized JSON Lines 制限が `BufReader::lines()` の後段チェックになっており DoS 耐性が不十分
- **ファイル**: `crates/server/src/ipc.rs`
- **内容**: `MAX_MESSAGE_BYTES` 超過判定は `lines.next_line()` のあとに実施されているため、改行なし巨大入力や極端に長い 1 行を受けると、その時点で巨大 `String` を確保してしまう。現在の上限チェックではメモリ消費そのものを抑えられていない。
- **改善案**: `tokio_util::codec::LinesCodec::new_with_max_length` か `read_until(b'\n', ...)` の手動上限チェックへ置き換え、改行なし巨大入力と境界値サイズのテストを追加する。
- **優先度**: High

### IMP-12: IPC 認証トークンが予測可能かつ平文永続化で、防御層として弱い
- **ファイル**: `crates/server/src/ipc.rs`, `src/app/bootstrap.rs`, `crates/client/src/connection.rs`
- **内容**: トークンは PID と timestamp を混ぜた独自生成で、`%APPDATA%\\yatamux\\<session>.token` に `File::create` で平文保存され、終了時にも削除されない。`require_auth` を「同一ユーザー内の追加防御」として使うには、エントロピー・寿命・ファイル ACL の面が弱い。
- **改善案**: OS CSPRNG でトークンを生成し、トークンファイルは明示 ACL で絞るか、可能なら reusable file token 自体をやめてログオンセッション由来の認証へ寄せる。少なくとも終了時削除と権限制御のテストを追加する。
- **優先度**: High

### IMP-13: Win32 側の `try_send` 失敗で入力や `Resize` が黙って消える
- **ファイル**: `crates/client/src/window/mod.rs`, `crates/client/src/window/win32/modes.rs`
- **内容**: `send_input()`、`resize_all_panes()`、`sync_pane_state()`、split/float/layout/close などが `try_send()` の失敗を無視している。チャネル飽和や閉塞時にキー入力・リサイズ同期・UI 操作が無言で脱落し、再現時もログに痕跡が残らない。
- **改善案**: 入力と `Resize` は lossy にしない送信経路へ切り替え、coalesce 可能なメッセージだけを別キューで間引く。少なくとも channel full 時のログと saturation テストを追加する。
- **優先度**: High

### IMP-14: `ExecWaitCondition::OutputRegex` がサーバー本体ループで重い capture 再構築を回している
- **ファイル**: `crates/server/src/session/mod.rs`, `crates/server/src/session/handlers/support.rs`
- **内容**: `poll_pending_execs()` は regex 待機中の各 exec について 200ms ごとに `build_capture_response()` を呼び、グリッドをロックしてスクロールバック込みの文字列を都度組み立てている。長い scrollback や同時待機が増えると、通常の `ClientMessage` / `Output` 処理と同じサーバーループを圧迫する。
- **改善案**: pane ごとに直近出力のリングバッファを持ち、増分出力へ regex を当てる方式へ寄せる。重い capture 構築は別タスク化または予算制御し、複数待機時の負荷テストを追加する。
- **優先度**: Medium

### IMP-15: `find_process_cwd()` の `LoadLibraryA("ntdll.dll")` が都度ロードのままで解放もキャッシュもない
- **ファイル**: `crates/terminal/src/process.rs`
- **内容**: `read_process_cwd_inner()` は呼ぶたびに `LoadLibraryA` → `GetProcAddress` を実行し、`FreeLibrary` を呼ばない。`list-panes` / `SaveAndQuit` のようなホットパスで繰り返すと、余計なモジュール参照カウント増加とオーバーヘッドが積み上がる。
- **改善案**: `OnceLock` で `NtQueryInformationProcess` の関数ポインタをキャッシュし、`GetModuleHandleA` か静的 import に寄せる。反復呼び出し時のベンチやリーク検知テストも欲しい。
- **優先度**: Medium

### IMP-16: クリップボード書き込み失敗時に `HGLOBAL` が解放されていない
- **ファイル**: `crates/client/src/window/mod.rs`
- **内容**: `write_clipboard_text()` は `GlobalAlloc` したメモリを `SetClipboardData()` 成功時に OS へ移譲する前提だが、失敗時の `GlobalFree` を省略している。OSC 52 が繰り返し失敗する環境ではヒープリークの原因になる。
- **改善案**: `SetClipboardData` 失敗時に `GlobalFree` で後始末し、Win32 API 失敗を注入できる薄い抽象を挟んで error-path テストを書けるようにする。
- **優先度**: Medium

### IMP-17: IME の自動テストが状態ヘルパー中心で、`WndProc` 経由の本流を固定できていない
- **ファイル**: `crates/client/src/ime.rs`, `crates/client/src/window/win32/wndproc.rs`
- **内容**: 既存テストは `ImeState` や `build_preedit_segments()` の単体確認が中心で、`WM_IME_COMPOSITION` → committed text 反映、保存プロンプトへの分岐、`WM_CHAR` 抑止、候補ウィンドウ更新の成否といった本流を直接カバーしていない。IME は Windows 固有回帰が出やすいわりに安全網が薄い。
- **改善案**: headless `ClientState` を組んで IME message handler を直接叩くテストを追加し、save prompt 中にペインへ誤送信しないことまで固定する。必要なら最小 Windows integration smoke も追加する。
- **優先度**: Medium

### IMP-18: CJK 幅計算は `width.rs` 単体に寄っており、wide cell の `Grid`/VT 統合保証が薄い
- **ファイル**: `crates/terminal/src/width.rs`, `crates/terminal/src/grid/mod.rs`, `crates/terminal/src/vt/*.rs`
- **内容**: `char_width()` / `str_width()` のテストはあるが、曖昧幅設定切り替え、行末折り返し、`Continuation` セルの上書き・リサイズ後整合、ZWJ 絵文字とカーソル移動の組み合わせなど、実際に壊れやすいのは `Grid` と VT 統合側の挙動である。
- **改善案**: VT バイト列を `Grid` へ流す統合テストを増やし、カーソル位置・折り返し・continuation cleanup・resize 後の見た目をケース化する。
- **優先度**: Medium

### IMP-19: スクロールバックの capture/view 境界ロジックに専用テストがなく、回帰を拾いにくい
- **ファイル**: `crates/server/src/session/handlers/support.rs`, `crates/terminal/src/grid/scrollback.rs`, `crates/client/src/window/win32/wndproc.rs`
- **内容**: `visible_text()` / `scrollback_tail()` / `captured_content()` と、scrollback+screen をまたぐクライアント側 row マッピングは helper 化されている一方、境界値テストがない。`lines=1/rows/rows+1`、alternate screen、長い scrollback、`scroll_offset` 付きの URL hover/選択で食い違いが起きても検知しづらい。
- **改善案**: helper 単位の境界値テストと `capture-pane --json` の厳密一致テストを追加し、client 側も scrollback 混在ビューの行解決を固定する。
- **優先度**: Medium
