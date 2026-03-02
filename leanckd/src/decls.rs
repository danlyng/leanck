//! Regex-free declaration scanner. Direct port of `leanck-lsp/src/regexDecls.ts`.
//!
//! Extracts top-level declaration names + start lines by scanning text, after
//! blanking comments. Catches syntax-extension commands (Mathlib's `lemma`) that
//! `textDocument/documentSymbol` filters out. The `range.end` is a coarse upper
//! bound (line before the next decl); callers refine via folding ranges.

use crate::state::{WirePosition, WireRange};

/// Sentinel "end of line" character (matches the TS `2 ** 30`).
pub const END_CHAR: u32 = 1 << 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedDecl {
    pub name: String,
    pub range: WireRange,
}

const DECL_KEYWORDS: &[&str] = &[
    "theorem",
    "lemma",
    "def",
    "definition",
    "abbrev",
    "instance",
    "structure",
    "class",
    "inductive",
    "opaque",
    "axiom",
];

const DECL_MODIFIERS: &[&str] = &["private", "protected", "noncomputable", "unsafe", "partial"];

/// Extract every top-level declaration name + start line.
pub fn extract_decls(source: &str) -> Vec<ParsedDecl> {
    let cleaned = strip_comments(source);
    let lines: Vec<&str> = cleaned.split('\n').collect();
    let mut hits: Vec<(usize, String)> = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if let Some(name) = extract_decl_name(line) {
            hits.push((i, name));
        }
    }
    let mut out = Vec::with_capacity(hits.len());
    let last_line = lines.len().saturating_sub(1);
    for j in 0..hits.len() {
        let (line, ref name) = hits[j];
        let end_line = match hits.get(j + 1) {
            Some((next_line, _)) => (*next_line).saturating_sub(1).max(line),
            None => last_line,
        };
        out.push(ParsedDecl {
            name: name.clone(),
            range: WireRange {
                start: WirePosition { line: line as u32, character: 0 },
                end: WirePosition { line: end_line as u32, character: END_CHAR },
            },
        });
    }
    out
}

fn extract_decl_name(raw_line: &str) -> Option<String> {
    let mut rest = raw_line.trim_start();
    // Strip simple `@[...]` attribute groups, repeatedly.
    while let Some(stripped) = rest.strip_prefix('@') {
        let close = stripped.find(']')?;
        rest = stripped[close + 1..].trim_start();
    }
    // Strip declaration modifiers (private, noncomputable, …).
    loop {
        let mut consumed = false;
        for m in DECL_MODIFIERS {
            if let Some(after) = rest.strip_prefix(m) {
                if after.is_empty() || after.starts_with(char::is_whitespace) {
                    rest = after.trim_start();
                    consumed = true;
                    break;
                }
            }
        }
        if !consumed {
            break;
        }
    }
    // Match a declaration keyword (must be followed by whitespace or EOL).
    let mut matched = false;
    for kw in DECL_KEYWORDS {
        if let Some(after) = rest.strip_prefix(kw) {
            if after.is_empty() || after.starts_with(char::is_whitespace) {
                rest = after.trim_start();
                matched = true;
                break;
            }
        }
    }
    if !matched {
        return None;
    }
    extract_name_token(rest)
}

/// First run of characters that aren't whitespace or a structural delimiter.
/// Mirrors `NAME_TOKEN_RE = /^([^\s:{(\[<,=⦃]+)/`.
fn extract_name_token(s: &str) -> Option<String> {
    let mut out = String::new();
    for ch in s.chars() {
        if ch.is_whitespace() || matches!(ch, ':' | '{' | '(' | '[' | '<' | ',' | '=' | '⦃') {
            break;
        }
        out.push(ch);
    }
    if out.is_empty() {
        None
    } else {
        Some(out)
    }
}

/// Replace block (`/- ... -/`, nested) and line (`-- ...`) comments with
/// whitespace, preserving newlines so line indices stay valid.
fn strip_comments(source: &str) -> String {
    let chars: Vec<char> = source.chars().collect();
    let mut out = String::with_capacity(source.len());
    let mut depth: i32 = 0;
    let mut in_line_comment = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        let n = chars.get(i + 1).copied();
        if c == '\n' {
            out.push('\n');
            in_line_comment = false;
            i += 1;
            continue;
        }
        if in_line_comment {
            out.push(' ');
            i += 1;
            continue;
        }
        if depth == 0 && c == '-' && n == Some('-') {
            in_line_comment = true;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if c == '/' && n == Some('-') {
            depth += 1;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if depth > 0 && c == '-' && n == Some('/') {
            depth -= 1;
            out.push(' ');
            out.push(' ');
            i += 2;
            continue;
        }
        if depth > 0 {
            out.push(' ');
            i += 1;
            continue;
        }
        out.push(c);
        i += 1;
    }
    out
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
        assert_eq!(decls.iter().map(|d| d.name.as_str()).collect::<Vec<_>>(), vec!["foo", "bar"]);
        assert_eq!(decls[0].range.start.line, 0);
        assert_eq!(decls[1].range.start.line, 1);
    }

    #[test]
    fn handles_mathlib_lemma() {
        let decls = extract_decls("lemma my_lemma (n : Nat) : n = n := rfl");
        assert_eq!(decls.len(), 1);
        assert_eq!(decls[0].name, "my_lemma");
    }

    #[test]
    fn strips_attributes_and_modifiers() {
        assert_eq!(names("@[simp] private noncomputable def baz : Nat := 0"), vec!["baz"]);
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
    fn bounds_each_range_at_line_before_next() {
        let decls = extract_decls("def a := 1\n\ndef b := 2");
        assert_eq!(decls[0].range.end.line, 1);
        assert_eq!(decls[1].range.end.line, 2);
    }
}
