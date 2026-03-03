//! Daemon runtime: bind the Unix socket, serve framed requests against the
//! [`Workspace`], enforce per-request timeouts + backpressure + cancellation,
//! and idle-reap.

use std::collections::HashMap;
use std::fs::File;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use crate::request::RequestContext;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::net::{UnixListener, UnixStream};
use tokio::task::JoinSet;
use tokio_util::sync::CancellationToken;

use crate::error::{Error, Result};
use crate::query;
use crate::workspace::Workspace;
use leanck_config::OnLimit;
use leanck_core::api::{
    CancelResult, Command, HealthResult, LifecycleHealth, LifecycleState, PROTOCOL_VERSION,
    Request, Response, ServerHealth, ShutdownResult,
};
use leanck_core::{
    error::{ErrorKind, WireError},
    instances::{
        MAX_SLOTS, SlotLock, SlotOutcome, SlotPolicy, SlotRecord, now_unix_secs, process_start_id,
    },
    paths::RuntimePaths,
    wire,
};

/// Maximum interval between idle accounting and retention sweeps.
const REAP_TICK: Duration = Duration::from_secs(60);

// Every configurable limit must be reachable by the slot scan.
const _: () = assert!(leanck_config::Instances::MAX_LIMIT == MAX_SLOTS as u64);

struct Daemon {
    ws: Arc<Workspace>,
    cancels: Mutex<HashMap<String, CancellationToken>>,
    shutdown: CancellationToken,
    settings: leanck_config::Daemon,
}

impl Drop for Daemon {
    fn drop(&mut self) {
        self.shutdown.cancel();
    }
}

/// Exclusive per-worktree startup/lifetime lock (advisory `flock` on the lock
/// file). Held for the daemon's whole run, so concurrent launches serialize and
/// only one warms up / fetches the Mathlib cache. The OS releases the lock when
/// the holder's fd closes (including on crash), so it never wedges a worktree.
struct StartupLock {
    // Held only to keep the locked fd open; the `flock` lasts until it closes.
    _file: File,
}

impl StartupLock {
    /// Try to take the lock without blocking. `Ok(None)` means another process
    /// already holds it (a daemon is running or warming up for this worktree).
    fn acquire(path: &Path) -> std::io::Result<Option<StartupLock>> {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(path)?;
        match file.try_lock() {
            Ok(()) => Ok(Some(StartupLock { _file: file })),
            Err(std::fs::TryLockError::WouldBlock) => Ok(None),
            Err(std::fs::TryLockError::Error(error)) => Err(error),
        }
    }
}

/// Runtime files are owned only after acquiring the mandatory lifetime lock.
/// Keep the lock inode: unlinking it lets a third launcher lock a different file.
struct RuntimeOwner {
    socket: PathBuf,
    pid: PathBuf,
    // Declared before `_lock` so the instance slot is released no later than
    // the worktree lock; both live for the daemon's whole run.
    _slot: Option<SlotLock>,
    _lock: StartupLock,
}

impl Drop for RuntimeOwner {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
        let _ = std::fs::remove_file(&self.pid);
    }
}

/// Take a machine-wide instance slot for this daemon. Fallback runtime
/// directories take no slot. Over the limit under `refuse` or `evict_idle`
/// this fails before any runtime file is written; the daemon never evicts.
fn acquire_slot(
    runtime: &RuntimePaths,
    worktree: &Path,
    instances: &leanck_config::Instances,
) -> Result<Option<SlotLock>> {
    if runtime.is_fallback() {
        tracing::warn!(
            directory = %runtime.directory().display(),
            "runtime directory fell back to the system temp directory; this daemon is not counted against instances.limit"
        );
        return Ok(None);
    }
    let record = SlotRecord {
        pid: std::process::id(),
        worktree: worktree
            .canonicalize()
            .unwrap_or_else(|_| worktree.to_path_buf()),
        socket: runtime.socket(),
        started_at: now_unix_secs(),
        start_id: process_start_id(std::process::id()),
    };
    let policy = match instances.on_limit {
        OnLimit::Warn => SlotPolicy::Continue,
        OnLimit::Refuse | OnLimit::EvictIdle => SlotPolicy::Refuse,
    };
    match SlotLock::acquire(runtime.directory(), instances.limit, policy, &record)? {
        SlotOutcome::OverLimit { held, limit } => {
            tracing::warn!(
                running = held,
                limit,
                on_limit = instances.on_limit.as_str(),
                "instance limit reached; refusing to start"
            );
            Err(Error::InstanceLimit {
                running: held,
                limit,
            })
        }
        SlotOutcome::Acquired { lock, running } => {
            if instances.limit > 0 && running > instances.limit {
                tracing::warn!(
                    slot = lock.slot(),
                    running,
                    limit = instances.limit,
                    "instance limit exceeded; starting anyway because on_limit=warn"
                );
            }
            Ok(Some(lock))
        }
    }
}

/// Run the daemon for `worktree` until idle-reaped or asked to shut down.
pub async fn run(worktree: &Path) -> Result<()> {
    let settings = leanck_config::ConfigArgs::default()
        .load(worktree)
        .map_err(|e| Error::BadRequest(format!("configuration: {e:#}")))?
        .settings;
    run_with_settings(worktree, settings).await
}

