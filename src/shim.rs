use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::config::{self, LaunchMode};

const OWNER_MARKER: &str = "codexline-shim-v1\n";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShimOutcome {
    Installed(PathBuf),
    Removed(PathBuf),
    Unchanged,
}

pub fn reconcile(mode: LaunchMode) -> Result<ShimOutcome> {
    let executable =
        std::env::current_exe().context("could not resolve the running Codexline executable")?;
    reconcile_at(mode, &config::suggested_shim_path()?, &executable)
}

fn reconcile_at(mode: LaunchMode, shim: &Path, executable: &Path) -> Result<ShimOutcome> {
    match mode {
        LaunchMode::Shim => install(shim, executable),
        LaunchMode::Explicit => remove(shim, executable),
    }
}

fn marker_path(shim: &Path) -> PathBuf {
    shim.with_file_name(format!(
        ".{}.codexline-owned",
        shim.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("codex")
    ))
}

fn marker_is_owned(shim: &Path) -> bool {
    fs::read_to_string(marker_path(shim)).is_ok_and(|value| value == OWNER_MARKER)
}

#[cfg(unix)]
fn points_to_executable(shim: &Path, executable: &Path) -> bool {
    fs::canonicalize(shim).ok() == fs::canonicalize(executable).ok()
}

fn write_marker(shim: &Path) -> Result<()> {
    let marker = marker_path(shim);
    fs::write(&marker, OWNER_MARKER)
        .with_context(|| format!("failed to write shim ownership marker {}", marker.display()))
}

#[cfg(unix)]
fn install(shim: &Path, executable: &Path) -> Result<ShimOutcome> {
    use std::os::unix::fs::symlink;

    if let Ok(metadata) = fs::symlink_metadata(shim) {
        anyhow::ensure!(
            metadata.file_type().is_symlink()
                && (marker_is_owned(shim) || points_to_executable(shim, executable)),
            "refusing to replace unrelated file at {}",
            shim.display()
        );
    }
    let parent = shim.parent().context("shim path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let pending = parent.join(format!(".codex.codexline-{}.new", std::process::id()));
    if pending.exists() {
        fs::remove_file(&pending)
            .with_context(|| format!("failed to clear stale {}", pending.display()))?;
    }
    symlink(executable, &pending)
        .with_context(|| format!("failed to create temporary shim {}", pending.display()))?;
    fs::rename(&pending, shim)
        .with_context(|| format!("failed to install shim {}", shim.display()))?;
    write_marker(shim)?;
    Ok(ShimOutcome::Installed(shim.to_path_buf()))
}

#[cfg(windows)]
fn install(shim: &Path, executable: &Path) -> Result<ShimOutcome> {
    if shim.exists() {
        anyhow::ensure!(
            marker_is_owned(shim),
            "refusing to replace unrelated file at {}",
            shim.display()
        );
    }
    let parent = shim.parent().context("shim path has no parent")?;
    fs::create_dir_all(parent).with_context(|| format!("failed to create {}", parent.display()))?;
    let pending = shim.with_extension("exe.new");
    fs::copy(executable, &pending)
        .with_context(|| format!("failed to stage shim {}", pending.display()))?;
    if shim.exists() {
        fs::remove_file(shim)
            .with_context(|| format!("failed to replace shim {}", shim.display()))?;
    }
    fs::rename(&pending, shim)
        .with_context(|| format!("failed to install shim {}", shim.display()))?;
    write_marker(shim)?;
    Ok(ShimOutcome::Installed(shim.to_path_buf()))
}

fn remove(shim: &Path, _executable: &Path) -> Result<ShimOutcome> {
    if !shim.exists() && fs::symlink_metadata(shim).is_err() {
        return Ok(ShimOutcome::Unchanged);
    }
    #[cfg(unix)]
    let owned = marker_is_owned(shim) || points_to_executable(shim, _executable);
    #[cfg(windows)]
    let owned = marker_is_owned(shim);
    anyhow::ensure!(
        owned,
        "refusing to remove unrelated file at {}",
        shim.display()
    );
    fs::remove_file(shim).with_context(|| format!("failed to remove {}", shim.display()))?;
    let marker = marker_path(shim);
    if marker.exists() {
        fs::remove_file(&marker)
            .with_context(|| format!("failed to remove {}", marker.display()))?;
    }
    Ok(ShimOutcome::Removed(shim.to_path_buf()))
}

#[cfg(test)]
mod tests {
    use super::reconcile_at;
    #[cfg(unix)]
    use super::{ShimOutcome, marker_path};
    use crate::config::LaunchMode;
    use std::fs;

    #[cfg(unix)]
    #[test]
    fn shim_install_and_remove_are_owned_and_reversible() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("codexline");
        fs::write(&executable, "binary").unwrap();
        let shim = directory.path().join("owned/bin/codex");

        assert_eq!(
            reconcile_at(LaunchMode::Shim, &shim, &executable).unwrap(),
            ShimOutcome::Installed(shim.clone())
        );
        assert_eq!(fs::read_link(&shim).unwrap(), executable);
        assert!(marker_path(&shim).exists());
        fs::remove_file(marker_path(&shim)).unwrap();
        assert_eq!(
            reconcile_at(LaunchMode::Shim, &shim, &executable).unwrap(),
            ShimOutcome::Installed(shim.clone())
        );
        assert_eq!(
            reconcile_at(LaunchMode::Explicit, &shim, directory.path()).unwrap(),
            ShimOutcome::Removed(shim.clone())
        );
        assert!(!shim.exists());
        assert!(!marker_path(&shim).exists());
    }

    #[test]
    fn shim_refuses_an_unrelated_file() {
        let directory = tempfile::tempdir().unwrap();
        let executable = directory.path().join("codexline");
        let shim = directory.path().join("codex");
        fs::write(&executable, "binary").unwrap();
        fs::write(&shim, "official codex").unwrap();
        let error = reconcile_at(LaunchMode::Shim, &shim, &executable).unwrap_err();
        assert!(error.to_string().contains("unrelated file"));
    }
}
