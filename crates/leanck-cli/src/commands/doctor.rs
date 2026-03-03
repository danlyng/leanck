use std::path::Path;

use crate::args::{OutputArgs, WorkspaceArgs};
use crate::commands::daemons::summary_line;
use leanck_client::instances::Summary;
use leanck_client::{Client, RequestOptions};
use leanck_config::OnLimit;
use leanck_core::workspace::has_project_marker;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[arg(skip)]
    pub(crate) settings: leanck_config::Settings,
    #[command(flatten)]
    pub(crate) workspace: WorkspaceArgs,

    /// Print all checks instead of just warnings and failures.
    #[arg(short, long)]
    pub(crate) verbose: bool,

    #[command(flatten)]
    pub(crate) output: OutputArgs,
}

#[derive(serde::Serialize)]
struct Check {
    status: &'static str,
    summary: String,
}

impl Check {
    fn new(status: &'static str, summary: impl Into<String>) -> Self {
        Self {
            status,
            summary: summary.into(),
        }
    }
}

pub(crate) fn run(args: Args) -> anyhow::Result<()> {
    let workspace = match args.workspace.workspace {
        Some(p) => p,
        None => leanck_core::workspace::WorkspaceRoot::discover_current()?.into_path_buf(),
    };
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let mut checks = Vec::new();
    let instances = leanck_client::instances::summary(&args.settings);
    if workspace.is_dir() && has_project_marker(&workspace) {
        checks.push(Check::new("ok", "Lake project marker found"));
        report_leanckd(&mut checks, &workspace, args.settings)?;
    } else {
        checks.push(Check::new("error", "workspace has no Lake project marker (lakefile.lean, lakefile.toml, or lean-toolchain)"));
    }
    checks.push(instances_check(&instances));
    let ok = !checks.iter().any(|check| check.status == "error");
    let issues: Vec<&Check> = checks.iter().filter(|check| check.status != "ok").collect();
    if args.output.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": ok,
                "workspace": workspace,
                "socket_path": leanck_core::paths::socket_path(&workspace),
                "issues": issues.iter().map(|check| serde_json::json!({
                    "level": check.status, "message": check.summary,
                })).collect::<Vec<_>>(),
                "checks": checks,
                "instances": instances,
            }))?
        );
    } else if args.verbose {
        println!("Workspace: {}", workspace.display());
        println!(
            "Socket: {}",
            leanck_core::paths::socket_path(&workspace).display()
        );
        for check in &checks {
            println!("{}: {}", check.status, check.summary);
        }
    } else if issues.is_empty() {
        println!("ok");
    } else {
        for check in issues {
            println!("{}: {}", check.status, check.summary);
        }
    }
    if !ok {
        anyhow::bail!("one or more doctor checks failed");
    }
    Ok(())
}

/// Machine-wide count from slot records; reads only, never creates the slot directory.
fn instances_check(summary: &Summary) -> Check {
    let line = format!("instances: {}", summary_line(summary));
    if summary.limit > 0 && summary.running >= summary.limit {
        let consequence = match summary.on_limit {
            OnLimit::Refuse => "a daemon for another worktree will be refused",
            OnLimit::EvictIdle => "a daemon for another worktree will evict the longest-idle one",
            OnLimit::Warn => "a daemon for another worktree will start past the limit",
        };
        Check::new(
            "warning",
            format!("{line}; {consequence} (free slots with `leanck daemons stop --idle`)"),
        )
    } else {
        Check::new("ok", line)
    }
}

/// Probe without starting a daemon: idle projects do not need a Lean process.
fn report_leanckd(
    checks: &mut Vec<Check>,
    workspace: &Path,
    settings: leanck_config::Settings,
) -> anyhow::Result<()> {
    let client = Client::from_path(workspace)?.with_settings(settings);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let health = rt.block_on(client.probe_existing(RequestOptions::default()));
    match health {
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
                checks.push(Check::new(
                    "error",
                    format!("daemon reported unhealthy: {}", serde_json::to_string(&h)?),
                ));
                return Ok(());
            }
            let pid = h.server.pid.map(u64::from);
            let proto = u64::from(h.protocol_version);
            let idle = h.idle_secs;
            let show =
                |value: Option<u64>| value.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
            checks.push(Check::new(
                "ok",
                format!(
                    "daemon running (pid {}, protocol v{}, idle {}s)",
                    show(pid),
                    proto,
                    idle
                ),
            ));
            let expected = u64::from(leanck_core::api::PROTOCOL_VERSION);
            if proto != expected {
                let actual = proto;
                checks.push(Check::new("warning", format!("daemon protocol v{actual} != this build's v{expected}; restart the stale daemon or wait for idle reap")));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn instances_check_warns_only_at_a_nonzero_limit() {
        for (running, limit, status) in
            [(0, 8, "ok"), (7, 8, "ok"), (8, 8, "warning"), (9, 0, "ok")]
        {
            let check = instances_check(&Summary {
                running,
                limit,
                on_limit: OnLimit::Refuse,
            });
            assert_eq!(check.status, status, "{running}/{limit}");
            assert!(
                check
                    .summary
                    .starts_with(&format!("instances: {running} running"))
            );
        }
        let check = instances_check(&Summary {
            running: 8,
            limit: 8,
            on_limit: OnLimit::Refuse,
        });
        assert_eq!(
            check.summary,
            "instances: 8 running, limit 8 (on_limit=refuse); a daemon for another worktree will be refused (free slots with `leanck daemons stop --idle`)"
        );
    }
}