pub async fn run_with_settings(worktree: &Path, settings: leanck_config::Settings) -> Result<()> {
    settings
        .validate()
        .map_err(|e| Error::BadRequest(format!("configuration: {e:#}")))?;
    let runtime = RuntimePaths::for_workspace(worktree).prepare()?;
    let Some(lock) = StartupLock::acquire(&runtime.lock())? else {
        tracing::info!(worktree = %worktree.display(), "another leanckd owns this worktree; exiting");
        return Ok(());
    };
    // Returning an error here drops the worktree lock; nothing else was written yet.
    let slot = acquire_slot(&runtime, worktree, &settings.instances)?;
    let owner = RuntimeOwner {
        socket: runtime.socket(),
        pid: runtime.pid(),
        _slot: slot,
        _lock: lock,
    };
    let _ = std::fs::remove_file(&owner.socket);
    std::fs::write(&owner.pid, std::process::id().to_string())?;
    let shutdown = CancellationToken::new();
    let mut tasks = JoinSet::new();
    // Install signal handling before cache warmup and Lean initialization.
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let stop = shutdown.clone();
    tasks.spawn(async move {
        tokio::select! { _ = term.recv() => {}, _ = interrupt.recv() => {}, _ = stop.cancelled() => return }
        stop.cancel();
    });
    let ws = match Workspace::start_configured(worktree, shutdown.clone(), &settings).await {
        Ok(ws) => ws,
        Err(Error::Cancelled) => return Ok(()),
        Err(error) => return Err(error),
    };
    let listener = match UnixListener::bind(&owner.socket) {
        Ok(listener) => listener,
        Err(error) => {
            ws.shutdown().await;
            return Err(error.into());
        }
    };
    tracing::info!(socket = %owner.socket.display(), "leanckd listening");
    // Warmup is not idle time: a freshly listening daemon must not look like
    // the longest-idle eviction candidate.
    ws.mark_active();
    let daemon = Arc::new(Daemon {
        ws,
        cancels: Mutex::new(HashMap::new()),
        shutdown,
        settings: settings.daemon,
    });
    tasks.spawn(reaper(daemon.clone()));
    loop {
        tokio::select! {
            biased;
            _ = daemon.shutdown.cancelled() => break,
            Some(result) = tasks.join_next() => {
                if let Err(error) = result { tracing::warn!(%error, "daemon task failed"); }
            }
            accepted = listener.accept() => match accepted {
                Ok((stream, _)) => { tasks.spawn(handle_connection(daemon.clone(), stream)); }
                Err(error) => { tracing::warn!(%error, "accept failed"); daemon.shutdown.cancel(); }
            }
        }
    }
    drop(listener);
    daemon.shutdown.cancel();
    // Stop admission, cancel active work, and let clients receive cancellation.
    if tokio::time::timeout(Duration::from_secs(2), async {
        while tasks.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tasks.abort_all();
        while tasks.join_next().await.is_some() {}
    }
    daemon.ws.shutdown().await;
    drop(owner);
    Ok(())
}

async fn reaper(daemon: Arc<Daemon>) {
    loop {
        tokio::select! {
            _ = tokio::time::sleep(if daemon.settings.idle_timeout_ms == 0 { REAP_TICK } else { REAP_TICK.min(Duration::from_millis(daemon.settings.idle_timeout_ms)) }) => {
                daemon.ws.evict_documents();
                // Closing admission first means a query admitted just now keeps
                // the daemon alive instead of being cancelled by the reap.
                if daemon.settings.idle_timeout_ms != 0 && daemon.ws.idle_for() >= Duration::from_millis(daemon.settings.idle_timeout_ms) && daemon.ws.close_if_idle() {
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
    let (reader, writer) = stream.into_split();
    let mut reader = leanck_core::wire::reader(BufReader::new(reader));
    let mut writer = leanck_core::wire::writer(writer);
    let disconnect = daemon.shutdown.child_token();
    loop {
        let frame = tokio::select! {
            biased;
            _ = daemon.shutdown.cancelled() => break,
            frame = tokio::time::timeout(Duration::from_secs(30), wire::read_frame(&mut reader)) => match frame { Ok(Ok(Some(frame))) => frame, _ => break }
        };
        let response = match serde_json::from_slice::<Request>(&frame) {
            Ok(req) => {
                let work = dispatch(&daemon, req, disconnect.clone());
                tokio::pin!(work);
                // Monitor EOF while working. Leave pipelined bytes buffered for
                // the next frame rather than consuming or spinning on them.
                if !reader.read_buffer().is_empty() {
                    work.await
                } else {
                    tokio::select! {
                        biased;
                        response = &mut work => response,
                        peek = reader.get_mut().fill_buf() => {
                            if !matches!(peek, Ok(bytes) if !bytes.is_empty()) { disconnect.cancel(); break; }
                            work.await
                        }
                    }
                }
            }
            Err(error) => Response::err(
                String::new(),
                WireError {
                    kind: ErrorKind::BadRequest,
                    message: format!("malformed request: {error}"),
                },
            ),
        };
        let Ok(bytes) = serde_json::to_vec(&response) else {
            break;
        };
        if !matches!(
            tokio::time::timeout(
                Duration::from_secs(2),
                wire::write_frame(&mut writer, &bytes)
            )
            .await,
            Ok(Ok(()))
        ) {
            break;
        }
    }
    disconnect.cancel();
}

struct Registration {
    daemon: Arc<Daemon>,
    id: String,
    token: CancellationToken,
}
impl Drop for Registration {
    fn drop(&mut self) {
        self.token.cancel();
        self.daemon.cancels.lock().unwrap().remove(&self.id);
    }
}

async fn dispatch(daemon: &Arc<Daemon>, req: Request, connection: CancellationToken) -> Response {
    let id = req.id.clone();

    let command = match req.command() {
        Ok(command) => command,
        Err(error) => return Response::err(id, to_wire(&Error::BadRequest(error.to_string()))),
    };
    // Control ops handled inline (no elaboration, no permit). They never touch
    // the idle clock, so probing a daemon does not postpone its idle shutdown
    // or hide how long it has been idle.
    match &command {
        Command::Health | Command::Status => return response(id, Ok(health(daemon))),
        Command::Shutdown(params) => {
            // A conditional shutdown that finds admission already closed (by
            // another shutdown or the idle reaper) reports the shutdown in
            // progress rather than a busy daemon. Closing is permanent, so the
            // second check cannot see a reopened daemon.
            let shutting_down = if params.if_idle {
                daemon.ws.close_if_idle() || daemon.ws.admission_closed()
            } else {
                daemon.ws.close();
                true
            };
            if shutting_down {
                daemon.shutdown.cancel();
            }
            return response(id, Ok(ShutdownResult { shutting_down }));
        }
        Command::Cancel(params) => {
            if let Some(token) = daemon.cancels.lock().unwrap().get(&params.target_id) {
                token.cancel();
            }
            return response(
                id,
                Ok(CancelResult {
                    cancelled: params.target_id.clone(),
                }),
            );
        }
        _ => {}
    }

    // Queries hold a lease from admission until their response is built; the
    // lease also touches the idle clock on both ends.
    let Some(_activity) = daemon.ws.begin_request() else {
        return Response::err(id, to_wire(&Error::ShuttingDown));
    };
    let token = connection.child_token();
    let ctx = match RequestContext::with_default(
        req.timeout_ms,
        token.clone(),
        Duration::from_millis(daemon.settings.request_timeout_ms),
    ) {
        Ok(ctx) => ctx,
        Err(error) => return Response::err(id, to_wire(&error)),
    };
    {
        let mut cancels = daemon.cancels.lock().unwrap();
        if cancels.contains_key(&id) {
            return Response::err(
                id,
                to_wire(&Error::BadRequest("request id already active".into())),
            );
        }
        cancels.insert(id.clone(), token.clone());
    }
    let _registration = Registration {
        daemon: daemon.clone(),
        id: id.clone(),
        token,
    };
    let result = ctx
        .run(query::QueryService::new(&daemon.ws).execute(command, &ctx))
        .await;
    match result {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, to_wire(&error)),
    }
}

fn encode<T: serde::Serialize>(value: T) -> Result<Value> {
    Ok(serde_json::to_value(value)?)
}

fn response<T: serde::Serialize>(id: String, result: Result<T>) -> Response {
    match result.and_then(encode) {
        Ok(value) => Response::ok(id, value),
        Err(error) => Response::err(id, to_wire(&error)),
    }
}

fn health(daemon: &Arc<Daemon>) -> HealthResult {
    let server = daemon.ws.server();
    let running = !server.is_dead();
    let state = if daemon.ws.is_shutting_down() {
        LifecycleState::ShuttingDown
    } else if !running {
        LifecycleState::Recovering
    } else {
        LifecycleState::Ready
    };
    HealthResult {
        ok: state == LifecycleState::Ready,
        workspace: daemon.ws.worktree().to_string_lossy().into_owned(),
        protocol_version: PROTOCOL_VERSION,
        server: ServerHealth {
            running,
            pid: server.pid(),
        },
        idle_secs: daemon.ws.idle_for().as_secs(),
        lifecycle: LifecycleHealth {
            state,
            active_requests: daemon.ws.active_requests(),
        },
    }
}

fn to_wire(e: &Error) -> WireError {
    let kind = match e {
        Error::Timeout => ErrorKind::Timeout,
        Error::Cancelled => ErrorKind::Cancelled,
        Error::Unresolved(_)
        | Error::InvalidPath(_)
        | Error::Document(leanck_core::document::DocumentError::Path { .. }) => ErrorKind::NotFound,
        Error::Document(_) => ErrorKind::BadRequest,
        Error::BadRequest(_) => ErrorKind::BadRequest,
        Error::ServerDown(_) | Error::Spawn(_) | Error::ShuttingDown => ErrorKind::ServerDown,
        Error::Overloaded => ErrorKind::Overloaded,
        _ => ErrorKind::Internal,
    };
    WireError {
        kind,
        message: e.to_string(),
    }
}
