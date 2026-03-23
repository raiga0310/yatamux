//! ConPTY セッション管理
//!
//! `portable-pty` クレートをラップし、ペインごとの PTY ライフサイクルを管理する。
//! 要件定義書 §5「PTY 管理レイヤー」参照。

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, NativePtySystem, PtySize, PtySystem};
use tokio::sync::mpsc;

use cmux_protocol::types::TermSize;

/// PTY セッションへの制御インターフェース
pub struct PtySession {
    /// 子プロセスへの書き込み側（take_writer() で取得）
    writer: Box<dyn std::io::Write + Send>,
    /// resize 用に master を保持
    master: Box<dyn portable_pty::MasterPty + Send>,
    /// 子プロセス
    child: Box<dyn portable_pty::Child + Send + Sync>,
}

impl PtySession {
    /// 新しい PTY セッションを開始する
    ///
    /// - `size`: 初期ターミナルサイズ
    /// - `cmd`: 起動するシェル/コマンド（None の場合 COMSPEC or cmd.exe）
    /// - `output_tx`: PTY からの出力をここに送る
    pub fn spawn(
        size: TermSize,
        cmd: Option<CommandBuilder>,
        output_tx: mpsc::Sender<Vec<u8>>,
    ) -> Result<Self> {
        let pty_system = NativePtySystem::default();
        let pty_size = PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        };

        let pair = pty_system
            .openpty(pty_size)
            .context("Failed to open PTY")?;

        let cmd = cmd.unwrap_or_else(default_shell);

        let child = pair.slave
            .spawn_command(cmd)
            .context("Failed to spawn child process")?;

        // PTY 読み取りスレッド（ConPTY 出力 → output_tx）
        // Microsoft の推奨：読み書きを別スレッドで処理しデッドロックを防ぐ
        let mut reader = pair.master.try_clone_reader()
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
        let writer = pair.master.take_writer()
            .context("Failed to take PTY writer")?;

        Ok(Self {
            writer,
            master: pair.master,
            child,
        })
    }

    /// PTY に書き込む（キーボード入力 → ConPTY）
    pub fn write(&mut self, data: &[u8]) -> Result<()> {
        use std::io::Write;
        self.writer.write_all(data).context("Failed to write to PTY")
    }

    /// PTY をリサイズする
    pub fn resize(&self, size: TermSize) -> Result<()> {
        self.master.resize(PtySize {
            rows: size.rows,
            cols: size.cols,
            pixel_width: 0,
            pixel_height: 0,
        }).context("Failed to resize PTY")
    }

    /// 子プロセスが終了しているか確認（非ブロッキング）
    pub fn try_wait(&mut self) -> Option<u32> {
        self.child.try_wait().ok()?.map(|s| s.exit_code())
    }
}

/// デフォルトシェルを返す（Windows: COMSPEC, フォールバック: cmd.exe）
fn default_shell() -> CommandBuilder {
    let shell = std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".to_string());
    CommandBuilder::new(shell)
}
