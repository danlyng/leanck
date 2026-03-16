use leanck_config::{
    AttributePlacement as A, DelimiterStyle, FinalNewline, HeadingCase, LineEndings,
};
use leanck_normalize::{
    Context, NormalizeRules, Options,
    history::{Contributor, FileHistory},
    normalize_with_context,
};
fn apply(source: &str, rules: &NormalizeRules) -> String {
    normalize_with_context(
        source,
        100,
        Options {
            headings: true,
            ..Default::default()
        },
        rules,
        &Context::default(),
    )
    .unwrap()
    .output
}
#[test]
fn history_is_explicit_and_existing_attribution_is_authoritative() {
    let history = FileHistory {
        contributors: vec![
            Contributor {
                name: "Alice Example".into(),
                email: "alice@example.invalid".into(),
                years: [2018, 2024].into(),
            },
            Contributor {
                name: "Bob Example".into(),
                email: "bob@example.invalid".into(),
                years: [2021].into(),
            },
        ],
    };
    let rules = NormalizeRules::default();
    let source = "module\nimport Mathlib\n";
    let first = normalize_with_context(
        source,
        100,
        Options::default(),
        &rules,
        &Context {
            history: Some(&history),
            ..Default::default()
        },
    )
    .unwrap()
    .output;
    assert!(first.contains("Copyright (c) 2018–2024 Alice Example, Bob Example."));
    assert_eq!(first.matches("Copyright (c)").count(), 1);
    assert!(!first.contains("2099") && !first.contains("Apache") && !first.contains("Daniel"));
    let second =
        normalize_with_context(&first, 100, Options::default(), &rules, &Context::default())
            .unwrap()
            .output;
    assert_eq!(first, second);
    let unknown = apply(source, &rules);
    assert!(!unknown.contains("Copyright"));
    let existing = "/-\n  Copyright (c) 1999 Other Person.\n  Released under a custom license.\n  Authors: Other Person\n-/\nmodule\nimport Foo\n";
    let output = normalize_with_context(
        existing,
        100,
        Options::default(),
        &rules,
        &Context {
            history: Some(&history),
            ..Default::default()
        },
    )
    .unwrap()
    .output;
    assert!(output.contains("1999 Other Person"));
    assert!(output.contains("a custom license"));
    assert!(!output.contains("Alice"));
}
#[test]
fn headings_and_attribute_positions_are_independent_policies() {
    let mut rules = NormalizeRules::default();
    rules.headings.case = HeadingCase::Off;
    rules.headings.level = 3;
    assert!(
        apply("-- ====\n-- mixed CASE title\n-- ====\n", &rules)
            .contains("/-! ### mixed CASE title -/")
    );
    rules.attributes.overrides.clear();
    rules.attributes.placement = A::Above;
    let out = apply("@[simp, ext] theorem test : True := trivial\n", &rules);
    assert_eq!(out, "@[simp, ext]\ntheorem test : True := trivial\n");
    rules.attributes.placement = A::SameLine;
    assert_eq!(
        apply(&out, &rules),
        "@[simp, ext] theorem test : True := trivial\n"
    );
    rules.attributes.placement = A::Off;
    assert_eq!(apply(&out, &rules), out);
    assert_eq!(
        apply(
            "@[simp] -- keep this comment\ntheorem x : True := trivial\n",
            &rules
        ),
        "@[simp] -- keep this comment\ntheorem x : True := trivial\n"
    );
}
#[test]
fn substitutions_only_touch_prose() {
    let mut rules = NormalizeRules::default();
    rules.spelling.use_builtin = false;
    rules
        .spelling
        .replacements
        .insert("colour".into(), "tint".into());
    let input = "/-- colour `colour` $colour$ [colour](https://example.test/colour) https://example.test/colour\n\n```lean\ndef colour := \"colour\"\n\n\n#check colour\n```\n-/\n";
    let out = apply(input, &rules);
    assert!(
        out.contains("tint `colour` $colour$ [tint](https://example.test/colour)"),
        "{out}"
    );
    assert!(out.contains("https://example.test/colour"));
    assert!(out.contains("def colour := \"colour\""));
    assert!(out.contains("\n\n\n#check colour"), "{out}");
    let literal = "def text := \"/-- colour -/\"\n";
    assert_eq!(apply(literal, &rules), literal);
}
#[test]
fn directives_disable_all_or_specific_rules_and_preserve_line_endings() {
    let mut rules = NormalizeRules::default();
    rules.whitespace.line_endings = LineEndings::Off;
    let input = "/-- colour -/\n-- leanck: off\n/-- colour -/\n\n\n@[simp]\ndef x := 1\n-- leanck: on\n/-- colour -/\n";
    let out = apply(input, &rules);
    assert!(out.starts_with("/-- color -/"));
    assert!(out.contains("-- leanck: off\n/-- colour -/\n\n\n@[simp]\ndef"));
    assert!(out.ends_with("/-- color -/\n"));
    let out = apply(
        "-- leanck: off spelling\n/-- colour: blue -/\n-- leanck: on spelling\n",
        &rules,
    );
    assert!(out.contains("colour: blue"));
    assert_eq!(
        apply("def s := \"-- leanck: off\"\n/-- colour -/\n", &rules),
        "def s := \"-- leanck: off\"\n/-- color -/\n"
    );
    let input = "/-- colour -/\r\n";
    assert_eq!(apply(input, &rules), "/-- color -/\r\n");
    rules.whitespace.line_endings = LineEndings::Lf;
    rules.whitespace.final_newline = FinalNewline::Remove;
    assert_eq!(apply(input, &rules), "/-- color -/");
}
#[test]
fn imports_and_comment_layout_are_configurable() {
    let mut rules = NormalizeRules::default();
    let out = apply(
        "module\nimport Z\nimport A\nimport A\n\nimport C\nimport B\n\n/-- keep -/\ndef x := 1\n",
        &rules,
    );
    assert!(
        out.contains("import A\nimport Z\n\nimport B\nimport C"),
        "{out}"
    );
    rules.comments.declaration = DelimiterStyle::Separate;
    assert_eq!(apply("/-- text -/\n", &rules), "/--\ntext\n-/\n");
    rules.comments.declaration = DelimiterStyle::Off;
    assert_eq!(apply("/--\ncolour -/\n", &rules), "/--\ncolor -/\n");
}

