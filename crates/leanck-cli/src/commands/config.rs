use crate::args::ConfigArgs;
use anyhow::{Context, Result, bail};
use leanck_config::{Settings, init_path, starter_config};
use std::path::PathBuf;

/// Configuration inspection is shared by CLI tooling; it never starts Lean.
#[derive(Debug, clap::Args)]
pub(crate) struct Args {
    #[command(subcommand)]
    pub action: Action,
    /// Directory or file used to discover project configuration.
    #[arg(long, global = true, hide_short_help = true)]
    pub workspace: Option<PathBuf>,
    /// Print machine-readable JSON, including configuration sources where available.
    #[arg(long, global = true, hide_short_help = true)]
    pub json: bool,
}
#[derive(Debug, clap::Subcommand)]
pub enum Action {
    /// Show effective configuration (JSON also includes sources).
    #[command(after_long_help = "Examples:\n  leanck config show\n  leanck config show --json")]
    Show,
    /// Validate all selected configuration files and overrides.
    #[command(
        after_long_help = "Examples:\n  leanck config check\n  leanck --config custom.toml config check"
    )]
    Check,
    /// Print all built-in defaults as a ready-to-save TOML template.
    #[command(after_long_help = "Example:\n  leanck config defaults")]
    Defaults,
    /// Show loaded files and the winning source of every setting.
    #[command(after_long_help = "Example:\n  leanck config sources")]
    Sources,
    /// Set up configuration interactively, or write a starter file without a terminal.
    #[command(
        after_long_help = "Examples:\n  leanck config init\n  leanck config init --user\n  leanck config init --no-interactive"
    )]
    Init {
        /// Destination file (default: leanck.toml in the current directory).
        path: Option<PathBuf>,
        /// Configure user defaults for all projects instead of project settings.
        #[arg(long, conflicts_with = "path")]
        user: bool,
        /// Write a starter file without prompts; never overwrite an existing file.
        #[arg(long)]
        no_interactive: bool,
    },
}
impl Args {
    pub fn run(&self, args: &ConfigArgs) -> Result<()> {
        if matches!(self.action, Action::Defaults | Action::Init { .. }) {
            let defaults = Settings::default();
            let text = toml::to_string_pretty(&defaults)?;
            if let Action::Init {
                path,
                user,
                no_interactive,
            } = &self.action
            {
                if leanck_config::wizard::interactive(self.json, *no_interactive) {
                    return leanck_config::wizard::run(
                        path.as_deref(),
                        *user,
                        self.workspace.as_deref(),
                        &args.options(),
                    );
                }
                let path = init_path(path.as_deref(), *user, self.workspace.as_deref())?;
                let text = starter_config(&defaults)?;
                if *user {
                    std::fs::create_dir_all(path.parent().context("user config has no parent")?)?;
                }
                use std::io::Write;
                let mut file = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .with_context(|| {
                        format!(
                            "cannot create {}; existing files are never overwritten",
                            path.display()
                        )
                    })?;
                file.write_all(text.as_bytes())?;
                if self.json {
                    println!("{}", serde_json::json!({"created":path}));
                } else {
                    println!("created {}", path.display());
                }
            } else if self.json {
                println!("{}", serde_json::to_string_pretty(&defaults)?);
            } else {
                print!("{text}");
            }
            return Ok(());
        }
        let start = self.workspace.clone().unwrap_or(std::env::current_dir()?);
        let loaded = args.load(&start)?;
        let warnings = loaded.settings.warnings();
        if self.json && matches!(self.action, Action::Sources) && !warnings.is_empty() {
            eprintln!("{}", serde_json::json!({"warnings": warnings}));
        } else if !self.json {
            for warning in &warnings {
                eprintln!("warning: {}", warning.message);
            }
        }
        match self.action {
            Action::Show => {
                if self.json {
                    println!(
                        "{}",
                        serde_json::to_string_pretty(
                            &serde_json::json!({"settings": loaded.settings, "files": loaded.files, "sources": loaded.sources, "warnings": warnings})
                        )?
                    );
                } else {
                    print!("{}", loaded.to_toml()?);
                }
            }
            Action::Check => {
                if self.json {
                    println!(
                        "{}",
                        serde_json::json!({"valid":true,"files":loaded.files,"warnings":warnings})
                    );
                } else {
                    println!("configuration valid ({} files)", loaded.files.len());
                }
            }
            Action::Sources => {
                if self.json {
                    println!("{}", serde_json::to_string_pretty(&loaded.sources)?);
                } else {
                    for (key, source) in loaded.sources {
                        println!("{key}: {source}");
                    }
                }
            }
            _ => bail!("unreachable configuration action"),
        }
        Ok(())
    }
}
