# 設計メモ: ステータスバー + モードベース UI (C-1)

## 概要

画面下部に固定高さのステータスバーを設け、現在のモードと利用可能なキーバインドをリアルタイム表示する。

## モード定義

| モード | 説明 | 切り替えキー |
|--------|------|-------------|
| `Normal` | 通常入力（デフォルト）| — |
| `Pane` | ペイン操作（分割・移動・削除）| `Ctrl+B` |

Normal モードでは全キー入力を PTY に透過する。
Pane モードに入ると次のキー 1 打でペイン操作を行い、即座に Normal に戻る（one-shot）。

## ステータスバーレイアウト

```
[NORMAL] Ctrl+B: Pane mode                                   pane 1/2
[PANE]   H/J/K/L: 移動  E/O: 分割  W: 削除  q: キャンセル   pane 2/2
```

- 高さ: `cell_height` × 1 行分
- 左: モード名 + ヒント
- 右: アクティブペイン番号 / 総ペイン数

## 実装方針

### `ClientMode` 列挙型 (`window.rs`)

```rust
#[derive(Clone, Copy, PartialEq, Eq)]
enum ClientMode {
    Normal,
    Pane,
}
```

### `ClientState` への追加フィールド

```rust
pub mode: std::cell::Cell<ClientMode>,
```

### `WM_KEYDOWN` の変更

- Normal モードで `Ctrl+B` → `mode` を `Pane` に変更、`LRESULT(0)` を返す
- Pane モードでは既存の分割・移動・削除を処理し、`mode` を `Normal` に戻す
- Pane モードで `q` / `Escape` → `mode` を `Normal` に戻す
- Pane モード中は文字入力を PTY に送らない（`WM_CHAR` でガード）

### `paint()` の変更

- コンテンツ領域の下端から `cell_height` 分を予約してステータスバー領域とする
- ペインの `compute_rects()` に渡す `total_rect.h` から `cell_height` を引く
- ステータスバーを `paint_status_bar()` で描画

### ウィンドウサイズの調整

- `content_rect` 計算（`WM_SIZE`）でステータスバー分の高さを引く
- 初期ウィンドウサイズに `cell_height` を加算

## データフロー

```
WM_KEYDOWN (Ctrl+B)
  → state.mode.set(ClientMode::Pane)
  → InvalidateRect → WM_PAINT → paint_status_bar() で "PANE" 表示

WM_KEYDOWN (H/J/K/L in Pane mode)
  → focus_pane_in_direction()
  → state.mode.set(ClientMode::Normal)
  → InvalidateRect
```
