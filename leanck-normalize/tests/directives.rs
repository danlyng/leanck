//! Observable directive behavior: each expected segment spells out which rules run.
use leanck_config::{FinalNewline, LineEndings};
use leanck_normalize::{Context, NormalizeError, NormalizeRules, Options, normalize_with_context};

const RAW: &str = "-- ====\n-- mixed heading\n-- ====\n/-- colour: blue -/\n\n\n@[simp]\ntheorem demo : True := trivial\n";
const NORMALIZED: &str =
    "/-! ## Mixed Heading -/\n/-- color: Blue -/\n\n@[simp] theorem demo : True := trivial\n";
const SPELLING_AND_ATTRIBUTES: &str = "-- ====\n-- mixed heading\n-- ====\n/-- color: blue -/\n\n\n@[simp] theorem demo : True := trivial\n";
const WITHOUT_ATTRIBUTES_AND_HEADINGS: &str = "-- ====\n-- mixed heading\n-- ====\n/-- color: Blue -/\n\n@[simp]\ntheorem demo : True := trivial\n";
const WITHOUT_ATTRIBUTES: &str =
    "/-! ## Mixed Heading -/\n/-- color: Blue -/\n\n@[simp]\ntheorem demo : True := trivial\n";
const WITHOUT_ATTRIBUTES_AND_SPELLING: &str =
    "/-! ## Mixed Heading -/\n/-- colour: Blue -/\n\n@[simp]\ntheorem demo : True := trivial\n";

fn options() -> Options {
    Options {
        headings: true,
        capitalize_after_colon: true,
        ..Default::default()
    }
}

fn apply(source: &str, options: Options, rules: &NormalizeRules) -> Result<String, NormalizeError> {
    normalize_with_context(source, 100, options, rules, &Context::default()).map(|r| r.output)
}

fn assert_output(source: &str, expected: &str) {
    let rules = NormalizeRules::default();
    let actual = apply(source, options(), &rules).unwrap();
    assert_eq!(actual, expected);
    assert_eq!(
        apply(&actual, options(), &rules).unwrap(),
        actual,
        "not idempotent"
    );
}

#[test]
fn off_all_then_on_named_rules_reenables_only_those_rules() {
    let source = format!("-- leanck: off\n{RAW}-- leanck: on spelling,attributes\n{RAW}");
    let expected = format!(
        "-- leanck: off\n{RAW}-- leanck: on spelling,attributes\n{SPELLING_AND_ATTRIBUTES}"
    );
    assert_output(&source, &expected);
}

#[test]
fn overlapping_named_directives_accumulate_until_bare_on_resets_everything() {
    let source = format!(
        "-- leanck: off attributes,headings\n{RAW}-- leanck: on headings\n{RAW}-- leanck: off spelling\n{RAW}-- leanck: on \n{RAW}"
    );
    let expected = format!(
        "-- leanck: off attributes,headings\n{WITHOUT_ATTRIBUTES_AND_HEADINGS}-- leanck: on headings\n{WITHOUT_ATTRIBUTES}-- leanck: off spelling\n{WITHOUT_ATTRIBUTES_AND_SPELLING}-- leanck: on \n{NORMALIZED}"
    );
    assert_output(&source, &expected);
}

#[test]
fn repeated_directives_and_duplicate_names_are_idempotent_not_nested() {
    let source = format!(
        "-- leanck: off attributes\n-- leanck: off attributes, attributes\n-- leanck: on attributes\n{RAW}-- leanck: on attributes\n{RAW}"
    );
    let expected = format!(
        "-- leanck: off attributes\n-- leanck: off attributes, attributes\n-- leanck: on attributes\n{NORMALIZED}-- leanck: on attributes\n{NORMALIZED}"
    );
    assert_output(&source, &expected);
}

#[test]
fn explicit_all_and_mixed_all_lists_have_the_same_reset_semantics() {
    let source = format!(
        "-- leanck: off all,spelling\n{RAW}-- leanck: on spelling,attributes\n{RAW}-- leanck: on all,attributes\n{RAW}"
    );
    let expected = format!(
        "-- leanck: off all,spelling\n{RAW}-- leanck: on spelling,attributes\n{SPELLING_AND_ATTRIBUTES}-- leanck: on all,attributes\n{NORMALIZED}"
    );
    assert_output(&source, &expected);
}

