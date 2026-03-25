# send-keys / list-panes 動作確認手順

`feat/send-keys` ブランチの実装を手動で確認するための手順書。

---

## 前提

- Windows 10 1903 (Build 18362) 以降
- `cargo` がインストール済み
- ターミナルを **2 つ** 用意する（ウィンドウ A: yatamux 本体、ウィンドウ B: CLI 操作用）

---

## ビルド

```powershell
cd C:\Users\raiga\dev\cmux-win
cargo build
```

バイナリ: `target\debug\yatamux.exe`

---

## ケース 1 — yatamux 未起動時に list-panes を実行するとエラーになる

**ウィンドウ B** で実行（yatamux は起動しない）:

```powershell
.\target\debug\yatamux.exe list-panes
```

**期待結果:**
```
Error: yatamux is not running (could not connect to IPC pipe)
```
exit code が 0 以外であること。

---

## ケース 2 — 起動後に list-panes でペイン一覧が取得できる

**ウィンドウ A** で yatamux を起動:

```powershell
.\target\debug\yatamux.exe
```

GUI ウィンドウが開き、シェルが起動することを確認する。

**ウィンドウ B** で実行:

```powershell
.\target\debug\yatamux.exe list-panes
```

**期待結果:**
```
pane   surface  cols   rows   title
----------------------------------------
1      1        220    50
```

- `pane` 列に ID が表示されること
- `cols` / `rows` がウィンドウの実際のサイズと一致すること

---

## ケース 3 — send-keys でペインにテキストを送信できる

ケース 2 の続き。`list-panes` で確認した pane ID（以下 `<ID>`）を使う。

**ウィンドウ B** で実行:

```powershell
.\target\debug\yatamux.exe send-keys --pane <ID> "echo hello\r"
```

**期待結果:**
- コマンドが exit 0 で終了する
- **ウィンドウ A** の yatamux 画面に `echo hello` が入力されて実行され、`hello` が出力される

---

## ケース 4 — ペインを分割後に list-panes で複数ペインが取得できる

**ウィンドウ A** の yatamux 上で:

- `Ctrl+Shift+E` → 縦分割
- または `Ctrl+Shift+O` → 横分割

**ウィンドウ B** で実行:

```powershell
.\target\debug\yatamux.exe list-panes
```

**期待結果:**
```
pane   surface  cols   rows   title
----------------------------------------
1      1        110    50
2      1        110    50
```

2 行表示されること。`cols` が分割後の幅（半分）になっていること。

---

## ケース 5 — 別ペインに send-keys で送信できる

ケース 4 の続き。2 つ目のペイン ID（例: `2`）に対して送信する。

**ウィンドウ B** で実行:

```powershell
.\target\debug\yatamux.exe send-keys --pane 2 "echo from-cli\r"
```

**期待結果:**
- ウィンドウ A の **2 つ目のペイン**（フォーカスがなくても）に `echo from-cli` が入力・実行される
- 1 つ目のペインには何も起きない

---

## ケース 6 — 存在しない pane ID に send-keys を送っても yatamux が落ちない

```powershell
.\target\debug\yatamux.exe send-keys --pane 9999 "test\r"
```

**期待結果:**
- コマンドが exit 0 で終了する（エラーメッセージなし）
- ウィンドウ A の yatamux が継続して動作すること

---

## ケース 7 — 引数不足時に使い方が表示される

```powershell
.\target\debug\yatamux.exe send-keys --pane 1
```

**期待結果:**
```
Usage: yatamux send-keys --pane <id> <text>
```
exit code が 0 以外であること。

---

## ケース 8 — CJK 文字を送信できる

```powershell
.\target\debug\yatamux.exe send-keys --pane <ID> "echo こんにちは\r"
```

**期待結果:**
- ウィンドウ A に `こんにちは` が正しく表示される（文字化けしない）

---

## 確認チェックリスト

| # | 内容 | 結果 |
|---|------|------|
| 1 | 未起動時に list-panes → エラー終了 | |
| 2 | 起動後に list-panes → pane 一覧表示 | |
| 3 | send-keys でテキスト送信・実行される | |
| 4 | ペイン分割後に list-panes → 複数ペイン表示 | |
| 5 | 非アクティブペインへの send-keys が届く | |
| 6 | 存在しない pane ID への send-keys で yatamux が落ちない | |
| 7 | 引数不足で使い方表示 | |
| 8 | CJK 文字送信で文字化けしない | |
