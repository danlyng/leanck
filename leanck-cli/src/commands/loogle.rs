use clap::CommandFactory;
use leanck_loogle::LoogleQuery;

use crate::cli::{Cli, parse_positive_usize_cli};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
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
        value_parser = parse_positive_usize_cli
    )]
    pub(crate) limit: usize,

    /// Return the full JSON response instead of compact text.
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    if !args.positional.is_empty() {
        return Err(positional_args_error(&args.positional));
    }

    let input = LoogleQuery {
        constant: args.constant,
        substr: args.substr,
        subexpr: args.subexpr,
        conclusion: args.conclusion,
    };
    let has_multiple_terms = input.has_multiple_terms();
    let query = input.build().ok_or_else(|| {
        anyhow::anyhow!(
            "no Loogle query provided. Use `--constant`, `--substr`, `--subexpr`, or `--conclusion`."
        )
    })?;
    let (mut result, endpoint) = leanck_loogle::search_with_endpoint(&query, args.limit)
        .await
        .map_err(|err| anyhow::anyhow!("{err}"))?;
    result.add_empty_multi_term_note(has_multiple_terms);
    if args.json {
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("{}", leanck_loogle::format_human(&result));
    }
    // The local endpoint also indexes Econlib; any other endpoint is
    // Mathlib-only. Warn (on stderr, so stdout/JSON stays clean) when the
    // request fell through to a non-local endpoint.
    if !leanck_loogle::endpoint_searches_econlib(&endpoint) {
        eprintln!(
            "warning: served by {endpoint}; Econlib is not being searched \
             (only the local endpoint at localhost:8118 indexes Econlib)."
        );
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
