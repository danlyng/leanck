use leanck_lake::{ExecutionOptionsError, LakeError, LakeErrorKind, OutputStream};
use serde_json::Value;
fn fixture(name: &str) -> Value {
    serde_json::from_str::<Value>(include_str!("../../../test-support/contracts/v4.json")).unwrap()
        [name]
        .clone()
}

#[test]
fn build_report_json_contract() {
    let report = leanck_lake::build::BuildReport {
        success: false,
        errors: vec!["error: TestProject/Basic.lean:1:0: contract error".into()],
        warnings: vec![],
        provenance: None,
    };
    assert_eq!(serde_json::to_value(&report).unwrap(), fixture("build"));
    assert!(leanck_lake::build::format_human(&report, false).contains("contract error"));
}

#[test]
fn error_categories_preserve_json_names_and_map_every_variant() {
    let io_error = || std::io::Error::other("test failure");
    let cases = [
        (LakeError::TimedOut, LakeErrorKind::Timeout, "timeout"),
        (LakeError::Cancelled, LakeErrorKind::Cancelled, "cancelled"),
        (
            LakeError::OutputLimit {
                stream: OutputStream::Stdout,
                limit: 1024,
            },
            LakeErrorKind::OutputLimit,
            "output_limit",
        ),
        (
            LakeError::InvalidOptions(ExecutionOptionsError::ZeroOutputLimit),
            LakeErrorKind::InvalidOptions,
            "invalid_options",
        ),
        (
            LakeError::InvalidOptions(ExecutionOptionsError::TimeoutTooLarge),
            LakeErrorKind::InvalidOptions,
            "invalid_options",
        ),
        (
            LakeError::Coordination {
                path: "lock".into(),
                source: io_error(),
            },
            LakeErrorKind::Coordination,
            "coordination",
        ),
        (
            LakeError::Project {
                path: "project".into(),
                source: io_error(),
            },
            LakeErrorKind::InvalidWorkspace,
            "invalid_workspace",
        ),
        (
            LakeError::Spawn {
                executable: "lake".into(),
                project: "project".into(),
                source: io_error(),
            },
            LakeErrorKind::Spawn,
            "spawn",
        ),
        (
            LakeError::Output(io_error()),
            LakeErrorKind::Internal,
            "internal",
        ),
        (
            LakeError::WorkerStopped,
            LakeErrorKind::Internal,
            "internal",
        ),
    ];
    for (error, kind, name) in cases {
        assert_eq!(error.kind(), kind);
        let json = serde_json::to_value(error.kind()).unwrap();
        assert_eq!(json, name);
        assert_eq!(serde_json::from_value::<LakeErrorKind>(json).unwrap(), kind);
    }
}

#[tokio::test]
async fn invalid_execution_reasons_are_typed_before_workspace_access_or_spawn() {
    use leanck_lake::{ExecutionOptions, Lake};
    use std::{path::Path, time::Duration};

    let lake = Lake::new("/nonexistent/leanck-lake-test-executable");
    let project = Path::new("/nonexistent/leanck-lake-test-workspace");
    for (options, expected) in [
        (
            ExecutionOptions {
                max_output_bytes: 0,
                ..Default::default()
            },
            ExecutionOptionsError::ZeroOutputLimit,
        ),
        (
            ExecutionOptions {
                timeout: Duration::MAX,
                ..Default::default()
            },
            ExecutionOptionsError::TimeoutTooLarge,
        ),
    ] {
        let error = lake.run_with(project, &[], options).await.unwrap_err();
        let LakeError::InvalidOptions(reason) = error else {
            panic!("expected invalid options, got {error:?}");
        };
        assert_eq!(reason, expected);
    }
}
