use std::path::PathBuf;

use leanckd::proto::ListParams;

use crate::commands::lean::{connect, print_json, render_list};

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// File to list (workspace-relative path or `file://` URI).
    pub(crate) file: String,

    /// Restrict to these Lean keywords (`theorem`, `def`, `lemma`, …) or LSP
    /// kinds (`namespace`, `class`, …). Repeatable. Empty lists everything.
    #[arg(long = "kind")]
    pub(crate) kind: Vec<String>,

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
    let params = ListParams {
        file: args.file.clone(),
        kind: (!args.kind.is_empty()).then(|| args.kind.clone()),
    };
    let body = client.list(&params).await.map_err(|e| anyhow::anyhow!("{e}"))?;
    if args.json {
        print_json(&body)?;
    } else {
        println!("{}", render_list(&body));
    }
    Ok(())
}
