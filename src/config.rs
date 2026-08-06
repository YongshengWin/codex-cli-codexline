use std::fmt;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context, Result};
#[cfg(windows)]
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub version: u16,
    pub launch: LaunchConfig,
    pub sources: SourcesConfig,
    pub display: DisplayConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 2,
            launch: LaunchConfig::default(),
            sources: SourcesConfig::default(),
            display: DisplayConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SourcesConfig {
    /// Starts a separate read-only official app-server process for account capacity data.
    pub app_server: bool,
    /// Routes the TUI through a loopback app-server proxy to receive live thread events.
    pub remote_proxy: bool,
}

impl Default for SourcesConfig {
    fn default() -> Self {
        Self {
            app_server: true,
            remote_proxy: false,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LaunchMode {
    #[default]
    Shim,
    Explicit,
}

impl fmt::Display for LaunchMode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Shim => "keep `codex` command",
            Self::Explicit => "explicit `codexline` command",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LaunchConfig {
    pub mode: LaunchMode,
    pub bypass_flag: String,
}

impl Default for LaunchConfig {
    fn default() -> Self {
        Self {
            mode: LaunchMode::Shim,
            bypass_flag: "--no-companion".into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DisplayConfig {
    pub theme: Theme,
    pub glyphs: Glyphs,
    pub refresh_hz: u8,
    pub rows: u8,
    pub segments: Vec<Segment>,
    pub separator: String,
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            theme: Theme::PastelSyntax,
            glyphs: Glyphs::Unicode,
            refresh_hz: 8,
            rows: 3,
            segments: vec![
                Segment::App,
                Segment::Model,
                Segment::Work,
                Segment::Context,
                Segment::Tokens,
                Segment::RateLimits,
                Segment::Git,
                Segment::Worktree,
                Segment::Tools,
                Segment::Agents,
                Segment::Plan,
                Segment::Compactions,
                Segment::Safety,
                Segment::Elapsed,
                Segment::Cwd,
                Segment::Status,
            ],
            separator: " │ ".into(),
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "kebab-case")]
pub enum Segment {
    App,
    Model,
    Reasoning,
    Work,
    Context,
    ContextRemaining,
    ContextUsed,
    ContextWindow,
    Tokens,
    InputTokens,
    CachedTokens,
    OutputTokens,
    RateLimits,
    FiveHourLimit,
    WeeklyLimit,
    ResetCredits,
    Git,
    GitDirty,
    GitStaged,
    GitModified,
    GitSync,
    Worktree,
    Tools,
    Agents,
    AgentCount,
    Plan,
    Compactions,
    Safety,
    Elapsed,
    Cwd,
    ProjectRoot,
    SessionId,
    Status,
    HooksHealth,
    AppServerHealth,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
    Inherit,
    Ox96f,
    TokyoNight,
    CatppuccinMocha,
    Dracula,
    Nord,
    Gruvbox,
    RosePine,
    PastelSyntax,
    CodexDark,
    CodexLight,
    Minimal,
    Mono,
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Glyphs {
    Ascii,
    #[default]
    Unicode,
}

impl Config {
    pub fn load_or_default() -> Result<Self> {
        let path = path()?;
        if !path.exists() {
            return Ok(Self::default());
        }
        let source = fs::read_to_string(&path)
            .with_context(|| format!("failed to read {}", path.display()))?;
        let mut config: Self = toml::from_str(&source)
            .with_context(|| format!("invalid configuration in {}", path.display()))?;
        let migrated = config.migrate()?;
        config.validate()?;
        if migrated {
            // Migration must never prevent Codex from starting. Persist it when possible so
            // subsequent launches and manual inspection both see the safe v2 source policy.
            let _ = config.save_atomic();
        }
        Ok(config)
    }

    fn migrate(&mut self) -> Result<bool> {
        match self.version {
            1 => {
                // v1 shipped the experimental WebSocket proxy as the default. A proxy
                // disconnect terminates the official TUI, so existing users migrate to the
                // read-only sidecar unless they explicitly opt back in using a v2 config.
                self.version = 2;
                self.sources.remote_proxy = false;
                Ok(true)
            }
            2 => Ok(false),
            version => anyhow::bail!("unsupported config version {version}"),
        }
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.version == 2,
            "unsupported config version {}",
            self.version
        );
        anyhow::ensure!(
            (1..=20).contains(&self.display.refresh_hz),
            "display.refresh_hz must be between 1 and 20"
        );
        anyhow::ensure!(
            (1..=3).contains(&self.display.rows),
            "display.rows must be between 1 and 3"
        );
        anyhow::ensure!(
            !self.display.segments.is_empty(),
            "display.segments must contain at least one segment"
        );
        let unique: std::collections::HashSet<_> = self.display.segments.iter().collect();
        anyhow::ensure!(
            unique.len() == self.display.segments.len(),
            "display.segments cannot contain duplicates"
        );
        anyhow::ensure!(
            !self.display.separator.chars().any(char::is_control),
            "display.separator cannot contain terminal control characters"
        );
        anyhow::ensure!(
            self.display.separator.chars().count() <= 8,
            "display.separator cannot be longer than 8 characters"
        );
        anyhow::ensure!(
            self.launch.bypass_flag == "--no-companion",
            "launch.bypass_flag must be --no-companion"
        );
        Ok(())
    }

    pub fn save_atomic(&self) -> Result<()> {
        self.validate()?;
        let path = path()?;
        let parent = path.parent().context("config path has no parent")?;
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
        let mut file = tempfile::NamedTempFile::new_in(parent).with_context(|| {
            format!("failed to create a temporary file in {}", parent.display())
        })?;
        use std::io::Write as _;
        file.write_all(toml::to_string_pretty(self)?.as_bytes())?;
        file.as_file().sync_all()?;
        file.persist(&path)
            .with_context(|| format!("failed to replace {}", path.display()))?;
        Ok(())
    }
}

pub fn path() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let project = ProjectDirs::from("dev", "codexline", "codexline")
            .context("could not resolve the user configuration directory")?;
        Ok(project.config_dir().join("config.toml"))
    }
    #[cfg(not(windows))]
    {
        if let Some(directory) = std::env::var_os("XDG_CONFIG_HOME") {
            return Ok(PathBuf::from(directory).join("codexline/config.toml"));
        }
        let home = directories::BaseDirs::new().context("could not resolve home directory")?;
        Ok(home.home_dir().join(".config/codexline/config.toml"))
    }
}

pub fn suggested_shim_path() -> Result<PathBuf> {
    #[cfg(windows)]
    {
        let project = ProjectDirs::from("dev", "codexline", "codexline")
            .context("could not resolve the user data directory")?;
        Ok(project.data_local_dir().join("bin").join("codex.exe"))
    }
    #[cfg(not(windows))]
    {
        let data_home = if let Some(directory) = std::env::var_os("XDG_DATA_HOME") {
            PathBuf::from(directory)
        } else {
            let home = directories::BaseDirs::new().context("could not resolve home directory")?;
            home.home_dir().join(".local/share")
        };
        Ok(data_home.join("codexline/bin/codex"))
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, LaunchMode};

    #[test]
    fn defaults_are_valid_and_keep_codex() {
        let config = Config::default();
        assert_eq!(config.launch.mode, LaunchMode::Shim);
        assert!(matches!(config.display.theme, super::Theme::PastelSyntax));
        assert!(config.sources.app_server);
        assert!(!config.sources.remote_proxy);
        config.validate().unwrap();
    }

    #[test]
    fn version_one_migrates_away_from_the_experimental_proxy() {
        let mut config = Config {
            version: 1,
            ..Config::default()
        };
        config.sources.remote_proxy = true;
        assert!(config.migrate().unwrap());
        assert_eq!(config.version, 2);
        assert!(!config.sources.remote_proxy);
        config.validate().unwrap();
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let config = Config {
            version: 3,
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }

    #[test]
    fn rejects_duplicate_segments_and_control_characters() {
        let mut config = Config::default();
        config.display.segments = vec![super::Segment::App, super::Segment::App];
        assert!(config.validate().is_err());

        let mut config = Config::default();
        config.display.separator = "\u{1b}".into();
        assert!(config.validate().is_err());
    }

    #[cfg(not(windows))]
    #[test]
    fn shim_uses_a_codexline_owned_directory() {
        let path = super::suggested_shim_path().unwrap();
        assert!(path.ends_with("codexline/bin/codex"));
        assert!(!path.ends_with(".local/bin/codex"));
    }
}
