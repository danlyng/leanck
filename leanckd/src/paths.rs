//! Deterministic per-worktree paths for the socket, PID file, and lock file.
//!
//! Both the daemon and the `leanck-mcp` client derive these identically from
//! the canonical worktree path, so every agent pointed at the same worktree
//! converges on the same socket (one shared daemon — the whole point).

use std::path::{Path, PathBuf};

use sha1::{Digest, Sha1};

/// Directory holding the socket/pid/lock files. `$XDG_RUNTIME_DIR` if set,
/// else the system temp dir.
pub fn runtime_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR") {
        let p = PathBuf::from(dir);
        if !p.as_os_str().is_empty() {
            return p;
        }
    }
    std::env::temp_dir()
}

/// 16-hex-char key for `worktree` (first 8 bytes of its SHA-1). Short on
/// purpose: the full socket path must fit in `sockaddr_un.sun_path` (≤104
/// bytes on macOS).
pub fn worktree_key(worktree: &Path) -> String {
    let canonical = worktree
        .canonicalize()
        .unwrap_or_else(|_| worktree.to_path_buf());
    let mut hasher = Sha1::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let digest = hasher.finalize();
    let mut s = String::with_capacity(16);
    for b in digest.iter().take(8) {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Unix socket path the daemon binds and clients connect to.
pub fn socket_path(worktree: &Path) -> PathBuf {
    runtime_dir().join(format!("leanckd-{}.sock", worktree_key(worktree)))
}

/// PID file recording the running daemon's process id.
pub fn pid_path(worktree: &Path) -> PathBuf {
    runtime_dir().join(format!("leanckd-{}.pid", worktree_key(worktree)))
}

/// Advisory lock file serializing single-instance startup across processes.
pub fn lock_path(worktree: &Path) -> PathBuf {
    runtime_dir().join(format!("leanckd-{}.lock", worktree_key(worktree)))
}

/// Log file the detached daemon redirects stdout/stderr to.
pub fn log_path(worktree: &Path) -> PathBuf {
    runtime_dir().join(format!("leanckd-{}.log", worktree_key(worktree)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn key_is_deterministic_and_worktree_specific() {
        let a1 = worktree_key(Path::new("/tmp/leanckd-ws-a"));
        let a2 = worktree_key(Path::new("/tmp/leanckd-ws-a"));
        let b = worktree_key(Path::new("/tmp/leanckd-ws-b"));
        assert_eq!(a1, a2);
        assert_ne!(a1, b);
        assert_eq!(a1.len(), 16);
    }

    #[test]
    fn socket_name_is_short_and_suffixed() {
        let p = socket_path(Path::new("/some/deep/worktree/path"));
        let name = p.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with("leanckd-"));
        assert!(name.ends_with(".sock"));
        // socket filename must stay well under the sun_path limit.
        assert!(name.len() < 40, "socket name too long: {name}");
    }
}
