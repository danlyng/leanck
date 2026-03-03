use std::fs;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

struct Case {
    name: &'static str,
    width: &'static str,
    extra_args: &'static [&'static str],
}

const CASES: &[Case] = &[
    Case {
        name: "full_module_shuffle",
        width: "72",
        extra_args: &[],
    },
    Case {
        name: "comment_edge_shapes",
        width: "56",
        extra_args: &[],
    },
    Case {
        name: "american_to_british",
        width: "64",
        extra_args: &["--british"],
    },
    Case {
        name: "heading_banners",
        width: "72",
        extra_args: &["--headings"],
    },
];

#[test]
fn pathological_fixtures_normalize_to_expected_output() {
    for case in CASES {
        let input = fixture(case.name, "input.lean");
        let expected = fixture(case.name, "expected.lean");
        let actual = run_normalizer(case, &input);

        assert_eq!(
            actual,
            expected,
            "fixture `{}` did not normalize as expected\n{}",
            case.name,
            first_difference(&actual, &expected)
        );
    }
}

#[test]
fn pathological_expected_outputs_are_idempotent() {
    for case in CASES {
        let expected = fixture(case.name, "expected.lean");
        let actual = run_normalizer(case, &expected);

        assert_eq!(
            actual,
            expected,
            "fixture `{}` expected output was not idempotent\n{}",
            case.name,
            first_difference(&actual, &expected)
        );
    }
}

#[test]
fn pathological_outputs_do_not_contain_tabs() {
    for case in CASES {
        let input = fixture(case.name, "input.lean");
        let expected = fixture(case.name, "expected.lean");
        let actual = run_normalizer(case, &input);

        assert_no_tabs(case.name, "expected", &expected);
        assert_no_tabs(case.name, "actual", &actual);
    }
}

fn fixture(case: &str, kind: &str) -> String {
    let path = fixture_path(case, kind);
    fs::read_to_string(&path)
        .unwrap_or_else(|err| panic!("failed to read fixture {}: {err}", path.display()))
}

fn fixture_path(case: &str, kind: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("pathological")
        .join(format!("{case}.{kind}"))
}

fn run_normalizer(case: &Case, input: &str) -> String {
    let mut child = Command::new(env!("CARGO_BIN_EXE_leanck-normalize"))
        .args(case.extra_args)
        .args(["--width", case.width])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap_or_else(|err| panic!("failed to spawn normalizer for {}: {err}", case.name));

    child
        .stdin
        .as_mut()
        .expect("normalizer stdin was not piped")
        .write_all(input.as_bytes())
        .unwrap_or_else(|err| panic!("failed to write normalizer stdin for {}: {err}", case.name));

    let output = child
        .wait_with_output()
        .unwrap_or_else(|err| panic!("failed to wait for normalizer for {}: {err}", case.name));

    assert!(
        output.status.success(),
        "normalizer failed for `{}` with status {}\nstderr:\n{}",
        case.name,
        output.status,
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8(output.stdout)
        .unwrap_or_else(|err| panic!("normalizer wrote non-UTF-8 output for {}: {err}", case.name))
}

fn assert_no_tabs(case: &str, kind: &str, text: &str) {
    for (idx, line) in text.lines().enumerate() {
        assert!(
            !line.contains('\t'),
            "fixture `{case}` {kind} output contains a tab on line {}: {line:?}",
            idx + 1
        );
    }
}

fn first_difference(actual: &str, expected: &str) -> String {
    for (idx, (actual_line, expected_line)) in actual.lines().zip(expected.lines()).enumerate() {
        if actual_line != expected_line {
            return format!(
                "first differing line {}:\nactual:   {:?}\nexpected: {:?}",
                idx + 1,
                actual_line,
                expected_line
            );
        }
    }

    let actual_count = actual.lines().count();
    let expected_count = expected.lines().count();
    if actual_count != expected_count {
        format!("line counts differ: actual {actual_count}, expected {expected_count}")
    } else {
        "strings differ outside line content (possibly trailing newline)".to_string()
    }
}
