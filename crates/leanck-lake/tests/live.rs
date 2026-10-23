//! Real Lake builds exercise the shared Lean project, ignored by default.
//!
//! ```sh
//! cargo test -p leanck-lake --test live --locked -- --ignored
//! ```
//!
//! Set `LEANCK_TEST_PROJECT` to optionally override the in-repo template.

use leanck_lake::build::{self, BuildOptions};

#[path = "../../../test-support/lean_project.rs"]
mod lean_project;
use lean_project::LeanProject;

#[tokio::test]
#[ignore]
async fn build_succeeds_for_the_template() {
    let project = LeanProject::new();
    let report = build::run(
        project.path(),
        BuildOptions {
            targets: vec!["TestProject".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(report.success, "{report:?}");
    assert!(report.errors.is_empty(), "{report:?}");
}

#[tokio::test]
#[ignore]
async fn warnings_and_strict_mode_report_sorries() {
    let project = LeanProject::new();
    let report = build::run(
        project.path(),
        BuildOptions {
            warnings: true,
            targets: vec!["TestProject.Contract.Warnings".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(report.success, "{report:?}");
    assert_eq!(report.warnings.len(), 2, "{report:?}");

    let strict = build::run(
        project.path(),
        BuildOptions {
            strict: true,
            targets: vec!["TestProject.Contract.Warnings".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!strict.success, "{strict:?}");
    assert!(!strict.warnings.is_empty(), "{strict:?}");
}

#[tokio::test]
#[ignore]
async fn failed_build_reports_the_error_location() {
    let project = LeanProject::new();
    std::fs::write(
        project.path().join("TestProject/Broken.lean"),
        "theorem broken : False := rfl\n",
    )
    .unwrap();
    let report = build::run(
        project.path(),
        BuildOptions {
            targets: vec!["TestProject.Broken".into()],
            ..Default::default()
        },
    )
    .await
    .unwrap();
    assert!(!report.success, "{report:?}");
    assert!(
        report
            .errors
            .iter()
            .any(|error| error.contains("Broken.lean:1:")),
        "{report:?}"
    );
}

#[tokio::test]
#[ignore]
async fn concurrent_builds_both_complete() {
    let project = LeanProject::new();
    let options = BuildOptions {
        targets: vec!["TestProject".into()],
        ..Default::default()
    };
    let (first, second) = tokio::join!(
        build::run(project.path(), options.clone()),
        build::run(project.path(), options),
    );
    for report in [first.unwrap(), second.unwrap()] {
        assert!(report.success, "{report:?}");
    }
}
