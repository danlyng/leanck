use crate::args::{OutputArgs, WorkspaceArgs};
#[cfg(test)]
use crate::commands::daemons::summary_line;
#[cfg(test)]
use leanck_client::instances::Summary;
#[cfg(test)]
use leanck_config::OnLimit;

#[derive(clap::Args, Debug)]
#[command(after_long_help = "Examples:\n  leanck doctor\n  leanck doctor --verbose")]
pub(crate) struct Args {
    #[arg(skip)]
    pub(crate) settings: leanck_config::Settings,
    #[arg(skip)]
    pub(crate) config_warnings: Vec<leanck_config::Warning>,
    #[command(flatten)]
    pub(crate) workspace: WorkspaceArgs,

    /// Print all checks instead of just warnings and failures.
    #[arg(short, long)]
    pub(crate) verbose: bool,

    #[command(flatten)]
    pub(crate) output: OutputArgs,
}

#[cfg(test)]
#[derive(serde::Serialize)]
struct Check {
    status: &'static str,
    summary: String,
}

#[cfg(test)]
impl Check {
    fn new(status: &'static str, summary: impl Into<String>) -> Self {
        Self {
            status,
            summary: summary.into(),
        }
    }
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let workspace = args.workspace.workspace.unwrap_or_else(|| {
        leanck_core::workspace::WorkspaceRoot::discover_current()
            .map(|r| r.into_path_buf())
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default())
    });
    let (report, updates) = tokio::join!(
        leanck_client::doctor::probe(&workspace, &args.settings),
        leanck_update::current_status(env!("CARGO_PKG_VERSION"), &args.settings.updates)
    );
    let report = report.with_config_warnings(args.config_warnings);
    if args.output.json {
        let mut value = serde_json::to_value(&report)?;
        value["updates"] = serde_json::to_value(&updates)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", report.text(args.verbose));
        if let Some(notice) = &updates.notice {
            println!("update: {notice}");
        }
        if let Some(error) = &updates.error {
            println!("info: update check unavailable: {error}");
        }
    }
    if !report.ok {
        anyhow::bail!("one or more doctor checks failed");
    }
    Ok(())
}

/// Machine-wide count from slot records; reads only, never creates the slot directory.
#[cfg(test)]
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
