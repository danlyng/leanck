//! Foreground release checks and native, receipt-bound installation updates.
mod check;
pub mod gate;
pub mod install;
pub mod receipt;
#[cfg(test)]
mod test_support;
mod transaction;

use anyhow::{Context, Result, ensure};
pub use check::{Checker, ReleaseSource, UpdateStatus, current_status};
use serde::Serialize;
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

pub fn state_directory() -> Result<PathBuf> {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/state")))
        .context("set HOME or an absolute XDG_STATE_HOME for update state")?;
    Ok(base.join("leanck"))
}

pub(crate) fn safe_path(path: &Path) -> Result<()> {
    ensure!(
        path.is_absolute(),
        "expected absolute path: {}",
        path.display()
    );
    ensure!(
        !path
            .components()
            .any(|c| matches!(c, std::path::Component::ParentDir)),
        "parent traversal in {}",
        path.display()
    );
    for parent in path.ancestors() {
        match fs::symlink_metadata(parent) {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink() || system_alias(parent),
                "refusing symlink: {}",
                parent.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn system_alias(path: &Path) -> bool {
    matches!((path.to_str(), path.canonicalize().ok().as_deref()),
        (Some("/var"), Some(p)) if p == Path::new("/private/var"))
        || matches!((path.to_str(), path.canonicalize().ok().as_deref()),
        (Some("/tmp"), Some(p)) if p == Path::new("/private/tmp"))
}
#[cfg(not(target_os = "macos"))]
fn system_alias(_path: &Path) -> bool {
    false
}

pub(crate) fn lock_file(path: &Path) -> Result<File> {
    safe_path(path)?;
    fs::create_dir_all(path.parent().context("lock needs a parent directory")?)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    Ok(options.open(path)?)
}

pub(crate) fn atomic_bytes(path: &Path, bytes: &[u8]) -> Result<()> {
    safe_path(path)?;
    let parent = path.parent().context("state needs a parent directory")?;
    fs::create_dir_all(parent)?;
    let mut file = tempfile::NamedTempFile::new_in(parent)?;
    file.write_all(bytes)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    File::open(parent)?.sync_all()?;
    Ok(())
}

pub(crate) fn atomic_json(path: &Path, value: &impl Serialize) -> Result<()> {
    atomic_bytes(path, &serde_json::to_vec_pretty(value)?)
}

pub(crate) fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
