//! Runtime location and workspace hash. Computing paths never creates directories.
use blake2::{Blake2b, Digest, digest::consts::U8};
use std::path::{Path, PathBuf};

/// `$XDG_CACHE_HOME/leanck`, `$HOME/.cache/leanck`, otherwise the system temp directory.
pub fn cache_dir() -> PathBuf {
    user_cache_dir().unwrap_or_else(std::env::temp_dir)
}

/// `$XDG_CACHE_HOME/leanck` or `$HOME/.cache/leanck`; `None` when this user has
/// neither, so the only candidate would be the shared system temp directory.
fn user_cache_dir() -> Option<PathBuf> {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME")
        && !dir.is_empty()
    {
        Some(PathBuf::from(dir).join("leanck"))
    } else {
        std::env::home_dir().map(|home| home.join(".cache/leanck"))
    }
}

/// Preserve the existing eight-byte Blake2b digest of canonical platform path bytes.
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
    /// The directory is the legacy system temp fallback rather than the selected location.
    fallback: bool,
}

impl RuntimePaths {
    pub fn in_directory(directory: PathBuf, worktree: &Path) -> Self {
        Self {
            directory,
            key: worktree_key(worktree),
            fallback: false,
        }
    }

    /// Discover an existing legacy temp-directory fallback as well as the
    /// preferred location. Without a per-user cache directory, the system temp
    /// directory is the only location, and it is a fallback.
    pub fn for_workspace(worktree: &Path) -> Self {
        Self::select(user_cache_dir(), worktree)
    }

    fn select(user_cache: Option<PathBuf>, worktree: &Path) -> Self {
        let fallback = Self {
            fallback: true,
            ..Self::in_directory(std::env::temp_dir(), worktree)
        };
        let Some(directory) = user_cache else {
            return fallback;
        };
        let preferred = Self::in_directory(directory, worktree);
        if !preferred.socket().exists()
            && !preferred.pid().exists()
            && (fallback.socket().exists() || fallback.pid().exists())
        {
            fallback
        } else {
            preferred
        }
    }

    /// Explicit startup preparation. Retain the legacy temp fallback if mkdir fails.
    pub fn prepare(self) -> std::io::Result<Self> {
        if std::fs::create_dir_all(&self.directory).is_ok() {
            return Ok(self);
        }
        let fallback = Self {
            directory: std::env::temp_dir(),
            key: self.key,
            fallback: true,
        };
        std::fs::create_dir_all(&fallback.directory)?;
        Ok(fallback)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }
    /// Whether these paths use the legacy system temp fallback. Fallback runtimes
    /// take no instance slot: the slot directory is `directory()` only when this is false.
    pub fn is_fallback(&self) -> bool {
        self.fallback
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
    /// Held by the client that is starting this worktree's daemon, so
    /// concurrent clients make room and spawn once.
    pub fn start_lock(&self) -> PathBuf {
        self.artifact("start.lock")
    }
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

    #[test]
    fn without_a_user_cache_directory_the_temp_runtime_is_a_fallback() {
        let worktree = std::env::temp_dir();
        let shared = RuntimePaths::select(None, &worktree);
        assert!(shared.is_fallback());
        assert_eq!(shared.directory(), std::env::temp_dir());
        let missing = std::env::temp_dir().join(format!("leanck-paths-{}", std::process::id()));
        let preferred = RuntimePaths::select(Some(missing.clone()), &worktree);
        assert!(!preferred.is_fallback());
        assert_eq!(preferred.directory(), missing);
    }
}
