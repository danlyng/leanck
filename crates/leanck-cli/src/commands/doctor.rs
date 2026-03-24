use crate::args::{OutputArgs, WorkspaceArgs};

#[derive(clap::Args, Debug)]
#[command(after_help = crate::help::examples("  leanck doctor\n  leanck doctor --verbose"))]
pub(crate) struct Args {
    #[arg(skip)]
    pub(crate) settings: leanck_config::Settings,
    #[arg(skip)]
    pub(crate) config_warnings: Vec<leanck_config::Warning>,
    #[command(flatten)]
    pub(crate) workspace: WorkspaceArgs,

    /// Print all checks instead of just warnings and failures.
    #[arg(short, long)]
    pub(crate) verbose: bool,

    #[command(flatten)]
    pub(crate) output: OutputArgs,
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let workspace = args.workspace.workspace.unwrap_or_else(|| {
        leanck_core::workspace::WorkspaceRoot::discover_current()
            .map(|r| r.into_path_buf())
            .unwrap_or_else(|_| std::env::current_dir().unwrap_or_default())
    });
    let (report, updates) = tokio::join!(
        leanck_client::doctor::probe(&workspace, &args.settings),
        leanck_update::current_status(env!("CARGO_PKG_VERSION"), &args.settings.updates)
    );
    let report = report.with_config_warnings(args.config_warnings);
    if args.output.json {
        let mut value = serde_json::to_value(&report)?;
        value["updates"] = serde_json::to_value(&updates)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        println!("{}", report.text(args.verbose));
        if let Some(notice) = &updates.notice {
            println!("update: {notice}");
        }
        if let Some(error) = &updates.error {
            println!("info: update check unavailable: {error}");
        }
    }
    if !report.ok {
        anyhow::bail!("one or more doctor checks failed");
    }
    Ok(())
}
