//! Shared plumbing for the live-Lean CLI commands (diagnostics, goal, sorry,
//! list, restart_file). These speak to the same `leanckd` daemon the MCP server
//! uses — auto-spawning one warm `lake serve` per worktree — so a human running
//! `leanck diagnostics Foo.lean` hits the identical elaborator an agent would.

use std::path::PathBuf;

use leanckd::client::{self, DaemonClient};
use serde_json::Value;

use crate::workspace::detect_workspace;

/// Resolve the workspace (auto-detecting when not given) and connect to its
/// daemon, spawning it if nothing is listening. `timeout_secs` becomes the
/// per-request deadline (and stretches the daemon's elaboration wait).
pub(crate) async fn connect(
    workspace: Option<PathBuf>,
    timeout_secs: Option<u64>,
) -> anyhow::Result<(PathBuf, DaemonClient)> {
    let ws = match workspace {
        Some(p) => p,
        None => detect_workspace()?,
    };
    let ws = ws.canonicalize().unwrap_or(ws);
    let client = client::connect_or_spawn(&ws)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?
        .with_timeout_ms(timeout_secs.map(|s| s.saturating_mul(1000)));
    Ok((ws, client))
}

/// Render a `goal` response (the `$/lean/plainGoal` payload, `rendered` stripped).
pub(crate) fn render_goal(body: &Value) -> String {
    if body.is_null() {
        return "no goal here (position is not inside a tactic block)".to_string();
    }
    let goals: Vec<&str> = body
        .get("goals")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|v| v.as_str())
        .collect();
    if goals.is_empty() {
        return "goals accomplished (no open goals)".to_string();
    }
    let mut out = Vec::new();
    for (i, goal) in goals.iter().enumerate() {
        if goals.len() > 1 {
            out.push(format!("goal {}:", i + 1));
        }
        out.push((*goal).to_string());
        if i + 1 < goals.len() {
            out.push(String::new());
        }
    }
    out.join("\n")
}

/// Render a `sorry` response (single file or recursive directory scan).
pub(crate) fn render_sorry(body: &Value) -> String {
    // Directory mode: `{ uri, total, files: [{ uri, sorries }] }`.
    if let Some(files) = body.get("files").and_then(|v| v.as_array()) {
        let total = body.get("total").and_then(|v| v.as_u64()).unwrap_or(0);
        if total == 0 {
            return "no sorries found".to_string();
        }
        let mut out = vec![format!("{total} sorries")];
        for file in files {
            let uri = file.get("uri").and_then(|v| v.as_str()).unwrap_or("?");
            out.push(format!("  {}", strip_file_uri(uri)));
            for entry in file.get("sorries").and_then(|v| v.as_array()).into_iter().flatten() {
                out.push(format!("    {}", sorry_line(entry)));
            }
        }
        return out.join("\n");
    }

    // Single-file mode: `{ uri, sorries }`.
    let uri = body.get("uri").and_then(|v| v.as_str()).unwrap_or("?");
    let sorries: Vec<&Value> = body
        .get("sorries")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .collect();
    if sorries.is_empty() {
        return format!("no sorries in {}", strip_file_uri(uri));
    }
    let mut out = vec![format!("{} ({} sorries)", strip_file_uri(uri), sorries.len())];
    for entry in sorries {
        out.push(format!("  {}", sorry_line(entry)));
    }
    out.join("\n")
}

fn sorry_line(entry: &Value) -> String {
    let line = entry
        .get("range")
        .and_then(|r| r.get("start"))
        .and_then(|s| s.get("line"))
        .and_then(|v| v.as_u64())
        .unwrap_or(0)
        + 1;
    let kind = entry.get("kind").and_then(|v| v.as_str()).unwrap_or("sorry");
    format!("L{line} [{kind}]")
}

/// Render a `list` response (`{ uri, symbols: [{ fullName, kind, range }] }`).
pub(crate) fn render_list(body: &Value) -> String {
    let symbols: Vec<&Value> = body
        .get("symbols")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .collect();
    if symbols.is_empty() {
        return "no declarations".to_string();
    }
    let mut out = vec![format!("{} declarations", symbols.len())];
    for sym in symbols {
        let line = sym
            .get("range")
            .and_then(|r| r.get("start"))
            .and_then(|s| s.get("line"))
            .and_then(|v| v.as_u64())
            .unwrap_or(0)
            + 1;
        let kind = sym.get("kind").and_then(|v| v.as_str()).unwrap_or("?");
        let name = sym
            .get("fullName")
            .and_then(|v| v.as_str())
            .or_else(|| sym.get("name").and_then(|v| v.as_str()))
            .unwrap_or("?");
        out.push(format!("  L{line:<5} {kind:<10} {name}"));
    }
    out.join("\n")
}

/// Render a `restart_file` response (`{ uri, restarted }`).
pub(crate) fn render_restart(body: &Value) -> String {
    let uri = body.get("uri").and_then(|v| v.as_str()).unwrap_or("?");
    format!("restarted {}", strip_file_uri(uri))
}

/// Trim a `file://` prefix for terser human output; leave other strings as-is.
fn strip_file_uri(uri: &str) -> String {
    uri.strip_prefix("file://").unwrap_or(uri).to_string()
}

/// Pretty-print a JSON value (for `--json`).
pub(crate) fn print_json(body: &Value) -> anyhow::Result<()> {
    println!("{}", serde_json::to_string_pretty(body)?);
    Ok(())
}
