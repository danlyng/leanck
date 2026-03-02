//! Command-line tools for Lean diagnostics, builds, Loogle, and normalization.

mod cli;
mod commands;
mod telemetry;
mod workspace;

use clap::Parser;

use cli::{Cli, Cmd};

/// Run an async command body on a fresh current-thread runtime.
fn block_on<F: std::future::Future<Output = anyhow::Result<()>>>(fut: F) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(fut)
}

fn main() -> anyhow::Result<()> {
    telemetry::init_tracing();

    let cli = Cli::parse();
    match cli.command {
        // Live Lean state.
        Cmd::Diagnostics(args) => block_on(commands::diagnostics::run(args)),
        Cmd::Goal(args) => block_on(commands::goal::run(args)),
        Cmd::Sorry(args) => block_on(commands::sorry::run(args)),
        Cmd::List(args) => block_on(commands::list::run(args)),
        Cmd::RestartFile(args) => block_on(commands::restart_file::run(args)),
        Cmd::Build(args) => block_on(commands::build::run(args)),
        Cmd::Normalize(args) => leanck_normalize::run(args),

        // Loogle search.
        Cmd::Loogle(args) => block_on(commands::loogle::run(args)),

        // Lean daemon health.
        Cmd::Doctor(args) => commands::doctor::run(args),
    }
}
