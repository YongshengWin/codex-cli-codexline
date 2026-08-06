use std::io::{Read, Write};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use portable_pty::{CommandBuilder, PtySize, native_pty_system};

#[test]
fn native_pty_or_conpty_supports_a_bidirectional_session() {
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
        command.args(["/D", "/Q"]);
        command
    };
    #[cfg(not(windows))]
    let command = CommandBuilder::new("/bin/sh");
    let expected = "CODEXLINE_PTY_OK";
    let mut reader = pair
        .master
        .try_clone_reader()
        .expect("PTY output reader should be available");
    let mut input = pair
        .master
        .take_writer()
        .expect("PTY input writer should be available");
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("fixture should start inside the PTY");
    drop(pair.slave);
    let (query_tx, _query_rx) = mpsc::channel();
    let (output_tx, output_rx) = mpsc::channel();
    let reader_thread = thread::spawn(move || {
        let mut output = Vec::new();
        let mut buffer = [0_u8; 4096];
        let mut reported_query = false;
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => {
                    let _ = output_tx.send(Ok(output));
                    break;
                }
                Ok(count) => {
                    output.extend_from_slice(&buffer[..count]);
                    if !reported_query
                        && output
                            .windows(b"\x1b[6n".len())
                            .any(|window| window == b"\x1b[6n")
                    {
                        let _ = query_tx.send(());
                        reported_query = true;
                    }
                }
                Err(error) => {
                    let _ = output_tx.send(Err(error));
                    break;
                }
            }
        }
    });

    #[cfg(windows)]
    {
        // ConPTY asks its terminal emulator for the cursor position (`CSI 6 n`) during
        // startup. Codexline normally relays the real terminal's reply; this headless test
        // must provide the conventional row 1, column 1 response itself.
        _query_rx
            .recv_timeout(Duration::from_secs(5))
            .expect("ConPTY should query the cursor position during startup");
        input
            .write_all(b"\x1b[1;1R")
            .expect("ConPTY cursor-position reply should be writable");
        input.flush().expect("ConPTY reply should flush");
        thread::sleep(Duration::from_millis(20));
        input
            .write_all(b"echo CODEXLINE_PTY_OK\r\nexit\r\n")
            .expect("PTY input should be writable");
    }
    #[cfg(not(windows))]
    input
        .write_all(b"printf CODEXLINE_PTY_OK\\n\nexit\n")
        .expect("PTY input should be writable");
    input.flush().expect("PTY input should flush");

    // Taking and dropping the writer generates EOF if the shell exits before consuming `exit`.
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

    assert!(output.contains(expected), "output was {output:?}");
}
