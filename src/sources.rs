use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};

use crate::state::StatusSnapshot;

pub fn local_snapshot(codex_args: &[String]) -> StatusSnapshot {
    let config = read_codex_config();
    let settings = local_codex_settings(codex_args, config.as_ref());
    let git = git_status_at(std::env::current_dir().ok().as_deref());
    StatusSnapshot {
        model: settings.model,
        reasoning: settings.reasoning,
        model_live: false,
        work: Some("ready".into()),
        // A plain interactive launch starts with an empty context. Resumed and forked threads
        // remain unknown until app-server publishes their real token usage.
        context_percent: starts_fresh_thread(codex_args).then_some(0),
        context_used: starts_fresh_thread(codex_args).then_some(0),
        context_live: false,
        cwd: std::env::current_dir()
            .ok()
            .map(|path| path.to_string_lossy().into_owned()),
        project_root: git.project_root,
        git_branch: git.branch,
        git_dirty: git.dirty,
        git_staged: git.staged,
        git_modified: git.modified,
        git_ahead: git.ahead,
        git_behind: git.behind,
        worktree: git.worktree,
        linked_worktree: git.linked_worktree,
        sandbox: settings.sandbox,
        approval_policy: settings.approval_policy,
        approvals_reviewer: settings.approvals_reviewer,
        settings_live: false,
        ..StatusSnapshot::default()
    }
}

pub fn start_local_refresh(snapshot: Arc<RwLock<StatusSnapshot>>) {
    thread::spawn(move || {
        let mut config_stamp = codex_config_stamp();
        loop {
            thread::sleep(Duration::from_secs(3));
            let next_stamp = codex_config_stamp();
            let settings = (next_stamp != config_stamp)
                .then(read_codex_config)
                .flatten()
                .map(|config| local_codex_settings(&[], Some(&config)));
            config_stamp = next_stamp;
            let cwd = snapshot
                .read()
                .ok()
                .and_then(|state| state.cwd.as_ref().map(PathBuf::from));
            let git = git_status_at(cwd.as_deref());
            let Ok(mut state) = snapshot.write() else {
                break;
            };
            state.project_root = git.project_root;
            state.git_branch = git.branch;
            state.git_dirty = git.dirty;
            state.git_staged = git.staged;
            state.git_modified = git.modified;
            state.git_ahead = git.ahead;
            state.git_behind = git.behind;
            state.worktree = git.worktree;
            state.linked_worktree = git.linked_worktree;
            if let Some(settings) = settings {
                apply_persisted_settings(&mut state, settings);
            }
        }
    });
}

fn apply_persisted_settings(state: &mut StatusSnapshot, settings: LocalCodexSettings) {
    if settings.model.is_some() {
        state.model = settings.model;
        state.reasoning = settings.reasoning;
        state.model_live = true;
    }
    state.sandbox = settings.sandbox;
    state.approval_policy = settings.approval_policy;
    state.approvals_reviewer = settings.approvals_reviewer;
    state.permission_mode = None;
    state.settings_live = true;
}

fn starts_fresh_thread(args: &[String]) -> bool {
    !args
        .iter()
        .any(|arg| matches!(arg.as_str(), "resume" | "fork"))
}

fn read_codex_config() -> Option<toml::Value> {
    toml::from_str(&fs::read_to_string(codex_config_path()?).ok()?).ok()
}

fn codex_config_path() -> Option<PathBuf> {
    let home = if let Some(path) = std::env::var_os("CODEX_HOME") {
        PathBuf::from(path)
    } else {
        directories::BaseDirs::new()?.home_dir().join(".codex")
    };
    Some(home.join("config.toml"))
}

fn codex_config_stamp() -> Option<(std::time::SystemTime, u64)> {
    let metadata = fs::metadata(codex_config_path()?).ok()?;
    Some((metadata.modified().ok()?, metadata.len()))
}

struct LocalCodexSettings {
    model: Option<String>,
    reasoning: Option<String>,
    sandbox: Option<String>,
    approval_policy: Option<String>,
    approvals_reviewer: Option<String>,
}

fn local_codex_settings(args: &[String], config: Option<&toml::Value>) -> LocalCodexSettings {
    let model = option_value(args, &["-m", "--model"])
        .map(str::to_owned)
        .or_else(|| config_string(config, "model"));
    let reasoning = config_string(config, "model_reasoning_effort");
    let sandbox = option_value(args, &["-s", "--sandbox"])
        .map(str::to_owned)
        .or_else(|| config_string(config, "sandbox_mode"));
    let approval_policy = option_value(args, &["-a", "--ask-for-approval"])
        .map(str::to_owned)
        .or_else(|| config_string(config, "approval_policy"));
    LocalCodexSettings {
        model,
        reasoning,
        sandbox,
        approval_policy,
        approvals_reviewer: config_string(config, "approvals_reviewer"),
    }
}

fn option_value<'a>(args: &'a [String], names: &[&str]) -> Option<&'a str> {
    for (index, arg) in args.iter().enumerate().rev() {
        if names.contains(&arg.as_str()) {
            return args.get(index + 1).map(String::as_str);
        }
        for name in names {
            if let Some(value) = arg.strip_prefix(&format!("{name}=")) {
                return Some(value);
            }
        }
    }
    None
}

fn config_string(config: Option<&toml::Value>, key: &str) -> Option<String> {
    config?.get(key)?.as_str().map(ToOwned::to_owned)
}

