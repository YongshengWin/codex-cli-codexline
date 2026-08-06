use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

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
    let expected_bytes = expected.as_bytes().to_vec();
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
        let mut buffer = [0_u8; 4096];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    output.extend_from_slice(&buffer[..count]);
                    if output
                        .windows(expected_bytes.len())
                        .any(|window| window == expected_bytes)
                    {
                        break;
                    }
                }
                Err(error) => {
                    let _ = output_tx.send(Err(error));
                    return;
                }
            }
        }
        let _ = output_tx.send(Ok(output));
    });

    let output = output_rx
        .recv_timeout(Duration::from_secs(5))
        .expect("PTY output reader timed out")
        .expect("PTY output should be readable");
    reader_thread.join().expect("PTY reader should stop");
    let output = String::from_utf8_lossy(&output);

    assert!(output.contains(&expected), "output was {output:?}");
    // ConPTY's host lifetime is owned by the master handle. Close its input and master before
    // reaping so the test models Codexline's real teardown order on every supported platform.
    drop(input);
    drop(pair.master);
    let _ = child.kill();
    child.wait().expect("fixture should be reaped");
}
