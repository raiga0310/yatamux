## テスト計画: ペインアラート（C-41）

通知受信時にペインボーダーをアクセントカラーで点滅させる機能のテスト計画。
Ubuntu 上で `cargo test -p yatamux-client` および `cargo test` で実行できるものを中心に列挙する。

---

### 設定（config.rs）

#### TC-C41-01: `[appearance] alert_border` フィールドを読み込める
- **前提**: `config.toml` に `[appearance]\nalert_border = "#FF0000"` を記述
- **操作**: `AppConfig::load()` で読み込む
- **期待結果**: `appearance.alert_border == Some("#FF0000")`

#### TC-C41-02: `alert_border` を省略するとデフォルト（`None`）になる
- **前提**: `config.toml` に `[appearance]` セクションがない、または `alert_border` キーがない
- **操作**: `AppConfig::load()` で読み込む
- **期待結果**: `appearance.alert_border == None`

---

### アラート状態管理（store.rs）

#### TC-C41-10: `trigger_alert` でペインが `alerting_panes` に追加される
- **前提**: `PaneStore::new(PaneId(1), ...)`
- **操作**: `store.trigger_alert(PaneId(2))`
- **期待結果**: `alerting_panes` に `PaneId(2)` が存在し、フリップ数 == `ALERT_FLIP_COUNT`

#### TC-C41-11: 初期状態で `alerting_panes` は空
- **前提**: `PaneStore::new(...)`
- **期待結果**: `store.alerting_panes.is_empty()`

#### TC-C41-12: トリガー直後は `is_alert_on` が `true`
- **前提**: `trigger_alert(PaneId(1))` 後
- **期待結果**: `store.is_alert_on(PaneId(1)) == true`

#### TC-C41-13: 登録されていないペインの `is_alert_on` は `false`
- **前提**: 初期状態
- **期待結果**: `store.is_alert_on(PaneId(99)) == false`

#### TC-C41-14: `tick_alert` を `ALERT_TICK_DIVISOR` 回呼ぶとフリップ数が 1 減る
- **前提**: `trigger_alert(PaneId(1))` 後（count = `ALERT_FLIP_COUNT`）
- **操作**: `tick_alert()` を `ALERT_TICK_DIVISOR` 回呼ぶ
- **期待結果**: `alerting_panes[PaneId(1)] == ALERT_FLIP_COUNT - 1`

#### TC-C41-15: `tick_alert` が `true` を返す間、アラート中のペインが存在する
- **前提**: `trigger_alert(PaneId(1))` 後
- **操作**: `tick_alert()` を 1 回呼ぶ
- **期待結果**: 戻り値 == `true`

#### TC-C41-16: フリップ数が 0 になるとペインが `alerting_panes` から除去される
- **前提**: `trigger_alert(PaneId(1))` 後（count = `ALERT_FLIP_COUNT`）
- **操作**: `tick_alert()` を `ALERT_TICK_DIVISOR * ALERT_FLIP_COUNT` 回呼ぶ
- **期待結果**: `alerting_panes.is_empty() == true`、`is_alert_on(PaneId(1)) == false`

#### TC-C41-17: 全フリップ完了後 `tick_alert` は `false` を返す
- **前提**: `trigger_alert` 後にすべてのフリップを消費
- **操作**: `tick_alert()` を `ALERT_TICK_DIVISOR * ALERT_FLIP_COUNT + 1` 回呼ぶ
- **期待結果**: 最後の `tick_alert()` 戻り値 == `false`

#### TC-C41-18: ON/OFF フェーズが交互に切り替わる
- **前提**: `trigger_alert(PaneId(1))` 後（count = `ALERT_FLIP_COUNT`、偶数）
- **操作**: `tick_alert()` を `ALERT_TICK_DIVISOR` 回呼ぶ（1フリップ進める）
- **期待結果**: 最初は `is_alert_on == true`（偶数カウント）、フリップ後は `false`（奇数カウント）

#### TC-C41-19: `clear_alert` でペインのアラートが即座に解除される
- **前提**: `trigger_alert(PaneId(1))` 後
- **操作**: `store.clear_alert(PaneId(1))`
- **期待結果**: `is_alert_on(PaneId(1)) == false`、`alerting_panes.is_empty() == true`

#### TC-C41-20: 複数ペインを同時にアラートできる
- **前提**: `trigger_alert(PaneId(1))`、`trigger_alert(PaneId(2))`
- **期待結果**: 両方 `is_alert_on == true`

#### TC-C41-21: `clear_alert` は存在しないペインに対してパニックしない
- **前提**: 初期状態
- **操作**: `store.clear_alert(PaneId(99))`
- **期待結果**: パニックなし

#### TC-C41-22: `trigger_alert` を再トリガーするとフリップ数がリセットされる
- **前提**: `trigger_alert(PaneId(1))` → 数回 `tick_alert()`
- **操作**: `trigger_alert(PaneId(1))` を再度呼ぶ
- **期待結果**: `alerting_panes[PaneId(1)] == ALERT_FLIP_COUNT`（リセット済み）

---

### AlertingBackend（notification.rs）

