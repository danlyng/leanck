use leanck_config::{AttributePlacement, DelimiterStyle};
use leanck_normalize::{Context, NormalizeRules, Options, normalize_with_context};

#[test]
fn linkification_without_reflow_preserves_soft_breaks_and_protects_code_and_math() {
    let source = "/-- See https://example.test/path\non the next line, `https://code.test`, $x + y$,\nand [existing](https://existing.test). -/\n";
    let options = Options {
        comment_reflow: false,
        linkify_urls: true,
        ..Default::default()
    };
    let result = normalize_with_context(
        source,
        10,
        options,
        &NormalizeRules::default(),
        &Context::default(),
    )
    .unwrap();
    assert!(
        result
            .output
            .contains("[https://example.test/path](https://example.test/path)\non the next line"),
        "{}",
        result.output
    );
    assert!(result.output.contains("`https://code.test`"));
    assert!(result.output.contains("$x + y$"));
    assert!(result.output.contains("[existing](https://existing.test)"));
    assert_eq!(result.output.lines().count(), source.lines().count());
    assert!(result.warnings.is_empty(), "{:?}", result.warnings);
    let twice = normalize_with_context(
        &result.output,
        10,
        options,
        &NormalizeRules::default(),
        &Context::default(),
    )
    .unwrap();
    assert_eq!(result.output, twice.output);
}

#[test]
fn no_reflow_linkification_also_applies_with_unchanged_delimiter_layout() {
    let mut rules = NormalizeRules::default();
    rules.comments.declaration = DelimiterStyle::Off;
    let options = Options {
        comment_reflow: false,
        linkify_urls: true,
        ..Default::default()
    };
    let source = "/--\nSee https://example.test\nwithout joining this line. -/\n";
    let result = normalize_with_context(source, 10, options, &rules, &Context::default()).unwrap();
    assert!(
        result
            .output
            .contains("[https://example.test](https://example.test)\nwithout joining this line."),
        "{}",
        result.output
    );
    assert_eq!(
        result.output,
        source.replace(
            "https://example.test",
            "[https://example.test](https://example.test)"
        )
    );
    assert_eq!(
        normalize_with_context(&result.output, 10, options, &rules, &Context::default())
            .unwrap()
            .output,
        result.output
    );
}

#[test]
fn actual_attribute_conflicts_warn_once_at_the_original_source_line_and_preserve_the_group() {
    let mut rules = NormalizeRules::default();
    rules
        .attributes
        .overrides
        .insert("ext".into(), AttributePlacement::Above);
    let source = "/- header -/\nmodule\nimport Foo\n@[simp, ext] theorem demo : True := trivial\n";
    let result =
        normalize_with_context(source, 100, Options::default(), &rules, &Context::default())
            .unwrap();
    let warnings: Vec<_> = result
        .warnings
        .iter()
        .filter(|w| w.code == "conflicting_attribute_placement")
        .collect();
    assert_eq!(warnings.len(), 1, "{:?}", result.warnings);
    assert_eq!(warnings[0].line, Some(4));
    assert!(warnings[0].message.contains("@[simp, ext]"));
    assert!(
        result
            .output
            .contains("@[simp, ext] theorem demo : True := trivial")
    );
    let options = Options {
        attributes: false,
        ..Default::default()
    };
    let result = normalize_with_context(source, 100, options, &rules, &Context::default()).unwrap();
    assert!(
        !result
            .warnings
            .iter()
            .any(|w| w.code == "conflicting_attribute_placement")
    );
}

#[test]
fn attribute_conflicts_respect_directives_and_report_original_line_offsets() {
    let mut rules = NormalizeRules::default();
    rules
        .attributes
        .overrides
        .insert("ext".into(), AttributePlacement::Above);
    let source = "-- leanck: off attributes\n@[simp, ext] theorem first : True := trivial\n-- leanck: on attributes\n@[simp] @[ext] theorem second : True := trivial\n";
    let result =
        normalize_with_context(source, 100, Options::default(), &rules, &Context::default())
            .unwrap();
    let warnings: Vec<_> = result
        .warnings
        .iter()
        .filter(|w| w.code == "conflicting_attribute_placement")
        .collect();
    assert_eq!(warnings.len(), 1, "{:?}", result.warnings);
    assert_eq!(warnings[0].line, Some(4));
}

#[test]
fn only_explicit_named_on_directives_warn_about_globally_disabled_features() {
    let source = "-- leanck: on\n-- leanck: on all\n-- leanck: on headings\n-- leanck: on frontmatter\n/-- colour -/\n";
    let result = normalize_with_context(
        source,
        100,
        Options::default(),
        &NormalizeRules::default(),
        &Context::default(),
    )
    .unwrap();
    assert_eq!(result.warnings.len(), 2, "{:?}", result.warnings);
    assert_eq!(result.warnings[0].code, "globally_disabled_directive");
    assert_eq!(result.warnings[0].line, Some(3));
    assert_eq!(result.warnings[1].code, "frontmatter_directive_barrier");
    assert_eq!(result.warnings[1].line, Some(4));
    let options = Options {
        headings: true,
        ..Default::default()
    };
    let result = normalize_with_context(
        "-- leanck: on headings\n",
        100,
        options,
        &NormalizeRules::default(),
        &Context::default(),
    )
    .unwrap();
    assert!(result.warnings.is_empty());
}

