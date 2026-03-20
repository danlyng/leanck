use leanck_config::{FinalNewline, LineEndings};
use leanck_normalize::{Context, NormalizeRules, Options, normalize_with_context};

fn apply(source: &str, rules: &NormalizeRules) -> String {
    normalize_with_context(source, 100, Options::default(), rules, &Context::default())
        .unwrap()
        .output
}

#[test]
fn defaults_use_lf_ensure_a_final_newline_and_remove_only_the_leading_bom() {
    let source = "\u{feff}def text := \"inside\u{feff}text\"\r\ndef x := 1";
    let expected = "def text := \"inside\u{feff}text\"\ndef x := 1\n";
    let rules = NormalizeRules::default();
    assert_eq!(apply(source, &rules), expected);
    assert_eq!(apply(expected, &rules), expected);
}

#[test]
fn bom_retention_is_independent_of_line_endings_and_the_final_newline() {
    let mut rules = NormalizeRules::default();
    rules.whitespace.remove_bom = false;
    assert_eq!(
        apply("\u{feff}def x := 1\r\n", &rules),
        "\u{feff}def x := 1\n"
    );
    rules.whitespace.line_endings = LineEndings::Off;
    rules.whitespace.final_newline = FinalNewline::Off;
    let source = "\u{feff}def x := 1\r\ndef y := 2";
    assert_eq!(apply(source, &rules), source);
    rules.whitespace.remove_bom = true;
    assert_eq!(apply(source, &rules), source.trim_start_matches('\u{feff}'));
    rules.whitespace.line_endings = LineEndings::Crlf;
    rules.whitespace.final_newline = FinalNewline::Ensure;
    assert_eq!(apply("\u{feff}def x := 1", &rules), "def x := 1\r\n");
}

#[test]
fn local_whitespace_exceptions_protect_the_initial_bom_without_changing_later_policy() {
    let rules = NormalizeRules::default();
    for command in ["off", "off whitespace"] {
        let source = format!(
            "\u{feff}-- leanck: {command}\r\ndef x := 1\r\n-- leanck: on whitespace\r\ndef y := 2"
        );
        let expected = format!(
            "\u{feff}-- leanck: {command}\r\ndef x := 1\r\n-- leanck: on whitespace\r\ndef y := 2\n"
        );
        assert_eq!(apply(&source, &rules), expected);
    }
    let source = "\u{feff}def x := 1\r\n-- leanck: off whitespace\r\ndef y := 2";
    assert_eq!(
        apply(source, &rules),
        "def x := 1\n-- leanck: off whitespace\r\ndef y := 2"
    );
    assert_eq!(
        apply("\u{feff}-- leanck: off spelling\r\ndef x := 1", &rules),
        "-- leanck: off spelling\r\ndef x := 1\n"
    );
}

#[test]
fn disabling_blank_line_collapsing_does_not_disable_bom_removal() {
    let options = Options {
        collapse_blank_lines: false,
        ..Default::default()
    };
    let result = normalize_with_context(
        "\u{feff}def x := 1",
        100,
        options,
        &NormalizeRules::default(),
        &Context::default(),
    )
    .unwrap();
    assert_eq!(result.output, "def x := 1\n");
}
