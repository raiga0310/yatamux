# Yatamux エージェント操作ガイド

このファイルは Claude Code が Yatamux を操作するためのクイックリファレンスです。
詳細なチュートリアルは [`README.md`](./README.md) を参照してください。

## 利用可能なコマンド

### ペイン一覧の確認

```bash
yatamux list-panes --json
```

- 現在のすべてのペインを JSON 配列で返す
- フィールド: `id`（数値）、`surface`、`title`、`cols`、`rows`
- **操作前に必ずこれを実行して現在の状態を確認すること**

### ペインの作成

```bash
yatamux split-pane --pane <id>            # 縦分割（左右）
yatamux split-pane --pane <id> --horizontal  # 横分割（上下）
```

- 成功時の出力: `Created pane <new_id>`
- `<new_id>` を取得して以降の操作に使用する

### コマンドの送信

```bash
# コマンドを実行する（--enter は必須）
yatamux send-keys --pane <id> --enter -- "<command>"

# コマンド完了まで待機する（OSC 133;D 対応シェルが必要）
yatamux send-keys --pane <id> --enter --wait-for-prompt -- "<command>"

# Ctrl+C を送信する（実行中のコマンドを中断する）
yatamux send-keys --pane <id> --raw -- $'\x03'
```

### 出力の取得

```bash
# AI が読みやすいプレーンテキスト形式
yatamux capture-pane --pane <id> --plain-text

# 構造化 JSON 形式（タイトル・カーソル位置・表示テキスト・スクロールバック）
yatamux capture-pane --pane <id> --json

# 行数を指定する（デフォルト 100 行）
yatamux capture-pane --pane <id> --plain-text --lines 50
```

### レイアウト管理

```bash
yatamux layout list              # 保存済みレイアウトの一覧
yatamux layout export <name>     # レイアウト設定を TOML で出力
yatamux layout delete <name>     # レイアウトを削除
```

---

## マルチエージェントのベストプラクティス

### 1. 操作前に必ず `list-panes` を実行する

ペイン ID はセッション再起動時や新規作成時に変わる可能性があります。
コマンド送信前に毎回確認してください。

```bash
PANES=$(yatamux list-panes --json)
# $PANES を解析して目的のペインの ID を特定する
```

### 2. タスクごとにペインを分ける

複数のタスクを同じペインで実行しないでください。
独立したペインを使うことで、出力の混在を防ぎ、結果の回収が確実になります。

```
ペイン 1: メインエージェント（あなた）
ペイン 2: フロントエンドのビルド・テスト
ペイン 3: バックエンドのビルド・テスト
ペイン 4: データベースのマイグレーション
```

### 3. `--wait-for-prompt` の限界を理解する

`--wait-for-prompt` はシェルが OSC 133;D を出力する設定になっている場合のみ機能します。
不明な場合は `capture-pane` によるポーリングで完了を確認してください。

```bash
# ポーリングの例（完了キーワードが含まれるまで繰り返す）
for i in $(seq 1 10); do
  output=$(yatamux capture-pane --pane 2 --plain-text --lines 5)
  echo "$output" | grep -qE "^\$|error:|warning:" && break
  sleep 10
done
```

### 4. 誤ったペインへの送信を防ぐ

コマンドを送信する前に、対象ペインの `title` や `capture-pane` の出力内容を確認して
意図したペインであることを検証してください。

### 5. ラッパースクリプトを活用する

`integrations/claude-code/scripts/run-in-pane.sh` を使うと
ペイン作成・コマンド送信・出力回収を一度の呼び出しで行えます。

```bash
./integrations/claude-code/scripts/run-in-pane.sh "my-task" "cargo test --all"
```

---

## よくあるパターン

### サブエージェントを起動して結果を回収する

```bash
# 1. ペインを作成する
yatamux split-pane --pane 1
# 出力: Created pane 2

# 2. サブエージェントを起動する
yatamux send-keys --pane 2 --enter -- 'claude --print "このコードをレビューしてください: $(cat src/main.rs)"'

# 3. 完了を待つ（OSC 133;D 設定済みの場合）
yatamux send-keys --pane 2 --wait-for-prompt -- ""

# 4. 結果を回収する
yatamux capture-pane --pane 2 --plain-text --lines 500
```

### ビルドエラーの自動検出

```bash
# ビルドを実行して完了を待つ
yatamux send-keys --pane 2 --enter --wait-for-prompt -- "cargo build 2>&1"

# 出力を取得してエラーチェック
OUTPUT=$(yatamux capture-pane --pane 2 --plain-text)
if echo "$OUTPUT" | grep -q "^error"; then
    echo "ビルドエラーが検出されました"
    echo "$OUTPUT"
fi
```

---

## トラブルシューティング

| 症状 | 原因 | 対処法 |
|------|------|--------|
| `yatamux is not running` | Yatamux が起動していない | Yatamux を起動してから再実行する |
| `pane N not found` | 指定 ID のペインが存在しない | `list-panes` で現在の ID を確認する |
| `--wait-for-prompt` がタイムアウト | シェルの OSC 133;D が未設定 | シェルインテグレーションを設定するか `capture-pane` ポーリングに切り替える |
| 出力が空 | コマンドがまだ実行中 | 少し待ってから `capture-pane` を再実行する |
