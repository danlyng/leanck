use std::path::PathBuf;

use leanckd::proto::GoalParams;

use crate::commands::lean::{connect, print_json, render_goal};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// File (workspace-relative path or `file://` URI).
    pub(crate) file: String,

    /// 0-indexed line number.
    pub(crate) line: u32,

    /// 0-indexed character column.
    pub(crate) character: u32,

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
    let params = GoalParams {
        file: args.file.clone(),
        line: args.line,
        character: args.character,
    };
    let body = client.goal(&params).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    if args.json {
        print_json(&body)?;
    } else {
        println!("{}", render_goal(&body));
    }
    Ok(())
}
