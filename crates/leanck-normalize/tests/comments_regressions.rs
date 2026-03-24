//! Regressions from stress-testing comment normalization: crashes, math,
//! `#guard_msgs` text, comment layouts Lean reads differently, blank lines,
//! opt-in prose rules, and idempotence.
use leanck_config::{DelimiterStyle, LineEndings, NormalizeRules};
use leanck_normalize::{CommentKind, Context, NormalizeError, Options, normalize_with_context};

fn try_run(
    source: &str,
    width: usize,
    options: Options,
    rules: &NormalizeRules,
) -> Result<String, NormalizeError> {
    normalize_with_context(source, width, options, rules, &Context::default())
        .map(|result| result.output)
}

/// Normalize, asserting that normalizing the result again changes nothing.
fn run(source: &str, width: usize, options: Options, rules: &NormalizeRules) -> String {
    let once = try_run(source, width, options, rules).unwrap();
    let twice = try_run(&once, width, options, rules).unwrap();
    assert_eq!(twice, once, "normalizing {source:?} again changed it");
    once
}

fn default(source: &str, width: usize) -> String {
    run(
        source,
        width,
        Options::default(),
        &NormalizeRules::default(),
    )
}

fn without_attributes() -> Options {
    Options {
        attributes: false,
        ..Options::default()
    }
}

// ---------------------------------------------------------------------------
// Crashes
// ---------------------------------------------------------------------------

#[test]
fn non_ascii_whitespace_is_not_indentation() {
    assert_eq!(default("/--\u{a0}\n x -/\n", 100), "/-- \u{a0} x -/\n");
    assert_eq!(
        default("/--\n\u{3000}x\n  y -/\n", 100),
        "/-- \u{3000}x y -/\n"
    );
}

#[test]
fn a_slash_after_the_opening_delimiter_does_not_close_it() {
    // Lean scans for `-/` only after the third character, like `/--/ 1`.
    let rules = NormalizeRules::default();
    let error = try_run("def f := /--/ 1\n", 100, Options::default(), &rules).unwrap_err();
    assert_eq!(
        error,
        NormalizeError::UnmatchedOpeningDelimiter {
            kind: CommentKind::Block,
            line: 1
        }
    );
    let error = try_run("def f := /-!/ 1\n", 100, Options::default(), &rules).unwrap_err();
    assert_eq!(
        error,
        NormalizeError::UnmatchedOpeningDelimiter {
            kind: CommentKind::Doc,
            line: 1
        }
    );
    assert_eq!(
        default("/--/ ends here -/\ndef f := 1\n", 100),
        "/-- / ends here -/\ndef f := 1\n"
    );
}

#[test]
fn comments_whose_markdown_renders_to_a_bare_marker_keep_a_body() {
    assert_eq!(default("/-- - -/\n", 100), "/-- * -/\n");
    assert_eq!(default("/-- * -/\n", 100), "/-- * -/\n");
    assert_eq!(default("/-- 1. -/\n", 100), "/-- 1. -/\n");
    assert_eq!(default("/----/\n", 100), "/-- * -/\n");
    assert_eq!(default("/-- * -/\r\n", 100), "/-- * -/\n");
}

#[test]
fn markdown_parser_panics_leave_the_comment_unchanged() {
    // markdown-rs reaches `unreachable!` on this input.
    let source = "/-- * $$\n1) x -/\n";
    assert_eq!(default(source, 100), source);
}

// ---------------------------------------------------------------------------
// Math
// ---------------------------------------------------------------------------

#[test]
fn inline_math_with_backticks_and_dollars_is_restored() {
    let source = "/-- Inline $x`y$ and $a$b$ text -/\n";
    assert_eq!(default(source, 100), source);
}

#[test]
fn inline_math_counts_its_own_width() {
    // The first line is exactly 40 columns.
    assert_eq!(
        default("/-- aaaa $a + b$ cccc $c * d$ eeee fffff gggg -/\n", 40),
        "/-- aaaa $a + b$ cccc $c * d$ eeee fffff\ngggg -/\n"
    );
}

