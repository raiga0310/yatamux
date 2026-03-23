//! ペイン — PTY セッション + グリッドの組み合わせ
//!
//! 各ペインは 3 つのタスクを所有する（zellij 方式のマルチスレッドアーキテクチャ）:
//!   1. PTY 読み取り（ConPTY output → VT パース → Grid 更新）
//!   2. PTY 書き込み / リサイズ（クライアント入力 → ConPTY input）
//!   3. VT 処理は PTY 読み取りタスク内でインライン実行

use std::sync::Arc;
use tokio::sync::{mpsc, Mutex};
use anyhow::Result;
use tracing::{debug, info};

use yatamux_protocol::types::{PaneId, TermSize};
use yatamux_terminal::{Grid, CjkWidthConfig};

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
    pub title: Arc<Mutex<String>>,
}

impl Pane {
    /// 新しいペインを作成し PTY を起動する
    pub fn spawn(
        id: PaneId,
        size: TermSize,
        width_config: CjkWidthConfig,
        client_output_tx: mpsc::Sender<(PaneId, Arc<[u8]>)>,
    ) -> Result<Self> {
        let grid = Arc::new(Mutex::new(Grid::new(size.cols, size.rows, width_config)));
        let title = Arc::new(Mutex::new(String::new()));

        let (pty_output_tx, mut pty_output_rx) = mpsc::channel::<Vec<u8>>(256);
        let (cmd_tx, mut cmd_rx) = mpsc::channel::<PtyCmd>(64);

        let mut pty = yatamux_terminal::PtySession::spawn(size, None, pty_output_tx)?;

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
                {
                    let mut g = grid_clone.lock().await;
                    let mut proc = yatamux_terminal::vt::VtProcessor::new(&mut g);
                    yatamux_terminal::vt::feed_bytes(&mut parser, &mut proc, &data);

                    if let Some(t) = proc.title.take() {
                        *title_clone.lock().await = t;
                    }
                }

                // クライアントに生データを転送（Arc でラップしてファンアウト時のコピーを回避）
                let data: Arc<[u8]> = Arc::from(data);
                if output_tx_clone.send((id, data)).await.is_err() {
                    break;
                }
            }
            info!("Pane {:?} output task ended", id);
        });

        Ok(Self {
            id,
            grid,
            output_tx: client_output_tx,
            cmd_tx,
            title,
        })
    }

    /// ペインに入力を送信
    pub async fn send_input(&self, data: Vec<u8>) -> Result<()> {
        self.cmd_tx.send(PtyCmd::Input(data)).await
            .map_err(|_| anyhow::anyhow!("Pane {:?} cmd channel closed", self.id))
    }

    /// ペインをリサイズ（グリッドと PTY の両方）
    pub async fn resize(&self, size: TermSize) -> Result<()> {
        self.grid.lock().await.resize(size.cols, size.rows);
        self.cmd_tx.send(PtyCmd::Resize(size)).await
            .map_err(|_| anyhow::anyhow!("Pane {:?} cmd channel closed", self.id))
    }
}
