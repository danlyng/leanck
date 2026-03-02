use std::path::PathBuf;

use leanckd::proto::DiagnosticsParams;

use crate::commands::lean::{connect, print_json};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// File to query (workspace-relative path or `file://` URI).
    pub(crate) file: String,

    /// Restrict diagnostics to the range of this declaration.
    #[arg(long)]
    pub(crate) ident: Option<String>,

    /// Severity filter: `error`, `warning`, `info`, `hint`, or `all`.
    /// Repeatable (or comma-separated). Defaults to `error` only.
    #[arg(long = "severity")]
    pub(crate) severity: Vec<String>,

    /// Pagination offset into the filtered diagnostics.
    #[arg(long)]
    pub(crate) offset: Option<usize>,

    /// Maximum number of diagnostics to return (default 5).
    #[arg(long)]
    pub(crate) limit: Option<usize>,

    /// Per-request timeout in seconds. Also stretches the daemon's
    /// elaboration wait (defaults: 300s request, 120s elaboration wait).
    #[arg(long, value_name = "SECS")]
    pub(crate) timeout: Option<u64>,

    /// Workspace root. Defaults to the auto-detected Lake project.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Print the raw daemon JSON instead of compact text.
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let (workspace, client) = connect(args.workspace.clone(), args.timeout).await?;
    let severity = if args.severity.is_empty() {
        None
    } else {
        Some(args.severity.join(","))
    };
    let params = DiagnosticsParams {
        file: args.file.clone(),
        ident: args.ident.clone(),
        severity,
        limit: args.limit,
        offset: args.offset,
    };
    let body = client
        .diagnostics(&params)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if args.json {
        print_json(&body)?;
    } else {
        println!(
            "{}",
            leanckd::format::format_diagnostics_human(&body, &args.file, &workspace)
        );
    }
    Ok(())
}
