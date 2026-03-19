use leanck_config::DelimiterStyle as Style;
use leanck_normalize::{
    Context, FrontmatterOptions, NormalizeRules, Options, normalize_with_context,
};

fn options() -> Options {
    Options {
        frontmatter: FrontmatterOptions {
            enabled: false,
            ..Default::default()
        },
        spelling: None,
        ..Default::default()
    }
}

fn apply(source: &str, options: Options, rules: &NormalizeRules) -> String {
    let output = normalize_with_context(source, 100, options, rules, &Context::default())
        .unwrap()
        .output;
    let repeated = normalize_with_context(&output, 100, options, rules, &Context::default())
        .unwrap()
        .output;
    assert_eq!(output, repeated, "heading layout must settle in one pass");
    output
}

#[test]
fn heading_only_layout_is_independent_of_level_position_and_existing_layout() {
    let mut rules = NormalizeRules::default();
    rules.comments.module = Style::Separate;
    rules.comments.section = Style::Separate;
    rules.comments.heading = Some(Style::Compact);
    for level in 1..=6 {
        let title = format!("{} Heading", "#".repeat(level));
        for prefix in ["", "def before := 1\n\n"] {
            for comment in [
                format!("/-! {title} -/\n"),
                format!("/-!\n{title}\n-/\n"),
                format!("/-!\n\n{title}\n\n-/\n"),
            ] {
                for reflow in [false, true] {
                    let options = Options {
                        comment_reflow: reflow,
                        ..options()
                    };
                    assert_eq!(
                        apply(&format!("{prefix}{comment}"), options, &rules),
                        format!("{prefix}/-! {title} -/\n")
                    );
                }
            }
        }
    }
}

#[test]
fn headings_with_a_body_use_documentation_layout_and_declarations_keep_their_rule() {
    let mut rules = NormalizeRules::default();
    rules.comments.heading = Some(Style::Compact);
    rules.comments.module = Style::Separate;
    rules.comments.section = Style::Separate;
    for body in [
        "# Heading\n\nSome documentation",
        "## Heading\n\n- A list item",
        "# Heading\n\n```lean\n#check Nat\n```",
        "# First heading\n\n## Second heading",
        "> # A quoted heading",
        "- # A heading inside a list",
        "# Heading\n\n    # An indented code block",
    ] {
        let comment = format!("/-!\n{body}\n-/\n");
        for prefix in ["", "def before := 1\n\n"] {
            let output = apply(&format!("{prefix}{comment}"), options(), &rules);
            assert!(output.starts_with(&format!("{prefix}/-!\n")), "{output}");
            assert!(output.ends_with("\n-/\n"), "{output}");
        }
    }
    rules.comments.heading = Some(Style::Separate);
    assert_eq!(
        apply("/-- # Declaration heading -/\n", options(), &rules),
        "/-- # Declaration heading -/\n"
    );
}

#[test]
fn omitted_heading_inherits_section_and_explicit_off_preserves_delimiter_placement() {
    let mut rules = NormalizeRules::default();
    let source = "/-! # Heading -/\n";
    for style in [Style::Compact, Style::Separate, Style::Off] {
        rules.comments.section = style;
        let inherited = apply(source, options(), &rules);
        rules.comments.heading = Some(style);
        assert_eq!(apply(source, options(), &rules), inherited);
        rules.comments.heading = None;
    }
    rules.comments.heading = Some(Style::Off);
    rules.comments.section = Style::Separate;
    for source in ["/-! # Heading -/\n", "/-!\n# Heading\n-/\n"] {
        assert_eq!(apply(source, options(), &rules), source);
    }
}

#[test]
fn heading_classification_is_shared_with_frontmatter_and_survives_markdown_rendering() {
    let mut rules = NormalizeRules::default();
    rules.comments.heading = Some(Style::Compact);
    rules.comments.section = Style::Separate;
    let options = Options {
        frontmatter: FrontmatterOptions {
            header: false,
            ..Default::default()
        },
        ..options()
    };
    for heading in ["# Heading", "Heading\n=======", "# #"] {
        let source = format!(
            "module\nimport B\nimport A\nnamespace Example\n/-!\n{heading}\n-/\ndef body := 1\nend Example\n"
        );
        let output = apply(&source, options, &rules);
        assert!(output.contains("import A\nimport B"), "{output}");
        assert!(output.contains("namespace Example\n\n/-! #"), "{output}");
    }
    // Headings generated from banners use the same layout rule even when it
    // differs from section documentation.
    let options = Options {
        headings: true,
        ..options
    };
    rules.comments.heading = Some(Style::Separate);
    rules.comments.section = Style::Compact;
    let source = "module\nimport A\nnamespace Example\n-- ====\n-- A heading\n-- ====\ndef body := 1\nend Example\n";
    let output = apply(source, options, &rules);
    assert!(
        output.contains("namespace Example\n\n/-!\n## A Heading\n-/"),
        "{output}"
    );
}