#[test]
fn inline_math_across_lines_is_joined() {
    assert_eq!(
        default("/-- Text with math $a +\n    b$ across lines -/\n", 40),
        "/-- Text with math $a + b$ across\nlines -/\n"
    );
}

#[test]
fn unclosed_display_math_leaves_the_comment_unchanged() {
    // `$$,` opens display math that runs to the end of the comment.
    let source = "/--\nThe norm is\n$$,\nwhere x is positive.\n-/\n";
    assert_eq!(default(source, 100), source);
}

#[test]
fn indented_display_math_is_restored_inside_its_list_item() {
    let source = "/--\n* item\n\n  $$\n  x = y\n  $$\n\n* next\n-/\n";
    assert_eq!(
        default(source, 100),
        "/--\n* item\n\n  $$\n  x = y\n  $$\n\n* next -/\n"
    );
}

// ---------------------------------------------------------------------------
// Exact text and fences
// ---------------------------------------------------------------------------

#[test]
fn guard_msgs_expected_output_is_left_byte_identical() {
    let rules = NormalizeRules::default();
    let options = Options {
        capitalize_after_colon: true,
        linkify_urls: true,
        headings: true,
        ..Options::default()
    };
    for source in [
        "/-- info: _x_ ?_ colour: see https://example.com\n\n\n---\nwarning: `` `u` `` -/\n#guard_msgs in\n#eval 1\n",
        "/--\ninfo: | a | b |\n|---|---|\n| \\| | 2 |\n-/\n-- the table\n#guard_msgs in\n#eval 1\n",
        "  /-- error: a long message that is much wider than the configured width of twenty -/\n  #guard_msgs (error) in\n  #eval 1\n",
        // A directive between them ends a segment but not the docstring's role.
        "/-- info: colour\n\n\nfoo -/\n-- leanck: off attributes\n#guard_msgs in\n#eval 1\n",
        "/-- info: colour\n\n\nfoo -/\n-- leanck: off attributes\n-- note\n\n-- leanck: on attributes\n#guard_msgs in\n#eval 1\n",
    ] {
        assert_eq!(run(source, 20, options, &rules), source);
    }
}

#[test]
fn fences_never_share_a_delimiter_line() {
    // Verso doc comments require fences at their own columns.
    let source = "/--\n```lean\ndef x := 1\n```\n-/\ndef y := 1\n";
    assert_eq!(default(source, 100), source);
    assert_eq!(
        default("/-- Example:\n```lean\ndef x := 1\n``` -/\n", 100),
        "/-- Example:\n\n```lean\ndef x := 1\n```\n-/\n"
    );
    let no_reflow = Options {
        comment_reflow: false,
        ..Options::default()
    };
    assert_eq!(
        run(
            "/-- ```lean\ndef x := 1\n``` -/\n",
            100,
            no_reflow,
            &NormalizeRules::default()
        ),
        "/--\n```lean\ndef x := 1\n```\n-/\n"
    );
}

// ---------------------------------------------------------------------------
// Layouts Lean reads differently
// ---------------------------------------------------------------------------

#[test]
fn continuation_lines_after_a_mid_line_opener_use_the_line_indentation() {
    let options = without_attributes();
    let rules = NormalizeRules::default();
    assert_eq!(
        run(
            "@[to_dual /-- A long docstring that will wrap around the width limit -/]\ntheorem t : True := trivial\n",
            40,
            options,
            &rules
        ),
        "@[to_dual /-- A long docstring that will\nwrap around the width limit -/]\ntheorem t : True := trivial\n"
    );
    assert_eq!(
        run(
            "  #adaptation_note /-- A note that wraps around the width.\n\n  Second paragraph. -/\n",
            40,
            options,
            &rules
        ),
        "  #adaptation_note /-- A note that wraps\n  around the width.\n\n  Second paragraph. -/\n"
    );
}

#[test]
fn delimiter_column_continuations_become_paragraphs_not_code() {
    // Lean removes the indentation shared by the later lines, so the second
    // paragraph is prose; it must not turn into an indented code block.
    let source = "library_note \"x\" /-- First paragraph.\n\n                  Second paragraph at the delimiter column. -/\n";
    assert_eq!(
        default(source, 100),
        "library_note \"x\" /-- First paragraph.\n\nSecond paragraph at the delimiter column. -/\n"
    );
}

