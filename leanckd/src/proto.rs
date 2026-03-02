//! Wire protocol between `leanck-mcp` (client) and `leanckd` (daemon).
//!
//! Framing: each message is a 4-byte big-endian length prefix followed by a
//! JSON body. Requests and responses are correlated by `id`.

use std::io;

use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

/// Bumped whenever the request/response shapes change incompatibly. Reported by
/// `health` so a client can detect a stale daemon left running from an older
/// build and trigger a restart.
pub const PROTOCOL_VERSION: u32 = 1;

/// Hard cap on a single frame (16 MiB) to bound memory on a malformed length.
const MAX_FRAME: u32 = 16 * 1024 * 1024;

/// Write a length-prefixed frame (4-byte big-endian length + body).
pub async fn write_frame<W: AsyncWrite + Unpin>(w: &mut W, body: &[u8]) -> io::Result<()> {
    let len = u32::try_from(body.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "frame too large"))?;
    w.write_all(&len.to_be_bytes()).await?;
    w.write_all(body).await?;
    w.flush().await
}

/// Read one length-prefixed frame. Returns `Ok(None)` at clean EOF.
pub async fn read_frame<R: AsyncRead + Unpin>(r: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match r.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame exceeds limit"));
    }
    let mut body = vec![0u8; len as usize];
    r.read_exact(&mut body).await?;
    Ok(Some(body))
}

/// A request from a client. `op` selects the handler; `params` is the
/// op-specific payload (deserialized per-op by the daemon).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: String,
    pub op: String,
    /// Per-request deadline. `None` means use the daemon default.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    #[serde(default)]
    pub params: serde_json::Value,
}

/// A response to a [`Request`], correlated by `id`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: String,
    pub ok: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WireError>,
}

impl Response {
    pub fn ok(id: String, result: serde_json::Value) -> Self {
        Response { id, ok: true, result: Some(result), error: None }
    }

    pub fn err(id: String, error: WireError) -> Self {
        Response { id, ok: false, result: None, error: Some(error) }
    }
}

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

// ---- op parameter payloads ------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DiagnosticsParams {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ident: Option<String>,
    /// Comma-separated severity list (`error,warning,...`), or `all`. Empty/None
    /// means `error` only. Parsed by the daemon (mirrors the old HTTP API).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub severity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<usize>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GoalParams {
    pub file: String,
    pub line: u32,
    pub character: u32,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SorryParams {
    /// Either a file or a directory (recursive scan).
    pub path: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ListParams {
    pub file: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<Vec<String>>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RestartFileParams {
    pub file: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ReferencesParams {
    /// File containing the declaration whose references are wanted.
    pub file: String,
    /// The declaration identifier (e.g. `Foo.bar` or `bar`).
    pub ident: String,
    /// Optional namespace to qualify a bare `ident` against.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CancelParams {
    pub target_id: String,
}

/// Op names. Kept as `&str` constants so both sides agree on the wire string.
pub mod op {
    pub const HEALTH: &str = "health";
    pub const DIAGNOSTICS: &str = "diagnostics";
    pub const GOAL: &str = "goal";
    pub const SORRY: &str = "sorry";
    pub const LIST: &str = "list";
    pub const REFERENCES: &str = "references";
    pub const RESTART_FILE: &str = "restart_file";
    pub const CANCEL: &str = "cancel";
    pub const STATUS: &str = "status";
    pub const SHUTDOWN: &str = "shutdown";
}
