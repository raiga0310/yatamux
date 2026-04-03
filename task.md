# yatamux タスク一覧

運用用のインデックス。
日常的に見る未完了タスクと、履歴として残す完了済みタスクを分離した。

## 参照先

- 進行中の機能・バグ・ドキュメントタスク: `docs/tasks/active.md`
- 完了済みリファクタリング履歴: `docs/tasks/refactor.md`（A-2〜A-8 すべて完了済み）
- タスク履歴アーカイブ: `docs/tasks/archive-2026-03-30.md`, `docs/tasks/archive-2026-04-04.md`

## 直近の優先候補

- `C-30`: 高水準 `exec` API
- `C-31`: ペイン状態メタデータ取得強化
- `C-32`: 出力購読 API
- `C-33`: 明示的な割り込み・キャンセル API
- `C-35`: `capture-pane --json` の詰め（残サブタスク 3 件）
- `C-36`: 待機条件 API の一般化
- `D-1`: `docs/test-plan-*` と実装済み自動テストの同期

## 運用ルール

- 新しい未完了タスクは原則 `docs/tasks/active.md` か `docs/tasks/refactor.md` に追加する
- 完了済みで履歴を残したいものは、定期的にアーカイブへ寄せる
- `task.md` 自体は薄い入口のまま維持し、詳細を抱え込まない
