//! Live smoke test against a real `lake serve`. Ignored by default; run with:
//!
//! ```sh
//! cargo test -p leanckd --lib live_smoke -- --ignored --nocapture
//! ```
//!
//! Set `LEANCK_TEST_PROJECT` to optionally override the in-repo template.

use crate::query;
use crate::workspace::Workspace;
use leanck_core::api::{DiagnosticsParams, GoalParams, ListParams, ReferencesParams, SorryParams};

#[path = "../../../test-support/mod.rs"]
mod support;
#[tokio::test]
#[ignore]
async fn diagnostics_goal_list_sorry() {
    let mut project = support::TestProject::new();
    let worktree = project.path().to_path_buf();
    // Standalone builds neither start Lean's server nor require daemon RPC.
    let standalone = leanck_lake::build::run(
        &worktree,
        leanck_lake::build::BuildOptions {
            targets: vec!["TestProject.Dep".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(standalone.success, "{:?}", standalone.errors);
    assert!(standalone.provenance.unwrap().execution.coordinated);
    assert!(
        !leanck_core::paths::RuntimePaths::for_workspace(&worktree)
            .socket()
            .exists()
    );
    let unknown = leanck_lake::build::run(
        &worktree,
        leanck_lake::build::BuildOptions {
            targets: vec!["Stage8NoSuchTarget".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!unknown.success);
    assert!(
        unknown
            .errors
            .iter()
            .any(|e| e.contains("Stage8NoSuchTarget"))
    );
    let ws = Workspace::start(&worktree).await.expect("workspace start");

    project.track_lean(ws.pid());
    let file = "TestProject/Basic.lean";

    // Whole-file diagnostics on the clean template.
    let diags = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: file.into(),
            severity: leanck_core::api::SeverityFilter::All,
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .expect("diagnostics");
    let diags = serde_json::to_value(diags).unwrap();
    println!(
        "=== DIAGNOSTICS ===\n{}",
        serde_json::to_string_pretty(&diags).unwrap()
    );
    let mut expected = support::fixture("clean_diagnostics");
    expected["uri"] = diags["uri"].clone();
    expected["source"] = serde_json::json!(std::fs::read_to_string(worktree.join(file)).unwrap());
    assert_eq!(diags, expected);

    // List declarations (expect `hello_world`).
    let list = query::list(
        &ws,
        &ListParams {
            file: file.into(),
            kind: None,
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .expect("list");
    let list = serde_json::to_value(list).unwrap();
    println!(
        "=== LIST ===\n{}",
        serde_json::to_string_pretty(&list).unwrap()
    );
    let names: Vec<String> = list["symbols"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|s| s["name"].as_str().map(str::to_string))
        .collect();
    assert!(
        names.iter().any(|n| n == "hello_world"),
        "list missing hello_world: {names:?}"
    );

    // A non-tactic position returns null, distinct from an empty goals array.
    let goal = query::goal(
        &ws,
        &GoalParams {
            file: file.into(),
            line: 1,
            character: 2,
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .expect("goal");
    let goal = serde_json::to_value(goal).unwrap();
    assert_eq!(goal, support::fixture("goal_null"));

    // Sorry scan (none expected here).
    let sorry = query::sorry(
        &ws,
        &SorryParams { path: file.into() },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .expect("sorry");
    let sorry = serde_json::to_value(sorry).unwrap();
    println!(
        "=== SORRY ===\n{}",
        serde_json::to_string_pretty(&sorry).unwrap()
    );

    // A clean file must come back with the confirmed "ok" note — i.e. the
    // `textDocument/waitForDiagnostics` confirmation succeeded against the live
    // server (on failure the handler degrades to a "may be incomplete" note).
    let clean_rel = "TestProject/LiveSmokeClean.lean";
    let clean_abs = worktree.join(clean_rel);
    std::fs::write(&clean_abs, "theorem live_smoke_clean : 1 + 1 = 2 := rfl\n")
        .expect("write clean file");
    let clean = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: clean_rel.into(),
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await;
    let _ = std::fs::remove_file(&clean_abs);
    let clean = clean.expect("clean diagnostics");
    let clean = serde_json::to_value(clean).unwrap();
    println!(
        "=== CLEAN ===\n{}",
        serde_json::to_string_pretty(&clean).unwrap()
    );
    let notes: Vec<&str> = clean["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n.as_str())
        .collect();
    assert!(
        notes.contains(&"ok"),
        "clean file should be confirmed ok, got notes: {notes:?}"
    );
    assert_eq!(clean["total"], 0);

    let accomplished = query::goal(
        &ws,
        &GoalParams {
            file: support::CLEAN.into(),
            line: 2,
            character: 9,
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    let accomplished = serde_json::to_value(accomplished).unwrap();
    assert_eq!(accomplished, support::fixture("goal_empty"));

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
        &DiagnosticsParams {
            file: user_rel.into(),
            severity: leanck_core::api::SeverityFilter::All,
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await;
    let refs = query::references(
        &ws,
        &ReferencesParams {
            file: file.into(),
            ident: "hello_world".into(),
            namespace: None,
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await;
    let _ = std::fs::remove_file(&user_abs);
    let refs = refs.expect("references");
    let refs = serde_json::to_value(refs).unwrap();
    println!(
        "=== REFERENCES ===\n{}",
        serde_json::to_string_pretty(&refs).unwrap()
    );
    assert_eq!(refs["symbol"], "hello_world");
    assert!(
        refs["locations"].is_array(),
        "references must return a locations array"
    );
    let mut expected = support::fixture("references");
    expected["uri"] = refs["uri"].clone();
    expected["locations"] = refs["locations"].clone();
    assert_eq!(refs, expected);

    // Two known warnings make pagination independent of the template contents.
    let warnings = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: support::WARNINGS.into(),
            severity: leanck_core::api::SeverityFilter::All,
            limit: Some(1),
            offset: Some(1),
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    let warnings = serde_json::to_value(warnings).unwrap();
    assert_eq!(warnings["total"], 2);
    assert_eq!(warnings["offset"], 1);
    assert!(warnings.get("limit").is_none());
    assert_eq!(warnings["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(warnings["diagnostics"][0]["severity"], 2);
    assert!(
        !warnings["notes"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("ok"))
    );
    let first_page = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: support::WARNINGS.into(),
            severity: leanck_core::api::SeverityFilter::All,
            limit: Some(1),
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    let first_page = serde_json::to_value(first_page).unwrap();
    assert_eq!(first_page["limit"], 1);
    assert!(first_page.get("offset").is_none());
    #[cfg(target_pointer_width = "64")]
    {
        let enormous_page = query::diagnostics(
            &ws,
            &DiagnosticsParams {
                file: support::WARNINGS.into(),
                severity: leanck_core::api::SeverityFilter::All,
                limit: Some(u64::MAX),
                offset: Some(u64::MAX),
                ..Default::default()
            },
            &crate::request::RequestContext::new(None, Default::default()).unwrap(),
        )
        .await
        .unwrap();
        assert_eq!(enormous_page.total, 2);
        assert_eq!(enormous_page.offset, Some(u64::MAX));
        assert!(enormous_page.limit.is_none());
        assert!(enormous_page.diagnostics.is_empty());
    }
    // Exercise the real Lean diagnostic adapter and best-effort goal enrichment.
    let error_file = "TestProject/Contract/Error.lean";
    std::fs::write(
        worktree.join(error_file),
        "theorem contract_error (n : Nat) : n = n := by\n  exact unknown_contract_name\n",
    )
    .unwrap();
    let errors = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: error_file.into(),
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    assert!(errors.total > 0);
    assert!(errors.diagnostics.iter().any(|diagnostic| {
        diagnostic.message.contains("unknown_contract_name")
            && diagnostic.severity == Some(leanck_core::api::Severity::Error)
    }));
    assert!(!errors.notes.iter().any(|note| note == "ok"));
    println!(
        "ENRICHED ERRORS: {}",
        serde_json::to_string(&errors).unwrap()
    );
    std::fs::remove_file(worktree.join(error_file)).unwrap();
    let file_sorries = query::sorry(
        &ws,
        &SorryParams {
            path: support::WARNINGS.into(),
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    let file_sorries = serde_json::to_value(file_sorries).unwrap();
    let kinds: Vec<_> = file_sorries["sorries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["declaration", "declaration", "tactic", "tactic"]);
    assert!(file_sorries.get("total").is_none());
    // Lean's own warning must catch a sorry that no scanned token spells.
    let hidden = "TestProject/Contract/Nested/Hidden.lean";
    std::fs::create_dir_all(worktree.join("TestProject/Contract/Nested")).unwrap();
    std::fs::write(
        worktree.join(hidden),
        "macro \"trust_me\" : tactic =>\n  `(tactic| exact $(Lean.mkIdent (.mkSimple (\"sorry\" ++ \"Ax\"))) _ false)\ntheorem live_hidden : 1 = 2 := by trust_me\n",
    )
    .unwrap();
    // The directory check must elaborate even a cold file with no sorry tokens.
    let directory_params = SorryParams {
        path: "TestProject/Contract".into(),
    };
    let lexical = leanck_core::sorry::scan_path(
        &worktree.join(&directory_params.path),
        || Ok::<_, String>(()),
        |message| message,
    )
    .unwrap();
    let leanck_core::api::SorryResult::Directory(lexical) = lexical else {
        panic!("expected directory report");
    };
    assert_eq!(lexical.total, 2);
    assert!(
        !lexical
            .files
            .iter()
            .any(|file| file.uri.ends_with("Hidden.lean"))
    );
    let directory = query::sorry(
        &ws,
        &directory_params,
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    let directory = serde_json::to_value(directory).unwrap();
    assert_eq!(directory["total"], 5);
    let files = directory["files"].as_array().unwrap();
    assert_eq!(files.len(), 2);
    let hidden_report = files
        .iter()
        .find(|file| file["uri"].as_str().unwrap().ends_with("Hidden.lean"))
        .unwrap();
    assert_eq!(hidden_report["sorries"][0]["kind"], "declaration");
    assert_eq!(hidden_report["sorries"][0]["range"]["start"]["line"], 2);
    assert!(directory.get("notes").is_none());
    let hidden_sorries = query::sorry(
        &ws,
        &SorryParams {
            path: hidden.into(),
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await;
    std::fs::remove_file(worktree.join(hidden)).unwrap();
    let hidden_sorries = serde_json::to_value(hidden_sorries.unwrap()).unwrap();
    let hidden_lines: Vec<_> = hidden_sorries["sorries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| {
            (
                entry["kind"].as_str().unwrap(),
                entry["range"]["start"]["line"].as_u64().unwrap(),
            )
        })
        .collect();
    assert_eq!(hidden_lines, [("declaration", 2)]);
    let directory = query::sorry(
        &ws,
        &SorryParams {
            path: "TestProject/Contract".into(),
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    let directory = serde_json::to_value(directory).unwrap();
    assert_eq!(directory["total"], 4);
    assert_eq!(directory["files"].as_array().unwrap().len(), 1);

    assert!(directory.get("notes").is_none());
    // Directory checks must not combine cached declaration warnings with edited text.
    std::fs::write(
        worktree.join(support::WARNINGS),
        "def sorry_string := \"sorry /- not a comment -/\"\n-- sorry\n",
    )
    .unwrap();
    let scan = query::sorry(
        &ws,
        &SorryParams {
            path: "TestProject/Contract".into(),
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    let scan = serde_json::to_value(scan).unwrap();
    assert_eq!(scan["total"], 0);
    assert!(scan["files"].as_array().unwrap().is_empty());
    assert!(scan.get("notes").is_none());

    // Files with only quality notes must remain visible even with zero sorries.
    let flagged = worktree.join("TestProject/Contract/Flagged.lean");
    std::fs::write(&flagged, "axiom contract_cheat : False\n").unwrap();
    let flagged_scan = query::sorry(
        &ws,
        &directory_params,
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await;
    std::fs::remove_file(flagged).unwrap();
    let flagged_scan = serde_json::to_value(flagged_scan.unwrap()).unwrap();
    assert_eq!(flagged_scan["total"], 0);
    assert_eq!(flagged_scan["files"].as_array().unwrap().len(), 1);
    assert!(
        flagged_scan["files"][0]["notes"][0]
            .as_str()
            .unwrap()
            .contains("axiom")
    );

    // A dependency edit must reopen its unchanged consumer and rebuild its imports.
    let dep = worktree.join("TestProject/Stage7Dependency.lean");
    let consumer = "TestProject/Stage7Consumer.lean";
    let source = "import TestProject.Stage7Dependency\nexample : stage7_value = 1 := rfl\n";
    std::fs::write(&dep, "def stage7_value : Nat := 1\n").unwrap();
    std::fs::write(worktree.join(consumer), source).unwrap();
    let before = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: consumer.into(),
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    assert_eq!(
        before.total, 0,
        "imported dependency before edit: {:?}",
        before.diagnostics
    );
    std::fs::write(&dep, "def stage7_value : Nat := 2\n").unwrap();
    let server_pid = ws.pid();
    let compiled = leanck_lake::build::run(
        &worktree,
        leanck_lake::build::BuildOptions {
            targets: vec!["TestProject.Stage7Dependency".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(compiled.success, "{:?}", compiled.errors);
    assert_eq!(ws.pid(), server_pid);

    let after = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: consumer.into(),
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    assert!(after.version > before.version);
    assert_eq!(after.source, source);
    assert!(after.total > 0, "dependency edit was not observed");
    std::fs::remove_file(dep).unwrap();
    std::fs::remove_file(worktree.join(consumer)).unwrap();

    // URI and path callers must share one open-document identity even when
    // the name requires escaping. A second call must not increment its version.
    let escaped = "TestProject/Contract/Space # %.lean";
    std::fs::write(
        worktree.join(escaped),
        "theorem contract_escaped : True := by trivial\n",
    )
    .unwrap();
    let by_path = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: escaped.into(),
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    let by_path = serde_json::to_value(by_path).unwrap();
    assert_eq!(by_path["total"], 0);
    let uri = by_path["uri"].as_str().unwrap();
    assert!(uri.contains("Space%20%23%20%25.lean"));
    let by_uri = query::diagnostics(
        &ws,
        &DiagnosticsParams {
            file: uri.into(),
            ..Default::default()
        },
        &crate::request::RequestContext::new(None, Default::default()).unwrap(),
    )
    .await
    .unwrap();
    let by_uri = serde_json::to_value(by_uri).unwrap();
    assert_eq!(by_uri, by_path);
    ws.shutdown().await;
    project.track_lean(None);
}
