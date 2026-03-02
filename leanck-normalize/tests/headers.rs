use leanck_config::InvalidExistingHeader as Policy;
use leanck_normalize::{
    Context, NormalizeResult, NormalizeRules, Options,
    history::{Contributor, FileHistory},
    normalize_with_context,
};

fn history() -> FileHistory {
    FileHistory {
        contributors: vec![
            Contributor {
                name: "Human".into(),
                email: "human@example.test".into(),
                years: [2020].into(),
            },
            Contributor {
                name: "Codex".into(),
                email: "noreply@openai.com".into(),
                years: [2026].into(),
            },
        ],
    }
}
fn apply(source: &str, rules: &NormalizeRules, history: Option<&FileHistory>) -> NormalizeResult {
    normalize_with_context(
        source,
        100,
        Options::default(),
        rules,
        &Context {
            history,
            ..Default::default()
        },
    )
    .unwrap()
}

#[test]
fn policies_warn_ignore_or_replace_mismatching_headers() {
    let source = "/- Old attribution and old license. -/\n\nmodule\n";
    let history = history();
    for policy in [Policy::Warn, Policy::Ignore, Policy::Replace] {
        let mut rules = NormalizeRules::default();
        rules.frontmatter.normalize_existing = false;
        rules.frontmatter.invalid_existing = policy;
        rules.frontmatter.license_text = "Configured license.".into();
        let result = apply(source, &rules, Some(&history));
        match policy {
            Policy::Warn => {
                assert_eq!(result.output, source);
                assert_eq!(result.warnings.len(), 1);
                assert_eq!(result.warnings[0].code, "header_template_mismatch");
            }
            Policy::Ignore => {
                assert_eq!(result.output, source);
                assert!(result.warnings.is_empty());
            }
            Policy::Replace => {
                assert!(!result.output.contains("Old attribution"));
                assert!(result.output.contains("Copyright (c) 2020 Human."));
                assert!(result.output.contains("Authors: Human"));
                assert!(result.output.contains("Configured license."));
                assert!(!result.output.contains("Codex") && !result.output.contains("2026"));
                assert!(result.warnings.is_empty());
                let twice = apply(&result.output, &rules, None);
                assert_eq!(twice.output, result.output);
                assert!(twice.warnings.is_empty());
            }
        }
    }
}

#[test]
fn ignoring_mismatches_still_allows_independent_layout_normalization() {
    let mut rules = NormalizeRules::default();
    rules.frontmatter.invalid_existing = Policy::Ignore;
    let result = apply("/- Old attribution. -/\n\nmodule\n", &rules, None);
    assert!(result.output.starts_with("/-\nOld attribution.\n-/"));
    assert!(result.warnings.is_empty());
}

#[test]
fn replace_preserves_conforming_attribution_even_when_history_has_changed() {
    let source =
        "/-\nCopyright (c) 1999 Original Author.\nAuthors: Original Author\n-/\n\nmodule\n";
    let mut rules = NormalizeRules::default();
    rules.frontmatter.invalid_existing = Policy::Replace;
    for history in [None, Some(history())] {
        let result = apply(source, &rules, history.as_ref());
        assert_eq!(result.output, source);
        assert!(result.warnings.is_empty());
    }
}

#[test]
fn replacement_without_retained_history_warns_and_keeps_the_existing_header() {
    let source = "\n/- Existing attribution and license. -/\n\nmodule\n";
    let mut rules = NormalizeRules::default();
    rules.frontmatter.invalid_existing = Policy::Replace;
    rules.frontmatter.normalize_existing = false;
    rules.frontmatter.ignore_authors.push("Human".into());
    for history in [None, Some(FileHistory::default()), Some(history())] {
        let result = apply(source, &rules, history.as_ref());
        assert!(
            result
                .output
                .contains("/- Existing attribution and license. -/")
        );
        assert_eq!(result.warnings.len(), 1);
        assert_eq!(result.warnings[0].code, "header_replacement_unavailable");
        assert_eq!(result.warnings[0].line, Some(2));
    }
}

#[test]
fn replacement_respects_master_switches_directives_and_nested_header_boundaries() {
    let source = "/- Old /- nested -/ attribution. -/\n\nmodule\n";
    let history = history();
    let mut rules = NormalizeRules::default();
    rules.frontmatter.invalid_existing = Policy::Replace;
    rules.frontmatter.normalize_existing = false;
    for disabled in ["header", "frontmatter"] {
        let mut options = Options::default();
        if disabled == "header" {
            options.frontmatter.header = false;
        } else {
            options.frontmatter.enabled = false;
        }
        let result = normalize_with_context(
            source,
            100,
            options,
            &rules,
            &Context {
                history: Some(&history),
                ..Default::default()
            },
        )
        .unwrap();
        assert_eq!(result.output, source);
        assert!(result.warnings.is_empty());
    }
    for command in ["off", "off frontmatter"] {
        let source = format!("-- leanck: {command}\n{source}");
        let result = apply(&source, &rules, Some(&history));
        assert_eq!(result.output, source);
        assert!(result.warnings.is_empty());
    }
    let result = apply(source, &rules, Some(&history));
    assert!(!result.output.contains("nested") && !result.output.contains("attribution."));
    assert!(result.output.contains("Copyright (c) 2020 Human."));
    assert_eq!(result.output.matches("-/").count(), 1);
}

#[test]
fn all_policies_still_generate_missing_headers() {
    let history = history();
    for policy in [Policy::Replace, Policy::Warn, Policy::Ignore] {
        let mut rules = NormalizeRules::default();
        rules.frontmatter.invalid_existing = policy;
        let result = apply("module\n", &rules, Some(&history));
        assert!(result.output.contains("Copyright (c) 2020 Human."));
        assert!(result.warnings.is_empty());
    }
}
