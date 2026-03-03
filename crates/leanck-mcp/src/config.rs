use leanck_core::workspace::WorkspaceRoot;
use std::path::PathBuf;

use clap::Parser;

#[derive(Debug, Clone, Parser)]
#[command(
    name = "leanck-mcp",
    about = "MCP server that forwards Lean tool calls to the leanckd daemon"
)]
pub struct CliArgs {
    #[command(flatten)]
    pub config: leanck_config::ConfigArgs,
    /// Workspace root the daemon serves. If omitted, the bridge
    /// walks up from the current directory looking for `lakefile.lean`,
    /// `lakefile.toml`, or `lean-toolchain`. If nothing matches, it falls
    /// back to the current directory.
    #[arg(long)]
    pub workspace: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Config {
    pub args: leanck_config::ConfigArgs,
    pub settings: leanck_config::Settings,
    pub workspace: WorkspaceRoot,
}

impl Config {
    /// Explicit defaults for embedded callers, without filesystem/environment discovery.
    pub fn defaults(workspace: WorkspaceRoot) -> Self {
        Self {
            workspace,
            args: leanck_config::ConfigArgs {
                no_config: true,
                ..Default::default()
            },
            settings: leanck_config::Settings::default(),
        }
    }
    pub fn from_cli(args: CliArgs) -> anyhow::Result<Self> {
        let root = match args.workspace {
            Some(p) => WorkspaceRoot::from_path(p),
            None => WorkspaceRoot::discover_current(),
        }?;
        let settings = args.config.load(root.as_path())?.settings;
        Ok(Self {
            workspace: root,
            args: args.config,
            settings,
        })
    }
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
        assert_eq!(
            Config::from_cli(args).unwrap().workspace.as_path(),
            workspace
        );
        assert!(CliArgs::try_parse_from(["leanck-mcp", "--embedder", "voyage"]).is_err());
    }
}
