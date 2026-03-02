//! Full-project `lake build` runner with a compact, context-friendly report.
//!
//! **Hotfix mode:** this shells out to the project's *own* `lake` (the elan
//! toolchain, resolved on `PATH` in the project directory) and runs a plain
//! `lake build`, then greps the output into error/warning diagnostic blocks.
//!
//! We previously used a *forked* `lake` with a global `--json` flag for a
//! structured report. That fork compiled with its own bundled Lean (a different
//! git hash than the project toolchain), so the `.olean`s it produced carried an
//! incompatible header — `lake build` (trace-based) passed, but `lake serve`
//! (which actually imports them) failed with "incompatible header". Until the
//! fork is rebuilt against the matching release tag, we just use the real
//! toolchain and lose the structured report.
//!
//! The parse + render logic lives here (rather than in either front-end) so the
//! MCP server and the `leanck` CLI report identically, the same way
//! [`crate::format`] is shared.

use std::path::Path;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Options controlling a build run.
#[derive(Debug, Clone, Default)]
pub struct BuildOptions {
    /// Include warnings in the report (errors are always reported).
    pub warnings: bool,
    /// Treat warnings as build failures. Implies `warnings` so the diagnostics
    /// responsible for a strict failure are always included in the report.
    pub strict: bool,
    /// Targets to scope the build to, forwarded to `lake build` verbatim
    /// (module names like `MyPkg.Foo.Bar`, library/exe targets, facets, …).
    /// Empty means build the whole workspace, exactly as bare `lake build`.
    pub targets: Vec<String>,
}

/// A compact build result: pass/fail plus the diagnostic blocks grepped from
/// `lake`'s output. Serializable so the MCP/CLI `json` paths can return it.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BuildReport {
    /// Whether the build succeeded under the requested policy. In strict mode,
    /// this is false when `lake build` emitted any warnings.
    pub success: bool,
    /// Error diagnostics, each a (possibly multi-line) block from lake's output.
    pub errors: Vec<String>,
    /// Warning diagnostics. Empty unless warnings were requested or strict mode
    /// was enabled.
    pub warnings: Vec<String>,
}

/// Run a full `lake build` in `project` (using the project's own toolchain) and
/// return a parsed report.
pub async fn run(project: &Path, opts: BuildOptions) -> Result<BuildReport> {
    let output = tokio::process::Command::new("lake")
        .arg("build")
        .args(&opts.targets)
        .current_dir(project)
        .stdin(std::process::Stdio::null())
        .output()
        .await
        .with_context(|| {
            "running `lake build` — is `lake` on PATH (elan installed)?".to_string()
        })?;

    let lake_success = output.status.success();
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    Ok(build_report(lake_success, &stdout, &stderr, &opts))
}

fn build_report(
    lake_success: bool,
    stdout: &str,
    stderr: &str,
    opts: &BuildOptions,
) -> BuildReport {
    // Diagnostics land on either stream; scan both. Each diagnostic stays within
    // one stream, so concatenating keeps every block intact.
    let combined = format!("{stdout}\n{stderr}");

    let (errors, mut warnings) = extract_diagnostics(&combined);
    let has_located_diagnostics = !errors.is_empty() || !warnings.is_empty();

    let mut errors = errors;
    if !lake_success && errors.is_empty() {
        // A failure we couldn't attribute to a *located* diagnostic (unknown
        // target, lake config error, toolchain error, …). The real reason is
        // one of lake's locationless `error:` lines — and it can land on either
        // stream (the specific `error: no such file …` on stdout, the generic
        // `error: build failed` summary on stderr), so scan the combined output
        // rather than a single stream's tail.
        let mut fallback = extract_locationless_errors(&combined);
        if fallback.is_empty() && !has_located_diagnostics {
            // Nothing looked like a diagnostic or error line — surface the raw
            // tail so the failure isn't silent. Do not re-label an already
            // classified warning as an error merely because lake exited nonzero.
            let tail = stderr.trim();
            let tail = if tail.is_empty() { stdout.trim() } else { tail };
            if !tail.is_empty() {
                fallback.push(tail.to_string());
            }
        }
        errors = fallback;
    }

    if !opts.warnings && !opts.strict {
        warnings.clear();
    }

    let success = lake_success && (!opts.strict || warnings.is_empty());
    BuildReport {
        success,
        errors,
        warnings,
    }
}