#[test]
fn compact_layout_keeps_list_nesting() {
    assert_eq!(
        default("/--\n* a\n  * nested\n* b\n-/\n", 100),
        "/--\n* a\n  * nested\n* b -/\n"
    );
    assert_eq!(
        default("/--\n* a\n  * nested\n-/\n", 100),
        "/--\n* a\n  * nested -/\n"
    );
    // A list item on the opening line may keep unindented siblings there.
    assert_eq!(default("/--\n* a\n* b\n-/\n", 100), "/-- * a\n* b -/\n");
}

#[test]
fn comments_lean_reads_differently_from_their_layout_are_left_alone() {
    // The space after `/--` moves the item's content column, so Lean reads
    // `nested` as a sibling here; reformatting would change which one.
    let source = "/-- * a\n  * nested\n* b -/\n";
    assert_eq!(default(source, 100), source);
}

#[test]
fn nested_bullets_with_code_converge_in_one_pass() {
    // From Mathlib/Tactic/ApplyWith.lean.
    let source = "/--\n* `apply (config := cfg) e` allows for additional configuration (see `Lean.Meta.ApplyConfig`):\n  * `newGoals` controls which new goals are added by `apply`, in which order.\n  * `+approx` enables `isDefEq` approximations (see `Lean.Meta.approxDefEq`).\n-/\n";
    assert_eq!(
        default(source, 100),
        "/--\n* `apply (config := cfg) e` allows for additional configuration (see\n  `Lean.Meta.ApplyConfig`):\n  * `newGoals` controls which new goals are added by `apply`, in which order.\n  * `+approx` enables `isDefEq` approximations (see `Lean.Meta.approxDefEq`). -/\n"
    );
}

// ---------------------------------------------------------------------------
// Blank lines and line endings
// ---------------------------------------------------------------------------

#[test]
fn zero_blank_lines_keep_markdown_blocks_apart_inside_doc_comments() {
    let mut rules = NormalizeRules::default();
    rules.whitespace.max_blank_lines = 0;
    let source =
        "/-- a\n\nb\n\n| x | y |\n|---|---|\n| 1 | 2 |\n\nc -/\ndef x := 1\n\n\ndef y := 1\n";
    assert_eq!(
        run(source, 100, Options::default(), &rules),
        "/-- a\n\nb\n\n| x   | y   |\n| --- | --- |\n| 1   | 2   |\n\nc -/\ndef x := 1\ndef y := 1\n"
    );
}

#[test]
fn trailing_blank_lines_are_removed_in_one_pass() {
    let mut rules = NormalizeRules::default();
    for max in [1, 2, 3] {
        rules.whitespace.max_blank_lines = max;
        assert_eq!(
            run("def a := 1\n\n\n\n\n", 100, Options::default(), &rules),
            "def a := 1\n"
        );
    }
}

#[test]
fn lone_carriage_returns_settle_in_one_pass() {
    assert_eq!(default("def a := 1\r", 100), "def a := 1\n");
    assert_eq!(
        default("def a := 1\r\r\ndef b := 2\n", 100),
        "def a := 1\ndef b := 2\n"
    );
    // The final line ending replaces a lone carriage return in every style.
    for (style, source, expected) in [
        (LineEndings::Crlf, "def b := 2\r", "def b := 2\r\n"),
        (LineEndings::Off, "def b := 2\r", "def b := 2\n"),
        (
            LineEndings::Off,
            "def a := 1\r\ndef b := 2\r",
            "def a := 1\r\ndef b := 2\r\n",
        ),
    ] {
        let mut rules = NormalizeRules::default();
        rules.whitespace.line_endings = style;
        assert_eq!(run(source, 100, Options::default(), &rules), expected);
    }
}

#[test]
fn unicode_spaces_are_not_blank_lines() {
    let source = "def a := 1\n\u{a0}\n\n\ndef b := 1\n";
    assert_eq!(default(source, 100), "def a := 1\n\u{a0}\n\ndef b := 1\n");
}

// ---------------------------------------------------------------------------
// Opt-in prose rules
// ---------------------------------------------------------------------------

