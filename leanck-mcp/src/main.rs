use clap::Parser;
use rmcp::{ServiceExt, transport::stdio};

use leanck_mcp::config::{CliArgs, Config};
use leanck_mcp::mcp::LeanckServer;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    init_tracing();
    let args = CliArgs::parse();
    let config = Config::from_cli(args)?;
    tracing::info!(
        workspace = %config.workspace.as_path().display(),
        "starting leanck-mcp MCP server (stdio)"
    );

    let server = LeanckServer::new(&config);
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}

fn init_tracing() {
    use tracing_subscriber::EnvFilter;
    // stdout is the MCP wire — route ALL tracing to stderr.
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,leanck_mcp=debug"));
    tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(std::io::stderr)
        .with_target(true)
        .init();
}
