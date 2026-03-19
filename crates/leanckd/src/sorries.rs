//! Sorry reports from a matching diagnostic snapshot and shared text tokens.
//!
//! Two signals: Lean's "declaration uses `sorry`" warnings (declaration-level)
//! and the shared text scan (tactic-position). Lean tags the warning only
//! internally and sends no diagnostic code, so it is recognized by message.

use leanck_core::api::Severity;
pub use leanck_core::api::{SorryEntry, SorryKind};
pub use leanck_core::sorry::{TextScan, scan_source};

/// Current toolchains quote the sorry term in backticks; older ones used `'sorry'`.
const SORRY_WARNINGS: [&str; 2] = ["declaration uses `", "declaration uses '"];

pub fn with_diagnostics(diagnostics: &[crate::state::WireDiagnostic], source: &str) -> TextScan {
    let mut entries = Vec::new();
    let mut errors = 0usize;
    for diagnostic in diagnostics {
        if SORRY_WARNINGS
            .iter()
            .any(|prefix| diagnostic.message.starts_with(prefix))
        {
            let entry = SorryEntry {
                range: diagnostic.range,
                kind: SorryKind::Declaration,
            };
            if !entries.contains(&entry) {
                entries.push(entry);
            }
        } else if diagnostic.severity_or_error() == Severity::Error {
            errors += 1;
        }
    }
    let text = scan_source(source);
    entries.extend(text.sorries);
    let mut notes = text.notes;
    if errors > 0 {
        notes.push(format!(
            "{errors} error diagnostics; Lean omits the sorry warning for declarations with errors"
        ));
    }
    TextScan {
        sorries: entries,
        notes,
    }
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

    fn diag(range: WireRange, severity: Severity, message: &str) -> WireDiagnostic {
        WireDiagnostic {
            range,
            severity: Some(severity),
            code: None,
            message: message.to_string(),
            tags: None,
            related_information: None,
            data: None,
            goals: None,
        }
    }

    // Messages as Lean 4.35 publishes them: no `code`, warning severity.
    #[test]
    fn combines_declaration_warnings_and_text_positions() {
        let scan = with_diagnostics(
            &[diag(
                r(0, 8, 0, 9),
                Severity::Warning,
                "declaration uses `sorry`",
            )],
            "theorem t : True := by\n  admit\n",
        );
        assert_eq!(scan.sorries.len(), 2);
        assert_eq!(scan.sorries[0].kind, SorryKind::Declaration);
        assert_eq!(scan.sorries[1].kind, SorryKind::Tactic);
        assert_eq!(scan.sorries[1].range.start, r(1, 2, 1, 7).start);
        assert!(scan.notes.is_empty());
    }
    #[test]
    fn recognizes_older_quoting_and_warnings_promoted_to_errors() {
        let scan = with_diagnostics(
            &[
                diag(r(0, 0, 0, 1), Severity::Warning, "declaration uses 'sorry'"),
                diag(r(1, 0, 1, 1), Severity::Error, "declaration uses `sorry`"),
            ],
            "",
        );
        assert_eq!(scan.sorries.len(), 2);
        assert!(scan.notes.is_empty());
    }
    #[test]
    fn ignores_unrelated_warnings_and_notes_errors() {
        let scan = with_diagnostics(
            &[
                diag(r(0, 0, 0, 1), Severity::Warning, "unused variable `sorry`"),
                diag(r(1, 0, 1, 1), Severity::Error, "unsolved goals"),
            ],
            "-- sorry\n/- sorry /- sorry -/ -/\ndef x := \"sorry\"\ndef sorryNot := 1",
        );
        assert!(scan.sorries.is_empty());
        assert_eq!(
            scan.notes,
            ["1 error diagnostics; Lean omits the sorry warning for declarations with errors"]
        );
    }
    #[test]
    fn dedupes_identical_diagnostics() {
        let scan = with_diagnostics(
            &[
                diag(r(0, 0, 0, 5), Severity::Warning, "declaration uses `sorry`"),
                diag(
                    r(0, 0, 0, 5),
                    Severity::Warning,
                    "declaration uses `sorryAx`",
                ),
            ],
            "",
        );
        assert_eq!(scan.sorries.len(), 1);
    }
}
