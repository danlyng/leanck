//! `leanck _install` options: every `install.sh` flag, as one clap struct.
//!
//! Values are kept as raw text. [`crate::installer::Settings::resolve`]
//! validates and normalizes them so that errors carry the installer's
//! messages. The order-sensitive mode flags (`--plugin`, `--all`,
//! `--standalone`) are resolved from argument indices by [`InstallArgs::parse_from`].
use clap::{ArgAction, ArgMatches, Args, CommandFactory, FromArgMatches, Parser, error::ErrorKind};
use serde::Serialize;
use std::{ffi::OsString, path::PathBuf};

const ABOUT: &str = "Install leanck — configure, review, and approve changes to your user account.";

const AFTER_HELP: &str = "\
Installation modes:
  Plugin (default): bundles the CLI, all skills, agents, and hooks for Claude Code
  or Codex, with optional MCP. A la carte (--standalone): installs only the CLI,
  individual skills, and agents you select, without plugin registration or hooks.

Skills:
  leanck        leanck command reference
  lean-proving  opinionated workflow for proving theorems
  lean-style    opinionated workflow for code and documentation style cleanup

Examples:
  /bin/bash -c \"$(curl -fsSL https://github.com/danlyng/leanck/releases/latest/download/install.sh)\"
  sh install.sh --plugin codex
  sh install.sh --standalone --skills both --skill leanck
  sh install.sh --yes --all --version v0.30.0
  sh install.sh --from-repo .";

/// Installation mode.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    Plugin,
    Standalone,
}
impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Plugin => "plugin",
            Mode::Standalone => "standalone",
        }
    }
}

/// Flags of `leanck _install`, which `install.sh` runs from an extracted release.
#[derive(Args, Debug, Clone, Default)]
#[command(
    about = ABOUT,
    after_help = AFTER_HELP,
    args_override_self = true,
    disable_version_flag = true
)]
pub struct InstallArgs {
    /// Unpacked release directory to install from (supplied by install.sh).
    #[arg(long, value_name = "DIR", help_heading = "Source & output")]
    pub release_dir: Option<PathBuf>,

