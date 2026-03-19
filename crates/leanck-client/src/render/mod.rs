//! Shared live-query presentation. Utility renderers remain with their utilities.
mod diagnostics;
mod health;
pub use diagnostics::format_diagnostics_human;
pub use health::{HealthIssue, HealthIssueLevel, HealthReport, HealthStatus, format_health_human};
use leanck_core::api::{GoalResult, ListResult, RestartFileResult, SorryEntry, SorryResult};

pub fn render_goal(body: &GoalResult) -> String {
    let GoalResult::Goals(body) = body else {
        return "no goal here (position is not inside a tactic block)".into();
    };
    if body.goals.is_empty() {
        return "goals accomplished (no open goals)".into();
    }
    let mut out = Vec::new();
    for (index, goal) in body.goals.iter().enumerate() {
        if body.goals.len() > 1 {
            out.push(format!("goal {}:", index + 1));
        }
        out.push(goal.clone());
        if index + 1 < body.goals.len() {
            out.push(String::new());
        }
    }
    out.join("\n")
}

pub fn render_sorry(body: &SorryResult) -> String {
    let notes = match body {
        SorryResult::Directory(b) => &b.notes,
        SorryResult::File(b) => &b.notes,
    };
    with_notes(render_sorry_body(body), notes)
}
fn render_sorry_body(body: &SorryResult) -> String {
    match body {
        SorryResult::Directory(body) => {
            if body.files.is_empty() {
                return "no sorries found".into();
            }
            let mut out = vec![format!("{} sorries", body.total)];
            for file in &body.files {
                out.push(format!("  {}", display_uri(&file.uri)));
                out.extend(
                    file.sorries
                        .iter()
                        .map(|entry| format!("    {}", sorry_line(entry))),
                );
                out.extend(file.notes.iter().map(|note| format!("    note: {note}")));
            }
            out.join("\n")
        }
        SorryResult::File(body) => {
            if body.sorries.is_empty() {
                return format!("no sorries in {}", display_uri(&body.uri));
            }
            let mut out = vec![format!(
                "{} ({} sorries)",
                display_uri(&body.uri),
                body.sorries.len()
            )];
            out.extend(
                body.sorries
                    .iter()
                    .map(|entry| format!("  {}", sorry_line(entry))),
            );
            out.join("\n")
        }
    }
}

fn sorry_line(entry: &SorryEntry) -> String {
    let kind = match entry.kind {
        leanck_core::api::SorryKind::Declaration => "declaration",
        leanck_core::api::SorryKind::Tactic => "tactic",
    };
    format!("L{} [{kind}]", u64::from(entry.range.start.line) + 1)
}

pub fn render_list(body: &ListResult) -> String {
    with_notes(render_list_body(body), &body.notes)
}
fn with_notes(mut text: String, notes: &[String]) -> String {
    for note in notes {
        text.push_str("\n  note: ");
        text.push_str(note);
    }
    text
}
fn render_list_body(body: &ListResult) -> String {
    if body.symbols.is_empty() {
        return "no declarations".into();
    }
    let mut out = vec![format!("{} declarations", body.symbols.len())];
    for symbol in &body.symbols {
        let line = u64::from(symbol.range.start.line) + 1;
        out.push(format!(
            "  L{line:<5} {:<10} {}",
            symbol.kind, symbol.full_name
        ));
    }
    out.join("\n")
}

pub fn render_restart(body: &RestartFileResult) -> String {
    format!("restarted {}", display_uri(&body.uri))
}

fn display_uri(uri: &str) -> String {
    leanck_core::document::uri_to_path(uri)
        .map(|path| path.display().to_string())
        .unwrap_or_else(|_| uri.into())
}

#[cfg(test)]
mod tests;
