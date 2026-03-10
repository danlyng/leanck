//! Runtime discovery is read-only; preparation validates before Lean is started.
use blake2::{Blake2b, Digest, digest::consts::U8};
use std::{
    fs::{File, OpenOptions, TryLockError},
    io,
    os::unix::{
        fs::{DirBuilderExt, MetadataExt, OpenOptionsExt},
        net::UnixListener,
    },
    path::{Path, PathBuf},
    sync::atomic::{AtomicU32, Ordering},
};

/// Effective cache location (runtime sockets prefer `XDG_RUNTIME_DIR`).
pub fn cache_dir() -> PathBuf {
    user_cache_dir().unwrap_or_else(|| private_temp(&std::env::temp_dir()))
}

fn absolute_env(name: &str) -> Option<PathBuf> {
    std::env::var_os(name)
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
}

fn user_cache_dir() -> Option<PathBuf> {
    absolute_env("XDG_CACHE_HOME")
        .or_else(|| {
            std::env::home_dir()
                .filter(|p| p.is_absolute())
                .map(|p| p.join(".cache"))
        })
        .map(|p| p.join("leanck"))
}

fn private_temp(base: &Path) -> PathBuf {
    // SAFETY: geteuid has no preconditions.
    base.join(format!("leanck-{}", unsafe { libc::geteuid() }))
}

/// Candidate runtime/slot directories, in preference order. Never creates them.
/// Relative XDG values are ignored. An existing XDG runtime base must be private.
pub fn runtime_directories() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Some(base) = absolute_env("XDG_RUNTIME_DIR")
        && valid_directory(&base, true).is_ok()
    {
        dirs.push(base.join("leanck"));
    }
    dirs.extend(user_cache_dir());
    dirs.push(private_temp(&std::env::temp_dir()));
    dirs.push(private_temp(Path::new("/tmp")));
    deduplicate(dirs)
}

fn deduplicate(dirs: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut seen = std::collections::HashSet::new();
    dirs.into_iter()
        .filter(|p| seen.insert(p.canonicalize().unwrap_or_else(|_| p.clone())))
        .collect()
}

/// Existing, safe directories in a canonical lock order. Aliases are locked once.
pub fn coordination_directories(dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut result: Vec<_> = dirs
        .iter()
        .filter(|p| valid_directory(p, false).is_ok())
        .filter_map(|p| p.canonicalize().ok())
        .collect();
    result.sort();
    result.dedup();
    result
}

fn valid_directory(path: &Path, private: bool) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory must be absolute",
        ));
    }
    let meta = std::fs::symlink_metadata(path)?;
    // Existing cache directories may be readable by others, but never writable.
    let private = private || path.file_name() == private_temp(Path::new("/tmp")).file_name();
    let forbidden = if private { 0o077 } else { 0o022 };
    if !meta.is_dir()
        || meta.uid() != unsafe { libc::geteuid() }
        || meta.mode() & forbidden != 0
        || meta.mode() & 0o700 != 0o700
    {
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "directory must be owned by this user, not a symlink, and have safe permissions",
        ));
    }
    Ok(())
}

fn prepare_directory(path: &Path) -> io::Result<()> {
    if !path.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "directory must be absolute",
        ));
    }
    std::fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(path)?;
    valid_directory(path, false)
}

