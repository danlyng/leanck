//! Real daemon processes with a local fake Lake; these tests never require Lean.
use leanck_client::{CancellationToken, Client, ClientError, RequestOptions};
use leanck_core::{
    api::{GoalParams, HealthResult, LifecycleState},
    error::ErrorKind,
    paths::RuntimePaths,
};
use std::{
    path::PathBuf,
    process::Stdio,
    sync::atomic::{AtomicU32, Ordering},
    time::Duration,
};
use tokio::process::{Child, Command};

struct Fixture {
    root: PathBuf,
    lean_pids: Vec<u32>,
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
        }
    }
    fn silent() -> Self {
        let reply = r#"{"jsonrpc":"2.0","id":0,"result":{"capabilities":{}}}"#;
        Self::new(&format!(
            "#!/bin/sh\nprintf '%s\\r\\n\\r\\n%s' 'Content-Length: {}' '{}'\ncat >/dev/null\n",
            reply.len(),
            reply
        ))
    }
    fn runtime(&self) -> RuntimePaths {
        RuntimePaths::for_workspace(&self.root)
    }
    fn client(&self) -> Client {
        Client::from_path(&self.root).unwrap()
    }
    fn launch(&self) -> Child {
        let path = format!(
            "{}:{}",
            self.root.join("bin").display(),
            std::env::var("PATH").unwrap()
        );
        Command::new(env!("CARGO_BIN_EXE_leanckd"))
            .arg(&self.root)
            .env("PATH", path)
            .args(["--set", "cache.enabled=false"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .unwrap()
    }
    async fn ready(&mut self) -> HealthResult {
        let health = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Ok(Some(health)) = self
                    .client()
                    .probe_existing(RequestOptions {
                        transport_timeout_ms: Some(100),
                        ..Default::default()
                    })
                    .await
                {
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
