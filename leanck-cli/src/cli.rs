use clap::{Parser, Subcommand};

#[derive(Parser, Debug)]
#[command(
    name = "leanck",
    version,
    about = "Lean diagnostics, proof goals, builds, Loogle search, and source normalization",
    long_about = "Standalone tools for Lean 4 projects. Live Lean commands share a warm \
                  leanckd daemon, which starts automatically without an editor.",
    after_help = "Examples:\n  leanck diagnostics Main.lean\n  leanck loogle --constant Nat.add\n  leanck normalize Main.lean"
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) command: Cmd,
}

#[derive(Subcommand, Debug)]
pub(crate) enum Cmd {
    // ── Live Lean state (same `leanckd` daemon the MCP server uses) ──────────
    /// Wait for elaboration and report diagnostics for a file (alias: `d`).
    #[command(visible_alias = "d")]
    Diagnostics(crate::commands::diagnostics::Args),

    /// Show the Lean proof goal at a file position (0-indexed line/character).
    Goal(crate::commands::goal::Args),

    /// List `sorry` locations under a file or directory.
    Sorry(crate::commands::sorry::Args),

    /// List declarations in a file, optionally filtered by kind.
    List(crate::commands::list::Args),

    /// Restart the Lean elaborator worker for a single file.
    #[command(name = "restart_file", visible_alias = "restart-file")]
    RestartFile(crate::commands::restart_file::Args),

    /// Run a full `lake build` and report a compact pass/fail log. Uses the
    /// project's Lake toolchain so dependency caches are reused.
    Build(crate::commands::build::Args),

    /// Normalize Lean comment formatting and top-of-file frontmatter. Reflows
    /// block comments, sorts imports, fixes spelling, and more. Reads stdin
    /// when no path is given; rewrites a file or directory in place otherwise.
    Normalize(leanck_normalize::Args),

    /// Search Lean and Mathlib declarations through Loogle.
    Loogle(crate::commands::loogle::Args),

    // Lean daemon health.
    /// Check the Lean workspace and daemon (alias: `health`). Exits non-zero on failure.
    #[command(visible_alias = "health")]
    Doctor(crate::commands::doctor::Args),
}

pub(crate) fn parse_positive_usize_cli(value: &str) -> Result<usize, String> {
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("`{value}` is not a valid positive integer"))?;
    if parsed == 0 {
        Err("value must be at least 1".to_string())
    } else {
        Ok(parsed)
    }
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

    #[test]
    fn parse_positive_usize_accepts_positive_values() {
        assert_eq!(parse_positive_usize_cli("1"), Ok(1));
        assert_eq!(parse_positive_usize_cli("42"), Ok(42));
    }

    #[test]
    fn parse_positive_usize_rejects_zero() {
        assert!(parse_positive_usize_cli("0").is_err());
    }

    #[test]
    fn parse_positive_usize_rejects_non_numbers() {
        assert!(parse_positive_usize_cli("abc").is_err());
        assert!(parse_positive_usize_cli("-3").is_err());
        assert!(parse_positive_usize_cli("").is_err());
    }
}
