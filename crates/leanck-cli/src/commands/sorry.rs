use crate::args::LiveArgs;

use leanck_core::api::SorryParams;

use crate::commands::output::print_json;
use leanck_client::render::render_sorry;

#[derive(clap::Args, Debug)]
pub(crate) struct Args {
    /// File or directory to scan. A directory is scanned recursively for
    /// `.lean` files. Uses Lean diagnostics and source tokens unless --fast is set.
    #[arg(value_name = "PATH")]
    pub(crate) path: String,

    /// Scan source text only, without starting or querying the daemon. Misses
    /// sorries that macros produce without spelling one.
    #[arg(long)]
    pub(crate) fast: bool,

    #[command(flatten)]
    pub(crate) live: LiveArgs,
}

pub(crate) async fn run(args: Args) -> anyhow::Result<()> {
    let client = args.live.client()?;
    let params = SorryParams {
        path: args.path.clone(),
    };
    let body = if args.fast {
        client.scan_sorries(&params).await?
    } else {
        client.sorries(&params, args.live.request_options()).await?
    };
    if args.live.output.json {
        print_json(&body)?;
    } else {
        println!("{}", render_sorry(&body));
    }
    Ok(())
}
