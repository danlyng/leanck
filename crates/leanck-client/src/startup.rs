//! Auto-start policy, including warming liveness, the existing binary search
//! order, the machine-wide instance limit, and idle eviction.
use crate::{
    discovery::resolve_binary,
    error::{ClientError, Result},
    instances::{EVICT_WAIT, count_live_in, evict_idle_in},
    transport::can_connect,
};
use leanck_config::{OnLimit, Settings};
use leanck_core::{
    instances::{EXIT_INSTANCE_LIMIT, live, pid_alive},
    paths::RuntimePaths,
};
use std::fs::{File, TryLockError};
use std::os::unix::process::ExitStatusExt;
use std::path::Path;
use std::time::{Duration, Instant};
use tokio::process::Child;

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

/// Spawn attempts under `evict_idle`; a daemon that loses a slot race exits
/// with `EXIT_INSTANCE_LIMIT`, and the client evicts and retries.
const SPAWN_ATTEMPTS: u32 = 3;

/// Whether the daemon recorded in `pid_file` is a live process. Used to decide
/// whether to keep waiting through a slow warmup rather than time out.
fn daemon_alive(pid_file: &Path) -> bool {
    std::fs::read_to_string(pid_file)
        .ok()
        .and_then(|contents| contents.trim().parse::<u32>().ok())
        .is_some_and(leanck_core::instances::pid_alive)
}

fn spawn_daemon(
    bin: &Path,
    worktree: &Path,
    runtime: &RuntimePaths,
    settings: &Settings,
) -> Result<Child> {
    let log = runtime.log();
    let out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log)
        .map_err(|e| ClientError::Log {
            path: log.clone(),
            source: e,
        })?;
    let err = out.try_clone().map_err(|e| ClientError::Log {
        path: log.clone(),
        source: e,
    })?;

    let mut cmd = tokio::process::Command::new(bin);
    cmd.arg(worktree).arg("--no-config");
    for assignment in settings.daemon_assignments() {
        cmd.arg("--set").arg(assignment);
    }
    cmd.stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::from(out))
        .stderr(std::process::Stdio::from(err))
        // New process group so the daemon outlives the caller (the agent's MCP
        // process or a one-shot CLI invocation) and ignores its terminal signals.
        .process_group(0)
        // Outlive this operation while Tokio reaps exited detached children.
        .kill_on_drop(false);
    cmd.spawn().map_err(|e| ClientError::Spawn {
        binary: bin.to_path_buf(),
        source: e,
    })
}

/// Wait up to `wait` for the `pending` pids to exit, keeping those still alive.
async fn wait_for_exits(pending: &mut Vec<u32>, wait: Duration) {
    let deadline = Instant::now() + wait;
    loop {
        pending.retain(|&pid| pid_alive(pid));
        if pending.is_empty() || Instant::now() >= deadline {
            return;
        }
        tokio::time::sleep(STARTUP_POLL).await;
    }
}

/// Enforce the instance limit before a spawn: refuse, warn, or evict the
/// longest-idle daemons until a slot is free. `exiting` holds the pids of
/// daemons evicted by an earlier attempt that had not exited yet. Their slots
/// are about to free, so they are waited for and never replaced by another
/// eviction; evictions that have not exited are added to it.
async fn make_room(slot_dir: &Path, settings: &Settings, exiting: &mut Vec<u32>) -> Result<()> {
    let limit = settings.instances.limit;
    if limit == 0 {
        return Ok(());
    }
    wait_for_exits(exiting, EVICT_WAIT).await;
    let running = count_live_in(slot_dir);
    // Free slots needed for one more daemon, beyond those already freeing.
    let needed = (running + 1)
        .saturating_sub(limit)
        .saturating_sub(exiting.len() as u64);
    if needed == 0 {
        return Ok(());
    }
    match settings.instances.on_limit {
        OnLimit::Refuse => Err(ClientError::InstanceLimit {
            running,
            limit,
            busy: false,
        }),
        OnLimit::Warn => {
            eprintln!(
                "warning: {running} leanck daemons running (instances.limit = {limit}); starting another"
            );
            Ok(())
        }
        OnLimit::EvictIdle => {
            let needed = usize::try_from(needed).unwrap_or(usize::MAX);
            let evicted = evict_idle_in(slot_dir, settings, needed).await?;
            exiting.extend(
                evicted
                    .iter()
                    .filter(|eviction| !eviction.exited)
                    .map(|eviction| eviction.daemon.pid),
            );
            if evicted.len() < needed {
                return Err(ClientError::InstanceLimit {
                    running: count_live_in(slot_dir),
                    limit,
                    busy: true,
                });
            }
            Ok(())
        }
    }
}

