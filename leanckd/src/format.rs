//! Compact, token-frugal renderers for daemon JSON payloads.
//!
//! These turn the daemon's structured `diagnostics` response into the terse
//! text both the MCP server (to save agent tokens) and the human CLI render.
//! The daemon itself always returns JSON; formatting is a client-side concern,
//! but it lives here so the two front-ends render identically.

use std::path::{Path, PathBuf};

use serde_json::Value;

/// Render a `diagnostics` response as compact text. `file` is the original
/// caller-supplied path/URI (used as a label and to locate source for
/// excerpts); `workspace` resolves relative paths.
pub fn format_diagnostics_human(body: &Value, file: &str, workspace: &Path) -> String {
    let label = diagnostics_label(body, file);
    let diagnostics = body
        .get("diagnostics")
        .and_then(|v| v.as_array())
        .map(Vec::as_slice)
        .unwrap_or(&[]);
    let notes: Vec<&str> = body
        .get("notes")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .filter(|note| *note != "ok")
        .collect();
    let total = body
        .get("total")
        .and_then(|v| v.as_u64())
        .unwrap_or(diagnostics.len() as u64);

    if diagnostics.is_empty() && notes.is_empty() && total == 0 {
        return format!("  ✓ {label}");
    }

    let source_lines = read_source_lines(file, body, workspace);
    let mut lines = vec![format!("  {label}")];
    for (i, diagnostic) in diagnostics.iter().enumerate() {
        if i > 0 {
            lines.push(String::new());
        }
        append_diagnostic_human(&mut lines, diagnostic, source_lines.as_deref());
    }

    let offset = body.get("offset").and_then(|v| v.as_u64()).unwrap_or(0);
    let shown_end = offset + diagnostics.len() as u64;
    if total > shown_end {
        if !diagnostics.is_empty() {
            lines.push(String::new());
        }
        let remaining = total - shown_end;
        let noun = if remaining == 1 {
            "diagnostic"
        } else {
            "diagnostics"
        };
        lines.push(format!(
            "  ... and {remaining} more {noun}; use offset={shown_end}"
        ));
    }

    if !notes.is_empty() {
        if lines.last().is_some_and(|line| !line.is_empty()) && !diagnostics.is_empty() {
            lines.push(String::new());
        }
        for note in notes {
            lines.push(format!("  note: {note}"));
        }
    }

    lines.join("\n")
}

fn diagnostics_label(body: &Value, file: &str) -> String {
    if let Some(symbol) = body.get("symbol").and_then(|v| v.as_str())
        && !symbol.is_empty()
    {
        return symbol.to_string();
    }
    if !file.is_empty() {
        return file.to_string();
    }
    body.get("uri")
        .and_then(|v| v.as_str())
        .unwrap_or("file")
        .to_string()
}

fn append_diagnostic_human(
    lines: &mut Vec<String>,
    diagnostic: &Value,
    source_lines: Option<&[String]>,
) {
    let (line, character) = diagnostic_position(diagnostic);
    let severity = diagnostic_severity(diagnostic);
    let excerpt = source_excerpt(source_lines, line);
    match excerpt {
        Some(excerpt) => lines.push(format!(
            "  L{line}:{character} [{severity}] (at {excerpt}):"
        )),
        None => lines.push(format!("  L{line}:{character} [{severity}]:")),
    }

    if let Some(message) = diagnostic.get("message").and_then(|v| v.as_str()) {
        for message_line in message.lines() {
            lines.push(format!("    {message_line}"));
        }
        if message.to_ascii_lowercase().contains("no goals") {
            let previous_line = line.saturating_sub(1);
            lines.push(format!(
                "    hint: the previous tactic (before L{previous_line}:{character}) closed the goal"
            ));
        }
    }

    let goals: Vec<&str> = diagnostic
        .get("goals")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .collect();
    for (idx, goal) in goals.iter().enumerate() {
        let label = if goals.len() == 1 {
            "goal".to_string()
        } else {
            format!("goal {}", idx + 1)
        };
        lines.push(format!("    {label}:"));
        for goal_line in compact_goal_lines(goal) {
            lines.push(format!("      {goal_line}"));
        }
    }
}

fn diagnostic_position(diagnostic: &Value) -> (u64, u64) {
    let start = diagnostic
        .get("range")
        .and_then(|v| v.get("start"))
        .unwrap_or(&Value::Null);
    let line = start.get("line").and_then(|v| v.as_u64()).unwrap_or(0) + 1;
    let character = start.get("character").and_then(|v| v.as_u64()).unwrap_or(0);
    (line, character)
}

fn diagnostic_severity(diagnostic: &Value) -> &'static str {
    match diagnostic.get("severity").and_then(|v| v.as_u64()) {
        Some(2) => "warning",
        Some(3) => "info",
        Some(4) => "hint",
        _ => "error",
    }
}

fn source_excerpt(source_lines: Option<&[String]>, one_based_line: u64) -> Option<String> {
    let idx = one_based_line.checked_sub(1)? as usize;
    let raw = source_lines?.get(idx)?.replace('\t', " ");
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_chars(trimmed, 25))
}

