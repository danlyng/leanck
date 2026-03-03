//! Canonical document paths and correctly escaped local file URIs.
use std::path::{Path, PathBuf};
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum DocumentError {
    #[error("invalid document path {path:?}: {source}")]
    Path {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("expected an absolute document path: {0:?}")]
    RelativePath(PathBuf),
    #[error("invalid local file URI: {0:?}")]
    InvalidUri(String),
}

/// A canonical existing filesystem identity, also usable for directory scans.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct DocumentPath(PathBuf);

impl DocumentPath {
    /// Missing paths fail here; URI conversion alone does not require existence.
    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, DocumentError> {
        let path = path.as_ref();
        path.canonicalize()
            .map(Self)
            .map_err(|source| DocumentError::Path {
                path: path.to_path_buf(),
                source,
            })
    }

    pub fn resolve(workspace: &Path, argument: &str) -> Result<Self, DocumentError> {
        Self::from_path(argument_path(workspace, argument)?)
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }
    pub fn into_path_buf(self) -> PathBuf {
        self.0
    }
    pub fn uri(&self) -> String {
        path_to_uri(&self.0).expect("canonical absolute document path")
    }
}

/// Interpret an argument without requiring it to exist. Use `DocumentPath` for identity.
pub fn argument_path(workspace: &Path, argument: &str) -> Result<PathBuf, DocumentError> {
    if argument.starts_with("file:") || argument.contains("://") {
        uri_to_path(argument)
    } else {
        Ok(workspace.join(argument))
    }
}

/// Encode an absolute path without touching the filesystem, including missing paths.
pub fn path_to_uri(path: &Path) -> Result<String, DocumentError> {
    Url::from_file_path(path)
        .map(String::from)
        .map_err(|_| DocumentError::RelativePath(path.to_path_buf()))
}

/// Decode a local file URL; remote hosts, fragments, and queries are rejected.
pub fn uri_to_path(uri: &str) -> Result<PathBuf, DocumentError> {
    let invalid = || DocumentError::InvalidUri(uri.to_string());
    let url = Url::parse(uri).map_err(|_| invalid())?;
    if !uri.starts_with("file://")
        || url.scheme() != "file"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err(invalid());
    }
    url.to_file_path().map_err(|_| invalid())
}
