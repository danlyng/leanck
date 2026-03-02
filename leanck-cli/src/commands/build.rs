use std::path::PathBuf;

use crate::workspace::detect_workspace;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// Workspace root. Defaults to the auto-detected Lake project (walks up
    /// looking for `lakefile.lean`, `lakefile.toml`, or `lean-toolchain`),
    /// falling back to the current directory.
    #[arg(long)]
    pub(crate) workspace: Option<PathBuf>,

    /// Also report build warnings, but only those from libraries/executables
    /// declared in the project's own `lakefile.toml` (dependency warnings such
    /// as Mathlib's are dropped).
    #[arg(long, default_value_t = false)]
    pub(crate) warnings: bool,

    /// Treat build warnings as failures. Implies `--warnings`.
    #[arg(long, default_value_t = false)]
    pub(crate) strict: bool,

    /// Print the full machine-readable JSON build report instead of compact
    /// text.
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,

    /// Targets to scope the build to, forwarded to `lake build` verbatim (e.g.
    /// `MyPkg.Foo.Bar` for a single module, `MyPkg` for a whole library). With
    /// none given, the entire workspace is built.
    #[arg(value_name = "TARGET")]
    pub(crate) targets: Vec<String>,
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let workspace = match args.workspace {
        Some(ws) => ws,
        None => detect_workspace()?,
    };
    let show_warnings = args.warnings || args.strict;
    let report = leanckd::build::run(
        &workspace,
        leanckd::build::BuildOptions {
            warnings: args.warnings,
            strict: args.strict,
            targets: args.targets,
        },
    )
    .await?;

    if args.json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!("{}", leanckd::build::format_human(&report, show_warnings));
    }

    // Mirror lake by default; strict mode additionally rejects warnings.
    if !report.success {
        std::process::exit(1);
    }
    Ok(())
}
