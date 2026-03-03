//! Folding-range helpers. Port of `leanck-lsp/src/foldingRanges.ts`.
//!
//! Lean emits one folding range per top-level command (including syntax-
//! extension commands like `lemma` that `documentSymbol` filters out), so we use
//! them to find the exact body end of a textually discovered declaration — at the
//! cost of one cheap request to the **already-warm** worker (never a cold parse).

#[derive(Debug, Clone)]
pub struct FoldingRange {
    pub start_line: u32,
    pub end_line: u32,
    pub kind: Option<String>,
}

/// Pick the smallest `region`-kind fold containing `start_line`, preferring an
/// exact start-line match. Returns its `end_line`, or `None`.
pub fn find_body_end(folds: &[FoldingRange], start_line: u32) -> Option<u32> {
    let mut best: Option<&FoldingRange> = None;
    let mut best_starts_here = false;
    for f in folds {
        if let Some(k) = &f.kind {
            if k != "region" {
                continue;
            }
        }
        if f.start_line > start_line || f.end_line < start_line {
            continue;
        }
        let starts_here = f.start_line == start_line;
        match best {
            None => {
                best = Some(f);
                best_starts_here = starts_here;
            }
            Some(b) => {
                if starts_here && !best_starts_here {
                    best = Some(f);
                    best_starts_here = true;
                } else if starts_here == best_starts_here {
                    let size = f.end_line.saturating_sub(f.start_line);
                    let best_size = b.end_line.saturating_sub(b.start_line);
                    if size < best_size {
                        best = Some(f);
                    }
                }
            }
        }
    }
    best.map(|b| b.end_line)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fold(start: u32, end: u32) -> FoldingRange {
        FoldingRange {
            start_line: start,
            end_line: end,
            kind: None,
        }
    }

    #[test]
    fn none_when_no_region_contains_start() {
        assert_eq!(find_body_end(&[], 5), None);
        assert_eq!(find_body_end(&[fold(10, 20)], 5), None);
    }

    #[test]
    fn ignores_non_region_kinds() {
        let folds = vec![FoldingRange {
            start_line: 0,
            end_line: 9,
            kind: Some("comment".into()),
        }];
        assert_eq!(find_body_end(&folds, 0), None);
    }

    #[test]
    fn prefers_exact_start_line() {
        let folds = vec![fold(0, 30), fold(5, 12)];
        assert_eq!(find_body_end(&folds, 5), Some(12));
    }

    #[test]
    fn smallest_among_equal_start_preference() {
        let folds = vec![fold(2, 40), fold(2, 18)];
        assert_eq!(find_body_end(&folds, 2), Some(18));
    }
}
