# 設計メモ: clone 監査と所有権ライフサイクル整理 (A-2)

## 目的

`clone()` を一律に悪者扱いせず、次の 3 種類に分類して扱う。

1. **cheap handle clone**
   `Arc<T>` / `mpsc::Sender<T>` のような共有ハンドル複製。Rust 的に自然で、通常は問題ではない。
2. **snapshot clone**
   lock を早く解放する、描画時に安定スナップショットを持つ、などの目的で状態を複製するもの。
   必要性はあるが、clone 対象が大きいと再設計余地がある。
3. **accidental clone / stringly clone**
   API 形状やイベント表現の都合で余計に複製されているもの。
   Rust 的には move / borrow / enum 化 / `Arc<str>` 化で改善しやすい。

このメモでは、`clone` を含む「Rust-tic ではない記述」を、
**改善可能か** と **実装難易度（影響範囲）** で分類する。

## 難易度区分

- **Low**
  単一ファイルか、局所 API の修正で閉じる。既存テストの更新も限定的。
- **Medium**
  2〜4 ファイルにまたがり、状態所有や関数シグネチャの見直しが必要。
- **High**
  コンポーネント境界をまたぐ。内部イベント型や共有状態の責務分割まで触る。

## 先に結論

- 今すぐ直す価値が高いのは、**文字列 clone の事故** と **`LayoutNode` 部分木 clone**。
- 根本的に Rust らしくしたいなら、**stringly typed な内部通知** と
  **`PaneStore` を丸ごと lock して snapshot clone する描画フロー** を見直す必要がある。
- `Arc` / `mpsc::Sender` の clone 自体は問題ではない。そこを削っても設計改善にはほぼならない。

## 分類表

| 分類 | 箇所 | 現状 | 改善可能性 | 難易度 | 影響範囲 | コメント |
|---|---|---|---|---|---|---|
| cheap handle clone | `src/app.rs` fan-out の `server_tx.send(msg.clone())` | `ServerMessage` を GUI と IPC に二重配送するため clone | あり | High | `app` / `protocol` / IPC 配線 | `Arc<ServerMessage>` 化や broadcast 化で clone を消せるが、境界変更の割に今すぐの価値は中程度 |
| accidental clone | `src/app.rs` `HooksConfig::is_enabled(&Some(cmd.clone()))` | 判定のためだけに `String` を複製 | あり | Low | `app.rs` | `as_deref()` ベースに変えれば clone 不要 |
| accidental clone | `src/app.rs` hook 実行時の `let cmd = cmd.clone()` | `tokio::spawn` に入れるため所有化 | あり | Low | `app.rs` | 最終的な所有化 1 回は必要。判定用 clone を消すだけで十分 |
| accidental clone | `src/app.rs` `queue[0].clone()` | 次の `direction` 参照だけにタプル全体 clone | あり | Low | `app.rs` | `queue.front().map(...)` で十分 |
| snapshot clone | `src/app.rs` layout 適用時の `p.command.clone()` / `cmd.clone().into_bytes()` | 設定文字列を queue と `pane_commands` と PTY 入力へ複製 | あり | Medium | `app.rs` / `layout.rs` / 保存復元系 | `Arc<str>` や move 中心の queue 構造にすると減らせる |
| snapshot clone | `crates/client/src/window.rs` `store.copy_mode.clone()` | 描画中に lock を持ち続けないため状態を複製 | 条件付きであり | Medium | `window.rs` / `layout.rs` | `CopyState` は小さめ。優先度は中 |
| snapshot clone | `crates/client/src/window.rs` `store.launcher.clone()` | ランチャー全体を描画前に複製 | あり | Medium | `window.rs` / `layout.rs` | `entries` と preview を丸ごと複製しており、件数が増えると効く |
| snapshot clone | `crates/client/src/window.rs` `store.theme_launcher.clone()` | テーマ一覧全体を描画前に複製 | あり | Medium | `window.rs` / `layout.rs` | 読み取り用 view struct に切り出せる |
| snapshot clone | `crates/client/src/window.rs` `store.save_prompt.clone()` | 入力中の名前を描画用に複製 | あり | Low | `window.rs` | 文字列のみで軽量。改善余地はあるが効果は小さい |
| snapshot clone | `crates/client/src/window.rs` `store.layout.clone()` | 保存プロンプト描画でレイアウト木全体を複製 | あり | High | `window.rs` / `layout.rs` / 描画フロー | 毎フレームの大きな clone。view model 導入対象として最優先 |
| accidental clone | `crates/client/src/window.rs` `selected_name().map(str::to_owned)` | UI 適用のための一時所有化 | 限定的にあり | Low | `window.rs` | 1 回の選択確定時のみ。優先度は低い |
| accidental clone | `crates/client/src/layout.rs` `*self = (**second).clone()` / `(**first).clone()` | 部分木削除で subtree 全体を clone | あり | Medium | `layout.rs` | `mem::replace` / `Option::take` ベースに置換しやすい |
| acceptable clone | `crates/client/src/layout.rs` preview 構築時の `p.command.clone()` | TOML パース後に preview 用所有データを保持 | ありだが価値低 | Low | `layout.rs` | preview は永続参照できないので所有化自体は妥当 |
| accidental clone | `crates/server/src/session.rs` `body: body.clone()` | Notification 送信後も `"Process exited"` 判定に使うため複製 | あり | Low | `session.rs` | `let should_close = body == ...;` を先に評価すれば clone 不要 |
| accidental clone | `crates/server/src/session.rs` `name.clone()` | `Workspace` 保存と `WorkspaceCreated` 返信に同じ名前を使う | 限定的にあり | Low | `session.rs` | move と `Arc<str>` で減るが実利は薄い |
| snapshot clone | `crates/server/src/session.rs` `title.lock().unwrap().clone()` | IPC 応答に owned title が必要 | 改善余地小 | Low | `session.rs` / `pane.rs` | lock 境界をまたぐので現状妥当。`Arc<str>` 化は可能だが優先度低い |
| cheap handle clone | `crates/server/src/session.rs` `self.width_config.clone()` / Sender clone | `Pane::spawn` に必要な所有権配布 | 低い | Low | `session.rs` | 問題なし |
| stringly typed event | `crates/server/src/pane.rs` `"Process exited"` / `"Bell"` / `"__cmd_finished__"` | 内部通知チャネルが `String` ベース | 対応済み | High | `pane.rs` / `session.rs` / protocol | `PaneEvent` enum 化で内部文字列組み立て/解析を廃止 |
| accidental state | `crates/server/src/pane.rs` `pub output_tx` | struct フィールドとして保持しているが実使用は spawn 内 clone のみ | あり | Low | `pane.rs` | 不要フィールドなら削除できる |

