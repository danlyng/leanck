//! End-to-end: the `leanck-mcp` daemon client auto-spawns `leanckd` and gets
//! real results. Ignored by default; run with:
//!
//! ```sh
//! cargo build --workspace --locked
//!
//! cargo test -p leanck-mcp --test daemon_e2e -- --ignored --nocapture
//! ```
//!
//! Set `LEANCK_TEST_PROJECT` to optionally override the in-repo template.

use leanck_client::RequestOptions;
use leanck_core::{
    api::{DiagnosticsParams, GoalParams, GoalResult, ListParams},
    workspace::WorkspaceRoot,
};
use leanck_mcp::workspace::WorkspaceSelection;

#[path = "../../../test-support/mod.rs"]
mod support;

#[tokio::test]
#[ignore]
async fn client_auto_spawns_and_queries() {
    let mut project = support::TestProject::new();

    let root = WorkspaceRoot::from_path(project.path()).unwrap();
    let selection = WorkspaceSelection::new(root.clone());
    let daemon = std::env::current_exe()
        .unwrap()
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("leanck");
    assert!(
        daemon.is_file(),
        "build the workspace before running this live test"
    );
    let mut settings = leanck_config::Settings::default();
    settings.client.daemon_bin = Some(daemon);
    selection.set_configured(root, settings);
    // The first operation auto-spawns the daemon (cold start).
    let client = selection.capture();

    let health = client
        .health(RequestOptions::default())
        .await
        .expect("health");
    println!("HEALTH: {}", serde_json::to_string_pretty(&health).unwrap());
    assert!(health.ok);
    project.track_lean(health.server.pid);

    let diags = client
        .diagnostics(
            &DiagnosticsParams {
                file: "TestProject/Basic.lean".into(),
                severity: leanck_core::api::SeverityFilter::All,
                ..Default::default()
            },
            RequestOptions::default(),
        )
        .await
        .expect("diagnostics");
    println!(
        "DIAGNOSTICS: {}",
        serde_json::to_string_pretty(&diags).unwrap()
    );
    let mut expected = support::fixture("clean_diagnostics");
    expected["uri"] = serde_json::json!(diags.uri);
    expected["source"] = serde_json::json!(
        std::fs::read_to_string(project.path().join("TestProject/Basic.lean")).unwrap()
    );
    assert_eq!(serde_json::to_value(diags).unwrap(), expected);

    let list = client
        .list(
            &ListParams {
                file: "TestProject/Basic.lean".into(),
                kind: Some(vec![]),
            },
            RequestOptions::default(),
        )
        .await
        .expect("list");
    println!("LIST: {}", serde_json::to_string_pretty(&list).unwrap());
    let names: Vec<_> = list
        .symbols
        .iter()
        .map(|symbol| symbol.name.as_str())
        .collect();
    assert!(
        names.contains(&"hello_world"),
        "missing hello_world: {names:?}"
    );

    // A second captured client reuses the already-running daemon.
    let client2 = selection.capture();
    let goal = client2
        .goal(
            &GoalParams {
                file: "TestProject/Basic.lean".into(),
                line: 1,
                character: 2,
            },
            RequestOptions::default(),
        )
        .await
        .expect("goal");
    assert_eq!(goal, GoalResult::NoGoal);
    assert_eq!(
        client2
            .health(RequestOptions::default())
            .await
            .unwrap()
            .server
            .pid,
        health.server.pid
    );

    // Shut the daemon down so the test leaves nothing running.
    project.shutdown_daemon().await;
}
