#[path = "../../../test-support/fake_lake.rs"]
mod fake_lake;
#[path = "../../../test-support/mcp.rs"]
mod mcp;

use fake_lake::Project;
use serde_json::json;

#[tokio::test]
async fn mcp_uses_selected_workspace_configuration_and_keeps_stdout_clean() {
    let cwd = Project::new("exit 99");
    let selected = Project::new("exit 99");
    let other = Project::new("exit 99");
    std::fs::write(cwd.0.join("leanck.toml"), "malformed = [").unwrap();
    std::fs::write(
        selected.0.join("leanck.toml"),
        "[output]\njson=true\n[diagnostics]\nlimit=17\n",
    )
    .unwrap();
    std::fs::write(other.0.join("leanck.toml"), "[diagnostics]\nlimit=29\n").unwrap();
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_leanck"));
    command
        .current_dir(&cwd.0)
        .env("XDG_CONFIG_HOME", cwd.0.join("empty-config"))
        .env("XDG_CACHE_HOME", cwd.0.join("cache"))
        .env_remove("LEANCK_CONFIG")
        .args([
            "--set",
            "cache.enabled=false",
            "serve",
            "mcp",
            "--workspace",
        ])
        .arg(&selected.0);
    let mut mcp = mcp::Mcp::start(command).await;
    let config = mcp.tool("configuration", json!({})).await;
    assert_eq!(config["workspace"], selected.0.to_str().unwrap());
    assert_eq!(config["settings"]["diagnostics"]["limit"], 17);
    assert_eq!(config["settings"]["output"]["json"], true);
    assert_eq!(config["settings"]["cache"]["enabled"], false);
    assert!(config["settings"]["client"]["daemon_bin"].is_null());
    mcp.tool("set_workspace", json!({"path": other.0})).await;
    let config = mcp.tool("configuration", json!({})).await;
    assert_eq!(config["workspace"], other.0.to_str().unwrap());
    assert_eq!(config["settings"]["diagnostics"]["limit"], 29);
    assert_eq!(config["settings"]["cache"]["enabled"], false);
    mcp.close().await;
    selected.assert_no_daemon();
    other.assert_no_daemon();
}

#[test]
fn server_help_and_arguments_are_available_without_workspace_or_configuration() {
    let cwd = Project::new("exit 99");
    std::fs::write(cwd.0.join("leanck.toml"), "malformed = [").unwrap();
    for args in [
        vec!["serve", "--help"],
        vec!["serve", "mcp", "--help"],
        vec!["serve", "daemon", "--help"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_leanck"))
            .current_dir(&cwd.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
    for args in [
        vec!["serve"],
        vec!["serve", "daemon"],
        vec!["serve", "mcp", "--json"],
    ] {
        let output = std::process::Command::new(env!("CARGO_BIN_EXE_leanck"))
            .current_dir(&cwd.0)
            .args(args)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}
