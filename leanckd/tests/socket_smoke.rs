//! End-to-end test of the daemon over its Unix socket. Ignored by default:
//!
//! ```sh
//! LEANCKD_TEST_PROJECT=/path/to/test-project \
//!   cargo test -p leanckd --test socket_smoke -- --ignored --nocapture
//! ```

use std::time::Duration;

use leanck_core::{
    api::{Request, Response, op},
    error::ErrorKind,
    wire,
};
use serde_json::json;
use tokio::net::UnixStream;

#[path = "../../test-support/mod.rs"]
mod support;

async fn call(worktree: &std::path::Path, req: &Request) -> Response {
    tokio::time::timeout(Duration::from_secs(150), call_inner(worktree, req))
        .await
        .expect("socket request must finish")
}

async fn call_inner(worktree: &std::path::Path, req: &Request) -> Response {
    let socket = leanck_core::paths::socket_path(worktree);
    let mut stream = UnixStream::connect(&socket).await.expect("connect");
    let (mut r, mut w) = stream.split();
    wire::write_frame(
        &mut leanck_core::wire::writer(&mut w),
        &serde_json::to_vec(req).unwrap(),
    )
    .await
    .unwrap();
    let frame = wire::read_frame(&mut leanck_core::wire::reader(&mut r))
        .await
        .unwrap()
        .expect("response");
    serde_json::from_slice(&frame).unwrap()
}

#[tokio::test]
#[ignore]
async fn daemon_serves_over_socket() {
    let Some(mut project) = support::TestProject::from_env() else {
        eprintln!("set LEANCKD_TEST_PROJECT to run this test");
        return;
    };
    let worktree = project.path().to_path_buf();

    // Start the daemon runtime in the background.
    let wt = worktree.clone();
    let daemon = tokio::spawn(async move { leanckd::run(&wt).await });

    // Wait for the socket to accept.
    let socket = leanck_core::paths::socket_path(&worktree);
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // health
    let health = call(
        &worktree,
        &Request {
            id: "1".into(),
            op: op::HEALTH.into(),
            timeout_ms: None,
            params: json!({}),
        },
    )
    .await;
    println!("HEALTH: {}", serde_json::to_string_pretty(&health).unwrap());
    assert!(health.ok);
    project.track_lean(
        health.result.as_ref().unwrap()["server"]["pid"]
            .as_u64()
            .map(|p| p as u32),
    );
    let mut expected = support::fixture("health");
    let actual = health.result.unwrap();
    expected["workspace"] = actual["workspace"].clone();
    expected["server"]["pid"] = actual["server"]["pid"].clone();
    expected["idle_secs"] = actual["idle_secs"].clone();
    assert_eq!(actual["lifecycle"]["state"], "ready");
    assert_eq!(actual["lifecycle"]["active_requests"], 0);

    assert_eq!(actual, expected);

    // diagnostics
    let diags = call(
        &worktree,
        &Request {
            id: "2".into(),
            op: op::DIAGNOSTICS.into(),
            timeout_ms: Some(120_000),
            params: json!({ "file": "TestProject/Basic.lean", "severity": {"mode": "all"} }),
        },
    )
    .await;
    println!(
        "DIAGNOSTICS: {}",
        serde_json::to_string_pretty(&diags).unwrap()
    );
    assert!(diags.ok, "diagnostics failed: {:?}", diags.error);
    assert_eq!(diags.result.unwrap()["total"], 0);
    let unsupported = call(
        &worktree,
        &Request {
            id: "contract".into(),
            op: "unsupported".into(),
            timeout_ms: None,
            params: json!({}),
        },
    )
    .await;
    assert_eq!(
        serde_json::to_value(unsupported).unwrap(),
        support::fixture("error")
    );
    for (file, expected_kind) in [
        ("missing.lean", ErrorKind::NotFound),
        ("https://example.com/Bad.lean", ErrorKind::BadRequest),
    ] {
        let failure = call(
            &worktree,
            &Request {
                id: "invalid-document".into(),
                op: op::LIST.into(),
                timeout_ms: None,
                params: json!({"file":file}),
            },
        )
        .await;
        assert!(!failure.ok);
        assert_eq!(failure.error.unwrap().kind, expected_kind);
    }
    let restart = call(
        &worktree,
        &Request {
            id: "restart".into(),
            op: op::RESTART_FILE.into(),
            timeout_ms: None,
            params: json!({"file":"TestProject/Basic.lean"}),
        },
    )
    .await;
    assert!(restart.ok);
    assert_eq!(restart.result.unwrap()["restarted"], true);

    project.shutdown_daemon().await;
    daemon.await.unwrap().unwrap();
}

/// The original bug: many agents hammering the same worktree concurrently used
/// to deadlock the shared VSCode server. Fire a burst of overlapping requests at
/// one daemon and confirm they all complete with bounded latency.
#[tokio::test]
#[ignore]
async fn concurrent_requests_do_not_deadlock() {
    let Some(mut project) = support::TestProject::from_env() else {
        eprintln!("set LEANCKD_TEST_PROJECT to run this test");
        return;
    };
    let worktree = project.path().to_path_buf();

    let wt = worktree.clone();
    let daemon = tokio::spawn(async move { leanckd::run(&wt).await });
    let socket = leanck_core::paths::socket_path(&worktree);
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let health = call(
        &worktree,
        &Request {
            id: "health".into(),
            op: op::HEALTH.into(),
            timeout_ms: None,
            params: json!({}),
        },
    )
    .await;
    project.track_lean(
        health.result.unwrap()["server"]["pid"]
            .as_u64()
            .map(|p| p as u32),
    );
    let file = "TestProject/Basic.lean";
    let ops = [
        (
            op::DIAGNOSTICS,
            json!({ "file": file, "severity": {"mode": "all"} }),
        ),
        (op::GOAL, json!({ "file": file, "line": 1, "character": 2 })),
        (op::LIST, json!({ "file": file })),
        (op::SORRY, json!({ "path": file })),
    ];

    // 24 overlapping requests (6 rounds of the 4 ops), all concurrent.
    let mut handles = Vec::new();
    for i in 0..24 {
        let (op_name, params) = ops[i % ops.len()].clone();
        let worktree = worktree.clone();
        handles.push(tokio::spawn(async move {
            let resp = call(
                &worktree,
                &Request {
                    id: format!("c{i}"),
                    op: op_name.into(),
                    timeout_ms: Some(120_000),
                    params,
                },
            )
            .await;
            (i, resp.ok, resp.error)
        }));
    }

    let results = tokio::time::timeout(Duration::from_secs(150), futures_join(handles))
        .await
        .expect("concurrent burst must not hang");

    for (i, ok, err) in &results {
        assert!(*ok, "request {i} failed: {err:?}");
    }
    println!("all {} concurrent requests succeeded", results.len());

    project.shutdown_daemon().await;
    daemon.await.unwrap().unwrap();
}

/// Minimal join over a set of `JoinHandle`s without pulling in `futures`.
async fn futures_join<T>(handles: Vec<tokio::task::JoinHandle<T>>) -> Vec<T> {
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.expect("task panicked"));
    }
    out
}
