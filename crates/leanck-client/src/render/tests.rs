use super::*;
use leanck_core::api::*;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use std::path::Path;

fn fixture<T: DeserializeOwned>(name: &str) -> T {
    let value =
        serde_json::from_str::<Value>(include_str!("../../../../test-support/contracts/v4.json"))
            .unwrap()[name]
            .clone();
    serde_json::from_value(value).unwrap()
}

#[test]
fn goals_distinguish_no_tactic_finished_and_multiple_goals() {
    assert_eq!(
        render_goal(&fixture("goal_null")),
        "no goal here (position is not inside a tactic block)"
    );
    assert_eq!(
        render_goal(&fixture("goal_empty")),
        "goals accomplished (no open goals)"
    );
    let goals = GoalResult::Goals(Goals {
        goals: vec!["⊢ True".into(), "n : Nat\n⊢ n = n".into()],
        extensions: Default::default(),
    });
    assert_eq!(
        render_goal(&goals),
        "goal 1:\n⊢ True\n\ngoal 2:\nn : Nat\n⊢ n = n"
    );
}

#[test]
fn sorry_reports_distinguish_file_and_directory() {
    assert_eq!(
        render_sorry(&fixture("sorry_file")),
        "no sorries in /contract/Basic.lean"
    );
    assert_eq!(
        render_sorry(&fixture("sorry_directory")),
        "no sorries found\n  note: text-only directory scan; declaration diagnostics are not included"
    );
    let entry = SorryEntry {
        range: Range {
            start: Position {
                line: 2,
                character: 0,
            },
            end: Position {
                line: 2,
                character: 5,
            },
        },
        kind: SorryKind::Tactic,
    };
    let file = SorryFile {
        uri: "file:///contract/Space%20%23.lean".into(),
        sorries: vec![entry],
        notes: vec![],
    };
    assert_eq!(
        render_sorry(&SorryResult::File(file.clone())),
        "/contract/Space #.lean (1 sorries)\n  L3 [tactic]"
    );
    assert_eq!(
        render_sorry(&SorryResult::Directory(SorryDirectory {
            uri: "file:///contract".into(),
            total: 1,
            files: vec![file],
            notes: vec![],
        })),
        "1 sorries\n  /contract/Space #.lean\n    L3 [tactic]"
    );
    assert_eq!(
        render_sorry(&SorryResult::Directory(SorryDirectory {
            uri: "file:///contract".into(),
            total: 0,
            files: vec![SorryFile {
                uri: "file:///contract/Flag.lean".into(),
                sorries: vec![],
                notes: vec!["red flag".into()],
            }],
            notes: vec![],
        })),
        "0 sorries\n  /contract/Flag.lean\n    note: red flag"
    );
}

#[test]
fn list_and_restart_keep_compact_output() {
    assert_eq!(
        render_list(&fixture("list")),
        "1 declarations\n  L13    theorem    hello_world"
    );
    assert_eq!(
        render_list(&ListResult {
            uri: "file:///contract".into(),
            symbols: vec![],
            notes: vec![],
        }),
        "no declarations"
    );
    assert_eq!(
        render_restart(&fixture("restart")),
        "restarted /contract/Basic.lean"
    );
}

#[test]
fn health_report_preserves_json_and_minimal_text() {
    let report = HealthReport::from_result(Ok(fixture("health")), Path::new("/tmp/contract.sock"));
    assert_eq!(format_health_human(&report), "ok");
    assert!(report.issues.is_empty());
    let mut health: Value = fixture("health");
    health["socket_path"] = json!("/tmp/contract.sock");
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        json!({"ok":true,"lsp":health,"issues":[]})
    );
}

#[test]
fn health_reports_failures_and_typed_issues() {
    let mut report = HealthReport::from_result(
        Err(crate::ClientError::StartupTimeout),
        Path::new("/tmp/contract.sock"),
    );
    assert!(!report.ok);
    assert_eq!(
        format_health_human(&report),
        "error: lsp: leanckd did not start within the timeout"
    );
    report.ok = true;
    report.issues = vec![
        HealthIssue {
            level: HealthIssueLevel::Warning,
            message: "lsp: daemon protocol mismatch".into(),
        },
        HealthIssue {
            level: HealthIssueLevel::Warning,
            message: "lsp: not reachable".into(),
        },
    ];
    assert_eq!(
        format_health_human(&report),
        "warning: lsp: daemon protocol mismatch\nwarning: lsp: not reachable"
    );
    report.issues.clear();
    report.ok = false;
    assert_eq!(format_health_human(&report), "error: health checks failed");
    let mut health: HealthResult = fixture("health");
    health.ok = false;
    let report = HealthReport::from_result(Ok(health), Path::new("/tmp/contract.sock"));
    assert_eq!(format_health_human(&report), "error: lsp: not reachable");
}

#[test]
fn diagnostics_keep_source_excerpts_pagination_and_notes() {
    let mut body: DiagnosticsResult = fixture("clean_diagnostics");
    body.source = "theorem example : True := by\n  exact missing\n".into();
    body.total = 3;
    body.offset = Some(1);
    body.notes = vec!["ok".into(), "contract note".into()];
    body.diagnostics = vec![
        serde_json::from_value(json!({
            "range":{"start":{"line":1,"character":2},"end":{"line":1,"character":15}},
            "severity":2,"message":"contract warning"
        }))
        .unwrap(),
    ];
    assert_eq!(
        format_diagnostics_human(&body, "Basic.lean"),
        "  Basic.lean\n  L2:2 [warning] (at exact missing):\n    contract warning\n\n  ... and 1 more diagnostic; use offset=2\n\n  note: contract note"
    );
    body.offset = Some(u64::MAX);
    assert!(!format_diagnostics_human(&body, "Basic.lean").contains("more diagnostic"));
}

#[test]
fn diagnostics_render_the_pinned_source_after_disk_changes() {
    let scratch = crate::tests::Scratch::new();
    let path = scratch.0.join("Basic.lean");
    std::fs::write(&path, "changed disk source\n").unwrap();
    let mut body: DiagnosticsResult = fixture("clean_diagnostics");
    body.uri = leanck_core::document::path_to_uri(&path).unwrap();
    body.source = "pinned source\n".into();
    body.notes.clear();
    body.total = 1;
    body.diagnostics=vec![serde_json::from_value(json!({"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":6}},"severity":1,"message":"error"})).unwrap()];
    let text = format_diagnostics_human(&body, "Basic.lean");
    assert!(text.contains("at pinned source"));
    assert!(!text.contains("changed disk"));
    let mut empty_source = body.clone();
    empty_source.source.clear();
    assert!(!format_diagnostics_human(&empty_source, "Basic.lean").contains("(at "));
    std::fs::remove_file(path).unwrap();
    assert_eq!(format_diagnostics_human(&body, "Basic.lean"), text);
}
