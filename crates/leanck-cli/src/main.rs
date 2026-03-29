//! Command-line tools for Lean diagnostics, builds, Loogle, and normalization.

mod args;
mod cli;
mod commands;
mod configuration;
mod help;
mod logging;

use clap::{CommandFactory, FromArgMatches};

use cli::{Cli, Cmd};
use commands::{completions, serve};

/// Run an async command body on a fresh current-thread runtime.
fn block_on<T, F: std::future::Future<Output = anyhow::Result<T>>>(fut: F) -> anyhow::Result<T> {
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
        // Register before starting work; `ctrl_c()` installs its handler only
        // when first polled, which can be after the work has spawned children.
        let mut interrupt =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
        let cancel = leanck_client::CancellationToken::new();
        let work = run(cancel.clone());
        tokio::pin!(work);
        tokio::select! {
            result = &mut work => result,
            _ = interrupt.recv() => {
                cancel.cancel();
                tokio::time::timeout(std::time::Duration::from_secs(2), work).await
                    .unwrap_or_else(|_| Err(anyhow::anyhow!("operation interrupted")))
            }
        }
    })
}

fn main() {
    // The installer owns its argument errors, exit codes, and JSON outcome.
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    if argv.get(1).is_some_and(|arg| arg == "_install") {
        std::process::exit(leanck_update::installer::main(
            argv[2..].to_vec(),
            env!("CARGO_PKG_VERSION"),
        ));
    }
    // Uninstalling takes the maintenance lock itself, never a usage lease.
    if argv.get(1).is_some_and(|arg| arg == "_uninstall") {
        std::process::exit(leanck_update::installer::uninstall::main(
            argv[2..].to_vec(),
        ));
    }
    let mut json = std::env::args_os().any(|arg| arg == "--json");
    if let Err(error) = run(&mut json) {
        let instance_limit = matches!(
            error.downcast_ref::<leanckd::Error>(),
            Some(leanckd::Error::InstanceLimit { .. })
        );
        if json {
            eprintln!(
                "{}",
                serde_json::json!({"error": {"message": format!("{error:#}")}})
            );
        } else {
            eprintln!("error: {error:#}");
        }
        std::process::exit(if instance_limit {
            leanck_core::instances::EXIT_INSTANCE_LIMIT
        } else {
            1
        });
    }
}

fn run(json: &mut bool) -> anyhow::Result<()> {
    let matches = Cli::command().get_matches();
    let mut cli = Cli::from_arg_matches(&matches)?;
    if let Cmd::Completions(args) = cli.command {
        return completions::run(args);
    }
    if let Cmd::UpdateInternal(args) = cli.command {
        return commands::update::internal(args);
    }
    if let Cmd::Serve(args) = cli.command {
        let _lease = block_on(commands::update::foreground(
            &leanck_config::Settings::default(),
            false,
        ))?;
        return serve::run(args, cli.config);
    }
    logging::init_tracing();
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
    if let Cmd::Doctor(args) = &mut cli.command {
        args.config_warnings = loaded.warnings;
    }
    let _lease = if matches!(cli.command, Cmd::Update(_)) {
        None
    } else {
        block_on(commands::update::foreground(
            &loaded.settings,
            !matches!(cli.command, Cmd::Doctor(_)),
        ))?
    };

    match cli.command {
        Cmd::Update(args) => block_on(commands::update::run(args)),
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
        Cmd::Normalize(args) => commands::normalize::run(args, &loaded.settings.normalize),
        Cmd::Config(_)
        | Cmd::Serve(_)
        | Cmd::Completions(_)
        | Cmd::UpdateInternal(_)
        | Cmd::Install(_)
        | Cmd::Uninstall(_) => {
            unreachable!()
        }

        // Loogle search.
        Cmd::Loogle(args) => block_on(commands::loogle::run(args)),

        // Lean daemon health.
        Cmd::Doctor(args) => block_on(commands::doctor::run(args)),
        Cmd::Daemons(args) => block_on(commands::daemons::run(args)),
    }
}