#[test]
fn partial_reenable_can_be_disabled_again_without_restoring_other_rules() {
    let source = "-- leanck: off\n-- leanck: on spelling\n/-- colour: blue -/\n-- leanck: off spelling\n/-- colour: blue -/\n-- leanck: on attributes\n@[simp]\ntheorem demo : True := trivial\n/-- colour: blue -/\n";
    let expected = "-- leanck: off\n-- leanck: on spelling\n/-- color: blue -/\n-- leanck: off spelling\n/-- colour: blue -/\n-- leanck: on attributes\n@[simp] theorem demo : True := trivial\n/-- colour: blue -/\n";
    assert_output(source, expected);
}

#[test]
fn spelling_only_keeps_comment_layout_reflow_spacing_and_other_prose_rules_off() {
    let source = "-- leanck: off\n-- leanck: on spelling\n/-!\n  colour: blue\n\n\n  colour on its own line\n-/\n/--\ncolour: blue -/\n";
    let expected = "-- leanck: off\n-- leanck: on spelling\n/-!\n  color: blue\n\n\n  color on its own line\n-/\n/--\ncolor: blue -/\n";
    assert_output(source, expected);
}

#[test]
fn named_on_restores_configured_options_without_forcing_disabled_features_on() {
    let source = format!("-- leanck: off\n-- leanck: on\n{RAW}");
    let options = Options {
        attributes: false,
        headings: false,
        spelling: None,
        comment_reflow: false,
        ..Default::default()
    };
    let expected = source.replace("\n\n\n", "\n\n");
    let actual = apply(&source, options, &NormalizeRules::default()).unwrap();
    assert_eq!(actual, expected);
    let source = "-- leanck: off\n-- leanck: on spelling,attributes,headings\n/-- colour -/\n-- ====\n-- mixed heading\n-- ====\n@[simp]\ntheorem demo : True := trivial\n";
    assert_eq!(
        apply(source, options, &NormalizeRules::default()).unwrap(),
        source
    );
}

#[test]
fn directive_text_in_strings_nested_comments_and_inline_comments_is_inert() {
    let source = "def text := \"-- leanck: off\"\ndef raw := r#\"-- leanck: off\"#\n/- outer\n/- -- leanck: off -/\n-- leanck: off\n-/\ndef x := 1 -- leanck: off\ndef y := 2 -- leanck: nonsense invalid_name\n-- leanck off\n-- leanckish: off\n/-- colour -/\n";
    assert_output(source, &source.replace("/-- colour -/", "/-- color -/"));
}

#[test]
fn invalid_standalone_directives_are_errors_even_inside_disabled_regions() {
    for command in ["nonsense", "off unknown", "on spelling,", "off all,unknown"] {
        let source = format!("-- leanck: off\n-- leanck: {command}\n/-- colour -/\n");
        assert!(
            matches!(
                apply(&source, options(), &NormalizeRules::default()),
                Err(NormalizeError::InvalidConfiguration(_))
            ),
            "accepted {command}"
        );
    }
}

#[test]
fn disabling_directive_support_ignores_commands_and_validation() {
    let rules = NormalizeRules {
        directives: false,
        ..Default::default()
    };
    let source = "-- leanck: off\n-- leanck: nonsense invalid\n/-- colour: blue -/\n";
    let expected = source.replace("colour: blue", "color: Blue");
    assert_eq!(apply(source, options(), &rules).unwrap(), expected);
}

#[test]
fn directives_do_not_allow_attribute_moves_across_their_lines() {
    let source = "@[simp]\n-- leanck: off spelling\ntheorem demo : True := trivial\n-- leanck: on spelling\n";
    assert_output(source, source);
}

#[test]
fn fully_disabled_regions_keep_bom_crlf_trailing_spaces_and_missing_final_newline() {
    let mut rules = NormalizeRules::default();
    rules.whitespace.line_endings = LineEndings::Lf;
    rules.whitespace.final_newline = FinalNewline::Ensure;
    let source = "\u{feff}-- leanck: off\r\n/-- colour -/  \r\n\r\n\r\ndef x := 1  ";
    assert_eq!(apply(source, options(), &rules).unwrap(), source);
}

#[test]
fn reenabling_whitespace_alone_restores_line_endings_and_eof_policy() {
    let mut rules = NormalizeRules::default();
    rules.whitespace.line_endings = LineEndings::Lf;
    rules.whitespace.final_newline = FinalNewline::Ensure;
    let source = "-- leanck: off\r\n/-- colour -/\r\n-- leanck: on whitespace\r\n/-- colour -/\r\n\r\n\r\ndef x := 1";
    let expected = "-- leanck: off\r\n/-- colour -/\r\n-- leanck: on whitespace\r\n/-- colour -/\n\ndef x := 1\n";
    assert_eq!(apply(source, options(), &rules).unwrap(), expected);
}

