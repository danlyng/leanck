use crate::args::LiveArgs;

use leanck_core::api::RestartFileParams;

use crate::commands::output::print_json;
use leanck_client::render::render_restart;

#[derive(clap::Args, Debug)]
#[command(after_long_help = "Example:\n  leanck restart-file Main.lean")]
pub(crate) struct Args {
    /// File whose worker should be restarted; this reloads dependencies.
    ///
    /// Paths are relative to the workspace. Remember to pass the file whose
    /// dependencies are stale, not the dependencies themselves.
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