#[test]
fn capitalize_after_colon_leaves_single_letters() {
    let options = Options {
        capitalize_after_colon: true,
        ..Options::default()
    };
    assert_eq!(
        run(
            "/-- f: a → b, x: y, note: this works, see [$f: a$](u) -/\n",
            100,
            options,
            &NormalizeRules::default()
        ),
        "/-- f: a → b, x: y, note: This works, see [$f: a$](u) -/\n"
    );
}

#[test]
fn title_case_leaves_single_letter_variables() {
    let options = Options {
        headings: true,
        ..Options::default()
    };
    let rules = NormalizeRules::default();
    assert_eq!(
        run(
            "/-! # results for x in a set -/\n/-! ## a heading for x -/\n",
            100,
            options,
            &rules
        ),
        "/-! # Results for x in a Set -/\n/-! ## A Heading for x -/\n"
    );
    assert_eq!(
        run("-- ====\n-- lemmas for x\n-- ====\n", 100, options, &rules),
        "/-! ## Lemmas for x -/\n"
    );
    // A leading small word is capitalized; later ones stay lowercase.
    assert_eq!(
        run(
            "-- ====\n-- the expectation of the gamma distribution\n-- ====\n",
            100,
            options,
            &rules
        ),
        "/-! ## The Expectation of the Gamma Distribution -/\n"
    );
}

#[test]
fn linkify_without_reflow_only_adds_link_syntax() {
    let mut rules = NormalizeRules::default();
    rules.comments.module = DelimiterStyle::Off;
    rules.comments.declaration = DelimiterStyle::Off;
    let options = Options {
        comment_reflow: false,
        linkify_urls: true,
        ..Options::default()
    };
    // Rendering the raw body as Markdown once turned this into a code fence.
    assert_eq!(run("/-!\tn-/\n", 100, options, &rules), "/-!\tn-/\n");
    assert_eq!(
        run(
            "/--   - See https://x.test/a_(b)\n    - and *https://y.test* or ![i](https://z.test) -/\n",
            100,
            options,
            &rules
        ),
        "/--   - See [https://x.test/a_(b)](https://x.test/a_(b))\n    - and *https://y.test* or ![i](https://z.test) -/\n"
    );
    // Lines too long for the width are neither wrapped nor joined.
    assert_eq!(
        run(
            "/--\nSee https://example.test\nwithout joining this line. -/\n",
            10,
            options,
            &rules
        ),
        "/--\nSee [https://example.test](https://example.test)\nwithout joining this line. -/\n"
    );
}

// ---------------------------------------------------------------------------
// Idempotence
// ---------------------------------------------------------------------------

const MIXED: &str = r#"/-!
# Module

Some prose with $x_1 * y_1$ math, `code`, and a [link](https://example.com).
* item one
  * nested item that is long enough to wrap at narrow widths for sure
* item two

| a | b |
|---|---|
| 1 | 2 |

$$
\sum_i x_i
$$
-/

/-- Doc with a long first line that keeps going and going until it wraps at least once. -/
def f := 1

@[simp] theorem t : True := trivial -- trailing

namespace N
  /-- Indented doc comment with `code` and $math$ that also wraps around the width. -/
  def g := 1

  #adaptation_note /-- Adaptation note text that wraps
  onto the next line.

  More text. -/
end N

/-- a /- b -/ c -/
def h := 1
/-- info: exact
text -/
#guard_msgs in
#eval 1
"#;

#[test]
fn mixed_files_are_idempotent_across_layouts_and_widths() {
    for width in [40, 80, 100] {
        for reflow in [true, false] {
            for style in [
                DelimiterStyle::Compact,
                DelimiterStyle::Separate,
                DelimiterStyle::Off,
            ] {
                let mut rules = NormalizeRules::default();
                rules.comments.declaration = style;
                rules.comments.section = style;
                rules.comments.module = style;
                let options = Options {
                    comment_reflow: reflow,
                    ..Options::default()
                };
                let output = run(MIXED, width, options, &rules);
                assert!(output.contains("/-- info: exact\ntext -/\n#guard_msgs in"));
            }
        }
    }
}
