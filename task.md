# yatamux タスク一覧

未実装・未解決の問題をここに積む。

---

## バグ

### B-1: 長い入力の折り返し描画が機能しない (issue #3)
- 入力文字列がペイン幅を超えても次の行に折り返して表示されない
- PR #5 で ConPTY サイズ整合・分割時リサイズ・描画クリップを修正済みだが、
  描画レイヤーの折り返し（readline/ConPTY が出力する VT シーケンスの処理）が
  まだ不完全な可能性あり
- **要調査**: tmux / zellij / wezterm など既存実装が折り返しをどう実現しているか
  （ConPTY 側に任せるのか、クライアント側グリッドで再計算するのか）

### B-2: Ctrl+C がエージェント（Claude 等）に届かない
- ペイン内で Claude などの AI エージェントを起動した状態で Ctrl+C を押しても
  プロセスが終了しない
- WM_KEYDOWN の Ctrl+C → `\x03` (ETX) は実装済みだが、ConPTY 経由で
  SIGINT 相当が子プロセスに伝わっていない可能性
- **要調査**: ConPTY における Ctrl+C の扱い、GenerateConsoleCtrlEvent の要否

### B-3: Ctrl+Shift+E/O でペイン分割すると ^E / ^O が入力欄に残る
- `WM_KEYDOWN` で Ctrl+Shift+E/O を捕捉して `split_tx` に送った後、
  `return LRESULT(0)` しているが、`TranslateMessage` / `WM_CHAR` 経由で
  `\x05`(^E) や `\x0f`(^O) が PTY に送られてしまっている可能性
- **対応方針**: `WM_KEYDOWN` でショートカットを消費した場合は `WM_CHAR` を
  スキップするフラグを立てるか、Ctrl+Shift の組み合わせは `WM_CHAR` では
  送信しないよう `WM_CHAR` ハンドラ側でガードする

---

## 機能改善

### F-1: 起動時のウィンドウ位置・サイズが不適切
- 起動直後にウィンドウが最大化されていないか、画面内の適切な位置に表示されない
- `CreateWindowExW` の `CW_USEDEFAULT` を見直すか、起動時に最大化する処理を追加

### F-3: ペイン分割・フォーカス移動のキーバインドを改善したい
- 現状: 分割 `Ctrl+Shift+E`(縦) / `Ctrl+Shift+O`(横)、移動 `Ctrl+Tab` / `Ctrl+Shift+Tab`
- 希望:
  - フォーカス移動を `Ctrl+←↑↓→` および `Ctrl+H/J/K/L`（vim 風）に対応
  - 分割ショートカットも見直し（B-3 の ^E/^O 混入問題とあわせて再設計）
- **対応方針**: `keydown_to_vt` の前段で方向キー＋修飾キーの組み合わせを判定し
  `cycle_pane` または新設の `focus_pane(direction)` を呼ぶ

### F-2: バイナリの実行が不便
- `.\target\release\cmux-win.exe` をフルパスで指定しないといけない
- 実行ディレクトリも移動させる必要がある
- 対応案:
  - `cargo install` でインストール可能にする
  - シェルスクリプト / PowerShell スクリプトのラッパーを用意
  - PATH に通せるよう `just install` などのタスクを追加
  - ポータブル実行のためにリソースファイルを exe と同梱する仕組みを整備
