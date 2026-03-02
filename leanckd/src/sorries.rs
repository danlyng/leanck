//! Sorry reports from a matching diagnostic snapshot and shared text tokens.
//!
//! Two signals: `hasSorry`-coded diagnostics (declaration-level) and a textual
//! scan for `sorry` tokens outside comments (tactic-position).

pub use leanck_core::api::{SorryEntry, SorryKind};

pub fn text_sorries(source: &str) -> Vec<SorryEntry> {
    crate::scan::tokens(source)
        .into_iter()
        .filter(|token| token.text == "sorry")
        .map(|token| SorryEntry {
            range: token.range,
            kind: SorryKind::Tactic,
        })
        .collect()
}
pub fn with_diagnostics(
    diagnostics: &[crate::state::WireDiagnostic],
    source: &str,
) -> Vec<SorryEntry> {
    let mut entries = Vec::new();
    for diagnostic in diagnostics {
        if diagnostic.code_is("hasSorry") {
            let entry = SorryEntry {
                range: diagnostic.range,
                kind: SorryKind::Declaration,
            };
            if !entries.contains(&entry) {
                entries.push(entry);
            }
        }
    }
    entries.extend(text_sorries(source));
    entries
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::WirePosition;
    use crate::state::{WireDiagnostic, WireRange};

    fn r(sl: u32, sc: u32, el: u32, ec: u32) -> WireRange {
        WireRange {
            start: WirePosition {
                line: sl,
                character: sc,
            },
            end: WirePosition {
                line: el,
                character: ec,
            },
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
    fn combines_declaration_diagnostics_and_text_positions() {
        let entries = with_diagnostics(
            &[has_sorry_diag(r(0, 0, 1, 7), "declaration uses sorry")],
            "theorem t : True := by\n  sorry\n",
        );
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].kind, SorryKind::Declaration);
        assert_eq!(entries[1].range.start, r(1, 2, 1, 7).start);
    }
    #[test]
    fn ignores_comments_strings_and_substrings() {
        assert!(
            text_sorries(
                "-- sorry\n/- sorry /- sorry -/ -/\ndef x := \"sorry\"\ndef sorryNot := 1"
            )
            .is_empty()
        );
    }
    #[test]
    fn dedupes_identical_diagnostics() {
        assert_eq!(
            with_diagnostics(
                &[
                    has_sorry_diag(r(0, 0, 0, 5), "a"),
                    has_sorry_diag(r(0, 0, 0, 5), "b")
                ],
                ""
            )
            .len(),
            1
        );
    }
}
