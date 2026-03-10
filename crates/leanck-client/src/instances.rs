//! Machine-wide daemon inventory and socket-direct control.
//!
//! Daemons are found through their instance slot records, not through a
//! workspace, so a daemon whose worktree was removed can still be listed and
//! stopped. Nothing here spawns a daemon, retries a request, takes a slot lock,
//! or creates the slot directory.
use crate::{
    PreparedRequest, check_protocol,
    error::{ClientError, Result},
    transport,
};
use leanck_config::{OnLimit, Settings};
use leanck_core::{
    api::{Command, HealthResult, LifecycleState, Operation, ShutdownParams},
    error::ProtocolError,
    instances::{SlotEntry, live_across, now_unix_secs, pid_alive},
    paths::runtime_directories,
};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use tokio::task::JoinSet;
use uuid::Uuid;

/// Interval between liveness checks while waiting for a stopped daemon to exit.
const STOP_POLL: Duration = Duration::from_millis(50);
/// How long eviction waits for a candidate that accepted the stop to exit.
pub const EVICT_WAIT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DaemonState {
    /// Alive, but its socket is not bound yet (warming up).
    Starting,
    Ready,
    Recovering,
    ShuttingDown,
    /// The socket exists but the health probe failed; see `error`.
    Unreachable,
}

impl From<LifecycleState> for DaemonState {
    fn from(state: LifecycleState) -> Self {
        match state {
            LifecycleState::Ready => Self::Ready,
            LifecycleState::Recovering => Self::Recovering,
            LifecycleState::ShuttingDown => Self::ShuttingDown,
        }
    }
}

/// One live daemon, from its slot record and, when reachable, its health.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DaemonInfo {
    pub slot: u32,
    pub pid: u32,
    pub worktree: PathBuf,
    pub socket: PathBuf,
    /// Unix seconds.
    pub started_at: u64,
    pub uptime_secs: u64,
    pub state: DaemonState,
    pub health: Option<HealthResult>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl DaemonInfo {
    pub fn idle_secs(&self) -> Option<u64> {
        self.health.as_ref().map(|health| health.idle_secs)
    }

    pub fn active_requests(&self) -> Option<u64> {
        self.health
            .as_ref()
            .map(|health| health.lifecycle.active_requests)
    }

    /// Ready with no queued or running operations.
    pub fn is_idle(&self) -> bool {
        self.state == DaemonState::Ready && self.active_requests() == Some(0)
    }
}

/// Live daemon count against the configured limit.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub running: u64,
    pub limit: u64,
    pub on_limit: OnLimit,
}

/// Whether a stop is unconditional or conditional on the daemon being idle.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StopMode {
    /// Shut down even if work is queued or running; that work is cancelled.
    Always,
    /// Shut down only if nothing is queued or running. The daemon decides
    /// atomically with admission, so work it has accepted is never cancelled.
    IfIdle,
}

/// The result of [`stop_socket`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stopped {
    /// The daemon exited (or was already gone) within the wait.
    Exited,
    /// The daemon accepted the shutdown but had not exited when the wait ended.
    Running,
    /// Under [`StopMode::IfIdle`], the daemon refused because it was busy.
    Busy,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopOutcome {
    pub pid: Option<u32>,
    pub worktree: PathBuf,
    /// False when a conditional stop was refused because the daemon was busy.
    pub accepted: bool,
    /// Whether the process exited (or, without a pid, its socket vanished) within the wait.
    pub exited: bool,
}

/// Preferred slot directory. Inventory scans all runtime candidates; never creates them.
pub fn slot_directory() -> PathBuf {
    runtime_directories().remove(0)
}

pub fn count_live() -> u64 {
    count_live_across(&runtime_directories())
}

pub fn count_live_in(dir: &Path) -> u64 {
    count_live_across(&[dir.to_path_buf()])
}

pub(crate) fn count_live_across(dirs: &[PathBuf]) -> u64 {
    live_across(dirs).len() as u64
}

pub fn summary(settings: &Settings) -> Summary {
    Summary {
        running: count_live(),
        limit: settings.instances.limit,
        on_limit: settings.instances.on_limit,
    }
}

pub fn summary_in(dir: &Path, settings: &Settings) -> Summary {
    Summary {
        running: count_live_in(dir),
        limit: settings.instances.limit,
        on_limit: settings.instances.on_limit,
    }
}

pub async fn list(settings: &Settings) -> Vec<DaemonInfo> {
    list_across(&runtime_directories(), settings).await
}

