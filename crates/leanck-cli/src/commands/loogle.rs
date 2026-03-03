use clap::CommandFactory;
use leanck_loogle::LoogleQuery;

use crate::cli::Cli;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    #[arg(skip)]
    pub(crate) settings: leanck_config::Loogle,
    /// Replace the configured endpoint list, in fallback order (repeatable).
    #[arg(long)]
    pub(crate) endpoint: Vec<String>,
    /// HTTP timeout for each endpoint, in milliseconds.
    #[arg(long)]
    pub(crate) timeout_ms: Option<u64>,
    /// Positional queries are intentionally unsupported; capture them so we can
    /// return a help-rich error instead of clap's generic "unexpected argument".
    #[arg(value_name = "QUERY", hide = true)]
    pub(crate) positional: Vec<String>,

    /// Exact constants / declarations to search for, e.g. `Real.sin`.
    #[arg(long = "constant")]
    pub(crate) constant: Vec<String>,

    /// Name substrings to search for.
    #[arg(long = "substr")]
    pub(crate) substr: Vec<String>,

    /// Subexpressions to search for, e.g. `_ * (_ ^ _)`.
    #[arg(long = "subexpr")]
    pub(crate) subexpr: Vec<String>,

    /// Conclusion type patterns. A `|-` turnstile is added when omitted.
    #[arg(long = "conclusion")]
    pub(crate) conclusion: Vec<String>,

    /// Max results to show.
    #[arg(
        short = 'k',
        long = "limit",
        default_value_t = leanck_loogle::DEFAULT_LIMIT,
    )]
    pub(crate) limit: usize,

    /// Return the full JSON response instead of compact text.
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
}

pub(crate) async fn run(mut args: Args) -> anyhow::Result<()> {
    if !args.positional.is_empty() {
        return Err(positional_args_error(&args.positional));
    }

    if !args.endpoint.is_empty() {
        args.settings.endpoints = args.endpoint;
    }
    if let Some(timeout) = args.timeout_ms {
        args.settings.timeout_ms = timeout;
    }
    let settings = leanck_config::Settings {
        loogle: args.settings.clone(),
        ..Default::default()
    };
    settings.validate()?;
    let input = LoogleQuery {
        constant: args.constant,
        substr: args.substr,
        subexpr: args.subexpr,
        conclusion: args.conclusion,
    };
    let outcome = leanck_loogle::Client::from_config(&args.settings)
        .search(&input, leanck_loogle::SearchOptions { limit: args.limit })
        .await?;
    if args.json {
        println!("{}", serde_json::to_string_pretty(&outcome)?);
    } else {
        println!("{}", leanck_loogle::format_human(&outcome));
    }
    Ok(())
}

fn positional_args_error(positional: &[String]) -> anyhow::Error {
    let command = Cli::command();
    let mut loogle_command = command
        .find_subcommand("loogle")
        .expect("loogle subcommand exists")
        .clone()
        .bin_name("leanck loogle");
    let help = loogle_command
        .render_help()
        .to_string()
        .split("\n")
        .enumerate()
        .filter_map(|(i, s)| if i != 0 { Some(s.to_string()) } else { None })
        .collect::<Vec<_>>()
        .join("\n");
    anyhow::anyhow!(
        "positional Loogle queries are not supported: {}\nUsage: leanck loogle [OPTIONS]\n{help}",
        positional.join(" ")
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli::{Cli, Cmd};
    use clap::Parser;

    #[test]
    fn positional_args_are_parsed_for_help_rich_error() {
        let cli = Cli::try_parse_from(["leanck", "loogle", "Nat.add"]).unwrap();
        let Cmd::Loogle(args) = cli.command else {
            panic!("expected loogle command");
        };

        let error = positional_args_error(&args.positional).to_string();
        assert!(error.contains("positional Loogle queries are not supported: Nat.add"));
        assert!(error.contains("Usage: leanck loogle"));
        assert!(error.contains("--constant"));
        assert!(error.contains("--substr"));
        assert!(error.contains("--subexpr"));
        assert!(error.contains("--conclusion"));
    }
}
