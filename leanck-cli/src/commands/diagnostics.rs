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
pub(crate) struct Args {
    /// File to query (workspace-relative path or `file://` URI).
    pub(crate) file: String,

    /// Restrict diagnostics to the range of this declaration.
    #[arg(long)]
    pub(crate) ident: Option<String>,

    /// Severity filter (repeatable or comma-separated). Defaults to `error` only.
    #[arg(long, value_enum, value_delimiter = ',')]
    pub(crate) severity: Vec<SeverityArg>,

    /// Include diagnostics of every severity.
    #[arg(long, conflicts_with = "severity")]
    pub(crate) all_severities: bool,

    /// Pagination offset into the filtered diagnostics.
    #[arg(long)]
    pub(crate) offset: Option<u64>,

    /// Maximum number of diagnostics to return (default 5).
    #[arg(long)]
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
    use clap::{Parser, error::ErrorKind};

    #[derive(Debug, Parser)]
    struct TestCli {
        #[command(flatten)]
        args: Args,
    }

    #[test]
    fn default_severity_is_error_only() {
        let cli = TestCli::try_parse_from(["diagnostics", "Main.lean"]).unwrap();
        assert!(cli.args.severity.is_empty());
        assert!(!cli.args.all_severities);
        assert!(matches!(
            cli.args.severity_filter(),
            SeverityFilter::Only { severities }
                if matches!(severities.as_slice(), [DiagnosticSeverity::Error])
        ));
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

    #[test]
    fn all_severities_selects_all() {
        let cli =
            TestCli::try_parse_from(["diagnostics", "Main.lean", "--all-severities"]).unwrap();
        assert!(matches!(cli.args.severity_filter(), SeverityFilter::All));
    }

    #[test]
    fn all_severities_conflicts_with_severity_in_either_order() {
        for flags in [
            ["--all-severities", "--severity", "warning"],
            ["--severity", "warning", "--all-severities"],
        ] {
            let error =
                TestCli::try_parse_from(["diagnostics", "Main.lean"].into_iter().chain(flags))
                    .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::ArgumentConflict);
        }
    }

    #[test]
    fn severity_rejects_invalid_values_and_legacy_aliases() {
        for value in [
            "all",
            "info",
            "warn",
            "invalid",
            "error,info",
            "warning,all",
        ] {
            let error = TestCli::try_parse_from(["diagnostics", "Main.lean", "--severity", value])
                .unwrap_err();
            assert_eq!(error.kind(), ErrorKind::InvalidValue, "value: {value}");
        }
    }
}