fn header_warnings(source: &str, rules: &NormalizeRules) -> Vec<leanck_config::Warning> {
    normalize_with_context(source, 100, Options::default(), rules, &Context::default())
        .unwrap()
        .warnings
        .into_iter()
        .filter(|warning| warning.code == "header_template_mismatch")
        .collect()
}

#[test]
fn existing_headers_are_checked_without_replacing_authoritative_text() {
    let rules = NormalizeRules::default();
    let valid = "/-\n  Copyright (c) 1998–2001 Original Author, Another Author.\n  Authors: Original Author, Another Author\n-/\nmodule\n";
    assert!(header_warnings(valid, &rules).is_empty());
    // Layout whitespace, CRLF, blank lines, and delimiter placement are not mismatches.
    assert!(
        header_warnings(
            "/- Copyright (c) 1998 Original Author.\r\n\r\nAuthors: Original Author -/\r\n",
            &rules
        )
        .is_empty()
    );
    for invalid in [
        "Copyright (c) 1998 Original Author.", // Missing authors field.
        "Authors: Original Author",            // Missing notice.
        "Copyright (c) 1998 Original Author.\nCopyright (c) 2001 Another Author.\nAuthors: Original Author, Another Author",
        "Authors: Original Author\nCopyright (c) 1998 Original Author.", // Wrong order.
        "Copyright (c) 1998 Original Author.\nAuthors: Original Author\nUnconfigured license.",
    ] {
        let source = format!("\n\n/-\n{invalid}\n-/\nmodule\n");
        let result = normalize_with_context(
            &source,
            100,
            Options::default(),
            &rules,
            &Context::default(),
        )
        .unwrap();
        assert_eq!(result.warnings.len(), 1, "{invalid}: {:?}", result.warnings);
        assert_eq!(result.warnings[0].code, "header_template_mismatch");
        assert_eq!(result.warnings[0].line, Some(3));
        assert!(result.output.contains(invalid));
    }
}

#[test]
fn custom_header_templates_check_literals_license_and_dynamic_fields() {
    let mut rules = NormalizeRules::default();
    rules.frontmatter.header_template =
        "/-\nMaintainers: {authors}\nYears: {first_year} to {last_year} ({years})\n{license}\n-/"
            .into();
    rules.frontmatter.license_text = "Project [license] (v1).\nKeep this {literal} text.".into();
    let valid = "/-\nMaintainers: Existing Person\nYears: 1990 to 2000 (1990–2000)\nProject [license] (v1).\nKeep this {literal} text.\n-/\n";
    assert!(header_warnings(valid, &rules).is_empty());
    for invalid in [
        valid.replace("Maintainers:", "Authors:"),
        valid.replace("(v1)", "(v2)"),
        valid.replace("1990 to", "unknown to"),
    ] {
        assert_eq!(header_warnings(&invalid, &rules).len(), 1, "{invalid}");
    }
    rules.frontmatter.header_template = "/- {authors}: {years} -/".into();
    rules.frontmatter.license_text.clear();
    assert!(header_warnings("/- Person: 1999 -/\n", &rules).is_empty());
    for template in [
        "/- {license} {authors} -/",
        "/- {authors} {license} -/",
        "/- {authors} {license} ({years}) -/",
    ] {
        rules.frontmatter.header_template = template.into();
        let header = template
            .replace("{authors}", "Person")
            .replace("{years}", "1999")
            .replace("{license}", "");
        assert!(header_warnings(&header, &rules).is_empty(), "{header}");
    }
}

#[test]
fn header_warnings_respect_master_switches_and_directive_boundaries() {
    let source = "/- Existing attribution. -/\nmodule\n";
    let mut rules = NormalizeRules::default();
    rules.frontmatter.normalize_existing = false;
    let result =
        normalize_with_context(source, 100, Options::default(), &rules, &Context::default())
            .unwrap();
    assert_eq!(result.warnings.len(), 1); // Preserving layout does not disable inspection.
    assert!(result.output.starts_with("/- Existing attribution. -/"));
    let mut options = Options::default();
    options.frontmatter.header = false;
    assert!(
        normalize_with_context(source, 100, options, &rules, &Context::default())
            .unwrap()
            .warnings
            .is_empty()
    );
    options.frontmatter = Default::default();
    options.frontmatter.enabled = false;
    assert!(
        normalize_with_context(source, 100, options, &rules, &Context::default())
            .unwrap()
            .warnings
            .is_empty()
    );
    for prefix in ["-- leanck: off frontmatter\n", "-- leanck: off\n"] {
        assert!(header_warnings(&format!("{prefix}{source}"), &rules).is_empty());
    }
    // Docstrings and ordinary comments later in the body are not attribution headers.
    assert!(header_warnings("/-! Module documentation. -/\nmodule\n", &rules).is_empty());
    assert!(header_warnings("def x := 1\n/- Body comment. -/\n", &rules).is_empty());
}
