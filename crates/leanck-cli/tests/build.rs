#[path = "../../../test-support/fake_lake.rs"]
mod fake_lake;
use fake_lake::{Project, alive, wait_file};
use std::{process::Stdio, time::Duration};
fn cli(project: &Project) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_leanck"));
    command
        .args(["build", "--json", "--workspace"])
        .arg(&project.0)
        .env("PATH", project.path_env())
        .arg("--set")
        .arg(format!(
            "client.daemon_bin={}",
            serde_json::to_string(&project.0.join("missing-daemon")).unwrap()
        ))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    command
}
#[tokio::test]
async fn sigint_stops_and_reaps_the_build_and_returns_structured_cancellation() {
    let project = Project::new("printf '%s' \"$$\" > lake.pid\nexec sleep 30");
    let child = cli(&project).spawn().unwrap();
    wait_file(&project.0.join("lake.pid")).await;
    assert!(
        std::process::Command::new("kill")
            .args(["-INT", &child.id().unwrap().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let output = tokio::time::timeout(Duration::from_secs(10), child.wait_with_output())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.status.code(), Some(130));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["kind"], "cancelled");
    assert!(!alive(
        &std::fs::read_to_string(project.0.join("lake.pid")).unwrap()
    ));
    project.assert_no_daemon();
}
#[tokio::test]
async fn timeout_errors_are_json_and_do_not_start_lake() {
    let project = Project::new("touch should-not-start");
    let output = cli(&project)
        .args(["--timeout", "0"])
        .output()
        .await
        .unwrap();
    assert!(!output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["error"]["kind"], "timeout");
    assert!(!project.0.join("should-not-start").exists());
    project.assert_no_daemon();
}
#[tokio::test]
async fn builds_preserve_targets_policy_exit_codes_provenance_and_standalone_ownership() {
    let project =
        Project::new("printf '%s\\n' \"$@\" > args\nprintf 'warning: configuration warning\\n'");
    let output = cli(&project)
        .args(["--strict", "Target:olean"])
        .output()
        .await
        .unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["success"], false);
    assert_eq!(report["warnings"][0], "warning: configuration warning");
    assert_eq!(report["provenance"]["exit_code"], 0);
    assert_eq!(
        report["provenance"]["execution"]["args"],
        serde_json::json!(["build", "Target:olean"])
    );
    assert_eq!(
        std::fs::read_to_string(project.0.join("args")).unwrap(),
        "build\nTarget:olean\n"
    );
    project.assert_no_daemon();
    let output = cli(&project).output().await.unwrap();
    assert!(output.status.success());
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert!(report["warnings"].as_array().unwrap().is_empty());
    assert_eq!(report["provenance"]["strict"], false);
}

#[tokio::test]
async fn configured_build_policy_and_targets_can_be_overridden() {
    let project = Project::new("printf 'warning: configuration warning\\n'");
    std::fs::write(
        project.0.join("leanck.toml"),
        "[build]\nstrict=true\ntargets=['Configured']\ntimeout_ms=10000\nmax_output_bytes=4096\n",
    )
    .unwrap();
    let output = cli(&project).output().await.unwrap();
    assert_eq!(output.status.code(), Some(1));
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["provenance"]["strict"], true);
    assert_eq!(
        report["provenance"]["execution"]["args"],
        serde_json::json!(["build", "Configured"])
    );
    let output = cli(&project)
        .args(["--set", "build.strict=false", "Explicit"])
        .output()
        .await
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report["provenance"]["strict"], false);
    assert_eq!(
        report["provenance"]["execution"]["args"],
        serde_json::json!(["build", "Explicit"])
    );
}
