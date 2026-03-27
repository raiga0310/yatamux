## テスト計画: 現在のペイン構成を名前付きレイアウトファイルとして保存 (C-19)

### TC-01: Pane モードで `S` キーを押すと保存プロンプトが表示される

- **前提**: Normal モードで複数ペインが存在する状態。
- **操作**: `Ctrl+B` で Pane モードに入り、`S` キーを押す。
- **期待結果**: 画面中央に「レイアウトを保存」プロンプトが表示される。テキスト入力フィールドが空の状態で表示される。

### TC-02: プロンプトでファイル名を入力して Enter を押すと保存される

- **前提**: TC-01 でプロンプトが表示された状態。
- **操作**: `work` と入力して Enter を押す。
- **期待結果**: `%APPDATA%\yatamux\layouts\work.toml` が作成される。「レイアウト「work」を保存しました」というトースト通知が表示される。プロンプトが閉じる。

### TC-03: Esc でプロンプトをキャンセルできる

- **前提**: TC-01 でプロンプトが表示された状態。
- **操作**: Esc を押す。
- **期待結果**: プロンプトが閉じる。ファイルは作成されない。

### TC-04: 空の名前で Enter を押してもファイルが作成されない

- **前提**: TC-01 でプロンプトが表示された状態（入力フィールドが空）。
- **操作**: 何も入力せずに Enter を押す。
- **期待結果**: プロンプトが閉じる。ファイルは作成されない。

### TC-05: Backspace で文字を削除できる

- **前提**: プロンプトに `mylay` と入力した状態。
- **操作**: Backspace を押す。
- **期待結果**: 末尾の `y` が削除されて `myla` になる。

### TC-06: 保存したファイルをランチャーで読み込める

- **前提**: TC-02 で `work.toml` が保存された状態。
- **操作**: Pane モードで `L` キーを押してランチャーを開く。
- **期待結果**: `work` がランチャーの一覧に表示される。

### TC-07: 上書き保存（同名ファイルの再保存）

- **前提**: `work.toml` が既に存在する状態。
- **操作**: Pane モードで `S` → `work` → Enter。
- **期待結果**: 既存の `work.toml` が上書きされる。エラーにならない。

### TC-08: ステータスバーに `S: 保存` が表示される

- **前提**: Pane モードに移行した状態。
- **期待結果**: ステータスバーのヒントに `S: 保存` が含まれる。

---

### ユニットテスト（自動）

#### TC-C19-01: layout_to_toml — 単一ペイン

```
layout_to_toml(&Leaf(1))
→ "[[panes]]\n\n"
```

#### TC-C19-02: layout_to_toml — 垂直分割

```
layout_to_toml(&Split(Vertical, Leaf(1), Leaf(2)))
→ "[[panes]]\n\n[[panes]]\nsplit = \"vertical\"\n\n"
```

#### TC-C19-03: layout_to_toml — 水平分割を含むネスト構造

```
layout_to_toml(&Split(Vertical, Leaf(1), Split(Horizontal, Leaf(2), Leaf(3))))
→ 3 エントリ（1つ目はsplitなし、2つ目は vertical、3つ目は horizontal）
```

#### TC-C19-04: save_layout_file — 正常書き込み

```
save_layout_file("test_c19", "[[panes]]\n\n")
→ Ok(())、%APPDATA%\yatamux\layouts\test_c19.toml が作成される
```

---

## C-23: レイアウト保存時にペインのコマンドも含める

### TC-C23-01: コマンドなしペインは command 行を出力しない

```
layout_to_toml(&Leaf(1), &{})
→ "[[panes]]\n\n"
```

### TC-C23-02: コマンドありペインは command 行を出力する

```
layout_to_toml(&Leaf(1), &{PaneId(1) => "cargo watch"})
→ '[[panes]]\ncommand = "cargo watch"\n\n'
```

### TC-C23-03: 垂直分割でコマンドを持つペインが TOML に含まれる

```
layout_to_toml(
  &Split { direction: Vertical, ratio: 0.5, first: Leaf(1), second: Leaf(2) },
  &{PaneId(2) => "cargo test"}
)
→ '[[panes]]\n\n[[panes]]\ncommand = "cargo test"\nsplit = "vertical"\n\n'
```

### TC-C23-04: レイアウト適用後に保存するとコマンドが引き継がれる（手動確認）

- **前提**: `layouts/my-dev.toml` に `command = "cargo watch"` を含む2ペイン定義
- **操作**: `Ctrl+B` → `L` でランチャー起動 → `my-dev` を選択・適用 → `Ctrl+B` → `S` → 名前入力 → Enter
- **期待結果**: 保存された TOML に `command = "cargo watch"` が含まれる
