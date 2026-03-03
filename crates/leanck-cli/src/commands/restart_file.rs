use crate::args::LiveArgs;

use leanck_core::api::RestartFileParams;

use crate::commands::output::print_json;
use leanck_client::render::render_restart;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// File whose elaborator worker should be restarted (workspace-relative
    /// path or `file://` URI). Rebuilds stale/missing dependencies through the
    /// warm server (`dependencyBuildMode = once`).
    pub(crate) file: String,

    #[command(flatten)]
    pub(crate) live: LiveArgs,
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let client = args.live.client()?;
    let params = RestartFileParams {
        file: args.file.clone(),
    };
    let body = client
        .restart_file(&params, args.live.request_options())
        .await?;
    if args.live.output.json {
        print_json(&body)?;
    } else {
        println!("{}", render_restart(&body));
    }
    Ok(())
}
