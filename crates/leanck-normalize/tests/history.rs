//! Commits are created only inside disposable fixture repositories.
use std::{
    fs,
    path::PathBuf,
    process::Command,
    sync::atomic::{AtomicU32, Ordering},
};
static NEXT: AtomicU32 = AtomicU32::new(0);
struct Repo(PathBuf);
impl Repo {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "leanck-history-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        let repo = Self(path);
        repo.git(&["init", "-q"]);
        repo
    }
    fn git(&self, args: &[&str]) {
        let out = Command::new("git")
            .current_dir(&self.0)
            .args(args)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn commit(&self, name: &str, email: &str, date: &str) {
        self.git(&["add", "."]);
        let out = Command::new("git")
            .current_dir(&self.0)
            .args([
                "-c",
                "core.hooksPath=/dev/null",
                "-c",
                "commit.gpgsign=false",
                "-c",
                "user.name=Fixture Committer",
                "-c",
                "user.email=fixture@example.invalid",
                "commit",
                "-q",
                "-m",
                "fixture",
            ])
            .env("GIT_AUTHOR_NAME", name)
            .env("GIT_AUTHOR_EMAIL", email)
            .env("GIT_AUTHOR_DATE", date)
            .env("GIT_COMMITTER_DATE", date)
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
    }
    fn normalize(&self, file: &str) -> std::process::Output {
        Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
            .current_dir(&self.0)
            .args(["--no-config", file])
            .output()
            .unwrap()
    }
}
impl Drop for Repo {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
#[test]
fn missing_header_uses_renamed_file_history_and_mailmap() {
    let repo = Repo::new();
    fs::write(repo.0.join("Old.lean"), "module\nimport Foo\n").unwrap();
    repo.commit(
        "Alias",
        "alias@example.invalid",
        "2020-01-02T12:00:00+00:00",
    );
    repo.git(&["mv", "Old.lean", "Renamed.lean"]);
    repo.commit(
        "Editor",
        "editor@example.invalid",
        "2022-03-04T12:00:00+00:00",
    );
    fs::write(
        repo.0.join("Renamed.lean"),
        "module\nimport Foo\nimport Bar\n",
    )
    .unwrap();
    repo.commit(
        "Alias",
        "alias@example.invalid",
        "2024-05-06T12:00:00+00:00",
    );
    fs::write(
        repo.0.join(".mailmap"),
        "Canonical Person <canonical@example.invalid> Alias <alias@example.invalid>\n",
    )
    .unwrap();
    let output = repo.normalize("Renamed.lean");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let source = fs::read_to_string(repo.0.join("Renamed.lean")).unwrap();
    assert!(
        source.contains("Copyright (c) 2020–2024 Canonical Person, Editor."),
        "{source}\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(source.matches("Copyright (c)").count(), 1);
    assert!(!source.contains("Alias") && !source.contains("Daniel") && !source.contains("Apache"));
    assert!(repo.normalize("Renamed.lean").status.success());
    assert_eq!(
        fs::read_to_string(repo.0.join("Renamed.lean")).unwrap(),
        source
    );
}

#[test]
fn hidden_config_maps_mailmapped_names_in_generated_and_replaced_headers() {
    let repo = Repo::new();
    fs::write(repo.0.join("Main.lean"), "def x := 1\n").unwrap();
    repo.commit(
        "Alias",
        "alias@example.invalid",
        "2020-01-02T12:00:00+00:00",
    );
    fs::write(
        repo.0.join(".mailmap"),
        "Canonical Person <canonical@example.invalid> Alias <alias@example.invalid>\n",
    )
    .unwrap();
    fs::write(repo.0.join(".leanck.toml"), "[normalize.rules.frontmatter]\ninvalid_existing='replace'\n[normalize.rules.frontmatter.author_map]\nAlias='Wrong: raw identity'\n'Canonical Person'='Display Person'\n").unwrap();
    fs::create_dir_all(repo.0.join("config-home")).unwrap();
    let normalize = || {
        Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
            .current_dir(&repo.0)
            .env("XDG_CONFIG_HOME", repo.0.join("config-home"))
            .env_remove("LEANCK_CONFIG")
            .arg("Main.lean")
            .output()
            .unwrap()
    };
    for input in ["def x := 1\n", "/- Old attribution -/\ndef x := 1\n"] {
        fs::write(repo.0.join("Main.lean"), input).unwrap();
        let output = normalize();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let source = fs::read_to_string(repo.0.join("Main.lean")).unwrap();
        assert!(
            source.contains("Copyright (c) 2020 Display Person."),
            "{source}"
        );
        assert!(source.contains("Authors: Display Person"), "{source}");
        assert!(
            !source.contains("Canonical")
                && !source.contains("Wrong")
                && !source.contains("Old attribution")
        );
        assert!(normalize().status.success());
        assert_eq!(
            fs::read_to_string(repo.0.join("Main.lean")).unwrap(),
            source
        );
    }
}
#[test]
fn existing_header_needs_no_git_and_untracked_files_get_no_invented_identity() {
    let repo = Repo::new();
    fs::write(repo.0.join("Existing.lean"),"/-\n Copyright (c) 1998 Original Author.\n Authors: Original Author\n Custom license text.\n-/\nmodule\nimport Foo\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
        .current_dir(&repo.0)
        .env("PATH", "/no/git/here")
        .args(["--no-config", "Existing.lean"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("existing header does not match"));
    assert!(!String::from_utf8_lossy(&output.stderr).contains("Git history"));
    let source = fs::read_to_string(repo.0.join("Existing.lean")).unwrap();
    assert!(source.contains("1998 Original Author.") && source.contains("Custom license text."));
    fs::write(repo.0.join("New.lean"), "module\nimport Foo\n").unwrap();
    let output = repo.normalize("New.lean");
    assert!(output.status.success());
    assert!(
        !fs::read_to_string(repo.0.join("New.lean"))
            .unwrap()
            .contains("Copyright")
    );
    assert!(String::from_utf8_lossy(&output.stderr).contains("left absent"));
}

#[test]
fn tracked_declaration_only_files_get_headers_and_dry_run_is_non_writing() {
    let repo = Repo::new();
    fs::write(repo.0.join("Basic.lean"), "def answer := 42\n").unwrap();
    repo.commit(
        "History Author",
        "author@example.invalid",
        "2017-01-01T12:00:00+00:00",
    );
    let output = Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
        .current_dir(&repo.0)
        .args(["--no-config", "--dry-run", "Basic.lean"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(repo.0.join("Basic.lean")).unwrap(),
        "def answer := 42\n"
    );
    assert!(repo.normalize("Basic.lean").status.success());
    assert!(
        fs::read_to_string(repo.0.join("Basic.lean"))
            .unwrap()
            .contains("Copyright (c) 2017 History Author.")
    );
}
#[test]
fn shallow_history_is_reported_without_inventing_complete_attribution() {
    let repo = Repo::new();
    fs::write(repo.0.join("Basic.lean"), "module\nimport Foo\n").unwrap();
    repo.commit(
        "History Author",
        "author@example.invalid",
        "2017-01-01T12:00:00+00:00",
    );
    let head = Command::new("git")
        .current_dir(&repo.0)
        .args(["rev-parse", "HEAD"])
        .output()
        .unwrap();
    fs::write(repo.0.join(".git/shallow"), head.stdout).unwrap();
    let output = repo.normalize("Basic.lean");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("shallow"));
    assert!(
        !fs::read_to_string(repo.0.join("Basic.lean"))
            .unwrap()
            .contains("Copyright")
    );
}

#[cfg(unix)]
#[test]
fn history_timeout_is_bounded_and_keeps_missing_header_absent() {
    use std::os::unix::fs::PermissionsExt;
    let repo = Repo::new();
    fs::write(repo.0.join("Basic.lean"), "module\nimport Foo\n").unwrap();
    fs::create_dir(repo.0.join("fake-bin")).unwrap();
    let fake = repo.0.join("fake-bin/git");
    fs::write(&fake, "#!/bin/sh\nexec /bin/sleep 2\n").unwrap();
    fs::set_permissions(&fake, fs::Permissions::from_mode(0o700)).unwrap();
    let start = std::time::Instant::now();
    let output = Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
        .current_dir(&repo.0)
        .env("PATH", repo.0.join("fake-bin"))
        .args([
            "--no-config",
            "--set",
            "normalize.rules.frontmatter.history_timeout_ms=30",
            "Basic.lean",
        ])
        .output()
        .unwrap();
    assert!(start.elapsed() < std::time::Duration::from_secs(1));
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("timed out"));
    assert!(
        !fs::read_to_string(repo.0.join("Basic.lean"))
            .unwrap()
            .contains("Copyright")
    );
}

#[test]
fn ai_authors_are_excluded_with_their_years_and_can_be_restored() {
    let repo = Repo::new();
    fs::write(repo.0.join("Basic.lean"), "def x := 1\n").unwrap();
    repo.commit(
        "Claude Opus",
        "noreply@anthropic.com",
        "2015-01-01T12:00:00+00:00",
    );
    fs::write(repo.0.join("Basic.lean"), "def x := 2\n").unwrap();
    repo.commit(
        "Human",
        "human@example.invalid",
        "2020-01-01T12:00:00+00:00",
    );
    // A mailmap must not hide the ignored raw identity.
    fs::write(
        repo.0.join(".mailmap"),
        "Disguised Agent <alias@example.invalid> Claude Opus <noreply@anthropic.com>\n",
    )
    .unwrap();
    let output = repo.normalize("Basic.lean");
    assert!(output.status.success());
    let source = fs::read_to_string(repo.0.join("Basic.lean")).unwrap();
    assert!(source.contains("Copyright (c) 2020 Human."), "{source}");
    assert!(!source.contains("2015") && !source.contains("Agent") && !source.contains("Claude"));
    fs::write(repo.0.join("Basic.lean"), "def x := 2\n").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
        .current_dir(&repo.0)
        .args([
            "--no-config",
            "--set",
            "normalize.rules.frontmatter.ignore_authors=[]",
            "Basic.lean",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let source = fs::read_to_string(repo.0.join("Basic.lean")).unwrap();
    assert!(
        source.contains("Copyright (c) 2015–2020 Disguised Agent, Human."),
        "{source}"
    );
}

#[test]
fn fully_excluded_history_leaves_the_header_absent_with_a_note() {
    let repo = Repo::new();
    fs::write(repo.0.join("Basic.lean"), "def x := 1\n").unwrap();
    repo.commit("Codex", "noreply@openai.com", "2026-01-01T12:00:00+00:00");
    let output = repo.normalize("Basic.lean");
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("all Git authors"));
    assert!(
        !fs::read_to_string(repo.0.join("Basic.lean"))
            .unwrap()
            .contains("Copyright")
    );
}

#[test]
fn header_mismatch_is_reported_in_dry_run_mode_without_changing_the_file() {
    let repo = Repo::new();
    let source = "/- Existing attribution. -/\n\nmodule\n";
    fs::write(repo.0.join("Basic.lean"), source).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
        .current_dir(&repo.0)
        .env("PATH", "/no/git/here")
        .args(["--no-config", "--dry-run", "Basic.lean"])
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Basic.lean:1: existing header does not match"),
        "{stderr}"
    );
    assert!(!stderr.contains("Git history"));
    assert_eq!(
        fs::read_to_string(repo.0.join("Basic.lean")).unwrap(),
        source
    );
}

#[test]
fn agent_committers_and_coauthor_trailers_do_not_remove_human_authorship() {
    let repo = Repo::new();
    fs::write(repo.0.join("Basic.lean"), "def x := 1\n").unwrap();
    repo.commit(
        "Human (aider)",
        "human@example.invalid",
        "2020-01-01T12:00:00+00:00",
    );
    repo.git(&[
        "-c",
        "core.hooksPath=/dev/null",
        "-c",
        "commit.gpgsign=false",
        "-c",
        "user.name=Codex",
        "-c",
        "user.email=noreply@openai.com",
        "commit",
        "--amend",
        "-q",
        "-m",
        "Human change\n\nCo-authored-by: Claude Opus <noreply@anthropic.com>",
    ]);
    let output = repo.normalize("Basic.lean");
    assert!(output.status.success());
    let source = fs::read_to_string(repo.0.join("Basic.lean")).unwrap();
    assert!(
        source.contains("Copyright (c) 2020 Human (aider)."),
        "{source}"
    );
    assert!(!source.contains("Codex") && !source.contains("Claude"));
}

fn header_policy_command(repo: &Repo, policy: &str) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_leanck-normalize"));
    command
        .current_dir(&repo.0)
        .args(["--no-config", "--set"])
        .arg(format!(
            "normalize.rules.frontmatter.invalid_existing='{policy}'"
        ))
        .args([
            "--set",
            "normalize.rules.frontmatter.normalize_existing=false",
            "Basic.lean",
        ]);
    command
}

#[test]
fn replacing_invalid_headers_uses_filtered_history_and_dry_run_does_not_write() {
    let repo = Repo::new();
    let source = "/- Old attribution and license. -/\n\nmodule\n\ndef x := 1\n";
    fs::write(repo.0.join("Basic.lean"), source).unwrap();
    repo.commit(
        "Human",
        "human@example.invalid",
        "2020-01-01T12:00:00+00:00",
    );
    let source = source.replace("x := 1", "x := 2");
    fs::write(repo.0.join("Basic.lean"), &source).unwrap();
    repo.commit("Codex", "noreply@openai.com", "2026-01-01T12:00:00+00:00");
    let license = "normalize.rules.frontmatter.license_text='Configured license.'";
    let output = header_policy_command(&repo, "replace")
        .args(["--set", license, "--dry-run"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("would update"));
    assert_eq!(
        fs::read_to_string(repo.0.join("Basic.lean")).unwrap(),
        source
    );
    let output = header_policy_command(&repo, "replace")
        .args(["--set", license])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let replaced = fs::read_to_string(repo.0.join("Basic.lean")).unwrap();
    assert!(replaced.contains("Copyright (c) 2020 Human."));
    assert!(replaced.contains("Configured license."));
    assert!(
        !replaced.contains("Old attribution")
            && !replaced.contains("Codex")
            && !replaced.contains("2026")
    );
    // A conforming header does not require Git, even with replacement selected.
    let output = header_policy_command(&repo, "replace")
        .args(["--set", license])
        .env("PATH", "/no/git/here")
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(
        output.stderr.is_empty(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(repo.0.join("Basic.lean")).unwrap(),
        replaced
    );
}

#[test]
fn header_policies_with_unavailable_git_warn_ignore_or_retain_as_configured() {
    let repo = Repo::new();
    let source = "/- Existing attribution. -/\n\nmodule\n";
    for policy in ["warn", "ignore", "replace"] {
        fs::write(repo.0.join("Basic.lean"), source).unwrap();
        let output = header_policy_command(&repo, policy)
            .env("PATH", "/no/git/here")
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            fs::read_to_string(repo.0.join("Basic.lean")).unwrap(),
            source
        );
        let stderr = String::from_utf8_lossy(&output.stderr);
        match policy {
            "warn" => {
                assert!(stderr.contains("existing header does not match"));
                assert!(!stderr.contains("Git history"));
            }
            "ignore" => assert!(stderr.is_empty(), "{stderr}"),
            "replace" => {
                assert_eq!(stderr.matches("warning:").count(), 1, "{stderr}");
                assert!(stderr.contains("Basic.lean:1: cannot replace existing header"));
                assert!(stderr.contains("cannot read Git history"));
            }
            _ => unreachable!(),
        }
    }
}

#[test]
fn replacement_with_fully_excluded_git_history_preserves_existing_attribution() {
    let repo = Repo::new();
    let source = "/- Existing attribution. -/\n\nmodule\n";
    fs::write(repo.0.join("Basic.lean"), source).unwrap();
    repo.commit("Codex", "noreply@openai.com", "2026-01-01T12:00:00+00:00");
    let output = header_policy_command(&repo, "replace").output().unwrap();
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert_eq!(stderr.matches("warning:").count(), 1, "{stderr}");
    assert!(stderr.contains("all Git authors") && stderr.contains("existing header retained"));
    assert_eq!(
        fs::read_to_string(repo.0.join("Basic.lean")).unwrap(),
        source
    );
}