#[test]
fn an_unclosed_comment_in_a_fully_disabled_tail_is_left_verbatim() {
    let source = "-- leanck: off\n/-- unclosed colour";
    assert_output(source, source);
}

#[test]
fn each_independent_prose_rule_can_be_reenabled_after_off_all() {
    for (name, source, expected) in [
        (
            "headings",
            "/-! ## mixed heading -/\n/-- colour: blue -/\n",
            "/-! ## Mixed Heading -/\n/-- colour: blue -/\n",
        ),
        (
            "capitalize_after_colon",
            "/-- colour: blue -/\n",
            "/-- colour: Blue -/\n",
        ),
        ("spelling", "/-- colour: blue -/\n", "/-- color: blue -/\n"),
        (
            "attributes",
            "@[simp]\ntheorem demo : True := trivial\n/-- colour -/\n",
            "@[simp] theorem demo : True := trivial\n/-- colour -/\n",
        ),
    ] {
        let prefix = format!("-- leanck: off\n-- leanck: on {name}\n");
        assert_output(&format!("{prefix}{source}"), &format!("{prefix}{expected}"));
    }
}

#[test]
fn comment_master_can_be_partially_reenabled_then_restored_as_a_group() {
    let source = "-- leanck: off comments\n/--\ncolour: blue -/\n-- leanck: on spelling\n/--\ncolour: blue -/\n-- leanck: on comments\n/--\ncolour: blue -/\n";
    let expected = "-- leanck: off comments\n/--\ncolour: blue -/\n-- leanck: on spelling\n/--\ncolor: blue -/\n-- leanck: on comments\n/-- color: Blue -/\n";
    assert_output(source, expected);
}

#[test]
fn restoring_comments_does_not_restore_unrelated_disabled_rules() {
    let source = format!("-- leanck: off\n-- leanck: on comments\n{RAW}");
    let expected = "-- leanck: off\n-- leanck: on comments\n-- ====\n-- mixed heading\n-- ====\n/-- color: Blue -/\n\n\n@[simp]\ntheorem demo : True := trivial\n";
    assert_output(&source, expected);
}

#[test]
fn reflow_alone_does_not_enable_spelling_capitalization_or_url_linkification() {
    let source = "-- leanck: off\n-- leanck: on comment_reflow\n/-- colour: blue one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen https://example.test -/\n";
    let options = Options {
        linkify_urls: true,
        ..options()
    };
    let rules = NormalizeRules::default();
    let out = apply(source, options, &rules).unwrap();
    assert!(out.contains("colour: blue"), "{out}");
    assert!(
        !out.contains("color") && !out.contains("Blue") && !out.contains("](https"),
        "{out}"
    );
    assert!(out.lines().all(|line| line.chars().count() <= 100), "{out}");
    assert!(out.lines().count() > source.lines().count(), "{out}");
    assert_eq!(apply(&out, options, &rules).unwrap(), out);
}

#[test]
fn reenabling_frontmatter_does_not_cross_a_directive_barrier() {
    let source = "module\nimport Z\nimport A\n-- leanck: off\n-- leanck: on frontmatter\nimport Y\nimport B\n";
    let expected = "module\n\nimport A\nimport Z\n-- leanck: off\n-- leanck: on frontmatter\nimport Y\nimport B\n";
    assert_output(source, expected);
}

#[test]
fn indentation_tabs_and_no_terminal_newline_are_allowed_on_directives() {
    let source = "  -- leanck: off\n/-- colour -/\n\t-- leanck: on\tspelling , attributes\n/-- colour -/\n@[simp]\ntheorem demo : True := trivial\n-- leanck: off";
    let expected = "  -- leanck: off\n/-- colour -/\n\t-- leanck: on\tspelling , attributes\n/-- color -/\n@[simp] theorem demo : True := trivial\n-- leanck: off";
    assert_output(source, expected);
}

#[test]
fn enabling_attributes_does_not_require_disabled_comment_parsing() {
    let source = "-- leanck: off\n-- leanck: on attributes\n@[simp]\ntheorem demo : True := trivial\n/-- unclosed colour";
    let expected = source.replace("@[simp]\ntheorem", "@[simp] theorem");
    assert_output(source, &expected);
}

#[test]
fn partial_reenable_keeps_trailing_blank_lines_while_whitespace_is_off() {
    let source =
        "-- leanck: off\n-- leanck: on attributes\n@[simp]\ntheorem demo : True := trivial\n\n\n";
    let expected = source.replace("@[simp]\ntheorem", "@[simp] theorem");
    assert_output(source, &expected);
}