#[derive(Default)]
struct LocalGit {
    branch: Option<String>,
    dirty: Option<bool>,
    staged: Option<u16>,
    modified: Option<u16>,
    ahead: Option<u16>,
    behind: Option<u16>,
    worktree: Option<String>,
    linked_worktree: Option<bool>,
    project_root: Option<String>,
}

fn git_status_at(cwd: Option<&std::path::Path>) -> LocalGit {
    let branch = run_git(cwd, &["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|(success, _)| *success)
        .map(|(_, output)| output);
    if branch.is_none() {
        return LocalGit::default();
    }
    let porcelain = run_git(cwd, &["status", "--porcelain", "--untracked-files=no"])
        .filter(|(success, _)| *success)
        .map(|(_, output)| output);
    let staged = porcelain.as_ref().map(|output| {
        bounded_count(output.lines().filter(|line| {
            line.as_bytes()
                .first()
                .is_some_and(|value| *value != b' ' && *value != b'?')
        }))
    });
    let modified = porcelain.as_ref().map(|output| {
        bounded_count(
            output
                .lines()
                .filter(|line| line.as_bytes().get(1).is_some_and(|value| *value != b' ')),
        )
    });
    let (behind, ahead) = run_git(
        cwd,
        &["rev-list", "--left-right", "--count", "@{upstream}...HEAD"],
    )
    .filter(|(success, _)| *success)
    .and_then(|(_, output)| {
        let mut values = output
            .split_whitespace()
            .filter_map(|value| value.parse().ok());
        Some((values.next()?, values.next()?))
    })
    .map_or((None, None), |(behind, ahead)| (Some(behind), Some(ahead)));
    let root = run_git(cwd, &["rev-parse", "--show-toplevel"])
        .filter(|(success, _)| *success)
        .map(|(_, output)| PathBuf::from(output));
    let git_dir = run_git(cwd, &["rev-parse", "--absolute-git-dir"])
        .filter(|(success, _)| *success)
        .map(|(_, output)| PathBuf::from(output));
    let common_dir = run_git(cwd, &["rev-parse", "--git-common-dir"])
        .filter(|(success, _)| *success)
        .map(|(_, output)| PathBuf::from(output));
    let linked_worktree = match (&git_dir, &common_dir) {
        (Some(git_dir), Some(common_dir)) => Some(
            fs::canonicalize(git_dir).unwrap_or_else(|_| git_dir.clone())
                != fs::canonicalize(common_dir).unwrap_or_else(|_| common_dir.clone()),
        ),
        _ => None,
    };
    let worktree = root.as_ref().and_then(|path| {
        path.file_name()
            .map(|name| name.to_string_lossy().into_owned())
    });
    LocalGit {
        branch,
        dirty: porcelain.as_ref().map(|output| !output.is_empty()),
        staged,
        modified,
        ahead,
        behind,
        worktree,
        linked_worktree,
        project_root: root.map(|path| path.to_string_lossy().into_owned()),
    }
}

fn bounded_count<'a>(items: impl Iterator<Item = &'a str>) -> u16 {
    items.take(usize::from(u16::MAX)).count() as u16
}

fn run_git(cwd: Option<&std::path::Path>, args: &[&str]) -> Option<(bool, String)> {
    let mut command = Command::new("git");
    command.args(args);
    if let Some(cwd) = cwd {
        command.current_dir(cwd);
    }
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_millis(100);
    let status = loop {
        if let Some(status) = child.try_wait().ok()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return None;
        }
        thread::sleep(Duration::from_millis(2));
    };
    let mut bytes = Vec::new();
    child
        .stdout
        .take()?
        .take(4096)
        .read_to_end(&mut bytes)
        .ok()?;
    Some((
        status.success(),
        String::from_utf8_lossy(&bytes).trim().to_owned(),
    ))
}

#[cfg(test)]
mod tests {
    use super::{apply_persisted_settings, local_codex_settings, option_value};
    use crate::state::StatusSnapshot;

    #[test]
    fn command_line_model_wins() {
        let args = vec!["--model=gpt-test".into(), "-s".into(), "read-only".into()];
        assert_eq!(option_value(&args, &["-m", "--model"]), Some("gpt-test"));
        let settings = local_codex_settings(&args, None);
        assert_eq!(settings.model.as_deref(), Some("gpt-test"));
        assert_eq!(settings.sandbox.as_deref(), Some("read-only"));
    }

    #[test]
    fn persisted_codex_settings_refresh_model_and_reviewer() {
        let config = toml::from_str(
            r#"
            model = "gpt-new"
            model_reasoning_effort = "high"
            sandbox_mode = "workspace-write"
            approval_policy = "on-request"
            approvals_reviewer = "guardian_subagent"
            "#,
        )
        .unwrap();
        let settings = local_codex_settings(&[], Some(&config));
        let mut snapshot = StatusSnapshot::default();
        apply_persisted_settings(&mut snapshot, settings);
        assert_eq!(snapshot.model.as_deref(), Some("gpt-new"));
        assert_eq!(snapshot.reasoning.as_deref(), Some("high"));
        assert_eq!(
            snapshot.approvals_reviewer.as_deref(),
            Some("guardian_subagent")
        );
        assert!(snapshot.model_live);
        assert!(snapshot.settings_live);
    }
}
