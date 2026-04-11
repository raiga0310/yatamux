# yatamux エージェント運用ガイド

AI エージェント（Claude Code / Codex / その他）が yatamux を安全に操作するための
リファレンス。CLI レベルの操作説明は README を参照。このドキュメントは
「どの順番で何を確認するか」「失敗時にどうリカバリするか」に焦点を当てる。

## 目次

1. [標準監視フロー](#標準監視フロー)
2. [ペイン選択の判断基準](#ペイン選択の判断基準)
3. [lagged 後の再同期手順](#lagged-後の再同期手順)
4. [セキュリティ・信頼境界](#セキュリティ信頼境界)
5. [ファイル保存場所とトラブルシュート](#ファイル保存場所とトラブルシュート)
6. [表示制限（CJK / IME / 絵文字）](#表示制限)

---

## 標準監視フロー

### 単発コマンド実行

最もシンプルなパターン。`exec` に完了条件とタイムアウトを指定する。

```bash
# 1. 利用可能なペインを確認
yatamux list-panes --json

# 2. 特定ペインにコマンドを送信し、完了を待つ
yatamux exec --pane <id> --timeout 60 -- cargo test

# 3. 出力を確認（完了後）
yatamux capture-pane --pane <id> --json --lines 100
```

`exec` は `ExecResult` が返るまでブロックする。`--timeout` を超えるとタイムアウトエラーになる。

### 長時間ジョブの監視

```bash
# 1. ペイン一覧で ワーカーペインを確認
yatamux list-panes --json

# 2. ジョブを開始（完了を待たずに返る wait 条件: none）
yatamux exec --pane <id> --timeout 1 -- ./long_job.sh

# 3. ストリームを購読してリアルタイム監視
yatamux subscribe-pane --pane <id> --json
#   → 1行ごとに {"type":"output","pane":N,"data":...} が届く

# 4. lagged エラーが出たら再同期（後述）
```

### エラー・ハング時の強制停止

```bash
# Ctrl+C 相当
yatamux interrupt-pane --pane <id>

# それでも止まらない場合は強制 kill
yatamux terminate-pane --pane <id>

# ペインごと閉じる（不要になったペインの後始末）
yatamux close-pane --pane <id>
```

---

## ペイン選択の判断基準

`list-panes --json` が返す各フィールドの活用方針:

| フィールド | 用途 |
|-----------|------|
| `busy` | `true` の場合はコマンドが実行中。誤送信を避けるため `false` のペインを選ぶ |
| `command` | 現在実行中のプロセス名（シェル以外の場合のみ）。agent / test / server など role に使う |
| `role` | `set-pane-meta --role` で付与した論理名。`worker` `monitor` など |
| `alias` | 人間が付けた短縮名。`tests` `server-a` など |
| `active` | GUI でフォーカスされているペイン。エージェントが意図せず割り込まないよう注意 |
| `floating` | フローティングペイン（一時的な用途向け）かどうか |
| `cwd` | 現在の作業ディレクトリ。コマンドを送る前にプロジェクトルートか確認する |

**推奨**: ペインに事前に `alias` / `role` を付与しておくことで、ペイン ID が変わっても論理名で参照できる。

```bash
yatamux set-pane-meta --pane <id> --alias worker-a --role executor
```

---

## lagged 後の再同期手順

`subscribe-pane` のストリームが途切れた場合（`{"type":"error","message":"subscription lagged by N messages..."}` を受信）:

```bash
# 1. 現在の画面状態をスナップショットで取得
yatamux capture-pane --pane <id> --json --lines 200

# 2. スナップショットの visible_text / scrollback_tail を読んで状態を把握

# 3. 購読を再開
yatamux subscribe-pane --pane <id> --json
```

lagged は broadcast バッファ（256 メッセージ）が溢れたときに発生する。
長時間の大量出力ジョブでは `capture-pane` によるポーリングの方が安全なことがある。

---

## セキュリティ・信頼境界

### Named Pipe の信頼境界

`\\.\pipe\yatamux-{session}` に接続できるのは **同一 Windows ユーザー** のみ。
DACL で他ユーザーからの接続は OS レベルで拒否される（C-41 対応済み）。

### 想定するローカル脅威モデル

| 脅威 | 対策 |
|------|------|
| 同一ユーザーの悪意あるプロセスが pipe に接続 | ユーザーが自分の意思でインストールしたプロセスと同等の信頼。pipe 名を知られても同ユーザーのみ接続可 |
| oversized メッセージによる DoS | 1 MiB 超のメッセージはエラー応答後に切断（C-41 対応済み） |
| broadcast lag による誤読 | lagged 発生時はエラー通知して切断（黙って継続しない） |
| 他ユーザーからのセッション盗聴 | DACL で拒否（C-41 対応済み） |

### プロトコルバージョン不一致

接続直後に `Handshake` メッセージを送ることで、バージョン不一致を早期検出できる（C-42）。
旧クライアントは `Handshake` を送らなくてもレガシーモードで接続可能。

### 推奨設定

```toml
# %APPDATA%\yatamux\config.toml

[appearance]
alert_border = "#FF6B6B"   # 通知時のペインボーダー色（デフォルト）
```

---

## ファイル保存場所とトラブルシュート

すべての設定・データは `%APPDATA%\yatamux\` 以下に保存される。

| パス | 内容 | 上書きタイミング |
|------|------|----------------|
| `%APPDATA%\yatamux\config.toml` | 全体設定（フォント / テーマ / フック） | 手動編集 / `yatamux source-config` |
| `%APPDATA%\yatamux\session.toml` | セッションレイアウト（Ctrl+B → S で保存 / SaveAndQuit 時） | 自動保存 |
| `%APPDATA%\yatamux\layouts\<name>.toml` | 名前付きレイアウト定義 | 手動管理 |
| `%APPDATA%\yatamux\themes\<name>.toml` | カラーテーマ定義 | 手動管理 |

### 設定の優先順位

1. `config.toml` の `[appearance]` セクション — フォント / 色を上書き
2. テーマファイル（`Ctrl+P` で切り替え）— `[appearance]` フィールドを上書き
3. ハードコードされたデフォルト値

テーマはランタイムに切り替え可能（フォントを除く）。フォント変更は再起動が必要。

### よくある問題

**セッション復元後にコマンドが HOME で起動する**:
- `session.toml` に `cwd` が保存されていない（古いバージョンのセッションファイル）
- 削除して再保存する: `%APPDATA%\yatamux\session.toml` を削除 → 再起動後に `Ctrl+B → S`

**`yatamux update` が失敗する**:
- ペイン内から実行している場合、`YATAMUX=1` が設定されている
- IPC 接続に失敗した場合は `exit 1` で終了する（誤った自己置換を防ぐため）
- `yatamux list-panes` で yatamux が動作中かどうか確認する

**ペインのフォント / 文字化け**:
- `config.toml` の `[appearance] font_family` で優先フォントを設定する
- CJK 文字の幅計算の詳細は「[表示制限](#表示制限)」を参照

---

## 表示制限

### CJK 全角文字

yatamux は East Asian Ambiguous 幅の計算を ConPTY のカーソル位置に依存せず、
独自の `CjkWidthConfig` で計算する。理由: ConPTY のカーソル位置報告が
フォントに依存して揺れるため。

**動作**:
- U+2E80–U+9FFF, U+AC00–U+D7A3, U+F900–U+FAFF, U+FE30–U+FE4F,
  U+FF01–U+FF60, U+FFE0–U+FFE6 など CJK/全角文字は幅 2 として扱う
- `Grapheme { width: 2 }` + `Continuation` セルのペアで格納する

**制限**:
- East Asian Ambiguous（U+00B2 など）は幅 1 として扱う（設定変更なし）
- 端末側（ConPTY）の幅計算と yatamux 側が一致しない場合、カーソルがずれることがある

### IME

- `WM_IME_COMPOSITION` で確定前の変換文字列を取得し、プレビュー表示する
- 変換確定時（`WM_IME_CHAR`）に PTY へ送信する
- IME 候補ウィンドウはカーソル位置に追従する
- **制限**: 保存プロンプト（レイアウト名入力）では IME 確定文字列はペインに誤送信されず入力欄へ反映される

### ZWJ 絵文字 / 合字

- 複数コードポイントを組み合わせる ZWJ シーケンス（👨‍💻 など）は
  現状 1 コードポイントずつ独立して処理される
- 表示は端末フォントと ConPTY の実装に依存する
- **制限**: ZWJ 絵文字は正しく 1 文字として幅計算されない場合がある

### 推奨フォント

等幅かつ CJK 幅が安定しているフォントを推奨:

1. HackGen Console NF / HackGen Console（最推奨）
2. Cascadia Mono / Cascadia Code
3. Consolas（フォールバック）

Nerd Fonts 系は罫線文字を `MoveToEx`/`LineTo`/`FillRect` で描画するため、
フォントに罫線グリフがなくても問題ない。
