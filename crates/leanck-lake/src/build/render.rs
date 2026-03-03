use super::BuildReport;

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
