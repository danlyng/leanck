use std::path::PathBuf;

use leanckd::proto::RestartFileParams;

use crate::commands::lean::{connect, print_json, render_restart};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// File whose elaborator worker should be restarted (workspace-relative
    /// path or `file://` URI). Rebuilds stale/missing dependencies through the
    /// warm server (`dependencyBuildMode = once`).
    pub(crate) file: String,

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
    let (_workspace, client) = connect(args.workspace.clone(), args.timeout).await?;
    let params = RestartFileParams { file: args.file.clone() };
    let body = client
        .restart_file(&params)
        .await
        .map_err(|e| anyhow::anyhow!("{e}"))?;
    if args.json {
        print_json(&body)?;
    } else {
        println!("{}", render_restart(&body));
    }
    Ok(())
}
