//! ConPTY セッション管理
//!
//! `portable-pty` クレートをラップし、ペインごとの PTY ライフサイクルを管理する。
//! 要件定義書 §5「PTY 管理レイヤー」参照。

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::sync::mpsc;

use yatamux_protocol::types::TermSize;

/// PTY セッションへの制御インターフェース
pub struct PtySession {
    /// 子プロセスへの書き込み側（take_writer() で取得）
    writer: Box<dyn std::io::Write + Send>,
    /// resize 用に master を保持
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// 子プロセス（take_child() で取り出し可能）
    child: Option<Box<dyn portable_pty::Child + Send + Sync>>,
}

impl PtySession {
    /// 新しい PTY セッションを開始する
    ///
    /// - `size`: 初期ターミナルサイズ
    /// - `cmd`: 起動するシェル/コマンド（None の場合 COMSPEC or cmd.exe）
    /// - `output_tx`: PTY からの出力をここに送る
    /// - `working_dir`: 作業ディレクトリ（None の場合はプロセスの CWD を引き継ぐ）
    pub fn spawn(
        size: TermSize,
        cmd: Option<CommandBuilder>,
        output_tx: mpsc::Sender<Vec<u8>>,
        working_dir: Option<String>,
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pty_size = PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system.openpty(pty_size).context("Failed to open PTY")?;

        let mut cmd = cmd.unwrap_or_else(default_shell);
        if let Some(ref dir) = working_dir {
            if !std::path::Path::new(dir).is_dir() {
                return Err(anyhow::anyhow!("working directory does not exist: {}", dir));
            }
            cmd.cwd(dir);
        }

        let child = pair
            .slave
            .spawn_command(cmd)
            .context("Failed to spawn child process")?;

        // PTY 読み取りスレッド（ConPTY 出力 → output_tx）
        // Microsoft の推奨：読み書きを別スレッドで処理しデッドロックを防ぐ
        let mut reader = pair
            .master
            .try_clone_reader()
            .context("Failed to clone PTY reader")?;

        tokio::task::spawn_blocking(move || {
            let mut buf = vec![0u8; 4096];
            loop {
                match std::io::Read::read(&mut reader, &mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        let data = buf[..n].to_vec();
                        if output_tx.blocking_send(data).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        // take_writer() で書き込み側を取得
        let writer = pair
            .master
            .take_writer()
            .context("Failed to take PTY writer")?;

        Ok(Self {
            writer,
            master: pair.master,
            child: Some(child),
        })
    }

    /// 子プロセスハンドルを取り出す（`spawn_blocking` で `wait()` するために使用）
    ///
    /// Windows の ConPTY では PTY master が子プロセス終了後も open のままになるため、
    /// `reader.read()` が EOF を返さないことがある。子プロセスの終了検知には
    /// このメソッドで取得したハンドルに対して `wait()` を呼ぶこと。
    pub fn take_child(&mut self) -> Option<Box<dyn portable_pty::Child + Send + Sync>> {
        self.child.take()
    }

    /// 子プロセスの kill ハンドルを複製して返す。
    ///
    /// `take_child()` の前に呼ぶこと。`Pane::Drop` で子プロセスを終了させるために使用する。
    /// `take_child()` 後は `child` が `None` になるため `None` を返す。
    pub fn clone_child_killer(&self) -> Option<Box<dyn portable_pty::ChildKiller + Send + Sync>> {
        self.child.as_ref().map(|c| c.clone_killer())
    }

    /// PTY に書き込む（キーボード入力 → ConPTY）
    ///
    /// write_all の後に flush することで、Ctrl+C (`\x03`) などの制御文字が
    /// バッファに残らず ConPTY に即座に届くようにする。
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        use std::io::Write;
        self.writer
            .write_all(data)
            .context("Failed to write to PTY")?;
        self.writer.flush().context("Failed to flush PTY")
    }

    /// PTY をリサイズする
    pub fn resize(&self, size: TermSize) -> Result<()> {
        self.master
            .resize(PtySize {
                rows: size.rows,
                cols: size.cols,
                pixel_width: 0,
                pixel_height: 0,
            })
            .context("Failed to resize PTY")
    }

    /// 子プロセスが終了しているか確認（非ブロッキング）
    pub fn try_wait(&mut self) -> Option<u32> {
        self.child.as_mut()?.try_wait().ok()?.map(|s| s.exit_code())
    }
}

/// デフォルトシェルを返す（Windows: COMSPEC, フォールバック: cmd.exe）
fn default_shell() -> CommandBuilder {
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    CommandBuilder::new(shell)
}
