use clap::CommandFactory;
use clap_complete::{Generator, Shell};

use crate::cli::Cli;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Shell to generate a completion script for.
    #[arg(value_enum)]
    shell: Shell,
}

pub(crate) fn run(args: Args) -> anyhow::Result<()> {
    let cli = Cli::command();
    // Static generators do not consistently honor hidden commands. Filter
    // before building, so the generated `help` subtree also omits them.
    let mut command = clap::Command::new("leanck")
        .bin_name("leanck")
        .version(env!("CARGO_PKG_VERSION"))
        .args(cli.get_arguments().cloned())
        .subcommands(cli.get_subcommands().filter(|c| !c.is_hide_set()).cloned());
    command.build();
    args.shell
        .try_generate(&command, &mut std::io::stdout().lock())?;
    Ok(())
}
