//! `leanckd` — the daemon binary. Owns one warm `lake serve` for a worktree and
//! serves proof queries over a Unix socket.
//!
//! Usage: `leanckd <worktree>`

use std::path::PathBuf;

use clap::Parser;

#[derive(Parser, Debug)]
#[command(name = "leanckd", about = "Lean worktree LSP daemon for leanck")]
struct Args {
    /// Worktree root (the Lake project directory).
    worktree: PathBuf,
}

fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_env("LEANCKD_LOG")
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(std::io::stderr)
        .init();

    let args = Args::parse();
    let worktree = args
        .worktree
        .canonicalize()
        .unwrap_or_else(|_| args.worktree.clone());

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async move {
        leanckd::daemon::run(&worktree).await.map_err(anyhow::Error::from)
    })
}
