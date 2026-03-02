//! Auto-start policy, including warming liveness and the existing binary search order.
use crate::{
    discovery::resolve_binary,
    error::{ClientError, Result},
    transport::can_connect,
};
use leanck_core::paths::RuntimePaths;
use std::path::Path;
use std::time::{Duration, Instant};

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

fn spawn_daemon(
    bin: &Path,
    worktree: &Path,
    runtime: &RuntimePaths,
    settings: &leanck_config::Settings,
) -> Result<()> {
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
    })?;
    Ok(())
}

/// Prepare and start only when the selected socket is absent. Duplicate launches
/// are serialized by the daemon's lifetime lock, exactly as before extraction.
pub(crate) async fn ensure_running(
    worktree: &Path,
    runtime: RuntimePaths,
    settings: &leanck_config::Settings,
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
    spawn_daemon(&binary, worktree, &runtime, settings)?;
    wait_for_socket(
        &runtime,
        StartupPolicy {
            grace: Duration::from_millis(settings.client.startup_grace_ms),
            hard_cap: Duration::from_millis(settings.client.startup_timeout_ms),
            ..StartupPolicy::default()
        },
    )
    .await?;
    Ok(runtime)
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

async fn wait_for_socket(runtime: &RuntimePaths, policy: StartupPolicy) -> Result<()> {
    let hard_deadline = Instant::now() + policy.hard_cap;
    let mut grace_deadline = Instant::now() + policy.grace;
    loop {
        if can_connect(&runtime.socket()).await {
            return Ok(());
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
    use crate::tests::Scratch;
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
}
