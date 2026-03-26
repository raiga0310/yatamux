## テスト計画: インタラクティブキーの PTY パススルー対応 (C-20)

### yatamux が現在消費するキー一覧

#### yatamux ショートカット（意図的な消費）

| キー | 動作 | skip_char? |
|------|------|-----------|
| Ctrl+B | Pane モード移行 | ✅ あり |
| Ctrl+F | フローティング切り替え | ✅ あり |
| Ctrl+V | クリップボードペースト | ❌ 未設定（修正対象） |
| Ctrl+C (選択あり) | テキストコピー | ✅ あり |
| Ctrl+Shift+E | 縦分割 | ctrl+shift で WM_CHAR 抑制 |
| Ctrl+Shift+O | 横分割 | ctrl+shift で WM_CHAR 抑制 |
| Ctrl+Shift+W | ペイン削除 | ctrl+shift で WM_CHAR 抑制 |
| Ctrl+Tab / Ctrl+Shift+Tab | ペインサイクル | code=9 で WM_CHAR 抑制 |
| Ctrl+Arrow | ペインフォーカス移動 | 矢印は WM_CHAR 非生成 |

#### keydown_to_vt で処理されるキー（PTY 向け）

| キー | 送信する VT シーケンス | WM_CHAR 二重送信? |
|------|---------------------|-----------------|
| Ctrl+A〜Z (汎用) | \x01〜\x1A | ❌ 二重送信（修正対象） |
| Backspace | \x7F | code=8 で WM_CHAR 抑制 ✅ |
| Tab | \t | code=9 で WM_CHAR 抑制 ✅ |
| 矢印キー / Home / End / PageUp/Down / F1-F12 | エスケープシーケンス | WM_CHAR 非生成 ✅ |

### 問題として報告されているキー

| キー | 制御コード | 状態 |
|------|----------|------|
| Ctrl+O | \x0F | WM_KEYDOWN + WM_CHAR で二重送信 → 実効的に動作不安定 |
| Ctrl+R | \x12 | 同上（シェル履歴検索で必要） |
| Ctrl+Z | \x1A | 同上 |
| Ctrl+\ | \x1C | WM_KEYDOWN では None → WM_CHAR のみ（正常） |

### 修正方針

`keydown_to_vt` が Some を返した場合に `skip_char.set(true)` して WM_CHAR を抑制する。
Ctrl+V にも `skip_char.set(true)` を追加する。

---

### TC-01: Ctrl+O が PTY に 1 回だけ届く

- **前提**: Normal モード。Claude Code または bash が起動中。
- **操作**: Ctrl+O を押す。
- **期待結果**: PTY に \x0F が 1 回だけ送信される。Claude Code ではエディタ起動ダイアログ、bash では operate-and-get-next が正常動作する。

### TC-02: Ctrl+R が PTY に 1 回だけ届く

- **前提**: Normal モード。bash / PowerShell が起動中。
- **操作**: Ctrl+R を押す。
- **期待結果**: PTY に \x12 が 1 回だけ送信される。bash では逆インクリメンタル検索（`(reverse-i-search)`）が起動する。

### TC-03: Ctrl+Z が PTY に 1 回だけ届く

- **前提**: Normal モード。シェルが起動中。
- **操作**: Ctrl+Z を押す。
- **期待結果**: PTY に \x1A が 1 回だけ送信される。bash では実行中プロセスを一時停止（SIGTSTP）できる。

### TC-04: Ctrl+\ が PTY に届く

- **前提**: Normal モード。シェルが起動中。
- **操作**: Ctrl+\ を押す。
- **期待結果**: PTY に \x1C が 1 回届く。実行中プロセスに SIGQUIT が送られる。
  （このキーは WM_KEYDOWN ではスキップ → WM_CHAR でのみ送信、修正前から正常）

### TC-05: Ctrl+V によるペーストで余分な \x16 が送られない

- **前提**: Normal モード。クリップボードに "hello" が入っている。
- **操作**: Ctrl+V を押す。
- **期待結果**: PTY に "hello" のみが送信される。\x16 (SYN) は送信されない。

### TC-06: Ctrl+B で Pane モードに入り \x02 は PTY に送られない

- **前提**: Normal モード。
- **操作**: Ctrl+B を押す。
- **期待結果**: Pane モードに遷移する。\x02 は PTY に送信されない。（修正前から正常）

### TC-07: 既存キーバインドが引き続き動作する

- **前提**: Normal モード。
- **操作**: Ctrl+Shift+E / Ctrl+Shift+O / Ctrl+Arrow などを押す。
- **期待結果**: 各ショートカットが従来通り動作する。

### TC-08: skip_char 二重送信防止ロジック（単体）

- **前提**: `skip_char` Cell を使った抑制ロジックのユニットテスト。
- **操作**: `skip_char.set(true)` → `skip_char.get()` → `skip_char.set(false)` のシーケンス。
- **期待結果**: フラグが正しくトグルする（既存テスト `test_skip_char_cell_behavior` で確認済み）。