/// Fallback for a failed build with **no located diagnostic**: collect lake's
/// locationless `error:` lines (unknown target, config/toolchain error, …)
/// together with their indented continuation lines (e.g. `  file: …`). Blocks
/// echoed to both streams are de-duplicated, and the bare `error: build failed`
/// / `error: Lean exited …` orchestration summaries are dropped whenever a more
/// specific error is present, since on their own they carry no information.
fn extract_locationless_errors(output: &str) -> Vec<String> {
    let mut blocks: Vec<String> = Vec::new();
    let mut current: Option<Vec<String>> = None;

    let flush = |cur: Option<Vec<String>>, out: &mut Vec<String>| {
        if let Some(buf) = cur {
            let block = buf.join("\n").trim_end().to_string();
            if !block.is_empty() {
                out.push(block);
            }
        }
    };

    for line in output.lines() {
        // Located diagnostics were already classified by `extract_diagnostics`.
        // In particular, do not promote a warning whose payload quotes `error:`.
        if diagnostic_header(line).is_some() {
            flush(current.take(), &mut blocks);
            continue;
        }
        if line.contains("error:") {
            flush(current.take(), &mut blocks);
            current = Some(vec![line.to_string()]);
            continue;
        }
        // Indented, non-blank lines continue the preceding error (lake's detail
        // lines like `  file: …`); anything else ends the block.
        if let Some(buf) = current.as_mut() {
            if !line.trim().is_empty() && line.starts_with(char::is_whitespace) {
                buf.push(line.to_string());
            } else {
                flush(current.take(), &mut blocks);
            }
        }
    }
    flush(current.take(), &mut blocks);

    // De-dupe blocks echoed to both stdout and stderr, preserving order.
    let mut seen = std::collections::HashSet::new();
    blocks.retain(|b| seen.insert(b.clone()));

    // Drop the generic orchestration summaries when a specific error survives.
    let is_generic = |b: &String| {
        let first = b.lines().next().unwrap_or("");
        first.contains("build failed") || first.contains("Lean exited with code")
    };
    if blocks.iter().any(|b| !is_generic(b)) {
        blocks.retain(|b| !is_generic(b));
    }
    blocks
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Error,
    Warning,
}

/// Grep `lake`/`lean` output into error and warning diagnostic blocks. A block
/// starts at a diagnostic header (`<loc>: error:` / `warning:`, or a line
/// starting with `error:`/`warning:`) and runs until the next header, a blank
/// line, or a build-progress line.
fn extract_diagnostics(output: &str) -> (Vec<String>, Vec<String>) {
    let mut errors = Vec::new();
    let mut warnings = Vec::new();
    let mut current: Option<(Kind, Vec<String>)> = None;

    let flush =
        |cur: Option<(Kind, Vec<String>)>, errs: &mut Vec<String>, warns: &mut Vec<String>| {
            if let Some((kind, buf)) = cur {
                let block = buf.join("\n");
                let block = block.trim_end().to_string();
                if !block.is_empty() {
                    match kind {
                        Kind::Error => errs.push(block),
                        Kind::Warning => warns.push(block),
                    }
                }
            }
        };

    for line in output.lines() {
        if let Some(kind) = diagnostic_header(line) {
            flush(current.take(), &mut errors, &mut warnings);
            current = Some((kind, vec![line.to_string()]));
            continue;
        }
        if let Some((_, buf)) = current.as_mut() {
            if line.trim().is_empty() || is_boundary(line) {
                flush(current.take(), &mut errors, &mut warnings);
            } else {
                buf.push(line.to_string());
            }
        }
        // Lines outside a block that aren't headers are build noise — dropped.
    }
    flush(current.take(), &mut errors, &mut warnings);

    (errors, warnings)
}

/// Classify a line as the start of an error/warning diagnostic, if it is one. A
/// real Lean diagnostic carries a `line:col` source location, which lets us
/// skip lake's locationless orchestration errors (`error: build failed`,
/// `error: Lean exited with code 1`). The first severity marker is authoritative:
/// wrappers such as `warning: Foo.lean:1:2: error: ...` are warnings even when
/// their quoted payload contains `error:`.
fn diagnostic_header(line: &str) -> Option<Kind> {
    if !has_location(line) {
        return None;
    }
    match (line.find("error:"), line.find("warning:")) {
        (Some(error), Some(warning)) if error < warning => Some(Kind::Error),
        (Some(_), Some(_)) => Some(Kind::Warning),
        (Some(_), None) => Some(Kind::Error),
        (None, Some(_)) => Some(Kind::Warning),
        (None, None) => None,
    }
}

