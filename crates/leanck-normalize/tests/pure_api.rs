use leanck_normalize::{
    CommentKind, FrontmatterOptions, NormalizeError, Options, SpellingDirection, normalize,
};

#[test]
fn explicit_year_options_and_statistics_are_reproducible() {
    let source = "module\nimport Zed\nimport Alpha\n\n/-! the colour: blue -/\n\n@[simp]\ndef example := 1\n\n\n";
    let first = normalize(source, 100, Options::default(), 2032).unwrap();
    let repeated = normalize(source, 100, Options::default(), 2032).unwrap();
    assert_eq!(first, repeated);
    assert!(
        !first.output.contains("Copyright"),
        "pure calls need explicit attribution context"
    );
    assert!(first.output.contains("import Alpha\nimport Zed"));
    assert!(first.output.contains("color: blue"));
    assert!(first.output.contains("@[simp] def example"));
    assert_eq!(first.stats.frontmatter_changes, 1);
    assert_eq!(first.stats.attributes_moved, 1);
    let second = normalize(&first.output, 100, Options::default(), 2099).unwrap();
    assert_eq!(
        second.output, first.output,
        "existing copyright is preserved"
    );
    // Stats describe each transformation pass. Frontmatter may adjust spacing
    // which comment reflow restores; the final source still remains unchanged.
    assert_eq!(second.stats.attributes_moved, 0);
    assert_eq!(second.stats.comment_blocks_changed, 0);
}

#[test]
fn pure_options_can_disable_transformations_without_command_parsing() {
    let options = Options {
        frontmatter: FrontmatterOptions {
            enabled: false,
            ..Default::default()
        },
        attributes: false,
        spelling: Some(SpellingDirection::AmericanToBritish),
        ..Default::default()
    };
    let result = normalize(
        "/-- the color of this center -/\n@[simp]\ndef x := 1\n",
        100,
        options,
        2000,
    )
    .unwrap();
    assert!(result.output.contains("colour"));
    assert!(result.output.contains("centre"));
    assert!(result.output.contains("@[simp]\ndef"));
    assert_eq!(result.stats.frontmatter_changes, 0);
    assert_eq!(result.stats.attributes_moved, 0);
    assert_eq!(result.stats.comment_blocks_changed, 1);
    assert_eq!(
        normalize("/-! unclosed", 100, options, 2000).unwrap_err(),
        NormalizeError::UnmatchedOpeningDelimiter {
            kind: CommentKind::Doc,
            line: 1,
        }
    );
    let error = normalize("source", 0, options, 2000).unwrap_err();
    assert_eq!(error, NormalizeError::InvalidWidth);
    assert_eq!(error.to_string(), "width must be greater than zero");
}

#[test]
fn unmatched_delimiters_preserve_kind_line_and_message() {
    let options = Options {
        frontmatter: FrontmatterOptions {
            enabled: false,
            ..Default::default()
        },
        ..Default::default()
    };
    for (delimiter, kind) in [("/-!", CommentKind::Doc), ("/--", CommentKind::Block)] {
        let source = format!("def x := 1\n\n{delimiter} unclosed\nmore text\n");
        let error = normalize(&source, 100, options, 2000).unwrap_err();
        assert_eq!(
            error,
            NormalizeError::UnmatchedOpeningDelimiter { kind, line: 3 }
        );
        assert_eq!(
            error.to_string(),
            format!("unmatched opening delimiter '{delimiter}' at line 3 (no closing '-/')")
        );
    }
}
