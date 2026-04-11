## テスト計画: IPC プロトコル固定化

### TC-01: handshake 成功
- **前提**: IPC 接続直後にクライアントが `protocol_version` と `capabilities` を含む handshake 要求を送れる
- **操作**: サポート対象バージョンと既知 capability を持つ handshake 要求を 1 本の名前付きパイプ接続で送信する
- **期待結果**: サーバーが handshake 成功応答を返し、その後の `ListPanes` など通常要求を同一接続で処理できる

### TC-02: `protocol_version` 不一致
- **前提**: サーバーが受理可能な protocol version 範囲を持つ
- **操作**: 非対応の `protocol_version` を含む handshake 要求を送信する
- **期待結果**: サーバーは接続を黙殺せず、非対応バージョン・受理可能範囲・再試行指針を含む明示エラーを返す

### TC-03: capability 不足
- **前提**: handshake で capability 交渉を行い、要求側と応答側の capability 集合を比較できる
- **操作**: クライアントが必要 capability を宣言し、サーバー側がその一部を持たない状態で handshake または該当 request を送信する
- **期待結果**: サーバーは不足 capability 名を含む失敗応答を返し、未サポート機能だけを拒否するか接続全体を拒否するかの方針が仕様どおりに固定される

### TC-04: error envelope
- **前提**: request / response 系メッセージに `request_id` を持つ error envelope 型が定義されている
- **操作**: `request_id` 付き request を送信し、サーバー側でバリデーションエラーまたは実行エラーを発生させる
- **期待結果**: 応答は `request_id` を保持した失敗形式で返り、成功応答と同じ相関キーで多重化接続上の caller が対象 request の失敗だけを識別できる

### TC-05: 旧クライアントとの後方互換
- **前提**: handshake 未実装の旧クライアント互換モードを残す方針がある
- **操作**: 接続直後に handshake を送らず、現行どおり最初の `ListPanes` または `CapturePane` を送信する
- **期待結果**: サーバーは legacy client として扱うか、許容期間付きの互換モードへ自動遷移し、既存 CLI が handshake なしでも破壊的変更なく動作する
