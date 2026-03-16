//! Frontmatter regressions: import detection limited to Lean's module header,
//! linear-time sorting, Lean 4.28 import forms, stable reordering, and header
//! formatting that keeps inner indentation.
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

/// The output and a second pass over it.
fn twice(source: &str) -> (String, String) {
    let once = normalize(source);
    let again = normalize(&once);
    (once, again)
}

#[test]
fn imports_after_a_comment_on_the_same_line_do_not_panic() {
    for (source, expected) in [
        (
            "import B\n/- c -/ import A\n",
            "import B\n/- c -/ import A\n",
        ),
        ("/- c -/ import A\n", "/-\nc\n-/ import A\n"),
        ("/-! d -/ import A\n", "/-!\nd\n-/ import A\n"),
        ("module\n/- c -/ import A\n", "module\n\n/- c -/ import A\n"),
    ] {
        let (once, again) = twice(source);
        assert_eq!(once, expected, "{source:?}");
        assert_eq!(again, once, "{source:?}");
    }
}

#[test]
fn import_text_inside_an_interpolated_string_is_not_an_import() {
    // Reduced from Mathlib/Tactic/Linter/DeprecatedModule.lean.
    let source = concat!(
        "        m!\"{msg?.getD \"\"}\\n\\\n",
        "          {String.join (preferred.foldl (·.push s!\"import {·}\\n\") #[]).toList}\"\n",
    );
    assert_eq!(normalize(source), source);
}

#[test]
fn imports_after_the_first_command_are_left_alone() {
    let source = "module\n\nimport B\nimport A\n\ndef x := 1\n\nimport D\nimport C\n";
    assert_eq!(
        normalize(source),
        "module\n\nimport A\nimport B\n\ndef x := 1\n\nimport D\nimport C\n"
    );
}

#[test]
fn import_sorting_is_linear_in_file_length() {
    let mut source = String::from("module\n\npublic import B\npublic import A\n\n");
    for i in 0..20_000 {
        source.push_str(&format!("theorem t{i} : True := trivial\n"));
    }
    let start = std::time::Instant::now();
    let output = normalize(&source);
    // The former per-line rescan took 30 s in release builds for this input.
    assert!(
        start.elapsed() < std::time::Duration::from_secs(10),
        "{:?}",
        start.elapsed()
    );
    assert!(output.starts_with("module\n\npublic import A\npublic import B\n\ntheorem t0"));
}

#[test]
fn meta_all_and_public_meta_imports_sort_and_deduplicate_as_imports() {
    let source = concat!(
        "module\n\n",
        "public import B\n",
        "meta import Z\n",
        "public meta import Y\n",
        "import all C\n",
        "public import A\n",
        "import A\n",
        "public import B\n",
        "meta import all W\n",
        "\n",
        "def x := 1\n",
    );
    let (once, again) = twice(source);
    assert_eq!(
        once,
        concat!(
            "module\n\n",
            "public meta import Y\n",
            "public import A\n",
            "public import B\n",
            "meta import Z\n",
            "meta import all W\n",
            "import A\n",
            "import all C\n",
            "\n",
            "def x := 1\n",
        )
    );
    assert_eq!(again, once);
}

#[test]
fn meta_import_groups_are_not_barriers_to_reordering() {
    // A docstring that is only a heading would be a section heading.
    let source =
        "/-! Doc. -/\n\nmodule\n\npublic import A\n\nmeta import B\n\n@[expose] public section\n";
    let (once, again) = twice(source);
    assert_eq!(
        once,
        "module\n\npublic import A\n\nmeta import B\n\n/-!\nDoc.\n-/\n\n@[expose] public section\n"
    );
    assert_eq!(again, once);
}

#[test]
fn a_plain_comment_after_the_imports_stays_in_place() {
    let source = "module\n\nimport M\n\n/- A note -/\ndef x := 1\n";
    let (once, again) = twice(source);
    assert_eq!(once, source);
    assert_eq!(again, source);
}

