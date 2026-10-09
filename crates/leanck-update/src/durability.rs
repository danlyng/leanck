//! When installation writes reach the disk.
//!
//! Every journal, backup, staged file, and state file is synced before the
//! step that depends on it, so a crash leaves either the old or the new
//! installation plus a journal that restores it. That is [`Durability::Full`],
//! the default, and the only setting outside tests.
//!
//! Tests apply many installations to throwaway directories, where each
//! `fsync` (`F_FULLFSYNC` on macOS) costs milliseconds and proves nothing a
//! test can observe. They opt into [`Durability::Relaxed`] for their own
//! thread with [`relax_for_this_thread`]; the ordering of writes, renames,
//! and journal updates is the same in both settings.
use std::{cell::Cell, fs::File, io, path::Path};

/// Whether writes are synced to disk.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Durability {
    /// Sync files and directories before each dependent step.
    #[default]
    Full,
    /// Skip syncing (tests only).
    #[cfg_attr(not(test), allow(dead_code))]
    Relaxed,
}

thread_local! {
    static CURRENT: Cell<Durability> = const { Cell::new(Durability::Full) };
}

fn current() -> Durability {
    CURRENT.with(Cell::get)
}

/// Skip syncing for the rest of this thread. Each test runs on its own
/// thread, so this never affects another test or a real installation.
#[cfg(test)]
pub(crate) fn relax_for_this_thread() {
    CURRENT.with(|current| current.set(Durability::Relaxed));
}

/// Sync an open file's contents and metadata.
pub(crate) fn sync_file(file: &File) -> io::Result<()> {
    match current() {
        Durability::Full => file.sync_all(),
        Durability::Relaxed => Ok(()),
    }
}

/// Sync a file or directory by path, such as a directory after a rename in it.
pub(crate) fn sync_path(path: &Path) -> io::Result<()> {
    match current() {
        Durability::Full => File::open(path)?.sync_all(),
        Durability::Relaxed => Ok(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn full_durability_is_the_default_and_relaxing_stays_on_this_thread() {
        assert_eq!(current(), Durability::Full);
        relax_for_this_thread();
        assert_eq!(current(), Durability::Relaxed);
        let other = std::thread::spawn(current).join().unwrap();
        assert_eq!(other, Durability::Full);
        // Relaxed syncs succeed without opening the path.
        assert!(sync_path(Path::new("/nonexistent/leanck")).is_ok());
    }
}
