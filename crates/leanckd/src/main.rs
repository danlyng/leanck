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
    #[command(flatten)]
    config: leanck_config::ConfigArgs,
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
    let worktree =
        leanck_core::workspace::WorkspaceRoot::from_path(&args.worktree)?.into_path_buf();

    let settings = args.config.load(&worktree)?.settings;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    let result = runtime.block_on(leanckd::run_with_settings(&worktree, settings));
    match result {
        Err(error @ leanckd::Error::InstanceLimit { .. }) => {
            tracing::error!(%error, "leanckd refused to start");
            // Both locks were released by the `Err` return; drop the runtime
            // before `exit`, which skips destructors.
            drop(runtime);
            std::process::exit(leanck_core::instances::EXIT_INSTANCE_LIMIT);
        }
        result => result.map_err(anyhow::Error::from),
    }
}
