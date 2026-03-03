//! Independent Rust processes exercise workspace coordination, not just a Tokio mutex.
use leanck_lake::{
    Lake,
    build::{BuildOptions, BuildOutcome},
    cache::{CacheOptions, CacheOutcome},
};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::process::{Child, Command};
struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "lck-coordinate-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        std::fs::write(
            root.join("lake-manifest.json"),
            r#"{"packages":[{"name":"mathlib"}]}"#,
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::write(root.join("fake-lake"),"#!/bin/sh\nprintf '%s' \"$$\" > \"$1.pid\"\nmkdir busy 2>/dev/null || { printf 'error: overlapping Lake jobs\\n' >&2; touch overlap; exit 7; }\ntouch \"$1.started\"\nwhile [ ! -f release ]; do sleep 0.01; done\nrmdir busy\n").unwrap();
        std::fs::set_permissions(
            root.join("fake-lake"),
            std::fs::Permissions::from_mode(0o700),
        )
        .unwrap();
        Self(root)
    }
    fn worker(&self, mode: &str) -> Child {
        Command::new(std::env::current_exe().unwrap())
            .args(["--exact", "process_worker", "--nocapture"])
            .env("LEANCK_LAKE_WORKER", mode)
            .env("LEANCK_LAKE_PROJECT", &self.0)
            .kill_on_drop(true)
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .unwrap()
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::write(self.0.join("release"), "");
        for file in ["build.pid", "exe.pid"] {
            if let Ok(pid) = std::fs::read_to_string(self.0.join(file))
                .and_then(|s| s.parse::<i32>().map_err(std::io::Error::other))
            {
                if pid > 1 {
                    unsafe {
                        libc::kill(-pid, libc::SIGKILL);
                    }
                }
            }
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
#[test]
fn process_worker() {
    let Ok(mode) = std::env::var("LEANCK_LAKE_WORKER") else {
        return;
    };
    let root = PathBuf::from(std::env::var_os("LEANCK_LAKE_PROJECT").unwrap());
    let lake = Lake::new(root.join("fake-lake"));
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            if mode == "build" {
                assert!(matches!(
                    leanck_lake::build::execute(
                        &lake,
                        &root,
                        BuildOptions::default(),
                        Some(Duration::from_secs(5))
                    )
                    .await
                    .unwrap(),
                    BuildOutcome::Completed { .. }
                ));
            } else {
                assert!(matches!(
                    leanck_lake::cache::fetch_with(
                        &lake,
                        &root,
                        CacheOptions {
                            execution: leanck_lake::ExecutionOptions {
                                timeout: Duration::from_secs(5),
                                ..Default::default()
                            },
                            ..Default::default()
                        }
                    )
                    .await
                    .unwrap(),
                    CacheOutcome::Completed(_)
                ));
            }
        });
}
async fn wait_file(path: &std::path::Path) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !path.exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
#[tokio::test]
async fn separate_build_and_cache_processes_never_overlap() {
    let project = Project::new();
    let first = project.worker("build");
    wait_file(&project.0.join("build.started")).await;
    let mut second = project.worker("cache");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        second.try_wait().unwrap().is_none(),
        "second utility must wait for the first rather than collide"
    );
    assert!(!project.0.join("overlap").exists());
    std::fs::write(project.0.join("release"), "").unwrap();
    for child in [first, second] {
        let output = tokio::time::timeout(Duration::from_secs(3), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
    assert!(project.0.join("exe.started").exists());
}

#[tokio::test]
async fn separate_build_processes_serialize_and_distinct_workspaces_remain_independent() {
    let a = Project::new();
    let b = Project::new();
    let first = a.worker("build");
    wait_file(&a.0.join("build.started")).await;
    let second = a.worker("build");
    let other = b.worker("build");
    wait_file(&b.0.join("build.started")).await;
    assert!(!a.0.join("overlap").exists());
    std::fs::write(a.0.join("release"), "").unwrap();
    std::fs::write(b.0.join("release"), "").unwrap();
    for child in [first, second, other] {
        let output = tokio::time::timeout(Duration::from_secs(3), child.wait_with_output())
            .await
            .unwrap()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}
#[tokio::test]
async fn queued_timeout_and_cancellation_never_spawn_and_aliases_use_one_lock() {
    let project = Project::new();
    let first = project.worker("build");
    wait_file(&project.0.join("build.started")).await;
    let alias = project.0.join("alias");
    std::os::unix::fs::symlink(&project.0, &alias).unwrap();
    let lake = Lake::new(project.0.join("fake-lake"));
    let timeout = leanck_lake::cache::fetch_with(
        &lake,
        &alias,
        CacheOptions {
            execution: leanck_lake::ExecutionOptions {
                timeout: Duration::from_millis(50),
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(matches!(timeout, CacheOutcome::TimedOut));
    assert!(!project.0.join("exe.pid").exists());
    let cancel = leanck_lake::CancellationToken::new();
    let token = cancel.clone();
    let root = alias.clone();
    let work = tokio::spawn(async move {
        leanck_lake::cache::fetch_with(
            &lake,
            &root,
            CacheOptions {
                execution: leanck_lake::ExecutionOptions {
                    cancellation: Some(token),
                    ..Default::default()
                },
                ..Default::default()
            },
        )
        .await
    });
    tokio::time::sleep(Duration::from_millis(30)).await;
    cancel.cancel();
    assert!(matches!(
        work.await.unwrap(),
        Err(leanck_lake::LakeError::Cancelled)
    ));
    assert!(!project.0.join("exe.pid").exists());
    std::fs::write(project.0.join("release"), "").unwrap();
    assert!(first.wait_with_output().await.unwrap().status.success());
}
#[tokio::test]
async fn a_surviving_lake_child_keeps_the_lock_after_its_caller_crashes() {
    let project = Project::new();
    let mut first = project.worker("build");
    wait_file(&project.0.join("build.started")).await;
    first.kill().await.unwrap();
    let mut second = project.worker("cache");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(second.try_wait().unwrap().is_none());
    assert!(!project.0.join("exe.pid").exists());
    std::fs::write(project.0.join("release"), "").unwrap();
    let result = tokio::time::timeout(Duration::from_secs(3), second.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stdout)
    );
}
#[tokio::test]
async fn dropping_a_build_retains_coordination_until_its_child_has_stopped() {
    let project = Project::new();
    let root = project.0.clone();
    let lake = Lake::new(project.0.join("fake-lake"));
    let work = tokio::spawn(async move {
        leanck_lake::build::execute(&lake, &root, BuildOptions::default(), None).await
    });
    wait_file(&project.0.join("build.started")).await;
    let pid = std::fs::read_to_string(project.0.join("build.pid")).unwrap();
    use std::os::unix::fs::MetadataExt;
    let lock_path = project.0.join(".lake/leanck-execution.lock");
    let inode = std::fs::metadata(&lock_path).unwrap().ino();
    work.abort();
    let _ = work.await;
    // The next job checks the old process before mutating anything.
    use std::os::unix::fs::PermissionsExt;
    let followup = project.0.join("next-lake");
    std::fs::write(
        &followup,
        format!("#!/bin/sh\nkill -0 {pid} 2>/dev/null && exit 9\nexit 0\n"),
    )
    .unwrap();
    std::fs::set_permissions(&followup, std::fs::Permissions::from_mode(0o700)).unwrap();
    let result = leanck_lake::build::execute(
        &Lake::new(followup),
        &project.0,
        BuildOptions::default(),
        Some(Duration::from_secs(2)),
    )
    .await
    .unwrap();
    assert!(matches!(result, BuildOutcome::Completed { .. }));
    assert_eq!(std::fs::metadata(&lock_path).unwrap().ino(), inode);
}
#[tokio::test]
async fn coordination_failure_is_fatal_instead_of_running_unlocked() {
    let project = Project::new();
    std::fs::write(project.0.join(".lake"), "not a directory").unwrap();
    let result = leanck_lake::build::execute(
        &Lake::new(project.0.join("fake-lake")),
        &project.0,
        BuildOptions::default(),
        None,
    )
    .await;
    assert!(matches!(
        result,
        Err(leanck_lake::LakeError::Coordination { .. })
    ));
    assert!(!project.0.join("build.pid").exists());
}
