use leanck_normalize::{Options, SpellingDirection};
use std::fs;
use std::path::Path;

struct Case {
    name: &'static str,
    width: usize,
    options: Options,
}

fn cases() -> [Case; 4] {
    [
        Case {
            name: "full_module_shuffle",
            width: 72,
            options: Options::default(),
        },
        Case {
            name: "comment_edge_shapes",
            width: 56,
            options: Options::default(),
        },
        Case {
            name: "american_to_british",
            width: 64,
            options: Options {
                spelling: Some(SpellingDirection::AmericanToBritish),
                ..Options::default()
            },
        },
        Case {
            name: "heading_banners",
            width: 72,
            options: Options {
                headings: true,
                ..Options::default()
            },
        },
    ]
}

#[test]
fn pathological_fixtures_normalize_to_expected_output() {
    for case in cases() {
        let input = fixture(case.name, "input.lean");
        let expected = fixture(case.name, "expected.lean");
        let actual = run_normalizer(&case, &input);

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
    for case in cases() {
        let expected = fixture(case.name, "expected.lean");
        let actual = run_normalizer(&case, &expected);

        assert_eq!(
            actual,
            expected,
            "fixture `{}` expected output was not idempotent\n{}",
            case.name,
            first_difference(&actual, &expected)
        );
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

fn run_normalizer(case: &Case, source: &str) -> String {
    leanck_normalize::normalize_with_context(
        source,
        case.width,
        case.options,
        &leanck_config::NormalizeRules::default(),
        &leanck_normalize::Context::default(),
    )
    .unwrap()
    .output
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
