use leanck_config::AttributePlacement as A;
use leanck_normalize::{
    Context, FrontmatterOptions, NormalizeRules, Options, normalize_with_context,
};

fn options(comment_reflow: bool) -> Options {
    Options {
        frontmatter: FrontmatterOptions {
            enabled: false,
            ..Default::default()
        },
        comment_reflow,
        ..Default::default()
    }
}

fn run(source: &str, options: Options, rules: &NormalizeRules) -> String {
    normalize_with_context(source, 100, options, rules, &Context::default())
        .unwrap()
        .output
}

fn apply(source: &str) -> String {
    run(source, options(false), &NormalizeRules::default())
}

#[test]
fn attribute_groups_with_doc_comments_converge_in_one_pass() {
    // Mathlib/Order/Hom/Bounded.lean: the comment pass collapses the doc comment
    // onto the attribute line, after which a second run used to join the now
    // single-line group onto `protected def`.
    let source = "@[to_dual (attr := simps) /--\nReinterpret a bot homomorphism. -/]\nprotected def dual : True := trivial\n";
    let expected = "@[to_dual (attr := simps) /-- Reinterpret a bot homomorphism. -/]\nprotected def dual : True := trivial\n";
    let first = apply(source);
    assert_eq!(first, expected);
    assert_eq!(apply(&first), expected);
    for reflow in [false, true] {
        let out = run(expected, options(reflow), &NormalizeRules::default());
        assert_eq!(out, expected);
    }
}

#[test]
fn single_line_attribute_groups_with_comments_are_left_in_place() {
    let mut rules = NormalizeRules::default();
    for source in [
        "@[to_dual /-- doc -/]\ntheorem a : True := trivial\n",
        "@[simp /- note -/]\ntheorem b : True := trivial\n",
        "@[simp] /- note -/ @[ext]\ntheorem c : True := trivial\n",
        "/- note -/ @[simp]\ntheorem d : True := trivial\n",
        "@[simp]\n/- note -/ theorem e : True := trivial\n",
        "@[simp] -- note\n@[ext]\ntheorem f : True := trivial\n",
    ] {
        for placement in [A::SameLine, A::Above] {
            rules.attributes.placement = placement;
            assert_eq!(run(source, options(false), &rules), source, "{placement:?}");
        }
    }
    rules.attributes.placement = A::Above;
    let source = "@[to_dual /-- doc -/] theorem a : True := trivial\n";
    assert_eq!(run(source, options(false), &rules), source);
}

#[test]
fn comment_like_text_in_attribute_strings_is_not_a_comment() {
    let source = "@[deprecated \"use /- x -/ instead\" (since := \"2024-01-01\")]\ntheorem a : True := trivial\n";
    assert_eq!(
        apply(source),
        "@[deprecated \"use /- x -/ instead\" (since := \"2024-01-01\")] theorem a : True := trivial\n"
    );
}

#[test]
fn moved_attributes_take_the_declaration_indentation() {
    // The declaration line never moves: attributes join it, or sit above it, at
    // its own column, whatever the attribute's original indentation.
    let source = "namespace X\n  @[simp]\ntheorem a : True := trivial\n\n@[simp]\n  theorem b : True := trivial\n\n  @[simp]\n  theorem c : True := trivial\nend X\n";
    let expected = "namespace X\n@[simp] theorem a : True := trivial\n\n  @[simp] theorem b : True := trivial\n\n  @[simp] theorem c : True := trivial\nend X\n";
    assert_eq!(apply(source), expected);
    assert_eq!(apply(expected), expected);

    let mut rules = NormalizeRules::default();
    rules.attributes.placement = A::Above;
    let source = "  @[simp]\n    @[ext]\ntheorem a : True := trivial\n  @[simp] theorem b : True := trivial\n";
    let expected =
        "@[simp]\n@[ext]\ntheorem a : True := trivial\n  @[simp]\n  theorem b : True := trivial\n";
    assert_eq!(run(source, options(false), &rules), expected);
    assert_eq!(run(expected, options(false), &rules), expected);
}

#[test]
fn a_doc_comment_on_the_declaration_line_does_not_count_toward_above_when_long() {
    let mut rules = NormalizeRules::default();
    rules.attributes.above_when_long = true;
    let words = "word ".repeat(30);
    let source = format!("@[simp]def z := /-- {words}-/ 1\n");
    // The joined line is long only because of the comment, which reflow
    // wraps; the attribute stays joined on every run.
    let first = run(&source, options(true), &rules);
    assert!(first.starts_with("@[simp] def z := /-- word"), "{first}");
    assert!(first.lines().count() > 1, "{first}");
    assert_eq!(run(&first, options(true), &rules), first);
}

#[test]
fn a_plain_block_comment_on_the_declaration_line_counts_toward_above_when_long() {
    let mut rules = NormalizeRules::default();
    rules.attributes.above_when_long = true;
    // Comment reflow leaves `/- -/` comments alone, so the code before and
    // after one stays on the line and decides the placement.
    let declaration = format!(
        "theorem foo /- c -/ {}: True := trivial",
        "(x : Nat) ".repeat(10)
    );
    let source = format!("@[simp]\n{declaration}\n");
    assert_eq!(run(&source, options(true), &rules), source);
    let short = "@[simp]\ntheorem foo /- c -/ : True := trivial\n";
    assert_eq!(
        run(short, options(true), &rules),
        "@[simp] theorem foo /- c -/ : True := trivial\n"
    );
}
