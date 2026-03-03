//! Runtime location and workspace hash. Computing paths never creates directories.
use blake2::{Blake2b, Digest, digest::consts::U8};
use std::path::{Path, PathBuf};

/// `$XDG_CACHE_HOME/leanck`, `$HOME/.cache/leanck`, otherwise the system temp directory.
pub fn cache_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_CACHE_HOME")
        && !dir.is_empty()
    {
        PathBuf::from(dir).join("leanck")
    } else if let Some(home) = std::env::home_dir() {
        home.join(".cache/leanck")
    } else {
        std::env::temp_dir()
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
}

impl RuntimePaths {
    pub fn in_directory(directory: PathBuf, worktree: &Path) -> Self {
        Self {
            directory,
            key: worktree_key(worktree),
        }
    }

    /// Discover an existing legacy temp-directory fallback as well as the preferred location.
    pub fn for_workspace(worktree: &Path) -> Self {
        let preferred = Self::in_directory(cache_dir(), worktree);
        let fallback = Self::in_directory(std::env::temp_dir(), worktree);
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
        };
        std::fs::create_dir_all(&fallback.directory)?;
        Ok(fallback)
    }

    pub fn directory(&self) -> &Path {
        &self.directory
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
