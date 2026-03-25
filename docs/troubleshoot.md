# トラブルシュート集

このプロジェクトで実際に発生したバグ・デッドロック・テスト失敗のパターンと解決策。
新機能追加・バグ修正の前にざっと確認しておくこと。

---

## T-01: tokio::sync::Mutex をメッセージハンドラ内で `.lock().await` するとデッドロックする

### 症状
- `ListPanes` などのメッセージ処理中にテストや本番が無応答になる
- `test_list_panes_returns_all_panes` が 60 秒以上ハングする

### 原因
`handle_client_message` は `Server::run()` の `select!` ループ内で呼ばれる。
この関数内で `tokio::sync::Mutex::lock().await` すると、`await` 点で tokio が他のタスクを実行しようとする。
しかし PTY 読み取りタスクが `pane_output_tx` に書き込んで `select!` に戻るのを待っているため、
`pane_output_rx` が詰まり → PTY タスクがブロック → Grid ロックが手放されない → `ListPanes` が永久待ち、
という循環デッドロックが成立する。

```
handle_client_message (lock().await)
  └─ select! ループが止まる
       └─ pane_output_rx が drain されない
            └─ PTY read task の send がブロック
                 └─ grid ロックが手放されない  ← デッドロック
```

### 解決策
`Pane` の `title` / `size` など、メッセージハンドラ内から参照するフィールドは
**`std::sync::Mutex`** を使う。`.lock().unwrap()` は `await` を伴わないため循環しない。

```rust
// NG: tokio::sync::Mutex
let title = pane.title.lock().await.clone();

// OK: std::sync::Mutex
let title = pane.title.lock().unwrap().clone();
```

### 横展開チェックリスト
- `handle_client_message` 内（または呼び出しチェーン上）で新しく Mutex を触る場合
  → `tokio::sync::Mutex` ではなく `std::sync::Mutex` を検討する
- `select!` ループの中で長い `await` チェーンを増やした場合
  → チャネル詰まりによるスタベーションが起きないか確認する

---

## T-02: テストでハングする（タイムアウトなし）

### 症状
- `cargo test` が特定のテストで無応答のまま終わらない
- CI が無限にブロックする

### 原因
- デッドロック（T-01 参照）
- `recv()` が永久待ちになる（送信側チャネルが先に drop されていない等）
- 孤児プロセスがリソースを消費し、後続テストがリソース不足でハングする（T-03 参照）

### 解決策
すべての `#[tokio::test]` は `with_timeout` ラッパーで 120 秒 hard-fail にする。

```rust
async fn with_timeout<F: std::future::Future<Output = ()>>(test_fn: F) {
    tokio::time::timeout(Duration::from_secs(120), test_fn)
        .await
        .expect("test timed out after 120s — likely deadlock or resource exhaustion")
}

#[tokio::test]
async fn test_something() {
    with_timeout(async {
        // テスト本体
    })
    .await
}
```

制御メッセージを待つ `recv` ループには個別に 60 秒タイムアウトを入れる。
`Output` / `Notification` / `PaneClosed` などノイズメッセージをスキップしつつ制御応答を拾う
`recv_ctrl` ヘルパーを活用すること（`session.rs` 参照）。

---

## T-03: テスト後に cmd.exe が残留してリソースを食い尽くす

### 症状
- テストを繰り返すと OS のプロセス数が増え続ける
- 後半のテストが遅くなる・失敗する

### 原因
`Pane` が Drop されるとき、内部で起動した cmd.exe を kill しないと孤児プロセスになる。
tokio ランタイムが落ちても子プロセスは生き続ける。

### 解決策
`Pane` に `Drop` impl を追加し、`ChildKiller` で子プロセスを終了させる。

```rust
impl Drop for Pane {
    fn drop(&mut self) {
        if let Some(mut killer) = self.child_killer.take() {
            let _ = killer.kill();
        }
    }
}
```

`child_killer` は `PtySession::clone_child_killer()` で `take_child()` の**前**に取得する。
`take_child()` 後は `child` が `None` になり `clone_killer()` が呼べなくなる。

### 横展開チェックリスト
- 新たに子プロセスを起動するリソースを追加した場合 → `Drop` impl で必ず cleanup する
- `spawn_blocking` で `wait()` を呼ぶタスクがある場合 → `Pane` が先に Drop されると
  `killer.kill()` 後に `wait()` が即座に戻るので問題ない

---

## T-04: Windows で CLI 出力が文字化けする（Shift-JIS / CP932）

### 症状
- `capture-pane` 等の CLI 出力で日本語が `逶ｴ謗･` のように化ける
- `println!` の UTF-8 文字列が Shift-JIS として解釈される

### 原因
Windows のデフォルト ANSI コードページは CP932（Shift-JIS）。
`SetConsoleOutputCP(65001)` はコンソール表示層のみ変更し、
プロセス全体の ANSI CP は変わらない。

### 解決策
アプリケーションマニフェストに `activeCodePage = UTF-8` を設定する。
これによりプロセス全体の ANSI CP が UTF-8 になる（Windows 10 1903 以降）。

`manifest.xml`（プロジェクトルート）:
```xml
<assembly ...>
  <application>
    <windowsSettings>
      <activeCodePage xmlns="...">UTF-8</activeCodePage>
    </windowsSettings>
  </application>
</assembly>
```

`build.rs`（プロジェクトルート）でリンク時に埋め込む:
```rust
println!("cargo:rustc-link-arg=/MANIFEST:EMBED");
println!("cargo:rustc-link-arg=/MANIFESTINPUT:{manifest_dir}/manifest.xml");
```

詳細は `manifest.xml` と `build.rs` を参照。

---

## T-05: select! でスタベーションが起きる

### 症状
- `CreatePane` などのメッセージが長時間処理されない
- PTY 出力が大量に来るとクライアントからのコマンドが遅延する

### 原因
tokio の `select!` はデフォルトで擬似ランダムに branch を選ぶが、
`pane_output_rx` が常に ready だと実質的に `client_rx` が飢える。

### 解決策
現状の実装（`Server::run()`）は許容範囲として受け入れている。
もし深刻になった場合は `biased;` を**外し**、出力チャネルにバックプレッシャーを追加するか、
専用の出力転送タスクに切り出す（zellij 方式）。
