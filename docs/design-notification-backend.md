# 設計: 通知バックエンド抽象化 (F-7)

## 概要

yatamux がバックグラウンドにいるときも通知を受け取れるよう、通知の配信口を
`NotificationBackend` トレイトとして抽象化し、フォーカス状態に応じて
InternalToast / NativeToast を自動切り替えする。

## トレイト定義

```rust
pub trait NotificationBackend: Send + Sync {
    fn notify(&self, pane_id: PaneId, message: String);
}
```

## 実装

### InternalToast（既存経路）

- `Arc<Mutex<PaneStore>>` を保持
- `notify()` → `store.pending_toasts.push_back(Toast { ... })`
- Win32 スレッドの `WM_TIMER` が `pending_toasts` → `active_toasts` へ移動し `paint_toasts()` で描画

### NativeToast（OS ネイティブ通知）

- `Arc<Mutex<VecDeque<(String, String)>>>` (title, body キュー) を保持
- `notify()` → キューに `("yatamux", message)` を push
- Win32 スレッドの `WM_TIMER` がキューを drain し `Shell_NotifyIconW` でバルーンチップを表示
- バルーン表示から約 5 秒後に `NIM_DELETE` でアイコンを削除

#### Shell_NotifyIcon フロー

```
NIM_ADD  (NIF_ICON | NIF_TIP | NIF_INFO + NIIF_INFO)  → バルーン表示
  ↓ ~5 秒後 (WM_TIMER カウントダウン)
NIM_DELETE                                              → アイコン・バルーン削除
```

### FocusAwareBackend（切り替えラッパー）

- `focused: Arc<AtomicBool>` を保持
- `notify()`:
  - `focused == true`  → `InternalToast.notify()`（yatamux 内トースト）
  - `focused == false` → `NativeToast.notify()`（OS バルーン）

## フォーカス検知

- `WM_ACTIVATEAPP` で `wparam != 0`（アクティブ化）か判定
- `ClientState.app_focused: Arc<AtomicBool>` に格納
- `app.rs` がこの Arc を `FocusAwareBackend` と共有

## 変更ファイル

| ファイル | 変更内容 |
|---------|---------|
| `Cargo.toml` (workspace) | `Win32_UI_Shell` feature 追加 |
| `crates/client/src/notification.rs` | NEW: トレイト + 3 実装 |
| `crates/client/src/window.rs` | `WM_ACTIVATEAPP`、`WM_TIMER` native balloon、`run_window` 引数追加 |
| `crates/client/src/lib.rs` | `notification` モジュール re-export |
| `src/app.rs` | `FocusAwareBackend` を生成し `backend.notify()` で通知 |
