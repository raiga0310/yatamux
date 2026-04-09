//! 通知バックエンド抽象化
//!
//! yatamux がフォアグラウンドのときは [`InternalToast`]（右下スライドイン）、
//! バックグラウンドのときは [`NativeToast`]（Windows バルーンチップ）を使う。
//! [`FocusAwareBackend`] がフォーカス状態に応じて自動切り替えする。

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use yatamux_protocol::types::PaneId;

use crate::layout::{PaneStore, Toast};

/// 通知を配信する共通インターフェース
pub trait NotificationBackend: Send + Sync {
    fn notify(&self, pane_id: PaneId, message: String);
}

// ── InternalToast ──────────────────────────────────────────────────────────

/// yatamux ウィンドウ内のトースト通知（既存の GDI 描画経路）
pub struct InternalToast {
    store: Arc<Mutex<PaneStore>>,
}

impl InternalToast {
    pub fn new(store: Arc<Mutex<PaneStore>>) -> Self {
        Self { store }
    }
}

impl NotificationBackend for InternalToast {
    fn notify(&self, pane_id: PaneId, message: String) {
        self.store.lock().unwrap().pending_toasts.push_back(Toast {
            pane_id,
            message,
            elapsed_ms: 0,
        });
    }
}

// ── NativeToast ────────────────────────────────────────────────────────────

/// Win32 スレッドが消費するバルーンチップキューエントリ
pub struct NativeToastMsg {
    pub title: String,
    pub body: String,
}

/// Windows バルーンチップ通知（Shell_NotifyIconW 経由）
///
/// `notify()` はキューに積むだけ。Win32 スレッドの `WM_TIMER` で実際に表示する。
pub struct NativeToast {
    queue: Arc<Mutex<VecDeque<NativeToastMsg>>>,
}

impl NativeToast {
    /// キューを共有する NativeToast と、Win32 側が参照する Arc を返す
    pub fn new() -> (Self, Arc<Mutex<VecDeque<NativeToastMsg>>>) {
        let queue = Arc::new(Mutex::new(VecDeque::new()));
        (
            Self {
                queue: Arc::clone(&queue),
            },
            Arc::clone(&queue),
        )
    }
}

impl NotificationBackend for NativeToast {
    fn notify(&self, pane_id: PaneId, message: String) {
        self.queue.lock().unwrap().push_back(NativeToastMsg {
            title: format!("yatamux (pane {})", pane_id.0),
            body: message,
        });
    }
}

// ── AlertingBackend ────────────────────────────────────────────────────────

/// 通知受信時にペインボーダーアラートを起動するラッパー。
///
/// 内部バックエンド（`FocusAwareBackend` 等）に処理を委譲しつつ、
/// `PaneStore::trigger_alert` を必ず呼んでボーダー点滅を開始する。
pub struct AlertingBackend<I: NotificationBackend> {
    store: Arc<Mutex<PaneStore>>,
    inner: I,
}

impl<I: NotificationBackend> AlertingBackend<I> {
    pub fn new(store: Arc<Mutex<PaneStore>>, inner: I) -> Self {
        Self { store, inner }
    }
}

impl<I: NotificationBackend> NotificationBackend for AlertingBackend<I> {
    fn notify(&self, pane_id: PaneId, message: String) {
        // ボーダーアラートを開始（フォーカス状態に関わらず常に）
        self.store.lock().unwrap().trigger_alert(pane_id);
        // 内部バックエンド（トースト / OS 通知）に委譲
        self.inner.notify(pane_id, message);
    }
}

// ── FocusAwareBackend ──────────────────────────────────────────────────────

/// フォーカス状態に応じて InternalToast / NativeToast を切り替えるラッパー
pub struct FocusAwareBackend {
    focused: Arc<AtomicBool>,
    internal: InternalToast,
    native: NativeToast,
}

impl FocusAwareBackend {
    /// `focused` は `WM_ACTIVATEAPP` で更新される Arc<AtomicBool>。
    /// 返り値の Arc はそのまま `run_window` に渡す。
    pub fn new(
        focused: Arc<AtomicBool>,
        store: Arc<Mutex<PaneStore>>,
    ) -> (Self, Arc<Mutex<VecDeque<NativeToastMsg>>>) {
        let internal = InternalToast::new(store);
        let (native, queue) = NativeToast::new();
        (
            Self {
                focused,
                internal,
                native,
            },
            queue,
        )
    }
}

impl NotificationBackend for FocusAwareBackend {
    fn notify(&self, pane_id: PaneId, message: String) {
        if self.focused.load(Ordering::Relaxed) {
            self.internal.notify(pane_id, message);
        } else {
            self.native.notify(pane_id, message);
        }
    }
}

