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
    pub display: DisplayConfig,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            version: 1,
            launch: LaunchConfig::default(),
            display: DisplayConfig::default(),
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
}

impl Default for DisplayConfig {
    fn default() -> Self {
        Self {
            theme: Theme::CodexDark,
            glyphs: Glyphs::Unicode,
            refresh_hz: 8,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Theme {
    #[default]
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
        let config: Self = toml::from_str(&source)
            .with_context(|| format!("invalid configuration in {}", path.display()))?;
        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<()> {
        anyhow::ensure!(
            self.version == 1,
            "unsupported config version {}",
            self.version
        );
        anyhow::ensure!(
            (1..=20).contains(&self.display.refresh_hz),
            "display.refresh_hz must be between 1 and 20"
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
        let home = directories::BaseDirs::new().context("could not resolve home directory")?;
        Ok(home.home_dir().join(".local/bin/codex"))
    }
}

#[cfg(test)]
mod tests {
    use super::{Config, LaunchMode};

    #[test]
    fn defaults_are_valid_and_keep_codex() {
        let config = Config::default();
        assert_eq!(config.launch.mode, LaunchMode::Shim);
        config.validate().unwrap();
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let config = Config {
            version: 2,
            ..Config::default()
        };
        assert!(config.validate().is_err());
    }
}
