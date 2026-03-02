//! Heuristic declaration extraction over the shared Lean token scanner.
//!
//! Extracts declaration names, kinds, and positions outside comments/strings.
//! Catches syntax-extension commands (Mathlib's `lemma`) that
//! `textDocument/documentSymbol` filters out. The `range.end` is a coarse upper
//! bound (line before the next decl); callers refine via folding ranges.

use crate::state::{WirePosition, WireRange};
use leanck_core::api::LeanDeclarationKind;

/// Sentinel "end of line" character (matches the TS `2 ** 30`).
pub const END_CHAR: u32 = 1 << 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDecl {
    pub name: String,
    pub kind: LeanDeclarationKind,
    pub selection_range: WireRange,
    pub range: WireRange,
}

const DECL_MODIFIERS: &[&str] = &["private", "protected", "noncomputable", "unsafe", "partial"];

/// Extract every top-level declaration name + start line.
pub fn extract_decls(source: &str) -> Vec<ParsedDecl> {
    let tokens = crate::scan::tokens(source);
    let mut hits = Vec::new();
    let mut scopes: Vec<Option<String>> = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let start = i;
        let line = tokens[i].range.start.line;
        while i < tokens.len() && tokens[i].range.start.line == line {
            i += 1;
        }
        let row = &tokens[start..i];
        let mut n = 0;
        if row[0].text == "namespace" {
            let prefix = scopes.iter().rev().find_map(|s| s.as_deref()).unwrap_or("");
            scopes.push(row.get(1).map(|t| crate::resolve::qualify(prefix, &t.text)));
            continue;
        }
        if row[0].text == "section" {
            scopes.push(None);
            continue;
        }
        if row[0].text == "end" {
            scopes.pop();
            continue;
        }
        while n < row.len() && row[n].text == "@" {
            n += 1;
            let mut depth = 0;
            while n < row.len() {
                if row[n].text == "[" {
                    depth += 1;
                }
                if row[n].text == "]" {
                    depth -= 1;
                    n += 1;
                    if depth == 0 {
                        break;
                    }
                    continue;
                }
                n += 1;
            }
        }
        while n < row.len() && DECL_MODIFIERS.contains(&row[n].text.as_str()) {
            n += 1;
        }
        if n + 1 >= row.len() {
            continue;
        }
        let Ok(kind) = row[n].text.parse::<LeanDeclarationKind>() else {
            continue;
        };
        let name = &row[n + 1];
        if name
            .text
            .chars()
            .next()
            .is_none_or(|c| matches!(c, ':' | '{' | '(' | '[' | '⦃' | '='))
        {
            continue;
        }
        let prefix = scopes.iter().rev().find_map(|s| s.as_deref()).unwrap_or("");
        let full = crate::resolve::qualify(prefix, &name.text);
        hits.push((line, full, kind, name.range));
    }
    let last = source.lines().count().saturating_sub(1) as u32;
    hits.iter()
        .enumerate()
        .map(|(i, (line, name, kind, selection))| ParsedDecl {
            name: name.clone(),
            kind: *kind,
            selection_range: *selection,
            range: WireRange {
                start: WirePosition {
                    line: *line,
                    character: 0,
                },
                end: WirePosition {
                    line: hits
                        .get(i + 1)
                        .map(|h| h.0.saturating_sub(1).max(*line))
                        .unwrap_or(last),
                    character: END_CHAR,
                },
            },
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(src: &str) -> Vec<String> {
        extract_decls(src).into_iter().map(|d| d.name).collect()
    }

    #[test]
    fn extracts_theorem_and_def() {
        let decls = extract_decls("theorem foo : True := trivial\ndef bar := 1\n");
        assert_eq!(
            decls.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(),
            vec!["foo", "bar"]
        );
        assert_eq!(decls[0].range.start.line, 0);
        assert_eq!(decls[1].range.start.line, 1);
    }

    #[test]
    fn every_lexical_keyword_has_a_typed_kind() {
        for kind in LeanDeclarationKind::ALL {
            let declarations = extract_decls(&format!("{} example := 0", kind.as_str()));
            assert_eq!(declarations.len(), 1);
            assert_eq!(declarations[0].kind, *kind);
        }
    }

    #[test]
    fn handles_mathlib_lemma() {
        let decls = extract_decls("lemma my_lemma (n : Nat) : n = n := rfl");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].name, "my_lemma");
    }

    #[test]
    fn strips_attributes_and_modifiers() {
        assert_eq!(
            names("@[simp] private noncomputable def baz : Nat := 0"),
            vec!["baz"]
        );
    }

    #[test]
    fn captures_dotted_and_unicode_names() {
        let decls = extract_decls("theorem Foo.bar_α : True := trivial");
        assert_eq!(decls[0].name, "Foo.bar_α");
    }

    #[test]
    fn blanks_comments() {
        let src = "-- def commented := 1\n/- theorem alsoComment -/\ndef real := 2";
        assert_eq!(names(src), vec!["real"]);
    }

    #[test]
    fn skips_anonymous_instances() {
        assert_eq!(extract_decls("instance : Inhabited Nat := ⟨0⟩").len(), 0);
    }

    #[test]
    fn does_not_match_keyword_prefixed_identifiers() {
        assert_eq!(extract_decls("definitely_not_a_decl := 1").len(), 0);
    }

    #[test]
    fn namespace_sections_and_root_prefix_share_qualification() {
        assert_eq!(
            names(
                "namespace A\nsection\nnamespace B\nlemma α : True := trivial\nend B\nend\nnamespace _root_.C\ndef δ := 1\nend C\ndef _root_.global := 2\nend A"
            ),
            ["A.B.α", "C.δ", "global"]
        );
    }
    #[test]
    fn bounds_each_range_at_line_before_next() {
        let decls = extract_decls("def a := 1\n\ndef b := 2");
        assert_eq!(decls[0].range.end.line, 1);
        assert_eq!(decls[1].range.end.line, 2);
    }
}