#[test]
fn frontmatter_order_does_not_move_scopes_across_declarations() {
    let rules = NormalizeRules::default();
    let source = "namespace Outer\ndef before := 1\n@[expose] public section\n/-! # docs -/\n";
    let out = apply(source, &rules);
    assert!(out.find("namespace Outer").unwrap() < out.find("def before").unwrap());
    assert!(out.find("def before").unwrap() < out.find("public section").unwrap());
    let mut rules = rules;
    rules.headings.level = 1;
    assert_eq!(
        apply("-- ====\n-- one heading\n-- ====\n", &rules),
        "/-! # One Heading -/\n"
    );
    let once = apply("-- ====\n-- one heading\n-- ====\n", &rules);
    assert_eq!(
        apply(&once, &rules),
        once,
        "level-one headings must be idempotent"
    );
    rules.frontmatter.module_doc_template = Some("/-! # About {module} -/".into());
    let out = normalize_with_context(
        "module\nimport Foo\ndef x := 1\n",
        100,
        Options::default(),
        &rules,
        &Context {
            module_name: Some("Example"),
            ..Default::default()
        },
    )
    .unwrap()
    .output;
    assert!(out.find("About Example").unwrap() < out.find("def x").unwrap());
}
#[test]
fn dictionaries_resolve_chains_reject_cycles_and_preserve_terms() {
    let mut rules = NormalizeRules::default();
    rules.spelling.use_builtin = false;
    rules
        .spelling
        .replacements
        .insert("foo".into(), "bar".into());
    rules
        .spelling
        .replacements
        .insert("bar".into(), "baz".into());
    assert_eq!(apply("/-- foo bar -/\n", &rules), "/-- baz baz -/\n");
    rules.spelling.protected_words.push("bar".into());
    assert_eq!(apply("/-- foo bar -/\n", &rules), "/-- bar bar -/\n");
    rules.spelling.protected_words.clear();
    rules
        .spelling
        .replacements
        .insert("baz".into(), "foo".into());
    assert!(
        normalize_with_context(
            "/-- foo -/",
            100,
            Options::default(),
            &rules,
            &Context::default()
        )
        .is_err()
    );
}
#[test]
fn raw_strings_character_literals_and_opted_in_bom_are_preserved() {
    let mut rules = NormalizeRules::default();
    rules.whitespace.remove_bom = false;
    let source = "\u{feff}def s := r#\"an inner \" quote\n-- ====\n-- not a heading\n-- ====\n\n\n/-- colour -/\"#\ndef c := '\"'\n/-- colour -/\n";
    let out = apply(source, &rules);
    assert!(out.starts_with('\u{feff}'));
    assert!(out.contains("\n\n\n/-- colour -/\"#"), "{out}");
    assert!(out.contains("-- not a heading"));
    assert!(out.ends_with("/-- color -/\n"), "{out}");
}

