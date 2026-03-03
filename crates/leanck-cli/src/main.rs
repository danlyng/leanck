//! Command-line tools for Lean diagnostics, builds, Loogle, and normalization.

mod args;
mod cli;
mod commands;
mod configuration;
mod telemetry;

use clap::{CommandFactory, FromArgMatches};

use cli::{Cli, Cmd};

/// Run an async command body on a fresh current-thread runtime.
fn block_on<F: std::future::Future<Output = anyhow::Result<()>>>(fut: F) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(fut)
}

/// SIGINT cancels the operation and gives its owner a bounded cleanup window.
fn block_on_live<F, Fut>(run: F) -> anyhow::Result<()>
where
    F: FnOnce(leanck_client::CancellationToken) -> Fut,
    Fut: std::future::Future<Output = anyhow::Result<()>>,
{
    block_on(async {
        let cancel = leanck_client::CancellationToken::new();
        let work = run(cancel.clone());
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => result,
            signal = tokio::signal::ctrl_c() => {
                signal?;
                cancel.cancel();
                tokio::time::timeout(std::time::Duration::from_secs(2), work).await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("operation interrupted")))
            }
        }
    })
}

fn main() {
    let mut json = std::env::args_os().any(|arg| arg == "--json");
    if let Err(error) = run(&mut json) {
        if json {
            eprintln!(
                "{}",
                serde_json::json!({"error": {"message": format!("{error:#}")}})
            );
        } else {
            eprintln!("error: {error:#}");
        }
        std::process::exit(1);
    }
}

fn run(json: &mut bool) -> anyhow::Result<()> {
    telemetry::init_tracing();

    let matches = Cli::command().get_matches();
    let mut cli = Cli::from_arg_matches(&matches)?;
    if let Cmd::Config(command) = &cli.command {
        return command.run(&cli.config);
    }
    let loaded = cli.config.load(&configuration::start(&cli.command)?)?;
    *json |= loaded.settings.output.json;
    configuration::apply(
        &mut cli.command,
        &loaded.settings,
        matches.subcommand().unwrap().1,
    );

    match cli.command {
        // Live Lean state.
        Cmd::Diagnostics(mut args) => block_on_live(|cancel| {
            args.live.cancellation = Some(cancel);
            commands::diagnostics::run(args)
        }),
        Cmd::Goal(mut args) => block_on_live(|cancel| {
            args.live.cancellation = Some(cancel);
            commands::goal::run(args)
        }),
        Cmd::Sorry(mut args) => block_on_live(|cancel| {
            args.live.cancellation = Some(cancel);
            commands::sorry::run(args)
        }),
        Cmd::List(mut args) => block_on_live(|cancel| {
            args.live.cancellation = Some(cancel);
            commands::list::run(args)
        }),
        Cmd::RestartFile(mut args) => block_on_live(|cancel| {
            args.live.cancellation = Some(cancel);
            commands::restart_file::run(args)
        }),
        Cmd::Build(mut args) => block_on_live(|cancel| {
            args.cancellation = Some(cancel);
            commands::build::run(args)
        }),
        Cmd::Normalize(args) => {
            leanck_normalize::command::run_with_settings(args, &loaded.settings.normalize)
        }
        Cmd::Config(_) => unreachable!(),

        // Loogle search.
        Cmd::Loogle(args) => block_on(commands::loogle::run(args)),

        // Lean daemon health.
        Cmd::Doctor(args) => commands::doctor::run(args),
        Cmd::Daemons(args) => block_on(commands::daemons::run(args)),
    }
}
