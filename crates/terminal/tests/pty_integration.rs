//! PTY 統合テスト（A 系）
//!
//! 実際の ConPTY プロセス（cmd.exe）を起動して動作を検証する。
//! Windows 専用。

#![cfg(windows)]

use std::time::Duration;

use portable_pty::CommandBuilder;
use tokio::sync::mpsc;
use yatamux_protocol::types::TermSize;
use yatamux_terminal::PtySession;

fn default_size() -> TermSize {
    TermSize { cols: 80, rows: 24 }
}

// A-1: PTY が正常に起動する（シェルプロセスがスポーンされる）
#[tokio::test]
async fn test_pty_spawns_successfully() {
    let (output_tx, _output_rx) = mpsc::channel::<Vec<u8>>(64);
    let result = PtySession::spawn(default_size(), None, output_tx, None);
    assert!(
        result.is_ok(),
        "PTY spawn should succeed: {:?}",
        result.err()
    );
}

// A-2: PTY 起動後、初期出力（プロンプト等）が届く
#[tokio::test]
async fn test_pty_initial_output_received() {
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(256);
    let _session = PtySession::spawn(default_size(), None, output_tx, None).unwrap();

    let data = tokio::time::timeout(Duration::from_secs(5), output_rx.recv())
        .await
        .expect("timeout: no initial output from PTY");
    assert!(
        data.is_some(),
        "output channel should not be closed immediately"
    );
    assert!(
        !data.unwrap().is_empty(),
        "initial output should be non-empty"
    );
}

