//! Long-lived modes select their own runtime, configuration root, and logging.
use std::path::PathBuf;

use clap::{Args as ClapArgs, Subcommand};
use leanck_config::ConfigArgs;

#[derive(ClapArgs, Debug)]
pub(crate) struct Args {
    #[command(subcommand)]
    mode: Mode,
}

#[derive(Subcommand, Debug)]
enum Mode {
    /// Serve MCP over stdin/stdout; all logs go to stderr.
    Mcp(leanck_mcp::config::Args),
    /// Serve one Lake worktree over its Unix socket, in the foreground.
    Daemon {
        /// Lake project directory.
        worktree: PathBuf,
    },
}

pub(crate) fn run(args: Args, config: ConfigArgs) -> anyhow::Result<()> {
    use tracing_subscriber::EnvFilter;

    let filter = match &args.mode {
        Mode::Mcp(_) => EnvFilter::try_from_default_env()
            .unwrap_or_else(|_| EnvFilter::new("info,leanck_mcp=debug")),
        Mode::Daemon { .. } => {
            EnvFilter::try_from_env("LEANCKD_LOG").unwrap_or_else(|_| EnvFilter::new("info"))
        }
    };
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .init();

    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    match args.mode {
        Mode::Mcp(args) => {
            let config = leanck_mcp::config::Config::from_args(args, config)?;
            let server = leanck_mcp::mcp::LeanckServer::new(&config)
                .with_daemon_binary(std::env::current_exe()?);
            runtime.block_on(leanck_mcp::serve(server))
        }
        Mode::Daemon { worktree } => {
            let root = leanck_core::workspace::WorkspaceRoot::from_path(worktree)?;
            let settings = config.load(root.as_path())?.settings;
            runtime
                .block_on(leanckd::run_with_settings(root.as_path(), settings))
                .map_err(anyhow::Error::from)
        }
    }
}
