#[path = "../../../test-support/fake_lake.rs"]
mod fake_lake;

use fake_lake::Project;
use std::process::Command;

#[test]
fn scripts_cover_public_commands_without_loading_config_or_starting_a_daemon() {
    let project = Project::new("exit 99");
    std::fs::write(project.0.join("leanck.toml"), "malformed = [").unwrap();
    for shell in ["fish", "bash", "zsh", "powershell", "elvish"] {
        let output = Command::new(env!("CARGO_BIN_EXE_leanck"))
            .current_dir(&project.0)
            .env("XDG_CONFIG_HOME", project.0.join("config"))
            .env("XDG_CACHE_HOME", project.0.join("cache"))
            .args(["--config", "missing.toml", "_completions", shell])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "{shell}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let script = String::from_utf8(output.stdout).unwrap();
        assert!(
            !script.contains('\u{1b}'),
            "{shell}: ANSI escapes in script"
        );
        for public in [
            "diagnostics",
            "update",
            "normalize",
            "daemons",
            "restart-file",
            "workspace",
        ] {
            assert!(script.contains(public), "{shell}: missing {public}");
        }
        assert!(
            !script.contains("_completions"),
            "{shell}: generator advertises itself"
        );
        assert!(
            !script
                .split(|c: char| !c.is_ascii_alphanumeric() && c != '_' && c != '-')
                .any(|word| word == "serve" || word.contains("__serve")),
            "{shell}: hidden server mode in script"
        );
    }
    project.assert_no_daemon();
    assert!(!project.0.join("cache").exists());
}
