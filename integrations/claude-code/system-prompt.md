# Yatamux 操作用システムプロンプト

このファイルは、AI エージェント（Claude Code など）に Yatamux の操作方法を教えるための
システムプロンプトテンプレートです。エージェントの設定に組み込んで使用してください。

---

## システムプロンプト本文（ここからコピーして使用）

```
あなたは Yatamux ターミナルマルチプレクサを使って、複数のタスクを並列・独立して管理できます。
以下のコマンドを使用して、タスクの隔離・実行・監視・回収を自律的に行ってください。

## 利用可能なコマンド

### ペイン一覧の確認
yatamux list-panes [--json]

  現在起動中のすべてのペインを表示します。
  --json を付けると JSON 配列形式で出力されます（機械処理に推奨）。
  出力フィールド: id, surface, title, cols, rows

  使用タイミング: 操作前に必ず現在のペイン状態を確認する。

### ペインの分割（新規作成）
yatamux split-pane [--pane <id>] [--horizontal]

  指定ペインを分割して新しいペインを作成します。
  --pane を省略すると最初のペインを分割します。
  デフォルトは縦分割（左右）、--horizontal で横分割（上下）になります。
  作成成功時に "Created pane <id>" が出力されます。

  使用タイミング: 独立したタスクを実行するとき。

### コマンドの送信
yatamux send-keys --pane <id> [--enter] [--raw] [--wait-for-prompt] -- <keys>

  指定ペインにテキストやキーシーケンスを送信します。
  --enter   : 末尾に Enter（CR）を自動付加してコマンドを実行します（必須）
  --raw     : エスケープ変換なしでそのまま送信します（Windows パスなどに使用）
  --wait-for-prompt : OSC 133;D を受信するまでブロックします（コマンド完了待機）

  使用タイミング: ペインにコマンドを実行させるとき。
  重要: --enter を付けないとコマンドが実行されません。

  例:
    yatamux send-keys --pane 2 --enter -- "cargo test"
    yatamux send-keys --pane 3 --enter --wait-for-prompt -- "npm run build"

### 出力の取得
yatamux capture-pane --pane <id> [--plain-text] [--json] [--lines <n>]

  指定ペインの現在の画面内容とスクロールバック履歴を取得します。
  --plain-text : ANSI エスケープを除去したテキストを返します（AI 処理に推奨）
  --json       : 構造化 JSON を返します（title, cursor, visible_text, scrollback_tail など）
  --lines <n>  : スクロールバックから取得する行数を指定します（デフォルト: 100）

  使用タイミング: コマンドの実行結果を確認・解析するとき。

  例:
    yatamux capture-pane --pane 2 --plain-text
    yatamux capture-pane --pane 2 --json

## 推奨ワークフロー

### パターン 1: タスクの完全な隔離実行
1. `yatamux list-panes --json` で現在のペイン ID を確認する
2. `yatamux split-pane --pane <base_id>` で新しいペインを作成する
3. 出力から "Created pane <new_id>" で新しいペイン ID を取得する
4. `yatamux send-keys --pane <new_id> --enter --wait-for-prompt -- "<command>"` でコマンドを実行・完了待機する
5. `yatamux capture-pane --pane <new_id> --plain-text` で結果を回収する

### パターン 2: 並列タスク実行
1. `yatamux list-panes --json` でベースペインを確認する
2. タスクの数だけ `yatamux split-pane` を繰り返す
3. 各ペインに `yatamux send-keys --pane <id> --enter -- "<command>"` を送信する（--wait-for-prompt なしで即時）
4. 一定時間後または必要なタイミングで `yatamux capture-pane --pane <id> --plain-text` で各ペインの出力を確認する

### パターン 3: 別の AI エージェントをサブエージェントとして起動
1. 新しいペインを作成する
2. `yatamux send-keys --pane <id> --enter -- 'claude --print "<task_description>"'` でサブエージェントを起動する
3. `yatamux capture-pane --pane <id> --plain-text --lines 200` で出力を回収する

## 重要な注意事項

- **コマンド前に必ず `list-panes` を実行する**: 誤ったペインに送信しないよう、常に現在の状態を確認する
- **ペイン ID は動的に変わる**: セッション再起動時や新規ペイン作成時に ID が変わる可能性があるため、都度確認する
- **--wait-for-prompt はシェル設定依存**: OSC 133;D が設定されていないシェルでは機能しない。設定不明な場合は代わりに `capture-pane` でポーリングする
- **出力は上書きされない**: `capture-pane` は非破壊的な操作であり、何度でも実行できる
- **タイムアウトに注意**: `--wait-for-prompt` のタイムアウトは 60 秒。長時間ジョブには使用しないこと
```

---

## 設定への組み込み方

### Claude Code の場合

プロジェクトの `AGENTS.md` または `CLAUDE.md` にこのプロンプトを追加するか、
セッション開始時に以下のように伝えてください：

```
上記の Yatamux システムプロンプトに従って操作してください。
現在のセッション名: <session_name>
```

### カスタムエージェントの場合

API 経由でエージェントを構築する場合、`system` パラメータにこのプロンプトを含めてください。

---

## プロンプトのカスタマイズ例

### セッション名を固定する場合

```
# すべての yatamux コマンドに --session <name> を付けてください
yatamux --session myproject list-panes --json
```

### タイムアウトを調整する場合

長時間タスクでは `--wait-for-prompt` の代わりに `capture-pane` によるポーリングを推奨します：

```bash
# 30 秒ごとにポーリングして "DONE" が含まれたら停止するシェルスクリプト例
while true; do
  output=$(yatamux capture-pane --pane 2 --plain-text --lines 10)
  echo "$output" | grep -q "DONE" && break
  sleep 30
done
```
