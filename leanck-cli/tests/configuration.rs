use std::{
    fs,
    io::Write,
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "leanck-cli-config-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(path.join("user/leanck")).unwrap();
        fs::create_dir_all(path.join("project")).unwrap();
        fs::write(path.join("project/lean-toolchain"), "").unwrap();
        Self(path)
    }
    fn command(&self) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_leanck"));
        command
            .current_dir(self.0.join("project"))
            .env("XDG_CONFIG_HOME", self.0.join("user"));
        command.env_remove("LEANCK_CONFIG");
        command
    }
    fn run(&self, args: &[&str]) -> Output {
        self.command().args(args).output().unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}
#[test]
fn precedence_user_project_explicit_config_selector_and_set() {
    let dir = Scratch::new();
    fs::write(
        dir.0.join("user/leanck/config.toml"),
        "[normalize]\nwidth=71\nheader=false\n",
    )
    .unwrap();
    fs::write(
        dir.0.join("project/leanck.toml"),
        "[normalize]\nwidth=81\n[loogle]\nendpoints=['https://project.test/json']\n",
    )
    .unwrap();
    fs::write(dir.0.join("explicit.toml"), "[normalize]\nwidth=91\n").unwrap();
    let output = dir
        .command()
        .args([
            "config",
            "show",
            "--json",
            "--config",
            "../explicit.toml",
            "--set",
            "normalize.width=101",
        ])
        .env(
            "LEANCK_CONFIG",
            "missing-ignored-when-config-flag-is-present",
        )
        .output()
        .unwrap();
    let report = json(&output);
    assert_eq!(report["settings"]["normalize"]["width"], 101);
    assert_eq!(report["settings"]["normalize"]["header"], false);
    assert_eq!(
        report["settings"]["loogle"]["endpoints"],
        serde_json::json!(["https://project.test/json"])
    );
    assert_eq!(report["sources"]["normalize.width"], "--set");
    assert_eq!(report["files"].as_array().unwrap().len(), 3);
    let output = dir
        .command()
        .args(["config", "show", "--json"])
        .env("LEANCK_CONFIG", "../explicit.toml")
        .output()
        .unwrap();
    assert_eq!(json(&output)["settings"]["normalize"]["width"], 91);
    let output = dir
        .command()
        .args([
            "--no-config",
            "--set",
            "normalize.width=55",
            "config",
            "show",
            "--json",
        ])
        .env("LEANCK_CONFIG", "missing")
        .output()
        .unwrap();
    let report = json(&output);
    assert_eq!(report["settings"]["normalize"]["width"], 55);
    assert_eq!(report["settings"]["cache"]["enabled"], true);
    assert!(report["settings"]["client"]["daemon_bin"].is_null());
    assert!(report["files"].as_array().unwrap().is_empty());
}
#[test]
fn normalize_uses_target_config_dry_run_excludes_and_flags() {
    let dir = Scratch::new();
    fs::create_dir_all(dir.0.join("project/generated")).unwrap();
    fs::write(
        dir.0.join("project/leanck.toml"),
        "[normalize]\nspelling='off'\nexclude=['generated/**']\n",
    )
    .unwrap();
    let dirty = "/-- colour -/\ndef x := 1\n";
    for file in ["Main.lean", "generated/Keep.lean"] {
        fs::write(dir.0.join("project").join(file), dirty).unwrap();
    }
    assert!(dir.run(&["normalize", "--dry-run", "."]).status.success());
    let output = dir.run(&[
        "normalize",
        "--dry-run",
        ".",
        "--set",
        "normalize.spelling='american'",
    ]);
    assert!(!output.status.success());
    assert_eq!(
        fs::read_to_string(dir.0.join("project/Main.lean")).unwrap(),
        dirty
    );
    let output = dir
        .command()
        .current_dir(&dir.0)
        .args([
            "normalize",
            "project",
            "--set",
            "normalize.spelling='american'",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        fs::read_to_string(dir.0.join("project/Main.lean"))
            .unwrap()
            .contains("color")
    );
    assert_eq!(
        fs::read_to_string(dir.0.join("project/generated/Keep.lean")).unwrap(),
        dirty
    );
    let mut child = dir
        .command()
        .args([
            "normalize",
            "-",
            "--british",
            "--set",
            "normalize.spelling='off'",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"/-- color -/\n")
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert!(String::from_utf8(output.stdout).unwrap().contains("colour"));
}
#[test]
fn validation_errors_are_json_and_init_never_overwrites() {
    let dir = Scratch::new();
    let first = dir.run(&["config", "init"]);
    assert!(first.status.success());
    let path = dir.0.join("project/leanck.toml");
    let original = fs::read(&path).unwrap();
    assert!(!dir.run(&["config", "init"]).status.success());
    assert_eq!(fs::read(&path).unwrap(), original);
    assert!(dir.run(&["config", "check"]).status.success());
    fs::write(path, "[normalize]\nwidht=80").unwrap();
    let output = dir.run(&["config", "check", "--json"]);
    assert!(!output.status.success());
    let error: serde_json::Value = serde_json::from_slice(&output.stderr).unwrap();
    assert!(
        error["error"]["message"]
            .as_str()
            .unwrap()
            .contains("widht")
    );
    assert!(dir.run(&["config", "defaults"]).status.success());
}

#[test]
fn normalize_partial_order_from_project_config_pins_namespace() {
    let dir = Scratch::new();
    fs::write(dir.0.join("project/leanck.toml"),
        "[normalize.rules.frontmatter]\norder=['header','module','imports','docstring','section']\n").unwrap();
    let path = dir.0.join("project/Main.lean");
    let source =
        "module\nimport Foo\nnamespace Example\n@[expose] public section\n/-! Module docs -/\n";
    fs::write(&path, source).unwrap();
    let output = dir.run(&["normalize", "Main.lean", "--no-header"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let actual = fs::read_to_string(&path).unwrap();
    assert!(actual.find("namespace Example").unwrap() < actual.find("Module docs").unwrap());
    assert!(actual.find("Module docs").unwrap() < actual.find("public section").unwrap());
    assert!(
        dir.run(&["normalize", "Main.lean", "--no-header", "--dry-run"])
            .status
            .success()
    );
}

#[test]
fn colon_capitalization_is_disabled_by_default_and_can_be_enabled() {
    let dir = Scratch::new();
    let path = dir.0.join("project/Main.lean");
    let source = "/-- Note: lowercase text -/\ndef x := 1\n";
    fs::write(&path, source).unwrap();
    assert!(dir.run(&["normalize", "Main.lean"]).status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    assert!(
        dir.run(&[
            "normalize",
            "Main.lean",
            "--set",
            "normalize.capitalize_after_colon=true"
        ])
        .status
        .success()
    );
    assert_eq!(
        fs::read_to_string(&path).unwrap(),
        source.replace(": lowercase", ": Lowercase")
    );
}

#[test]
fn normalize_stdin_warns_about_conflicting_spacing_without_contaminating_source_output() {
    let dir = Scratch::new();
    for dry_run in [false, true] {
        let mut child = dir
            .command()
            .args([
                "normalize",
                "-",
                "--no-header",
                "--set",
                "normalize.rules.frontmatter.blank_lines=3",
            ])
            .args(if dry_run { vec!["--dry-run"] } else { vec![] })
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(b"module\n\nimport A\n")
            .unwrap();
        let output = child.wait_with_output().unwrap();
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{stderr}");
        assert_eq!(stderr.matches("warning:").count(), 1, "{stderr}");
        assert!(
            stderr
                .contains("max_blank_lines=1 overrides normalize.rules.frontmatter.blank_lines=3"),
            "{stderr}"
        );
        let expected: &[u8] = if dry_run {
            b""
        } else {
            b"module\n\nimport A\n"
        };
        assert_eq!(output.stdout, expected);
    }
}

#[test]
fn config_inspection_exposes_structured_nonfatal_warnings_without_opening_dictionaries() {
    let dir = Scratch::new();
    fs::write(dir.0.join("project/leanck.toml"), "[normalize]\nspelling='off'\n[normalize.rules.headings]\nlevel=3\n[normalize.rules.spelling]\nfiles=['missing.json']\n[client]\ntransport_timeout_ms=100\n").unwrap();
    for action in ["show", "check"] {
        let output = dir.run(&["config", action, "--json"]);
        let report = json(&output);
        assert!(output.stderr.is_empty());
        let warnings = report["warnings"].as_array().unwrap();
        for code in [
            "inactive_heading_rules",
            "inactive_custom_dictionaries",
            "short_transport_timeout",
        ] {
            assert!(warnings.iter().any(|w| w["code"] == code), "{report}");
        }
    }
    let output = dir.run(&["config", "show"]);
    assert!(output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("use_builtin=false"));
    assert!(stderr.contains("normalize.headings=false"));
    assert!(!String::from_utf8_lossy(&output.stdout).contains("warning:"));
    let output = dir.run(&["config", "sources", "--json"]);
    let report = json(&output);
    assert!(report["normalize.spelling"].is_string());
    assert!(
        serde_json::from_slice::<serde_json::Value>(&output.stderr).unwrap()["warnings"].is_array()
    );
}

#[test]
fn normalization_warnings_respect_flags_and_never_open_disabled_custom_dictionaries() {
    let dir = Scratch::new();
    let path = dir.0.join("project/Main.lean");
    fs::write(&path, "/-- colour -/\n").unwrap();
    fs::write(
        dir.0.join("project/leanck.toml"),
        "[normalize.rules.headings]\nlevel=3\n",
    )
    .unwrap();
    let output = dir.run(&[
        "normalize",
        "Main.lean",
        "--no-header",
        "--no-spelling",
        "--dictionary",
        "missing.json",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("normalize.headings=false"));
    assert!(stderr.contains("use_builtin=false"));
    assert_eq!(fs::read_to_string(&path).unwrap(), "/-- colour -/\n");
    let output = dir.run(&["normalize", "Main.lean", "--no-header", "--headings"]);
    assert!(output.status.success());
    assert!(!String::from_utf8_lossy(&output.stderr).contains("normalize.headings=false"));
}

#[test]
fn legacy_setting_environment_variables_do_not_change_settings_or_sources() {
    let dir = Scratch::new();
    fs::write(dir.0.join("project/leanck.toml"),
        "[loogle]\nendpoints=['https://project.test/json']\n[client]\ndaemon_bin='configured-daemon'\n[cache]\nenabled=true\n").unwrap();
    let expected = json(&dir.run(&["config", "show", "--json"]));
    let output = dir
        .command()
        .args(["config", "show", "--json"])
        .env("LOOGLE_URL", "not a URL")
        .env("LEANCKD_BIN", "missing-env-daemon")
        .env("LEANCKD_NO_CACHE_GET", "1")
        .output()
        .unwrap();
    let actual = json(&output);
    assert_eq!(actual, expected);
    assert_eq!(actual["settings"]["cache"]["enabled"], true);
    assert_eq!(
        actual["settings"]["loogle"]["endpoints"],
        serde_json::json!(["https://project.test/json"])
    );
    assert!(
        actual["settings"]["client"]["daemon_bin"]
            .as_str()
            .unwrap()
            .ends_with("/configured-daemon")
    );
}
