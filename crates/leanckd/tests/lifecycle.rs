//! Real daemon processes with a local fake Lake; these tests never require Lean.
use leanck_client::{
    CancellationToken, Client, ClientError, RequestOptions,
    instances::{self, StopMode, Stopped},
};
use leanck_core::{
    api::{GoalParams, HealthResult, LifecycleState},
    error::ErrorKind,
    instances::{EXIT_INSTANCE_LIMIT, live, now_unix_secs, scan},
    paths::RuntimePaths,
};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};
use tokio::process::{Child, Command};

/// A private `XDG_CACHE_HOME` shared by one test's isolated fixtures, so slot
/// tests never see or touch the developer's real daemons. Removed on drop.
struct CacheDir(PathBuf);
impl CacheDir {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = PathBuf::from("/tmp").join(format!(
            "lcf-cache-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    /// The runtime and slot directory daemons derive from this cache home.
    fn slots(&self) -> PathBuf {
        self.0.join("leanck")
    }
}
impl Drop for CacheDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn silent_script() -> String {
    let reply = r#"{"jsonrpc":"2.0","id":0,"result":{"capabilities":{}}}"#;
    format!(
        "#!/bin/sh\nprintf '%s\\r\\n\\r\\n%s' 'Content-Length: {}' '{}'\ncat >/dev/null\n",
        reply.len(),
        reply
    )
}

struct Fixture {
    root: PathBuf,
    lean_pids: Vec<u32>,
    /// `XDG_CACHE_HOME` for an isolated fixture; `None` uses the real cache directory.
    cache: Option<PathBuf>,
}
impl Fixture {
    fn new(script: &str) -> Self {
        use std::os::unix::fs::PermissionsExt;
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = PathBuf::from("/tmp").join(format!(
            "lcf-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(root.join("bin")).unwrap();
        std::fs::write(root.join("lakefile.toml"), "name = \"fake\"\n").unwrap();
        std::fs::write(
            root.join("Basic.lean"),
            "theorem example : True := by trivial\n",
        )
        .unwrap();
        std::fs::write(
            root.join("bin/lake"),
            format!(
                "#!/bin/sh\nif [ \"$1\" = env ]; then printf '%s\\n' \"$PWD\"; exit 0; fi\n{}",
                script.strip_prefix("#!/bin/sh\n").unwrap_or(script)
            ),
        )
        .unwrap();
        std::fs::set_permissions(
            root.join("bin/lake"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        Self {
            root,
            lean_pids: Vec::new(),
            cache: None,
        }
    }
    fn silent() -> Self {
        Self::new(&silent_script())
    }
    /// A fixture whose daemons use `cache` as `XDG_CACHE_HOME`.
    fn isolated(script: &str, cache: &CacheDir) -> Self {
        let mut fixture = Self::new(script);
        fixture.cache = Some(cache.0.clone());
        fixture
    }
    fn runtime(&self) -> RuntimePaths {
        match &self.cache {
            Some(cache) => RuntimePaths::in_directory(cache.join("leanck"), &self.root),
            None => RuntimePaths::for_workspace(&self.root),
        }
    }
    /// The in-process client reads the real environment, so it cannot reach an
    /// isolated daemon; isolated tests use socket-direct `instances` calls.
    fn client(&self) -> Client {
        assert!(self.cache.is_none(), "isolated fixtures have no Client");
        Client::from_path(&self.root).unwrap()
    }
    fn launch(&self) -> Child {
        self.launch_with(&[])
    }
    /// Launch with `cache.enabled=false` and `instances.limit=0` first, so the
    /// real cache directory's slots never refuse a test daemon; later `--set`
    /// arguments in `extra` take precedence.
    fn launch_with(&self, extra: &[&str]) -> Child {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        let mut command = Command::new(env!("CARGO_BIN_EXE_leanckd"));
        command
            .arg(&self.root)
            .env("PATH", path)
            .args(["--set", "cache.enabled=false", "--set", "instances.limit=0"])
            .args(extra);
        if let Some(cache) = &self.cache {
            command.env("XDG_CACHE_HOME", cache);
        }
        command
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }
    async fn probe(&self) -> Option<HealthResult> {
        if self.cache.is_some() {
            instances::probe(&self.runtime().socket(), Duration::from_millis(100))
                .await
                .ok()
        } else {
            self.client()
                .probe_existing(RequestOptions {
                    transport_timeout_ms: Some(100),
                    ..Default::default()
                })
                .await
                .ok()
                .flatten()
        }
    }
    async fn ready(&mut self) -> HealthResult {
        let health = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(health) = self.probe().await {
                    break health;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("fake daemon ready");
        self.lean_pids.extend(health.server.pid);
        assert_eq!(health.lifecycle.state, LifecycleState::Ready);
        health
    }
    async fn active(&self, count: u64) {
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                let health = self
                    .client()
                    .probe_existing(RequestOptions::default())
                    .await
                    .unwrap()
                    .unwrap();
                if health.lifecycle.active_requests == count {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("active request count");
    }
    /// Socket-direct shutdown for isolated fixtures, then the usual cleanup checks.
    async fn stop(&self, daemon: &mut Child) {
        assert_eq!(
            instances::stop_socket(
                &self.runtime().socket(),
                // An unreaped child stays visible to `kill(pid, 0)`; wait for the socket instead.
                None,
                &leanck_config::Settings::default(),
                Duration::from_secs(7),
                StopMode::Always,
            )
            .await
            .unwrap(),
            Stopped::Exited
        );
        self.stopped(daemon).await;
    }
    async fn stopped(&self, daemon: &mut Child) {
        assert!(
            tokio::time::timeout(Duration::from_secs(7), daemon.wait())
                .await
                .unwrap()
                .unwrap()
                .success()
        );
        assert!(!self.runtime().socket().exists());
        assert!(!self.runtime().pid().exists());
        assert!(
            std::fs::File::open(self.runtime().lock())
                .unwrap()
                .try_lock()
                .is_ok()
        );
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        for pid in &self.lean_pids {
            unsafe {
                libc::kill(-(*pid as i32), libc::SIGKILL);
            }
        }
        for path in [
            self.runtime().socket(),
            self.runtime().pid(),
            self.runtime().log(),
        ] {
            let _ = std::fs::remove_file(path);
        }
        // Only remove a private fixture's lock after ownership is released.
        if let Ok(lock) = std::fs::File::open(self.runtime().lock())
            && lock.try_lock().is_ok()
        {
            let _ = std::fs::remove_file(self.runtime().lock());
        }
        let _ = std::fs::remove_dir_all(&self.root);
    }
}
fn goal() -> GoalParams {
    GoalParams {
        file: "Basic.lean".into(),
        line: 0,
        character: 0,
    }
}

#[tokio::test]
async fn queued_deadline_cancellation_disconnect_and_shutdown_release_admission() {
    let mut fixture = Fixture::silent();
    let mut daemon = fixture.launch();
    fixture.ready().await;
    let mut work = Vec::new();
    let mut tokens = Vec::new();
    for _ in 0..8 {
        let client = fixture.client();
        let token = CancellationToken::new();
        tokens.push(token.clone());
        work.push(tokio::spawn(async move {
            client
                .goal(
                    &goal(),
                    RequestOptions {
                        cancellation: Some(token),
                        ..Default::default()
                    },
                )
                .await
        }));
    }
    fixture.active(8).await;
    let client = fixture.client();
    let queued_token = CancellationToken::new();
    let request_token = queued_token.clone();
    let queued_cancel = tokio::spawn(async move {
        client
            .goal(
                &goal(),
                RequestOptions {
                    cancellation: Some(request_token),
                    ..Default::default()
                },
            )
            .await
    });
    fixture.active(9).await;
    queued_token.cancel();
    assert!(matches!(
        queued_cancel.await.unwrap(),
        Err(ClientError::Cancelled { .. })
    ));
    fixture.active(8).await;
    let queued = tokio::time::timeout(
        Duration::from_secs(1),
        fixture.client().goal(
            &goal(),
            RequestOptions {
                timeout_ms: Some(20),
                ..Default::default()
            },
        ),
    )
    .await
    .unwrap();
    assert!(
        matches!(
            queued,
            Err(ClientError::Daemon {
                kind: ErrorKind::Timeout,
                ..
            })
        ),
        "{queued:?}"
    );
    fixture.active(8).await;
    for token in tokens {
        token.cancel();
    }
    for request in work {
        assert!(matches!(
            request.await.unwrap(),
            Err(ClientError::Cancelled { .. })
        ));
    }
    fixture.active(0).await;
    // Dropped client futures must release their daemon operation, too.
    let client = fixture.client();
    let dropped =
        tokio::spawn(async move { client.goal(&goal(), RequestOptions::default()).await });
    fixture.active(1).await;
    dropped.abort();
    let _ = dropped.await;
    fixture.active(0).await;
    // Raw connection loss has no client control message to assist cleanup.
    let mut raw = tokio::net::UnixStream::connect(fixture.runtime().socket())
        .await
        .unwrap();
    let request = leanck_core::api::Command::Goal(goal())
        .into_request("raw-disconnect".into(), None)
        .unwrap();
    leanck_core::wire::write_frame(
        &mut leanck_core::wire::writer(&mut raw),
        &serde_json::to_vec(&request).unwrap(),
    )
    .await
    .unwrap();
    fixture.active(1).await;
    drop(raw);
    fixture.active(0).await;
    // Shutdown cancels a live LSP request and cleans all runtime ownership.
    let client = fixture.client();
    let active = tokio::spawn(async move { client.goal(&goal(), RequestOptions::default()).await });
    fixture.active(1).await;
    fixture
        .client()
        .shutdown_existing(RequestOptions::default())
        .await
        .unwrap();
    assert!(matches!(
        active.await.unwrap(),
        Err(ClientError::Daemon {
            kind: ErrorKind::Cancelled,
            ..
        })
    ));
    fixture.stopped(&mut daemon).await;
}

#[tokio::test]
async fn control_operations_leave_the_idle_clock_and_conditional_shutdown_spares_work() {
    let mut fixture = Fixture::silent();
    let mut daemon = fixture.launch();
    fixture.ready().await;
    let socket = fixture.runtime().socket();
    let settings = leanck_config::Settings::default();
    // Health probes are control operations and never reset the idle clock.
    let first = fixture.probe().await.unwrap();
    tokio::time::sleep(Duration::from_millis(2100)).await;
    let second = fixture.probe().await.unwrap();
    assert!(
        second.idle_secs >= first.idle_secs + 2,
        "{} then {}",
        first.idle_secs,
        second.idle_secs
    );
    // A running query is not idle, and a conditional shutdown leaves it alone.
    let client = fixture.client();
    let token = CancellationToken::new();
    let request_token = token.clone();
    let query = tokio::spawn(async move {
        client
            .goal(
                &goal(),
                RequestOptions {
                    cancellation: Some(request_token),
                    ..Default::default()
                },
            )
            .await
    });
    fixture.active(1).await;
    assert_eq!(fixture.probe().await.unwrap().idle_secs, 0);
    assert_eq!(
        instances::stop_socket(
            &socket,
            None,
            &settings,
            Duration::from_secs(1),
            StopMode::IfIdle
        )
        .await
        .unwrap(),
        Stopped::Busy
    );
    let health = fixture.probe().await.unwrap();
    assert_eq!(health.lifecycle.state, LifecycleState::Ready);
    assert_eq!(health.lifecycle.active_requests, 1);
    token.cancel();
    assert!(matches!(
        query.await.unwrap(),
        Err(ClientError::Cancelled { .. })
    ));
    fixture.active(0).await;
    // Finishing the query restarted the idle clock.
    assert!(fixture.probe().await.unwrap().idle_secs < 2);
    // Idle again, the same conditional shutdown is accepted.
    assert_eq!(
        instances::stop_socket(
            &socket,
            None,
            &settings,
            Duration::from_secs(7),
            StopMode::IfIdle
        )
        .await
        .unwrap(),
        Stopped::Exited
    );
    fixture.stopped(&mut daemon).await;
}

#[tokio::test]
async fn duplicate_launch_and_crash_restart_use_the_same_stable_lock() {
    use std::os::unix::fs::MetadataExt;
    let mut fixture = Fixture::silent();
    let mut first = fixture.launch();
    fixture.ready().await;
    let inode = std::fs::metadata(fixture.runtime().lock()).unwrap().ino();
    let pid = std::fs::read_to_string(fixture.runtime().pid()).unwrap();
    let mut duplicate = fixture.launch();
    assert!(
        tokio::time::timeout(Duration::from_secs(2), duplicate.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(fixture.runtime().pid()).unwrap(),
        pid
    );
    assert!(
        std::fs::File::open(fixture.runtime().lock())
            .unwrap()
            .try_lock()
            .is_err()
    );
    first.kill().await.unwrap();
    assert!(
        std::fs::File::open(fixture.runtime().lock())
            .unwrap()
            .try_lock()
            .is_ok()
    );
    // SIGKILL cannot run destructors; clean the uniquely owned fake Lean group.
    for pid in fixture.lean_pids.drain(..) {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
    let mut replacement = fixture.launch();
    fixture.ready().await;
    assert_eq!(
        std::fs::metadata(fixture.runtime().lock()).unwrap().ino(),
        inode
    );
    fixture
        .client()
        .shutdown_existing(RequestOptions::default())
        .await
        .unwrap();
    fixture.stopped(&mut replacement).await;
}

#[tokio::test]
async fn startup_failure_cleans_pid_and_socket_and_lock_errors_are_fatal() {
    let fixture = Fixture::new("#!/bin/sh\nexit 1\n");
    let mut daemon = fixture.launch();
    assert!(
        !tokio::time::timeout(Duration::from_secs(3), daemon.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(!fixture.runtime().pid().exists());
    assert!(!fixture.runtime().socket().exists());
    assert!(
        std::fs::File::open(fixture.runtime().lock())
            .unwrap()
            .try_lock()
            .is_ok()
    );
    std::fs::remove_file(fixture.runtime().lock()).unwrap();
    std::fs::create_dir(fixture.runtime().lock()).unwrap();
    std::fs::write(fixture.runtime().pid(), "owned sentinel").unwrap();
    let mut daemon = fixture.launch();
    assert!(
        !tokio::time::timeout(Duration::from_secs(3), daemon.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert_eq!(
        std::fs::read_to_string(fixture.runtime().pid()).unwrap(),
        "owned sentinel"
    );
    std::fs::remove_dir(fixture.runtime().lock()).unwrap();
}

#[tokio::test]
async fn shutdown_during_recovery_cancels_initialize_and_cannot_publish_a_new_server() {
    let mut fixture = Fixture::silent();
    let mut daemon = fixture.launch();
    let health = fixture.ready().await;
    // The replacement starts but never responds to initialize.
    std::fs::write(
        fixture.root.join("bin/lake"),
        "#!/bin/sh\nprintf '%s' \"$$\" > recovery-pid\ncat >/dev/null\n",
    )
    .unwrap();
    unsafe {
        libc::kill(-(health.server.pid.unwrap() as i32), libc::SIGKILL);
    }
    tokio::time::timeout(Duration::from_secs(4), async {
        while !fixture.root.join("recovery-pid").exists() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let recovery_pid = std::fs::read_to_string(fixture.root.join("recovery-pid"))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    fixture.lean_pids.push(recovery_pid);
    let health = fixture
        .client()
        .health(RequestOptions::default())
        .await
        .unwrap();
    assert_eq!(health.lifecycle.state, LifecycleState::Recovering);
    fixture
        .client()
        .shutdown_existing(RequestOptions::default())
        .await
        .unwrap();
    fixture.stopped(&mut daemon).await;
    assert_eq!(
        unsafe { libc::kill(recovery_pid as i32, 0) },
        -1,
        "recovery child reaped"
    );
}

#[tokio::test]
async fn termination_during_startup_reaps_lean_and_removes_published_liveness() {
    let mut fixture =
        Fixture::new("#!/bin/sh\nprintf '%s' \"$$\" > initializing-pid\ncat >/dev/null\n");
    let mut daemon = fixture.launch();
    tokio::time::timeout(Duration::from_secs(3), async {
        while !fixture.root.join("initializing-pid").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let lean_pid = std::fs::read_to_string(fixture.root.join("initializing-pid"))
        .unwrap()
        .parse::<u32>()
        .unwrap();
    fixture.lean_pids.push(lean_pid);
    assert!(fixture.runtime().pid().exists());
    assert!(!fixture.runtime().socket().exists());
    unsafe {
        libc::kill(daemon.id().unwrap() as i32, libc::SIGTERM);
    }
    fixture.stopped(&mut daemon).await;
    assert_eq!(unsafe { libc::kill(lean_pid as i32, 0) }, -1);
}

#[tokio::test]
async fn pipelined_frames_survive_read_ahead_while_a_query_is_waiting() {
    use leanck_core::{api::Command, wire};
    use tokio::io::AsyncWriteExt;
    let mut fixture = Fixture::silent();
    let mut daemon = fixture.launch();
    fixture.ready().await;
    let requests = [
        Command::Goal(goal())
            .into_request("pipeline-query".into(), Some(20))
            .unwrap(),
        Command::Health
            .into_request("pipeline-health".into(), None)
            .unwrap(),
    ];
    let mut encoded = Vec::new();
    {
        let mut writer = wire::writer(&mut encoded);
        for request in requests {
            wire::write_frame(&mut writer, &serde_json::to_vec(&request).unwrap())
                .await
                .unwrap();
        }
    }
    let mut stream = tokio::net::UnixStream::connect(fixture.runtime().socket())
        .await
        .unwrap();
    stream.write_all(&encoded).await.unwrap();
    let mut framed = wire::framed(stream);
    let query: leanck_core::api::Response =
        serde_json::from_slice(&wire::read_frame(&mut framed).await.unwrap().unwrap()).unwrap();
    let health: leanck_core::api::Response =
        serde_json::from_slice(&wire::read_frame(&mut framed).await.unwrap().unwrap()).unwrap();
    assert_eq!(query.id, "pipeline-query");
    assert_eq!(query.error.unwrap().kind, ErrorKind::Timeout);
    assert_eq!(health.id, "pipeline-health");
    assert_eq!(health.result.unwrap()["lifecycle"]["active_requests"], 0);
    drop(framed);
    fixture
        .client()
        .shutdown_existing(RequestOptions::default())
        .await
        .unwrap();
    fixture.stopped(&mut daemon).await;
}

#[tokio::test]
async fn configured_operation_deadline_and_idle_shutdown_are_enforced() {
    let mut fixture = Fixture::silent();
    std::fs::write(
        fixture.root.join("leanck.toml"),
        "[daemon]\nrequest_timeout_ms=35\nidle_timeout_ms=200\nmax_inflight=1\n",
    )
    .unwrap();
    let mut child = fixture.launch();
    fixture.ready().await;
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        fixture.client().goal(&goal(), RequestOptions::default()),
    )
    .await
    .unwrap();
    assert!(
        matches!(
            response,
            Err(ClientError::Daemon {
                kind: ErrorKind::Timeout,
                ..
            })
        ),
        "{response:?}"
    );
    assert!(
        tokio::time::timeout(Duration::from_secs(3), child.wait())
            .await
            .unwrap()
            .unwrap()
            .success()
    );
    assert!(!fixture.runtime().socket().exists());
    assert!(!fixture.runtime().pid().exists());
    assert!(fixture.runtime().lock().exists());
}

const REFUSE: [&str; 4] = [
    "--set",
    "instances.limit=1",
    "--set",
    "instances.on_limit=\"refuse\"",
];

fn lock_is_free(path: &Path) -> bool {
    std::fs::File::open(path).unwrap().try_lock().is_ok()
}

#[tokio::test]
async fn second_daemon_over_the_limit_is_refused_before_publishing_liveness() {
    let cache = CacheDir::new();
    let mut first = Fixture::isolated(&silent_script(), &cache);
    let second = Fixture::isolated(&silent_script(), &cache);
    let mut running = first.launch_with(&REFUSE);
    first.ready().await;
    let mut refused = second.launch_with(&REFUSE);
    let status = tokio::time::timeout(Duration::from_secs(5), refused.wait())
        .await
        .expect("refused daemon exits promptly")
        .unwrap();
    assert_eq!(status.code(), Some(EXIT_INSTANCE_LIMIT));
    assert!(!second.runtime().pid().exists());
    assert!(!second.runtime().socket().exists());
    assert!(lock_is_free(&second.runtime().lock()));
    let slots = scan(&cache.slots());
    assert_eq!(slots.len(), 1, "{slots:?}");
    assert_eq!(slots[0].record.pid, running.id().unwrap());
    assert_eq!(live(&cache.slots()).len(), 1);
    first.stop(&mut running).await;
}

#[tokio::test]
async fn warn_mode_starts_past_the_limit() {
    let cache = CacheDir::new();
    let mut first = Fixture::isolated(&silent_script(), &cache);
    let mut second = Fixture::isolated(&silent_script(), &cache);
    let warn = [
        "--set",
        "instances.limit=1",
        "--set",
        "instances.on_limit=\"warn\"",
    ];
    let mut a = first.launch_with(&warn);
    first.ready().await;
    let mut b = second.launch_with(&warn);
    second.ready().await;
    let slots: Vec<_> = live(&cache.slots()).iter().map(|e| e.slot).collect();
    assert_eq!(slots, vec![0, 1]);
    first.stop(&mut a).await;
    second.stop(&mut b).await;
    assert!(live(&cache.slots()).is_empty());
}

#[tokio::test]
async fn admission_counts_running_daemons_not_the_free_slot_index() {
    let cache = CacheDir::new();
    let mut first = Fixture::isolated(&silent_script(), &cache);
    let mut second = Fixture::isolated(&silent_script(), &cache);
    let mut third = Fixture::isolated(&silent_script(), &cache);
    let mut a = first.launch();
    first.ready().await;
    let mut b = second.launch();
    second.ready().await;
    let b_pid = b.id().unwrap();
    first.stop(&mut a).await;
    let slots: Vec<_> = live(&cache.slots())
        .iter()
        .map(|e| (e.slot, e.record.pid))
        .collect();
    assert_eq!(slots, vec![(1, b_pid)]);

    // Slot 0 is free, but one daemon already runs against a limit of one.
    let mut refused = third.launch_with(&REFUSE);
    let status = tokio::time::timeout(Duration::from_secs(5), refused.wait())
        .await
        .expect("refused daemon exits promptly")
        .unwrap();
    assert_eq!(status.code(), Some(EXIT_INSTANCE_LIMIT));
    assert!(!third.runtime().socket().exists());
    assert!(lock_is_free(&leanck_core::instances::slot_path(
        &cache.slots(),
        0
    )));
    assert_eq!(live(&cache.slots()).len(), 1);

    let mut c = third.launch_with(&[
        "--set",
        "instances.limit=2",
        "--set",
        "instances.on_limit=\"refuse\"",
    ]);
    third.ready().await;
    let slots: Vec<_> = live(&cache.slots())
        .iter()
        .map(|e| (e.slot, e.record.pid))
        .collect();
    assert_eq!(slots, vec![(0, c.id().unwrap()), (1, b_pid)]);
    third.stop(&mut c).await;
    second.stop(&mut b).await;
    assert!(live(&cache.slots()).is_empty());
}

#[tokio::test]
async fn slot_record_describes_the_running_daemon() {
    let cache = CacheDir::new();
    let mut fixture = Fixture::isolated(&silent_script(), &cache);
    let mut daemon = fixture.launch();
    fixture.ready().await;
    let slots = live(&cache.slots());
    assert_eq!(slots.len(), 1, "{slots:?}");
    let record = &slots[0].record;
    assert_eq!(record.pid, daemon.id().unwrap());
    assert_eq!(record.worktree, fixture.root.canonicalize().unwrap());
    assert_eq!(record.socket, fixture.runtime().socket());
    assert!(now_unix_secs().abs_diff(record.started_at) <= 10);
    assert_eq!(
        record.start_id,
        leanck_core::instances::process_start_id(record.pid)
    );
    assert!(!lock_is_free(&leanck_core::instances::slot_path(
        &cache.slots(),
        slots[0].slot
    )));
    fixture.stop(&mut daemon).await;
    assert!(live(&cache.slots()).is_empty());
}

#[tokio::test]
async fn evict_idle_stops_the_idle_daemon() {
    let cache = CacheDir::new();
    let mut first = Fixture::isolated(&silent_script(), &cache);
    let mut second = Fixture::isolated(&silent_script(), &cache);
    let mut a = first.launch();
    first.ready().await;
    let mut b = second.launch();
    second.ready().await;
    let (a_pid, b_pid) = (a.id().unwrap(), b.id().unwrap());
    let (slots, settings) = (cache.slots(), leanck_config::Settings::default());
    // The test is both daemons' parent, so reap concurrently: an unreaped
    // daemon stays visible to `kill(pid, 0)` and eviction would wait it out.
    let (evicted, exited) = tokio::time::timeout(Duration::from_secs(20), async {
        tokio::join!(instances::evict_idle_in(&slots, &settings, 1), async {
            tokio::select! {
                status = a.wait() => (a_pid, status.unwrap()),
                status = b.wait() => (b_pid, status.unwrap()),
            }
        })
    })
    .await
    .expect("eviction bounded");
    let evicted = evicted.unwrap();
    assert_eq!(evicted.len(), 1, "{evicted:?}");
    assert!(evicted[0].exited);
    let evicted = &evicted[0].daemon;
    // The first daemon took slot 0 and has been idle at least as long (health
    // probes do not reset the idle clock), so either rule picks it.
    assert_eq!((evicted.slot, evicted.pid), (0, a_pid));
    assert_eq!(evicted.worktree, first.root.canonicalize().unwrap());
    assert_eq!(exited.0, a_pid);
    assert!(exited.1.success());
    first.stopped(&mut a).await;
    let remaining = live(&cache.slots());
    assert_eq!(remaining.len(), 1, "{remaining:?}");
    assert_eq!(remaining[0].record.pid, b_pid);
    second.stop(&mut b).await;
    assert!(live(&cache.slots()).is_empty());
}
