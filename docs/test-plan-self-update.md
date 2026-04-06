## テスト計画: セルフアップデート機能

Codex との壁打ち（2026-04-05）を経て策定。

---

### 設計前提

- `yatamux update` CLI サブコマンドでエージェントから呼び出せる
- GitHub Releases からバイナリ + `checksums.txt` を取得
- 実行中インスタンスへ IPC（named pipe）で `SaveAndQuit` を送信
- バイナリ置換は `--apply-update <pid> <new_path>` 内部ヘルパーモードで別プロセスが担当
- 新インスタンスは `session.toml` から自動復元

---

### unit test

#### TC-01: バージョン比較ロジック
- **前提**: `current = "0.1.0"`
- **操作**: latest が `0.2.0` / `0.1.0` / `0.0.9` / `0.2.0-beta.1` の各ケースで比較
- **期待結果**: `0.2.0` のみ更新対象、同版・旧版・prerelease はスキップ

#### TC-02: GitHub Releases JSON パース
- **前提**: モック JSON（正常系・assets なし・yatamux.exe なし）
- **操作**: `parse_release_info()` を呼ぶ
- **期待結果**: asset URL と tag_name を正しく抽出、該当なし時は `None`

#### TC-03: SHA256 検証
- **前提**: 既知バイト列とその SHA256 ハッシュ文字列（モック不要）
- **操作**: `verify_checksum(bytes, expected_hex)` を呼ぶ
- **期待結果**: 一致時 `Ok`、不一致時 `Err`

#### TC-04: ファイルパス導出
- **前提**: `current_exe = C:\...\yatamux.exe`
- **操作**: `plan_update_paths(exe)` を呼ぶ
- **期待結果**: `.new` と `.bak` パスが正しく導出される。既存 `.bak` がある場合の扱いも定義する

#### TC-05: エラー分岐の更新計画
- **操作**: `download_fail` / `hash_mismatch` / `quit_timeout` / `replace_fail` / `launch_fail` の各ケースをシミュレート
- **期待結果**: `.new` ファイルの残留有無と rollback 動作が仕様通りになること

#### TC-06: SaveAndQuit の serde
- **前提**: `ClientMessage::SaveAndQuit` を JSON シリアライズ
- **期待結果**: デシリアライズで元の型に戻る

#### TC-07: PaneStore → LayoutSnapshot 保存（共有関数化後）
- **前提**: `WM_CLOSE` から切り出した `save_session(store, path)` を直接呼ぶ
- **操作**: ペイン 2 枚の `PaneStore` を渡す
- **期待結果**: `session.toml` に正しくシリアライズされる（既存の `test_snapshot_file_roundtrip` 相当）

#### TC-08: 新プロセス起動コマンドの組み立て
- **前提**: `APPDATA` が設定されている環境
- **操作**: `build_launch_command(exe_path)` を呼ぶ
- **期待結果**: 正しいパスと引数が含まれる

---

### integration test

#### TC-09: mock HTTP でダウンロード〜チェックサム検証
- **前提**: ローカルに mock HTTP サーバーを立てて latest release JSON・ダウンロード URL・checksums.txt を返す
- **操作**: `fetch_and_verify()` をエンドツーエンドで実行
- **期待結果**: バイトが取得でき、SHA256 が一致したら `Ok`

#### TC-10: app bridge の SaveAndQuit 経路で `session.toml` が書き出される
- **前提**: temp APPDATA 上で `spawn_bridge_fanout()` + `spawn_server_bridge()` を組み合わせる
- **操作**: `ServerMessage::SaveAndQuit` を流し、続けて `ServerMessage::AllPaneProcesses` を返す
- **期待結果**: `ClientMessage::QueryAllPaneProcesses` が送られたあと、`session.toml` が書き出され、`should_quit` が `true` になる

#### TC-11: 起動時 restore path が `session.toml` からレイアウトとメタデータを復元する
- **前提**: APPDATA を temp dir に向け、既知の `session.toml` を配置
- **操作**: `load_initial_layout(None, ...)` を呼び、必要な `PaneCreated` 応答を返す
- **期待結果**: 保存時と同じ split 構成、active pane、command / alias / role が復元される

#### TC-12: checksum 不一致時は置換しない
- **操作**: `verify_checksum` に不正ハッシュを渡してフロー全体を流す
- **期待結果**: staging 前に stale `.new` が掃除され、失敗後も `.new` が残らず、既存 exe が変更されない

#### TC-13: quit timeout 時は rename しない
- **操作**: SaveAndQuit を送っても応答がないケースをシミュレート（IPC mock）
- **期待結果**: `.bak` への rename が走らず、`<exe>.new` は保持されたまま既存 exe が変更されない

#### TC-14: バイナリ置換（`--apply-update` ヘルパーモード）
- **前提**: temp dir に `yatamux.exe`（実行中）と `yatamux.exe.new` を配置
- **操作**: ヘルパーモードで「PID 終了待ち → old を `.bak` に rename → new を本体名に rename → 新 exe 起動」
- **期待結果**: `old → .bak`、`new → yatamux.exe`、新 exe が起動できる
- **注意**: 待機は `sleep` ではなく process handle / PID wait で行うこと

---

### モック境界

| モックする | Real で動かす |
|-----------|--------------|
| GitHub API 応答・HTTP | SHA256、serde |
| clock / backoff | temp filesystem |
| progress UI・ログ | named pipe IPC |
| `ReleaseClient`・`IpcClient`・`Launcher` | session.toml 保存/読込・process spawn |

---

### CI 実行ポリシー

| テスト種別 | 実行タイミング |
|-----------|--------------|
| unit（TC-01〜08） | 常時（`cargo test`）|
| integration（TC-09〜13） | PR / CI（Windows-only）|
| バイナリ置換（TC-14） | PR / CI（Windows-only）|
| update smoke（実 GitHub 通信） | release 前のみ |

---

### 実装上の前提条件（テスト前に整備が必要なもの）

- `WM_CLOSE` 内の保存処理を `save_session(store, path)` として切り出す（`wndproc.rs:L484` 付近）
- `ClientMessage::SaveAndQuit` を `crates/protocol/src/message.rs` に追加する
- `yatamux --apply-update <pid> <new_path>` の内部ヘルパーモードを `src/main.rs` に追加する
