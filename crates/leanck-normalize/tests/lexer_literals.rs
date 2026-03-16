//! Comment detection follows Lean's lexer around interpolated strings,
//! escaped identifiers, primes, character literals, and `/--/`.
use leanck_normalize::{Context, NormalizeRules, Options, normalize_with_context};

fn normalize(source: &str) -> String {
    normalize_with_context(
        source,
        100,
        Options::default(),
        &NormalizeRules::default(),
        &Context::default(),
    )
    .unwrap()
    .output
}

#[test]
fn interpolated_strings_hide_nested_strings_and_comment_openers() {
    let source = "def f := s!\"{\"/--\"}\"\n/--   doc   -/\ndef g := 1\n";
    assert_eq!(
        normalize(source),
        "def f := s!\"{\"/--\"}\"\n/-- doc -/\ndef g := 1\n"
    );
    // A `--` inside the interpolant must not hide the rest of the line.
    let source = "example := throwErrorAt stx \"{\"--\"}\" /--   doc   -/\ndef g := 1\n";
    assert_eq!(
        normalize(source),
        "example := throwErrorAt stx \"{\"--\"}\" /-- doc -/\ndef g := 1\n"
    );
}

#[test]
fn every_core_interpolating_form_hides_quotes_in_its_interpolants() {
    // An odd quote inside the braces would otherwise end the string early,
    // so the later string literal would be read as code with a comment.
    let literal = "def s := \"\n/-- colour -/\n\"\n";
    for form in [
        "dbg_trace \"{'\"'}\"; pure ()",
        "throwNamedError lean.unknown \"{'\"'}\"",
        "logNamedError lean.unknown \"{'\"'}\"",
        "logNamedWarning lean.unknown \"{'\"'}\"",
        "throwNamedErrorAt stx lean.unknown \"{'\"'}\"",
        "logNamedErrorAt (f x) lean.unknown \"{'\"'}\"",
        "logNamedWarningAt stx[0] lean.unknown \"{'\"'}\"",
    ] {
        let source = format!("def h : IO Unit := do\n  {form}\n{literal}");
        assert_eq!(normalize(&source), source, "{form}");
    }
}

#[test]
fn escaped_identifiers_may_contain_comment_delimiters() {
    let source = "def «x/--y» := 1\ndef «a--b» := 1 /--   doc   -/\n";
    assert_eq!(
        normalize(source),
        "def «x/--y» := 1\ndef «a--b» := 1 /-- doc -/\n"
    );
}

#[test]
fn a_prime_ends_an_identifier_before_a_character_literal() {
    // `x'` is one identifier and `'"'` a character literal, so no string opens.
    let source = "def f := g x' '\"'\n/--   doc   -/\ndef h := \"x\"\n";
    assert_eq!(
        normalize(source),
        "def f := g x' '\"'\n/-- doc -/\ndef h := \"x\"\n"
    );
}

#[test]
fn a_slash_after_a_doc_opener_does_not_close_it() {
    assert_eq!(
        normalize("def x := 1\n/--/ doc -/\ndef y := 1\n"),
        "def x := 1\n/-- / doc -/\ndef y := 1\n"
    );
}
