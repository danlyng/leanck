use std::path::{Path, PathBuf};

use crate::workspace::{detect_workspace, find_project_root};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Workspace root. Defaults to the auto-detected Lake project.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Print all checks instead of just warnings and failures.
    #[arg(short, long)]
    pub(crate) verbose: bool,

    /// Emit the full machine-readable JSON health payload.
    #[arg(long)]
    pub(crate) json: bool,
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
    let workspace = match args.workspace {
        Some(p) => p,
        None => detect_workspace()?,
    };
    let workspace = workspace.canonicalize().unwrap_or(workspace);
    let mut checks = Vec::new();
    if workspace.is_dir() && find_project_root(&workspace).as_deref() == Some(&workspace) {
        checks.push(Check::new("ok", "Lake project marker found"));
        report_leanckd(&mut checks, &workspace)?;
    } else {
        checks.push(Check::new("error", "workspace has no Lake project marker (lakefile.lean, lakefile.toml, or lean-toolchain)"));
    }
    let ok = !checks.iter().any(|check| check.status == "error");
    let issues: Vec<&Check> = checks.iter().filter(|check| check.status != "ok").collect();
    if args.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": ok,
                "workspace": workspace,
                "socket_path": leanckd::paths::socket_path(&workspace),
                "issues": issues.iter().map(|check| serde_json::json!({
                    "level": check.status, "message": check.summary,
                })).collect::<Vec<_>>(),
                "checks": checks,
            }))?
        );
    } else if args.verbose {
        println!("Workspace: {}", workspace.display());
        println!(
            "Socket: {}",
            leanckd::paths::socket_path(&workspace).display()
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

/// Probe without starting a daemon: idle projects do not need a Lean process.
fn report_leanckd(checks: &mut Vec<Check>, workspace: &Path) -> anyhow::Result<()> {
    let socket = leanckd::paths::socket_path(workspace);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    let health = rt.block_on(async {
        if !leanckd::client::can_connect(&socket).await {
            return None;
        }
        Some(leanckd::client::DaemonClient::new(socket).health().await)
    });
    match health {
        None => checks.push(Check::new(
            "ok",
            "daemon not running (auto-starts on the first live-Lean query)",
        )),
        Some(Err(e)) => checks.push(Check::new(
            "error",
            format!("daemon health check failed: {e}"),
        )),
        Some(Ok(h)) => {
            if h.get("ok").and_then(|v| v.as_bool()) == Some(false) {
                checks.push(Check::new(
                    "error",
                    format!("daemon reported unhealthy: {h}"),
                ));
                return Ok(());
            }
            let pid = h
                .get("server")
                .and_then(|s| s.get("pid"))
                .and_then(|v| v.as_u64());
            let proto = h.get("protocol_version").and_then(|v| v.as_u64());
            let idle = h.get("idle_secs").and_then(|v| v.as_u64());
            let show =
                |value: Option<u64>| value.map(|n| n.to_string()).unwrap_or_else(|| "?".into());
            checks.push(Check::new(
                "ok",
                format!(
                    "daemon running (pid {}, protocol v{}, idle {}s)",
                    show(pid),
                    show(proto),
                    show(idle)
                ),
            ));
            let expected = u64::from(leanckd::proto::PROTOCOL_VERSION);
            if let Some(actual) = proto
                && actual != expected
            {
                checks.push(Check::new("warning", format!("daemon protocol v{actual} != this build's v{expected}; restart the stale daemon or wait for idle reap")));
            }
        }
    }
    Ok(())
}
