use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use crate::state::StatusSnapshot;

pub fn local_snapshot(codex_args: &[String]) -> StatusSnapshot {
    let config = read_codex_config();
    let (model, reasoning, safety) = local_codex_settings(codex_args, config.as_ref());
    let git = git_status();
    StatusSnapshot {
        model,
        reasoning,
        work: Some("ready".into()),
        // A plain interactive launch starts with an empty context. Resumed and forked threads
        // remain unknown until app-server publishes their real token usage.
        context_percent: starts_fresh_thread(codex_args).then_some(0),
        context_used: starts_fresh_thread(codex_args).then_some(0),
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
        safety,
        ..StatusSnapshot::default()
    }
}

fn starts_fresh_thread(args: &[String]) -> bool {
    !args
        .iter()
        .any(|arg| matches!(arg.as_str(), "resume" | "fork"))
}

fn read_codex_config() -> Option<toml::Value> {
    let home = if let Some(path) = std::env::var_os("CODEX_HOME") {
        PathBuf::from(path)
    } else {
        directories::BaseDirs::new()?.home_dir().join(".codex")
    };
    toml::from_str(&fs::read_to_string(home.join("config.toml")).ok()?).ok()
}

fn local_codex_settings(
    args: &[String],
    config: Option<&toml::Value>,
) -> (Option<String>, Option<String>, Option<String>) {
    let model = option_value(args, &["-m", "--model"])
        .map(str::to_owned)
        .or_else(|| config_string(config, "model"));
    let reasoning = config_string(config, "model_reasoning_effort");
    let sandbox = option_value(args, &["-s", "--sandbox"])
        .map(str::to_owned)
        .or_else(|| config_string(config, "sandbox_mode"));
    let approval = option_value(args, &["-a", "--ask-for-approval"])
        .map(str::to_owned)
        .or_else(|| config_string(config, "approval_policy"));
    let safety = match (sandbox, approval) {
        (Some(sandbox), Some(approval)) => Some(format!(
            "{} · {}",
            compact_safety(&sandbox),
            compact_safety(&approval)
        )),
        (Some(value), None) | (None, Some(value)) => Some(compact_safety(&value)),
        (None, None) => None,
    };
    (model, reasoning, safety)
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

fn compact_safety(value: &str) -> String {
    match value {
        "workspace-write" => "workspace".into(),
        "on-request" => "ask".into(),
        other => other.to_owned(),
    }
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

fn git_status() -> LocalGit {
    let branch = run_git(&["rev-parse", "--abbrev-ref", "HEAD"])
        .filter(|(success, _)| *success)
        .map(|(_, output)| output);
    if branch.is_none() {
        return LocalGit::default();
    }
    let porcelain = run_git(&["status", "--porcelain", "--untracked-files=no"])
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
    let (behind, ahead) = run_git(&["rev-list", "--left-right", "--count", "@{upstream}...HEAD"])
        .filter(|(success, _)| *success)
        .and_then(|(_, output)| {
            let mut values = output
                .split_whitespace()
                .filter_map(|value| value.parse().ok());
            Some((values.next()?, values.next()?))
        })
        .map_or((None, None), |(behind, ahead)| (Some(behind), Some(ahead)));
    let root = run_git(&["rev-parse", "--show-toplevel"])
        .filter(|(success, _)| *success)
        .map(|(_, output)| PathBuf::from(output));
    let git_dir = run_git(&["rev-parse", "--absolute-git-dir"])
        .filter(|(success, _)| *success)
        .map(|(_, output)| PathBuf::from(output));
    let common_dir = run_git(&["rev-parse", "--git-common-dir"])
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

fn run_git(args: &[&str]) -> Option<(bool, String)> {
    let mut child = Command::new("git")
        .args(args)
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
    use super::{local_codex_settings, option_value};

    #[test]
    fn command_line_model_wins() {
        let args = vec!["--model=gpt-test".into(), "-s".into(), "read-only".into()];
        assert_eq!(option_value(&args, &["-m", "--model"]), Some("gpt-test"));
        let settings = local_codex_settings(&args, None);
        assert_eq!(settings.0.as_deref(), Some("gpt-test"));
        assert_eq!(settings.2.as_deref(), Some("read-only"));
    }
}
