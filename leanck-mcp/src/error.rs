use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("resolver could not locate identifier {0:?}")]
    Unresolved(String),

    #[error("timed out waiting for elaboration")]
    Timeout,

    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// The daemon returned a structured error (`kind` + message).
    #[error("leanckd error [{code}]: {message}")]
    Daemon { code: String, message: String },

    /// Connection-level failure talking to the daemon (retryable after respawn).
    #[error("leanckd transport error: {0}")]
    DaemonTransport(String),

    /// Failed to spawn the `leanckd` binary.
    #[error("could not spawn leanckd: {0}")]
    DaemonSpawn(String),

    /// The daemon did not come up within the startup timeout.
    #[error("leanckd did not start within the timeout")]
    DaemonStartupTimeout,
}

pub type Result<T> = std::result::Result<T, Error>;