/// Every live slot in `dir`, probed concurrently and sorted by slot.
pub async fn list_in(dir: &Path, settings: &Settings) -> Vec<DaemonInfo> {
    list_across(&[dir.to_path_buf()], settings).await
}

async fn list_across(dirs: &[PathBuf], settings: &Settings) -> Vec<DaemonInfo> {
    let timeout = Duration::from_millis(settings.client.probe_timeout_ms);
    let now = now_unix_secs();
    let mut probes = JoinSet::new();
    for SlotEntry { slot, record } in live_across(dirs) {
        probes.spawn(async move {
            let (state, health, error) = if !record.socket.exists() {
                (DaemonState::Starting, None, None)
            } else {
                match probe(&record.socket, timeout).await {
                    Ok(health) => (health.lifecycle.state.into(), Some(health), None),
                    Err(error) => (DaemonState::Unreachable, None, Some(error.to_string())),
                }
            };
            DaemonInfo {
                slot,
                pid: record.pid,
                uptime_secs: now.saturating_sub(record.started_at),
                worktree: record.worktree,
                socket: record.socket,
                started_at: record.started_at,
                state,
                health,
                error,
            }
        });
    }
    let mut daemons = Vec::new();
    while let Some(result) = probes.join_next().await {
        // A probe task only panics on a bug; omit that daemon rather than fail the listing.
        if let Ok(info) = result {
            daemons.push(info);
        }
    }
    daemons.sort_by_key(|info| info.slot);
    daemons
}

/// Health of the daemon at `socket`, protocol-checked. Never starts a daemon,
/// retries, or sends a cancellation.
pub async fn probe(socket: &Path, timeout: Duration) -> Result<HealthResult> {
    let request = PreparedRequest::new(Command::Health, format!("probe-{}", Uuid::new_v4()), None)?;
    let value = transport::call_with_options(socket, &request, timeout, None, false).await?;
    check_protocol(&value)?;
    serde_json::from_value(value).map_err(|source| {
        ClientError::Protocol(ProtocolError::InvalidResult {
            operation: Operation::Health,
            source,
        })
    })
}

/// Ask the daemon at `socket` to shut down, then wait up to `wait` for it to
/// exit: poll `pid` every 50 ms, or without a pid, until the socket vanishes.
/// Under [`StopMode::IfIdle`], a daemon that answers `shutting_down: false`
/// is busy and is not waited for. A daemon that is already gone counts as
/// exited. The shutdown request is not protocol-checked, so an incompatible
/// daemon can still be stopped; a daemon too old to know `if_idle` stops
/// unconditionally.
pub async fn stop_socket(
    socket: &Path,
    pid: Option<u32>,
    settings: &Settings,
    wait: Duration,
    mode: StopMode,
) -> Result<Stopped> {
    let alive = || pid.map_or_else(|| socket.exists(), pid_alive);
    let params = ShutdownParams {
        if_idle: mode == StopMode::IfIdle,
    };
    let request = PreparedRequest::new(
        Command::Shutdown(params),
        format!("stop-{}", Uuid::new_v4()),
        None,
    )?;
    match transport::call_with_options(
        socket,
        &request,
        Duration::from_millis(settings.client.probe_timeout_ms),
        None,
        false,
    )
    .await
    {
        // Only an explicit refusal is busy; any other answer is acceptance.
        Ok(value) if value.get("shutting_down").and_then(|v| v.as_bool()) == Some(false) => {
            return Ok(Stopped::Busy);
        }
        Ok(_) => {}
        Err(error) if error.is_pre_dispatch() && !alive() => return Ok(Stopped::Exited),
        Err(error) => return Err(error),
    }
    let deadline = Instant::now() + wait;
    loop {
        if !alive() {
            return Ok(Stopped::Exited);
        }
        if Instant::now() >= deadline {
            return Ok(Stopped::Running);
        }
        tokio::time::sleep(STOP_POLL).await;
    }
}

/// [`stop_socket`] for a listed daemon.
pub async fn stop(
    info: &DaemonInfo,
    settings: &Settings,
    wait: Duration,
    mode: StopMode,
) -> Result<StopOutcome> {
    let stopped = stop_socket(&info.socket, Some(info.pid), settings, wait, mode).await?;
    Ok(StopOutcome {
        pid: Some(info.pid),
        worktree: info.worktree.clone(),
        accepted: stopped != Stopped::Busy,
        exited: stopped == Stopped::Exited,
    })
}

/// A daemon that accepted an eviction.
#[derive(Debug, Clone, PartialEq)]
pub struct Evicted {
    pub daemon: DaemonInfo,
    /// Whether it exited within [`EVICT_WAIT`]; if not, its slot frees once it does.
    pub exited: bool,
}