fn read_source_lines(file: &str, body: &Value, workspace: &Path) -> Option<Vec<String>> {
    let mut candidates = Vec::new();
    if let Some(path) = local_path_from_input(file, workspace) {
        candidates.push(path);
    }
    if let Some(uri) = body.get("uri").and_then(|v| v.as_str())
        && let Some(path) = local_path_from_input(uri, workspace)
    {
        candidates.push(path);
    }
    candidates.into_iter().find_map(|path| {
        std::fs::read_to_string(path)
            .ok()
            .map(|text| text.lines().map(ToOwned::to_owned).collect())
    })
}

fn local_path_from_input(input: &str, workspace: &Path) -> Option<PathBuf> {
    if input.trim().is_empty() {
        return None;
    }
    if input.starts_with("file:") {
        return url::Url::parse(input)
            .ok()
            .and_then(|url| url.to_file_path().ok());
    }
    let path = PathBuf::from(input);
    Some(if path.is_absolute() {
        path
    } else {
        workspace.join(path)
    })
}

fn compact_goal_lines(goal: &str) -> Vec<String> {
    let lines: Vec<String> = goal
        .lines()
        .map(|line| line.trim_end().to_string())
        .collect();
    let Some(turnstile_idx) = lines
        .iter()
        .position(|line| line.trim_start().starts_with('⊢'))
    else {
        return conclusion_only_goal_lines(&lines);
    };

    let mut out = Vec::new();
    let context = &lines[..turnstile_idx];
    if context.len() > 4 {
        out.extend(
            context
                .iter()
                .take(2)
                .map(|line| line.trim_start().to_string()),
        );
        out.push("...".to_string());
        out.extend(
            context
                .iter()
                .skip(context.len() - 2)
                .map(|line| line.trim_start().to_string()),
        );
    } else {
        out.extend(context.iter().map(|line| line.trim_start().to_string()));
    }
    out.extend(
        lines[turnstile_idx..]
            .iter()
            .map(|line| line.trim_start().to_string()),
    );
    out
}

fn conclusion_only_goal_lines(lines: &[String]) -> Vec<String> {
    let mut nonempty = lines.iter().map(|line| line.trim_start());
    let Some(first) = nonempty.next() else {
        return vec!["⊢ ?".to_string()];
    };
    if first.is_empty() && lines.len() == 1 {
        return vec!["⊢ ?".to_string()];
    }
    let mut out = vec![format!("⊢ {first}")];
    out.extend(nonempty.map(|line| line.to_string()));
    out
}

fn truncate_chars(text: &str, max_chars: usize) -> String {
    let mut out: String = text.chars().take(max_chars).collect();
    if text.chars().count() > max_chars {
        out.push('…');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_format_ok_uses_checkmark() {
        let body = serde_json::json!({
            "symbol": "Foo.bar",
            "total": 0,
            "diagnostics": [],
            "notes": ["ok"],
        });

        assert_eq!(
            format_diagnostics_human(&body, "Foo.lean", Path::new(".")),
            "  ✓ Foo.bar"
        );
    }

    #[test]
    fn diagnostics_format_collapses_large_goal_context() {
        let body = serde_json::json!({
            "symbol": "Foo.bar",
            "total": 1,
            "diagnostics": [{
                "range": {
                    "start": { "line": 187, "character": 12 },
                    "end": { "line": 187, "character": 20 }
                },
                "severity": 1,
                "message": "unknown identifier 'mul_prod_left'",
                "goals": [
                    "n : Nat\nm : Nat\na : Nat\nb : Nat\nc : Nat\nd : Nat\n⊢ n = m"
                ]
            }],
            "notes": ["declaration contains sorry at line 203"],
        });

        assert_eq!(
            format_diagnostics_human(&body, "Foo.lean", Path::new(".")),
            concat!(
                "  Foo.bar\n",
                "  L188:12 [error]:\n",
                "    unknown identifier 'mul_prod_left'\n",
                "    goal:\n",
                "      n : Nat\n",
                "      m : Nat\n",
                "      ...\n",
                "      c : Nat\n",
                "      d : Nat\n",
                "      ⊢ n = m\n",
                "\n",
                "  note: declaration contains sorry at line 203"
            )
        );
    }

    #[test]
    fn diagnostics_format_adds_turnstile_to_conclusion_only_goal() {
        let body = serde_json::json!({
            "symbol": "Foo.bar",
            "total": 1,
            "diagnostics": [{
                "range": {
                    "start": { "line": 0, "character": 2 },
                    "end": { "line": 0, "character": 7 }
                },
                "severity": 1,
                "message": "unsolved goals",
                "goals": ["n = m"]
            }],
            "notes": [],
        });

        assert_eq!(
            format_diagnostics_human(&body, "Foo.lean", Path::new(".")),
            concat!(
                "  Foo.bar\n",
                "  L1:2 [error]:\n",
                "    unsolved goals\n",
                "    goal:\n",
                "      ⊢ n = m"
            )
        );
    }
}
