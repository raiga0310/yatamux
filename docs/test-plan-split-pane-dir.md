## テスト計画: split-pane --dir 作業ディレクトリ指定 (C-14)

### TC-C14-01: CreatePane { working_dir: Some(...) } がシリアライズ/デシリアライズされる
- **前提**: -
- **操作**: `working_dir: Some("C:/Users/test")` を含む CreatePane を JSON シリアライズして再デシリアライズ
- **期待結果**: フィールドが正確に復元される

### TC-C14-02: CreatePane { working_dir: None } が後方互換性を保つ
- **前提**: -
- **操作**: 旧フォーマット（working_dir フィールドなし）の JSON を CreatePane にデシリアライズ
- **期待結果**: working_dir が None としてデシリアライズされる

### TC-C14-03: split-pane --dir で指定したディレクトリで PTY が起動する（手動確認）
- **前提**: yatamux GUI が起動済み
- **操作**: `yatamux split-pane --dir C:/tmp` を実行
- **期待結果**: C:/tmp を作業ディレクトリとした新規ペインが作成される

### TC-C14-04: split-pane --dir なしでも既存動作が変わらない（手動確認）
- **前提**: yatamux GUI が起動済み
- **操作**: `yatamux split-pane` を引数なしで実行
- **期待結果**: 通常の Vertical 分割でペインが作成される

### TC-C14-05: split-pane --direction horizontal で水平分割になる（手動確認）
- **前提**: yatamux GUI が起動済み
- **操作**: `yatamux split-pane --direction horizontal` を実行
- **期待結果**: 水平分割でペインが作成される
