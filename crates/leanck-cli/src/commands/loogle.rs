use clap::CommandFactory;
use leanck_loogle::LoogleQuery;

use crate::cli::Cli;

const USAGE: &str = "leanck loogle [OPTIONS] \
    <--constant|--substr|--subexpr|--conclusion> <QUERY>...";

#[derive(clap::Args, Debug)]
#[command(override_usage = USAGE, after_help = crate::help::examples("  leanck loogle --constant Nat.add\n  leanck loogle --substr add --conclusion \"_ <= _\""))]
pub(crate) struct Args {
    /// Exact constants / declarations to search for, e.g. `Real.sin`.
    #[arg(short = 'c', long, help_heading = "Query (Repeatable)")]
    pub(crate) constant: Vec<String>,

    /// Name substrings to search for.
    #[arg(short = 's', long, help_heading = "Query (Repeatable)")]
    pub(crate) substr: Vec<String>,

    /// Subexpressions to search for, e.g. `_ * (_ ^ _)`.
    #[arg(short = 'x', long, help_heading = "Query (Repeatable)")]
    pub(crate) subexpr: Vec<String>,

    /// Conclusion type patterns, e.g. `tsum _ = _`.
    #[arg(short = 'n', long, help_heading = "Query (Repeatable)")]
    pub(crate) conclusion: Vec<String>,

    #[arg(skip)]
    pub(crate) settings: leanck_config::Loogle,
    /// Replace the configured endpoint list, in fallback order (repeatable).
    #[arg(long, hide_short_help = true)]
    pub(crate) endpoint: Vec<String>,
    /// HTTP timeout for each endpoint, in milliseconds.
    #[arg(long, hide_short_help = true)]
    pub(crate) timeout_ms: Option<u64>,
    /// Positional queries are intentionally unsupported; capture them so we can
    /// return a help-rich error instead of clap's generic "unexpected argument".
    #[arg(value_name = "QUERY", hide = true)]
    pub(crate) positional: Vec<String>,

    /// Max results to show.
    #[arg(
        short = 'k',
        long = "limit",
        default_value_t = leanck_loogle::DEFAULT_LIMIT
    )]
    pub(crate) limit: usize,

    /// Return the full JSON response instead of compact text.
    #[arg(long, default_value_t = false, hide_short_help = true)]
    pub(crate) json: bool,
}

pub(crate) async fn run(mut args: Args) -> anyhow::Result<()> {
    if !args.positional.is_empty() {
        return Err(positional_args_error(&args.positional));
    }
    let input = LoogleQuery {
        constant: args.constant,
        substr: args.substr,
        subexpr: args.subexpr,
        conclusion: args.conclusion,
    };
    if input.term_count() == 0 {
        return Err(help_error(
            "at least one --constant, --substr, --subexpr, or --conclusion is required",
        ));
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
    help_error(&format!(
        "positional Loogle queries are not supported: {}",
        positional.join(" ")
    ))
}

fn help_error(message: &str) -> anyhow::Error {
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
    anyhow::anyhow!("{message}\n{help}")
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

    #[test]
    fn usage_requires_a_query_flag() {
        let mut command = Cli::command();
        let usage = command
            .find_subcommand_mut("loogle")
            .expect("loogle subcommand exists")
            .render_usage()
            .to_string();
        assert!(
            usage.contains("<--constant|--substr|--subexpr|--conclusion>"),
            "{usage}"
        );
    }

    #[test]
    fn query_flags_have_their_own_help_section() {
        let mut command = Cli::command();
        let help = command
            .find_subcommand_mut("loogle")
            .expect("loogle subcommand exists")
            .render_help()
            .to_string();
        let query = help.find("\nQuery (Repeatable):\n").expect("query section");
        for flag in ["--constant", "--substr", "--subexpr", "--conclusion"] {
            assert!(
                help[query..].contains(flag),
                "{flag} not under Query (Repeatable):\n{help}"
            );
        }
        assert!(!help[query..].contains("--limit"), "{help}");
    }

    #[tokio::test]
    async fn blank_query_flags_are_rejected_before_search() {
        let cli = Cli::try_parse_from(["leanck", "loogle", "--constant", " "]).unwrap();
        let Cmd::Loogle(args) = cli.command else {
            panic!("expected loogle command");
        };

        let error = run(args).await.unwrap_err().to_string();
        assert!(error.starts_with("at least one --constant"), "{error}");
        assert!(error.contains("Usage: leanck loogle"), "{error}");
    }
}
