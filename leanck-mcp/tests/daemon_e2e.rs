//! End-to-end: the `leanck-mcp` daemon client auto-spawns `leanckd` and gets
//! real results. Ignored by default; run with:
//!
//! ```sh
//! LEANCKD_BIN=target/debug/leanckd \
//! LEANCKD_TEST_PROJECT=/path/to/test-project \
//!   cargo test -p leanck-mcp --test daemon_e2e -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use leanck_mcp::daemon::DaemonCache;
use serde_json::json;

fn project() -> Option<PathBuf> {
    PathBuf::from(std::env::var("LEANCKD_TEST_PROJECT").ok()?).canonicalize().ok()
}

#[tokio::test]
#[ignore]
async fn client_auto_spawns_and_queries() {
    let Some(worktree) = project() else {
        eprintln!("set LEANCKD_TEST_PROJECT (and LEANCKD_BIN) to run this test");
        return;
    };

    let cache = DaemonCache::new(worktree);
    // First get() auto-spawns the daemon (cold start).
    let client = cache.get().await.expect("daemon should auto-spawn");

    let health = client.healthz().await.expect("health");
    println!("HEALTH: {}", serde_json::to_string_pretty(&health).unwrap());
    assert_eq!(health["ok"], json!(true));

    let diags = client
        .diagnostics(&json!({ "file": "TestProject/Basic.lean", "severity": "all" }))
        .await
        .expect("diagnostics");
    println!("DIAGNOSTICS: {}", serde_json::to_string_pretty(&diags).unwrap());
    assert!(diags["total"].as_u64().unwrap() >= 1);

    let list = client
        .list(&json!({ "file": "TestProject/Basic.lean", "kind": [] }))
        .await
        .expect("list");
    println!("LIST: {}", serde_json::to_string_pretty(&list).unwrap());
    let names: Vec<String> = list["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect();
    assert!(names.iter().any(|n| n == "hello_world"), "missing hello_world: {names:?}");

    // Second cache.get() should reuse the already-running daemon (no spawn).
    let client2 = cache.get().await.expect("reuse running daemon");
    let goal = client2
        .goal(&json!({ "file": "TestProject/Basic.lean", "line": 1, "character": 2 }))
        .await
        .expect("goal");
    println!("GOAL: {}", serde_json::to_string_pretty(&goal).unwrap());

    // Shut the daemon down so the test leaves nothing running.
    let _ = client2.restart_file(&json!({ "file": "TestProject/Basic.lean" })).await;
}
