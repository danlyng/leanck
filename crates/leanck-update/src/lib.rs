//! Foreground release checks and native, receipt-bound installation updates.
mod check;
mod durability;
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

/// Check a path the installer or updater reads, writes, or replaces.
///
/// `path` must be absolute, without `..`, and not itself a symlink. A
/// symlinked ancestor is followed only when the current user or root owns
/// the link: dotfile links such as `~/.claude -> dotfiles/claude` and system
/// aliases such as macOS `/var` and `/tmp`. A link someone else owns could
/// redirect writes outside the user's control, so it is refused.
pub(crate) fn safe_path(path: &Path) -> Result<()> {
    safe_dir(path)?;
    match fs::symlink_metadata(path) {
        Ok(meta) => ensure!(
            !meta.file_type().is_symlink(),
            "refusing symlink: {}",
            path.display()
        ),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

/// Check a directory that holds managed paths, such as a destination's
/// parent, the state directory, or a host home. Every component, `dir`
/// included, follows the ancestor rule of [`safe_path`].
pub(crate) fn safe_dir(dir: &Path) -> Result<()> {
    use std::os::unix::fs::MetadataExt;
    ensure!(
        dir.is_absolute(),
        "expected absolute path: {}",
        dir.display()
    );
    ensure!(
        !dir.components()
            .any(|c| matches!(c, std::path::Component::ParentDir)),
        "parent traversal in {}",
        dir.display()
    );
    for ancestor in dir.ancestors() {
        match fs::symlink_metadata(ancestor) {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink() || trusted_owner(meta.uid()),
                "refusing symlink owned by another user: {}",
                ancestor.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Check a path inside the managed tree `root`, such as the discovery marker
/// in the plugin bundle: [`safe_path`], and no symlink at or below `root`.
pub(crate) fn safe_path_within(path: &Path, root: &Path) -> Result<()> {
    safe_path(path)?;
    ensure!(
        path.starts_with(root),
        "{} is outside {}",
        path.display(),
        root.display()
    );
    for inside in path.ancestors().take_while(|a| a.starts_with(root)) {
        match fs::symlink_metadata(inside) {
            Ok(meta) => ensure!(
                !meta.file_type().is_symlink(),
                "refusing symlink in a managed tree: {}",
                inside.display()
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

/// Whether a link owned by `uid` may be followed: the current user's or root's.
fn trusted_owner(uid: u32) -> bool {
    // SAFETY: geteuid has no preconditions and cannot fail.
    let user = unsafe { libc::geteuid() };
    uid == user || uid == 0
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
    crate::durability::sync_file(file.as_file())?;
    file.persist(path).map_err(|e| e.error)?;
    crate::durability::sync_path(parent)?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::symlink;

    #[test]
    fn links_are_followed_only_above_a_destination_and_outside_managed_trees() {
        let (_dir, root) = test_support::scratch();
        // A dotfiles setup: ~/.claude is the user's own link.
        let dotfiles = root.join("dotfiles/claude");
        fs::create_dir_all(dotfiles.join("skills/leanck")).unwrap();
        let claude = root.join(".claude");
        symlink(&dotfiles, &claude).unwrap();
        assert!(safe_dir(&claude).is_ok());
        assert!(safe_dir(&claude.join("skills")).is_ok());
        assert!(safe_path(&claude.join("skills/leanck")).is_ok());
        assert!(safe_path(&claude.join("skills/new/SKILL.md")).is_ok());
        // The destination itself is never followed.
        let error = safe_path(&claude).unwrap_err().to_string();
        assert_eq!(error, format!("refusing symlink: {}", claude.display()));
        // Nor is a link anywhere inside a managed tree.
        let bundle = root.join("bundle");
        fs::create_dir_all(bundle.join("real")).unwrap();
        symlink(bundle.join("real"), bundle.join("plugin")).unwrap();
        let marker = bundle.join("plugin/.leanck-installation.json");
        assert!(safe_path(&marker).is_ok());
        assert_eq!(
            safe_path_within(&marker, &bundle).unwrap_err().to_string(),
            format!(
                "refusing symlink in a managed tree: {}",
                bundle.join("plugin").display()
            )
        );
        assert!(safe_path_within(&bundle.join("real/x"), &bundle).is_ok());
        // A managed tree reached through the user's own link is still checked below it.
        assert!(
            safe_path_within(
                &claude.join("skills/leanck/SKILL.md"),
                &claude.join("skills/leanck")
            )
            .is_ok()
        );
        assert!(safe_path(&root.join("a/../b")).is_err());
        assert!(safe_dir(Path::new("relative")).is_err());
    }

    #[test]
    fn only_links_owned_by_the_user_or_root_are_trusted() {
        // SAFETY: geteuid has no preconditions.
        let user = unsafe { libc::geteuid() };
        assert!(trusted_owner(user));
        assert!(trusted_owner(0));
        let stranger = if user == 4242 { 4243 } else { 4242 };
        assert!(!trusted_owner(stranger));
        // System aliases such as macOS /tmp -> private/tmp belong to root.
        if fs::symlink_metadata("/tmp").is_ok_and(|m| m.file_type().is_symlink()) {
            assert!(safe_dir(Path::new("/tmp/leanck-state")).is_ok());
        }
    }
}