/// The caller's turn to start a worktree's daemon, or the daemon is serving.
enum StartTurn {
    /// Holds the worktree's start lock until dropped.
    Mine(File),
    Served,
}

/// Wait for exclusive use of the worktree's start lock, in this process or any
/// other, so concurrent cold starts make room and spawn once. Returns
/// [`StartTurn::Served`] as soon as the socket accepts connections instead.
async fn take_start_turn(runtime: &RuntimePaths, policy: StartupPolicy) -> Result<StartTurn> {
    let prepare = |source| ClientError::Prepare {
        directory: runtime.directory().to_path_buf(),
        source,
    };
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(runtime.start_lock())
        .map_err(prepare)?;
    let deadline = Instant::now() + policy.hard_cap;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(StartTurn::Mine(file)),
            Err(TryLockError::WouldBlock) => {}
            Err(TryLockError::Error(source)) => return Err(prepare(source)),
        }
        if can_connect(&runtime.socket()).await {
            return Ok(StartTurn::Served);
        }
        if Instant::now() >= deadline {
            return Err(ClientError::StartupTimeout);
        }
        tokio::time::sleep(policy.poll).await;
    }
}

/// Prepare and start only when the selected socket is absent. Clients starting
/// the same worktree take turns on its start lock, so only one makes room; the
/// daemon's lifetime lock still serializes duplicate launches.
pub(crate) async fn ensure_running(
    worktree: &Path,
    runtime: RuntimePaths,
    settings: &Settings,
) -> Result<RuntimePaths> {
    if can_connect(&runtime.socket()).await {
        return Ok(runtime);
    }
    let directory = runtime.directory().to_path_buf();
    let runtime = runtime
        .prepare()
        .map_err(|source| ClientError::Prepare { directory, source })?;
    if can_connect(&runtime.socket()).await {
        return Ok(runtime);
    }
    let binary = match &settings.client.daemon_bin {
        Some(path) if path.is_file() => path.clone(),
        Some(path) => return Err(ClientError::InvalidBinary(path.clone())),
        None => resolve_binary()?,
    };
    let policy = StartupPolicy {
        grace: Duration::from_millis(settings.client.startup_grace_ms),
        hard_cap: Duration::from_millis(settings.client.startup_timeout_ms),
        ..StartupPolicy::default()
    };
    let slot_dir = (!runtime.is_fallback()).then(|| runtime.directory().to_path_buf());
    if slot_dir.is_none() {
        eprintln!(
            "warning: leanck runtime directory fell back to {}; this daemon is not counted against instances.limit",
            runtime.directory().display()
        );
    }
    let _turn = match take_start_turn(&runtime, policy).await? {
        StartTurn::Served => return Ok(runtime),
        StartTurn::Mine(file) => file,
    };
    if can_connect(&runtime.socket()).await {
        return Ok(runtime);
    }
    let mut exiting = Vec::new();
    let mut attempt = 1;
    loop {
        // A live slot for this worktree's own socket is its daemon warming up:
        // the spawn below only waits for it, so it needs no room.
        if let Some(dir) = &slot_dir
            && !live(dir)
                .iter()
                .any(|entry| entry.record.socket == runtime.socket())
        {
            make_room(dir, settings, &mut exiting).await?;
        }
        let child = spawn_daemon(&binary, worktree, &runtime, settings)?;
        let watch = Watch {
            child,
            slot_dir: slot_dir.as_deref(),
            limit: settings.instances.limit,
        };
        match wait_for_daemon(&runtime, Some(watch), policy).await {
            Err(ClientError::InstanceLimit { .. })
                if settings.instances.on_limit == OnLimit::EvictIdle
                    && slot_dir.is_some()
                    && attempt < SPAWN_ATTEMPTS =>
            {
                attempt += 1;
            }
            result => return result.map(|()| runtime),
        }
    }
}