    /// Approve selected settings without prompts (required without a TTY).
    #[arg(short = 'y', long, help_heading = "Common options")]
    pub yes: bool,
    /// Preview without modifying files or running host CLIs.
    #[arg(long, help_heading = "Common options")]
    pub dry_run: bool,
    /// Back up and replace conflicting files (does not imply --yes).
    #[arg(long, help_heading = "Common options")]
    pub force: bool,
    /// Register plugins: claude, codex, both, or none (default: detected).
    #[arg(long, value_name = "HOST", action = ArgAction::Append, allow_hyphen_values = true, help_heading = "Common options")]
    pub plugin: Vec<String>,
    /// Plugin mode for both hosts plus Codex custom agents.
    #[arg(long, help_heading = "Common options")]
    pub all: bool,
    /// Install standalone components instead of the plugin.
    #[arg(long, help_heading = "Common options")]
    pub standalone: bool,
    /// Release tag to install (default: latest, or saved selection).
    #[arg(
        long,
        value_name = "VERSION",
        allow_hyphen_values = true,
        help_heading = "Common options"
    )]
    pub version: Option<String>,
    /// Build and install from a local checkout.
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Common options"
    )]
    pub from_repo: Option<String>,

    /// Install the executable (default).
    #[arg(long, overrides_with = "no_cli", help_heading = "Component selection")]
    pub cli: bool,
    /// Skip the executable.
    #[arg(long, overrides_with = "cli", help_heading = "Component selection")]
    pub no_cli: bool,
    /// Enable plugin MCP definitions.
    #[arg(long, overrides_with = "no_mcp", help_heading = "Component selection")]
    pub mcp: bool,
    /// Disable plugin MCP definitions (default).
    #[arg(long, overrides_with = "mcp", help_heading = "Component selection")]
    pub no_mcp: bool,
    /// Standalone skills: none, claude, codex, or both.
    #[arg(long, value_name = "HOST", action = ArgAction::Append, allow_hyphen_values = true, help_heading = "Component selection")]
    pub skills: Vec<String>,
    /// Select an a la carte skill (repeatable; default: all; or none).
    #[arg(long, value_name = "NAME", action = ArgAction::Append, allow_hyphen_values = true, help_heading = "Component selection")]
    pub skill: Vec<String>,
    /// Custom agents: none, claude, codex, or both.
    #[arg(long, value_name = "HOST", action = ArgAction::Append, allow_hyphen_values = true, help_heading = "Component selection")]
    pub agents: Vec<String>,
    /// Shell completions: bash, zsh, fish, powershell, elvish, or none.
    #[arg(
        long,
        value_name = "SHELL",
        allow_hyphen_values = true,
        help_heading = "Component selection"
    )]
    pub completions: Option<String>,

    /// Executable dir (plugin: DATA_DIR/bin; standalone: ~/.local/bin).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub bin_dir: Option<String>,
    /// Plugin data dir (default: $XDG_DATA_HOME/leanck).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub data_dir: Option<String>,
    /// Installer state & backups (default: $XDG_STATE_HOME/leanck/installer).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub state_dir: Option<String>,
    /// Claude config dir (default: $CLAUDE_CONFIG_DIR or ~/.claude).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub claude_dir: Option<String>,
    /// Codex config dir (default: $CODEX_HOME or ~/.codex).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub codex_dir: Option<String>,
    /// Claude standalone skills dir (default: CLAUDE_DIR/skills).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub claude_skills_dir: Option<String>,
    /// Claude agent definitions dir (default: CLAUDE_DIR/agents).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub claude_agents_dir: Option<String>,
    /// Codex standalone skills dir (default: ~/.agents/skills).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub codex_skills_dir: Option<String>,
    /// Codex agent definitions dir (default: CODEX_DIR/agents/leanck).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub codex_agents_dir: Option<String>,
    /// Override the selected shell's completions directory.
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub completions_dir: Option<String>,
    /// Fish completions default dir (honors XDG_CONFIG_HOME).
    #[arg(
        long,
        value_name = "DIR",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub fish_completions_dir: Option<String>,
    /// Additional skills directory (repeatable).
    #[arg(long, value_name = "DIR", action = ArgAction::Append, allow_hyphen_values = true, help_heading = "Directories & paths")]
    pub extra_skills_dir: Vec<String>,
    /// Additional Claude agent directory (repeatable).
    #[arg(long, value_name = "DIR", action = ArgAction::Append, allow_hyphen_values = true, help_heading = "Directories & paths")]
    pub extra_claude_agents_dir: Vec<String>,
    /// Additional Codex agent directory (repeatable).
    #[arg(long, value_name = "DIR", action = ArgAction::Append, allow_hyphen_values = true, help_heading = "Directories & paths")]
    pub extra_codex_agents_dir: Vec<String>,
    /// Shell for PATH updates: fish, bash, or zsh (default: $SHELL).
    #[arg(
        long,
        value_name = "SHELL",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub shell: Option<String>,
    /// Add a managed PATH block to the selected shell profile.
    #[arg(
        long,
        overrides_with = "no_add_to_path",
        help_heading = "Directories & paths"
    )]
    pub add_to_path: bool,
    /// Disable a remembered PATH update.
    #[arg(
        long,
        overrides_with = "add_to_path",
        help_heading = "Directories & paths"
    )]
    pub no_add_to_path: bool,
    /// Override the shell profile file to update.
    #[arg(
        long,
        value_name = "FILE",
        allow_hyphen_values = true,
        help_heading = "Directories & paths"
    )]
    pub path_file: Option<String>,

    /// HTTPS release mirror (default: GitHub releases).
    #[arg(
        long,
        value_name = "URL",
        allow_hyphen_values = true,
        help_heading = "Source & output"
    )]
    pub release_url: Option<String>,
    /// Command timeout in seconds (default: 300).
    #[arg(
        long,
        value_name = "SECONDS",
        allow_hyphen_values = true,
        help_heading = "Source & output"
    )]
    pub timeout: Option<String>,
    /// Emit JSON status/errors; disables prompts (use --yes to install).
    #[arg(long, help_heading = "Source & output")]
    pub json: bool,
    /// Disable ANSI color (also honors NO_COLOR and TERM=dumb).
    #[arg(long, help_heading = "Source & output")]
    pub no_color: bool,
    /// Print long plans without paging.
    #[arg(long, help_heading = "Source & output")]
    pub no_pager: bool,
    /// Suppress non-essential progress messages.
    #[arg(short, long, help_heading = "Source & output")]
    pub quiet: bool,
    /// Increase detail (repeatable for command output).
    #[arg(short, long, action = ArgAction::Count, help_heading = "Source & output")]
    pub verbose: u8,

    /// The last of `--plugin HOST`/`--all` (plugin) and `--plugin none`/`--standalone`.
    #[arg(skip)]
    pub mode_request: Option<Mode>,
    /// `--plugin` values in argument order, with `both` for each `--all`.
    #[arg(skip)]
    pub plugin_order: Option<Vec<String>>,
    /// The last of `--cli`/`--all` (true) and `--no-cli` (false).
    #[arg(skip)]
    pub cli_request: Option<bool>,
}

