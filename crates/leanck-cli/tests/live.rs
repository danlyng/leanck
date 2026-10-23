//! Real CLI queries and builds exercise the shared Lean project, ignored by default.
//!
//! ```sh
//! cargo test -p leanck-cli --test live --locked -- --ignored
//! ```
//!
//! Set `LEANCK_TEST_PROJECT` to optionally override the in-repo template.

use serde_json::Value;
use tokio::process::Command;

#[path = "../../../test-support/mod.rs"]
mod support;

async fn run(project: &support::TestProject, args: &[&str]) -> Value {
    let output = Command::new(env!("CARGO_BIN_EXE_leanck"))
        .args(["--set", "cache.enabled=false", "--set", "instances.limit=0"])
        .args(args)
        .arg("--json")
        .current_dir(project.path())
        .kill_on_drop(true)
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "leanck {args:?}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).expect("CLI JSON output")
}

#[tokio::test]
#[ignore]
async fn cli_queries_and_builds_the_template() {
    let mut project = support::TestProject::new();
    let clean = run(
        &project,
        &[
            "diagnostics",
            "TestProject/Basic.lean",
            "--severity",
            "error",
        ],
    )
    .await;
    assert_eq!(clean["total"], 0, "{clean}");
    assert!(
        clean["diagnostics"].as_array().unwrap().is_empty(),
        "{clean}"
    );

    let warnings = run(
        &project,
        &["diagnostics", support::WARNINGS, "--severity", "warning"],
    )
    .await;
    assert_eq!(warnings["total"], 2, "{warnings}");
    assert_eq!(
        warnings["diagnostics"].as_array().unwrap().len(),
        2,
        "{warnings}"
    );

    let sorries = run(&project, &["sorry", support::WARNINGS]).await;
    // Each `by sorry` is reported as a declaration warning and as a tactic token.
    for kind in ["declaration", "tactic"] {
        let count = sorries["sorries"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|sorry| sorry["kind"] == kind)
            .count();
        assert_eq!(count, 2, "{kind}: {sorries}");
    }

    let build = run(&project, &["build"]).await;
    assert_eq!(build["success"], true, "{build}");
    project.shutdown_daemon().await;
}
