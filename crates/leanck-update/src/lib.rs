//! Foreground release checks and native, receipt-bound installation updates.
mod check;
pub mod gate;
pub mod install;
pub mod installer;
pub mod paths;
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

/// `$XDG_STATE_HOME/leanck`, defaulting to `~/.local/state/leanck`.
///
/// A relative `XDG_STATE_HOME` resolves against the working directory, as the
/// installer resolves it, so both agree on the installer state directory.
pub fn state_directory() -> Result<PathBuf> {
    let home = std::env::var("HOME")
        .ok()
        .filter(|home| !home.is_empty())
        .map(PathBuf::from);
    let base = match std::env::var("XDG_STATE_HOME")
        .ok()
        .filter(|value| !value.is_empty())
    {
        Some(value) => paths::absolute(
            &value,
            home.as_deref().unwrap_or(Path::new("/")),
            &std::env::current_dir()?,
        )?,
        None => home
            .context("set HOME or XDG_STATE_HOME for update state")?
            .join(".local/state"),
    };
    Ok(base.join("leanck"))
}

/// Parse the first token of a `.sha256` sidecar as a SHA-256 digest, in any case.
pub fn parse_sha256(sidecar: &str) -> Result<String> {
    let digest = sidecar.split_whitespace().next().unwrap_or_default();
    ensure!(
        digest.len() == 64 && digest.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid SHA-256 checksum"
    );
    Ok(digest.to_ascii_lowercase())
}

/// Hex digests compare case-insensitively: sidecars may use upper case.
pub fn checksum_matches(expected: &str, actual: &str) -> bool {
    expected.len() == actual.len() && expected.eq_ignore_ascii_case(actual)
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