#[test]
fn nested_header_comments_are_one_block() {
    let source = "module\n\nimport M\n\n/- outer /- inner -/ still outer -/\ndef x := 1\n";
    assert_eq!(normalize(source), source);
    let source = "/- Header /- nested -/ text. -/\nmodule\n";
    let (once, again) = twice(source);
    assert_eq!(once, "/-\nHeader /- nested -/ text.\n-/\n\nmodule\n");
    assert_eq!(again, once);
}

#[test]
fn header_normalization_keeps_inner_indentation() {
    let source = "/-\n\nCopyright (c) 2020 A.\n  def commented :=\n    out\n   -/\nmodule\n";
    let (once, again) = twice(source);
    assert_eq!(
        once,
        "/-\nCopyright (c) 2020 A.\n  def commented :=\n    out\n-/\n\nmodule\n"
    );
    assert_eq!(again, once);
    // Text on the opening line loses only the space after the delimiter.
    let source = "/-   Copyright (c) 2020 A.\n  Authors: A -/\nmodule\n";
    assert_eq!(
        normalize(source),
        "/-\nCopyright (c) 2020 A.\n  Authors: A\n-/\n\nmodule\n"
    );
    // A uniformly indented body keeps its indentation.
    let source = "/-\n  Copyright (c) 2020 A.\n  Authors: A\n-/\nmodule\n";
    assert_eq!(
        normalize(source),
        "/-\n  Copyright (c) 2020 A.\n  Authors: A\n-/\n\nmodule\n"
    );
}

#[test]
fn import_groups_merged_by_zero_blank_lines_sort_in_one_pass() {
    let mut rules = NormalizeRules::default();
    rules.whitespace.max_blank_lines = 0;
    let normalize = |source: &str| {
        normalize_with_context(source, 100, Options::default(), &rules, &Context::default())
            .unwrap()
            .output
    };
    let once = normalize("import c\n\nimport B\n");
    assert_eq!(once, "import B\nimport c\n");
    assert_eq!(normalize(&once), once);
}

#[test]
fn converted_banner_headings_are_not_module_docstrings() {
    let options = Options {
        headings: true,
        ..Options::default()
    };
    let normalize = |source: &str| {
        normalize_with_context(
            source,
            100,
            options,
            &NormalizeRules::default(),
            &Context::default(),
        )
        .unwrap()
        .output
    };
    for (source, expected) in [
        (
            "module\n-- ====\n-- banner\n-- ====\ndef n := 1\n",
            "module\n\n/-! ## Banner -/\ndef n := 1\n",
        ),
        (
            "import A\nnamespace F\n-- ====\n-- banner\n-- ====\n",
            "import A\n\nnamespace F\n\n/-! ## Banner -/\n",
        ),
    ] {
        let once = normalize(source);
        assert_eq!(once, expected);
        assert_eq!(normalize(&once), once);
    }
    // A level-one banner heading is a section heading too.
    let mut rules = NormalizeRules::default();
    rules.headings.level = 1;
    let source = "module\n\nimport A\n\n@[expose] public section\n\n-- ====\n-- banner\n-- ====\ndef n := 1\n";
    let expected =
        "module\n\nimport A\n\n@[expose] public section\n\n/-! # Banner -/\ndef n := 1\n";
    let normalize = |source: &str| {
        normalize_with_context(source, 100, options, &rules, &Context::default())
            .unwrap()
            .output
    };
    let once = normalize(source);
    assert_eq!(once, expected);
    assert_eq!(normalize(&once), once);
    // An empty heading, as `# #` becomes without its closing sequence, is a
    // section heading before and after the rewrite.
    let (once, again) = twice("module\n/-!# #-/\n=\n");
    assert_eq!(once, "module\n\n/-! # -/\n=\n");
    assert_eq!(again, once);
}