// ── テスト ─────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use yatamux_terminal::Grid;

    fn make_store() -> Arc<Mutex<PaneStore>> {
        let grid = Arc::new(Mutex::new(Grid::new(80, 24, Default::default())));
        Arc::new(Mutex::new(PaneStore::new(PaneId(1), grid)))
    }

    // TC-C41-30: AlertingBackend::notify で trigger_alert が呼ばれる
    #[test]
    fn test_alerting_backend_triggers_alert() {
        let store = make_store();
        let internal = InternalToast::new(Arc::clone(&store));
        let backend = AlertingBackend::new(Arc::clone(&store), internal);
        backend.notify(PaneId(2), "test".to_string());
        let store = store.lock().unwrap();
        assert!(store.alerting_panes.contains_key(&PaneId(2)));
    }

    // TC-C41-31: AlertingBackend::notify は内部バックエンドにも委譲する
    #[test]
    fn test_alerting_backend_delegates_to_inner() {
        let store = make_store();
        let internal = InternalToast::new(Arc::clone(&store));
        let backend = AlertingBackend::new(Arc::clone(&store), internal);
        backend.notify(PaneId(2), "hello".to_string());
        let store = store.lock().unwrap();
        // InternalToast が pending_toasts に追加しているはず
        assert_eq!(store.pending_toasts.len(), 1);
        assert_eq!(store.pending_toasts[0].message, "hello");
    }

    // TC-C41-32: AlertingBackend はアクティブペインへの通知でも trigger_alert を呼ぶ
    #[test]
    fn test_alerting_backend_triggers_alert_for_active_pane() {
        let store = make_store();
        // store.active = PaneId(1)（デフォルト）
        let internal = InternalToast::new(Arc::clone(&store));
        let backend = AlertingBackend::new(Arc::clone(&store), internal);
        backend.notify(PaneId(1), "active pane msg".to_string());
        let store = store.lock().unwrap();
        // AlertingBackend はアクティブチェックを行わず常に trigger_alert を呼ぶ
        assert!(store.alerting_panes.contains_key(&PaneId(1)));
    }

    // TC-07: InternalToast — notify が pending_toasts に追加される
    #[test]
    fn test_internal_toast_pushes_to_store() {
        let store = make_store();
        let backend = InternalToast::new(Arc::clone(&store));
        backend.notify(PaneId(2), "hello".to_string());
        let store = store.lock().unwrap();
        assert_eq!(store.pending_toasts.len(), 1);
        assert_eq!(store.pending_toasts[0].message, "hello");
        assert_eq!(store.pending_toasts[0].pane_id, PaneId(2));
    }

    // TC-08: NativeToast — notify が native queue に追加される
    #[test]
    fn test_native_toast_pushes_to_queue() {
        let (backend, queue) = NativeToast::new();
        backend.notify(PaneId(3), "world".to_string());
        let queue = queue.lock().unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(queue[0].body, "world");
    }

    // TC-01: focused=true → InternalToast にルーティング
    #[test]
    fn test_focus_aware_routes_to_internal_when_focused() {
        let store = make_store();
        let focused = Arc::new(AtomicBool::new(true));
        let (backend, native_queue) =
            FocusAwareBackend::new(Arc::clone(&focused), Arc::clone(&store));
        backend.notify(PaneId(2), "msg".to_string());
        assert_eq!(store.lock().unwrap().pending_toasts.len(), 1);
        assert_eq!(native_queue.lock().unwrap().len(), 0);
    }

    // TC-02: focused=false → NativeToast にルーティング
    #[test]
    fn test_focus_aware_routes_to_native_when_unfocused() {
        let store = make_store();
        let focused = Arc::new(AtomicBool::new(false));
        let (backend, native_queue) =
            FocusAwareBackend::new(Arc::clone(&focused), Arc::clone(&store));
        backend.notify(PaneId(2), "msg".to_string());
        assert_eq!(store.lock().unwrap().pending_toasts.len(), 0);
        assert_eq!(native_queue.lock().unwrap().len(), 1);
    }

    // TC-03: フォーカス状態が動的に切り替わる
    #[test]
    fn test_focus_aware_dynamic_switch() {
        let store = make_store();
        let focused = Arc::new(AtomicBool::new(true));
        let (backend, native_queue) =
            FocusAwareBackend::new(Arc::clone(&focused), Arc::clone(&store));

        backend.notify(PaneId(2), "internal".to_string());
        focused.store(false, Ordering::Relaxed);
        backend.notify(PaneId(2), "native".to_string());

        assert_eq!(store.lock().unwrap().pending_toasts.len(), 1);
        assert_eq!(native_queue.lock().unwrap().len(), 1);
    }
}