#[derive(Clone, Copy)]
struct StartupPolicy {
    grace: Duration,
    hard_cap: Duration,
    poll: Duration,
}
impl Default for StartupPolicy {
    fn default() -> Self {
        Self {
            grace: STARTUP_GRACE,
            hard_cap: STARTUP_HARD_CAP,
            poll: STARTUP_POLL,
        }
    }
}

/// The spawned daemon process, watched for an early exit while waiting.
struct Watch<'a> {
    child: Child,
    /// Where to count running daemons if the child refuses over the limit.
    slot_dir: Option<&'a Path>,
    limit: u64,
}

/// Exit status as a code, or 128 plus the signal number.
fn exit_code(status: std::process::ExitStatus) -> i32 {
    status
        .code()
        .or_else(|| status.signal().map(|signal| 128 + signal))
        .unwrap_or(-1)
}

#[cfg(test)]
async fn wait_for_socket(runtime: &RuntimePaths, policy: StartupPolicy) -> Result<()> {
    wait_for_daemon(runtime, None, policy).await
}

/// Wait for the socket to accept connections. While a spawned child is
/// watched, its exit ends the wait at once: `EXIT_INSTANCE_LIMIT` is an
/// instance-limit refusal and any other failure is reported with the log path.
/// A clean exit means another daemon owns the worktree, so waiting continues on
/// the pid file alone.
async fn wait_for_daemon(
    runtime: &RuntimePaths,
    mut watch: Option<Watch<'_>>,
    policy: StartupPolicy,
) -> Result<()> {
    let hard_deadline = Instant::now() + policy.hard_cap;
    let mut grace_deadline = Instant::now() + policy.grace;
    loop {
        if can_connect(&runtime.socket()).await {
            return Ok(());
        }
        if let Some(spawned) = &mut watch
            && let Ok(Some(status)) = spawned.child.try_wait()
        {
            match exit_code(status) {
                0 => watch = None,
                EXIT_INSTANCE_LIMIT => {
                    return Err(ClientError::InstanceLimit {
                        running: spawned.slot_dir.map_or(0, count_live_in),
                        limit: spawned.limit,
                        busy: false,
                    });
                }
                code => {
                    return Err(ClientError::DaemonExited {
                        code,
                        log: runtime.log(),
                    });
                }
            }
        }
        let now = Instant::now();
        if now >= hard_deadline {
            return Err(ClientError::StartupTimeout);
        }
        if daemon_alive(&runtime.pid()) {
            grace_deadline = now + policy.grace;
        } else if now >= grace_deadline {
            return Err(ClientError::StartupTimeout);
        }
        tokio::time::sleep(policy.poll).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::{Scratch, request, respond};
    use leanck_core::{
        api::{
            HealthResult, LifecycleHealth, LifecycleState, PROTOCOL_VERSION, Response,
            ServerHealth, op,
        },
        instances::{SlotRecord, now_unix_secs, pid_alive, slot_path},
    };
    use serde_json::json;
    use std::path::PathBuf;
    use std::sync::{Arc, Mutex};
    use tokio::net::UnixListener;

    #[tokio::test]
    async fn startup_waits_through_live_warmup_beyond_grace() {
        let scratch = Scratch::new();
        let runtime = RuntimePaths::in_directory(scratch.0.join("r"), &scratch.0)
            .prepare()
            .unwrap();
        std::fs::write(runtime.pid(), std::process::id().to_string()).unwrap();
        let socket = runtime.socket();
        let binder = tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(60)).await;
            UnixListener::bind(socket).unwrap()
        });
        wait_for_socket(
            &runtime,
            StartupPolicy {
                grace: Duration::from_millis(20),
                hard_cap: Duration::from_secs(2),
                poll: Duration::from_millis(5),
            },
        )
        .await
        .unwrap();
        let _listener = binder.await.unwrap();
    }

    #[tokio::test]
    async fn startup_has_grace_and_absolute_caps() {
        let scratch = Scratch::new();
        let runtime = RuntimePaths::in_directory(scratch.0.join("r"), &scratch.0)
            .prepare()
            .unwrap();
        let grace = StartupPolicy {
            grace: Duration::from_millis(20),
            hard_cap: Duration::from_secs(10),
            poll: Duration::from_millis(5),
        };
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), wait_for_socket(&runtime, grace))
                .await
                .unwrap(),
            Err(ClientError::StartupTimeout)
        ));
        std::fs::write(runtime.pid(), std::process::id().to_string()).unwrap();
        let live = StartupPolicy {
            hard_cap: Duration::from_millis(80),
            ..grace
        };
        assert!(matches!(
            tokio::time::timeout(Duration::from_secs(1), wait_for_socket(&runtime, live))
                .await
                .unwrap(),
            Err(ClientError::StartupTimeout)
        ));
        for contents in ["invalid", "0", "-1"] {
            std::fs::write(runtime.pid(), contents).unwrap();
            assert!(!daemon_alive(&runtime.pid()));
        }
    }

    #[tokio::test]
    async fn configured_launch_preserves_workspace_argument_and_log_redirection() {
        use std::os::unix::fs::PermissionsExt;
        let scratch = Scratch::new();
        let runtime = RuntimePaths::in_directory(scratch.0.join("r"), &scratch.0)
            .prepare()
            .unwrap();
        let binary = scratch.0.join("fake-leanckd");
        std::fs::write(&binary, "#!/bin/sh\nprintf '%s' \"$1\" > \"$1/started\"\nprintf 'stdout\\n'\nprintf 'stderr\\n' >&2\nprintf '%s' \"$$\" > \"$1/child-pid\"\nprintf 'done' > \"$1/finished\"\n").unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut settings = leanck_config::Settings::default();
        settings.client.daemon_bin = Some(scratch.0.join("missing"));
        assert!(matches!(
            ensure_running(&scratch.0, runtime.clone(), &settings).await,
            Err(ClientError::InvalidBinary(_))
        ));
        settings.client.daemon_bin = Some(binary);
        let finished = scratch.0.join("finished");
        let socket = runtime.socket();
        let binder = tokio::spawn(async move {
            while !finished.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            UnixListener::bind(socket).unwrap()
        });
        tokio::time::timeout(
            Duration::from_secs(2),
            ensure_running(&scratch.0, runtime.clone(), &settings),
        )
        .await
        .unwrap()
        .unwrap();
        let _listener = binder.await.unwrap();
        assert_eq!(
            std::fs::read_to_string(scratch.0.join("started")).unwrap(),
            scratch.0.to_string_lossy()
        );
        let log = std::fs::read_to_string(runtime.log()).unwrap();
        assert!(log.contains("stdout") && log.contains("stderr"));
        let pid = std::fs::read_to_string(scratch.0.join("child-pid"))
            .unwrap()
            .parse::<i32>()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while unsafe { libc::kill(pid, 0) } == 0 {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("exited detached child reaped while the client runtime remains alive");
    }

    fn private_runtime(scratch: &Scratch) -> RuntimePaths {
        RuntimePaths::in_directory(scratch.0.join("r"), &scratch.0)
            .prepare()
            .unwrap()
    }

    /// A fake `leanckd` that runs `body`; `$1` is the worktree.
    fn fake_binary(scratch: &Scratch, body: &str) -> PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let binary = scratch.0.join("fake-leanckd");
        std::fs::write(&binary, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&binary, std::fs::Permissions::from_mode(0o700)).unwrap();
        binary
    }

    /// Marks the spawn, then exits 0 as if another daemon owned the worktree.
    const MARKING: &str = "printf x > \"$1/spawned\"\nprintf done > \"$1/finished\"";

    fn limited(binary: PathBuf, limit: u64, on_limit: OnLimit) -> Settings {
        let mut settings = Settings::default();
        settings.client.daemon_bin = Some(binary);
        settings.client.probe_timeout_ms = 500;
        settings.instances.limit = limit;
        settings.instances.on_limit = on_limit;
        settings
    }

    /// Bind the runtime socket once the fake daemon writes `finished`.
    fn bind_when_finished(
        scratch: &Scratch,
        runtime: &RuntimePaths,
    ) -> tokio::task::JoinHandle<UnixListener> {
        let finished = scratch.0.join("finished");
        let socket = runtime.socket();
        tokio::spawn(async move {
            while !finished.exists() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            UnixListener::bind(socket).unwrap()
        })
    }

    fn write_record(dir: &Path, slot: u32, pid: u32, socket: PathBuf) {
        let record = SlotRecord {
            pid,
            worktree: dir.join(format!("wt{slot}")),
            socket,
            started_at: now_unix_secs(),
            start_id: None,
        };
        std::fs::write(
            slot_path(dir, slot),
            format!("{}\n", serde_json::to_string(&record).unwrap()),
        )
        .unwrap();
    }

    fn health(idle: u64, active: u64) -> HealthResult {
        HealthResult {
            ok: true,
            workspace: "/wt".into(),
            protocol_version: PROTOCOL_VERSION,
            server: ServerHealth {
                running: true,
                pid: Some(7),
            },
            idle_secs: idle,
            lifecycle: LifecycleHealth {
                state: LifecycleState::Ready,
                active_requests: active,
            },
        }
    }

    /// A fake daemon at `socket` whose pid is a `sleep` child. It answers health
    /// with `idle` and `active`. A conditional (`if_idle`) shutdown is refused
    /// with `shutting_down: false` while `active > 0` or when `busy_at_shutdown`
    /// simulates work admitted after the listing; otherwise shutdown kills and
    /// reaps the child, so the pid stops being alive. Returns the pid, the
    /// operations received, and the listener task.
    fn fake_daemon(
        socket: &Path,
        idle: u64,
        active: u64,
        busy_at_shutdown: bool,
    ) -> (u32, Arc<Mutex<Vec<String>>>, tokio::task::JoinHandle<()>) {
        let mut child = tokio::process::Command::new("sleep")
            .arg("60")
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let pid = child.id().unwrap();
        let listener = UnixListener::bind(socket).unwrap();
        let ops = Arc::new(Mutex::new(Vec::new()));
        let seen = ops.clone();
        let task = tokio::spawn(async move {
            loop {
                let (mut stream, request) = request(&listener).await;
                seen.lock().unwrap().push(request.op.clone());
                if request.op == op::SHUTDOWN {
                    let if_idle = request.params["if_idle"].as_bool().unwrap_or(false);
                    if if_idle && (active > 0 || busy_at_shutdown) {
                        respond(
                            &mut stream,
                            Response::ok(request.id, json!({"shutting_down": false})),
                        )
                        .await;
                        continue;
                    }
                    child.kill().await.unwrap();
                    respond(
                        &mut stream,
                        Response::ok(request.id, json!({"shutting_down": true})),
                    )
                    .await;
                    break;
                }
                let result = serde_json::to_value(health(idle, active)).unwrap();
                respond(&mut stream, Response::ok(request.id, result)).await;
            }
        });
        (pid, ops, task)
    }

    #[tokio::test]
    async fn daemon_refusing_over_the_limit_is_reported_before_the_grace_expires() {
        let scratch = Scratch::new();
        let runtime = private_runtime(&scratch);
        let settings = limited(fake_binary(&scratch, "exit 75"), 1, OnLimit::Refuse);
        let result = tokio::time::timeout(
            Duration::from_secs(3),
            ensure_running(&scratch.0, runtime, &settings),
        )
        .await
        .expect("refusal is noticed without waiting for the startup grace");
        assert!(
            matches!(
                result,
                Err(ClientError::InstanceLimit {
                    running: 0,
                    limit: 1,
                    busy: false
                })
            ),
            "{result:?}"
        );
    }

    #[tokio::test]
    async fn daemon_failing_at_startup_is_reported_with_its_log() {
        for (body, expected) in [("exit 1", 1), ("kill -9 $$", 128 + libc::SIGKILL)] {
            let scratch = Scratch::new();
            let runtime = private_runtime(&scratch);
            let settings = limited(fake_binary(&scratch, body), 8, OnLimit::EvictIdle);
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                ensure_running(&scratch.0, runtime.clone(), &settings),
            )
            .await
            .expect("early exit is noticed without waiting for the startup grace");
            match result {
                Err(ClientError::DaemonExited { code, log }) => {
                    assert_eq!(code, expected);
                    assert_eq!(log, runtime.log());
                }
                other => panic!("{body}: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn full_slots_refuse_before_spawning_or_warn_and_spawn() {
        let scratch = Scratch::new();
        let runtime = private_runtime(&scratch);
        write_record(
            runtime.directory(),
            0,
            std::process::id(),
            scratch.0.join("other.sock"),
        );
        let binary = fake_binary(&scratch, MARKING);
        let refuse = limited(binary.clone(), 1, OnLimit::Refuse);
        let result = ensure_running(&scratch.0, runtime.clone(), &refuse).await;
        assert!(
            matches!(
                result,
                Err(ClientError::InstanceLimit {
                    running: 1,
                    limit: 1,
                    busy: false
                })
            ),
            "{result:?}"
        );
        assert!(!scratch.0.join("spawned").exists());

        let warn = limited(binary, 1, OnLimit::Warn);
        let binder = bind_when_finished(&scratch, &runtime);
        tokio::time::timeout(
            Duration::from_secs(3),
            ensure_running(&scratch.0, runtime.clone(), &warn),
        )
        .await
        .unwrap()
        .unwrap();
        let _listener = binder.await.unwrap();
        assert!(scratch.0.join("spawned").exists());
    }

    #[tokio::test]
    async fn full_slots_evict_the_longest_idle_daemon_then_spawn() {
        let scratch = Scratch::new();
        let runtime = private_runtime(&scratch);
        let dir = runtime.directory();
        let (recent_socket, stale_socket) =
            (scratch.0.join("recent.sock"), scratch.0.join("stale.sock"));
        // The recent daemon holds the lower slot, so only idle time decides.
        let (recent_pid, recent_ops, recent) = fake_daemon(&recent_socket, 10, 0, false);
        let (stale_pid, stale_ops, stale) = fake_daemon(&stale_socket, 300, 0, false);
        write_record(dir, 0, recent_pid, recent_socket);
        write_record(dir, 1, stale_pid, stale_socket);
        let settings = limited(fake_binary(&scratch, MARKING), 2, OnLimit::EvictIdle);
        let binder = bind_when_finished(&scratch, &runtime);
        tokio::time::timeout(
            Duration::from_secs(10),
            ensure_running(&scratch.0, runtime.clone(), &settings),
        )
        .await
        .unwrap()
        .unwrap();
        let _listener = binder.await.unwrap();
        stale.await.unwrap();
        assert_eq!(
            stale_ops.lock().unwrap().last().map(String::as_str),
            Some(op::SHUTDOWN)
        );
        assert!(!pid_alive(stale_pid));
        recent.abort();
        assert!(
            !recent_ops
                .lock()
                .unwrap()
                .iter()
                .any(|name| name == op::SHUTDOWN)
        );
        assert!(scratch.0.join("spawned").exists());
    }

    #[tokio::test]
    async fn a_candidate_busy_at_shutdown_stays_and_the_next_idle_daemon_is_evicted() {
        let scratch = Scratch::new();
        let runtime = private_runtime(&scratch);
        let dir = runtime.directory();
        let (raced_socket, next_socket) =
            (scratch.0.join("raced.sock"), scratch.0.join("next.sock"));
        // The longest-idle daemon reports idle but admits work before the stop.
        let (raced_pid, raced_ops, raced) = fake_daemon(&raced_socket, 300, 0, true);
        let (next_pid, next_ops, next) = fake_daemon(&next_socket, 10, 0, false);
        write_record(dir, 0, raced_pid, raced_socket);
        write_record(dir, 1, next_pid, next_socket);
        let settings = limited(fake_binary(&scratch, MARKING), 2, OnLimit::EvictIdle);
        let binder = bind_when_finished(&scratch, &runtime);
        tokio::time::timeout(
            Duration::from_secs(10),
            ensure_running(&scratch.0, runtime.clone(), &settings),
        )
        .await
        .unwrap()
        .unwrap();
        let _listener = binder.await.unwrap();
        next.await.unwrap();
        assert!(!pid_alive(next_pid));
        assert!(pid_alive(raced_pid), "a busy candidate is never stopped");
        raced.abort();
        // The raced daemon was asked once, conditionally, and refused; the
        // client did not re-probe it or fall back to an unconditional stop.
        let raced_ops = raced_ops.lock().unwrap();
        assert_eq!(
            raced_ops
                .iter()
                .filter(|name| *name == op::SHUTDOWN)
                .count(),
            1
        );
        assert_eq!(raced_ops.last().map(String::as_str), Some(op::SHUTDOWN));
        assert_eq!(
            next_ops.lock().unwrap().last().map(String::as_str),
            Some(op::SHUTDOWN)
        );
        assert!(scratch.0.join("spawned").exists());
    }

    #[tokio::test]
    async fn over_the_limit_enough_idle_daemons_are_evicted_for_one_spawn() {
        let scratch = Scratch::new();
        let runtime = private_runtime(&scratch);
        let dir = runtime.directory();
        // Three idle daemons against a lowered limit of two: two must go.
        let mut daemons = Vec::new();
        for (slot, idle) in [(0, 10), (1, 300), (2, 200)] {
            let socket = scratch.0.join(format!("idle{slot}.sock"));
            let (pid, ops, task) = fake_daemon(&socket, idle, 0, false);
            write_record(dir, slot, pid, socket);
            daemons.push((pid, ops, task));
        }
        let settings = limited(fake_binary(&scratch, MARKING), 2, OnLimit::EvictIdle);
        let binder = bind_when_finished(&scratch, &runtime);
        tokio::time::timeout(
            Duration::from_secs(10),
            ensure_running(&scratch.0, runtime.clone(), &settings),
        )
        .await
        .unwrap()
        .unwrap();
        let _listener = binder.await.unwrap();
        let (recent_pid, recent_ops, recent) = daemons.remove(0);
        for (pid, ops, task) in daemons {
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("evicted before the spawn")
                .unwrap();
            assert!(!pid_alive(pid));
            assert_eq!(
                ops.lock().unwrap().last().map(String::as_str),
                Some(op::SHUTDOWN)
            );
        }
        assert!(pid_alive(recent_pid));
        recent.abort();
        assert!(
            !recent_ops
                .lock()
                .unwrap()
                .iter()
                .any(|name| name == op::SHUTDOWN)
        );
        assert!(scratch.0.join("spawned").exists());
    }

    #[tokio::test]
    async fn concurrent_cold_starts_of_one_worktree_evict_once() {
        let scratch = Scratch::new();
        let runtime = private_runtime(&scratch);
        let dir = runtime.directory();
        let mut daemons = Vec::new();
        for (slot, idle) in [(0, 10), (1, 300)] {
            let socket = scratch.0.join(format!("idle{slot}.sock"));
            let (pid, ops, task) = fake_daemon(&socket, idle, 0, false);
            write_record(dir, slot, pid, socket);
            daemons.push((ops, task));
        }
        let settings = limited(fake_binary(&scratch, MARKING), 2, OnLimit::EvictIdle);
        let binder = bind_when_finished(&scratch, &runtime);
        let (first, second) = tokio::time::timeout(Duration::from_secs(10), async {
            tokio::join!(
                ensure_running(&scratch.0, runtime.clone(), &settings),
                ensure_running(&scratch.0, runtime.clone(), &settings),
            )
        })
        .await
        .unwrap();
        first.unwrap();
        second.unwrap();
        let _listener = binder.await.unwrap();
        let stopped = daemons
            .into_iter()
            .filter(|(ops, task)| {
                task.abort();
                ops.lock().unwrap().iter().any(|name| name == op::SHUTDOWN)
            })
            .count();
        assert_eq!(stopped, 1, "one spawn needs one free slot");
    }

    #[tokio::test]
    async fn full_slots_of_busy_daemons_are_never_evicted() {
        let scratch = Scratch::new();
        let runtime = private_runtime(&scratch);
        let dir = runtime.directory();
        let mut daemons = Vec::new();
        for (slot, idle) in [(0, 10), (1, 300)] {
            let socket = scratch.0.join(format!("busy{slot}.sock"));
            let (pid, ops, task) = fake_daemon(&socket, idle, 1, false);
            write_record(dir, slot, pid, socket);
            daemons.push((ops, task));
        }
        let settings = limited(fake_binary(&scratch, MARKING), 2, OnLimit::EvictIdle);
        let result = tokio::time::timeout(
            Duration::from_secs(5),
            ensure_running(&scratch.0, runtime, &settings),
        )
        .await
        .unwrap();
        assert!(
            matches!(
                result,
                Err(ClientError::InstanceLimit {
                    running: 2,
                    limit: 2,
                    busy: true
                })
            ),
            "{result:?}"
        );
        assert!(!scratch.0.join("spawned").exists());
        for (ops, task) in daemons {
            task.abort();
            let ops = ops.lock().unwrap();
            assert!(!ops.is_empty() && ops.iter().all(|name| name == op::HEALTH));
        }
    }
}