pub fn worktree_key(worktree: &Path) -> String {
    let canonical = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    hex::encode(Blake2b::<U8>::digest(
        canonical.as_os_str().as_encoded_bytes(),
    ))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuntimePaths {
    directory: PathBuf,
    key: String,
    candidates: Vec<PathBuf>,
}

impl RuntimePaths {
    /// Explicit location, useful to library callers and isolated tests.
    pub fn in_directory(directory: PathBuf, worktree: &Path) -> Self {
        Self {
            candidates: vec![directory.clone()],
            directory,
            key: worktree_key(worktree),
        }
    }

    pub fn for_workspace(worktree: &Path) -> Self {
        Self::select(runtime_directories(), worktree)
    }

    fn select(candidates: Vec<PathBuf>, worktree: &Path) -> Self {
        let key = worktree_key(worktree);
        let mut search = candidates.clone();
        // Older versions used the shared temporary directory directly. Discover
        // their live sockets/locks, but never start a new daemon there.
        search.push(std::env::temp_dir());
        let active = search.iter().find_map(|dir| active_runtime(dir, &key));
        let directory = active
            .or_else(|| {
                candidates
                    .iter()
                    .find(|dir| {
                        socket_length(&dir.join(format!("leanck-{key}.sock"))).is_ok()
                            && (!dir.exists() || valid_directory(dir, false).is_ok())
                    })
                    .cloned()
            })
            .unwrap_or_else(|| candidates[0].clone());
        Self {
            directory,
            key,
            candidates,
        }
    }

    /// Recheck locations after waiting for another client to finish startup.
    pub fn rediscover(&self, worktree: &Path) -> Self {
        let selected = Self::select(self.candidates.clone(), worktree);
        if active_runtime(selected.directory(), &selected.key).is_some() {
            selected
        } else {
            self.clone()
        }
    }

    /// Pin a launcher's prepared directory while retaining the shared inventory.
    /// This explicit handoff must not silently select a different fallback.
    pub fn selected(directory: PathBuf, worktree: &Path) -> io::Result<Self> {
        let mut runtime = Self::for_workspace(worktree);
        runtime.directory = directory;
        runtime.validate()?;
        if !runtime.candidates.contains(&runtime.directory) {
            runtime.candidates.push(runtime.directory.clone());
        }
        Ok(runtime)
    }

    pub fn prepare(self) -> io::Result<Self> {
        // A live daemon must never be bypassed just because another candidate
        // has become available since it started.
        if let Some(directory) = active_runtime(&self.directory, &self.key) {
            return Ok(Self { directory, ..self });
        }
        let mut candidates = vec![self.directory.clone()];
        candidates.extend(self.candidates.clone());
        candidates.push(private_temp(&std::env::temp_dir()));
        candidates.push(private_temp(Path::new("/tmp")));
        let mut failures = Vec::new();
        for directory in deduplicate(candidates) {
            let mut runtime = self.clone();
            runtime.directory = directory;
            match runtime.validate() {
                Ok(()) => {
                    if !runtime.candidates.contains(&runtime.directory) {
                        runtime.candidates.push(runtime.directory.clone());
                    }
                    return Ok(runtime);
                }
                Err(error) => failures.push(format!("{}: {error}", runtime.directory.display())),
            }
        }
        Err(io::Error::other(format!(
            "no usable leanck runtime directory; {}",
            failures.join("; ")
        )))
    }

    fn validate(&self) -> io::Result<()> {
        socket_length(&self.socket())?;
        prepare_directory(&self.directory)?;
        probe(&self.directory)
    }

    /// A warming daemon holds its lifetime lock before publishing its socket.
    pub fn has_owner(&self) -> bool {
        OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(self.lock())
            .is_ok_and(|file| {
                file.metadata()
                    .is_ok_and(|m| m.uid() == unsafe { libc::geteuid() })
                    && matches!(file.try_lock(), Err(TryLockError::WouldBlock))
            })
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
    /// Includes other candidate roots so a fallback never escapes accounting.
    pub fn accounting_directories(&self) -> Vec<PathBuf> {
        let mut dirs = self.candidates.clone();
        dirs.push(self.directory.clone());
        coordination_directories(&dirs)
    }
    /// Lock every existing candidate, including the former shared-temp location,
    /// so a daemon in an older location cannot be duplicated during migration.
    pub fn lock_paths(&self, extension: &str) -> Vec<PathBuf> {
        let mut dirs = self.accounting_directories();
        // Even if permissions changed after preparation, never silently omit
        // the selected directory's mandatory lifetime/start lock.
        dirs.push(
            self.directory
                .canonicalize()
                .unwrap_or_else(|_| self.directory.clone()),
        );
        let legacy = std::env::temp_dir().join(format!("leanck-{}.{}", self.key, extension));
        let mut paths: Vec<_> = dirs
            .drain(..)
            .map(|dir| dir.join(format!("leanck-{}.{}", self.key, extension)))
            .collect();
        if legacy.exists() {
            paths.push(legacy);
        }
        paths.sort();
        paths.dedup();
        paths
    }
    fn artifact(&self, extension: &str) -> PathBuf {
        self.directory
            .join(format!("leanck-{}.{}", self.key, extension))
    }
    pub fn socket(&self) -> PathBuf {
        self.artifact("sock")
    }
    pub fn pid(&self) -> PathBuf {
        self.artifact("pid")
    }
    pub fn lock(&self) -> PathBuf {
        self.artifact("lock")
    }
    pub fn log(&self) -> PathBuf {
        self.artifact("log")
    }
    pub fn start_lock(&self) -> PathBuf {
        self.artifact("start.lock")
    }
}

fn active_runtime(directory: &Path, key: &str) -> Option<PathBuf> {
    let socket = directory.join(format!("leanck-{key}.sock"));
    let lock = directory.join(format!("leanck-{key}.lock"));
    if !lock.exists()
        && std::fs::symlink_metadata(&socket).is_ok_and(|m| {
            use std::os::unix::fs::FileTypeExt;
            m.file_type().is_socket() && m.uid() == unsafe { libc::geteuid() }
        })
    {
        return Some(directory.to_path_buf());
    }
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(&lock)
        .ok()?;
    if file.metadata().ok()?.uid() != unsafe { libc::geteuid() } {
        return None;
    }
    if !matches!(file.try_lock(), Err(TryLockError::WouldBlock)) {
        return None;
    }
    // New daemons publish the selected directory in every lifetime lock they
    // hold. Older daemons leave it empty and used the lock's own directory.
    Some(
        std::fs::read_to_string(lock)
            .ok()
            .and_then(|s| serde_json::from_str::<PathBuf>(&s).ok())
            .filter(|p| p.is_absolute())
            .unwrap_or_else(|| directory.to_path_buf()),
    )
}

fn socket_length(path: &Path) -> io::Result<()> {
    // SAFETY: sockaddr_un is a plain C structure; inspect only its array length.
    let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
    let length = path.as_os_str().as_encoded_bytes().len();
    if length >= address.sun_path.len() {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "socket path is {length} bytes; maximum is {}",
                address.sun_path.len() - 1
            ),
        ))
    } else {
        Ok(())
    }
}

