# yatamux IPC プロトコル仕様

外部ツール（CLI / エージェント / MCP サーバー）が yatamux セッションと通信するための
Named Pipe IPC プロトコルのリファレンス。

## トランスポート

| 項目 | 値 |
|------|-----|
| パイプ名 | `\\.\pipe\yatamux-{session_name}` |
| エンコーディング | UTF-8 JSON Lines（1 メッセージ = 1 行 = 末尾 `\n`） |
| 最大メッセージサイズ | 1 MiB（超過するとエラー応答後に切断） |
| セキュリティ | DACL により同一ユーザー SID のみ接続可 |

## 接続フロー

```
Client                                    Server
  │                                          │
  │── Handshake ─────────────────────────────▶│  接続直後に必ず送る
  │◀─ HandshakeAccepted ────────────────────── │  バージョン / capability を確認
  │                                          │
  │── (任意) SubscribePane ──────────────────▶│  ストリーム購読
  │◀─ Output / Notification / ... ────────── │  購読イベント
  │                                          │
  │── ListPanes ──────────────────────────────▶│
  │◀─ PanesListed ─────────────────────────── │
  │                                          │
  │── Exec ─────────────────────────────────▶│
  │◀─ ExecResult ───────────────────────────── │
```

### バージョン管理

| 定数 | 値 | 意味 |
|------|-----|------|
| `PROTOCOL_VERSION` | 1 | 現在のサーバープロトコルバージョン |
| `MIN_CLIENT_VERSION` | 1 | サーバーが受け入れる最小クライアントバージョン |

クライアントが `protocol_version < MIN_CLIENT_VERSION` を送ると、サーバーは
`Error` メッセージを返して切断する。

### 後方互換性

- **旧クライアント → 新サーバー**: `Handshake` を送らないと、サーバーは legacy mode で継続動作する。
- **新クライアント → 旧サーバー**: `Handshake` を送ると、旧サーバーは未知メッセージとして警告ログを出し継続する。

---

## クライアント → サーバー メッセージ

メッセージには `"type"` フィールドがあり、`snake_case` で送る。

### Handshake

接続直後に送るプロトコルネゴシエーション。

```json
{
  "type": "handshake",
  "protocol_version": 1,
  "capabilities": ["subscribe_pane", "exec"]
}
```

| フィールド | 型 | 必須 | 説明 |
|------------|-----|------|------|
| `protocol_version` | `u32` | ✓ | クライアントのプロトコルバージョン |
| `capabilities` | `string[]` | — | クライアントがサポートする機能一覧 |

### ListPanes

全ペイン情報を取得する。

```json
{"type": "list_panes"}
```

