use leanck_config::DelimiterStyle;
use leanck_normalize::{Context, NormalizeRules, Options, normalize_with_context};

fn apply(source: &str, rules: &NormalizeRules) -> String {
    normalize_with_context(
        source,
        60,
        Options {
            comment_reflow: false,
            ..Default::default()
        },
        rules,
        &Context::default(),
    )
    .unwrap()
    .output
}

#[test]
fn delimiter_layout_without_reflow_has_no_padding_and_is_idempotent() {
    let paragraph = "This paragraph is deliberately longer than the configured width, and keeps its original line breaks when reflow is disabled.";
    let expected = format!(
        "/-!\n# Module\n\n{paragraph}\n-/\n\n/-- # Result\n\n{paragraph}\nSee the reference. -/\ntheorem demo : True := by trivial\n"
    );
    let rules = NormalizeRules::default();
    for module_open in ["/-! # Module", "/-!\n# Module", "/-!\n\n# Module"] {
        for declaration_open in ["/-- # Result", "/--\n# Result", "/-- \n\n# Result"] {
            let source = format!(
                "{module_open}\n\n{paragraph}\n-/\n\n{declaration_open}\n\n{paragraph}\nSee the reference. -/\ntheorem demo : True := by trivial\n"
            );
            let output = apply(&source, &rules);
            assert_eq!(output, expected);
            assert!(output.lines().all(|line| line.trim_end() == line));
            assert_eq!(apply(&output, &rules), output);
        }
    }
}

#[test]
fn no_reflow_keeps_markdown_breaks_lists_math_and_code() {
    let source = "  /--\n  # Result\n\n  A line with a Markdown hard break.  \n  The next line keeps `colour` and $x + y$.\n\n  - First item\n    continuation\n\n  ```lean\n  def colour := 1  \n\n\n  #check colour\n  ```\n  -/\n";
    // A closing fence keeps its own line; Verso requires fences at their column.
    let expected = "  /-- # Result\n\n  A line with a Markdown hard break.  \n  The next line keeps `colour` and $x + y$.\n\n  - First item\n    continuation\n\n  ```lean\n  def colour := 1  \n\n\n  #check colour\n  ```\n  -/\n";
    let rules = NormalizeRules::default();
    assert_eq!(apply(source, &rules), expected);
    assert_eq!(apply(expected, &rules), expected);
}

#[test]
fn no_reflow_keeps_indented_code_at_the_start_of_a_comment() {
    let rules = NormalizeRules::default();
    // Lean removes the indentation shared by the lines after the first, so a
    // comment whose only line is indented is a paragraph, not code.
    let source = "/--\n    #check colour\n-/\n";
    let expected = "/-- #check color -/\n";
    assert_eq!(apply(source, &rules), expected);
    assert_eq!(apply(expected, &rules), expected);
    // Code that Lean keeps indented stays code on its own line.
    let source = "/--\n    #check colour\nis an example\n-/\n";
    let expected = "/--\n    #check colour\nis an example -/\n";
    assert_eq!(apply(source, &rules), expected);
    assert_eq!(apply(expected, &rules), expected);
}

#[test]
fn disabled_layout_preserves_comment_whitespace_during_prose_edits() {
    let mut rules = NormalizeRules::default();
    rules.comments.module = DelimiterStyle::Off;
    rules.comments.declaration = DelimiterStyle::Off;
    let source = "/-!   # Module\n\ncolour\n-/\n\n  /--\n  colour\n  -/\n";
    let expected = source.replace("colour", "color");
    assert_eq!(apply(source, &rules), expected);
    assert_eq!(apply(&expected, &rules), expected);
}
