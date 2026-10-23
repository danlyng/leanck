use std::{
    fs,
    io::Write,
    process::{Command, Output, Stdio},
};

use serde_json::{Value, json};
use tempfile::TempDir;

struct Project(TempDir);

impl Project {
    fn new(toolchain: &str, enabled: bool) -> Self {
        let project = Self(tempfile::tempdir().unwrap());
        fs::write(project.0.path().join("lean-toolchain"), toolchain).unwrap();
        fs::write(project.0.path().join("X.lean"), "push_neg at h").unwrap();
        if enabled {
            fs::write(
                project.0.path().join("leanck.toml"),
                "[hooks]\npush_not = true\n",
            )
            .unwrap();
        }
        project
    }

    fn run(&self, payload: &str) -> Output {
        let mut child = Command::new(env!("CARGO_BIN_EXE_leanck"))
            .args(["_hook", "post-edit"])
            .current_dir(self.0.path())
            .env("HOME", self.0.path())
            .env("XDG_CONFIG_HOME", self.0.path())
            .env_remove("LEANCK_CONFIG")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(payload.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert_eq!(output.status.code(), Some(0));
        assert!(output.stderr.is_empty());
        output
    }

    fn edit(&self) -> Output {
        self.run(&json!({"tool_input": {"file_path": "X.lean"}}).to_string())
    }

    fn source(&self) -> String {
        fs::read_to_string(self.0.path().join("X.lean")).unwrap()
    }
}

#[test]
fn disabled_by_default() {
    let project = Project::new("leanprover/lean4:v4.29.0", false);
    assert!(project.edit().stdout.is_empty());
    assert_eq!(project.source(), "push_neg at h");
}

#[test]
fn enabled_hook_rewrites_and_reports_context() {
    let project = Project::new("leanprover/lean4:v4.29.0", true);
    let output = project.edit();
    assert_eq!(project.source(), "push Not at h");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(
        report,
        json!({
            "hookSpecificOutput": {
                "hookEventName": "PostToolUse",
                "additionalContext": "Auto-rewrote 1 deprecated `push_neg` occurrence to `push Not` in X.lean. Re-read the file before making further edits to it."
            }
        })
    );
}

#[test]
fn release_candidate_does_not_rewrite() {
    let project = Project::new("leanprover/lean4:v4.29.0-rc8", true);
    assert!(project.edit().stdout.is_empty());
    assert_eq!(project.source(), "push_neg at h");
}

#[test]
fn invalid_json_is_silent_and_successful() {
    let project = Project::new("leanprover/lean4:v4.29.0", true);
    assert!(project.run("{invalid json").stdout.is_empty());
    assert_eq!(project.source(), "push_neg at h");
}
