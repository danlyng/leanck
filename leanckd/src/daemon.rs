//! Daemon runtime: bind the Unix socket, serve framed requests against the
//! [`Workspace`], enforce per-request timeouts + backpressure + cancellation,
//! and idle-reap.

use std::collections::HashMap;
use std::fs::File;
use std::os::fd::AsRawFd;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tokio::net::{UnixListener, UnixStream};
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::proto::{self, ErrorKind, PROTOCOL_VERSION, Request, Response, WireError, op};
use crate::query;
use crate::workspace::Workspace;

/// Overall per-request deadline when the client doesn't specify one. Bounds the
/// whole op (elaboration wait + LSP calls) so nothing rides forever.
const DEFAULT_REQUEST_TIMEOUT: Duration = Duration::from_secs(300);
/// Idle period after which the daemon shuts itself down.
const IDLE_REAP: Duration = Duration::from_secs(60 * 60);
const REAP_TICK: Duration = Duration::from_secs(60);

struct Daemon {
    ws: Arc<Workspace>,
    socket: PathBuf,
    pid_file: PathBuf,
    cancels: Mutex<HashMap<String, CancellationToken>>,
    shutdown: CancellationToken,
}

/// Exclusive per-worktree startup/lifetime lock (advisory `flock` on the lock
/// file). Held for the daemon's whole run, so concurrent launches serialize and
/// only one warms up / fetches the Mathlib cache. The OS releases the lock when
/// the holder's fd closes (including on crash), so it never wedges a worktree.
struct StartupLock {
    // Held only to keep the locked fd open; the `flock` lasts until it closes.
    _file: File,
    path: PathBuf,
}

impl StartupLock {
    /// Try to take the lock without blocking. `Ok(None)` means another process
    /// already holds it (a daemon is running or warming up for this worktree).
    fn acquire(path: &Path) -> std::io::Result<Option<StartupLock>> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .open(path)?;
        // SAFETY: `file` owns a valid fd for the duration of the call.
        let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
        if rc == 0 {
            return Ok(Some(StartupLock { _file: file, path: path.to_path_buf() }));
        }
        let err = std::io::Error::last_os_error();
        match err.raw_os_error() {
            Some(libc::EWOULDBLOCK) => Ok(None),
            _ => Err(err),
        }
    }
}

impl Drop for StartupLock {
    fn drop(&mut self) {
        // Closing the fd (when `_file` drops) releases the lock; remove the file
        // for tidiness. Best-effort — a leftover unlocked file is harmless.
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Run the daemon for `worktree` until idle-reaped or asked to shut down.
pub async fn run(worktree: &Path) -> Result<()> {
    let socket = crate::paths::socket_path(worktree);
    let pid_file = crate::paths::pid_path(worktree);
    let lock_file = crate::paths::lock_path(worktree);

    // Single-instance guard, held for the daemon's whole lifetime. Acquired
    // before any slow startup work (the Mathlib cache fetch) so concurrent
    // launches don't each kick off their own `lake exe cache get`; the OS drops
    // the lock if we die, so a crash never wedges the worktree.
    let _lock = match StartupLock::acquire(&lock_file) {
        Ok(Some(lock)) => Some(lock),
        Ok(None) => {
            tracing::info!(
                worktree = %worktree.display(),
                "another leanckd already owns this worktree; exiting"
            );
            return Ok(());
        }
        Err(e) => {
            // Don't block startup on a lock-file problem; fall back to the
            // socket-based duplicate check.
            tracing::warn!(error = %e, "could not acquire startup lock; proceeding without it");
            if socket.exists() && UnixStream::connect(&socket).await.is_ok() {
                tracing::info!("daemon already running; exiting");
                return Ok(());
            }
            None
        }
    };

    // We hold the lock, so any socket on disk is stale (its owner is gone).
    if socket.exists() {
        let _ = std::fs::remove_file(&socket);
    }
    // Publish liveness early so a client waiting through a slow cache fetch can
    // poll this pid and keep waiting instead of timing out.
    let _ = std::fs::write(&pid_file, std::process::id().to_string());

    // Warm up fully BEFORE advertising readiness: fetch the Mathlib cache and
    // start `lake serve` (both inside `Workspace::start`). Only then bind the
    // socket, so clients see the daemon exactly when it can actually serve.
    let ws = Workspace::start(worktree).await?;

    let listener = UnixListener::bind(&socket)?;
    tracing::info!(socket = %socket.display(), "leanckd listening");

    let daemon = Arc::new(Daemon {
        ws,
        socket: socket.clone(),
        pid_file: pid_file.clone(),
        cancels: Mutex::new(HashMap::new()),
        shutdown: CancellationToken::new(),
    });

    tokio::spawn(reaper(daemon.clone()));

    loop {
        tokio::select! {
            accepted = listener.accept() => {
                match accepted {
                    Ok((stream, _addr)) => {
                        let d = daemon.clone();
                        tokio::spawn(async move { handle_connection(d, stream).await; });
                    }
                    Err(e) => tracing::warn!(error = %e, "accept failed"),
                }
            }
            _ = daemon.shutdown.cancelled() => break,
        }
    }

    tracing::info!("leanckd shutting down");
    daemon.ws.shutdown().await;
    let _ = std::fs::remove_file(&daemon.socket);
    let _ = std::fs::remove_file(&daemon.pid_file);
    Ok(())
}

async fn reaper(daemon: Arc<Daemon>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(REAP_TICK) => {
                if daemon.ws.idle_for() >= IDLE_REAP {
                    tracing::info!("idle timeout reached; reaping");
                    daemon.shutdown.cancel();
                    return;
                }
            }
            _ = daemon.shutdown.cancelled() => return,
        }
    }
}

