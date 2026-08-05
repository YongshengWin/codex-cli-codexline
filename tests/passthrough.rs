#![cfg(unix)]

use std::process::Command;

use tempfile::tempdir;

#[test]
fn non_tty_mode_preserves_output_arguments_and_exit_code() {
    let binary = env!("CARGO_BIN_EXE_codexline");
    let output = Command::new(binary)
        .env("CODEXLINE_CODEX_BIN", "/bin/sh")
        .args([
            "run",
            "--",
            "-c",
            "printf 'fixture:%s' \"$1\"; exit 23",
            "sh",
            "hello",
        ])
        .output()
        .expect("codexline should launch the fixture");

    assert_eq!(output.status.code(), Some(23));
    assert_eq!(String::from_utf8_lossy(&output.stdout), "fixture:hello");
}

#[test]
fn explicit_bypass_is_consumed_by_codexline() {
    let binary = env!("CARGO_BIN_EXE_codexline");
    let output = Command::new(binary)
        .env("CODEXLINE_CODEX_BIN", "/bin/sh")
        .args(["run", "--", "--no-companion", "-c", "exit 0"])
        .output()
        .expect("codexline should launch the fixture");

    assert!(output.status.success());
}

#[test]
fn invalid_config_never_blocks_codex() {
    let binary = env!("CARGO_BIN_EXE_codexline");
    let config_home = tempdir().unwrap();
    std::fs::create_dir(config_home.path().join("codexline")).unwrap();
    std::fs::write(
        config_home.path().join("codexline/config.toml"),
        "this is not valid toml = [",
    )
    .unwrap();

    let output = Command::new(binary)
        .env("XDG_CONFIG_HOME", config_home.path())
        .env("CODEXLINE_CODEX_BIN", "/bin/sh")
        .args(["run", "--", "-c", "printf safe-fallback"])
        .output()
        .expect("invalid config must not block the fixture");

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout), "safe-fallback");
    assert!(String::from_utf8_lossy(&output.stderr).contains("ignoring invalid configuration"));
}