/// Whether a line ends the current diagnostic block: build-progress / trace
/// noise, or one of lake's locationless `error:`/`warning:` summary lines (a
/// located diagnostic would already have been taken as a new header).
fn is_boundary(line: &str) -> bool {
    is_progress(line) || line.contains("error:") || line.contains("warning:")
}

/// Whether a line is `lake` build-progress / trace noise.
fn is_progress(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with('[')            // "[3/42] Building Foo"
        || t.starts_with("trace:")
        || t.starts_with("info:")
        || t.starts_with("Building ")
        || t.starts_with('ℹ')
        || t.starts_with('✔')
        || t.starts_with('✖')
        || t.starts_with('⚠')
        || t.starts_with('❯')
}

/// Whether `line` contains a `:<line>:<col>` source position (two consecutive
/// all-digit, colon-separated segments), as in `Foo.lean:6:18:`.
fn has_location(line: &str) -> bool {
    let segments: Vec<&str> = line.split(':').collect();
    segments.windows(2).any(|w| {
        !w[0].is_empty()
            && !w[1].is_empty()
            && w[0].bytes().all(|b| b.is_ascii_digit())
            && w[1].bytes().all(|b| b.is_ascii_digit())
    })
}

/// Render a [`BuildReport`] as compact text. `warnings_requested` controls
/// whether the (possibly empty) warnings section is shown at all.
pub fn format_human(report: &BuildReport, warnings_requested: bool) -> String {
    let mut lines = Vec::new();

    if report.success && report.errors.is_empty() {
        lines.push("✓ build succeeded".to_string());
    } else if report.errors.is_empty() && !report.warnings.is_empty() {
        let n = report.warnings.len();
        lines.push(format!(
            "✗ build failed ({n} {})",
            if n == 1 { "warning" } else { "warnings" }
        ));
    } else {
        let n = report.errors.len();
        lines.push(format!(
            "✗ build failed ({n} {})",
            if n == 1 { "error" } else { "errors" }
        ));
    }

    for err in &report.errors {
        lines.push(String::new());
        append_block(&mut lines, '✗', err);
    }

    if warnings_requested && !report.warnings.is_empty() {
        let n = report.warnings.len();
        lines.push(String::new());
        lines.push(format!(
            "⚠ {n} {}",
            if n == 1 { "warning" } else { "warnings" }
        ));
        for warn in &report.warnings {
            append_block(&mut lines, '⚠', warn);
        }
    }

    lines.join("\n")
}

