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
    Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
        .args(args)
        .arg(root.as_os_str())
        .output()
        .expect("failed to run leanck-normalize")
}

fn stdout(output: &Output) -> String {
    String::from_utf8(output.stdout.clone()).unwrap()
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
    assert!(
        out.contains("checked 2 files, 2 updated"),
        "got: {out:?}"
    );

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
    assert!(out.contains("updated") && out.contains("a.lean"), "got: {out:?}");
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
    assert!(err.contains("bad.lean"), "got: {err:?}");

    fs::remove_dir_all(&root).unwrap();
}