fn probe(directory: &Path) -> io::Result<()> {
    static NEXT: AtomicU32 = AtomicU32::new(0);
    let name = format!(
        ".p{:x}{:x}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    );
    let path = directory.join(name);
    let file = OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    struct Cleanup(PathBuf);
    impl Drop for Cleanup {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
        }
    }
    let _cleanup = Cleanup(path.clone());
    file.try_lock().map_err(io::Error::other)?;
    let other = OpenOptions::new().read(true).write(true).open(&path)?;
    if !matches!(other.try_lock(), Err(TryLockError::WouldBlock)) {
        return Err(io::Error::other("filesystem does not enforce file locks"));
    }
    drop((other, file));
    std::fs::remove_file(&path)?;
    let _listener = UnixListener::bind(&path)?;
    Ok(())
}

/// A failed secondary coordination location may be skipped only when it has
/// no live slot records. The selected directory is always mandatory.
pub fn inactive_secondary(path: &Path, selected: &Path) -> bool {
    let Some(directory) = path.parent() else {
        return false;
    };
    let canonical = |p: &Path| p.canonicalize().unwrap_or_else(|_| p.to_path_buf());
    canonical(directory) != canonical(selected) && crate::instances::live(directory).is_empty()
}

/// Open a persistent coordination file without following a preexisting symlink.
pub fn open_lock(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

pub fn socket_path(worktree: &Path) -> PathBuf {
    RuntimePaths::for_workspace(worktree).socket()
}
pub fn pid_path(worktree: &Path) -> PathBuf {
    RuntimePaths::for_workspace(worktree).pid()
}
pub fn lock_path(worktree: &Path) -> PathBuf {
    RuntimePaths::for_workspace(worktree).lock()
}
pub fn log_path(worktree: &Path) -> PathBuf {
    RuntimePaths::for_workspace(worktree).log()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU32 = AtomicU32::new(0);
            let path = PathBuf::from("/tmp").join(format!(
                "lcp-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            prepare_directory(&path).unwrap();
            Self(path)
        }
        fn select(&self, roots: Vec<PathBuf>) -> RuntimePaths {
            RuntimePaths::select(roots, &self.0)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn selection_is_read_only_and_preparation_creates_private_directory_without_probes_left_behind()
    {
        let s = Scratch::new();
        let preferred = s.0.join("runtime");
        let cache = s.0.join("cache");
        let runtime = s.select(vec![preferred.clone(), cache.clone()]);
        assert_eq!(runtime.directory(), preferred);
        assert!(!preferred.exists());
        let prepared = runtime.prepare().unwrap();
        assert_eq!(
            std::fs::metadata(prepared.directory()).unwrap().mode() & 0o777,
            0o700
        );
        assert_eq!(std::fs::read_dir(prepared.directory()).unwrap().count(), 0);
        assert!(!cache.exists());
    }

    #[test]
    fn long_unwritable_relative_and_symlink_candidates_fall_back_before_startup() {
        let s = Scratch::new();
        let long = s.0.join("x".repeat(120));
        let blocked = s.0.join("blocked");
        std::fs::write(&blocked, "not a directory").unwrap();
        let readonly = s.0.join("readonly");
        prepare_directory(&readonly).unwrap();
        std::fs::set_permissions(&readonly, std::fs::Permissions::from_mode(0o500)).unwrap();
        let alias = s.0.join("alias");
        std::os::unix::fs::symlink(&s.0, &alias).unwrap();
        let good = s.0.join("good");
        for bad in [
            long.clone(),
            blocked.join("child"),
            readonly.clone(),
            alias,
            PathBuf::from("relative"),
        ] {
            let runtime = s.select(vec![bad, good.clone()]).prepare().unwrap();
            assert_eq!(runtime.directory(), good);
        }
        assert!(
            !long.exists(),
            "path-length rejection precedes directory creation"
        );
        std::fs::set_permissions(readonly, std::fs::Permissions::from_mode(0o700)).unwrap();
    }

    #[test]
    fn xdg_runtime_base_requires_private_user_owned_directory() {
        let s = Scratch::new();
        assert!(valid_directory(&s.0, true).is_ok());
        std::fs::set_permissions(&s.0, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(valid_directory(&s.0, true).is_err());
        assert!(
            valid_directory(&s.0, false).is_ok(),
            "legacy cache permissions remain usable"
        );
        std::fs::set_permissions(&s.0, std::fs::Permissions::from_mode(0o777)).unwrap();
        assert!(valid_directory(&s.0, false).is_err());
    }

    #[test]
    fn socket_limit_counts_bytes_and_reserves_terminator() {
        let address: libc::sockaddr_un = unsafe { std::mem::zeroed() };
        assert!(socket_length(Path::new(&"x".repeat(address.sun_path.len() - 1))).is_ok());
        assert!(socket_length(Path::new(&"x".repeat(address.sun_path.len()))).is_err());
        assert!(socket_length(Path::new(&"é".repeat(address.sun_path.len() / 2))).is_err());
    }

    #[test]
    fn held_migration_lock_points_to_actual_socket_and_stale_artifacts_do_not_pin_selection() {
        use std::io::Write;
        let s = Scratch::new();
        let first = s.0.join("first");
        let second = s.0.join("second");
        prepare_directory(&first).unwrap();
        prepare_directory(&second).unwrap();
        let runtime = RuntimePaths::in_directory(first.clone(), &s.0);
        let mut file = open_lock(&runtime.lock()).unwrap();
        file.try_lock().unwrap();
        serde_json::to_writer(&mut file, &second).unwrap();
        file.flush().unwrap();
        let selected = s.select(vec![first.clone(), second.clone()]);
        assert_eq!(selected.directory(), second);
        drop(file);
        std::fs::write(runtime.pid(), std::process::id().to_string()).unwrap();
        let stale_socket = UnixListener::bind(runtime.socket()).unwrap();
        drop(stale_socket);
        let selected = s.select(vec![second.clone(), first]);
        assert_eq!(
            selected.directory(),
            second,
            "stale pid and socket do not override preference"
        );
    }

    #[test]
    fn explicit_handoff_rejects_invalid_directory_instead_of_silently_reselecting() {
        let s = Scratch::new();
        assert!(RuntimePaths::selected(s.0.join("x".repeat(120)), &s.0).is_err());
    }
}
