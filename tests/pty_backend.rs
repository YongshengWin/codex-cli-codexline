use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[test]
fn native_pty_or_conpty_runs_codexline() {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 12,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("native PTY backend should initialize");

    let mut command = CommandBuilder::new(env!("CARGO_BIN_EXE_codexline"));
    command.arg("--version");
    let expected = format!("codexline {}", env!("CARGO_PKG_VERSION"));
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("PTY output reader should be available");
    let input = pair
        .master
        .take_writer()
        .expect("PTY input writer should be available");
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("fixture should start inside the PTY");
    drop(pair.slave);
    let (output_tx, output_rx) = mpsc::channel();
    let reader_thread = thread::spawn(move || {
        let mut output = Vec::new();
        let result = reader.read_to_end(&mut output).map(|_| output);
        let _ = output_tx.send(result);
    });

    // portable-pty documents that taking and then dropping the writer is required to generate
    // EOF reliably. A short grace period also avoids losing very short-lived output on macOS.
    thread::sleep(Duration::from_millis(20));
    drop(input);

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        if child
            .try_wait()
            .expect("fixture status should be readable")
            .is_some()
        {
            break;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("PTY fixture did not exit within five seconds");
        }
        thread::sleep(Duration::from_millis(10));
    }

    // ConPTY may retain the final screen data until its master handle closes.
    drop(pair.master);
    let output = output_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("PTY output reader timed out")
        .expect("PTY output should be readable");
    reader_thread.join().expect("PTY reader should stop");
    let output = String::from_utf8_lossy(&output);

    assert!(output.contains(&expected), "output was {output:?}");
}
