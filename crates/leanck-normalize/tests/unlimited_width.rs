use leanck_config::{AttributePlacement, DelimiterStyle};
use leanck_normalize::{
    Context, FrontmatterOptions, NormalizeRules, NormalizeWidth, Options, normalize,
    normalize_with_context,
};

fn options() -> Options {
    Options {
        frontmatter: FrontmatterOptions {
            enabled: false,
            ..Default::default()
        },
        spelling: None,
        collapse_blank_lines: false,
        ..Default::default()
    }
}

fn apply(source: &str, options: Options, rules: &NormalizeRules) -> String {
    let output = normalize_with_context(
        source,
        NormalizeWidth::Unlimited,
        options,
        rules,
        &Context::default(),
    )
    .unwrap()
    .output;
    let repeated = normalize_with_context(
        &output,
        NormalizeWidth::Unlimited,
        options,
        rules,
        &Context::default(),
    )
    .unwrap()
    .output;
    assert_eq!(
        output, repeated,
        "unlimited normalization must be idempotent"
    );
    output
}

#[test]
fn unlimited_joins_soft_breaks_and_keeps_delimiter_policies() {
    let source = "/-- First part\ncontinues here. -/\n";
    let mut rules = NormalizeRules::default();
    for (style, expected) in [
        (
            DelimiterStyle::Compact,
            "/-- First part continues here. -/\n",
        ),
        (
            DelimiterStyle::Separate,
            "/--\nFirst part continues here.\n-/\n",
        ),
        (DelimiterStyle::Off, "/-- First part continues here. -/\n"),
    ] {
        rules.comments.declaration = style;
        assert_eq!(apply(source, options(), &rules), expected);
    }
    assert_eq!(
        apply(
            "/-! First part\ncontinues here. -/\n",
            options(),
            &NormalizeRules::default()
        ),
        "/-!\nFirst part continues here.\n-/\n"
    );
    // The legacy pure entry point accepts the same typed width.
    assert_eq!(
        normalize(source, NormalizeWidth::Unlimited, options(), 2026)
            .unwrap()
            .output,
        "/-- First part continues here. -/\n"
    );
}

#[test]
fn unlimited_preserves_markdown_structure_code_math_and_hard_breaks() {
    let source = "/--\nA paragraph with `inline code` and $x + y$\ncontinues without wrapping.\n\nA hard break.  \nThe next line.\n\n- First item\n  continues here.\n- Second item\n\n```lean\ndef colour := 1\n#check colour\n```\n\n$$\nx + y\n= z\n$$\n-/\n";
    let output = apply(source, options(), &NormalizeRules::default());
    for preserved in [
        "A paragraph with `inline code` and $x + y$ continues without wrapping.",
        "A hard break.\\\nThe next line.",
        "* First item continues here.\n* Second item",
        "```lean\ndef colour := 1\n#check colour\n```",
        "$$\nx + y\n= z\n$$",
    ] {
        assert!(
            output.contains(preserved),
            "missing {preserved:?} in {output}"
        );
    }
}

#[test]
fn unlimited_does_not_disable_the_no_reflow_switch() {
    let source = "/-- First part\ncontinues here. -/\n";
    let options = Options {
        comment_reflow: false,
        ..options()
    };
    assert_eq!(apply(source, options, &NormalizeRules::default()), source);
}

#[test]
fn unlimited_applies_to_indented_comments_and_attribute_length_checks() {
    let long = "Long prose. ".repeat(30);
    let source = format!("namespace Demo\n  /-- {long}\n  The continuation. -/\nend Demo\n");
    let expected = format!("namespace Demo\n  /-- {long}The continuation. -/\nend Demo\n");
    assert_eq!(
        apply(&source, options(), &NormalizeRules::default()),
        expected
    );

    let mut rules = NormalizeRules::default();
    rules.attributes.above_when_long = true;
    let name = "long_name_".repeat(30);
    let source = format!("@[simp]\ntheorem {name} : True := trivial\n");
    let joined = format!("@[simp] theorem {name} : True := trivial\n");
    assert_eq!(apply(&source, options(), &rules), joined);
    let finite =
        normalize_with_context(&source, 100, options(), &rules, &Context::default()).unwrap();
    assert_eq!(finite.output, source);
    rules.attributes.placement = AttributePlacement::Above;
    assert_eq!(apply(&joined, options(), &rules), source);
}
