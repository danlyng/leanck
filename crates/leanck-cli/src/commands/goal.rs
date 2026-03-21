use crate::args::LiveArgs;

use leanck_core::api::GoalParams;

use crate::commands::output::print_json;
use leanck_client::render::render_goal;

#[derive(clap::Args, Debug)]
#[command(after_help = crate::help::examples("  leanck goal Main.lean 10 4"))]
pub(crate) struct Args {
    /// File (paths are relative to the workspace).
    pub(crate) file: String,

    /// 0-indexed line number.
    pub(crate) line: u32,

    /// 0-indexed character column.
    pub(crate) character: u32,

    #[command(flatten)]
    pub(crate) live: LiveArgs,
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let client = args.live.client()?;
    let params = GoalParams {
        file: args.file.clone(),
        line: args.line,
        character: args.character,
    };
    let body = client.goal(&params, args.live.request_options()).await?;
    if args.live.output.json {
        print_json(&body)?;
    } else {
        println!("{}", render_goal(&body));
    }
    Ok(())
}
