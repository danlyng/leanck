//! Workspace selection: exact validation and ancestor discovery are distinct policies.
use std::path::{Path, PathBuf};
use thiserror::Error;

pub const PROJECT_MARKERS: &[&str] = &["lakefile.lean", "lakefile.toml", "lean-toolchain"];

#[derive(Debug, Error)]
pub enum WorkspaceError {
    #[error("could not canonicalize workspace path {path:?}: {source}")]
    Canonicalize {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("workspace path {0:?} is not a directory")]
    NotDirectory(PathBuf),
    #[error(
        "workspace path {0:?} has no Lake project marker (lakefile.lean, lakefile.toml, lean-toolchain)"
    )]
    NoProjectMarker(PathBuf),
    #[error("could not read current directory: {0}")]
    CurrentDirectory(std::io::Error),
}

/// Canonical existing directory. Bare Lean workspaces are allowed by `from_path`.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct WorkspaceRoot(PathBuf);

impl WorkspaceRoot {
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let path = path.as_ref();
        let canonical = path
            .canonicalize()
            .map_err(|source| WorkspaceError::Canonicalize {
                path: path.to_path_buf(),
                source,
            })?;
        if !canonical.is_dir() {
            return Err(WorkspaceError::NotDirectory(canonical));
        }
        Ok(Self(canonical))
    }

    /// Validate exactly this directory, without walking to an ancestor project.
    pub fn validate_project(path: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let root = Self::from_path(path)?;
        if !has_project_marker(root.as_path()) {
            return Err(WorkspaceError::NoProjectMarker(root.0));
        }
        Ok(root)
    }

    /// Walk to the nearest project marker, otherwise use the starting directory.
    pub fn discover(start: impl AsRef<Path>) -> Result<Self, WorkspaceError> {
        let start = Self::from_path(start)?;
        Self::from_path(find_project_root(start.as_path()).unwrap_or(start.0))
    }

    pub fn discover_current() -> Result<Self, WorkspaceError> {
        Self::discover(std::env::current_dir().map_err(WorkspaceError::CurrentDirectory)?)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
}

impl AsRef<Path> for WorkspaceRoot {
    fn as_ref(&self) -> &Path {
        self.as_path()
    }
}

/// Preserve the existing marker policy: presence is sufficient (no Lake invocation).
pub fn has_project_marker(path: &Path) -> bool {
    PROJECT_MARKERS
        .iter()
        .any(|marker| path.join(marker).exists())
}

pub fn find_project_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| has_project_marker(dir))
        .map(Path::to_path_buf)
}
