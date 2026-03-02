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
    pub(crate) config: leanck_config::ConfigArgs,
    #[command(subcommand)]
    pub(crate) command: Cmd,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Cmd {
    /// Inspect, validate, or initialize leanck configuration.
    Config(leanck_config::Command),
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
    #[command(name = "restart-file")]
    RestartFile(crate::commands::restart_file::Args),

    /// Wraps `lake build` for compact output.
    Build(crate::commands::build::Args),

    /// Normalize Lean comment formatting and top-of-file frontmatter.
    Normalize(leanck_normalize::command::Args),

    /// Search Lean and Mathlib declarations through Loogle.
    Loogle(crate::commands::loogle::Args),

    /// Check the Lean workspace and daemon.
    #[command(visible_alias = "health")]
    Doctor(crate::commands::doctor::Args),
}

#[cfg(test)]
mod tests {
    use super::*;
    use clap::CommandFactory;

    #[test]
    fn standalone_command_surface() {
        for removed in [
            "search",
            "premise_search",
            "premise-search",
            "registry",
            "agent",
        ] {
            assert!(Cli::try_parse_from(["leanck", removed]).is_err());
        }
        assert!(matches!(
            Cli::try_parse_from(["leanck", "normalize"])
                .unwrap()
                .command,
            Cmd::Normalize(_)
        ));
        assert!(matches!(
            Cli::try_parse_from(["leanck", "health"]).unwrap().command,
            Cmd::Doctor(_)
        ));
    }

    #[test]
    fn cli_definition_is_valid() {
        // Catches clap mis-declarations (duplicate flags, bad arg combos)
        // at test time rather than first invocation.
        Cli::command().debug_assert();
    }
}