#[test]
fn disabled_regions_keep_their_bytes_with_explicit_line_ending_policy() {
    let mut rules = NormalizeRules::default();
    rules.whitespace.line_endings = LineEndings::Lf;
    let source =
        "/-- colour -/\r\n-- leanck: off\r\n/-- colour -/\r\n-- leanck: on\r\n/-- colour -/\r\n";
    let out = apply(source, &rules);
    assert!(out.starts_with("/-- color -/\n"));
    assert!(out.contains("-- leanck: off\r\n/-- colour -/\r\n"));
    assert!(out.ends_with("/-- color -/\n"));
    let out = apply(
        "/-- colour -/\r\n-- leanck: off whitespace\r\n/-- colour -/\r\n",
        &NormalizeRules::default(),
    );
    assert!(out.starts_with("/-- color -/\n"));
    assert!(out.ends_with("/-- color -/\r\n"));
}

#[test]
fn every_partial_frontmatter_order_keeps_omitted_components_as_barriers() {
    use leanck_config::FrontmatterPart as P;
    let original = [
        P::Header,
        P::Module,
        P::Imports,
        P::Namespace,
        P::Section,
        P::Docstring,
    ];
    let markers = [
        (P::Header, "header marker"),
        (P::Module, "module"),
        (P::Imports, "import Foo"),
        (P::Namespace, "namespace Example"),
        (P::Section, "@[expose] public section"),
        (P::Docstring, "module docs"),
    ];
    let source = "/- header marker -/\nmodule\nimport Foo\nnamespace Example\n@[expose] public section\n/-! module docs -/\ndef body := 1\n";
    let options = Options {
        comments: false,
        ..Default::default()
    };
    // Exercise all 64 subsets, including empty, singleton, and complete orders.
    for included in 0..64 {
        let mut rules = NormalizeRules::default();
        rules.frontmatter.order = rules
            .frontmatter
            .order
            .iter()
            .enumerate()
            .filter(|(i, _)| included & (1 << i) != 0)
            .map(|(_, p)| *p)
            .collect();
        let out = normalize_with_context(source, 100, options, &rules, &Context::default())
            .unwrap()
            .output;
        for (anchor, part) in original.iter().enumerate() {
            if !rules.frontmatter.order.contains(part) {
                let marker = markers.iter().find(|(p, _)| p == part).unwrap().1;
                for (i, (other, other_marker)) in markers.iter().enumerate() {
                    if other != part {
                        assert_eq!(
                            out.find(other_marker).unwrap() < out.find(marker).unwrap(),
                            i < anchor,
                            "subset {included}: {other:?} crossed omitted {part:?}: {out}"
                        );
                    }
                }
            }
        }
        // Included components within the same run must follow the requested order.
        let mut runs = original.split(|p| !rules.frontmatter.order.contains(p));
        for run in &mut runs {
            let mut expected = run.to_vec();
            expected.sort_by_key(|p| rules.frontmatter.order.iter().position(|q| q == p).unwrap());
            for pair in expected.windows(2) {
                let left = markers.iter().find(|(p, _)| *p == pair[0]).unwrap().1;
                let right = markers.iter().find(|(p, _)| *p == pair[1]).unwrap().1;
                assert!(
                    out.find(left).unwrap() < out.find(right).unwrap(),
                    "subset {included}: {out}"
                );
            }
        }
        assert!(out.ends_with("def body := 1\n"), "subset {included}: {out}");
        let twice = normalize_with_context(&out, 100, options, &rules, &Context::default())
            .unwrap()
            .output;
        assert_eq!(twice, out, "subset {included} is not idempotent");
    }
}

#[test]
fn empty_frontmatter_order_preserves_positions_but_allows_internal_formatting() {
    let mut rules = NormalizeRules::default();
    rules.frontmatter.order.clear();
    let source = "/- Original authors: Someone -/\nmodule\nimport Z\nimport A\nnamespace Example\n@[expose] public section\n/-! Module docs -/\n";
    let out = apply(source, &rules);
    assert!(
        out.starts_with("/-\nOriginal authors: Someone\n-/\n"),
        "{out}"
    );
    assert!(out.contains("import A\nimport Z"), "{out}");
    assert!(out.find("namespace Example").unwrap() < out.find("public section").unwrap());
    assert!(out.find("public section").unwrap() < out.find("Module docs").unwrap());
}

#[test]
fn partial_frontmatter_order_accepts_custom_order_and_absent_barriers() {
    use leanck_config::FrontmatterPart as P;
    let mut rules = NormalizeRules::default();
    rules.frontmatter.order = vec![P::Namespace, P::Docstring, P::Section];
    let out = apply(
        "module\nimport Foo\n/-! docs -/\n@[expose] public section\nnamespace Example\n",
        &rules,
    );
    assert!(
        out.find("namespace Example").unwrap() < out.find("docs").unwrap(),
        "{out}"
    );
    assert!(
        out.find("docs").unwrap() < out.find("public section").unwrap(),
        "{out}"
    );
    // An omitted component only creates a barrier if it actually exists in the source.
    rules.frontmatter.order = vec![P::Docstring, P::Section];
    let out = apply(
        "module\nimport Foo\n@[expose] public section\n/-! docs -/\n",
        &rules,
    );
    assert!(
        out.find("docs").unwrap() < out.find("public section").unwrap(),
        "{out}"
    );
}

