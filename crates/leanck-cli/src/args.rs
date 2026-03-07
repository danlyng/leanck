//! Shared frontend flags; connection/startup/retry policy is owned by leanck-client.
use leanck_client::{Client, RequestOptions};
use leanck_core::workspace::{WorkspaceError, WorkspaceRoot};
use std::path::PathBuf;

#[derive(clap::Args, Debug)]
pub(crate) struct WorkspaceArgs {
    /// Workspace root. Defaults to the auto-detected Lake project.
    #[arg(long)]
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
    #[arg(long, default_value_t = false)]
    pub(crate) json: bool,
}

#[derive(clap::Args, Debug)]
pub(crate) struct LiveArgs {
    #[arg(skip)]
    pub(crate) settings: leanck_config::Settings,
    #[arg(skip)]
    pub(crate) cancellation: Option<leanck_client::CancellationToken>,
    /// Operation budget in seconds, including queueing. Cold startup uses a
    /// separate budget. An explicit value also overrides the applicable Lean
    /// wait (defaults: 300s operation, 120s elaboration).
    #[arg(long, value_name = "SECS")]
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
    use super::*;
    use crate::cli::{Cli, Cmd};
    use clap::Parser;

    #[test]
    fn live_commands_share_flags_and_keep_documented_aliases() {
        for command in ["diagnostics", "d", "goal", "sorry", "list", "restart-file"] {
            let mut argv = vec!["leanck", command, "Basic.lean"];
            if command == "goal" {
                argv.extend(["1", "2"]);
            }
            argv.extend(["--workspace", "/some/root", "--timeout", "42", "--json"]);
            let options = match Cli::try_parse_from(argv).unwrap().command {
                Cmd::Diagnostics(args) => args.live,
                Cmd::Goal(args) => args.live,
                Cmd::Sorry(args) => args.live,
                Cmd::List(args) => args.live,
                Cmd::RestartFile(args) => args.live,
                _ => unreachable!(),
            };
            assert_eq!(
                options.workspace.workspace,
                Some(PathBuf::from("/some/root"))
            );
            assert!(options.output.json);
            assert_eq!(options.request_options().timeout_ms, Some(42_000));
        }
    }

    #[test]
    fn restart_file_hidden_alias_is_rejected() {
        let error = Cli::try_parse_from(["leanck", "restart_file", "Basic.lean"]).unwrap_err();
        assert_eq!(error.kind(), clap::error::ErrorKind::InvalidSubcommand);
    }

    #[test]
    fn request_deadlines_keep_default_and_saturate() {
        for (timeout, expected) in [(None, None), (Some("18446744073709551615"), Some(u64::MAX))] {
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

    #[test]
    fn build_and_doctor_share_workspace_and_output_flags() {
        for command in ["build", "doctor", "health"] {
            let cli =
                Cli::try_parse_from(["leanck", command, "--workspace", "/some/root", "--json"])
                    .unwrap();
            let (workspace, output) = match cli.command {
                Cmd::Build(args) => (args.workspace, args.output),
                Cmd::Doctor(args) => (args.workspace, args.output),
                _ => unreachable!(),
            };
            assert_eq!(workspace.workspace, Some(PathBuf::from("/some/root")));
            assert!(output.json);
        }
    }
}
