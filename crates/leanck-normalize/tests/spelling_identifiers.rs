use leanck_normalize::{FrontmatterOptions, Options, SpellingDirection, normalize};

fn apply(source: &str, spelling: SpellingDirection) -> String {
    let options = Options {
        frontmatter: FrontmatterOptions {
            enabled: false,
            ..Default::default()
        },
        spelling: Some(spelling),
        ..Default::default()
    };
    normalize(source, 100, options, 2026).unwrap().output
}

#[test]
fn dotted_and_primed_names_in_doc_prose_keep_their_spelling() {
    let source =
        "/-- Use Colour.red with foo.normalise, colour', and .centre; the colour. -/\ndef x := 1\n";
    let expected =
        "/-- Use Colour.red with foo.normalise, colour', and .centre; the color. -/\ndef x := 1\n";
    assert_eq!(
        apply(source, SpellingDirection::BritishToAmerican),
        expected
    );
    assert_eq!(
        apply(expected, SpellingDirection::BritishToAmerican),
        expected
    );
}

#[test]
fn a_quote_closing_a_phrase_does_not_prime_its_last_word() {
    let source = "/-- We say 'the colour' here, but f colour' x is a name. -/\ndef x := 1\n";
    let expected = "/-- We say 'the color' here, but f colour' x is a name. -/\ndef x := 1\n";
    assert_eq!(
        apply(source, SpellingDirection::BritishToAmerican),
        expected
    );
}

#[test]
fn builtin_entries_wrong_in_mathematical_prose_are_not_applied() {
    let source =
        "/-- The gauge of a tonne; a cancellation. Program the meter on the disk. -/\ndef x := 1\n";
    assert_eq!(apply(source, SpellingDirection::BritishToAmerican), source);
    assert_eq!(apply(source, SpellingDirection::AmericanToBritish), source);
    let source = "/-- The cancelled programme. -/\ndef x := 1\n";
    assert_eq!(
        apply(source, SpellingDirection::BritishToAmerican),
        "/-- The canceled program. -/\ndef x := 1\n"
    );
}