#[test]
fn omitted_imports_do_not_allow_unsafe_module_or_scope_reordering() {
    use leanck_config::FrontmatterPart as P;
    let mut rules = NormalizeRules::default();
    rules.frontmatter.order = vec![P::Module, P::Docstring, P::Namespace];
    let out = apply("module\nnamespace Example\n/-! docs -/\n", &rules);
    assert!(
        out.find("module").unwrap() < out.find("docs").unwrap(),
        "{out}"
    );
    assert!(
        out.find("docs").unwrap() < out.find("namespace Example").unwrap(),
        "{out}"
    );
    // Already-invalid scope/import ordering cannot be repaired across an omitted scope.
    rules.frontmatter.order = vec![P::Module, P::Imports, P::Docstring];
    let out = apply(
        "module\nnamespace Example\nimport Foo\n/-! docs -/\n",
        &rules,
    );
    assert!(
        out.find("namespace Example").unwrap() < out.find("import Foo").unwrap(),
        "{out}"
    );
}

#[test]
fn explicit_history_applies_author_exclusions_before_rendering_all_placeholders() {
    let history = FileHistory {
        contributors: vec![
            Contributor {
                name: "Codex".into(),
                email: "noreply@openai.com".into(),
                years: [2010].into(),
            },
            Contributor {
                name: "Human".into(),
                email: "human@example.test".into(),
                years: [2020].into(),
            },
        ],
    };
    let mut rules = NormalizeRules::default();
    rules.frontmatter.header_template = "/-\n{copyright}\nAuthors: {authors}\nYears: {years}; first: {first_year}; last: {last_year}\n-/".into();
    let result = normalize_with_context(
        "def x := 1\n",
        100,
        Options::default(),
        &rules,
        &Context {
            history: Some(&history),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(result.output.contains("Copyright (c) 2020 Human."));
    assert!(
        result
            .output
            .contains("Years: 2020; first: 2020; last: 2020")
    );
    assert!(!result.output.contains("Codex") && !result.output.contains("2010"));
    let second = normalize_with_context(
        &result.output,
        100,
        Options::default(),
        &rules,
        &Context::default(),
    )
    .unwrap();
    assert!(second.warnings.is_empty(), "{:?}", second.warnings);
    rules.frontmatter.ignore_authors.push("Human".into());
    let result = normalize_with_context(
        "def x := 1\n",
        100,
        Options::default(),
        &rules,
        &Context {
            history: Some(&history),
            ..Default::default()
        },
    )
    .unwrap();
    assert!(!result.output.contains("Copyright"));
}

#[test]
fn author_mapping_is_exact_single_pass_and_deduplicates_display_names() {
    let history = FileHistory {
        contributors: [
            ("Alias", "old@example.test", 2020),
            ("Other Alias", "new@example.test", 2024),
            ("Unmapped", "other@example.test", 2022),
            ("alias", "case@example.test", 2021),
            ("Codex", "noreply@openai.com", 2010),
        ]
        .into_iter()
        .map(|(name, email, year)| Contributor {
            name: name.into(),
            email: email.into(),
            years: [year].into(),
        })
        .collect(),
    };
    let mut rules = NormalizeRules::default();
    rules.frontmatter.author_map = [
        ("Alias", "Émilie Example"),
        ("Other Alias", "Émilie Example"),
        ("Émilie Example", "Must not chain"),
        ("Codex", "Must remain excluded"),
    ]
    .into_iter()
    .map(|(key, value)| (key.into(), value.into()))
    .collect();
    let context = Context {
        history: Some(&history),
        ..Default::default()
    };
    let source = normalize_with_context("def x := 1\n", 100, Options::default(), &rules, &context)
        .unwrap()
        .output;
    assert!(
        source.contains("Copyright (c) 2020–2024 Unmapped, alias, Émilie Example."),
        "{source}"
    );
    assert!(
        source.contains("Authors: Unmapped, alias, Émilie Example"),
        "{source}"
    );
    assert_eq!(source.matches("Émilie Example").count(), 2);
    assert!(!source.contains("Must") && !source.contains("2010"));
    // Changing a map does not rewrite a retained existing header.
    rules
        .frontmatter
        .author_map
        .insert("Alias".into(), "New Display".into());
    assert_eq!(
        normalize_with_context(&source, 100, Options::default(), &rules, &context)
            .unwrap()
            .output,
        source
    );
}
