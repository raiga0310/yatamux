# テスト計画: OSC 52 クリップボード対応 (C-2)

## 概要

OSC 52 (`\x1b]52;c;<base64>\x07`) エスケープシーケンスを VT プロセッサで
パースし、base64 デコード済みのバイト列をシステムクリップボードへ書き込む。

## 対象ファイル

- `crates/terminal/src/vt.rs` — `osc_dispatch` に OSC 52 ハンドラを追加
- `crates/client/src/window.rs` — `WM_PAINT` / タイマー処理でクリップボード書き込み

---

## テストケース一覧

### TC-01: ASCII 文字列のコピー（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | `\x1b]52;c;aGVsbG8=\x07` |
| **期待値** | `clipboard_data = Some(b"hello".to_vec())` |
| **説明** | `aGVsbG8=` は `"hello"` の base64 エンコード |

```rust
#[test]
fn test_osc52_ascii() {
    let mut grid = Grid::new(80, 24, CjkWidthConfig::default());
    let mut proc = VtProcessor::new(&mut grid);
    let mut parser = vte::Parser::new();
    feed_bytes(&mut parser, &mut proc, b"\x1b]52;c;aGVsbG8=\x07");
    assert_eq!(proc.clipboard_data, Some(b"hello".to_vec()));
}
```

---

### TC-02: 日本語 UTF-8 文字列のコピー（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | `\x1b]52;c;<base64("こんにちは")>\x07` |
| **期待値** | `clipboard_data = Some("こんにちは".as_bytes().to_vec())` |
| **説明** | マルチバイト UTF-8 が正しく base64 デコードされること |

```rust
#[test]
fn test_osc52_utf8_japanese() {
    let b64 = base64_encode("こんにちは");
    let seq = format!("\x1b]52;c;{}\x07", b64);
    let mut grid = Grid::new(80, 24, CjkWidthConfig::default());
    let mut proc = VtProcessor::new(&mut grid);
    let mut parser = vte::Parser::new();
    feed_bytes(&mut parser, &mut proc, seq.as_bytes());
    assert_eq!(proc.clipboard_data, Some("こんにちは".as_bytes().to_vec()));
}
```

---

### TC-03: ST（String Terminator）終端のサポート（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | `\x1b]52;c;aGVsbG8=\x1b\\` （BEL ではなく ST 終端）|
| **期待値** | `clipboard_data = Some(b"hello".to_vec())` |
| **説明** | `\x1b\x5c`（ESC + `\`）による ST 終端も BEL と同様に処理すること |

```rust
#[test]
fn test_osc52_st_terminator() {
    let mut grid = Grid::new(80, 24, CjkWidthConfig::default());
    let mut proc = VtProcessor::new(&mut grid);
    let mut parser = vte::Parser::new();
    feed_bytes(&mut parser, &mut proc, b"\x1b]52;c;aGVsbG8=\x1b\\");
    assert_eq!(proc.clipboard_data, Some(b"hello".to_vec()));
}
```

---

### TC-04: 空データ（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | `\x1b]52;c;\x07` |
| **期待値** | `clipboard_data = Some(b"".to_vec())` （空スライス）|
| **説明** | 空のデータでクリップボードをクリアする用途にも対応 |

```rust
#[test]
fn test_osc52_empty_data() {
    let mut grid = Grid::new(80, 24, CjkWidthConfig::default());
    let mut proc = VtProcessor::new(&mut grid);
    let mut parser = vte::Parser::new();
    feed_bytes(&mut parser, &mut proc, b"\x1b]52;c;\x07");
    assert_eq!(proc.clipboard_data, Some(b"".to_vec()));
}
```

---

### TC-05: `c` 以外のクリップボード種別は無視（境界系）

| 項目 | 内容 |
|------|------|
| **入力** | `\x1b]52;p;aGVsbG8=\x07` （プライマリ選択 `p`）|
| **期待値** | `clipboard_data = None` （変化なし）|
| **説明** | `c`（クリップボード）のみ対応。`p`, `q`, `s` 等は無視 |

```rust
#[test]
fn test_osc52_non_clipboard_type_ignored() {
    let mut grid = Grid::new(80, 24, CjkWidthConfig::default());
    let mut proc = VtProcessor::new(&mut grid);
    let mut parser = vte::Parser::new();
    feed_bytes(&mut parser, &mut proc, b"\x1b]52;p;aGVsbG8=\x07");
    assert_eq!(proc.clipboard_data, None);
}
```

---

### TC-06: 不正な base64 は無視（エラー系）

| 項目 | 内容 |
|------|------|
| **入力** | `\x1b]52;c;!!!invalid!!!\x07` |
| **期待値** | `clipboard_data = None` （変化なし）|
| **説明** | 不正な base64 は panic せずに無視する |

```rust
#[test]
fn test_osc52_invalid_base64() {
    let mut grid = Grid::new(80, 24, CjkWidthConfig::default());
    let mut proc = VtProcessor::new(&mut grid);
    let mut parser = vte::Parser::new();
    feed_bytes(&mut parser, &mut proc, b"\x1b]52;c;!!!invalid!!!\x07");
    assert_eq!(proc.clipboard_data, None);
}
```

---

### TC-07: 複数回の OSC 52 で最新値に上書き（正常系）

| 項目 | 内容 |
|------|------|
| **入力** | `\x1b]52;c;Zmlyc3Q=\x07` → `\x1b]52;c;c2Vjb25k\x07` |
| **期待値** | `clipboard_data = Some(b"second".to_vec())` |
| **説明** | 直近の OSC 52 が常に優先される |

```rust
#[test]
fn test_osc52_overwrite() {
    let mut grid = Grid::new(80, 24, CjkWidthConfig::default());
    let mut proc = VtProcessor::new(&mut grid);
    let mut parser = vte::Parser::new();
    feed_bytes(&mut parser, &mut proc, b"\x1b]52;c;Zmlyc3Q=\x07");
    feed_bytes(&mut parser, &mut proc, b"\x1b]52;c;c2Vjb25k\x07");
    assert_eq!(proc.clipboard_data, Some(b"second".to_vec()));
}
```

---

## 実装チェックリスト

- [ ] `VtProcessor` に `clipboard_data: Option<Vec<u8>>` フィールドを追加
- [ ] `osc_dispatch` の `"52"` アームで base64 デコードを実装
- [ ] `base64` クレートを `yatamux-terminal` の依存に追加
- [ ] TC-01 〜 TC-07 がすべてグリーンになること
- [ ] `window.rs` の描画ループで `clipboard_data.take()` し `SetClipboardData` を呼び出す

## 参照

- [XTerm OSC 52 仕様](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html#h3-Operating-System-Commands)
- WezTerm: `wezterm-term/src/terminalstate/mod.rs` の `Osc::SystemNotification`
