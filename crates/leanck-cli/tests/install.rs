//! `leanck _install` end to end: a dry run against an unpacked release
//! directory prints the plan, writes nothing, and reports JSON on stdout; an
//! approved install applies it, and `_uninstall` removes it again.
use std::{
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::{Command, Output},
};

struct Case {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl Case {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let release = root.join("release/leanck");
        let plugin = release.join("plugin");
        for sub in [
            ".claude-plugin",
            ".codex-plugin",
            "bin",
            "agents",
            "codex/agents",
        ] {
            fs::create_dir_all(plugin.join(sub)).unwrap();
        }
        fs::write(release.join(".leanck-release"), "v1.2.3\n").unwrap();
        fs::write(release.join(".leanck-update-protocol"), "3\n").unwrap();
        fs::create_dir_all(release.join("docs")).unwrap();
        fs::write(release.join("docs/leanck.example.toml"), "# example\n").unwrap();
        fs::write(plugin.join(".claude-plugin/plugin.json"), "{}").unwrap();
        fs::write(plugin.join(".codex-plugin/plugin.json"), "{}").unwrap();
        fs::copy(env!("CARGO_BIN_EXE_leanck"), plugin.join("bin/leanck")).unwrap();
        fs::set_permissions(plugin.join("bin/leanck"), fs::Permissions::from_mode(0o755)).unwrap();
        for skill in ["leanck", "lean-proving"] {
            fs::create_dir_all(plugin.join("skills").join(skill)).unwrap();
            fs::write(plugin.join("skills").join(skill).join("SKILL.md"), skill).unwrap();
        }
        fs::write(plugin.join("agents/lean-expert.md"), "agent").unwrap();
        fs::write(plugin.join("codex/agents/lean-expert.toml"), "agent").unwrap();
        fs::create_dir_all(root.join("home")).unwrap();
        Self { _dir: dir, root }
    }
    fn home(&self) -> PathBuf {
        self.root.join("home")
    }
    fn run(&self, args: &[&str]) -> Output {
        Command::new(env!("CARGO_BIN_EXE_leanck"))
            .arg("_install")
            .arg("--release-dir")
            .arg(self.root.join("release/leanck"))
            .args(args)
            .env_clear()
            .env("HOME", self.home())
            .env("PATH", "/usr/bin:/bin")
            .env("SHELL", "/bin/zsh")
            .env("TERM", "dumb")
            .current_dir(&self.root)
            .output()
            .unwrap()
    }
}

fn tree(path: &Path) -> Vec<PathBuf> {
    let mut paths = vec![];
    for entry in fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            paths.extend(tree(&path));
        }
        paths.push(path);
    }
    paths
}

