use std::io::Read;

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
    let mut child = pair
        .slave
        .spawn_command(command)
        .expect("fixture should start inside the PTY");
    drop(pair.slave);

    let mut buffer = [0_u8; 4096];
    let count = reader
        .read(&mut buffer)
        .expect("PTY output should be readable");
    let output = String::from_utf8_lossy(&buffer[..count]);
    let status = child.wait().expect("fixture should exit");

    assert!(status.success());
    assert!(output.contains("CODEXLINE_PTY_OK"), "output was {output:?}");
}
