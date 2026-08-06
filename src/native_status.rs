use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NativeStatusState {
    Enabled,
    Disabled,
    Unknown,
}

impl fmt::Display for NativeStatusState {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Enabled => "enabled",
            Self::Disabled => "disabled",
            Self::Unknown => "unknown",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Detection {
    pub state: NativeStatusState,
    pub source: &'static str,
}

impl Detection {
    fn new(state: NativeStatusState, source: &'static str) -> Self {
        Self { state, source }
    }
}

/// Detects the effective native footer conservatively. Project configuration is
/// trust-gated by Codex, and Codexline deliberately does not inspect private trust state.
pub fn detect(codex_args: &[String]) -> Detection {
    if let Some(detection) = command_line_override(codex_args) {
        return detection;
    }

    if project_config_mentions_status_line() {
        return Detection::new(
            NativeStatusState::Unknown,
            "project config (trust-dependent)",
        );
    }

    if let Some(profile) = selected_profile(codex_args) {
        let path = codex_home().join(format!("{profile}.config.toml"));
        match read_setting(&path) {
            Ok(Some(enabled)) => return configured(enabled, "selected profile"),
            Ok(None) => {}
            Err(_) => return Detection::new(NativeStatusState::Unknown, "selected profile"),
        }
    }

    let user_config = codex_home().join("config.toml");
    match read_setting(&user_config) {
        Ok(Some(enabled)) => return configured(enabled, "user config"),
        Ok(None) => {}
        Err(_) => return Detection::new(NativeStatusState::Unknown, "user config"),
    }

    #[cfg(unix)]
    {
        match read_setting(Path::new("/etc/codex/config.toml")) {
            Ok(Some(enabled)) => return configured(enabled, "system config"),
            Ok(None) => {}
            Err(_) => return Detection::new(NativeStatusState::Unknown, "system config"),
        }
    }

    Detection::new(NativeStatusState::Enabled, "Codex built-in default")
}

/// Disables the native footer for one companion-managed child process. An
/// explicit user override wins, so advanced users can intentionally keep it.
pub fn disable_for_companion(codex_args: &mut Vec<String>) -> bool {
    if command_line_override(codex_args).is_some() {
        return false;
    }
    codex_args.push("-c".into());
    codex_args.push("tui.status_line=[]".into());
    true
}

fn configured(enabled: bool, source: &'static str) -> Detection {
    Detection::new(
        if enabled {
            NativeStatusState::Enabled
        } else {
            NativeStatusState::Disabled
        },
        source,
    )
}

fn command_line_override(args: &[String]) -> Option<Detection> {
    let mut index = 0;
    let mut result = None;
    while index < args.len() {
        let value = if matches!(args[index].as_str(), "-c" | "--config") {
            index += 1;
            args.get(index).map(String::as_str)
        } else {
            args[index]
                .strip_prefix("--config=")
                .or_else(|| args[index].strip_prefix("-c="))
        };
        if let Some(value) = value {
            if let Some(raw) = value.strip_prefix("tui.status_line=") {
                result = Some(match parse_override(raw) {
                    Ok(enabled) => configured(enabled, "command-line override"),
                    Err(_) => Detection::new(NativeStatusState::Unknown, "command-line override"),
                });
            }
        }
        index += 1;
    }
    result
}

fn parse_override(raw: &str) -> Result<bool> {
    let document: toml::Value = toml::from_str(&format!("value = {raw}"))?;
    status_value(document.get("value").context("override has no value")?)
}

fn selected_profile(args: &[String]) -> Option<&str> {
    args.windows(2)
        .rev()
        .find(|pair| matches!(pair[0].as_str(), "-p" | "--profile"))
        .map(|pair| pair[1].as_str())
        .or_else(|| {
            args.iter()
                .rev()
                .find_map(|arg| arg.strip_prefix("--profile="))
        })
}

fn project_config_mentions_status_line() -> bool {
    let Ok(cwd) = std::env::current_dir() else {
        return true;
    };
    project_config_mentions_status_line_from(&cwd)
}

fn project_config_mentions_status_line_from(cwd: &Path) -> bool {
    for directory in cwd.ancestors() {
        let path = directory.join(".codex/config.toml");
        if matches!(read_setting(&path), Ok(Some(_)) | Err(_)) {
            return true;
        }
        // Codex project layers do not continue above the workspace root. Without
        // this boundary, $HOME/.codex/config.toml is misclassified as project config.
        if directory.join(".git").exists() {
            break;
        }
    }
    false
}

fn codex_home() -> PathBuf {
    if let Some(path) = std::env::var_os("CODEX_HOME") {
        return PathBuf::from(path);
    }
    directories::BaseDirs::new()
        .map(|dirs| dirs.home_dir().join(".codex"))
        .unwrap_or_else(|| PathBuf::from(".codex"))
}

fn read_setting(path: &Path) -> Result<Option<bool>> {
    if !path.exists() {
        return Ok(None);
    }
    let source = fs::read_to_string(path)
        .with_context(|| format!("failed to read Codex config at {}", path.display()))?;
    let document: toml::Value = toml::from_str(&source)
        .with_context(|| format!("invalid Codex config at {}", path.display()))?;
    let Some(value) = document.get("tui").and_then(|tui| tui.get("status_line")) else {
        return Ok(None);
    };
    status_value(value).map(Some)
}

fn status_value(value: &toml::Value) -> Result<bool> {
    let items = value
        .as_array()
        .context("tui.status_line must be an array")?;
    Ok(!items.is_empty())
}

#[cfg(test)]
mod tests {
    use super::{
        NativeStatusState, command_line_override, disable_for_companion,
        project_config_mentions_status_line_from, read_setting,
    };
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn empty_cli_array_disables_native_footer() {
        let args = vec!["-c".into(), "tui.status_line=[]".into()];
        let result = command_line_override(&args).unwrap();
        assert_eq!(result.state, NativeStatusState::Disabled);
    }

