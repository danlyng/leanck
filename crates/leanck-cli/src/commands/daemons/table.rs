//! Terminal-only daemon tables, stacked on narrow terminals to retain every field.
use super::{compatibility, duration, state_name};
use comfy_table::{ColumnConstraint, ContentArrangement, Table, Width, presets::UTF8_FULL};
use leanck_client::instances::DaemonInfo;

pub(super) fn render(daemons: &[DaemonInfo], verbose: bool, width: u16) -> String {
    let mut headers = vec!["Worktree", "PID", "Uptime", "Idle", "State", "Active"];
    if verbose {
        headers.extend(["Version", "Compatible"]);
    }
    let rows: Vec<_> = daemons.iter().map(|info| values(info, verbose)).collect();
    render_rows(&headers, &rows, width, if verbose { 120 } else { 72 })
}

fn values(info: &DaemonInfo, verbose: bool) -> Vec<String> {
    let mut state = state_name(info.state).to_owned();
    if let Some(error) = &info.error {
        state = format!("{state} ({error})");
    }
    let mut cells = vec![
        info.worktree.display().to_string(),
        info.pid.to_string(),
        duration(info.uptime_secs),
        info.idle_secs().map(duration).unwrap_or_else(|| "-".into()),
        state,
        info.active_requests()
            .map(|count| count.to_string())
            .unwrap_or_else(|| "-".into()),
    ];
    if verbose {
        cells.push(info.version.as_deref().unwrap_or("unknown").to_owned());
        cells.push(
            match compatibility::check(info.version.as_deref()) {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            }
            .into(),
        );
    }
    cells
}

fn table(width: u16) -> Table {
    let mut table = Table::new();
    table
        .load_style(UTF8_FULL)
        .set_content_arrangement(ContentArrangement::Dynamic)
        .set_width(width);
    table
}

fn render_rows(headers: &[&str], rows: &[Vec<String>], width: u16, wide_at: u16) -> String {
    if width >= wide_at {
        let mut table = table(width);
        table.set_header(headers.iter().copied());
        table.add_rows(rows.iter().cloned());
        table.to_string()
    } else {
        rows.iter()
            .map(|row| {
                let mut table = table(width);
                table.set_header(["Field", "Value"]);
                for (header, value) in headers.iter().zip(row) {
                    table.add_row([*header, value]);
                }
                let label_width = headers
                    .iter()
                    .map(|header| header.len() as u16)
                    .max()
                    .unwrap_or(5)
                    + 2;
                table.set_constraints([ColumnConstraint::LowerBoundary(Width::Fixed(
                    label_width.min(width.saturating_sub(10)),
                ))]);
                table.to_string()
            })
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tables_retain_fields_and_fit_wide_and_narrow_terminals() {
        let headers = ["Worktree", "PID", "State", "Version", "Compatible"];
        let rows = vec![vec![
            "/long/project with spaces/and/a/long/name".into(),
            "12345".into(),
            "ready".into(),
            "0.39.0".into(),
            "no".into(),
        ]];
        for width in [24, 40, 80, 160] {
            let output = render_rows(&headers, &rows, width, 120);
            assert!(output.contains("Compatible"));
            assert!(output.contains("12345"));
            assert!(output.contains("no"));
            assert!(
                output
                    .lines()
                    .all(|line| line.chars().count() <= usize::from(width)),
                "width {width}:\n{output}"
            );
            assert!(output.contains('│'));
        }
    }
}
