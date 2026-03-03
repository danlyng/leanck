use thiserror::Error;

/// Daemon-internal error type. The socket layer maps these onto the structured
/// [`leanck_core::error::WireError`] (`ErrorKind` + message) sent back to clients.
#[derive(Debug, Error)]
pub enum Error {
    #[error(transparent)]
    Document(#[from] leanck_core::document::DocumentError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// The Lean server returned a JSON-RPC error response.
    #[error("lean server error {code}: {message}")]
    Lsp {
        code: i64,
        message: String,
        data: Option<serde_json::Value>,
    },

    /// A request exceeded its deadline.
    #[error("timed out waiting for elaboration")]
    Timeout,

    /// A request was cancelled (by the client or because the server was torn down).
    #[error("request cancelled")]
    Cancelled,

    /// The Lean server is not running / crashed / is being respawned.
    #[error("lean server unavailable: {0}")]
    ServerDown(String),

    /// Too many in-flight elaboration-blocking requests.
    #[error("server overloaded")]
    Overloaded,

    /// A resolver could not locate the requested identifier.
    #[error("could not locate identifier {0:?}")]
    Unresolved(String),

    /// A path/file argument was invalid or not found.
    #[error("invalid path: {0}")]
    InvalidPath(String),

    /// A malformed request from the client.
    #[error("bad request: {0}")]
    BadRequest(String),

    /// Starting would exceed the machine-wide `instances.limit` under `refuse`
    /// or `evict_idle`. The daemon never evicts; the binary exits with
    /// [`leanck_core::instances::EXIT_INSTANCE_LIMIT`].
    #[error("instance limit reached: {running} leanck daemons running, limit {limit}")]
    InstanceLimit { running: u64, limit: u64 },

    /// Admission is closed because the daemon is shutting down.
    #[error("leanckd is shutting down")]
    ShuttingDown,

    /// Failed to spawn `lake serve`.
    #[error("failed to spawn lake serve: {0}")]
    Spawn(String),
}

pub type Result<T> = std::result::Result<T, Error>;
