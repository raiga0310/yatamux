# send-keys / list-panes — テスト仕様

クロスペインメッセージング機能のテストケース一覧。
実装は Red → Green の順で各ケースを消化する。

## 背景・目的

yatamux の IPC サーバー（`\\.\pipe\yatamux-{session}`）を GUI 起動時に常時稼働させ、
外部プロセス（エージェント・スクリプト等）がペインに任意のキー入力を送れるようにする。

```
# 使用イメージ
yatamux list-panes                     # 実行中のペイン一覧
yatamux send-keys --pane 2 "ls\n"      # ペイン 2 に "ls<Enter>" を送信
yatamux send-keys --pane 2 "q"         # ペイン 2 に "q" を送信
```

---

## テストケース

### P — プロトコル層（`yatamux-protocol`）

| ID | テスト名 | 内容 | 期待結果 |
|----|---------|------|---------|
| P-1 | `list_panes_message_serializes` | `ClientMessage::ListPanes` を JSON シリアライズ | `{"type":"list_panes"}` になる |
| P-2 | `panes_listed_message_deserializes` | `ServerMessage::PanesListed { panes: [...] }` を JSON デシリアライズ | `PaneInfo` のフィールドが正しく復元される |
| P-3 | `pane_info_has_required_fields` | `PaneInfo { id, surface, title, cols, rows }` が構築できる | コンパイル通過・フィールド値が保持される |

### G — サーバー層（`yatamux-server` / `session.rs`）

| ID | テスト名 | 内容 | 期待結果 |
|----|---------|------|---------|
| G-8 | `list_panes_returns_all_panes` | Workspace→Surface→Pane×2 を作成後に `ListPanes` を送信 | `PanesListed` に 2 件の `PaneInfo` が含まれる |
| G-9 | `list_panes_returns_empty_when_no_panes` | ペイン未作成の状態で `ListPanes` | `PanesListed { panes: [] }` が返る |
| G-10 | `send_input_to_inactive_pane` | フォーカスと異なるペイン ID に `Input` を送信 | サーバーがエラーを返さず、対象ペインの PTY に届く |

### F — IPC 層（`yatamux-server` / IPC テスト）

| ID | テスト名 | 内容 | 期待結果 |
|----|---------|------|---------|
| F-6 | `ipc_list_panes_returns_panes_listed` | IPC 経由で `ListPanes` を送信 | `PanesListed` が返ってくる |
| F-7 | `ipc_send_keys_routes_to_pane` | IPC 経由で `Input { pane, data }` を送信 | `ServerMessage::Output` が該当ペインから返ってくる（echo 確認） |
| F-8 | `ipc_send_keys_to_unknown_pane_returns_error` | 存在しない `PaneId(9999)` に `Input` | `ServerMessage::Error` が返る |

### C — CLI 層（`yatamux` バイナリ / integration）

| ID | テスト名 | 内容 | 期待結果 |
|----|---------|------|---------|
| C-1 | `cli_list_panes_exits_zero` | `yatamux list-panes` を実行（IPC サーバーあり） | exit code 0、stdout に pane ID が含まれる |
| C-2 | `cli_send_keys_exits_zero` | `yatamux send-keys --pane 1 "test"` を実行 | exit code 0 |
| C-3 | `cli_no_server_exits_nonzero` | IPC サーバー未起動の状態で `yatamux list-panes` | exit code 非 0、stderr にエラーメッセージ |
| C-4 | `cli_send_keys_missing_pane_flag` | `--pane` フラグなしで `yatamux send-keys "text"` | exit code 非 0、使い方を表示 |

---

## 実装スコープ

### 追加・変更するファイル

| ファイル | 変更内容 |
|---------|---------|
| `crates/protocol/src/message.rs` | `ClientMessage::ListPanes`、`ServerMessage::PanesListed { panes }` を追加 |
| `crates/protocol/src/types.rs` | `PaneInfo { id, surface, title, cols, rows }` を追加 |
| `crates/server/src/session.rs` | `handle_client_message` に `ListPanes` ハンドラを追加 |
| `crates/server/tests/ipc_integration.rs` | F-6 / F-7 / F-8 を追加 |
| `src/main.rs` | サブコマンド解析（`list-panes` / `send-keys`）、GUI モードで IPC サーバー起動 |
| `src/app.rs` | `run_ipc_server` を tokio::spawn で起動するよう追加 |

### スコープ外（この PR では対象外）

- セッション名の指定（`--session` フラグ）— デフォルト `"default"` 固定
- ペイン番号の人間向け alias（`%1` 記法など）
- GUI 上でのクロスペイン入力リダイレクト