#[derive(Parser)]
#[command(name = "leanck _install", disable_version_flag = true)]
struct Standalone {
    #[command(flatten)]
    args: InstallArgs,
}

impl InstallArgs {
    /// The `_install` command definition, for help output.
    pub fn command() -> clap::Command {
        Standalone::command()
    }

    /// Parse arguments that follow `_install`, resolving order-sensitive flags.
    pub fn parse_from<I, T>(argv: I) -> Result<Self, clap::Error>
    where
        I: IntoIterator<Item = T>,
        T: Into<OsString> + Clone,
    {
        let argv = std::iter::once(OsString::from("leanck _install"))
            .chain(argv.into_iter().map(Into::into));
        let matches = Self::command().try_get_matches_from(argv)?;
        let mut args = Self::from_arg_matches(&matches)?;
        args.mode_request = mode_request(&matches, &args);
        let mut plugin: Vec<(usize, String)> = matches
            .indices_of("plugin")
            .map(|indices| indices.zip(args.plugin.iter().cloned()).collect())
            .unwrap_or_default();
        let mut cli: Vec<(usize, bool)> = vec![];
        for (id, value) in [("all", true), ("cli", true), ("no_cli", false)] {
            if matches.get_flag(id)
                && let Some(index) = matches.indices_of(id).and_then(Iterator::max)
            {
                cli.push((index, value));
                if id == "all" {
                    plugin.push((index, "both".into()));
                }
            }
        }
        plugin.sort();
        cli.sort();
        args.plugin_order = Some(plugin.into_iter().map(|(_, value)| value).collect());
        args.cli_request = cli.last().map(|(_, value)| *value);
        Ok(args)
    }

    /// Whether prompts may be shown: no `--yes` or `--json`, and a terminal.
    pub fn interactive(&self, terminal: bool) -> bool {
        !self.yes && !self.json && terminal
    }

    /// Changes need `--yes` or a terminal to approve them in; dry runs need neither.
    pub fn require_approval(&self, interactive: bool) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.yes || self.dry_run || interactive,
            "approval is required; rerun in a terminal, use --yes for automation, or --dry-run to preview"
        );
        Ok(())
    }

    /// Value flags in the form `install.sh` reports them, paired with their values.
    pub(crate) fn values(&self) -> Vec<(&'static str, &str)> {
        let mut values = vec![];
        let single = [
            ("--version", &self.version),
            ("--from-repo", &self.from_repo),
            ("--completions", &self.completions),
            ("--bin-dir", &self.bin_dir),
            ("--data-dir", &self.data_dir),
            ("--state-dir", &self.state_dir),
            ("--claude-dir", &self.claude_dir),
            ("--codex-dir", &self.codex_dir),
            ("--claude-skills-dir", &self.claude_skills_dir),
            ("--claude-agents-dir", &self.claude_agents_dir),
            ("--codex-skills-dir", &self.codex_skills_dir),
            ("--codex-agents-dir", &self.codex_agents_dir),
            ("--completions-dir", &self.completions_dir),
            ("--fish-completions-dir", &self.fish_completions_dir),
            ("--shell", &self.shell),
            ("--path-file", &self.path_file),
            ("--release-url", &self.release_url),
            ("--timeout", &self.timeout),
        ];
        for (flag, value) in single {
            if let Some(value) = value {
                values.push((flag, value.as_str()));
            }
        }
        let lists = [
            ("--plugin", &self.plugin),
            ("--skills", &self.skills),
            ("--skill", &self.skill),
            ("--agents", &self.agents),
            ("--extra-skills-dir", &self.extra_skills_dir),
            ("--extra-claude-agents-dir", &self.extra_claude_agents_dir),
            ("--extra-codex-agents-dir", &self.extra_codex_agents_dir),
        ];
        for (flag, list) in lists {
            for value in list {
                values.push((flag, value.as_str()));
            }
        }
        values
    }
}