/// Append a diagnostic block: the first line gets `marker`, continuation lines
/// are indented to align under it.
fn append_block(lines: &mut Vec<String>, marker: char, block: &str) {
    let mut parts = block.lines();
    if let Some(first) = parts.next() {
        lines.push(format!("  {marker} {first}"));
        for rest in parts {
            lines.push(format!("      {rest}"));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const WRAPPED_WARNING: &str = "warning: Auctionlib/Statistics/OrderStatistics.lean:272:95: error: doc-strings should end with a single space or newline\n";

    const LAKE_OUTPUT: &str = "\
[1/5] Building TestProject.Ok
[2/5] Building TestProject.Bad
info: stdout from a build step
./TestProject/Bad.lean:5:2: error: type mismatch
  h1
has type
  Prop
./TestProject/Warn.lean:3:0: warning: unused variable `x`
[3/5] Building TestProject.More
./TestProject/Imp.lean:6:0: error: failed to read file 'X.olean', incompatible header
error: Lean exited with code 1
Some required targets logged failures:
- TestProject.Imp
error: build failed
";

    #[test]
    fn extracts_error_and_warning_blocks() {
        let (errors, warnings) = extract_diagnostics(LAKE_OUTPUT);
        assert_eq!(errors.len(), 2, "two real (located) error diagnostics");
        assert!(errors[0].starts_with("./TestProject/Bad.lean:5:2: error: type mismatch"));
        assert!(errors[0].contains("has type"), "captures multi-line detail");
        assert!(!errors[0].contains("[3/5]"), "stops at progress line");
        assert!(errors[1].contains("incompatible header"));
        // lake's locationless summary lines are not counted as diagnostics, and
        // don't get absorbed into the preceding block.
        assert!(!errors[1].contains("build failed"));
        assert!(errors.iter().all(|e| !e.contains("Lean exited with code")));

        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("unused variable"));
    }

    #[test]
    fn outer_warning_severity_wins_over_nested_error_text() {
        let (errors, warnings) = extract_diagnostics(WRAPPED_WARNING);
        assert!(errors.is_empty());
        assert_eq!(warnings, vec![WRAPPED_WARNING.trim_end()]);
    }

    #[test]
    fn wrapped_warning_only_fails_in_strict_mode() {
        let normal = build_report(
            true,
            WRAPPED_WARNING,
            "",
            &BuildOptions {
                warnings: true,
                strict: false,
                targets: vec![],
            },
        );
        assert!(normal.success);
        assert!(normal.errors.is_empty());
        assert_eq!(normal.warnings.len(), 1);

        let failed_lake = build_report(
            false,
            WRAPPED_WARNING,
            "",
            &BuildOptions {
                warnings: true,
                strict: false,
                targets: vec![],
            },
        );
        assert!(
            !failed_lake.success,
            "a failed lake process still fails the build"
        );
        assert!(
            failed_lake.errors.is_empty(),
            "the wrapped warning must not be promoted by the failure fallback"
        );
        assert_eq!(failed_lake.warnings.len(), 1);

        let strict = build_report(
            true,
            WRAPPED_WARNING,
            "",
            &BuildOptions {
                warnings: false,
                strict: true,
                targets: vec![],
            },
        );
        assert!(!strict.success);
        assert!(strict.errors.is_empty());
        assert_eq!(
            strict.warnings.len(),
            1,
            "strict mode implies warning output"
        );
        assert!(format_human(&strict, true).starts_with("✗ build failed (1 warning)"));
    }

    #[test]
    fn locationless_lake_errors_are_not_diagnostics() {
        let out = "error: no such target\nerror: build failed\n";
        let (errors, _) = extract_diagnostics(out);
        assert!(errors.is_empty(), "no located diagnostics");
    }

    #[test]
    fn locationless_fallback_surfaces_specific_error() {
        // Unknown-target failure: lake prints the real reason locationless
        // (with an indented `file:` detail), then a generic `build failed`
        // summary on the other stream. The fallback keeps the former, with its
        // detail line, and drops the latter.
        let out = "\
✖ [2/2] Running TestProject.DoesNotExist
error: no such file or directory (error code: 4294967294)
  file: /abs/path/TestProject/DoesNotExist.lean
Some required targets logged failures:
- TestProject.DoesNotExist
error: build failed
";
        let blocks = extract_locationless_errors(out);
        assert_eq!(blocks.len(), 1, "generic `build failed` dropped");
        assert!(blocks[0].contains("no such file or directory"));
        assert!(blocks[0].contains("  file: /abs/path/TestProject/DoesNotExist.lean"));
        assert!(!blocks[0].contains("build failed"));
    }

    #[test]
    fn locationless_fallback_keeps_summary_when_alone() {
        // If the only locationless line is the generic summary, keep it —
        // better than a silent failure.
        let out = "error: build failed\n";
        assert_eq!(
            extract_locationless_errors(out),
            vec!["error: build failed".to_string()]
        );
    }

    #[test]
    fn locationless_fallback_dedupes_across_streams() {
        // The same error echoed to both streams (combined = stdout + stderr)
        // collapses to one block.
        let out = "error: toolchain not found\nerror: toolchain not found\n";
        assert_eq!(
            extract_locationless_errors(out),
            vec!["error: toolchain not found".to_string()]
        );
    }

    #[test]
    fn no_diagnostics_in_clean_output() {
        let clean = "[1/2] Building A\n[2/2] Building B\ninfo: done\n";
        let (errors, warnings) = extract_diagnostics(clean);
        assert!(errors.is_empty());
        assert!(warnings.is_empty());
    }

    #[test]
    fn format_success_and_failure() {
        let ok = BuildReport {
            success: true,
            errors: vec![],
            warnings: vec![],
        };
        assert_eq!(format_human(&ok, false), "✓ build succeeded");

        let bad = BuildReport {
            success: false,
            errors: vec!["./Foo.lean:5:2: error: type mismatch\n  detail".into()],
            warnings: vec![],
        };
        let text = format_human(&bad, false);
        assert!(text.starts_with("✗ build failed (1 error)"));
        assert!(text.contains("./Foo.lean:5:2: error: type mismatch"));
        assert!(text.contains("      detail"));
    }

    #[test]
    fn warnings_only_shown_when_requested() {
        let report = BuildReport {
            success: true,
            errors: vec![],
            warnings: vec!["./Foo.lean:1:0: warning: unused".into()],
        };
        assert!(!format_human(&report, false).contains("warning"));
        assert!(format_human(&report, true).contains("unused"));
    }
}
