use leanck_lake::{
    ExecutionOptionsError, Lake, LakeError, OutputStream,
    build::{BuildOptions, BuildOutcome},
    cache::{CacheOptions, CacheOutcome, SkipReason},
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

struct Project(PathBuf);
impl Project {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!(
            "lck-lake-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    fn lake(&self, script: &str) -> Lake {
        use std::os::unix::fs::PermissionsExt;
        let executable = self.0.join("fake-lake");
        std::fs::write(&executable, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
        Lake::new(executable)
    }
    fn manifest(&self, text: &str) {
        std::fs::write(self.0.join("lake-manifest.json"), text).unwrap();
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[tokio::test]
async fn builds_preserve_toolchain_arguments_streams_and_warning_policy() {
    let project = Project::new();
    let lake = project.lake("printf '%s\\n' \"$@\" > args\npwd > cwd\nprintf 'warning: .lake/packages/Dep/Foo.lean:1:0: dependency warning\\n'\nprintf 'error: Local.lean:2:0: failure\\n  detail\\n' >&2\nexit 3");
    let outcome = leanck_lake::build::execute(
        &lake,
        &project.0,
        BuildOptions {
            warnings: true,
            targets: vec!["MyPkg.Foo".into()],
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    let BuildOutcome::Failed { output, report } = outcome else {
        panic!("failed process must be represented");
    };
    assert_eq!(output.exit_code, Some(3));
    assert!(!output.success);
    assert_eq!(report.errors, ["error: Local.lean:2:0: failure\n  detail"]);
    assert_eq!(
        report.warnings,
        ["warning: .lake/packages/Dep/Foo.lean:1:0: dependency warning"]
    );
    assert_eq!(
        std::fs::read_to_string(project.0.join("args")).unwrap(),
        "build\nMyPkg.Foo\n"
    );
    assert_eq!(
        std::fs::read_to_string(project.0.join("cwd"))
            .unwrap()
            .trim(),
        project.0.to_str().unwrap()
    );
    let lake = project.lake("printf 'warning: Dep.lean:1:0: warning\\n'");
    let outcome = leanck_lake::build::execute(&lake, &project.0, BuildOptions::default(), None)
        .await
        .unwrap();
    let BuildOutcome::Completed { output, report } = outcome else {
        panic!("warnings are allowed by default");
    };
    assert!(output.stdout.contains("warning"));
    assert!(report.warnings.is_empty());
    let outcome = leanck_lake::build::execute(
        &lake,
        &project.0,
        BuildOptions {
            strict: true,
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    let BuildOutcome::Failed { output, report } = outcome else {
        panic!("strict mode must reject warnings");
    };
    assert!(
        output.success,
        "policy failure is distinct from process failure"
    );
    assert_eq!(report.warnings.len(), 1);
}

#[tokio::test]
async fn cache_outcomes_distinguish_skip_success_failure_and_timeout() {
    let project = Project::new();
    let absent = Lake::new(project.0.join("absent"));
    assert!(matches!(
        leanck_lake::cache::fetch_with(&absent, &project.0, CacheOptions::default())
            .await
            .unwrap(),
        CacheOutcome::Skipped(SkipReason::NotMathlib)
    ));
    project.manifest("not json");
    assert!(!leanck_lake::cache::depends_on_mathlib(&project.0));
    project.manifest(r#"{"packages":[{"name":"batteries"},{"name":"Mathlib"}]}"#);
    assert!(leanck_lake::cache::depends_on_mathlib(&project.0));
    assert!(matches!(
        leanck_lake::cache::fetch_with(
            &absent,
            &project.0,
            CacheOptions {
                enabled: false,
                ..Default::default()
            }
        )
        .await
        .unwrap(),
        CacheOutcome::Skipped(SkipReason::Disabled)
    ));
    assert!(matches!(
        leanck_lake::cache::fetch_with(&absent, &project.0, CacheOptions::default()).await,
        Err(LakeError::Spawn { .. })
    ));
    let lake = project.lake("printf '%s\\n' \"$@\" > args\nprintf 'downloaded\\n'");
    let CacheOutcome::Completed(output) =
        leanck_lake::cache::fetch_with(&lake, &project.0, CacheOptions::default())
            .await
            .unwrap()
    else {
        panic!("cache completed");
    };
    assert_eq!(output.stdout, "downloaded\n");
    assert_eq!(
        std::fs::read_to_string(project.0.join("args")).unwrap(),
        "exe\ncache\nget\n"
    );
    let lake = project.lake("printf 'offline\\n' >&2\nexit 7");
    let CacheOutcome::Failed(output) =
        leanck_lake::cache::fetch_with(&lake, &project.0, CacheOptions::default())
            .await
            .unwrap()
    else {
        panic!("cache failed");
    };
    assert_eq!(output.exit_code, Some(7));
    assert_eq!(output.stderr, "offline\n");
    let lake = project.lake("printf '%s' \"$$\" > pid\nexec sleep 5");
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        leanck_lake::cache::fetch_with(
            &lake,
            &project.0,
            CacheOptions {
                execution: leanck_lake::ExecutionOptions {
                    timeout: Duration::from_millis(100),
                    ..Default::default()
                },
                ..Default::default()
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(outcome, CacheOutcome::TimedOut));
    let pid = std::fs::read_to_string(project.0.join("pid")).unwrap();
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success(),
        "timed-out child is reaped"
    );
    project.manifest(r#"{"packages":[{"name":"batteries"}]}"#);
    assert!(!leanck_lake::cache::depends_on_mathlib(&project.0));
}

#[tokio::test]
async fn build_timeout_and_spawn_errors_are_inspectable() {
    let project = Project::new();
    let lake = project.lake("exec sleep 5");
    let outcome = leanck_lake::build::execute(
        &lake,
        &project.0,
        BuildOptions::default(),
        Some(Duration::from_millis(50)),
    )
    .await
    .unwrap();
    assert!(matches!(outcome, BuildOutcome::TimedOut));
    assert!(matches!(
        leanck_lake::build::execute(
            &Lake::new(project.0.join("missing")),
            &project.0,
            BuildOptions::default(),
            None
        )
        .await,
        Err(LakeError::Spawn { .. })
    ));
}

#[tokio::test]
async fn dropping_a_utility_future_stops_the_owned_process_group() {
    let project = Project::new();
    let lake = project
        .lake("sleep 30 &\nprintf '%s' \"$!\" > descendant\nprintf '%s' \"$$\" > parent\nwait");
    let root = project.0.clone();
    let work = tokio::spawn(async move { lake.run(&root, &[], None).await });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !project.0.join("parent").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let descendant = std::fs::read_to_string(project.0.join("descendant")).unwrap();
    let parent = std::fs::read_to_string(project.0.join("parent")).unwrap();
    work.abort();
    let _ = work.await;
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let running = [&parent, &descendant].iter().any(|pid| {
                let output = std::process::Command::new("ps")
                    .args(["-o", "stat=", "-p", pid])
                    .output()
                    .unwrap();
                let state = String::from_utf8_lossy(&output.stdout);
                !state.trim().is_empty() && !state.trim().starts_with('Z')
            });
            if !running {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("utility and its descendant stopped");
}

#[tokio::test]
async fn output_limit_covers_one_unterminated_line_and_reaps_the_child() {
    let project = Project::new();
    let lake = project.lake("printf '%s' \"$$\" > pid\nhead -c 65536 /dev/zero\nexec sleep 30");
    let result = lake
        .run_with(
            &project.0,
            &[],
            leanck_lake::ExecutionOptions {
                max_output_bytes: 1024,
                ..Default::default()
            },
        )
        .await;
    assert!(matches!(
        result,
        Err(LakeError::OutputLimit {
            stream: OutputStream::Stdout,
            limit: 1024
        })
    ));
    let pid = std::fs::read_to_string(project.0.join("pid")).unwrap();
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
}
#[tokio::test]
async fn stderr_output_limit_reports_the_typed_stream() {
    let project = Project::new();
    let lake = project.lake("printf 'too much output' >&2");
    let error = lake
        .run_with(
            &project.0,
            &[],
            leanck_lake::ExecutionOptions {
                max_output_bytes: 4,
                ..Default::default()
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        LakeError::OutputLimit {
            stream: OutputStream::Stderr,
            limit: 4,
        }
    ));
}

#[tokio::test]
async fn token_cancellation_reaps_the_child_before_returning_and_preserves_the_parent_token() {
    let project = Project::new();
    let lake = project.lake("printf '%s' \"$$\" > pid\nexec sleep 30");
    let cancel = leanck_lake::CancellationToken::new();
    let token = cancel.clone();
    let root = project.0.clone();
    let work = tokio::spawn(async move {
        leanck_lake::build::execute_with(
            &lake,
            &root,
            BuildOptions::default(),
            leanck_lake::ExecutionOptions {
                cancellation: Some(token),
                ..Default::default()
            },
        )
        .await
    });
    tokio::time::timeout(Duration::from_secs(2), async {
        while !project.0.join("pid").exists() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    cancel.cancel();
    assert!(matches!(work.await.unwrap(), Err(LakeError::Cancelled)));
    let pid = std::fs::read_to_string(project.0.join("pid")).unwrap();
    assert!(
        !std::process::Command::new("kill")
            .args(["-0", &pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
    );
    let token = leanck_lake::CancellationToken::new();
    let lake = project.lake("printf 'complete\\n'");
    lake.run_with(
        &project.0,
        &[],
        leanck_lake::ExecutionOptions {
            cancellation: Some(token.clone()),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(
        !token.is_cancelled(),
        "completion must not cancel the caller's reusable token"
    );
}
#[tokio::test]
async fn execution_provenance_distinguishes_process_success_from_strict_policy_failure() {
    let project = Project::new();
    let lake = project.lake("printf 'warning: configuration warning\\n'");
    let outcome = leanck_lake::build::execute(
        &lake,
        &project.0,
        BuildOptions {
            strict: true,
            targets: vec!["MyTarget".into()],
            ..Default::default()
        },
        None,
    )
    .await
    .unwrap();
    let BuildOutcome::Failed { output, report } = outcome else {
        panic!("strict warnings must fail")
    };
    assert!(output.success);
    assert_eq!(report.warnings, ["warning: configuration warning"]);
    let provenance = report.provenance.unwrap();
    assert_eq!(provenance.exit_code, Some(0));
    assert!(provenance.strict && provenance.warnings_included);
    assert_eq!(provenance.execution.workspace, project.0.to_string_lossy());
    assert_eq!(provenance.execution.args, ["build", "MyTarget"]);
    assert!(provenance.execution.coordinated);
}
#[tokio::test]
async fn invalid_options_do_not_spawn_and_exact_output_limits_preserve_utf8() {
    let project = Project::new();
    let lake = project.lake("printf '😀'");
    assert!(matches!(
        lake.run_with(
            &project.0,
            &[],
            leanck_lake::ExecutionOptions {
                max_output_bytes: 0,
                ..Default::default()
            }
        )
        .await,
        Err(LakeError::InvalidOptions(
            ExecutionOptionsError::ZeroOutputLimit
        ))
    ));
    let outcome = lake
        .run_with(
            &project.0,
            &[],
            leanck_lake::ExecutionOptions {
                max_output_bytes: 4,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    let leanck_lake::CommandOutcome::Finished(output) = outcome else {
        panic!("finished")
    };
    assert_eq!(output.stdout, "😀");
    assert!(matches!(
        lake.run(&project.0, &[], Some(Duration::ZERO))
            .await
            .unwrap(),
        leanck_lake::CommandOutcome::TimedOut
    ));
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn report_provenance_remains_json_serializable_for_non_utf8_workspace_paths() {
    use std::os::unix::{ffi::OsStringExt, fs::PermissionsExt};
    let project = Project::new();
    let root = project
        .0
        .join(std::ffi::OsString::from_vec(b"workspace-\xff".to_vec()));
    std::fs::create_dir(&root).unwrap();
    let executable = root.join("lake");
    std::fs::write(&executable, "#!/bin/sh\nexit 0\n").unwrap();
    std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o700)).unwrap();
    let outcome =
        leanck_lake::build::execute(&Lake::new(executable), &root, BuildOptions::default(), None)
            .await
            .unwrap();
    let BuildOutcome::Completed { report, .. } = outcome else {
        panic!("completed")
    };
    assert!(
        serde_json::to_value(report).unwrap()["provenance"]["execution"]["workspace"]
            .as_str()
            .unwrap()
            .contains("workspace-")
    );
}
