//! Standalone `leanck-normalize` binary — a thin wrapper around the library
//! entry point, which is also exposed as the `leanck normalize` subcommand.

use clap::Parser;
use leanck_normalize::Args;

#[derive(Debug, Parser)]
#[command(
    name = "leanck-normalize",
    about = "Normalize Lean comment formatting and top-of-file frontmatter",
    override_usage = "leanck-normalize [OPTIONS] [WIDTH] [FILE_OR_DIR]",
    version
)]
struct Cli {
    #[command(flatten)]
    args: Args,
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = leanck_normalize::run(cli.args) {
        eprintln!("{e}");
        std::process::exit(1);
    }
}
