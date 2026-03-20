use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DIRTY: &str = "/-- colour -/\ndef x := 1\n";
const CLEAN: &str = "/-- color -/\ndef x := 1\n";

/// A fresh scratch directory under the system temp dir, unique per test.
fn temp_root(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "leanck-normalize-test-{}-{tag}",
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn run(args: &[&str], root: &Path) -> Output {
    normalizer()
        .args(args)
        .arg(root.as_os_str())
        .output()
        .expect("failed to run leanck-normalize")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
}

#[test]
fn numeric_filename_is_rewritten_in_place() {
    let root = temp_root("numeric-filename");
    let path = root.join("80");
    fs::write(&path, DIRTY).unwrap();

    let output = normalizer()
        .current_dir(&root)
        .args(["--width", "72", "80"])
        .output()
        .unwrap();
    assert!(output.status.success(), "stderr: {:?}", output.stderr);
    assert_eq!(fs::read_to_string(&path).unwrap(), CLEAN);
    assert!(stdout(&output).contains("updated 80"));

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn directory_is_normalized_recursively() {
    let root = temp_root("recursive");
    fs::create_dir_all(root.join("sub/nested")).unwrap();
    fs::create_dir_all(root.join(".hidden")).unwrap();
    let a = root.join("a.lean");
    let b = root.join("sub/nested/b.lean");
    let txt = root.join("notes.txt");
    let hidden = root.join(".hidden/c.lean");
    for f in [&a, &b, &txt, &hidden] {
        fs::write(f, DIRTY).unwrap();
    }

    let output = run(&[], &root);
    assert!(output.status.success(), "stderr: {:?}", output.stderr);

    assert_eq!(fs::read_to_string(&a).unwrap(), CLEAN);
    assert_eq!(fs::read_to_string(&b).unwrap(), CLEAN);
    assert_eq!(
        fs::read_to_string(&txt).unwrap(),
        DIRTY,
        "non-.lean files must be untouched by default"
    );
    assert_eq!(
        fs::read_to_string(&hidden).unwrap(),
        DIRTY,
        "hidden directories must be skipped"
    );

    let out = stdout(&output);
    assert!(out.contains("checked 2 files, 2 updated"), "got: {out:?}");

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn directory_output_is_summary_only_by_default() {
    let root = temp_root("summary");
    fs::write(root.join("a.lean"), DIRTY).unwrap();
    fs::write(root.join("b.lean"), CLEAN).unwrap();

    let output = run(&[], &root);
    assert!(output.status.success());
    let out = stdout(&output);
    assert!(
        !out.contains("a.lean"),
        "per-file lines need --verbose, got: {out:?}"
    );
    // The summary line carries the aggregated stats.
    assert!(
        out.contains("checked 2 files, 1 updated (comments=1)"),
        "got: {out:?}"
    );
    assert_eq!(out.lines().count(), 1, "got: {out:?}");

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn verbose_prints_each_updated_file() {
    let root = temp_root("verbose");
    fs::write(root.join("a.lean"), DIRTY).unwrap();
    fs::write(root.join("b.lean"), CLEAN).unwrap();

    let output = run(&["--verbose"], &root);
    assert!(output.status.success());
    let out = stdout(&output);
    assert!(
        out.contains("updated") && out.contains("a.lean"),
        "got: {out:?}"
    );
    assert!(
        !out.contains("b.lean"),
        "unchanged files are not listed even with --verbose, got: {out:?}"
    );
    assert!(out.contains("checked 2 files, 1 updated"), "got: {out:?}");

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn all_files_flag_includes_other_suffixes() {
    let root = temp_root("all-files");
    let txt = root.join("notes.txt");
    fs::write(&txt, DIRTY).unwrap();

    let output = run(&["--all-files"], &root);
    assert!(output.status.success());
    assert_eq!(fs::read_to_string(&txt).unwrap(), CLEAN);

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn errors_in_one_file_do_not_stop_others() {
    let root = temp_root("errors");
    let bad = root.join("bad.lean");
    let good = root.join("good.lean");
    fs::write(&bad, "/-- never closed\n").unwrap();
    fs::write(&good, DIRTY).unwrap();

    let output = run(&[], &root);
    assert!(
        !output.status.success(),
        "a failed file must produce a nonzero exit"
    );
    assert_eq!(
        fs::read_to_string(&good).unwrap(),
        CLEAN,
        "files after an error must still be processed"
    );
    let out = stdout(&output);
    assert!(
        out.contains("checked 2 files, 1 updated, 1 failed"),
        "got: {out:?}"
    );
    let err = String::from_utf8(output.stderr.clone()).unwrap();
    assert!(
        err.contains(&format!(
            "error in {}: unmatched opening delimiter '/--' at line 1 (no closing '-/')",
            bad.display()
        )),
        "got: {err:?}"
    );
    assert_eq!(fs::read_to_string(&bad).unwrap(), "/-- never closed\n");

    fs::remove_dir_all(&root).unwrap();
}

#[test]
fn standalone_loads_project_rules_and_honors_no_config() {
    let root = temp_root("config");
    fs::write(root.join("leanck.toml"), "[normalize]\nspelling='off'\n").unwrap();
    fs::write(root.join("Main.lean"), DIRTY).unwrap();
    let output = run(&["--dry-run"], &root);
    assert!(output.status.success());
    let output = run(&["--no-config", "--dry-run"], &root);
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(root.join("Main.lean")).unwrap(), DIRTY);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn configured_dictionary_paths_are_relative_to_config_and_invalid_resources_write_nothing() {
    let root = temp_root("dictionaries");
    fs::create_dir_all(root.join("settings")).unwrap();
    fs::write(
        root.join("settings/words.json"),
        r#"{"colour":"hue","special":"preferred"}"#,
    )
    .unwrap();
    fs::write(
        root.join("settings/config.toml"),
        "[normalize.rules.spelling]\nuse_builtin=false\nfiles=['words.json']\n",
    )
    .unwrap();
    fs::write(root.join("Main.lean"), "/-- colour special -/\n").unwrap();
    let output = normalizer()
        .arg("--config")
        .arg(root.join("settings/config.toml"))
        .arg(root.join("Main.lean"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        fs::read_to_string(root.join("Main.lean")).unwrap(),
        "/-- hue preferred -/\n"
    );
    fs::write(root.join("settings/words.json"), "{not json}").unwrap();
    let before = fs::read(root.join("Main.lean")).unwrap();
    let output = normalizer()
        .arg("--config")
        .arg(root.join("settings/config.toml"))
        .arg(root.join("Main.lean"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert_eq!(fs::read(root.join("Main.lean")).unwrap(), before);
    let output = normalizer()
        .args(["--no-config", "--export-dictionary", "american"])
        .output()
        .unwrap();
    let dictionary: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(dictionary["colour"], "color");
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn disabled_spelling_does_not_open_dictionary_files() {
    let root = temp_root("disabled-dictionary");
    fs::write(root.join("Main.lean"), DIRTY).unwrap();
    let output = normalizer()
        .current_dir(&root)
        .args([
            "--no-config",
            "--no-spelling",
            "--dictionary",
            "missing.json",
            "Main.lean",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(root.join("Main.lean")).unwrap(), DIRTY);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn directive_partial_reenable_and_dry_run_use_the_same_rules_and_errors_do_not_write() {
    let root = temp_root("directives");
    let path = root.join("Main.lean");
    let source = "-- leanck: off\n/-- colour: blue -/\n@[simp]\ntheorem a : True := trivial\n-- leanck: on spelling,attributes\n/-- colour: blue -/\n@[simp]\ntheorem b : True := trivial\n";
    let expected = "-- leanck: off\n/-- colour: blue -/\n@[simp]\ntheorem a : True := trivial\n-- leanck: on spelling,attributes\n/-- color: blue -/\n@[simp] theorem b : True := trivial\n";
    fs::write(&path, source).unwrap();
    let output = run(&["--no-config", "--dry-run"], &path);
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(&path).unwrap(), source);
    let output = run(&["--no-config"], &path);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), expected);
    assert!(run(&["--no-config", "--dry-run"], &path).status.success());
    let invalid = "/-- colour -/\n-- leanck: on unknown_rule\n";
    fs::write(&path, invalid).unwrap();
    let output = run(&["--no-config"], &path);
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("unknown directive rule"));
    assert_eq!(fs::read_to_string(&path).unwrap(), invalid);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn frontmatter_spacing_warning_respects_effective_caps_and_master_switches() {
    let root = temp_root("spacing-warning");
    let path = root.join("Main.lean");
    let source = "module\n\nimport A\n";
    for (extra, warning) in [
        (vec![], true),
        (
            vec!["--set", "normalize.rules.whitespace.max_blank_lines=0"],
            true,
        ),
        (
            vec!["--set", "normalize.rules.whitespace.max_blank_lines=3"],
            false,
        ),
        (
            vec!["--set", "normalize.rules.whitespace.max_blank_lines=5"],
            false,
        ),
        (vec!["--no-frontmatter"], false),
        (vec!["--no-collapse-blank-lines"], false),
        (vec!["--set", "normalize.frontmatter=false"], false),
        (vec!["--set", "normalize.collapse_blank_lines=false"], false),
    ] {
        fs::write(&path, source).unwrap();
        let mut args = vec![
            "--no-config",
            "--no-header",
            "--set",
            "normalize.rules.frontmatter.blank_lines=3",
        ];
        args.extend(extra);
        let output = run(&args, &path);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{args:?}: {stderr}");
        assert_eq!(
            stderr.matches("warning:").count(),
            usize::from(warning),
            "{args:?}: {stderr}"
        );
        assert!(!stdout(&output).contains("warning:"));
        if warning {
            assert!(
                stderr.contains("normalize.rules.whitespace.max_blank_lines="),
                "{stderr}"
            );
            assert!(
                stderr.contains("overrides normalize.rules.frontmatter.blank_lines=3"),
                "{stderr}"
            );
            assert!(
                stderr.contains("normalize.collapse_blank_lines=false"),
                "{stderr}"
            );
        }
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn frontmatter_spacing_warning_is_once_per_recursive_run_and_dry_run_still_succeeds() {
    let root = temp_root("spacing-warning-recursive");
    let source = "module\n\nimport A\n";
    for name in ["A.lean", "B.lean"] {
        fs::write(root.join(name), source).unwrap();
    }
    let output = run(
        &[
            "--no-config",
            "--no-header",
            "--dry-run",
            "--set",
            "normalize.rules.frontmatter.blank_lines=3",
        ],
        &root,
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(output.status.success(), "{stderr}");
    assert_eq!(stderr.matches("warning:").count(), 1, "{stderr}");
    assert!(
        stderr.contains("max_blank_lines=1 overrides normalize.rules.frontmatter.blank_lines=3"),
        "{stderr}"
    );
    for name in ["A.lean", "B.lean"] {
        assert_eq!(fs::read_to_string(root.join(name)).unwrap(), source);
    }
    let output = normalizer()
        .args([
            "--no-config",
            "--set",
            "normalize.rules.frontmatter.blank_lines=3",
            "--export-dictionary",
            "american",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert!(output.stderr.is_empty());
    assert!(
        serde_json::from_slice::<serde_json::Value>(&output.stdout)
            .unwrap()
            .is_object()
    );
    fs::remove_dir_all(root).unwrap();
}

fn normalizer() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_leanck"));
    command.arg("normalize");
    command
}
