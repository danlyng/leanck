use clap::{Parser, Subcommand};

const SHORT_ABOUT: &str = color_print::cstr!(
    r#"<b><bold>leanck</> ↦ Lean4 utilities for agents: diagnostics, goals, search, and more</>"#
);

const LONG_ABOUT: &str = color_print::cstr!(
    r#"<b><bold>leanck:</> Lean4 utilities for agents</>
  leanck is a virtual Lean4 IDE for coding agents. It allows agents to view file diagnostics,
  inspect goal state, and search <underline>Loogle</>, among other things, with special attention to minimizing
  token use and enabling quick iteration through fast feedback."#
);

const AFTER_HELP: &str = color_print::cstr!(
    r#"<underline,bold>Examples:</>
  leanck diagnostics Main.lean
  leanck loogle --constant Nat.add
  leanck normalize Main.lean"#
);

#[derive(Parser, Debug)]
#[command(
    name = "leanck",
    version,
    about = SHORT_ABOUT,
    long_about = LONG_ABOUT,
    after_help = AFTER_HELP,
)]
pub(crate) struct Cli {
    #[command(flatten)]
    pub(crate) config: crate::args::ConfigArgs,
    #[command(subcommand)]
    pub(crate) command: Cmd,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Cmd {
    // ── Live Lean state (same `leanckd` daemon the MCP server uses) ──────────
    /// Report diagnostics for a file.
    #[command(visible_alias = "d")]
    Diagnostics(crate::commands::diagnostics::Args),

    /// Show the proof goal at a file position.
    Goal(crate::commands::goal::Args),

    /// List `sorry` locations under a file or directory.
    Sorry(crate::commands::sorry::Args),

    /// List declarations in a file.
    List(crate::commands::list::Args),

    /// Restart the Lean file elaborator.
    #[command(name = "restart-file", alias = "restart_file")]
    RestartFile(crate::commands::restart_file::Args),

    /// Invoke a workspace build with compact output.
    Build(crate::commands::build::Args),

    /// Normalize Lean comment formatting and top-of-file frontmatter.
    ///
    /// Prefer configuration via the config file. See `leanck config --help` for details.
    #[command(
        after_help = crate::help::examples("  leanck normalize Main.lean\n  leanck normalize . --dry-run\n  leanck normalize Main.lean --width 100")
    )]
    Normalize(crate::commands::normalize::Args),

    /// Search Lean and Mathlib declarations through Loogle.
    ///
    /// Query flags are repeatable. Multiple filters are joined with AND.
    Loogle(crate::commands::loogle::Args),

    /// Check the Lean workspace and daemon.
    #[command(visible_alias = "health")]
    Doctor(crate::commands::doctor::Args),

    /// Manage leanck daemons
    Daemons(crate::commands::daemons::Args),

    /// Inspect, validate, or initialize leanck configuration.
    #[command(
        after_help = crate::help::examples("  leanck config show\n  leanck config check\n  leanck config init")
    )]
    Config(crate::commands::config::Args),

    /// Check for or install a leanck release.
    Update(crate::commands::update::Args),

    #[command(name = "_update", hide = true)]
    UpdateInternal(crate::commands::update::InternalArgs),
    /// Install leanck from an unpacked release; install.sh runs this.
    ///
    /// `main` dispatches `_install` before this parser runs, so installer
    /// errors keep their own messages and JSON output.
    #[command(name = "_install", hide = true)]
    Install(leanck_update::installer::InstallArgs),
    #[command(name = "_completions", hide = true)]
    Completions(crate::commands::completions::Args),
    /// Run the MCP server or a workspace daemon.
    #[command(hide = true)]
    Serve(crate::commands::serve::Args),
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn daemons_stop_needs_exactly_one_target() {
        for argv in [
            vec!["leanck", "daemons", "stop"],
            vec!["leanck", "daemons", "stop", "--idle", "--all"],
            vec!["leanck", "daemons", "stop", "/wt", "--all"],
            vec!["leanck", "daemons", "stop", "--incompatible", "--all"],
        ] {
            assert!(Cli::try_parse_from(argv).is_err());
        }
    }

    #[test]
    fn installer_subcommand_is_hidden_and_keeps_its_version_flag() {
        let command = Cli::command();
        let install = command.find_subcommand("_install").unwrap();
        assert!(install.is_hide_set());
        assert!(
            install
                .get_arguments()
                .any(|arg| arg.get_long() == Some("version") && arg.get_value_names().is_some())
        );
    }

    #[test]
    fn cli_definition_is_valid() {
        // Catches clap mis-declarations (duplicate flags, bad arg combos)
        // at test time rather than first invocation.
        Cli::command().debug_assert();
    }
}
