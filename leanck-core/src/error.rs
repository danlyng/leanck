//! Shared wire errors; runtime errors remain owned by the daemon.

use serde::{Deserialize, Serialize};

/// Structured error returned to the client.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WireError {
    pub kind: ErrorKind,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorKind {
    /// Request exceeded its deadline.
    Timeout,
    /// Request was cancelled.
    Cancelled,
    /// File / identifier not found.
    NotFound,
    /// Malformed request.
    BadRequest,
    /// The Lean server is down / restarting.
    ServerDown,
    /// The Lean server stopped responding.
    ServerUnhealthy,
    /// Too many in-flight elaboration-blocking requests.
    Overloaded,
    /// Catch-all internal error.
    Internal,
}

/// A reply cannot be interpreted as the result of the dispatched operation.
#[derive(Debug, thiserror::Error)]
pub enum ProtocolError {
    #[error("invalid daemon response envelope: {0}")]
    Envelope(#[source] serde_json::Error),
    #[error("response id {actual:?} does not match request {expected:?}")]
    MismatchedId { expected: String, actual: String },
    #[error("response must contain exactly a success result or a failure error")]
    InconsistentEnvelope,
    #[error("invalid {operation} result: {source}")]
    InvalidResult {
        operation: crate::api::Operation,
        #[source]
        source: serde_json::Error,
    },
}