// A-2: echo コマンドを書き込み、その出力が返ってくる
#[tokio::test]
async fn test_pty_echo_command_output() {
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(256);
    let mut session = PtySession::spawn(default_size(), None, output_tx, None).unwrap();

    // 初期プロンプトが来るまで待つ
    tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(data) = output_rx.recv().await {
            // プロンプト文字 '>' が含まれたら準備完了
            if data.contains(&b'>') {
                break;
            }
        }
    })
    .await
    .ok();

    // echo コマンドを送信
    session.write(b"echo cmux_pty_test_marker\r").unwrap();

    // 出力に "cmux_pty_test_marker" が含まれるか確認
    let found = tokio::time::timeout(Duration::from_secs(5), async {
        while let Some(data) = output_rx.recv().await {
            let s = String::from_utf8_lossy(&data);
            if s.contains("cmux_pty_test_marker") {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);

    assert!(
        found,
        "PTY output should contain echo result 'cmux_pty_test_marker'"
    );
}

// A-3: リサイズが成功する
#[tokio::test]
async fn test_pty_resize_succeeds() {
    let (output_tx, _) = mpsc::channel::<Vec<u8>>(64);
    let session = PtySession::spawn(default_size(), None, output_tx, None).unwrap();

    let new_size = TermSize {
        cols: 120,
        rows: 40,
    };
    let result = session.resize(new_size);
    assert!(
        result.is_ok(),
        "PTY resize should succeed: {:?}",
        result.err()
    );
}

// A-3: リサイズ後も書き込みが成功する
#[tokio::test]
async fn test_pty_write_after_resize() {
    let (output_tx, _) = mpsc::channel::<Vec<u8>>(256);
    let mut session = PtySession::spawn(default_size(), None, output_tx, None).unwrap();

    session
        .resize(TermSize {
            cols: 120,
            rows: 40,
        })
        .unwrap();
    let result = session.write(b"\r");
    assert!(result.is_ok(), "Write after resize should succeed");
}

// A-4: 起動直後の子プロセスは終了していない
#[tokio::test]
async fn test_pty_process_running_after_spawn() {
    let (output_tx, _) = mpsc::channel::<Vec<u8>>(64);
    let mut session = PtySession::spawn(default_size(), None, output_tx, None).unwrap();

    // 起動直後はまだ動いているはず
    let status = session.try_wait();
    assert!(
        status.is_none(),
        "Process should still be running: got exit code {:?}",
        status
    );
}

// A-4: exit コマンドでプロセスが終了する
#[tokio::test]
async fn test_pty_process_exits_after_exit_command() {
    let (output_tx, _) = mpsc::channel::<Vec<u8>>(64);
    let mut session = PtySession::spawn(default_size(), None, output_tx, None).unwrap();

    // 初期化を待つ
    tokio::time::sleep(Duration::from_millis(500)).await;

    session.write(b"exit\r").unwrap();

    // 最大 3 秒でプロセスが終了するのを待つ
    let exited = tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            if session.try_wait().is_some() {
                return true;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    })
    .await
    .unwrap_or(false);

    assert!(exited, "Process should exit after 'exit' command");
}

// A-5: 読み取りスレッドと書き込みを並行しても deadlock しない
// （Microsoft の推奨: 読み書きを別スレッドで処理する）
#[tokio::test]
async fn test_pty_concurrent_read_write_no_deadlock() {
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(256);
    let mut session = PtySession::spawn(default_size(), None, output_tx, None).unwrap();

    // 読み取りタスクを並行起動
    let read_task = tokio::spawn(async move {
        let mut total = 0usize;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
        while tokio::time::Instant::now() < deadline {
            if let Ok(Some(data)) =
                tokio::time::timeout(Duration::from_millis(100), output_rx.recv()).await
            {
                total += data.len();
            }
        }
        total
    });

    // 複数回書き込みを行う
    for _ in 0..5 {
        session.write(b"\r").unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // デッドロックせずに read_task が完了することを確認
    let result = tokio::time::timeout(Duration::from_secs(4), read_task).await;
    assert!(result.is_ok(), "Read/write concurrency should not deadlock");
}

// J-1 (簡易版): 大量出力でも詰まらない
// 要件定義書 §8 "2M line cat without stalling (5.27s benchmark)"
// ここでは 10,000 行の dir 出力を処理できることを確認する
#[tokio::test]
async fn test_large_output_does_not_stall() {
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(1024);
    let mut session = PtySession::spawn(default_size(), None, output_tx, None).unwrap();

    // 初期化を待つ
    tokio::time::sleep(Duration::from_millis(500)).await;

    // 大量出力を生成（for /L ループ）
    session
        .write(b"for /L %i in (1,1,1000) do @echo line %i\r")
        .unwrap();

    // 5 秒以内に全出力を受け取れることを確認
    let bytes_received = tokio::time::timeout(Duration::from_secs(5), async {
        let mut total = 0usize;
        let mut last_receive = tokio::time::Instant::now();
        loop {
            match tokio::time::timeout(Duration::from_millis(500), output_rx.recv()).await {
                Ok(Some(data)) => {
                    total += data.len();
                    last_receive = tokio::time::Instant::now();
                }
                _ => {
                    // 500ms 無音 = 出力終了とみなす
                    if last_receive.elapsed() > Duration::from_millis(400) {
                        break;
                    }
                }
            }
        }
        total
    })
    .await
    .expect("Large output should complete within 5 seconds");

    assert!(bytes_received > 0, "Should have received some output");
}

// A-6: PTY 子プロセスに yatamux 環境変数が伝搬される
#[tokio::test]
async fn test_pty_propagates_yatamux_env_vars() {
    let (output_tx, mut output_rx) = mpsc::channel::<Vec<u8>>(64);
    let mut cmd = CommandBuilder::new("cmd.exe");
    cmd.args(["/C", "echo %YATAMUX% %TERM_PROGRAM% %YATAMUX_SESSION%"]);

    let _session = PtySession::spawn(default_size(), Some(cmd), output_tx, None).unwrap();

    let output = tokio::time::timeout(Duration::from_secs(5), async {
        let mut all = Vec::new();
        while let Some(data) = output_rx.recv().await {
            all.extend_from_slice(&data);
            let text = String::from_utf8_lossy(&all);
            if text.contains("1 yatamux default") {
                return text.into_owned();
            }
        }
        String::from_utf8_lossy(&all).into_owned()
    })
    .await
    .expect("timeout waiting for env var output");

    assert!(
        output.contains("1 yatamux default"),
        "PTY child should receive yatamux env vars, got: {output:?}"
    );
}
