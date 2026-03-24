use crate::args::LiveArgs;

use leanck_core::api::{DiagnosticSeverity, DiagnosticsParams, SeverityFilter};

use crate::commands::output::print_json;

#[derive(clap::ValueEnum, Clone, Copy, Debug)]
pub(crate) enum SeverityArg {
    Error,
    Warning,
    Information,
    Hint,
}

impl From<SeverityArg> for DiagnosticSeverity {
    fn from(severity: SeverityArg) -> Self {
        match severity {
            SeverityArg::Error => Self::Error,
            SeverityArg::Warning => Self::Warning,
            SeverityArg::Information => Self::Information,
            SeverityArg::Hint => Self::Hint,
        }
    }
}

#[derive(clap::Args, Debug)]
#[command(
    after_help = crate::help::examples("  leanck diagnostics Main.lean\n  leanck diagnostics Main.lean --ident my_theorem\n  leanck diagnostics Main.lean --severity error,warning")
)]
pub(crate) struct Args {
    /// File to query (paths are relative to the workspace).
    pub(crate) file: String,

    /// Restrict diagnostics to the range of this declaration.
    #[arg(long)]
    pub(crate) ident: Option<String>,

    /// Include these severities (repeatable or comma-separated; default: error).
    #[arg(long, value_enum, value_delimiter = ',')]
    pub(crate) severity: Vec<SeverityArg>,

    /// Include diagnostics of every severity.
    #[arg(long, conflicts_with = "severity", hide_short_help = true)]
    pub(crate) all_severities: bool,

    /// Pagination offset into the filtered diagnostics.
    #[arg(long, hide_short_help = true)]
    pub(crate) offset: Option<u64>,

    /// Maximum number of diagnostics to return per page (default 5).
    #[arg(short = 'k', long)]
    pub(crate) limit: Option<u64>,

    #[command(flatten)]
    pub(crate) live: LiveArgs,
}

impl Args {
    fn severity_filter(&self) -> SeverityFilter {
        if self.all_severities {
            SeverityFilter::All
        } else if self.severity.is_empty() {
            use leanck_config::Severity as S;
            SeverityFilter::Only {
                severities: self
                    .live
                    .settings
                    .diagnostics
                    .severity
                    .iter()
                    .map(|s| match s {
                        S::Error => DiagnosticSeverity::Error,
                        S::Warning => DiagnosticSeverity::Warning,
                        S::Information => DiagnosticSeverity::Information,
                        S::Hint => DiagnosticSeverity::Hint,
                    })
                    .collect(),
            }
        } else {
            SeverityFilter::Only {
                severities: self.severity.iter().copied().map(Into::into).collect(),
            }
        }
    }
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let client = args.live.client()?;
    let severity = args.severity_filter();
    let params = DiagnosticsParams {
        file: args.file.clone(),
        ident: args.ident.clone(),
        severity,
        limit: args.limit,
        offset: args.offset,
    };
    let body = client
        .diagnostics(&params, args.live.request_options())
        .await?;
    if args.live.output.json {
        print_json(&body)?;
    } else {
        println!(
            "{}",
            leanck_client::render::format_diagnostics_human(&body, &args.file)
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::Parser;

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn severity_filter_defaults_to_errors_or_selects_all() {
        for (flags, expected) in [
            (
                &[][..],
                SeverityFilter::Only {
                    severities: vec![DiagnosticSeverity::Error],
                },
            ),
            (&["--all-severities"][..], SeverityFilter::All),
        ] {
            let cli =
                TestCli::try_parse_from(["diagnostics", "Main.lean"].iter().chain(flags)).unwrap();
            assert_eq!(cli.args.severity_filter(), expected);
        }
    }

    #[test]
    fn severity_accepts_repeated_and_comma_delimited_values() {
        let cli = TestCli::try_parse_from([
            "diagnostics",
            "Main.lean",
            "--severity",
            "error,warning",
            "--severity",
            "information,hint",
        ])
        .unwrap();
        assert!(matches!(
            cli.args.severity_filter(),
            SeverityFilter::Only { severities }
                if matches!(severities.as_slice(), [
                    DiagnosticSeverity::Error,
                    DiagnosticSeverity::Warning,
                    DiagnosticSeverity::Information,
                    DiagnosticSeverity::Hint,
                ])
        ));
    }
}
