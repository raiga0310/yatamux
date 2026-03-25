# 設計メモ: フローティングペイン (C-3)

## スコープ（今回実装）

スタックペインは複雑度が高いため、今回は **フローティングペイン** に絞る。

## 概要

既存タイルレイアウトの上に重なるオーバーレイ式ペイン。
コンテンツ領域の中央に 80% × 80% の矩形で描画される。
`Ctrl+F` で作成・表示/非表示トグル。

## データモデル

### PaneStore に追加するフィールド

```rust
/// フローティングペインの ID（None = 未作成）
pub floating: Option<PaneId>,
/// フローティングペインを表示中かどうか
pub floating_visible: bool,
/// フローティング表示前のアクティブペイン（非表示時に復帰用）
pub pre_float_active: Option<PaneId>,
```

### floating_rect()

```rust
pub fn floating_rect(content: PaneRect) -> PaneRect {
    let w = (content.w as f32 * 0.8) as i32;
    let h = (content.h as f32 * 0.8) as i32;
    PaneRect {
        x: (content.w - w) / 2,
        y: (content.h - h) / 2,
        w,
        h,
    }
}
```

## チャネル設計

```
Ctrl+F (Win32 スレッド)
  → float_tx.try_send(())
  → app.rs select! ループが受信
  → None: CreatePane（pending_float=true）
      PaneCreated → grids 追加、floating = Some(id)、floating_visible = true
  → Some: floating_visible トグル、active 更新
```

## 描画フロー

`paint()` の末尾（separator / toast より前）:

1. `floating_visible` かつ `floating = Some(id)` を確認
2. `floating_rect(content)` で矩形を計算
3. 通常ペインと同じセル描画ロジックで描画
4. 矩形の境界に 2px の枠線を描画（サーフェス感を演出）

## フォーカス管理

- フローティングペインを表示するとき: `pre_float_active = Some(active)`, `active = floating_id`
- フローティングペインを非表示にするとき: `active = pre_float_active.unwrap_or(layout.pane_ids()[0])`

## run_window() 追加パラメータ

```rust
float_tx: mpsc::Sender<()>
```

## キーバインド

Pane モード中:
- `F`: フローティングペイントグル

直接（Normal モード）でも `Ctrl+F` でトグルできるようにする（頻繁に使うため）。
