use std::path::{Path, PathBuf};

use anyhow::Context;

pub(crate) fn detect_workspace() -> anyhow::Result<PathBuf> {
    let cwd = std::env::current_dir().context("reading current directory")?;
    Ok(find_project_root(&cwd).unwrap_or(cwd))
}

pub(crate) fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if dir.join("lakefile.lean").exists()
            || dir.join("lakefile.toml").exists()
            || dir.join("lean-toolchain").exists()
        {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Unique scratch directory under the system temp dir, removed by the
    /// caller. Avoids pulling in a tempfile dev-dependency.
    fn scratch(tag: &str) -> PathBuf {
        use std::sync::atomic::{AtomicU32, Ordering};
        static N: AtomicU32 = AtomicU32::new(0);
        let n = N.fetch_add(1, Ordering::Relaxed);
        let p = std::env::temp_dir().join(format!("leanck-ws-{}-{tag}-{n}", std::process::id()));
        std::fs::create_dir_all(&p).unwrap();
        p
    }

    #[test]
    fn finds_root_via_lakefile_toml() {
        let root = scratch("toml");
        std::fs::write(root.join("lakefile.toml"), b"name = \"x\"").unwrap();
        assert_eq!(find_project_root(&root).as_deref(), Some(root.as_path()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn finds_root_by_walking_up_from_a_subdirectory() {
        let root = scratch("walk");
        std::fs::write(root.join("lean-toolchain"), b"leanprover/lean4:v4.0.0").unwrap();
        let nested = root.join("a/b/c");
        std::fs::create_dir_all(&nested).unwrap();
        assert_eq!(find_project_root(&nested).as_deref(), Some(root.as_path()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn recognizes_lakefile_lean_marker() {
        let root = scratch("lean");
        std::fs::write(root.join("lakefile.lean"), b"").unwrap();
        assert_eq!(find_project_root(&root).as_deref(), Some(root.as_path()));
        std::fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn returns_none_when_no_marker_is_present() {
        let root = scratch("bare");
        let nested = root.join("x/y");
        std::fs::create_dir_all(&nested).unwrap();
        // No markers anywhere under our scratch dir; temp-dir ancestors
        // are not Lean projects.
        assert!(find_project_root(&nested).is_none());
        std::fs::remove_dir_all(&root).ok();
    }
}
