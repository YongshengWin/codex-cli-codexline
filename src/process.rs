use std::env;
use std::fs;
use std::io::{self, IsTerminal, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::mpsc::{self, RecvTimeoutError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result};
use portable_pty::{CommandBuilder, PtySize, native_pty_system};

use crate::app_server::{AppServerSource, ProtocolProxy};
use crate::config::DisplayConfig;
use crate::events::EventServer;
use crate::render::{StatusRenderer, TerminalGuard};
use crate::state::StatusSnapshot;

#[derive(Debug, Clone, Copy)]
pub enum BypassReason {
    Explicit,
    NonInteractive,
    DumbTerminal,
    ContinuousIntegration,
    ExecSubcommand,
    TerminalTooSmall,
}

impl BypassReason {
    pub fn label(self) -> &'static str {
        match self {
            Self::Explicit => "requested with --no-companion",
            Self::NonInteractive => "stdin or stdout is not a TTY",
            Self::DumbTerminal => "TERM=dumb",
            Self::ContinuousIntegration => "CI environment detected",
            Self::ExecSubcommand => "codex exec is non-interactive",
            Self::TerminalTooSmall => "terminal is smaller than 40x8",
        }
    }
}

pub struct LaunchRequest {
    pub executable: PathBuf,
    pub args: Vec<String>,
    pub bypass: Option<BypassReason>,
    pub display: DisplayConfig,
    pub snapshot: StatusSnapshot,
    pub app_server: bool,
    pub remote_proxy: bool,
}

pub fn bypass_reason(explicit: bool) -> Option<BypassReason> {
    if explicit {
        return Some(BypassReason::Explicit);
    }
    if !io::stdin().is_terminal() || !io::stdout().is_terminal() {
        return Some(BypassReason::NonInteractive);
    }
    if env::var("TERM").is_ok_and(|value| value == "dumb") {
        return Some(BypassReason::DumbTerminal);
    }
    if env::var_os("CI").is_some() {
        return Some(BypassReason::ContinuousIntegration);
    }
    if let Ok((columns, rows)) = crossterm::terminal::size()
        && (columns < 40 || rows < 8)
    {
        return Some(BypassReason::TerminalTooSmall);
    }
    None
}

pub fn launch(request: LaunchRequest) -> Result<i32> {
    let exec_subcommand = request.args.first().is_some_and(|arg| arg == "exec");
    if request.bypass.is_some() || exec_subcommand {
        return launch_direct(
            &request.executable,
            &request.args,
            request.bypass.or(Some(BypassReason::ExecSubcommand)),
        );
    }
    match launch_pty(
        &request.executable,
        &request.args,
        &request.display,
        request.snapshot,
        request.app_server,
        request.remote_proxy,
    ) {
        PtyOutcome::Complete(code) => Ok(code),
        PtyOutcome::Unavailable(error) => {
            eprintln!("codexline: overlay unavailable ({error}); starting Codex directly");
            launch_direct(&request.executable, &request.args, None)
        }
        PtyOutcome::StartedFailure(error) => Err(error),
    }
}

enum PtyOutcome {
    Complete(i32),
    Unavailable(anyhow::Error),
    StartedFailure(anyhow::Error),
}

fn launch_direct(executable: &Path, args: &[String], _reason: Option<BypassReason>) -> Result<i32> {
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        let error = Command::new(executable).args(args).exec();
        Err(error).with_context(|| format!("failed to execute {}", executable.display()))
    }
    #[cfg(not(unix))]
    {
        let status = Command::new(executable)
            .args(args)
            .status()
            .with_context(|| format!("failed to execute {}", executable.display()))?;
        Ok(status.code().unwrap_or(1))
    }
}

fn launch_pty(
    executable: &Path,
    args: &[String],
    display: &DisplayConfig,
    snapshot: StatusSnapshot,
    app_server: bool,
    remote_proxy: bool,
) -> PtyOutcome {
    match prepare_pty(
        executable,
        args,
        display,
        snapshot,
        app_server,
        remote_proxy,
    ) {
        Ok(code) => PtyOutcome::Complete(code),
        Err((false, error)) => PtyOutcome::Unavailable(error),
        Err((true, error)) => PtyOutcome::StartedFailure(error),
    }
}

