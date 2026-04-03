#!/bin/bash
# run-in-pane.sh — Yatamux ペインでコマンドを実行して出力を返すラッパースクリプト
#
# 使用方法:
#   run-in-pane.sh <pane-title> <command> [options]
#
# 引数:
#   pane-title  : ペインを識別するための表示タイトル（例: "build-task"）
#   command     : 実行するコマンド（例: "cargo build --release"）
#
# オプション:
#   --session <name>  : 接続する Yatamux セッション名（デフォルト: "default"）
#   --base-pane <id>  : 分割元のペイン ID（デフォルト: 最初のペイン）
#   --horizontal      : 縦分割の代わりに横分割を使用する
#   --lines <n>       : 取得するスクロールバック行数（デフォルト: 200）
#   --timeout <sec>   : コマンド完了待機のタイムアウト秒数（デフォルト: 300）
#   --no-wait         : --wait-for-prompt を使わず即座に返す
#   --keep-pane       : コマンド完了後もペインを閉じない（デフォルト: 閉じない）
#
# 標準出力: 実行結果のプレーンテキスト
# 終了コード: コマンドの終了コード（0 = 成功、非0 = 失敗）
#
# 例:
#   ./run-in-pane.sh "build" "cargo build --release"
#   ./run-in-pane.sh "tests" "cargo test" --lines 500 --timeout 120

set -euo pipefail

# ---- デフォルト値 ----
SESSION="default"
BASE_PANE=""
HORIZONTAL=""
LINES=200
TIMEOUT=300
NO_WAIT=0

# ---- 引数パース ----
if [[ $# -lt 2 ]]; then
    echo "使用方法: $0 <pane-title> <command> [options]" >&2
    echo "詳細は $0 --help を参照してください" >&2
    exit 1
fi

if [[ "${1:-}" == "--help" ]]; then
    head -30 "$0" | grep "^#" | sed 's/^# \{0,1\}//'
    exit 0
fi

PANE_TITLE="$1"
COMMAND="$2"
shift 2

while [[ $# -gt 0 ]]; do
    case "$1" in
        --session)
            SESSION="$2"; shift 2 ;;
        --base-pane)
            BASE_PANE="$2"; shift 2 ;;
        --horizontal)
            HORIZONTAL="--horizontal"; shift ;;
        --lines)
            LINES="$2"; shift 2 ;;
        --timeout)
            TIMEOUT="$2"; shift 2 ;;
        --no-wait)
            NO_WAIT=1; shift ;;
        *)
            echo "不明なオプション: $1" >&2
            exit 1 ;;
    esac
done

# ---- yatamux コマンドの存在確認 ----
if ! command -v yatamux &>/dev/null; then
    echo "エラー: yatamux コマンドが見つかりません。" >&2
    echo "  インストール: cargo install --path . (リポジトリルートで実行)" >&2
    exit 1
fi

# セッションフラグ
SESSION_FLAG="--session $SESSION"

# ---- ステップ 1: 現在のペイン一覧を確認する ----
echo "[run-in-pane] ペイン一覧を確認しています..." >&2
PANES_JSON=$(yatamux $SESSION_FLAG list-panes --json 2>&1) || {
    echo "エラー: Yatamux に接続できません。Yatamux が起動しているか確認してください。" >&2
    exit 1
}

# ベースペインが指定されていない場合は最初のペインを使用する
if [[ -z "$BASE_PANE" ]]; then
    BASE_PANE=$(echo "$PANES_JSON" | grep -o '"id":[0-9]*' | head -1 | grep -o '[0-9]*')
    if [[ -z "$BASE_PANE" ]]; then
        echo "エラー: アクティブなペインが見つかりません。" >&2
        exit 1
    fi
fi

echo "[run-in-pane] ベースペイン: $BASE_PANE" >&2

# ---- ステップ 2: 新しいペインを作成する ----
echo "[run-in-pane] 新しいペインを作成しています (タイトル: $PANE_TITLE)..." >&2

SPLIT_OUTPUT=$(yatamux $SESSION_FLAG split-pane --pane "$BASE_PANE" $HORIZONTAL 2>&1) || {
    echo "エラー: ペインの作成に失敗しました: $SPLIT_OUTPUT" >&2
    exit 1
}

# 新しいペイン ID を抽出する（"Created pane <id>" 形式）
NEW_PANE=$(echo "$SPLIT_OUTPUT" | grep -oP 'Created pane \K[0-9]+' || true)
if [[ -z "$NEW_PANE" ]]; then
    echo "エラー: 新しいペイン ID を取得できませんでした。出力: $SPLIT_OUTPUT" >&2
    exit 1
fi

echo "[run-in-pane] 新しいペイン ID: $NEW_PANE" >&2

# ---- ステップ 3: ペインにタイトルを設定する（オプション）----
# OSC 0 でウィンドウタイトルを設定する（シェルがサポートしている場合）
yatamux $SESSION_FLAG send-keys --pane "$NEW_PANE" --enter -- \
    "printf '\\033]0;${PANE_TITLE}\\007'" 2>/dev/null || true

# ---- ステップ 4: コマンドを送信する ----
echo "[run-in-pane] コマンドを送信しています: $COMMAND" >&2

if [[ $NO_WAIT -eq 0 ]]; then
    # --wait-for-prompt でコマンド完了を待機する
    EXIT_CODE=0
    yatamux $SESSION_FLAG send-keys --pane "$NEW_PANE" --enter --wait-for-prompt \
        -- "$COMMAND" || EXIT_CODE=$?
    echo "[run-in-pane] コマンドが完了しました (終了コード: $EXIT_CODE)" >&2
else
    # 即時返却モード（ポーリングは呼び出し元の責任）
    yatamux $SESSION_FLAG send-keys --pane "$NEW_PANE" --enter -- "$COMMAND"
    EXIT_CODE=0
    echo "[run-in-pane] コマンドを送信しました（--no-wait モード）" >&2
fi

# ---- ステップ 5: 出力を取得する ----
echo "[run-in-pane] 出力を取得しています..." >&2
yatamux $SESSION_FLAG capture-pane --pane "$NEW_PANE" --plain-text --lines "$LINES"

# 終了コードを返す
exit $EXIT_CODE
