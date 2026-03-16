//! Directory runs isolate per-file panics, replace files atomically, and
//! summarize repeated missing-history notes.
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const DIRTY: &str = "/-- colour -/\ndef x := 1\n";
const CLEAN: &str = "/-- color -/\ndef x := 1\n";

fn temp_root(tag: &str) -> PathBuf {
    let dir =
        std::env::temp_dir().join(format!("leanck-normalize-cli-{}-{tag}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn normalize(root: &Path, args: &[&str], envs: &[(&str, &str)]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
        .args(["--no-config"])
        .args(args)
        .arg(root)
        .envs(envs.iter().copied())
        // Keep Git from discovering a repository above the scratch directory.
        .env("GIT_CEILING_DIRECTORIES", root.parent().unwrap())
        .output()
        .unwrap()
}

// The binary honors the panic hook only in builds with debug assertions,
// which share this test's profile.
#[cfg(debug_assertions)]
#[test]
fn a_panic_in_one_file_is_reported_and_the_rest_are_still_normalized() {
    let root = temp_root("panic");
    let panicking = format!("-- PANIC_HERE\n{DIRTY}");
    fs::write(root.join("a.lean"), DIRTY).unwrap();
    fs::write(root.join("b.lean"), &panicking).unwrap();
    fs::write(root.join("c.lean"), DIRTY).unwrap();

    let output = normalize(&root, &[], &[("LEANCK_NORMALIZE_TEST_PANIC", "PANIC_HERE")]);
    assert!(!output.status.success());
    assert_eq!(fs::read_to_string(root.join("a.lean")).unwrap(), CLEAN);
    assert_eq!(fs::read_to_string(root.join("b.lean")).unwrap(), panicking);
    assert_eq!(fs::read_to_string(root.join("c.lean")).unwrap(), CLEAN);
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert_eq!(
        stdout,
        "checked 3 files, 2 updated, 1 failed (comments=2)\n"
    );
    let stderr = String::from_utf8(output.stderr).unwrap();
    let prefix = format!(
        "error in {}: internal error (panicked at ",
        root.join("b.lean").display()
    );
    assert!(stderr.contains(&prefix), "{stderr}");
    assert!(
        stderr
            .contains("test panic requested by LEANCK_NORMALIZE_TEST_PANIC); file left unchanged"),
        "{stderr}"
    );
    assert!(!stderr.contains("thread 'main' panicked"), "{stderr}");
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn rewrites_are_atomic_preserve_permissions_and_follow_symlinks() {
    use std::os::unix::fs::{PermissionsExt, symlink};
    let root = temp_root("atomic");
    let file = root.join("a.lean");
    fs::write(&file, DIRTY).unwrap();
    fs::set_permissions(&file, fs::Permissions::from_mode(0o640)).unwrap();

    let output = normalize(&root, &[], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&file).unwrap(), CLEAN);
    assert_eq!(
        fs::metadata(&file).unwrap().permissions().mode() & 0o777,
        0o640
    );
    let entries: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, ["a.lean"], "no temporary file may remain");

    // A single-file argument that is a symbolic link updates its target.
    let target = root.join("target.lean");
    let link = root.join("link.lean");
    fs::write(&target, DIRTY).unwrap();
    symlink(&target, &link).unwrap();
    let output = normalize(&link, &[], &[]);
    assert!(output.status.success(), "{output:?}");
    assert!(
        fs::symlink_metadata(&link)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(fs::read_to_string(&target).unwrap(), CLEAN);
    fs::remove_dir_all(root).unwrap();
}

#[cfg(unix)]
#[test]
fn files_that_a_rename_cannot_replace_are_rewritten_in_place() {
    use std::os::unix::fs::PermissionsExt;
    // A writable file in a read-only directory.
    let root = temp_root("read-only-dir");
    let dir = root.join("d");
    fs::create_dir(&dir).unwrap();
    let file = dir.join("a.lean");
    fs::write(&file, DIRTY).unwrap();
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
    let output = normalize(&file, &[], &[]);
    fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&file).unwrap(), CLEAN);
    fs::remove_dir_all(root).unwrap();

    // Hard links keep sharing the new contents.
    let root = temp_root("hard-link");
    let file = root.join("h.lean");
    let link = root.join("h2.lean");
    fs::write(&file, DIRTY).unwrap();
    fs::hard_link(&file, &link).unwrap();
    let output = normalize(&file, &[], &[]);
    assert!(output.status.success(), "{output:?}");
    assert_eq!(fs::read_to_string(&file).unwrap(), CLEAN);
    assert_eq!(fs::read_to_string(&link).unwrap(), CLEAN);
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn missing_history_is_noted_once_for_a_directory_outside_git() {
    let root = temp_root("history-note");
    for name in ["A.lean", "B.lean", "C.lean"] {
        fs::write(root.join(name), "module\n").unwrap();
    }
    let output = normalize(&root, &[], &[]);
    assert!(output.status.success(), "{output:?}");
    let stderr = String::from_utf8(output.stderr).unwrap();
    let notes: Vec<_> = stderr.lines().filter(|l| l.starts_with("note:")).collect();
    assert_eq!(notes.len(), 1, "{stderr}");
    assert!(
        notes[0].ends_with(
            "; missing attribution headers left absent in 3 files (use --verbose to list them)"
        ),
        "{stderr}"
    );

    // Verbose runs still name every file.
    let output = normalize(&root, &["--verbose"], &[]);
    let stderr = String::from_utf8(output.stderr).unwrap();
    for name in ["A.lean", "B.lean", "C.lean"] {
        assert!(
            stderr.contains(&format!("note: {}: ", root.join(name).display())),
            "{stderr}"
        );
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn markdown_parser_panics_are_not_printed_when_reading_stdin() {
    use std::io::Write;
    let source = "/-- * $$\n1) x -/\ndef a := 1\n";
    let mut child = Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
        .args(["--no-config", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(source.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success());
    assert_eq!(String::from_utf8(output.stdout).unwrap(), source);
    assert_eq!(String::from_utf8(output.stderr).unwrap(), "");
}