→ [`PanesListed`](#paneslisted) が返る。

### CapturePane

ペインのスクロールバック末尾 N 行 + 現在画面を取得する。

```json
{
  "type": "capture_pane",
  "pane": 1,
  "lines": 50,
  "plain_text": true
}
```

| フィールド | 型 | 必須 | 説明 |
|------------|-----|------|------|
| `pane` | `PaneId` | ✓ | 対象ペイン ID |
| `lines` | `usize` | ✓ | 取得するスクロールバック行数 |
| `plain_text` | `bool` | — | `true` で ANSI エスケープ除去（デフォルト: `false`） |

→ [`PaneContent`](#panecontent) が返る。

### Exec

コマンド送信と完了待機を 1 回の要求にまとめる。

```json
{
  "type": "exec",
  "request_id": "req-001",
  "pane": 1,
  "data": [99, 97, 114, 103, 111, 32, 116, 101, 115, 116, 13],
  "wait": {"type": "output_regex", "pattern": "test result: ok", "lines": 200},
  "timeout_ms": 30000
}
```

| フィールド | 型 | 必須 | 説明 |
|------------|-----|------|------|
| `request_id` | `string` | ✓ | 応答照合用の一意 ID |
| `pane` | `PaneId` | ✓ | 対象ペイン |
| `data` | `u8[]` | ✓ | 送信バイト列（末尾 `\r` で Enter） |
| `wait` | `ExecWaitCondition` | ✓ | 完了待ち条件（後述） |
| `timeout_ms` | `u64` | ✓ | タイムアウト（ミリ秒） |

**ExecWaitCondition の種類**:

| 型 | フィールド | 説明 |
|----|-----------|------|
| `{"type": "output_regex", "pattern": "...", "lines": 200}` | `pattern`, `lines` | 直近 N 行中に正規表現マッチ |
| `{"type": "silence", "ms": 2000}` | `ms` | N ミリ秒無出力 |
| `{"type": "command_finished"}` | — | OSC 133;D（シェルインテグレーション） |
| `{"type": "pane_closed"}` | — | ペイン終了 |
| `{"type": "none"}` | — | 入力送信のみ（待機なし） |

→ [`ExecResult`](#execresult) が返る。

### SubscribePane / UnsubscribePane

指定ペインのリアルタイムストリームを購読・解除する。

```json
{"type": "subscribe_pane", "pane": 1}
{"type": "unsubscribe_pane", "pane": 1}
```

購読中は `Output` / `TitleChanged` / `Notification` / `ClipboardWrite` /
`PaneClosed` / `CommandFinished` / `PaneContent` / `PaneMetaUpdated` が届く。

未購読の場合、これらのイベントもすべてブロードキャストされる（後述）。

### Input

生バイト列を PTY に直接送る。`Exec` より低水準な操作。

```json
{"type": "input", "pane": 1, "data": [65, 66, 67]}
```

→ [`InputAccepted`](#inputaccepted) が返る。

### InterruptPane / TerminatePane / ClosePane

```json
{"type": "interrupt_pane", "pane": 1}   // Ctrl+C 送信
{"type": "terminate_pane", "pane": 1}   // 子プロセスを kill
{"type": "close_pane", "pane": 1}       // ペインを閉じる
```

### SetPaneMeta

ペインに alias / role を付与する。

```json
{
  "type": "set_pane_meta",
  "pane": 1,
  "alias": "worker-a",
  "role": "executor"
}
```

→ [`PaneMetaUpdated`](#panemetaupdated) が返る。

### SplitPane (CreatePane)

ペインを分割する。

```json
{
  "type": "create_pane",
  "surface": 1,
  "split_from": 1,
  "direction": "horizontal",
  "size": {"cols": 80, "rows": 24},
  "working_dir": "C:/Users/user/project"
}
```

`direction`: `"horizontal"` | `"vertical"`

### SaveAndQuit

セッションを保存して終了する（セルフアップデート用）。

```json
{"type": "save_and_quit"}
```

---

## サーバー → クライアント メッセージ

### HandshakeAccepted

`Handshake` への応答。

```json
{
  "type": "handshake_accepted",
  "protocol_version": 1,
  "min_client_version": 1,
  "capabilities": ["subscribe_pane", "exec", "capture_pane", "alias_role", "session_save"]
}
```

サーバーの capabilities 一覧:

| 値 | 意味 |
|----|------|
| `subscribe_pane` | `SubscribePane` / `UnsubscribePane` をサポート |
| `exec` | `Exec` / `ExecResult` をサポート |
| `capture_pane` | `CapturePane` （JSON モード含む）をサポート |
| `alias_role` | `SetPaneMeta` / `PaneMetaUpdated` をサポート |
| `session_save` | `SaveAndQuit` / セッション復元をサポート |

### PanesListed

`ListPanes` への応答。

```json
{
  "type": "panes_listed",
  "panes": [
    {
      "id": 1,
      "surface": 1,
      "title": "cmd",
      "cols": 120,
      "rows": 30,
      "cwd": "C:/Users/user/project",
      "command": "cargo",
      "busy": true,
      "last_output_unix_ms": 1744000000000,
      "active": true,
      "floating": false,
      "alias": "worker-a",
      "role": "executor"
    }
  ]
}
```

| フィールド | 型 | 説明 |
|------------|-----|------|
| `id` | `u32` | ペイン ID |
| `surface` | `u32` | 属するサーフェス ID |
| `title` | `string` | OSC 2 で設定されたタイトル |
| `cols` / `rows` | `u32` | 現在のサイズ |
| `cwd` | `string?` | 現在の作業ディレクトリ（取得不可の場合 null） |
| `command` | `string?` | 現在実行中のコマンド名（シェルを除く最初の子プロセス） |
| `busy` | `bool` | 直近 5 秒以内に出力があったら true |
| `last_output_unix_ms` | `u64?` | 最終出力の Unix タイムスタンプ（ms） |
| `active` | `bool` | GUI のアクティブペインかどうか |
| `floating` | `bool` | フローティングペインかどうか |
| `alias` | `string?` | 設定済みの alias |
| `role` | `string?` | 設定済みの role |

### PaneContent

`CapturePane` への応答。

```json
{
  "type": "pane_content",
  "pane": 1,
  "content": "line1\nline2\n...",
  "capture": {
    "title": "cmd",
    "cols": 120,
    "rows": 30,
    "lines_requested": 50,
    "scrollback_len": 12,
    "cursor": {"col": 5, "row": 2, "visible": true},
    "visible_text": ["line1", "line2"],
    "scrollback_tail": ["older_line"]
  }
}
```

`capture` は `--json` / 非 `plain_text` 時のみ含まれる（省略可）。

### ExecResult

`Exec` への応答。

```json
{
  "type": "exec_result",
  "request_id": "req-001",
  "pane": 1,
  "status": "completed",
  "exit_code": 0,
  "message": null
}
```

| `status` | 説明 |
|----------|------|
| `"completed"` | 待機条件を満たして正常完了 |
| `"timed_out"` | `timeout_ms` 超過 |
| `"pane_closed"` | 待機中にペインが閉じた |
| `"error"` | その他のエラー（`message` にエラー内容） |

### Output

購読中ペインの出力（VT シーケンスを含む生バイト列）。

```json
{"type": "output", "pane": 1, "data": [65, 66, 67]}
```

### Notification

OSC 9/99/777 または BEL（`\x07`）による通知。

```json
{"type": "notification", "pane": 1, "body": "Process exited"}
```

### PaneClosed

ペインが終了した。

```json
{"type": "pane_closed", "pane": 1}
```

### InputAccepted

`Input` メッセージが受理された。

```json
{"type": "input_accepted", "pane": 1}
```

### PaneMetaUpdated

`SetPaneMeta` の完了通知。

```json
{
  "type": "pane_meta_updated",
  "pane": 1,
  "alias": "worker-a",
  "role": "executor"
}
```

### Error

エラーが発生した場合。切断を伴う場合もある。

```json
{"type": "error", "message": "pane 99 not found"}
```

---

## 購読フィルタリング

`SubscribePane` で 1 つ以上のペインを登録すると、**登録外ペインのストリームイベント
（`Output` / `TitleChanged` / `Notification` / `ClipboardWrite` / `PaneClosed` /
`CommandFinished` / `PaneContent` / `PaneMetaUpdated`）は届かなくなる**。

`ExecResult` は購読状態に関わらず常に届く。

未購読（`SubscribePane` を一度も送っていない）の場合は、
すべてのブロードキャストメッセージが届く。

---

## lagged（購読遅延）処理

バックプレッシャーが発生し broadcast チャネルが lag した場合、サーバーは：

1. `Error { message: "subscription lagged by N messages; reconnect and use capture-pane --json to resync" }` を送信
2. 接続を切断する

**推奨再同期フロー**:

```
1. 再接続して Handshake を送る
2. CapturePane { lines: 200, plain_text: false } で現在の画面状態を取得
3. SubscribePane で購読を再開する
```

---

## エージェント向け標準監視フロー

```
# 1. セッション名を確認
yatamux list-panes --json

# 2. ワーカーペインにコマンドを送信して完了待機
yatamux exec --pane <id> --timeout 60 -- cargo test

# 3. ストリーム監視（長時間ジョブ）
yatamux subscribe-pane --pane <id> --json

# 4. lagged 後の再同期
yatamux capture-pane --pane <id> --json --lines 200
yatamux subscribe-pane --pane <id> --json

# 5. 失敗したジョブの停止
yatamux interrupt-pane --pane <id>
yatamux terminate-pane --pane <id>   # 強制終了が必要な場合
```

---

## セキュリティモデル

- Named Pipe の DACL は現在ユーザーの SID のみに限定されており、他ユーザーは接続不可。
- 1 メッセージ上限は 1 MiB。超過するクライアントはエラー応答後に切断される。
- broadcast lagged 発生時はエラーを通知してから切断する（黙って継続しない）。
- `SaveAndQuit` / `TerminatePane` などの破壊的操作は、同一ユーザーのローカルプロセスのみが実行可能。
