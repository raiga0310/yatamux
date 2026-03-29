## テスト計画: capture-pane JSON 出力 (C-35)

### TC-C35-01: PaneCapture メタデータが正しくシリアライズ/デシリアライズされる
- **前提**: -
- **操作**: `PaneCapture` を JSON にシリアライズして再度デシリアライズする
- **期待結果**: `title`、`cols`、`rows`、`cursor`、`visible_text`、`scrollback_tail` が正確に復元される

### TC-C35-02: PaneContent が capture メタデータ付きで round-trip できる
- **前提**: -
- **操作**: `ServerMessage::PaneContent { pane, content, capture: Some(...) }` を JSON round-trip する
- **期待結果**: `content` と `capture` の両方が保持される

### TC-C35-03: `yatamux capture-pane --json` が CLI で parse できる
- **前提**: -
- **操作**: `yatamux capture-pane --target 1 --lines 20 --json` を clap で parse する
- **期待結果**: `target=1`、`lines=20`、`json=true` で解釈される

### TC-C35-04: lines=0 の CapturePane でも JSON メタデータが返る
- **前提**: yatamux サーバーが起動しており、対象ペインが存在する
- **操作**: `CapturePane { lines: 0 }` を送る
- **期待結果**: `content=""`、`capture.visible_text=[]`、`capture.scrollback_tail=[]`、`capture.cols/rows/cursor` は取得できる

### TC-C35-05: 実在するペインに CapturePane を送ると JSON 用メタデータが返る
- **前提**: PTY に初期出力が出ているペインが存在する
- **操作**: `CapturePane { lines: 100 }` を送る
- **期待結果**: `capture.title`、`capture.cursor`、`capture.visible_text` が返り、`content` は従来どおり非空である

### TC-C35-06: `yatamux capture-pane --json` が整形済み JSON を標準出力に出せる
- **前提**: yatamux GUI が起動済み
- **操作**: `yatamux capture-pane --target <ID> --lines 30 --json` を実行する
- **期待結果**: JSON オブジェクトとして出力され、`content`、`visible_text`、`scrollback_tail`、`cursor` を含む
