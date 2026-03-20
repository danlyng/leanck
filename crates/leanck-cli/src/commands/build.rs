use crate::args::{OutputArgs, WorkspaceArgs};
use std::io::IsTerminal;
#[derive(clap::Args, Debug)]
#[command(
    after_long_help = "Examples:\n  leanck build\n  leanck build MyProject --warnings\n  leanck build --strict"
)]
pub(crate) struct Args {
    #[arg(skip)]
    pub(crate) settings: leanck_config::Build,
    #[command(flatten)]
    pub(crate) workspace: WorkspaceArgs,
    /// Also report warnings emitted by Lake, including dependency and configuration warnings.
    #[arg(long, default_value_t = false)]
    pub(crate) warnings: bool,
    /// Treat build warnings as failures. Implies --warnings.
    #[arg(long, default_value_t = false)]
    pub(crate) strict: bool,
    /// Budget in seconds, including waiting for other leanck builds/cache fetches. Default: 1800.
    #[arg(long, value_name = "SECS", hide_short_help = true)]
    pub(crate) timeout: Option<u64>,
    /// Maximum captured bytes per output stream. Raising this permits larger build logs.
    #[arg(long, value_name = "BYTES", default_value_t = leanck_lake::DEFAULT_OUTPUT_LIMIT, hide_short_help = true)]
    pub(crate) max_output: usize,
    #[arg(skip)]
    pub(crate) cancellation: Option<leanck_lake::CancellationToken>,
    #[command(flatten)]
    pub(crate) output: OutputArgs,
    /// Forward targets verbatim to lake build; omit to build the whole workspace.
    #[arg(value_name = "TARGET")]
    pub(crate) targets: Vec<String>,
}
pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let show_warnings = args.warnings || args.strict;
    if !args.output.json && std::io::stderr().is_terminal() {
        eprintln!("Building... (Ctrl-C to cancel)");
    }
    let result: anyhow::Result<_> = async {
        let workspace = args.workspace.resolve()?;
        Ok(leanck_lake::build::run_with(
            workspace.as_path(),
            leanck_lake::build::BuildOptions {
                warnings: args.warnings,
                strict: args.strict,
                targets: args.targets,
            },
            leanck_lake::ExecutionOptions {
                timeout: args
                    .timeout
                    .map(std::time::Duration::from_secs)
                    .unwrap_or(std::time::Duration::from_millis(args.settings.timeout_ms)),
                cancellation: args.cancellation,
                max_output_bytes: args.max_output,
            },
        )
        .await?)
    }
    .await;
    match result {
        Ok(report) => {
            if args.output.json {
                super::output::print_json(&report)?;
            } else {
                println!(
                    "{}",
                    leanck_lake::build::format_human(&report, show_warnings)
                );
            }
            if !report.success {
                std::process::exit(1);
            }
        }
        Err(error) => {
            let lake = error.downcast_ref::<leanck_lake::LakeError>();
            let kind = lake
                .map(|e| e.kind())
                .unwrap_or(leanck_lake::LakeErrorKind::InvalidWorkspace);
            if args.output.json {
                super::output::print_json(
                    &serde_json::json!({"success":false,"error":{"kind":kind,"message":error.to_string()}}),
                )?;
            } else {
                eprintln!("error: {error}");
            }
            std::process::exit(if matches!(lake, Some(leanck_lake::LakeError::Cancelled)) {
                130
            } else {
                1
            });
        }
    }
    Ok(())
}
