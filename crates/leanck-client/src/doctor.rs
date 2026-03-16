//! Non-spawning workspace checks shared by CLI and MCP.
use crate::{
    Client, RequestOptions,
    instances::{self, Summary},
};
use leanck_config::{OnLimit, Settings};
use leanck_core::workspace::has_project_marker;
use serde::Serialize;
use std::path::{Path, PathBuf};

#[derive(Debug, Serialize)]
pub struct Check {
    pub status: &'static str,
    pub summary: String,
}
impl Check {
    fn new(status: &'static str, summary: impl Into<String>) -> Self {
        Self {
            status,
            summary: summary.into(),
        }
    }
}
#[derive(Serialize)]
pub struct Report {
    pub ok: bool,
    pub workspace: PathBuf,
    pub socket_path: PathBuf,
    pub checks: Vec<Check>,
    pub issues: Vec<serde_json::Value>,
    pub instances: Summary,
}
impl Report {
    pub fn with_config_warnings(mut self, warnings: Vec<leanck_config::Warning>) -> Self {
        for warning in warnings {
            self.issues.push(serde_json::json!({
                "level": "warning", "message": warning.message
            }));
            self.checks.push(Check::new("warning", warning.message));
        }
        self
    }

    pub fn text(&self, verbose: bool) -> String {
        let mut lines = vec![];
        if verbose {
            lines.extend([
                format!("Workspace: {}", self.workspace.display()),
                format!("Socket: {}", self.socket_path.display()),
            ]);
        }
        lines.extend(
            self.checks
                .iter()
                .filter(|c| verbose || c.status != "ok")
                .map(|c| format!("{}: {}", c.status, c.summary)),
        );
        if lines.is_empty() {
            "ok".into()
        } else {
            lines.join("\n")
        }
    }
}
pub async fn probe(workspace: &Path, settings: &Settings) -> Report {
    let workspace = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_owned());
    let mut checks = vec![];
    let summary = instances::summary(settings);
    if !workspace.is_dir() || !has_project_marker(&workspace) {
        checks.push(Check::new("error", "workspace has no Lake project marker (lakefile.lean, lakefile.toml, or lean-toolchain)"));
    } else {
        checks.push(Check::new("ok", "Lake project marker found"));
        match Client::from_path(&workspace) {
            Err(e) => checks.push(Check::new("error", e.to_string())),
            Ok(client) => {
                match client
                    .with_settings(settings.clone())
                    .probe_existing(RequestOptions::default())
                    .await
                {
                    Ok(None) => checks.push(Check::new(
                        "ok",
                        "daemon not running (auto-starts on the first live-Lean query)",
                    )),
                    Err(e) => checks.push(Check::new(
                        "error",
                        format!("daemon health check failed: {e}"),
                    )),
                    Ok(Some(h)) => {
                        if !h.ok {
                            checks.push(Check::new("error", "daemon reported unhealthy"));
                        } else {
                            checks.push(Check::new(
                                "ok",
                                format!(
                                    "daemon running (pid {:?}, protocol v{}, idle {}s)",
                                    h.server.pid, h.protocol_version, h.idle_secs
                                ),
                            ));
                        }
                        if h.protocol_version != leanck_core::api::PROTOCOL_VERSION {
                            checks.push(Check::new("warning", format!("daemon protocol v{} != this build's v{}; restart the stale daemon", h.protocol_version, leanck_core::api::PROTOCOL_VERSION)));
                        }
                    }
                }
            }
        }
    }
    let line = if summary.limit == 0 {
        format!("instances: {} running, no limit", summary.running)
    } else {
        format!(
            "instances: {} running, limit {} (on_limit={})",
            summary.running,
            summary.limit,
            summary.on_limit.as_str()
        )
    };
    if summary.limit > 0 && summary.running >= summary.limit {
        let consequence = match summary.on_limit {
            OnLimit::Refuse => "a daemon for another worktree will be refused",
            OnLimit::EvictIdle => "a daemon for another worktree will evict the longest-idle one",
            OnLimit::Warn => "a daemon for another worktree will start past the limit",
        };
        checks.push(Check::new(
            "warning",
            format!("{line}; {consequence} (free slots with `leanck daemons stop --idle`)"),
        ));
    } else {
        checks.push(Check::new("ok", line));
    }
    Report {
        ok: !checks.iter().any(|c| c.status == "error"),
        socket_path: leanck_core::paths::socket_path(&workspace),
        workspace,
        issues: checks
            .iter()
            .filter(|c| c.status != "ok")
            .map(|c| serde_json::json!({"level": c.status, "message": c.summary}))
            .collect(),
        checks,
        instances: summary,
    }
}
