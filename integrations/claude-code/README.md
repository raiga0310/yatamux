# Yatamux × Claude Code 統合ガイド

このディレクトリは、Claude Code（およびその他の AI エージェント）が Yatamux を使って
**サブエージェントの起動・監視・結果回収**を自律的に行えるようにするための統合スキルを提供します。

---

## この統合が解決すること

Claude Code 単体では、長時間かかるタスク（ビルド、テスト、別エージェントの実行など）を
**バックグラウンドで並列実行しながら監視する**手段が限られています。

Yatamux の IPC CLI を活用することで、以下が可能になります：

- ペインを分割して**タスクごとに独立した端末環境**を用意する
- `send-keys` で各ペインにコマンドを送信して**エージェントや長時間ジョブを起動**する
- `capture-pane` で**出力を非破壊的に回収**する（ペインを閉じる必要なし）
- `list-panes` で**全ペインの状態を確認**する

---

## 基本的なワークフロー

### ステップ 1: ペインを分割してサブタスクを隔離する

```bash
# 現在のペイン ID を確認する
yatamux list-panes --json

# 新しいペインを作成する（縦分割）
yatamux split-pane --pane <id>

# 横分割の場合
yatamux split-pane --pane <id> --horizontal
```

### ステップ 2: サブエージェントにコマンドを送信する

```bash
# コマンドを送信して Enter キーを押す
yatamux send-keys --pane <id> --enter -- "cargo test --all"

# OSC 133;D 対応シェルではコマンド完了まで待機できる
yatamux send-keys --pane <id> --enter --wait-for-prompt -- "cargo build --release"
```

### ステップ 3: 出力を回収する

```bash
# プレーンテキストで出力を取得する（AI が読みやすい形式）
yatamux capture-pane --pane <id> --plain-text

# JSON 形式で構造化データを取得する
yatamux capture-pane --pane <id> --json

# 直近 50 行のみ取得する
yatamux capture-pane --pane <id> --plain-text --lines 50
```

---

## 実践的なユースケース

### ユースケース 1: 並列テスト実行

メインのエージェントが「フロントエンドのテスト」と「バックエンドのテスト」を
2 つのペインで並列実行し、両方が終わったら結果をまとめる例です。

```bash
# ペイン一覧を確認
PANES=$(yatamux list-panes --json)

# フロントエンドテスト用ペインを作成
FRONT_PANE=$(yatamux split-pane --pane 1 | grep -oP '\d+')

# バックエンドテスト用ペインを作成
BACK_PANE=$(yatamux split-pane --pane 1 --horizontal | grep -oP '\d+')

# 両ペインに並列でコマンドを送信
yatamux send-keys --pane "$FRONT_PANE" --enter -- "npm test"
yatamux send-keys --pane "$BACK_PANE" --enter -- "cargo test"

# （しばらく待ってから）それぞれの出力を回収
yatamux capture-pane --pane "$FRONT_PANE" --plain-text
yatamux capture-pane --pane "$BACK_PANE" --plain-text
```

### ユースケース 2: 別の Claude Code インスタンスをサブエージェントとして起動

```bash
# サブエージェント用ペインを作成
yatamux split-pane --pane 1

# 新しいペインで Claude Code を起動して特定タスクを依頼
yatamux send-keys --pane 2 --enter -- 'claude --print "src/lib.rs のユニットテストを書いてください"'

# 完了後に結果を確認
yatamux capture-pane --pane 2 --plain-text --lines 100
```

### ユースケース 3: ラッパースクリプトを使った簡易実行

同梱の `scripts/run-in-pane.sh` を使うと、ペイン作成・コマンド送信・出力回収を一括で行えます。

```bash
# ペインでコマンドを実行して出力を返す
./integrations/claude-code/scripts/run-in-pane.sh "build-task" "cargo build --release"
```

---

## AI エージェント向けのベストプラクティス

1. **コマンド前に `list-panes` で状態を確認する**
   誤ったペインにコマンドを送らないよう、常に現在のペイン一覧を確認してから操作する。

2. **`--wait-for-prompt` でコマンド完了を確実に待つ**
   OSC 133;D 対応シェル（bash/PowerShell のシェルインテグレーション設定済み）であれば、
   コマンド完了まで確実に待機できる。

3. **`--plain-text` を使って AI が読みやすい形式で出力を取得する**
   ANSI エスケープシーケンスを除去したプレーンテキストは、AI による解析に適している。

4. **ペインの用途をタイトルで管理する**
   各ペインに `echo -ne "\033]0;task-name\007"` でタイトルを付けると、
   `list-panes` の出力から目的のペインを特定しやすくなる。

---

## シェルインテグレーション設定（推奨）

`--wait-for-prompt` を使うには、シェルに OSC 133;D を出力させる設定が必要です。

### bash の場合

```bash
# ~/.bashrc に追記
PS1='\[\e]133;A\e\\\]'$PS1
trap 'printf "\e]133;C\e\\"' DEBUG
PROMPT_COMMAND='printf "\e]133;D;$?\e\\"'$'\n'"${PROMPT_COMMAND}"
```

### PowerShell の場合

```powershell
# $PROFILE に追記
function prompt {
    $code = $LASTEXITCODE
    [Console]::Write("`e]133;D;$code`e\")
    [Console]::Write("`e]133;A`e\")
    "PS $($executionContext.SessionState.Path.CurrentLocation)> "
}
```

---

## 関連ファイル

- [`system-prompt.md`](./system-prompt.md) — AI に Yatamux 操作を教えるシステムプロンプトテンプレート
- [`scripts/run-in-pane.sh`](./scripts/run-in-pane.sh) — ペイン実行ラッパースクリプト
- [`AGENTS.md`](./AGENTS.md) — エージェント向け操作ガイド（簡易版）
