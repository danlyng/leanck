//! Machine-wide daemon inventory: list running `leanckd` daemons and stop them.
//!
//! Daemons are found through their instance slot records. Nothing here spawns
//! a daemon or creates runtime directories.
use crate::commands::output::print_json;
use clap::{ArgGroup, Subcommand};
use leanck_client::{
    Client, RequestOptions,
    instances::{self, DaemonInfo, DaemonState, StopMode, StopOutcome, Stopped, Summary},
};
use leanck_config::Settings;
use leanck_core::paths::RuntimePaths;
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::task::JoinSet;

const AFTER_HELP: &str = color_print::cstr!(
    r#"<underline,bold>Examples:</>
  leanck daemons
  leanck daemons stop --idle
  leanck daemons stop ~/src/my-project --wait 30"#
);

#[derive(clap::Args, Debug)]
#[command(after_help = AFTER_HELP)]
pub(crate) struct Args {
    #[arg(skip)]
    pub(crate) settings: Settings,
    #[command(subcommand)]
    pub(crate) action: Option<Action>,
    /// Print machine-readable JSON instead of compact text.
    #[arg(long, global = true)]
    pub(crate) json: bool,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Action {
    /// List running daemons (the default).
    List,
    /// Ask daemons to shut down and wait for them to exit.
    Stop(StopArgs),
}

#[derive(clap::Args, Debug)]
#[command(group(
    ArgGroup::new("target")
        .required(true)
        .multiple(false)
        .args(["worktree", "idle", "all"])
))]
pub(crate) struct StopArgs {
    /// Worktree whose daemon to stop.
    pub(crate) worktree: Option<PathBuf>,
    /// Stop every ready daemon with no queued or running requests. Each one
    /// stops only if it is still idle when asked; one that has accepted work
    /// meanwhile keeps running and is reported as skipped.
    #[arg(long)]
    pub(crate) idle: bool,
    /// Stop every daemon that is serving its socket.
    #[arg(long)]
    pub(crate) all: bool,
    /// Seconds to wait for each daemon to exit.
    #[arg(long, value_name = "SECS", default_value_t = 15)]
    pub(crate) wait: u64,
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    match args.action {
        None | Some(Action::List) => list(&args.settings, args.json).await,
        Some(Action::Stop(stop_args)) => stop(&args.settings, stop_args, args.json).await,
    }
}

/// `N running, limit M (on_limit=X)`, shared with `doctor`.
pub(crate) fn summary_line(summary: &Summary) -> String {
    if summary.limit == 0 {
        format!("{} running, no limit", summary.running)
    } else {
        format!(
            "{} running, limit {} (on_limit={})",
            summary.running,
            summary.limit,
            summary.on_limit.as_str()
        )
    }
}

#[derive(Serialize)]
struct Listing<'a> {
    daemons: &'a [DaemonInfo],
    #[serde(flatten)]
    summary: Summary,
}

async fn list(settings: &Settings, json: bool) -> anyhow::Result<()> {
    let daemons = instances::list(settings).await;
    let summary = Summary {
        running: daemons.len() as u64,
        limit: settings.instances.limit,
        on_limit: settings.instances.on_limit,
    };
    if json {
        return print_json(&Listing {
            daemons: &daemons,
            summary,
        });
    }
    if daemons.is_empty() {
        println!("no leanck daemons running");
        return Ok(());
    }
    let rows: Vec<Vec<String>> = daemons.iter().map(row).collect();
    for line in align(&rows) {
        println!("{line}");
    }
    println!("{}", summary_line(&summary));
    Ok(())
}

fn row(info: &DaemonInfo) -> Vec<String> {
    let dash = |value: Option<String>| value.unwrap_or_else(|| "-".into());
    let mut state = state_name(info.state).to_owned();
    if let Some(error) = &info.error {
        state = format!("{state} ({error})");
    }
    vec![
        info.worktree.display().to_string(),
        format!("pid {}", info.pid),
        format!("up {}", duration(info.uptime_secs)),
        format!("idle {}", dash(info.idle_secs().map(duration))),
        state,
        format!(
            "active {}",
            dash(info.active_requests().map(|n| n.to_string()))
        ),
    ]
}

/// Pad every column but the last to its widest cell, separated by two spaces.
fn align(rows: &[Vec<String>]) -> Vec<String> {
    let columns = rows.iter().map(Vec::len).max().unwrap_or(0);
    let widths: Vec<usize> = (0..columns)
        .map(|column| {
            rows.iter()
                .filter_map(|cells| cells.get(column))
                .map(|cell| cell.chars().count())
                .max()
                .unwrap_or(0)
        })
        .collect();
    rows.iter()
        .map(|cells| {
            let last = cells.len().saturating_sub(1);
            cells
                .iter()
                .enumerate()
                .map(|(column, cell)| {
                    if column == last {
                        cell.clone()
                    } else {
                        format!("{cell:<width$}", width = widths[column])
                    }
                })
                .collect::<Vec<_>>()
                .join("  ")
                .trim_end()
                .to_owned()
        })
        .collect()
}

