use crate::cli::Cmd;
use clap::{ArgMatches, parser::ValueSource};
use leanck_config::Settings;
use std::path::PathBuf;

pub(crate) fn start(command: &Cmd) -> anyhow::Result<PathBuf> {
    let workspace = match command {
        Cmd::Diagnostics(a) => &a.live.workspace.workspace,
        Cmd::Goal(a) => &a.live.workspace.workspace,
        Cmd::Sorry(a) => &a.live.workspace.workspace,
        Cmd::List(a) => &a.live.workspace.workspace,
        Cmd::RestartFile(a) => &a.live.workspace.workspace,
        Cmd::Build(a) => &a.workspace.workspace,
        Cmd::Doctor(a) => &a.workspace.workspace,
        Cmd::Normalize(a) => {
            return Ok(a
                .path
                .clone()
                .filter(|p| p.as_os_str() != "-")
                .unwrap_or(std::env::current_dir()?));
        }
        _ => return Ok(std::env::current_dir()?),
    };
    Ok(workspace.clone().unwrap_or(std::env::current_dir()?))
}
fn live(args: &mut crate::args::LiveArgs, settings: &Settings) {
    args.settings = settings.clone();
    args.output.json |= settings.output.json;
}
pub(crate) fn apply(command: &mut Cmd, s: &Settings, matches: &ArgMatches) {
    match command {
        Cmd::Diagnostics(a) => {
            live(&mut a.live, s);
            a.limit = a.limit.or(Some(s.diagnostics.limit));
        }
        Cmd::Goal(a) => live(&mut a.live, s),
        Cmd::Sorry(a) => live(&mut a.live, s),
        Cmd::List(a) => live(&mut a.live, s),
        Cmd::RestartFile(a) => live(&mut a.live, s),
        Cmd::Build(a) => {
            a.settings = s.build.clone();
            a.warnings |= s.build.warnings;
            a.strict |= s.build.strict;
            a.output.json |= s.output.json;
            if matches.value_source("max_output_bytes") != Some(ValueSource::CommandLine) {
                a.max_output_bytes = s.build.max_output_bytes;
            }
            if a.targets.is_empty() {
                a.targets = s.build.targets.clone();
            }
        }
        Cmd::Doctor(a) => {
            a.settings = s.clone();
            a.output.json |= s.output.json;
        }
        Cmd::Daemons(a) => {
            a.settings = s.clone();
            a.json |= s.output.json;
        }
        Cmd::Loogle(a) => {
            a.settings = s.loogle.clone();
            a.json |= s.output.json;
            if matches.value_source("limit") != Some(ValueSource::CommandLine) {
                a.limit = s.loogle.limit;
            }
        }
        Cmd::Normalize(a) => {
            if matches.value_source("width") != Some(ValueSource::CommandLine) {
                a.width = s.normalize.width;
            }
        }
        Cmd::Config(_) | Cmd::Serve(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::Cli;
    use clap::{CommandFactory, FromArgMatches};
    #[test]
    fn explicit_default_valued_flags_beat_configured_values() {
        let mut settings = Settings::default();
        settings.normalize.width = 72;
        settings.loogle.limit = 42;
        settings.build.max_output_bytes = 1024;
        for argv in [
            vec!["leanck", "normalize", "--width", "100"],
            vec!["leanck", "loogle", "--limit", "10"],
            vec!["leanck", "build", "--max-output-bytes", "8388608"],
        ] {
            let matches = Cli::command().try_get_matches_from(argv).unwrap();
            let mut cli = Cli::from_arg_matches(&matches).unwrap();
            apply(&mut cli.command, &settings, matches.subcommand().unwrap().1);
            match cli.command {
                Cmd::Normalize(a) => assert_eq!(a.width, 100),
                Cmd::Loogle(a) => assert_eq!(a.limit, 10),
                Cmd::Build(a) => assert_eq!(a.max_output_bytes, 8388608),
                _ => unreachable!(),
            }
        }
    }
}
