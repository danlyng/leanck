//! Client for talking to a `leanckd` daemon over its Unix socket.
//!
//! Both `leanck-mcp` (per-agent MCP server) and `leanck` (the human CLI) speak
//! to the *same* daemon through this module, so the connect/auto-spawn/transport
//! logic lives in one place. The client is stateless: it opens a fresh framed
//! connection per request, so concurrent callers sharing one daemon don't
//! head-of-line block each other.

use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde::Serialize;
use serde_json::Value;
use tokio::net::UnixStream;
use uuid::Uuid;

use crate::paths;
use crate::proto::{self, op, ErrorKind, Request, Response, WireError};

/// Grace period to wait for a freshly spawned daemon to come alive (write its
/// pid file) before giving up when no daemon process is visible.
const STARTUP_GRACE: Duration = Duration::from_secs(30);

/// Absolute cap on waiting for the daemon to bind its socket, even while it is
/// demonstrably alive and warming up. A cold `lake exe cache get` can take many
/// minutes, and the daemon binds only once it can actually serve, so this is
/// deliberately generous; it's a backstop against a wedged daemon, not the
/// normal path.
const STARTUP_HARD_CAP: Duration = Duration::from_secs(30 * 60);

/// Poll interval while waiting for the daemon's socket to come up.
const STARTUP_POLL: Duration = Duration::from_millis(150);

/// A failure talking to the daemon. `Transport`/`StartupTimeout` are retryable
/// (the daemon may have been idle-reaped and need respawning); a `Daemon` error
/// is a real outcome and must not be retried.
#[derive(Debug, Clone)]
pub enum ClientError {
    /// Connection-level failure (socket gone, closed mid-request, encode error).
    Transport(String),
    /// Could not locate / launch the `leanckd` binary.
    Spawn(String),
    /// The daemon did not start within [`STARTUP_TIMEOUT`].
    StartupTimeout,
    /// The daemon returned a structured error.
    Daemon { kind: ErrorKind, message: String },
}

impl ClientError {
    /// Whether a retry after re-spawning the daemon is worthwhile.
    pub fn is_retryable(&self) -> bool {
        matches!(self, ClientError::Transport(_) | ClientError::StartupTimeout)
    }
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::Transport(m) => write!(f, "leanckd transport error: {m}"),
            ClientError::Spawn(m) => write!(f, "could not spawn leanckd: {m}"),
            ClientError::StartupTimeout => write!(f, "leanckd did not start within the timeout"),
            ClientError::Daemon { kind, message } => {
                write!(f, "leanckd error [{kind:?}]: {message}")
            }
        }
    }
}

impl std::error::Error for ClientError {}

type Result<T> = std::result::Result<T, ClientError>;

/// A connected client for one worktree's daemon.
#[derive(Debug, Clone)]
pub struct DaemonClient {
    socket: PathBuf,
    /// Optional per-request deadline forwarded in the request envelope. The
    /// daemon bounds the whole request by it *and* stretches its elaboration
    /// wait to it (otherwise both fall back to the daemon defaults).
    timeout_ms: Option<u64>,
}

impl DaemonClient {
    /// Build a client bound to an explicit socket path (no connectivity check).
    pub fn new(socket: PathBuf) -> Self {
        DaemonClient { socket, timeout_ms: None }
    }

    /// Return a client whose requests carry this deadline (`None` = daemon default).
    pub fn with_timeout_ms(mut self, timeout_ms: Option<u64>) -> Self {
        self.timeout_ms = timeout_ms;
        self
    }

    /// The socket path this client targets.
    pub fn socket(&self) -> &Path {
        &self.socket
    }

    /// Issue one request and await the response.
    pub async fn call(&self, op: &str, params: Value) -> Result<Value> {
        let req = Request {
            id: Uuid::new_v4().to_string(),
            op: op.to_string(),
            timeout_ms: self.timeout_ms,
            params,
        };
        let mut stream = UnixStream::connect(&self.socket)
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let (mut r, mut w) = stream.split();
        let bytes = serde_json::to_vec(&req).map_err(|e| ClientError::Transport(e.to_string()))?;
        proto::write_frame(&mut w, &bytes)
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?;
        let frame = proto::read_frame(&mut r)
            .await
            .map_err(|e| ClientError::Transport(e.to_string()))?
            .ok_or_else(|| ClientError::Transport("daemon closed the connection".into()))?;
        let resp: Response =
            serde_json::from_slice(&frame).map_err(|e| ClientError::Transport(e.to_string()))?;
        if resp.ok {
            Ok(resp.result.unwrap_or(Value::Null))
        } else {
            let err = resp.error.unwrap_or(WireError {
                kind: ErrorKind::Internal,
                message: "daemon returned an error with no detail".into(),
            });
            Err(ClientError::Daemon { kind: err.kind, message: err.message })
        }
    }

    /// Issue a request whose params come from any serializable value.
    pub async fn call_serialize<Q: Serialize + ?Sized>(&self, op: &str, q: &Q) -> Result<Value> {
        let params = serde_json::to_value(q).map_err(|e| ClientError::Transport(e.to_string()))?;
        self.call(op, params).await
    }

    pub async fn health(&self) -> Result<Value> {
        self.call(op::HEALTH, serde_json::json!({})).await
    }