fn state_name(state: DaemonState) -> &'static str {
    match state {
        DaemonState::Starting => "starting",
        DaemonState::Ready => "ready",
        DaemonState::Recovering => "recovering",
        DaemonState::ShuttingDown => "shutting_down",
        DaemonState::Unreachable => "unreachable",
    }
}

/// Compact duration: `45s`, `12m`, `3h05m`, `2d04h`.
fn duration(secs: u64) -> String {
    match secs {
        0..60 => format!("{secs}s"),
        60..3600 => format!("{}m", secs / 60),
        3600..86400 => format!("{}h{:02}m", secs / 3600, secs % 3600 / 60),
        _ => format!("{}d{:02}h", secs / 86400, secs % 86400 / 3600),
    }
}

#[derive(Debug, Default, Serialize)]
struct StopReport {
    stopped: Vec<StopOutcome>,
    skipped: Vec<Skipped>,
    failed: Vec<Failed>,
}

#[derive(Debug, Serialize)]
struct Skipped {
    worktree: PathBuf,
    pid: Option<u32>,
    reason: String,
}

#[derive(Debug, Serialize)]
struct Failed {
    worktree: PathBuf,
    pid: Option<u32>,
    error: String,
}

/// Why `--idle` leaves a daemon running, or `None` when it is idle.
fn not_idle_reason(info: &DaemonInfo) -> Option<String> {
    if info.is_idle() {
        return None;
    }
    Some(match info.state {
        DaemonState::Ready => match info.active_requests() {
            Some(1) => "busy (1 active request)".into(),
            Some(n) => format!("busy ({n} active requests)"),
            None => "busy".into(),
        },
        state => not_serving_reason(info).unwrap_or_else(|| state_name(state).into()),
    })
}

/// Why a daemon cannot be asked to stop yet, or `None` when it can.
fn not_serving_reason(info: &DaemonInfo) -> Option<String> {
    match info.state {
        DaemonState::Starting => Some("starting (socket not bound yet)".into()),
        DaemonState::Unreachable => Some(format!(
            "unreachable: {}",
            info.error.as_deref().unwrap_or("health probe failed")
        )),
        DaemonState::Recovering => Some("recovering".into()),
        DaemonState::ShuttingDown => Some("already shutting down".into()),
        DaemonState::Ready => None,
    }
}

async fn stop(settings: &Settings, args: StopArgs, json: bool) -> anyhow::Result<()> {
    let wait = Duration::from_secs(args.wait);
    let daemons = instances::list(settings).await;
    let mut report = StopReport::default();
    let mut targets = Vec::new();
    let mut unmatched = None;
    let skip = |report: &mut StopReport, info: &DaemonInfo, reason: String| {
        report.skipped.push(Skipped {
            worktree: info.worktree.clone(),
            pid: Some(info.pid),
            reason,
        });
    };
    if let Some(worktree) = &args.worktree {
        let canonical = resolve_existing(worktree);
        let matched: Vec<_> = daemons
            .into_iter()
            .filter(|info| info.worktree == canonical)
            .collect();
        if matched.is_empty() {
            match stop_without_slot(&canonical, settings, wait).await {
                Some(Ok(outcome)) => report.stopped.push(outcome),
                Some(Err(error)) => report.failed.push(Failed {
                    worktree: canonical.clone(),
                    pid: None,
                    error: format!("{error:#}"),
                }),
                None => unmatched = Some(canonical),
            }
        }
        for info in matched {
            // Only a daemon without a bound socket cannot receive the request.
            if info.state == DaemonState::Starting {
                skip(&mut report, &info, not_serving_reason(&info).unwrap());
            } else {
                targets.push(info);
            }
        }
    } else {
        for info in daemons {
            let reason = if args.idle {
                not_idle_reason(&info)
            } else if info.state == DaemonState::Starting {
                not_serving_reason(&info)
            } else {
                None
            };
            match reason {
                Some(reason) => skip(&mut report, &info, reason),
                None => targets.push(info),
            }
        }
    }

    // `--idle` must never cancel work admitted after the listing, so the
    // daemon decides atomically whether it is still idle.
    let mode = if args.idle {
        StopMode::IfIdle
    } else {
        StopMode::Always
    };
    let mut stops = JoinSet::new();
    for (index, info) in targets.into_iter().enumerate() {
        let settings = settings.clone();
        stops.spawn(async move {
            let result = instances::stop(&info, &settings, wait, mode).await;
            (index, info, result)
        });
    }
    let mut results = stops.join_all().await;
    results.sort_by_key(|(index, ..)| *index);
    for (_, info, result) in results {
        match result {
            Ok(outcome) if !outcome.accepted => {
                skip(&mut report, &info, "busy at shutdown".into());
            }
            Ok(outcome) => report.stopped.push(outcome),
            Err(error) => report.failed.push(Failed {
                worktree: info.worktree,
                pid: Some(info.pid),
                error: error.to_string(),
            }),
        }
    }

    if json {
        print_json(&report)?;
    } else if unmatched.is_none() {
        print_stop_report(&report, args.wait, args.idle);
    }
    if let Some(worktree) = unmatched {
        anyhow::bail!(
            "no leanck daemon is running for {}; list running daemons with `leanck daemons`",
            worktree.display()
        );
    }
    match report.failed.len() {
        0 => Ok(()),
        1 => anyhow::bail!("could not stop 1 leanck daemon"),
        n => anyhow::bail!("could not stop {n} leanck daemons"),
    }
}