fn mode_request(matches: &ArgMatches, args: &InstallArgs) -> Option<Mode> {
    let mut last: Option<(usize, Mode)> = None;
    let mut consider = |index: usize, mode: Mode| {
        if last.is_none_or(|(i, _)| index > i) {
            last = Some((index, mode));
        }
    };
    if let Some(indices) = matches.indices_of("plugin") {
        for (index, value) in indices.zip(&args.plugin) {
            consider(
                index,
                if value == "none" {
                    Mode::Standalone
                } else {
                    Mode::Plugin
                },
            );
        }
    }
    if args.all
        && let Some(index) = matches.indices_of("all").and_then(Iterator::max)
    {
        consider(index, Mode::Plugin);
    }
    if args.standalone
        && let Some(index) = matches.indices_of("standalone").and_then(Iterator::max)
    {
        consider(index, Mode::Standalone);
    }
    last.map(|(_, mode)| mode)
}

/// Convert a parse error to the installer's message, or `None` for help output.
pub fn parse_error_message(error: &clap::Error) -> Option<String> {
    use clap::error::ContextKind;
    let argument = || match error.get(ContextKind::InvalidArg) {
        Some(clap::error::ContextValue::String(s)) => {
            s.split([' ', '=']).next().unwrap_or(s).to_owned()
        }
        Some(other) => other.to_string(),
        None => String::new(),
    };
    Some(match error.kind() {
        ErrorKind::DisplayHelp | ErrorKind::DisplayVersion => return None,
        ErrorKind::UnknownArgument | ErrorKind::InvalidSubcommand => {
            format!("unknown option: {}; see --help", argument())
        }
        ErrorKind::InvalidValue | ErrorKind::NoEquals | ErrorKind::WrongNumberOfValues => {
            format!("{} needs a value; see --help", argument())
        }
        _ => {
            let text = error.render().to_string();
            let line = text.lines().next().unwrap_or("invalid arguments");
            format!(
                "{}; see --help",
                line.trim_start_matches("error: ").trim_end_matches('.')
            )
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(argv: &[&str]) -> InstallArgs {
        InstallArgs::parse_from(argv).unwrap()
    }

    #[test]
    fn definition_is_valid() {
        InstallArgs::command().debug_assert();
    }

    #[test]
    fn the_last_mode_flag_wins() {
        for (argv, expected) in [
            (&[][..], None),
            (&["--all"][..], Some(Mode::Plugin)),
            (&["--standalone", "--all"][..], Some(Mode::Plugin)),
            (&["--all", "--standalone"][..], Some(Mode::Standalone)),
            (
                &["--plugin", "codex", "--plugin", "none"][..],
                Some(Mode::Standalone),
            ),
            (
                &["--plugin", "none", "--plugin", "claude"][..],
                Some(Mode::Plugin),
            ),
            (
                &["--plugin", "claude", "--standalone"][..],
                Some(Mode::Standalone),
            ),
            (
                &["--standalone", "--plugin", "both"][..],
                Some(Mode::Plugin),
            ),
        ] {
            assert_eq!(parse(argv).mode_request, expected, "{argv:?}");
        }
    }

    #[test]
    fn later_values_and_paired_switches_override_earlier_ones() {
        let args = parse(&[
            "--bin-dir",
            "a",
            "--bin-dir",
            "b",
            "--no-cli",
            "--cli",
            "--mcp",
            "--no-mcp",
            "--add-to-path",
            "--no-add-to-path",
            "-vv",
            "-v",
        ]);
        assert_eq!(args.bin_dir.as_deref(), Some("b"));
        assert!(args.cli && !args.no_cli);
        assert!(args.no_mcp && !args.mcp);
        assert!(args.no_add_to_path && !args.add_to_path);
        assert_eq!(args.verbose, 3);
    }

    #[test]
    fn values_beginning_with_a_single_dash_are_values() {
        let args = parse(&["--bin-dir", "-odd", "--skill", "leanck", "--skill", "-x"]);
        assert_eq!(args.bin_dir.as_deref(), Some("-odd"));
        assert_eq!(args.skill, ["leanck", "-x"]);
    }

    #[test]
    fn parse_errors_use_installer_messages() {
        for (argv, expected) in [
            (&["--wat"][..], "unknown option: --wat; see --help"),
            (&["--skills"][..], "--skills needs a value; see --help"),
            (
                &["--signing-identity", "x"][..],
                "unknown option: --signing-identity; see --help",
            ),
            (&["-vx"][..], "unknown option: -x; see --help"),
        ] {
            let error = InstallArgs::parse_from(argv).unwrap_err();
            assert_eq!(
                parse_error_message(&error).as_deref(),
                Some(expected),
                "{argv:?}"
            );
        }
        let help = InstallArgs::parse_from(["--help"]).unwrap_err();
        assert_eq!(parse_error_message(&help), None);
    }
}
