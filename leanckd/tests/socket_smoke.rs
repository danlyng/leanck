//! End-to-end test of the daemon over its Unix socket. Ignored by default:
//!
//! ```sh
//! LEANCKD_TEST_PROJECT=/path/to/test-project \
//!   cargo test -p leanckd --test socket_smoke -- --ignored --nocapture
//! ```

use std::path::PathBuf;
use std::time::Duration;

use leanckd::proto::{self, op, Request, Response};
use serde_json::json;
use tokio::net::UnixStream;

fn project() -> Option<PathBuf> {
    let p = std::env::var("LEANCKD_TEST_PROJECT").ok()?;
    PathBuf::from(p).canonicalize().ok()
}

/// Both tests target the same per-worktree daemon socket and end with SHUTDOWN,
/// so running them concurrently kills one test's daemon under the other.
static SOCKET_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn call(worktree: &std::path::Path, req: &Request) -> Response {
    let socket = leanckd::paths::socket_path(worktree);
    let mut stream = UnixStream::connect(&socket).await.expect("connect");
    let (mut r, mut w) = stream.split();
    proto::write_frame(&mut w, &serde_json::to_vec(req).unwrap())
        .await
        .unwrap();
    let frame = proto::read_frame(&mut r).await.unwrap().expect("response");
    serde_json::from_slice(&frame).unwrap()
}

#[tokio::test]
#[ignore]
async fn daemon_serves_over_socket() {
    let Some(worktree) = project() else {
        eprintln!("set LEANCKD_TEST_PROJECT to run this test");
        return;
    };
    let _serial = SOCKET_LOCK.lock().await;

    // Start the daemon runtime in the background.
    let wt = worktree.clone();
    tokio::spawn(async move {
        let _ = leanckd::daemon::run(&wt).await;
    });

    // Wait for the socket to accept.
    let socket = leanckd::paths::socket_path(&worktree);
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // health
    let health = call(
        &worktree,
        &Request { id: "1".into(), op: op::HEALTH.into(), timeout_ms: None, params: json!({}) },
    )
    .await;
    println!("HEALTH: {}", serde_json::to_string_pretty(&health).unwrap());
    assert!(health.ok);

    // diagnostics
    let diags = call(
        &worktree,
        &Request {
            id: "2".into(),
            op: op::DIAGNOSTICS.into(),
            timeout_ms: Some(120_000),
            params: json!({ "file": "TestProject/Basic.lean", "severity": "all" }),
        },
    )
    .await;
    println!("DIAGNOSTICS: {}", serde_json::to_string_pretty(&diags).unwrap());
    assert!(diags.ok, "diagnostics failed: {:?}", diags.error);
    assert!(diags.result.unwrap()["total"].as_u64().unwrap() >= 1);

    // shutdown
    let _ = call(
        &worktree,
        &Request { id: "3".into(), op: op::SHUTDOWN.into(), timeout_ms: None, params: json!({}) },
    )
    .await;
}

/// The original bug: many agents hammering the same worktree concurrently used
/// to deadlock the shared VSCode server. Fire a burst of overlapping requests at
/// one daemon and confirm they all complete with bounded latency.
#[tokio::test]
#[ignore]
async fn concurrent_requests_do_not_deadlock() {
    let Some(worktree) = project() else {
        eprintln!("set LEANCKD_TEST_PROJECT to run this test");
        return;
    };
    let _serial = SOCKET_LOCK.lock().await;

    let wt = worktree.clone();
    tokio::spawn(async move {
        let _ = leanckd::daemon::run(&wt).await;
    });
    let socket = leanckd::paths::socket_path(&worktree);
    for _ in 0..100 {
        if UnixStream::connect(&socket).await.is_ok() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    let file = "TestProject/Basic.lean";
    let ops = [
        (op::DIAGNOSTICS, json!({ "file": file, "severity": "all" })),
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

    let _ = call(
        &worktree,
        &Request { id: "z".into(), op: op::SHUTDOWN.into(), timeout_ms: None, params: json!({}) },
    )
    .await;
}

/// Minimal join over a set of `JoinHandle`s without pulling in `futures`.
async fn futures_join<T>(handles: Vec<tokio::task::JoinHandle<T>>) -> Vec<T> {
    let mut out = Vec::with_capacity(handles.len());
    for h in handles {
        out.push(h.await.expect("task panicked"));
    }
    out
}