pub async fn evict_idle(settings: &Settings, needed: usize) -> Result<Vec<Evicted>> {
    evict_idle_across(&runtime_directories(), settings, needed).await
}

/// Stop up to `needed` of the longest-idle daemons in `dir` (ties go to the
/// lowest slot). Only daemons that are ready with no queued or running
/// operations are candidates; starting, recovering, shutting-down, and
/// unreachable daemons are never touched. Each candidate is asked to stop only
/// if it is still idle, which the daemon decides atomically with admission, so
/// a candidate that accepted work after the listing stays up and the next one
/// is tried. A candidate that accepted counts toward `needed` even if it has
/// not exited within [`EVICT_WAIT`]: it is already shutting down, so no other
/// daemon is stopped in its place. Nothing is ever killed. Returns fewer than
/// `needed` when too few daemons could be evicted, or the first stop error when
/// none was evicted and an attempt failed with one.
pub async fn evict_idle_in(dir: &Path, settings: &Settings, needed: usize) -> Result<Vec<Evicted>> {
    evict_idle_across(&[dir.to_path_buf()], settings, needed).await
}

pub(crate) async fn evict_idle_across(
    dirs: &[PathBuf],
    settings: &Settings,
    needed: usize,
) -> Result<Vec<Evicted>> {
    evict_across_waiting(dirs, settings, needed, EVICT_WAIT).await
}

#[cfg(test)]
async fn evict_idle_waiting(
    dir: &Path,
    settings: &Settings,
    needed: usize,
    wait: Duration,
) -> Result<Vec<Evicted>> {
    evict_across_waiting(&[dir.to_path_buf()], settings, needed, wait).await
}