    #[test]
    fn nonempty_cli_array_enables_native_footer() {
        let args = vec!["--config=tui.status_line=[\"model\"]".into()];
        let result = command_line_override(&args).unwrap();
        assert_eq!(result.state, NativeStatusState::Enabled);
    }

    #[test]
    fn companion_disables_native_footer_without_overriding_user_intent() {
        let mut args = vec!["--model".into(), "gpt-test".into()];
        assert!(disable_for_companion(&mut args));
        assert_eq!(args[args.len() - 2..], ["-c", "tui.status_line=[]"]);

        let mut explicit = vec!["-c".into(), "tui.status_line=[\"model\"]".into()];
        assert!(!disable_for_companion(&mut explicit));
        assert_eq!(explicit.len(), 2);
    }

    #[test]
    fn reads_enabled_and_disabled_files() {
        let directory = tempdir().unwrap();
        let path = directory.path().join("config.toml");
        fs::write(&path, "[tui]\nstatus_line = []\n").unwrap();
        assert_eq!(read_setting(&path).unwrap(), Some(false));
        fs::write(&path, "[tui]\nstatus_line = [\"model\"]\n").unwrap();
        assert_eq!(read_setting(&path).unwrap(), Some(true));
    }

    #[test]
    fn project_search_stops_before_home_config() {
        let directory = tempdir().unwrap();
        let project = directory.path().join("project");
        let nested = project.join("nested");
        fs::create_dir_all(project.join(".git")).unwrap();
        fs::create_dir_all(&nested).unwrap();
        fs::create_dir_all(directory.path().join(".codex")).unwrap();
        fs::write(
            directory.path().join(".codex/config.toml"),
            "[tui]\nstatus_line = [\"model\"]\n",
        )
        .unwrap();

        assert!(!project_config_mentions_status_line_from(&nested));
    }
}
