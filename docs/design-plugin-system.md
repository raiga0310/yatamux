# 設計メモ: プラグイン / 拡張システム (C-8)

## 採用方針: Candidate B — シェルフック + 既存 IPC

Lua / Rhai などのスクリプト言語の組み込みは依存クレートの追加によりバイナリサイズが増加する。
Zellij との差別化として「tmux 相当の軽量フットプリント」を維持するため、
**外部プロセス連携方式（Candidate B）** を採用する。

WASM は採用しない。

## アーキテクチャ

### 拡張の 2 経路

```
┌─────────────────────────────────────────────────────────┐
│ 拡張経路 1: イベントフック（push 型）                    │
│  yatamux → cmd.exe /C <hook_cmd>                        │
│  環境変数: YATAMUX_PANE_ID, YATAMUX_SESSION             │
├─────────────────────────────────────────────────────────┤
│ 拡張経路 2: IPC 操作（pull 型、既存）                   │
│  外部スクリプト → \\.\pipe\yatamux-{session}            │
│  JSON 行形式で ClientMessage を送信可能                  │
└─────────────────────────────────────────────────────────┘
```

### 設定ファイル

`%APPDATA%\yatamux\config.toml` に記述する。

```toml
[hooks]
# ペイン作成時に実行するコマンド（cmd.exe /C で実行）
on_pane_created = "echo pane %YATAMUX_PANE_ID% created >> %TEMP%\yatamux_events.log"

# ペイン終了時に実行するコマンド
on_pane_closed = ""
```

### 環境変数

| 変数名 | 内容 |
|---|---|
| `YATAMUX_PANE_ID` | イベントが発生したペインの数値 ID |
| `YATAMUX_SESSION` | セッション名（デフォルト: `default`） |

## フック実行の仕様

- 実行形式: `cmd.exe /C <command>` (Windows)
- 実行タイミング: イベント発生後、非同期（fire-and-forget）
- エラー: フックの失敗はトースト通知またはログ出力するが、yatamux の動作は継続
- 空文字列のフックは実行しない

## 将来的な拡張候補

- `on_session_save` / `on_session_load` フック
- `on_pane_output` フック（出力をパイプして加工）
- ステータスバーウィジェット（外部プロセスの stdout を表示）
  - `widget_left = "yatamux-widget-git-branch"` のような形式
- Lua / Rhai スクリプトは、ユーザー需要が高まった場合に feature flag で追加検討