/// Stop a daemon that holds no slot (started before instance slots existed, or
/// from a fallback runtime directory) through its per-worktree socket. `None`
/// means no daemon is listening there.
/// `path` made absolute with its longest existing ancestor canonicalized, so a
/// removed worktree, named relatively or through a symlink such as macOS
/// `/tmp`, still matches the canonical path its daemon recorded.
fn resolve_existing(path: &Path) -> PathBuf {
    let Ok(absolute) = std::path::absolute(path) else {
        return path.to_path_buf();
    };
    absolute
        .ancestors()
        .find_map(|ancestor| {
            let canonical = ancestor.canonicalize().ok()?;
            Some(canonical.join(absolute.strip_prefix(ancestor).ok()?))
        })
        .unwrap_or(absolute)
}

async fn stop_without_slot(
    worktree: &Path,
    settings: &Settings,
    wait: Duration,
) -> Option<anyhow::Result<StopOutcome>> {
    let client = Client::from_path(worktree)
        .ok()?
        .with_settings(settings.clone());
    // An incompatible or unhealthy daemon still owns the socket and can be stopped.
    if let Ok(None) = client.probe_existing(RequestOptions::default()).await {
        return None;
    }
    let pid = std::fs::read_to_string(RuntimePaths::for_workspace(client.workspace()).pid())
        .ok()
        .and_then(|contents| contents.trim().parse::<u32>().ok());
    Some(
        instances::stop_socket(&client.socket(), pid, settings, wait, StopMode::Always)
            .await
            .map(|stopped| StopOutcome {
                pid,
                worktree: client.workspace().to_path_buf(),
                accepted: stopped != Stopped::Busy,
                exited: stopped == Stopped::Exited,
            })
            .map_err(anyhow::Error::from),
    )
}

fn print_stop_report(report: &StopReport, wait: u64, idle: bool) {
    let pid = |pid: Option<u32>| pid.map(|pid| format!(" (pid {pid})")).unwrap_or_default();
    if report.stopped.is_empty() && report.skipped.is_empty() && report.failed.is_empty() {
        if idle {
            println!("no idle leanck daemons to stop");
        } else {
            println!("no leanck daemons running");
        }
        return;
    }
    for outcome in &report.stopped {
        if outcome.exited {
            println!("stopped {}{}", outcome.worktree.display(), pid(outcome.pid));
        } else {
            println!(
                "shutdown requested for {}{}, still running after {wait}s",
                outcome.worktree.display(),
                pid(outcome.pid)
            );
        }
    }
    for skipped in &report.skipped {
        println!(
            "skipped {}{}: {}",
            skipped.worktree.display(),
            pid(skipped.pid),
            skipped.reason
        );
    }
    for failed in &report.failed {
        eprintln!(
            "failed {}{}: {}",
            failed.worktree.display(),
            pid(failed.pid),
            failed.error
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use leanck_config::OnLimit;

    #[test]
    fn durations_are_compact() {
        for (secs, text) in [
            (0, "0s"),
            (59, "59s"),
            (60, "1m"),
            (3599, "59m"),
            (3600, "1h00m"),
            (3 * 3600 + 5 * 60, "3h05m"),
            (2 * 86400 + 4 * 3600, "2d04h"),
        ] {
            assert_eq!(duration(secs), text);
        }
    }

    #[test]
    fn rows_align_columns_and_mark_missing_health() {
        let info = DaemonInfo {
            slot: 0,
            pid: 42,
            worktree: "/a".into(),
            socket: "/a.sock".into(),
            started_at: 0,
            uptime_secs: 75,
            state: DaemonState::Starting,
            health: None,
            error: None,
        };
        let mut other = info.clone();
        other.worktree = "/longer".into();
        other.pid = 7;
        other.state = DaemonState::Unreachable;
        other.error = Some("refused".into());
        let lines = align(&[row(&info), row(&other)]);
        assert_eq!(
            lines,
            [
                "/a       pid 42  up 1m  idle -  starting               active -",
                "/longer  pid 7   up 1m  idle -  unreachable (refused)  active -",
            ]
        );
        assert_eq!(
            not_idle_reason(&info).unwrap(),
            "starting (socket not bound yet)"
        );
        assert_eq!(not_idle_reason(&other).unwrap(), "unreachable: refused");
    }

    #[test]
    fn summary_line_names_the_policy_or_no_limit() {
        let mut summary = Summary {
            running: 3,
            limit: 8,
            on_limit: OnLimit::EvictIdle,
        };
        assert_eq!(
            summary_line(&summary),
            "3 running, limit 8 (on_limit=evict_idle)"
        );
        summary.limit = 0;
        assert_eq!(summary_line(&summary), "3 running, no limit");
    }
}
