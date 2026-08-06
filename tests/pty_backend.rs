use std::io::Read;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[test]
fn native_pty_or_conpty_runs_a_real_child() {
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: 12,
            cols: 80,
            pixel_width: 0,
            pixel_height: 0,
        })
        .expect("native PTY backend should initialize");

    #[cfg(windows)]
    let mut command = {
        let mut command = CommandBuilder::new("cmd.exe");
        command.args(["/D", "/S", "/C", "echo CODEXLINE_PTY_OK"]);
        command
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut command = CommandBuilder::new("/bin/sh");
        command.args(["-c", "printf CODEXLINE_PTY_OK"]);
        command
    };

    command.env("CODEXLINE_PTY_FIXTURE", "1");
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
    // This fixture is intentionally non-interactive. Closing input gives cmd.exe an explicit
    // EOF and lets the Windows pseudo-console host tear down after `/C` completes.
    drop(input);

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
                        .windows(b"CODEXLINE_PTY_OK".len())
                        .any(|window| window == b"CODEXLINE_PTY_OK")
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

    assert!(output.contains("CODEXLINE_PTY_OK"), "output was {output:?}");
    // ConPTY's host lifetime is owned by the master handle, not by cmd.exe's output pipe.
    // Close that owner before reaping so the test models Codexline's real teardown order.
    drop(pair.master);
    let _ = child.kill();
    child.wait().expect("fixture should be reaped");
}