#[test]
fn dry_run_prints_the_plan_and_changes_nothing() {
    let case = Case::new();
    let output = case.run(&["--dry-run", "--standalone", "--skills", "claude"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(output.stdout.is_empty());
    let home = case.home();
    for expected in [
        "Welcome to the leanck v1.2.3 installer".to_owned(),
        "Detected coding agents: none".to_owned(),
        "- Selected skills: all".to_owned(),
        format!(
            "Would install {} (v1.2.3)",
            home.join(".local/bin/leanck").display()
        ),
        format!(
            "Would install {}",
            home.join(".claude/skills/leanck").display()
        ),
        format!(
            "Installer choices and replacement backups: {}",
            home.join(".local/state/leanck/installer").display()
        ),
        "[WARN] leanck will not be added to PATH".to_owned(),
        "[INFO] Dry run complete".to_owned(),
    ] {
        assert!(
            stderr.contains(&expected),
            "missing {expected:?} in:\n{stderr}"
        );
    }
    assert!(!stderr.contains('\x1b'));
    assert!(tree(&home).is_empty(), "{:?}", tree(&home));
}

#[test]
fn json_reports_preview_conflicts_and_parse_errors() {
    let case = Case::new();
    let output = case.run(&[
        "--dry-run",
        "--json",
        "--plugin",
        "none",
        "--skills",
        "claude",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "preview");
    assert_eq!(json["version"], "v1.2.3");
    assert!(json["actions"].as_array().unwrap().len() > 3);

    let skill = case.home().join(".claude/skills/leanck");
    fs::create_dir_all(&skill).unwrap();
    let output = case.run(&["--dry-run", "--json", "--standalone", "--skills", "claude"]);
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "failed");
    assert_eq!(
        json["message"],
        format!(
            "{} already exists or has local edits; use --force to back up and replace it, or rerun interactively",
            skill.display()
        )
    );
    let output = case.run(&[
        "--dry-run",
        "--json",
        "--standalone",
        "--skills",
        "claude",
        "--force",
    ]);
    assert!(output.status.success());

    let output = case.run(&["--json", "--wat"]);
    assert_eq!(output.status.code(), Some(1));
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["message"], "unknown option: --wat; see --help");
    assert!(String::from_utf8_lossy(&output.stderr).contains(" [ERR]: unknown option: --wat"));
}

/// The staged release executable is a script, so neither installing nor
/// uninstalling can reach the developer's real daemons.
fn fake_release_binary(case: &Case) -> PathBuf {
    let log = case.root.join("binary-executions");
    let script = format!(
        r#"#!/bin/sh
printf 'leanck %s\n' "$*" >> '{}'
case "$1" in
  --version) echo 'leanck 1.2.3' ;;
  daemons) [ "$2" = list ] && echo '{{"incompatible": 0}}' ;;
  _completions) printf '# completions for %s\n' "$2" ;;
esac
exit 0
"#,
        log.display()
    );
    let binary = case.root.join("release/leanck/plugin/bin/leanck");
    fs::write(&binary, script).unwrap();
    fs::set_permissions(&binary, fs::Permissions::from_mode(0o755)).unwrap();
    log
}

#[test]
fn approval_is_required_then_installs_and_uninstalls() {
    let case = Case::new();
    let output = case.run(&["--standalone"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&output.stderr).contains("approval is required"));
    assert!(tree(&case.home()).is_empty());
    let output = case.run(&["--help"]);
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("--release-dir"));

    let log = fake_release_binary(&case);
    let output = case.run(&["--standalone", "--yes", "--skills", "claude", "--json"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    let json: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["status"], "installed", "{stderr}");
    let home = case.home();
    let binary = home.join(".local/bin/leanck");
    assert!(binary.is_file());
    assert!(home.join(".claude/skills/leanck/SKILL.md").is_file());
    let state = home.join(".local/state/leanck/installer");
    for name in [
        "selections.tsv",
        "uninstall.tsv",
        "sources.tsv",
        "receipt.json",
    ] {
        assert!(state.join(name).is_file(), "{name}");
    }
    let receipt: serde_json::Value =
        serde_json::from_slice(&fs::read(state.join("receipt.json")).unwrap()).unwrap();
    assert_eq!(receipt["version"], "1.2.3");
    assert!(
        fs::read_to_string(&log)
            .unwrap()
            .contains("daemons list --verbose --json")
    );

    let uninstall = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_leanck"))
            .arg("_uninstall")
            .args(args)
            .env_clear()
            .env("HOME", case.home())
            .env("PATH", "/usr/bin:/bin")
            .current_dir(&case.root)
            .output()
            .unwrap()
    };
    let output = uninstall(&["--dry-run"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains(&format!("Would remove file: {}", binary.display())));
    assert!(binary.is_file());
    let output = uninstall(&["--timeout", "0"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr)
            .contains("leanck-uninstall: --timeout expects a positive integer")
    );
    let output = uninstall(&["--yes"]);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert!(stderr.contains("Uninstalled leanck."));
    assert!(
        fs::read_to_string(&log)
            .unwrap()
            .contains("daemons stop --all")
    );
    assert!(!binary.exists());
    assert!(!home.join(".claude/skills/leanck").exists());
    assert!(!state.join("receipt.json").exists());
    let output = uninstall(&["--yes"]);
    assert!(String::from_utf8_lossy(&output.stderr).contains("nothing to remove"));
}
