use leanck_core::workspace::WorkspaceRoot;
use std::path::PathBuf;

#[derive(Debug, Clone, clap::Args)]
pub struct Args {
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
    pub fn from_args(args: Args, config: leanck_config::ConfigArgs) -> anyhow::Result<Self> {
        let root = match args.workspace {
            Some(p) => WorkspaceRoot::from_path(p),
            None => WorkspaceRoot::discover_current(),
        }?;
        let settings = config.load(root.as_path())?.settings;
        Ok(Self {
            workspace: root,
            args: config,
            settings,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workspace_needs_no_registry_configuration() {
        let workspace = std::env::temp_dir().canonicalize().unwrap();
        let args = Args {
            workspace: Some(workspace.clone()),
        };
        assert_eq!(
            Config::from_args(args, leanck_config::ConfigArgs::default())
                .unwrap()
                .workspace
                .as_path(),
            workspace
        );
    }
}
