use crate::{LoogleResult, SearchOutcome};

pub fn format_results(result: &LoogleResult) -> String {
    let mut lines = Vec::new();
    let shown = result.hits.len();
    if result.count != shown || result.count == 0 {
        lines.push(format!("  {} results, showing {}", result.count, shown));
    }
    for hit in &result.hits {
        let mod_str = if hit.module.is_empty() {
            String::new()
        } else {
            format!("  ({})", hit.module)
        };
        let name = if hit.name.is_empty() { "?" } else { &hit.name };
        lines.push(format!("  {name}{mod_str}"));
        lines.push(format!("    {}", hit.type_sig));
    }
    if !result.suggestions.is_empty() {
        lines.push(String::new());
        lines.push(format!("  Suggestions: {}", result.suggestions.join(", ")));
    }
    if let Some(error) = &result.error
        && !error.is_empty()
    {
        lines.push(format!("  Error: {error}"));
    }
    if let Some(note) = &result.note
        && !note.is_empty()
    {
        lines.push(format!("  Note: {note}"));
    }
    lines.join("\n")
}

/// Identical presentation in CLI and MCP, including the actual response endpoint.
pub fn format_human(outcome: &SearchOutcome) -> String {
    format!(
        "{}\n  Endpoint: {}",
        format_results(&outcome.result),
        outcome.provenance.url
    )
}
