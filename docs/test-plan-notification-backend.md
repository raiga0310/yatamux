## テスト計画: 通知バックエンド抽象化 (F-7)

### TC-01: フォーカス中 → InternalToast にルーティング
- **前提**: `FocusAwareBackend` を `focused=true` で構築
- **操作**: `backend.notify(PaneId(2), "test".to_string())` を呼び出す
- **期待結果**: `PaneStore.pending_toasts` に Toast が 1 件追加される

### TC-02: フォーカスなし → NativeToast にルーティング
- **前提**: `FocusAwareBackend` を `focused=false` で構築
- **操作**: `backend.notify(PaneId(2), "test".to_string())` を呼び出す
- **期待結果**: `PaneStore.pending_toasts` は空のまま、native queue に 1 件入る

### TC-03: フォーカス状態が動的に切り替わる
- **前提**: `focused=true` で `FocusAwareBackend` を構築
- **操作**: `notify()` → `focused.store(false)` → `notify()`
- **期待結果**: 最初の notify は pending_toasts へ、2 番目は native queue へ

### TC-04: WM_ACTIVATEAPP でフォーカス状態が更新される（手動テスト）
- **前提**: yatamux を起動してウィンドウを表示
- **操作**: 別ウィンドウをクリックして yatamux をバックグラウンドに
- **期待結果**: `app_focused` が false になる

### TC-05: バックグラウンド時の通知がシステムバルーンで表示される（手動テスト）
- **前提**: 2 ペインに分割、yatamux をバックグラウンドに
- **操作**: バックグラウンドペインで `printf '\e]9;hello\a'` を実行
- **期待結果**: Windows タスクバーにバルーンチップ「yatamux - hello」が表示される

### TC-06: フォーカス中の通知が引き続き InternalToast で表示される（手動テスト）
- **前提**: 2 ペインに分割、yatamux をフォアグラウンドに
- **操作**: バックグラウンドペインで `printf '\e]9;hello\a'` を実行
- **期待結果**: yatamux 内のトースト通知（右下スライドイン）が表示される

### TC-07: InternalToast 単体 — notify が pending_toasts に追加
- **種別**: ユニットテスト
- **操作**: `InternalToast::new(store.clone()).notify(PaneId(1), "msg".into())`
- **期待結果**: `store.lock().pending_toasts.len() == 1`

### TC-08: NativeToast 単体 — notify が native queue に追加
- **種別**: ユニットテスト
- **操作**: `NativeToast::new(queue.clone()).notify(PaneId(1), "msg".into())`
- **期待結果**: `queue.lock().len() == 1`
