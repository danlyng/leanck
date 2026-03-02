//! Live smoke test against a real `lake serve`. Ignored by default; run with:
//!
//! ```sh
//! LEANCKD_TEST_PROJECT=/path/to/test-project \
//!   cargo test -p leanckd --test live_smoke -- --ignored --nocapture
//! ```

use std::path::PathBuf;

use leanckd::proto::{DiagnosticsParams, GoalParams, ListParams, ReferencesParams, SorryParams};
use leanckd::query;
use leanckd::workspace::Workspace;

fn project() -> Option<PathBuf> {
    let p = std::env::var("LEANCKD_TEST_PROJECT").ok()?;
    PathBuf::from(p).canonicalize().ok()
}

#[tokio::test]
#[ignore]
async fn diagnostics_goal_list_sorry() {
    let Some(worktree) = project() else {
        eprintln!("set LEANCKD_TEST_PROJECT to run this test");
        return;
    };
    let ws = Workspace::start(&worktree).await.expect("workspace start");

    let file = "TestProject/Basic.lean";

    // Whole-file diagnostics, all severities (the file has a deliberate error).
    let diags = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: file.into(),
            severity: Some("all".into()),
            ..Default::default()
        },
        None,
    )
    .await
    .expect("diagnostics");
    println!("=== DIAGNOSTICS ===\n{}", serde_json::to_string_pretty(&diags).unwrap());
    assert!(diags.get("version").is_some());

    // List declarations (expect `hello_world`).
    let list = query::list(&ws, &ListParams { file: file.into(), kind: None }, None)
        .await
        .expect("list");
    println!("=== LIST ===\n{}", serde_json::to_string_pretty(&list).unwrap());
    let names: Vec<String> = list["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect();
    assert!(names.iter().any(|n| n == "hello_world"), "list missing hello_world: {names:?}");

    // Goal inside the proof.
    let goal = query::goal(&ws, &GoalParams { file: file.into(), line: 1, character: 2 }, None)
        .await
        .expect("goal");
    println!("=== GOAL ===\n{}", serde_json::to_string_pretty(&goal).unwrap());

    // Sorry scan (none expected here).
    let sorry = query::sorry(&ws, &SorryParams { path: file.into() }, None)
        .await
        .expect("sorry");
    println!("=== SORRY ===\n{}", serde_json::to_string_pretty(&sorry).unwrap());

    // A clean file must come back with the confirmed "ok" note — i.e. the
    // `textDocument/waitForDiagnostics` confirmation succeeded against the live
    // server (on failure the handler degrades to a "may be incomplete" note).
    let clean_rel = "TestProject/LiveSmokeClean.lean";
    let clean_abs = worktree.join(clean_rel);
    std::fs::write(&clean_abs, "theorem live_smoke_clean : 1 + 1 = 2 := rfl\n")
        .expect("write clean file");
    let clean = query::diagnostics(
        &ws,
        &DiagnosticsParams { file: clean_rel.into(), ..Default::default() },
        None,
    )
    .await;
    let _ = std::fs::remove_file(&clean_abs);
    let clean = clean.expect("clean diagnostics");
    println!("=== CLEAN ===\n{}", serde_json::to_string_pretty(&clean).unwrap());
    let notes: Vec<&str> =
        clean["notes"].as_array().unwrap().iter().filter_map(|n| n.as_str()).collect();
    assert!(notes.contains(&"ok"), "clean file should be confirmed ok, got notes: {notes:?}");
    assert_eq!(clean["total"], 0);

    // References to `hello_world`. Load a consumer file first so the server has
    // a reference site to report; assert the op returns a well-formed
    // `{symbol, locations}` shape (consumer presence depends on the .ilean index,
    // so it's printed rather than hard-asserted).
    let user_rel = "TestProject/LiveSmokeUser.lean";
    let user_abs = worktree.join(user_rel);
    std::fs::write(
        &user_abs,
        "import TestProject.Basic\ntheorem live_smoke_user : True := by have := hello_world; trivial\n",
    )
    .expect("write user file");
    let _ = query::diagnostics(
        &ws,
        &DiagnosticsParams { file: user_rel.into(), severity: Some("all".into()), ..Default::default() },
        None,
    )
    .await;
    let refs = query::references(
        &ws,
        &ReferencesParams { file: file.into(), ident: "hello_world".into(), namespace: None },
        None,
    )
    .await;
    let _ = std::fs::remove_file(&user_abs);
    let refs = refs.expect("references");
    println!("=== REFERENCES ===\n{}", serde_json::to_string_pretty(&refs).unwrap());
    assert_eq!(refs["symbol"], "hello_world");
    assert!(refs["locations"].is_array(), "references must return a locations array");

    ws.shutdown().await;
}
