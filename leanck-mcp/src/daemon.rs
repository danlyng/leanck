//! Per-worktree handle to the `leanckd` daemon.
//!
//! The connect/auto-spawn/transport logic lives in `leanckd::client` (shared
//! with the `leanck` CLI). This module is the thin MCP-side adapter: a
//! workspace-swappable cache plus error mapping onto the MCP [`Error`] type so
//! the `forward!` retry logic in `mcp.rs` can distinguish "respawn and retry"
//! from a real daemon-reported error.

use std::path::PathBuf;
use std::sync::Arc;

use leanckd::client::{self, ClientError};
use serde::Serialize;
use serde_json::Value;
use tokio::sync::Mutex;

use crate::error::{Error, Result};

/// A connected client for one worktree's daemon. Wraps the shared
/// [`leanckd::client::DaemonClient`], mapping its errors onto [`Error`].
#[derive(Debug, Clone)]
pub struct DaemonClient {
    inner: client::DaemonClient,
}

impl DaemonClient {
    fn new(inner: client::DaemonClient) -> Self {
        DaemonClient { inner }
    }

    /// Forward an optional per-request deadline (ms). The daemon bounds the
    /// whole request by it and stretches its elaboration wait accordingly.
    pub fn with_timeout_ms(self, timeout_ms: Option<u64>) -> Self {
        DaemonClient {
            inner: self.inner.with_timeout_ms(timeout_ms),
        }
    }

    pub async fn healthz(&self) -> Result<Value> {
        self.inner.health().await.map_err(map_client_error)
    }

    pub async fn diagnostics<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.inner.diagnostics(q).await.map_err(map_client_error)
    }

    pub async fn goal<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.inner.goal(q).await.map_err(map_client_error)
    }

    pub async fn sorries<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.inner.sorries(q).await.map_err(map_client_error)
    }

    pub async fn list<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.inner.list(q).await.map_err(map_client_error)
    }

    pub async fn restart_file<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.inner.restart_file(q).await.map_err(map_client_error)
    }
}

/// Map a shared-client error onto the MCP error type. Only connection-level
/// failures are retryable (`DaemonTransport` / `DaemonStartupTimeout`); a
/// daemon-reported error is a real outcome and must not be retried.
fn map_client_error(e: ClientError) -> Error {
    match e {
        ClientError::Transport(m) => Error::DaemonTransport(m),
        ClientError::Spawn(m) => Error::DaemonSpawn(m),
        ClientError::StartupTimeout => Error::DaemonStartupTimeout,
        ClientError::Daemon { kind, message } => Error::Daemon {
            code: format!("{kind:?}"),
            message,
        },
    }
}

/// Lazy, retrying, workspace-swappable cache. The daemon is auto-spawned on
/// first use (one warm `leanckd` per worktree, shared across agents).
#[derive(Clone)]
pub struct DaemonCache {
    worktree: Arc<Mutex<PathBuf>>,
}

impl DaemonCache {
    pub fn new(worktree: PathBuf) -> Self {
        DaemonCache {
            worktree: Arc::new(Mutex::new(worktree)),
        }
    }

    /// Return a client for the current worktree, auto-spawning the daemon if it
    /// isn't already listening.
    pub async fn get(&self) -> Result<DaemonClient> {
        let worktree = self.worktree.lock().await.clone();
        let inner = client::connect_or_spawn(&worktree)
            .await
            .map_err(map_client_error)?;
        Ok(DaemonClient::new(inner))
    }

    /// Re-resolve and (re)spawn if the daemon is gone. Used by `forward!` after
    /// a transport error (the daemon may have been idle-reaped).
    pub async fn refresh(&self) -> Result<DaemonClient> {
        self.get().await
    }

    /// Point at a different worktree; the next `get()` resolves that worktree's
    /// daemon (spawning it if needed).
    pub async fn set_workspace(&self, worktree: PathBuf) {
        *self.worktree.lock().await = worktree;
    }

    /// The current workspace, shared by live queries and builds.
    pub async fn workspace(&self) -> PathBuf {
        self.worktree.lock().await.clone()
    }

    /// The socket path for the current worktree (for health display).
    pub async fn discovery_path(&self) -> PathBuf {
        leanckd::paths::socket_path(&self.worktree.lock().await.clone())
    }

    /// Retry-after-refresh only for transport / startup failures.
    pub fn should_refresh(err: &Error) -> bool {
        matches!(err, Error::DaemonTransport(_) | Error::DaemonStartupTimeout)
    }
}
