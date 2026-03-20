//! Standalone `leanck-normalize` binary — a thin wrapper around the library
//! entry point, which is also exposed as the `leanck normalize` subcommand.

use clap::{CommandFactory, FromArgMatches, Parser};
use leanck_normalize::command::Args;

#[derive(Debug, Parser)]
#[command(
    name = "leanck-normalize",
    about = "Normalize Lean comment formatting and top-of-file frontmatter",
    override_usage = "leanck-normalize [OPTIONS] [PATH]",
    after_long_help = "Examples:\n  leanck-normalize Main.lean\n  leanck-normalize . --dry-run\n  leanck-normalize Main.lean --width 100",
    version
)]
struct Cli {
    #[command(flatten)]
    config: leanck_config::ConfigArgs,
    #[command(flatten)]
    args: Args,
}

fn main() {
    let matches = Cli::command().get_matches();
    let result = (|| -> anyhow::Result<()> {
        let mut cli = Cli::from_arg_matches(&matches)?;
        let start = cli
            .args
            .path
            .clone()
            .filter(|p| p.as_os_str() != "-")
            .unwrap_or(std::env::current_dir()?);
        let settings = cli.config.load(&start)?.settings;
        if matches.value_source("width") != Some(clap::parser::ValueSource::CommandLine) {
            cli.args.width = settings.normalize.width;
        }
        leanck_normalize::command::run_with_settings(cli.args, &settings.normalize)
    })();
    if let Err(e) = result {
        eprintln!("{e:#}");
        std::process::exit(1);
    }
}