async fn handle_connection(daemon: Arc<Daemon>, stream: UnixStream) {
    let (mut reader, mut writer) = stream.into_split();
    loop {
        let frame = match proto::read_frame(&mut reader).await {
            Ok(Some(f)) => f,
            Ok(None) => break, // client closed
            Err(e) => {
                tracing::debug!(error = %e, "read_frame failed");
                break;
            }
        };
        let response = match serde_json::from_slice::<Request>(&frame) {
            Ok(req) => dispatch(&daemon, req).await,
            Err(e) => Response::err(
                String::new(),
                WireError {
                    kind: ErrorKind::BadRequest,
                    message: format!("malformed request: {e}"),
                },
            ),
        };
        let bytes = match serde_json::to_vec(&response) {
            Ok(b) => b,
            Err(_) => break,
        };
        if proto::write_frame(&mut writer, &bytes).await.is_err() {
            break;
        }
    }
}

async fn dispatch(daemon: &Arc<Daemon>, req: Request) -> Response {
    daemon.ws.mark_active();
    let id = req.id.clone();

    // Control ops handled inline (no elaboration, no permit).
    match req.op.as_str() {
        op::HEALTH => return Response::ok(id, health(daemon)),
        op::STATUS => return Response::ok(id, health(daemon)),
        op::SHUTDOWN => {
            daemon.shutdown.cancel();
            return Response::ok(id, json!({ "shutting_down": true }));
        }
        op::CANCEL => {
            return match serde_json::from_value::<proto::CancelParams>(req.params.clone()) {
                Ok(c) => {
                    if let Some(tok) = daemon.cancels.lock().unwrap().get(&c.target_id) {
                        tok.cancel();
                    }
                    Response::ok(id, json!({ "cancelled": c.target_id }))
                }
                Err(e) => Response::err(
                    id,
                    WireError {
                        kind: ErrorKind::BadRequest,
                        message: e.to_string(),
                    },
                ),
            };
        }
        _ => {}
    }

    // Query ops: register a cancellation token, bound by an overall timeout and
    // backpressure permit.
    let token = CancellationToken::new();
    daemon
        .cancels
        .lock()
        .unwrap()
        .insert(id.clone(), token.clone());

    let timeout = req
        .timeout_ms
        .map(Duration::from_millis)
        .unwrap_or(DEFAULT_REQUEST_TIMEOUT);

    let result = run_query(daemon, &req, &token, timeout).await;

    daemon.cancels.lock().unwrap().remove(&id);

    match result {
        Ok(v) => Response::ok(id, v),
        Err(e) => Response::err(id, to_wire(&e)),
    }
}

async fn run_query(
    daemon: &Arc<Daemon>,
    req: &Request,
    token: &CancellationToken,
    timeout: Duration,
) -> Result<Value> {
    // Backpressure for elaboration-blocking ops.
    let _permit = match req.op.as_str() {
        op::DIAGNOSTICS | op::GOAL | op::SORRY | op::LIST | op::REFERENCES => {
            Some(daemon.ws.acquire_inflight().await?)
        }
        _ => None,
    };

    let ws = &daemon.ws;
    let op_str = req.op.clone();
    let params = req.params.clone();
    // An explicit per-request deadline also stretches the elaboration wait
    // inside the handlers (it defaults to the shorter DEFAULT_ELAB_TIMEOUT).
    let wait_override = req.timeout_ms.map(Duration::from_millis);

    let work = async move {
        match op_str.as_str() {
            op::DIAGNOSTICS => query::diagnostics(ws, &parse(params)?, wait_override).await,
            op::GOAL => query::goal(ws, &parse(params)?, wait_override).await,
            op::SORRY => query::sorry(ws, &parse(params)?, wait_override).await,
            op::LIST => query::list(ws, &parse(params)?, wait_override).await,
            op::REFERENCES => query::references(ws, &parse(params)?, wait_override).await,
            op::RESTART_FILE => query::restart_file(ws, &parse(params)?).await,
            other => Err(Error::BadRequest(format!("unknown op: {other}"))),
        }
    };

    tokio::select! {
        r = work => r,
        _ = token.cancelled() => Err(Error::Cancelled),
        _ = tokio::time::sleep(timeout) => Err(Error::Timeout),
    }
}

fn parse<T: serde::de::DeserializeOwned>(params: Value) -> Result<T> {
    serde_json::from_value(params).map_err(|e| Error::BadRequest(format!("bad params: {e}")))
}

fn health(daemon: &Arc<Daemon>) -> Value {
    let server = daemon.ws.server();
    json!({
        "ok": !server.is_dead(),
        "workspace": daemon.ws.worktree().to_string_lossy(),
        "protocol_version": PROTOCOL_VERSION,
        "server": { "running": !server.is_dead(), "pid": daemon.ws.pid() },
        "idle_secs": daemon.ws.idle_for().as_secs(),
    })
}

fn to_wire(e: &Error) -> WireError {
    let kind = match e {
        Error::Timeout => ErrorKind::Timeout,
        Error::Cancelled => ErrorKind::Cancelled,
        Error::Unresolved(_) | Error::InvalidPath(_) => ErrorKind::NotFound,
        Error::BadRequest(_) => ErrorKind::BadRequest,
        Error::ServerDown(_) | Error::Spawn(_) => ErrorKind::ServerDown,
        Error::ServerUnhealthy => ErrorKind::ServerUnhealthy,
        Error::Overloaded => ErrorKind::Overloaded,
        _ => ErrorKind::Internal,
    };
    WireError {
        kind,
        message: e.to_string(),
    }
}