    pub async fn diagnostics<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.call_serialize(op::DIAGNOSTICS, q).await
    }

    pub async fn goal<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.call_serialize(op::GOAL, q).await
    }

    pub async fn sorries<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.call_serialize(op::SORRY, q).await
    }

    pub async fn list<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.call_serialize(op::LIST, q).await
    }

    pub async fn references<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.call_serialize(op::REFERENCES, q).await
    }

    pub async fn restart_file<Q: Serialize + ?Sized>(&self, q: &Q) -> Result<Value> {
        self.call_serialize(op::RESTART_FILE, q).await
    }
}

/// Return a client for `worktree`'s daemon, auto-spawning it if nothing is
/// listening. Concurrent callers racing here are safe: the daemon binds the
/// socket exclusively, so at most one survives and the rest connect to it.
pub async fn connect_or_spawn(worktree: &Path) -> Result<DaemonClient> {
    let socket = paths::socket_path(worktree);
    if can_connect(&socket).await {
        return Ok(DaemonClient::new(socket));
    }
    ensure_running(worktree, &socket).await?;
    Ok(DaemonClient::new(socket))
}

/// Whether the daemon socket accepts a connection right now (no spawn).
pub async fn can_connect(socket: &Path) -> bool {
    UnixStream::connect(socket).await.is_ok()
}

async fn ensure_running(worktree: &Path, socket: &Path) -> Result<()> {
    if can_connect(socket).await {
        return Ok(());
    }

    // Always (try to) spawn: a duplicate launch exits immediately when another
    // daemon already holds the worktree's startup lock, so this is cheap and
    // avoids guessing about liveness from a possibly stale pid file.
    spawn_daemon(worktree)?;

    // Then wait for *some* daemon to bind the socket. The winner publishes its
    // pid before the (potentially slow) Mathlib cache fetch + `lake serve`
    // warmup, so we keep waiting as long as a daemon is demonstrably alive,
    // capped by `STARTUP_HARD_CAP`. If nothing comes alive within the grace
    // window, the spawn didn't take — fail fast.
    let pid_file = paths::pid_path(worktree);
    let hard_deadline = Instant::now() + STARTUP_HARD_CAP;
    let mut grace_deadline = Instant::now() + STARTUP_GRACE;
    loop {
        if can_connect(socket).await {
            return Ok(());
        }
        let now = Instant::now();
        if now >= hard_deadline {
            return Err(ClientError::StartupTimeout);
        }
        if daemon_alive(&pid_file) {
            grace_deadline = now + STARTUP_GRACE;
        } else if now >= grace_deadline {
            return Err(ClientError::StartupTimeout);
        }
        tokio::time::sleep(STARTUP_POLL).await;
    }
}

/// Whether the daemon recorded in `pid_file` is a live process. Used to decide
/// whether to keep waiting through a slow warmup rather than time out.
fn daemon_alive(pid_file: &Path) -> bool {
    let Ok(contents) = std::fs::read_to_string(pid_file) else {
        return false;
    };
    let Ok(pid) = contents.trim().parse::<i32>() else {
        return false;
    };
    if pid <= 1 {
        return false;
    }
    // `kill(pid, 0)` probes existence without signaling: 0 => alive; EPERM =>
    // alive but owned by another user; ESRCH => no such process.
    let rc = unsafe { libc::kill(pid, 0) };
    rc == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::EPERM)
}

fn spawn_daemon(worktree: &Path) -> Result<()> {
    let bin = resolve_binary()?;
    let log = paths::log_path(worktree);
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|e| ClientError::Spawn(format!("cannot open log {}: {e}", log.display())))?;
    let err = out
        .try_clone()
        .map_err(|e| ClientError::Spawn(e.to_string()))?;

    let mut cmd = std::process::Command::new(&bin);
    cmd.arg(worktree)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(out))
        .stderr(std::process::Stdio::from(err))
        // New process group so the daemon outlives the caller (the agent's MCP
        // process or a one-shot CLI invocation) and ignores its terminal signals.
        .process_group(0);
    cmd.spawn()
        .map_err(|e| ClientError::Spawn(format!("{}: {e}", bin.display())))?;
    Ok(())
}

/// Resolve the `leanckd` binary: `LEANCKD_BIN` override, then a sibling of the
/// running executable (the plugin staging layout, where `leanck-mcp` and
/// `leanckd` are co-located), then `PATH`.
pub fn resolve_binary() -> Result<PathBuf> {
    if let Some(p) = std::env::var_os("LEANCKD_BIN") {
        let p = PathBuf::from(p);
        if p.exists() {
            return Ok(p);
        }
        return Err(ClientError::Spawn(format!(
            "LEANCKD_BIN set to {} but it does not exist",
            p.display()
        )));
    }
    if let Ok(exe) = std::env::current_exe()
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("leanckd");
        if sibling.exists() {
            return Ok(sibling);
        }
    }
    // Fall back to a `leanckd` found on PATH.
    if let Some(found) = find_on_path("leanckd") {
        return Ok(found);
    }
    Err(ClientError::Spawn(
        "could not find the `leanckd` binary. Install it alongside `leanck` \
         (`cargo install --path leanckd`, or `just build`), set `LEANCKD_BIN=/path/to/leanckd`, \
         or put `leanckd` on PATH."
            .into(),
    ))
}

/// Locate an executable named `name` by scanning `$PATH`. Returns the first
/// match that exists, so the caller can fail early with a clear message rather
/// than letting `spawn()` surface a bare "No such file or directory".
fn find_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}
