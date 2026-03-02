//! Sorry detection. Port of `leanck-lsp/src/sorries.ts`.
//!
//! Two signals: `hasSorry`-coded diagnostics (declaration-level) and a textual
//! scan for `sorry` tokens outside comments (tactic-position).

use serde::Serialize;

use crate::state::{StateStore, WirePosition, WireRange};

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum SorryKind {
    Declaration,
    Tactic,
}

#[derive(Debug, Clone, Serialize)]
pub struct SorryEntry {
    pub range: WireRange,
    pub kind: SorryKind,
}

/// Collect sorries for `uri` from current diagnostics + a text scan of `source`.
/// Deduplicated by (range, kind).
pub fn collect_sorries(state: &StateStore, uri: &str, source: &str) -> Vec<SorryEntry> {
    let mut result: Vec<SorryEntry> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut push = |range: WireRange, kind: SorryKind| {
        let key = format!(
            "{:?}:{}:{}:{}:{}",
            kind, range.start.line, range.start.character, range.end.line, range.end.character
        );
        if seen.insert(key) {
            result.push(SorryEntry { range, kind });
        }
    };

    if let Some(s) = state.get(uri) {
        for d in &s.diagnostics {
            if d.code_is("hasSorry") {
                push(d.range, SorryKind::Declaration);
            }
        }
    }

    for range in scan_sorry_tokens(source) {
        push(range, SorryKind::Tactic);
    }

    result
}

/// Yield ranges for `sorry` (as a word) outside line and block comments.
fn scan_sorry_tokens(source: &str) -> Vec<WireRange> {
    let chars: Vec<char> = source.chars().collect();
    let mut out = Vec::new();
    let mut line: u32 = 0;
    let mut col: u32 = 0;
    let mut i = 0usize;
    let mut in_block: i32 = 0;

    // advance `n` chars, tracking line/column.
    let advance = |i: &mut usize, line: &mut u32, col: &mut u32, n: usize| {
        for _ in 0..n {
            if chars.get(*i) == Some(&'\n') {
                *line += 1;
                *col = 0;
            } else {
                *col += 1;
            }
            *i += 1;
        }
    };

    while i < chars.len() {
        let ch = chars[i];
        let next = chars.get(i + 1).copied();

        if in_block > 0 {
            if ch == '-' && next == Some('/') {
                in_block -= 1;
                advance(&mut i, &mut line, &mut col, 2);
                continue;
            }
            if ch == '/' && next == Some('-') {
                in_block += 1;
                advance(&mut i, &mut line, &mut col, 2);
                continue;
            }
            advance(&mut i, &mut line, &mut col, 1);
            continue;
        }

        if ch == '-' && next == Some('-') {
            while i < chars.len() && chars[i] != '\n' {
                advance(&mut i, &mut line, &mut col, 1);
            }
            continue;
        }
        if ch == '/' && next == Some('-') {
            in_block = 1;
            advance(&mut i, &mut line, &mut col, 2);
            continue;
        }

        if ch == 's' && starts_with_at(&chars, i, "sorry") {
            let before = if i > 0 { chars[i - 1] } else { '\0' };
            let after = chars.get(i + 5).copied().unwrap_or('\0');
            if !is_ident_char(before) && !is_ident_char(after) {
                let start_line = line;
                let start_col = col;
                advance(&mut i, &mut line, &mut col, 5);
                out.push(WireRange {
                    start: WirePosition { line: start_line, character: start_col },
                    end: WirePosition { line, character: col },
                });
                continue;
            }
        }
        advance(&mut i, &mut line, &mut col, 1);
    }
    out
}

fn starts_with_at(chars: &[char], i: usize, word: &str) -> bool {
    word.chars().enumerate().all(|(k, wc)| chars.get(i + k) == Some(&wc))
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::WireDiagnostic;

    fn r(sl: u32, sc: u32, el: u32, ec: u32) -> WireRange {
        WireRange {
            start: WirePosition { line: sl, character: sc },
            end: WirePosition { line: el, character: ec },
        }
    }

    fn has_sorry_diag(range: WireRange, message: &str) -> WireDiagnostic {
        WireDiagnostic {
            range,
            severity: None,
            code: Some(serde_json::json!("hasSorry")),
            message: message.to_string(),
            tags: None,
            related_information: None,
            data: None,
            goals: None,
        }
    }

    #[test]
    fn surfaces_hassorry_as_declaration() {
        let s = StateStore::new();
        s.apply_diagnostics("u", None, vec![has_sorry_diag(r(3, 0, 3, 10), "declaration uses sorry")]);
        let entries = collect_sorries(&s, "u", "");
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].kind, SorryKind::Declaration);
        assert_eq!(entries[0].range.start.line, 3);
    }

    #[test]
    fn finds_tactic_position_sorry() {
        let s = StateStore::new();
        let entries = collect_sorries(&s, "u", "theorem t : True := by\n  sorry\n");
        let tactic: Vec<_> = entries.iter().filter(|e| e.kind == SorryKind::Tactic).collect();
        assert_eq!(tactic.len(), 1);
        assert_eq!(tactic[0].range.start.line, 1);
        assert_eq!(tactic[0].range.start.character, 2);
    }

    #[test]
    fn ignores_sorry_in_comments() {
        let s = StateStore::new();
        let doc = "-- sorry here\n/- sorry there -/\ndef x := 1\n";
        assert_eq!(collect_sorries(&s, "u", doc).len(), 0);
    }

    #[test]
    fn does_not_match_sorry_substring() {
        let s = StateStore::new();
        assert_eq!(collect_sorries(&s, "u", "def sorryNot := 1\n").len(), 0);
    }

    #[test]
    fn dedupes_identical_entries() {
        let s = StateStore::new();
        s.apply_diagnostics(
            "u",
            None,
            vec![has_sorry_diag(r(0, 0, 0, 5), "a"), has_sorry_diag(r(0, 0, 0, 5), "b")],
        );
        assert_eq!(collect_sorries(&s, "u", "").len(), 1);
    }
}