## 個別所見

### 1. cheap handle clone は削減対象ではない

`Arc::clone(&grid)` や `mpsc::Sender::clone()` は、共有所有権やタスク配線の明示として自然。
たとえば [src/app.rs](C:/Users/raiga/dev/cmux-win/src/app.rs) の `merged_tx.clone()` や
[crates/server/src/session.rs](C:/Users/raiga/dev/cmux-win/crates/server/src/session.rs) の
`self.pane_output_tx.clone()` は、所有権の分配として妥当。

改善対象にすべきなのは、**clone のコスト** より **clone が責務境界の曖昧さを隠していないか**。

### 2. `PaneStore` の coarse lock と描画 snapshot が大きな設計論点

[crates/client/src/window.rs](C:/Users/raiga/dev/cmux-win/crates/client/src/window.rs) では、
描画中に `PaneStore` lock を長時間保持しないために状態 clone を行っている。
方針自体は正しいが、`launcher` や `layout` まで丸ごと clone しているため、
「どの状態を frame-local snapshot にするべきか」が未整理。

特に `paint_save_prompt()` での `store.layout.clone()` は、
**レイアウト木全体を毎フレーム複製** しているため、A-2 の本命候補。

改善案:

- 描画専用の `RenderSnapshot` / `PromptView` / `LauncherView` を導入する
- `PaneStore` から必要な最小データだけ抽出する
- `layout` は clone ではなく、プレビュー描画に必要な情報だけ flatten して持ち出す

### 3. `LayoutNode::remove_pane()` は Rust 的にまだ改善しやすい

[crates/client/src/layout.rs](C:/Users/raiga/dev/cmux-win/crates/client/src/layout.rs) の
`remove_pane()` は subtree を `Clone` 前提で差し替えている。

これはロジック自体は分かりやすいが、所有権モデルとしては
`Box<LayoutNode>` を **move** で引き剥がせる場面で clone を選んでいる。

改善案:

- `std::mem::replace(self, LayoutNode::Leaf(dummy))`
- 一時的に `first` / `second` を take できる補助 enum / helper
- 再帰削除を `fn remove(self, id) -> (LayoutNode, Option<PaneId>)` 形式に寄せる

局所的な変更で済むため、実装順としては比較的着手しやすい。

### 4. 内部通知が stringly typed なのは Rust 的に最も弱い

[crates/server/src/pane.rs](C:/Users/raiga/dev/cmux-win/crates/server/src/pane.rs) と
[crates/server/src/session.rs](C:/Users/raiga/dev/cmux-win/crates/server/src/session.rs) の間は、
以前は `mpsc<(PaneId, String)>` で `"Process exited"` や `"__cmd_finished__:42"` をやり取りしていた。

これは clone 問題よりも大きく、以下を同時に招く。

- 文字列生成・解析の余計な割り当て
- typo や意味衝突の余地
- 「内部イベント」と「ユーザー向け通知メッセージ」が分離されていない

今回の改善:

```rust
enum PaneEvent {
    Notification(String),
    Bell,
    ProcessExited,
    CommandFinished(Option<i32>),
}
```

この変更で、PTy タスクと server event loop の境界が型安全になり、
内部の制御フローが文字列組み立て/解析に依存しなくなった。

### 5. 小さい clone は直せるが、優先度を上げすぎない

たとえば [src/app.rs](C:/Users/raiga/dev/cmux-win/src/app.rs) の
`queue[0].clone()`、`HooksConfig::is_enabled(&Some(cmd.clone()))`、
[crates/server/src/session.rs](C:/Users/raiga/dev/cmux-win/crates/server/src/session.rs) の
`body.clone()` は、いずれも簡単に直せる。

ただし、これらだけを直しても A-2 の本質である
「状態の owner / borrower / drop point の整理」には届かない。
**小手先の clone 削減だけで終わらせないこと** が重要。

## 優先順位案

1. **Low / 効果高**
   `HooksConfig::is_enabled(&Some(cmd.clone()))`、`queue[0].clone()`、`body.clone()` を整理
2. **Medium / 効果高**
   `LayoutNode::remove_pane()` を move ベースへ変更
3. **High / 効果高**
   `PaneStore` 描画 snapshot の view model 化
4. **High / 根本改善**
   `pane.rs` ↔ `session.rs` の内部通知を enum 化

## 今回の結論

- **改善しやすいもの**
  小さな `String` clone、`queue[0].clone()`、`LayoutNode` の subtree clone
- **改善できるが設計変更が必要なもの**
  fan-out の `ServerMessage` clone、layout command の多重所有、描画用 snapshot clone
- **Rust 的に最も弱く、再設計価値が高いもの**
  以前の `String` ベース内部通知チャネル。これは `PaneEvent` enum 化で改善済み