fn prepare_pty(
    executable: &Path,
    args: &[String],
    display: &DisplayConfig,
    snapshot: StatusSnapshot,
    app_server: bool,
    remote_proxy: bool,
) -> std::result::Result<i32, (bool, anyhow::Error)> {
    let before = |error| (false, error);
    let after = |error| (true, error);
    let (columns, rows) = crossterm::terminal::size()
        .context("could not read terminal size")
        .map_err(before)?;
    let mut reserved_rows = u16::from(display.rows);
    let child_rows = rows.saturating_sub(reserved_rows);
    let pair = native_pty_system()
        .openpty(PtySize {
            rows: child_rows,
            cols: columns,
            pixel_width: 0,
            pixel_height: 0,
        })
        .context("could not create PTY/ConPTY")
        .map_err(before)?;

    // Enter raw mode before spawning so any failure here is safe to fall back from.
    let terminal = TerminalGuard::enter(child_rows, reserved_rows).map_err(before)?;

    let snapshot = Arc::new(RwLock::new(snapshot));
    let event_server = EventServer::start(Arc::clone(&snapshot)).ok();

    let proxy = remote_proxy
        .then(|| ProtocolProxy::start(executable, Arc::clone(&snapshot)).ok())
        .flatten();
    let mut child_args = args.to_vec();
    if let Some(proxy) = &proxy {
        child_args.push("--remote".into());
        child_args.push(proxy.endpoint().into());
    }
    let mut command = CommandBuilder::new(executable);
    command.args(&child_args);
    command.env("CODEXLINE_ACTIVE", "1");
    if let Some(server) = &event_server {
        command.env("CODEXLINE_EVENT_ENDPOINT", server.endpoint());
        command.env("CODEXLINE_EVENT_TOKEN", server.token());
        if let Ok(current_exe) = std::env::current_exe() {
            command.env("CODEXLINE_HOOK_BIN", current_exe);
        }
    }
    let mut child = pair
        .slave
        .spawn_command(command)
        .with_context(|| format!("failed to start {}", executable.display()))
        .map_err(before)?;
    drop(pair.slave);

    // This optional read-only source never sits on the PTY relay path. Failure is silent and
    // leaves Hooks/local probes fully functional.
    let _app_server = (app_server && proxy.is_none())
        .then(|| AppServerSource::start(executable, Arc::clone(&snapshot)).ok())
        .flatten();

    let mut renderer = StatusRenderer::new(display.clone(), Arc::clone(&snapshot));
    let agent_panel = renderer.agent_panel();
    let input_snapshot = Arc::clone(&snapshot);

    let writer = Arc::new(Mutex::new(pair.master.take_writer().map_err(after)?));
    let input_writer = Arc::clone(&writer);
    // Detached deliberately: a blocking terminal read cannot be cancelled portably. The
    // operating system tears it down when the short-lived wrapper process exits.
    thread::spawn(move || {
        let mut stdin = io::stdin().lock();
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            let count = match stdin.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(count) => count,
            };
            let agent_count = input_snapshot
                .read()
                .map_or(0, |snapshot| snapshot.agents.len());
            let handled = agent_panel.lock().map_or(true, |mut panel| {
                panel.handle_input(&buffer[..count], agent_count)
            });
            if handled {
                continue;
            }
            let Ok(mut writer) = input_writer.lock() else {
                break;
            };
            if writer.write_all(&buffer[..count]).is_err() || writer.flush().is_err() {
                break;
            }
        }
    });

    let mut reader = pair.master.try_clone_reader().map_err(after)?;
    let (output_tx, output_rx) = mpsc::sync_channel::<io::Result<Vec<u8>>>(8);
    thread::spawn(move || {
        let mut buffer = vec![0_u8; 64 * 1024];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) => break,
                Ok(count) => {
                    if output_tx.send(Ok(buffer[..count].to_vec())).is_err() {
                        break;
                    }
                }
                Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                Err(error) => {
                    let _ = output_tx.send(Err(error));
                    break;
                }
            }
        }
    });
    let mut stdout = io::stdout().lock();
    reserved_rows = renderer
        .required_rows(columns)
        .min(rows.saturating_sub(4).max(1));
    if reserved_rows != u16::from(display.rows) {
        pair.master
            .resize(PtySize {
                rows: rows.saturating_sub(reserved_rows),
                cols: columns,
                pixel_width: 0,
                pixel_height: 0,
            })
            .map_err(after)?;
        terminal
            .update_reserved_rows(rows.saturating_sub(reserved_rows), reserved_rows)
            .map_err(after)?;
    }
    renderer.draw(&mut stdout, columns, rows).map_err(after)?;
    let mut last_size = (columns, rows);
    let mut last_draw = std::time::Instant::now();
    let frame_interval = Duration::from_millis(1000 / u64::from(display.refresh_hz));
    loop {
        match output_rx.recv_timeout(Duration::from_millis(50)) {
            Ok(Ok(bytes)) => {
                stdout
                    .write_all(&bytes)
                    .map_err(|error| after(error.into()))?;
                if last_draw.elapsed() >= frame_interval {
                    renderer
                        .draw(&mut stdout, last_size.0, last_size.1)
                        .map_err(after)?;
                    last_draw = std::time::Instant::now();
                }
                stdout.flush().map_err(|error| after(error.into()))?;
            }
            Ok(Err(error)) => {
                return Err(after(
                    anyhow::Error::new(error).context("failed to read child PTY"),
                ));
            }
            Err(RecvTimeoutError::Disconnected) => break,
            Err(RecvTimeoutError::Timeout) => {}
        }
        if let Ok(size) = crossterm::terminal::size()
            && size != last_size
        {
            last_size = size;
            pair.master
                .resize(PtySize {
                    rows: size.1.saturating_sub(reserved_rows),
                    cols: size.0,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(after)?;
            terminal
                .update_reserved_rows(size.1.saturating_sub(reserved_rows), reserved_rows)
                .map_err(after)?;
        }
        let wanted_rows = renderer
            .required_rows(last_size.0)
            .min(last_size.1.saturating_sub(4).max(1));
        if wanted_rows != reserved_rows {
            reserved_rows = wanted_rows;
            pair.master
                .resize(PtySize {
                    rows: last_size.1.saturating_sub(reserved_rows),
                    cols: last_size.0,
                    pixel_width: 0,
                    pixel_height: 0,
                })
                .map_err(after)?;
            terminal
                .update_reserved_rows(last_size.1.saturating_sub(reserved_rows), reserved_rows)
                .map_err(after)?;
            renderer
                .draw(&mut stdout, last_size.0, last_size.1)
                .map_err(after)?;
            stdout.flush().map_err(|error| after(error.into()))?;
            last_draw = std::time::Instant::now();
        }
        if last_draw.elapsed() >= Duration::from_secs(1) {
            renderer
                .draw(&mut stdout, last_size.0, last_size.1)
                .map_err(after)?;
            stdout.flush().map_err(|error| after(error.into()))?;
            last_draw = std::time::Instant::now();
        }
    }
    let status = child
        .wait()
        .context("failed to wait for Codex")
        .map_err(after)?;
    drop(writer);
    Ok(status.exit_code() as i32)
}

pub fn discover_codex() -> Result<PathBuf> {
    let current = env::current_exe()
        .ok()
        .and_then(|path| fs::canonicalize(path).ok());
    if let Some(path) = env::var_os("CODEXLINE_CODEX_BIN").map(PathBuf::from) {
        anyhow::ensure!(
            path.is_absolute(),
            "CODEXLINE_CODEX_BIN must be an absolute path"
        );
        return validate_candidate(path, current.as_deref());
    }
    let path = env::var_os("PATH").context("PATH is not set")?;
    for directory in env::split_paths(&path) {
        for name in executable_names() {
            let candidate = directory.join(name);
            if let Ok(path) = validate_candidate(candidate, current.as_deref()) {
                return Ok(path);
            }
        }
    }
    anyhow::bail!("official `codex` was not found; set CODEXLINE_CODEX_BIN to its absolute path")
}

fn validate_candidate(candidate: PathBuf, current: Option<&Path>) -> Result<PathBuf> {
    anyhow::ensure!(candidate.is_file(), "not a file");
    let canonical = fs::canonicalize(&candidate).context("could not resolve candidate")?;
    anyhow::ensure!(
        Some(canonical.as_path()) != current,
        "candidate is Codexline itself"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        anyhow::ensure!(
            fs::metadata(&canonical)?.permissions().mode() & 0o111 != 0,
            "candidate is not executable"
        );
    }
    Ok(canonical)
}

#[cfg(windows)]
fn executable_names() -> &'static [&'static str] {
    &["codex.exe", "codex.cmd", "codex.bat"]
}

#[cfg(not(windows))]
fn executable_names() -> &'static [&'static str] {
    &["codex"]
}

pub fn backend_name() -> &'static str {
    if cfg!(windows) {
        "ConPTY (portable-pty)"
    } else {
        "POSIX PTY (portable-pty)"
    }
}

#[cfg(test)]
mod tests {
    use super::validate_candidate;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn rejects_non_executable_candidate() {
        let directory = tempdir().unwrap();
        let candidate = directory.path().join("codex");
        fs::write(&candidate, "fixture").unwrap();
        assert!(validate_candidate(candidate, None).is_err());
    }
}
