pub mod config;
pub mod mcp;
pub mod workspace;

/// Serve MCP until stdin closes. The executable owns runtime and logging setup.
pub async fn serve(server: mcp::LeanckServer) -> anyhow::Result<()> {
    use rmcp::{ServiceExt, transport::stdio};
    let running = server.serve(stdio()).await?;
    running.waiting().await?;
    Ok(())
}
