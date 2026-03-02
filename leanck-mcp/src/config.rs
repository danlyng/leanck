use std::path::{Path, PathBuf};

use clap::Parser;

use crate::error::Result;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "leanck-mcp",
    about = "MCP server that forwards Lean tool calls to the leanckd daemon"
)]
pub struct CliArgs {
    /// Workspace root the daemon serves. If omitted, the bridge
    /// walks up from the current directory looking for `lakefile.lean`,
    /// `lakefile.toml`, or `lean-toolchain`. If nothing matches, it falls
    /// back to the current directory.
    #[arg(long)]
    pub workspace: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub workspace: PathBuf,
}

impl Config {
    pub fn from_cli(args: CliArgs) -> Result<Self> {
        let workspace = match args.workspace {
            Some(p) => p,
            None => detect_workspace()?,
        };
        Ok(Self {
            workspace: workspace.canonicalize().unwrap_or(workspace),
        })
    }
}

/// Walk up from the current directory until a Lake project marker is
/// found, falling back to the current directory if none is.
fn detect_workspace() -> Result<PathBuf> {
    let cwd = std::env::current_dir()?;
    Ok(find_project_root(&cwd).unwrap_or(cwd))
}

fn find_project_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(dir) = cur {
        if dir.join("lakefile.lean").exists()
            || dir.join("lakefile.toml").exists()
            || dir.join("lean-toolchain").exists()
        {
            return Some(dir.to_path_buf());
        }
        cur = dir.parent();
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn cli_definition_is_valid() {
        CliArgs::command().debug_assert();
    }

    #[test]
    fn workspace_needs_no_registry_configuration() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let args =
            CliArgs::try_parse_from(["leanck-mcp", "--workspace", workspace.to_str().unwrap()])
                .unwrap();
        assert_eq!(Config::from_cli(args).unwrap().workspace, workspace);
        assert!(CliArgs::try_parse_from(["leanck-mcp", "--embedder", "voyage"]).is_err());
    }
}
