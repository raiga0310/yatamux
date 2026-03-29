//! ペイン — PTY セッション + グリッドの組み合わせ
//!
//! 各ペインは 3 つのタスクを所有する（zellij 方式のマルチスレッドアーキテクチャ）:
//!   1. PTY 読み取り（ConPTY output → VT パース → Grid 更新）
//!   2. PTY 書き込み / リサイズ（クライアント入力 → ConPTY input）
//!   3. VT 処理は PTY 読み取りタスク内でインライン実行

use anyhow::Result;
use portable_pty::ChildKiller;
use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, info};

use yatamux_protocol::types::{PaneId, TermSize};
use yatamux_terminal::{CjkWidthConfig, Grid};

// ListPanes でのデッドロックを避けるため、サイズとタイトルは std::sync::Mutex で保持する。
// tokio::sync::Mutex の .lock().await は pane_output_rx が詰まると
// handle_client_message 内でデッドロックを起こす可能性がある。

/// ペイン書き込みタスクへの制御コマンド
enum PtyCmd {
    Input(Vec<u8>),
    Resize(TermSize),
}

/// ペインの状態
pub struct Pane {
    pub id: PaneId,
    pub grid: Arc<Mutex<Grid>>,
    /// クライアントへの出力転送チャネル
    pub output_tx: mpsc::Sender<(PaneId, Arc<[u8]>)>,
    /// PTY コマンドチャネル（入力 / リサイズ）
    cmd_tx: mpsc::Sender<PtyCmd>,
    /// タイトル文字列（std::sync::Mutex: ListPanes でブロッキングなし取得のため）
    pub title: Arc<std::sync::Mutex<String>>,
    /// 現在のペインサイズ（std::sync::Mutex: ListPanes でブロッキングなし取得のため）
    pub size: Arc<std::sync::Mutex<TermSize>>,
    /// 子プロセス kill ハンドル。Pane が Drop されるときに cmd.exe を終了させる。
    /// 孤児プロセス（テスト後の残留 cmd.exe）を防ぐために保持する。
    child_killer: Option<Box<dyn ChildKiller + Send + Sync>>,
}

impl Drop for Pane {
    fn drop(&mut self) {
        if let Some(mut killer) = self.child_killer.take() {
            let _ = killer.kill();
        }
    }
}

impl Pane {
    /// 新しいペインを作成し PTY を起動する
    pub fn spawn(
        id: PaneId,
        size: TermSize,
        width_config: CjkWidthConfig,
        client_output_tx: mpsc::Sender<(PaneId, Arc<[u8]>)>,
        client_notification_tx: mpsc::Sender<(PaneId, String)>,
        working_dir: Option<String>,
    ) -> Result<Self> {
        let grid = Arc::new(Mutex::new(Grid::new(size.cols, size.rows, width_config)));
        let title = Arc::new(std::sync::Mutex::new(String::new()));
        let pane_size = Arc::new(std::sync::Mutex::new(size));

        let (pty_output_tx, mut pty_output_rx) = mpsc::channel::<Vec<u8>>(256);
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<PtyCmd>(64);

        let mut pty = yatamux_terminal::PtySession::spawn(size, None, pty_output_tx, working_dir)?;

        // Drop 時に子プロセスを kill するためのハンドルを先に取得する。
        // take_child() の後は child が None になるため、必ず前に呼ぶこと。
        let child_killer = pty.clone_child_killer();

        // 子プロセス終了監視タスク（C-9）
        //
        // Windows の ConPTY では子プロセスが exit しても PTY master の reader が
        // EOF を返さないため、出力読み取りタスクのループ終了には頼れない。
        // child.wait()（WaitForSingleObject）で直接プロセス終了を検知する。
        if let Some(mut child) = pty.take_child() {
            let exit_tx = client_notification_tx.clone();
            tokio::task::spawn_blocking(move || {
                let _ = child.wait();
                let _ = exit_tx.blocking_send((id, "Process exited".to_string()));
            });
        }

        // PTY 書き込み / リサイズタスク
        tokio::spawn(async move {
            while let Some(cmd) = cmd_rx.recv().await {
                match cmd {
                    PtyCmd::Input(data) => {
                        if let Err(e) = pty.write(&data) {
                            debug!("PTY write error for pane {:?}: {}", id, e);
                            break;
                        }
                    }
                    PtyCmd::Resize(new_size) => {
                        if let Err(e) = pty.resize(new_size) {
                            debug!("PTY resize error for pane {:?}: {}", id, e);
                        }
                    }
                }
            }
            info!("Pane {:?} pty task ended", id);
        });

        // PTY 読み取り → VT パース → クライアント転送タスク
        let output_tx_clone = client_output_tx.clone();
        let grid_clone = Arc::clone(&grid);
        let title_clone = Arc::clone(&title);

        tokio::spawn(async move {
            let mut parser = vte::Parser::new();

            while let Some(data) = pty_output_rx.recv().await {
                // Grid 更新
                let (notif, cmd_finished, bell) = {
                    let mut g = grid_clone.lock().await;
                    let mut proc = yatamux_terminal::vt::VtProcessor::new(&mut g);
                    yatamux_terminal::vt::feed_bytes(&mut parser, &mut proc, &data);

                    if let Some(t) = proc.title.take() {
                        *title_clone.lock().unwrap() = t;
                    }
                    (
                        proc.notification.take(),
                        proc.command_finished.take(),
                        proc.bell,
                    )
                };

                // OSC 9/99/777 通知を転送
                if let Some(body) = notif {
                    let _ = client_notification_tx.send((id, body)).await;
                }
                // OSC 133;D コマンド終了通知を転送（exit_code を付加して session.rs で CommandFinished に変換）
                if let Some(exit_code) = cmd_finished {
                    let body = match exit_code {
                        Some(code) => format!("__cmd_finished__:{}", code),
                        None => "__cmd_finished__:".to_string(),
                    };
                    let _ = client_notification_tx.send((id, body)).await;
                }
                // BEL（\x07）通知を転送
                if bell {
                    let _ = client_notification_tx.send((id, "Bell".to_string())).await;
                }

                // クライアントに生データを転送（Arc でラップしてファンアウト時のコピーを回避）
                let data: Arc<[u8]> = Arc::from(data);
                if output_tx_clone.send((id, data)).await.is_err() {
                    break;
                }
            }
            // 出力タスク終了（通知は child watcher タスクが担当）
            info!("Pane {:?} output task ended", id);
        });

        Ok(Self {
            id,
            grid,
            output_tx: client_output_tx,
            cmd_tx,
            title,
            size: pane_size,
            child_killer,
        })
    }

    /// ペインに入力を送信
    pub async fn send_input(&self, data: Vec<u8>) -> Result<()> {
        self.cmd_tx
            .send(PtyCmd::Input(data))
            .await
            .map_err(|_| anyhow::anyhow!("Pane {:?} cmd channel closed", self.id))
    }

    /// ペインをリサイズ（グリッドと PTY の両方）
    pub async fn resize(&self, size: TermSize) -> Result<()> {
        *self.size.lock().unwrap() = size;
        self.grid.lock().await.resize(size.cols, size.rows);
        self.cmd_tx
            .send(PtyCmd::Resize(size))
            .await
            .map_err(|_| anyhow::anyhow!("Pane {:?} cmd channel closed", self.id))
    }
}
