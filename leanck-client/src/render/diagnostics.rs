//! Compact diagnostic rendering from the response's pinned source.

use leanck_core::api::{Diagnostic, DiagnosticsResult, Severity};

/// Render diagnostics using only their pinned source. `file` is the original
/// caller-supplied path/URI, used as a label.
pub fn format_diagnostics_human(body: &DiagnosticsResult, file: &str) -> String {
    let label = diagnostics_label(body, file);
    let diagnostics = &body.diagnostics;
    let notes: Vec<&str> = body
        .notes
        .iter()
        .map(String::as_str)
        .filter(|note| *note != "ok")
        .collect();
    let total = body.total;

    if diagnostics.is_empty() && notes.is_empty() && total == 0 {
        return format!("  ✓ {label}");
    }

    let source_lines: Vec<_> = body.source.lines().collect();
    let mut lines = vec![format!("  {label}")];
    for (i, diagnostic) in diagnostics.iter().enumerate() {
        if i > 0 {
            lines.push(String::new());
        }
        append_diagnostic_human(&mut lines, diagnostic, &source_lines);
    }

    let offset = body.offset.unwrap_or(0);
    let shown_end = offset.saturating_add(u64::try_from(diagnostics.len()).unwrap_or(u64::MAX));
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

fn diagnostics_label(body: &DiagnosticsResult, file: &str) -> String {
    if let Some(symbol) = body.symbol.as_deref().filter(|symbol| !symbol.is_empty()) {
        return symbol.to_owned();
    }
    if !file.is_empty() {
        return file.to_owned();
    }
    body.uri.clone()
}

fn append_diagnostic_human(
    lines: &mut Vec<String>,
    diagnostic: &Diagnostic,
    source_lines: &[&str],
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

    let message = &diagnostic.message;
    for message_line in message.lines() {
        lines.push(format!("    {message_line}"));
    }
    if message.to_ascii_lowercase().contains("no goals") {
        let previous_line = line.saturating_sub(1);
        lines.push(format!(
            "    hint: the previous tactic (before L{previous_line}:{character}) closed the goal"
        ));
    }

    let goals = diagnostic.goals.as_deref().unwrap_or_default();
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

fn diagnostic_position(diagnostic: &Diagnostic) -> (u64, u64) {
    (
        u64::from(diagnostic.range.start.line) + 1,
        u64::from(diagnostic.range.start.character),
    )
}

fn diagnostic_severity(diagnostic: &Diagnostic) -> &'static str {
    match diagnostic.severity_or_error() {
        Severity::Warning => "warning",
        Severity::Information => "info",
        Severity::Hint => "hint",
        _ => "error",
    }
}

fn source_excerpt(source_lines: &[&str], one_based_line: u64) -> Option<String> {
    let idx = usize::try_from(one_based_line.checked_sub(1)?).ok()?;
    let raw = source_lines.get(idx)?.replace('\t', " ");
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    Some(truncate_chars(trimmed, 25))
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
    fn typed(mut body: serde_json::Value) -> DiagnosticsResult {
        body["uri"] = serde_json::json!("file:///nonexistent/Foo.lean");
        body["range"] = serde_json::Value::Null;
        body["version"] = serde_json::json!(1);
        body["source"] = serde_json::json!("");
        serde_json::from_value(body).unwrap()
    }

    #[test]
    fn diagnostics_format_ok_uses_checkmark() {
        let body = serde_json::json!({
            "symbol": "Foo.bar",
            "total": 0,
            "diagnostics": [],
            "notes": ["ok"],
        });

        assert_eq!(
            format_diagnostics_human(&typed(body), "Foo.lean"),
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
            format_diagnostics_human(&typed(body), "Foo.lean"),
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
            format_diagnostics_human(&typed(body), "Foo.lean"),
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