async fn evict_across_waiting(
    dirs: &[PathBuf],
    settings: &Settings,
    needed: usize,
    wait: Duration,
) -> Result<Vec<Evicted>> {
    if needed == 0 {
        return Ok(Vec::new());
    }
    let mut candidates: Vec<DaemonInfo> = list_across(dirs, settings)
        .await
        .into_iter()
        .filter(DaemonInfo::is_idle)
        .collect();
    candidates.sort_by(|a, b| {
        b.idle_secs()
            .cmp(&a.idle_secs())
            .then_with(|| a.slot.cmp(&b.slot))
    });
    let mut evicted = Vec::new();
    let mut first_error = None;
    for candidate in candidates {
        if evicted.len() == needed {
            break;
        }
        match stop(&candidate, settings, wait, StopMode::IfIdle).await {
            Ok(outcome) if outcome.accepted => evicted.push(Evicted {
                daemon: candidate,
                exited: outcome.exited,
            }),
            // Busy at shutdown: try the next one.
            Ok(_) => {}
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    match first_error {
        Some(error) if evicted.is_empty() => Err(error),
        _ => Ok(evicted),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Scratch;
    use leanck_core::{
        api::{LifecycleHealth, PROTOCOL_VERSION, Request, Response, ServerHealth, op},
        instances::{SlotRecord, slot_path},
        wire,
    };
    use serde_json::json;
    use std::sync::{Arc, Mutex};
    use tokio::net::UnixListener;

    fn health(state: LifecycleState, active: u64, idle: u64) -> HealthResult {
        HealthResult {
            ok: state == LifecycleState::Ready,
            workspace: "/wt".into(),
            protocol_version: PROTOCOL_VERSION,
            server: ServerHealth {
                running: true,
                pid: Some(7),
            },
            idle_secs: idle,
            lifecycle: LifecycleHealth {
                state,
                active_requests: active,
            },
        }
    }

    /// Answer each connection's single request with `answer(request)`. Stops
    /// after answering a shutdown that it accepted.
    fn serve(
        listener: UnixListener,
        answer: impl Fn(&Request) -> serde_json::Value + Send + 'static,
    ) -> tokio::task::JoinHandle<Vec<String>> {
        tokio::spawn(async move {
            let mut methods = Vec::new();
            while let Ok(Ok((stream, _))) =
                tokio::time::timeout(Duration::from_millis(500), listener.accept()).await
            {
                let mut framed = wire::framed(stream);
                let Some(frame) = wire::read_frame(&mut framed).await.unwrap() else {
                    continue;
                };
                let request: Request = serde_json::from_slice(&frame).unwrap();
                methods.push(request.op.clone());
                let result = answer(&request);
                let accepted = result["shutting_down"] != json!(false);
                let shutdown = request.op == op::SHUTDOWN;
                let response = Response::ok(request.id, result);
                wire::write_frame(&mut framed, &serde_json::to_vec(&response).unwrap())
                    .await
                    .unwrap();
                if shutdown && accepted {
                    break;
                }
            }
            methods
        })
    }

    fn write_record(dir: &Path, slot: u32, record: &SlotRecord) {
        std::fs::create_dir_all(dir).unwrap();
        std::fs::write(
            slot_path(dir, slot),
            format!("{}\n", serde_json::to_string(record).unwrap()),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn listing_reports_starting_ready_and_unreachable_daemons_by_slot() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("slots");
        assert_eq!(count_live_in(&dir), 0);
        assert!(list_in(&dir, &Settings::default()).await.is_empty());
        assert!(!dir.exists(), "listing must not create the slot directory");

        let pid = std::process::id();
        let ready = scratch.0.join("ready.sock");
        let broken = scratch.0.join("broken.sock");
        let now = now_unix_secs();
        // A record dated before this process started would read as a reused
        // pid, so date it at the process start where that is known.
        let started_at = leanck_core::instances::process_start_time(pid).unwrap_or(now - 30);
        for (slot, socket) in [
            (2, ready.clone()),
            (0, scratch.0.join("absent.sock")),
            (1, broken.clone()),
        ] {
            write_record(
                &dir,
                slot,
                &SlotRecord {
                    pid,
                    worktree: scratch.0.join(format!("wt{slot}")),
                    socket,
                    started_at,
                    start_id: None,
                },
            );
        }
        let answered = health(LifecycleState::Ready, 0, 42);
        let reply = serde_json::to_value(&answered).unwrap();
        let server = serve(UnixListener::bind(&ready).unwrap(), move |_| reply.clone());
        let stale = UnixListener::bind(&broken).unwrap();
        drop(stale);

        let mut settings = Settings::default();
        settings.client.probe_timeout_ms = 300;
        let daemons = list_in(&dir, &settings).await;
        assert_eq!(
            daemons
                .iter()
                .map(|d| (d.slot, d.state))
                .collect::<Vec<_>>(),
            vec![
                (0, DaemonState::Starting),
                (1, DaemonState::Unreachable),
                (2, DaemonState::Ready),
            ]
        );
        assert!(daemons[0].health.is_none() && daemons[0].error.is_none());
        assert!(daemons[1].error.is_some());
        assert_eq!(daemons[2].health.as_ref(), Some(&answered));
        assert_eq!(daemons[2].idle_secs(), Some(42));
        assert_eq!(daemons[2].active_requests(), Some(0));
        assert!(daemons[2].is_idle() && !daemons[0].is_idle() && !daemons[1].is_idle());
        let uptime = now - started_at;
        assert!((uptime..uptime + 10).contains(&daemons[2].uptime_secs));
        let summary = summary_in(&dir, &settings);
        assert_eq!(
            serde_json::to_value(&summary).unwrap(),
            json!({"running": 3, "limit": 8, "on_limit": "evict_idle"})
        );
        let encoded = serde_json::to_value(&daemons[0]).unwrap();
        assert_eq!(encoded["state"], "starting");
        assert!(encoded.get("error").is_none());
        assert!(encoded["health"].is_null());
        server.abort();
    }

    #[tokio::test]
    async fn probe_rejects_incompatible_protocols_and_stop_waits_for_exit() {
        let scratch = Scratch::new();
        let socket = scratch.0.join("old.sock");
        let mut old = serde_json::to_value(health(LifecycleState::Ready, 0, 1)).unwrap();
        old["protocol_version"] = json!(1);
        let server = serve(UnixListener::bind(&socket).unwrap(), move |_| old.clone());
        assert!(matches!(
            probe(&socket, Duration::from_secs(1)).await,
            Err(ClientError::IncompatibleProtocol { actual: 1, .. })
        ));
        server.abort();
        let _ = server.await;
        std::fs::remove_file(&socket).unwrap();

        // Without a pid, stopping waits until the socket vanishes.
        let listener = UnixListener::bind(&socket).unwrap();
        let path = socket.clone();
        let server = serve(listener, |_| json!({"shutting_down": true}));
        let remover = tokio::spawn(async move {
            let methods = server.await.unwrap();
            tokio::time::sleep(Duration::from_millis(100)).await;
            std::fs::remove_file(path).unwrap();
            methods
        });
        assert_eq!(
            stop_socket(
                &socket,
                None,
                &Settings::default(),
                Duration::from_secs(3),
                StopMode::Always
            )
            .await
            .unwrap(),
            Stopped::Exited
        );
        assert_eq!(remover.await.unwrap(), vec![op::SHUTDOWN.to_owned()]);

        // An already-gone daemon counts as stopped; a live pid that never exits does not.
        assert_eq!(
            stop_socket(
                &socket,
                None,
                &Settings::default(),
                Duration::from_secs(1),
                StopMode::IfIdle
            )
            .await
            .unwrap(),
            Stopped::Exited
        );
        let server = serve(
            UnixListener::bind(&socket).unwrap(),
            |_| json!({"shutting_down": true}),
        );
        let started = Instant::now();
        assert_eq!(
            stop_socket(
                &socket,
                Some(std::process::id()),
                &Settings::default(),
                Duration::from_millis(150),
                StopMode::Always
            )
            .await
            .unwrap(),
            Stopped::Running
        );
        assert!(started.elapsed() >= Duration::from_millis(150));
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_accepted_eviction_that_exits_slowly_is_not_replaced() {
        let scratch = Scratch::new();
        let dir = scratch.0.join("slots");
        // Both records name this process, which never exits during the test.
        let pid = std::process::id();
        let started_at =
            leanck_core::instances::process_start_time(pid).unwrap_or(now_unix_secs() - 30);
        let mut servers = Vec::new();
        for (slot, idle) in [(0, 10), (1, 300)] {
            let socket = scratch.0.join(format!("d{slot}.sock"));
            write_record(
                &dir,
                slot,
                &SlotRecord {
                    pid,
                    worktree: scratch.0.join(format!("wt{slot}")),
                    socket: socket.clone(),
                    started_at,
                    start_id: None,
                },
            );
            servers.push(serve(
                UnixListener::bind(&socket).unwrap(),
                move |request| {
                    if request.op == op::SHUTDOWN {
                        json!({"shutting_down": true})
                    } else {
                        serde_json::to_value(health(LifecycleState::Ready, 0, idle)).unwrap()
                    }
                },
            ));
        }
        let mut settings = Settings::default();
        settings.client.probe_timeout_ms = 300;
        let evicted = evict_idle_waiting(&dir, &settings, 1, Duration::from_millis(100))
            .await
            .unwrap();
        assert_eq!(evicted.len(), 1);
        assert_eq!(evicted[0].daemon.slot, 1);
        assert!(!evicted[0].exited);
        let recent = servers.remove(0).await.unwrap();
        assert_eq!(recent, vec![op::HEALTH.to_owned()], "never asked to stop");
        let slow = servers.remove(0).await.unwrap();
        assert_eq!(slow.last().map(String::as_str), Some(op::SHUTDOWN));
    }

    #[tokio::test]
    async fn conditional_stop_sends_if_idle_and_a_busy_refusal_is_not_waited_for() {
        let scratch = Scratch::new();
        let socket = scratch.0.join("busy.sock");
        // Refuse a conditional shutdown, accept an unconditional one.
        let params = Arc::new(Mutex::new(Vec::new()));
        let seen = params.clone();
        let server = serve(UnixListener::bind(&socket).unwrap(), move |request| {
            seen.lock().unwrap().push(request.params.clone());
            json!({"shutting_down": request.params.get("if_idle").is_none()})
        });
        let info = DaemonInfo {
            slot: 0,
            pid: std::process::id(),
            worktree: scratch.0.join("wt"),
            socket: socket.clone(),
            started_at: now_unix_secs(),
            uptime_secs: 0,
            state: DaemonState::Ready,
            health: None,
            error: None,
        };
        let started = Instant::now();
        let refused = stop(
            &info,
            &Settings::default(),
            Duration::from_secs(5),
            StopMode::IfIdle,
        )
        .await
        .unwrap();
        assert!(
            started.elapsed() < Duration::from_secs(2),
            "busy is immediate"
        );
        assert_eq!(
            serde_json::to_value(&refused).unwrap(),
            json!({
                "pid": std::process::id(),
                "worktree": info.worktree,
                "accepted": false,
                "exited": false,
            })
        );
        // Our own pid never exits, so an accepted stop runs out its wait.
        let accepted = stop(
            &info,
            &Settings::default(),
            Duration::from_millis(100),
            StopMode::Always,
        )
        .await
        .unwrap();
        assert!(accepted.accepted && !accepted.exited);
        assert_eq!(
            server.await.unwrap(),
            vec![op::SHUTDOWN.to_owned(), op::SHUTDOWN.to_owned()]
        );
        assert_eq!(
            *params.lock().unwrap(),
            vec![json!({"if_idle": true}), json!({})]
        );
    }
}