#### TC-C41-30: `AlertingBackend::notify` で `trigger_alert` が呼ばれる
- **前提**: `AlertingBackend::new(store, inner)` を構築
- **操作**: `backend.notify(PaneId(2), "test".to_string())`
- **期待結果**: `store.alerting_panes` に `PaneId(2)` が含まれる

#### TC-C41-31: `AlertingBackend::notify` は内部バックエンドにも委譲する
- **前提**: inner として `InternalToast` を使用
- **操作**: `backend.notify(PaneId(2), "hello".to_string())`
- **期待結果**: `store.pending_toasts.len() == 1`（InternalToast が追加）

#### TC-C41-32: active ペインへの通知でも `trigger_alert` は呼ばれる
- **前提**: `store.active = PaneId(1)`、`AlertingBackend` でラップ
- **操作**: `backend.notify(PaneId(1), "msg".to_string())`
- **期待結果**: `alerting_panes` に `PaneId(1)` が含まれる
  （`notify_if_inactive` は呼ばない側なので実際には発火しないが、
   `AlertingBackend` 自体は active チェックを行わない）

---

### テーマ統合（theme.rs / config.rs / app.rs）

#### TC-C41-40: `Theme` に `alert_border` フィールドがある
- **前提**: `Theme::default()`
- **期待結果**: `theme.alert_border == None`

#### TC-C41-41: `build_theme` が `appearance.alert_border` を `Theme.alert_border` に変換する
- **前提**: `AppearanceConfig { alert_border: Some("#ff6b6b".to_string()), .. }`
- **操作**: `build_theme(&appearance)`
- **期待結果**: `theme.alert_border == Some(0xFF6B6B)`

---

### E2E（`tests/e2e_smoke.rs` — CI の `windows-latest` で自動実行）

#### TC-C41-60: BEL を bg ペインに送ると IPC Notification が届く
- **テスト名**: `e2e_bel_on_background_pane_triggers_notification`
- **前提**: 2 ペイン構成、root がアクティブ、bg がバックグラウンド
- **操作**: `ClientMessage::Input { pane: bg, data: [0x07] }` を送信
- **期待結果**: `ServerMessage::Notification { pane: bg, body: "Bell" }` が 10 秒以内に IPC ストリームに到着する
- **検証内容**: `notify_if_inactive` の bg != active パスが通ること、通知文字列が正確であること

#### TC-C41-61: bg ペインでプロセス終了すると Notification が届く
- **テスト名**: `e2e_process_exit_on_background_pane_triggers_notification`
- **前提**: 2 ペイン構成、root がアクティブ
- **操作**: bg ペインで `exit\r` を送り cmd.exe を終了させる
- **期待結果**: `ServerMessage::Notification { pane: bg, body: "Process exited" }` が 15 秒以内に到着する

#### TC-C41-62: OSC 9 を bg ペインに送ると Notification が届く
- **テスト名**: `e2e_osc9_on_background_pane_triggers_notification`
- **前提**: 2 ペイン構成、root がアクティブ
- **操作**: `\x1b]9;e2e-osc9-test\x07` を bg ペインに送信
- **期待結果**: `ServerMessage::Notification { pane: bg, body: "e2e-osc9-test" }` が 10 秒以内に到着する

> **CI との対応**: E2E ワークフロー（`.github/workflows/e2e.yml`）は `runs-on: windows-latest` で動作しており、
> 上記テストは `cargo test --test e2e_smoke -- --ignored --test-threads=1` で実際の Windows 上で実行される。
> Win32 アラートボーダー描画（TC-C41-50〜54）はヘッドレス環境で検証できないため手動テスト扱いとする。

---

### 描画（Windows 実機テスト）

以下は Win32 環境でのみ実施する手動テスト。Ubuntu での自動化対象外。

#### TC-C41-50（手動）: 非アクティブペインで通知発生時にボーダーが点滅する
- **前提**: 2 ペイン以上の状態でペイン 2 をバックグラウンドにする
- **操作**: ペイン 2 で BEL（`echo -e "\a"`）を発生させる
- **期待結果**: ペイン 2 の枠線が `#FF6B6B`（またはカスタム色）で数秒間点滅し、その後消える

#### TC-C41-51（手動）: アクティブになると点滅が停まる
- **前提**: TC-C41-50 の状態で点滅中
- **操作**: 点滅中のペインをクリックまたはペインモードでフォーカスを移す
- **期待結果**: 次の WM_TIMER ティック（≈16ms）以内に点滅が消える

#### TC-C41-52（手動）: `config.toml` の `alert_border` が反映される
- **前提**: `[appearance]\nalert_border = "#00FF00"` で起動
- **操作**: BEL を発火
- **期待結果**: ペイン枠線が緑（#00FF00）で点滅する

#### TC-C41-53（手動）: `alert_border` 省略時はデフォルト色（#FF6B6B）で点滅する
- **前提**: `config.toml` に `alert_border` なし
- **期待結果**: 赤橙色でボーダーが点滅する

#### TC-C41-54（手動）: OS Action Center 通知が届く
- **前提**: Windows フォーカス状態のまま非アクティブペインで通知発生
- **期待結果**: Windows 通知センターにバルーンが表示される
