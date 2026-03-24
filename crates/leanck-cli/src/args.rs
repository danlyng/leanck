//! Shared frontend flags; connection/startup/retry policy is owned by leanck-client.
use leanck_client::{Client, RequestOptions};
use leanck_core::workspace::{WorkspaceError, WorkspaceRoot};
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub(crate) struct WorkspaceArgs {
    /// Workspace root. Defaults to the auto-detected Lake project.
    #[arg(long, hide_short_help = true)]
    pub(crate) workspace: Option<PathBuf>,
}

impl WorkspaceArgs {
    pub(crate) fn resolve(&self) -> Result<WorkspaceRoot, WorkspaceError> {
        match &self.workspace {
            Some(path) => WorkspaceRoot::from_path(path),
            None => WorkspaceRoot::discover_current(),
        }
    }
}

#[derive(clap::Args, Debug)]
pub(crate) struct OutputArgs {
    /// Print machine-readable JSON instead of compact text.
    #[arg(long, default_value_t = false, hide_short_help = true)]
    pub(crate) json: bool,
}

#[derive(clap::Args, Debug)]
pub(crate) struct LiveArgs {
    #[arg(skip)]
    pub(crate) settings: leanck_config::Settings,
    #[arg(skip)]
    pub(crate) cancellation: Option<leanck_client::CancellationToken>,
    /// Operation budget in seconds, including queueing.
    ///
    /// Cold startup uses a
    /// separate budget. An explicit value also overrides the applicable Lean
    /// wait (defaults: 300s operation, 120s elaboration).
    #[arg(long, value_name = "SECS", hide_short_help = true)]
    pub(crate) timeout: Option<u64>,
    #[command(flatten)]
    pub(crate) workspace: WorkspaceArgs,
    #[command(flatten)]
    pub(crate) output: OutputArgs,
}

impl LiveArgs {
    pub(crate) fn client(&self) -> anyhow::Result<Client> {
        Ok(Client::new(self.workspace.resolve()?)
            .with_settings(self.settings.clone())
            .with_daemon_binary(std::env::current_exe()?))
    }
    pub(crate) fn request_options(&self) -> RequestOptions {
        RequestOptions {
            timeout_ms: self.timeout.map(|seconds| seconds.saturating_mul(1000)),
            cancellation: self.cancellation.clone(),
            ..Default::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::cli::{Cli, Cmd};
    use clap::Parser;

    #[test]
    fn request_deadlines_keep_default_and_saturate() {
        for (timeout, expected) in [
            (None, None),
            (Some("42"), Some(42_000)),
            (Some("18446744073709551615"), Some(u64::MAX)),
        ] {
            let mut argv = vec!["leanck", "list", "Basic.lean"];
            if let Some(timeout) = timeout {
                argv.extend(["--timeout", timeout]);
            }
            let Cmd::List(args) = Cli::try_parse_from(argv).unwrap().command else {
                unreachable!()
            };
            assert_eq!(args.live.request_options().timeout_ms, expected);
        }
    }
}

/// Common flags for all executable entry points.
#[derive(Debug, Clone, Default, clap::Args)]
pub struct ConfigArgs {
    /// Overlay this TOML file after user/project configuration (or LEANCK_CONFIG).
    #[arg(
        long,
        global = true,
        value_name = "FILE",
        conflicts_with = "no_config",
        hide_short_help = true,
        help_heading = "Global options"
    )]
    pub config: Option<PathBuf>,
    /// Ignore configuration files, including LEANCK_CONFIG.
    #[arg(
        long,
        global = true,
        hide_short_help = true,
        help_heading = "Global options"
    )]
    pub no_config: bool,
    /// Override any setting using a dotted TOML assignment; repeatable.
    #[arg(
        long = "set",
        global = true,
        value_name = "KEY=VALUE",
        hide_short_help = true,
        help_heading = "Global options"
    )]
    pub overrides: Vec<String>,
}

impl ConfigArgs {
    pub(crate) fn options(&self) -> leanck_config::ConfigArgs {
        leanck_config::ConfigArgs {
            config: self.config.clone(),
            no_config: self.no_config,
            overrides: self.overrides.clone(),
        }
    }
    pub(crate) fn load(
        &self,
        start: &std::path::Path,
    ) -> anyhow::Result<leanck_config::LoadedConfig> {
        self.options().load(start)
    }
}
