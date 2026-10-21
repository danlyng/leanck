//! Effective installer configuration, resolved from defaults, saved state,
//! the environment, detected hosts, and flags, in that order of precedence.
use super::{Environment, InstallArgs, Mode, SavedState};
use crate::paths::{absolute, valid_text};
use anyhow::{Result, bail, ensure};
use serde::Serialize;
use std::path::{Path, PathBuf};

pub const DEFAULT_RELEASE_URL: &str = "https://github.com/danlyng/leanck/releases";
pub const DEFAULT_TIMEOUT: u64 = 300;

/// A pair of host switches, written `none`, `claude`, `codex`, or `both`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct Hosts {
    pub claude: bool,
    pub codex: bool,
}
impl Hosts {
    pub const NONE: Hosts = Hosts {
        claude: false,
        codex: false,
    };
    pub fn any(self) -> bool {
        self.claude || self.codex
    }
    pub fn label(self) -> &'static str {
        match (self.claude, self.codex) {
            (false, false) => "none",
            (true, false) => "claude",
            (false, true) => "codex",
            (true, true) => "both",
        }
    }
    /// Parse one host-choice word.
    pub fn parse(value: &str) -> Option<Hosts> {
        Some(match value {
            "none" => Hosts::NONE,
            "claude" => Hosts {
                claude: true,
                codex: false,
            },
            "codex" => Hosts {
                claude: false,
                codex: true,
            },
            "both" => Hosts {
                claude: true,
                codex: true,
            },
            _ => return None,
        })
    }
    /// Fold repeated `--NAME HOST` values: values accumulate, and `none` clears.
    fn fold(name: &str, values: &[String]) -> Result<Hosts> {
        let mut hosts = Hosts::NONE;
        for value in values {
            let parsed = Hosts::parse(value).ok_or_else(|| {
                anyhow::anyhow!("{name}: expected none, claude, codex, or both (got {value})")
            })?;
            if parsed == Hosts::NONE {
                hosts = Hosts::NONE;
            } else {
                hosts.claude |= parsed.claude;
                hosts.codex |= parsed.codex;
            }
        }
        Ok(hosts)
    }
}

/// The selected standalone skills.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(into = "String")]
pub enum SkillSelection {
    All,
    None,
    Names(Vec<String>),
}
impl SkillSelection {
    pub fn parse(value: &str) -> SkillSelection {
        match value.trim() {
            "all" | "" => SkillSelection::All,
            "none" => SkillSelection::None,
            names => SkillSelection::Names(names.split_whitespace().map(str::to_owned).collect()),
        }
    }
    pub fn render(&self) -> String {
        match self {
            SkillSelection::All => "all".into(),
            SkillSelection::None => "none".into(),
            SkillSelection::Names(names) => names.join(" "),
        }
    }
    pub fn includes(&self, name: &str) -> bool {
        match self {
            SkillSelection::All => true,
            SkillSelection::None => false,
            SkillSelection::Names(names) => names.iter().any(|n| n == name),
        }
    }
    /// Fold repeated `--skill` values over a saved selection.
    fn fold(values: &[String]) -> Result<SkillSelection> {
        let mut selection = SkillSelection::None;
        for value in values {
            valid_skill_name(value)?;
            selection = match (value.as_str(), selection) {
                ("all", _) => SkillSelection::All,
                ("none", _) => SkillSelection::None,
                (name, SkillSelection::Names(mut names)) => {
                    if !names.iter().any(|n| n == name) {
                        names.push(name.to_owned());
                    }
                    SkillSelection::Names(names)
                }
                (name, _) => SkillSelection::Names(vec![name.to_owned()]),
            };
        }
        Ok(selection)
    }
}
impl From<SkillSelection> for String {
    fn from(value: SkillSelection) -> String {
        value.render()
    }
}

pub fn valid_skill_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            && !name.starts_with('-')
            && !name.ends_with('-')
            && !name.contains("--"),
        "invalid skill name: {name}; use --skill NAME (repeatable), all, or none"
    );
    Ok(())
}

/// Shell completion scripts the executable can generate.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum CompletionShell {
    None,
    Bash,
    Zsh,
    Fish,
    Powershell,
    Elvish,
}
impl CompletionShell {
    pub const ALL: [CompletionShell; 6] = [
        CompletionShell::None,
        CompletionShell::Bash,
        CompletionShell::Zsh,
        CompletionShell::Fish,
        CompletionShell::Powershell,
        CompletionShell::Elvish,
    ];
    pub fn parse(value: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|shell| shell.as_str() == value)
    }
    pub fn as_str(self) -> &'static str {
        match self {
            CompletionShell::None => "none",
            CompletionShell::Bash => "bash",
            CompletionShell::Zsh => "zsh",
            CompletionShell::Fish => "fish",
            CompletionShell::Powershell => "powershell",
            CompletionShell::Elvish => "elvish",
        }
    }
    /// The completion default for a login shell's basename.
    pub fn for_shell(shell: &str) -> Self {
        match shell {
            "bash" => CompletionShell::Bash,
            "zsh" => CompletionShell::Zsh,
            "fish" => CompletionShell::Fish,
            "elvish" => CompletionShell::Elvish,
            "pwsh" | "powershell" => CompletionShell::Powershell,
            _ => CompletionShell::None,
        }
    }
    /// The completion script's file name.
    pub fn file_name(self) -> Option<&'static str> {
        match self {
            CompletionShell::None => None,
            CompletionShell::Bash => Some("leanck"),
            CompletionShell::Zsh => Some("_leanck"),
            CompletionShell::Fish => Some("leanck.fish"),
            CompletionShell::Powershell => Some("leanck.ps1"),
            CompletionShell::Elvish => Some("leanck.elv"),
        }
    }
}

/// The resolved installer configuration. The interactive editor changes
/// these fields directly; [`super::plan`] consumes them without re-deriving.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Settings {
    pub mode: Mode,
    pub cli: bool,
    /// `--cli` or `--no-cli` was given.
    pub cli_explicit: bool,
    pub mcp_enabled: bool,
    pub plugin_hosts: Hosts,
    pub skill_hosts: Hosts,
    pub agent_hosts: Hosts,
    pub selected_skills: SkillSelection,
    /// Interactive defaults only; never part of saved or JSON settings.
    #[serde(skip)]
    pub skills_chosen: bool,
    #[serde(skip)]
    pub skill_hosts_chosen: bool,
    #[serde(skip)]
    pub agent_hosts_chosen: bool,
    #[serde(skip)]
    pub detected_hosts: Hosts,
    /// `--skill` was given.
    pub skill_explicit: bool,
    pub bin_dir: PathBuf,
    /// The original PATH entries, never persisted as installer selections.
    pub path_entries: Vec<PathBuf>,
    /// `--bin-dir` was given.
    pub bin_dir_explicit: bool,
    /// The remembered executable directories for each mode.
    pub standalone_bin_dir: PathBuf,
    pub plugin_bin_dir: Option<PathBuf>,
    pub data_dir: PathBuf,
    pub state_dir: PathBuf,
    pub claude_dir: PathBuf,
    /// `None` means `CLAUDE_DIR/skills`.
    pub claude_skills_dir: Option<PathBuf>,
    /// `None` means `CLAUDE_DIR/agents/leanck`.
    pub claude_agents_dir: Option<PathBuf>,
    pub codex_dir: PathBuf,
    pub codex_skills: PathBuf,
    /// `None` means `CODEX_DIR/agents/leanck`.
    pub codex_agents_dir: Option<PathBuf>,
    pub completions: CompletionShell,
    /// `None` means the selected shell's default directory.
    pub completions_dir: Option<PathBuf>,
    pub fish_completions_dir: PathBuf,
    pub add_to_path: bool,
    /// A PATH flag was given, or a PATH choice was saved.
    pub path_explicit: bool,
    pub shell: String,
    pub path_file: PathBuf,
    /// `--path-file` was given.
    pub path_file_explicit: bool,
    pub config_example: PathBuf,
    pub extra_skills: Vec<PathBuf>,
    pub extra_claude_agents: Vec<PathBuf>,
    pub extra_codex_agents: Vec<PathBuf>,
    /// `latest`, or a release tag without its `v`.
    pub version: String,
    /// `--version` was given.
    pub version_explicit: bool,
    pub release_url: String,
    /// A custom release API base remembered from `leanck update`.
    pub release_api: Option<String>,
    pub from_repo: Option<PathBuf>,
    pub timeout: u64,
    pub force: bool,
    pub dry_run: bool,
    /// XDG-derived bases for default destinations.
    pub data_home: PathBuf,
    pub config_home: PathBuf,
    pub home: PathBuf,
}

impl Settings {
    /// The installer state directory: `--state-dir`, else the XDG default.
    pub fn state_dir(args: &InstallArgs, env: &Environment) -> Result<PathBuf> {
        match &args.state_dir {
            Some(dir) => {
                need_value("--state-dir", dir)?;
                absolute(dir, &env.home, &env.cwd)
            }
            None => Ok(env.state_home().join("leanck/installer")),
        }
    }

    /// Resolve every choice except mode forcing; see [`Settings::apply_mode`].
    pub fn resolve(args: &InstallArgs, env: &Environment, saved: &SavedState) -> Result<Settings> {
        for (flag, value) in args.values() {
            need_value(flag, value)?;
            valid_text(value)?;
        }
        let home = env.home.clone();
        let path = |value: &str| absolute(value, &env.home, &env.cwd);
        let data_home = env.data_home();
        let config_home = env.config_home();
        let shell = env
            .shell
            .as_deref()
            .map(basename)
            .filter(|s| !s.is_empty())
            .unwrap_or("sh")
            .to_owned();
        let mut s = Settings {
            mode: Mode::Plugin,
            cli: true,
            cli_explicit: false,
            mcp_enabled: false,
            plugin_hosts: Hosts::NONE,
            skill_hosts: Hosts::NONE,
            agent_hosts: Hosts::NONE,
            selected_skills: SkillSelection::All,
            skills_chosen: false,
            skill_hosts_chosen: false,
            agent_hosts_chosen: false,
            detected_hosts: Hosts {
                claude: env.has_command("claude"),
                codex: env.has_command("codex"),
            },
            skill_explicit: false,
            bin_dir: home.join(".local/bin"),
            path_entries: env.path.clone(),
            bin_dir_explicit: false,
            standalone_bin_dir: home.join(".local/bin"),
            plugin_bin_dir: None,
            data_dir: data_home.join("leanck"),
            state_dir: Self::state_dir(args, env)?,
            claude_dir: home.join(".claude"),
            claude_skills_dir: None,
            claude_agents_dir: None,
            codex_dir: home.join(".codex"),
            codex_skills: home.join(".agents/skills"),
            codex_agents_dir: None,
            completions: CompletionShell::for_shell(&shell),
            completions_dir: None,
            fish_completions_dir: config_home.join("fish/completions"),
            add_to_path: false,
            path_explicit: false,
            path_file: default_profile(&shell, &home, &config_home),
            path_file_explicit: false,
            shell,
            config_example: config_home.join("leanck/config.example.toml"),
            extra_skills: vec![],
            extra_claude_agents: vec![],
            extra_codex_agents: vec![],
            version: "latest".into(),
            version_explicit: false,
            release_url: DEFAULT_RELEASE_URL.into(),
            release_api: None,
            from_repo: None,
            timeout: DEFAULT_TIMEOUT,
            force: args.force,
            dry_run: args.dry_run,
            data_home,
            config_home,
            home,
        };
        let mut completions = s.completions.as_str().to_owned();

        // Saved selections.
        for (key, value) in saved.entries() {
            valid_text(value)?;
            let flag = || -> Option<bool> {
                match value {
                    "0" => Some(false),
                    "1" => Some(true),
                    _ => None,
                }
            };
            let optional_path = || -> Result<Option<PathBuf>> {
                if value.is_empty() {
                    Ok(None)
                } else {
                    path(value).map(Some)
                }
            };
            match key {
                "extra_skills" => push_location(&mut s.extra_skills, path(value)?),
                "extra_claude_agents" => push_location(&mut s.extra_claude_agents, path(value)?),
                "extra_codex_agents" => push_location(&mut s.extra_codex_agents, path(value)?),
                "cli" => s.cli = flag().unwrap_or(s.cli),
                "skills_claude" => {
                    if let Some(enabled) = flag() {
                        s.skill_hosts.claude = enabled;
                        s.skill_hosts_chosen = true;
                    }
                }
                "skills_codex" => {
                    if let Some(enabled) = flag() {
                        s.skill_hosts.codex = enabled;
                        s.skill_hosts_chosen = true;
                    }
                }
                "agents_claude" => {
                    if let Some(enabled) = flag() {
                        s.agent_hosts.claude = enabled;
                        s.agent_hosts_chosen = true;
                    }
                }
                "agents_codex" => {
                    if let Some(enabled) = flag() {
                        s.agent_hosts.codex = enabled;
                        s.agent_hosts_chosen = true;
                    }
                }
                "mcp_enabled" => s.mcp_enabled = flag().unwrap_or(s.mcp_enabled),
                "add_to_path" => {
                    if let Some(enabled) = flag() {
                        // A saved PATH choice is respected in either mode.
                        s.add_to_path = enabled;
                        s.path_explicit = true;
                    }
                }
                "bin_dir" | "standalone_bin_dir" => {
                    if let Some(dir) = optional_path()? {
                        s.standalone_bin_dir = dir;
                    }
                }
                "plugin_bin_dir" => s.plugin_bin_dir = optional_path()?,
                "selected_skills" => {
                    s.selected_skills = SkillSelection::parse(value);
                    s.skills_chosen = true;
                }
                "version" if !value.is_empty() => s.version = value.trim_start_matches('v').into(),
                "data_dir" => set_path(&mut s.data_dir, optional_path()?),
                "claude_dir" => set_path(&mut s.claude_dir, optional_path()?),
                "codex_dir" => set_path(&mut s.codex_dir, optional_path()?),
                "codex_skills" => set_path(&mut s.codex_skills, optional_path()?),
                "codex_agents_dir" => s.codex_agents_dir = optional_path()?,
                "claude_skills_dir" => s.claude_skills_dir = optional_path()?,
                "claude_agents_dir" => s.claude_agents_dir = optional_path()?,
                "completions_dir" => s.completions_dir = optional_path()?,
                "fish_completions_dir" => set_path(&mut s.fish_completions_dir, optional_path()?),
                "path_file" => set_path(&mut s.path_file, optional_path()?),
                "completions" if !value.is_empty() => completions = value.into(),
                "shell" if !value.is_empty() => s.shell = basename(value).into(),
                "release_api" if !value.is_empty() => s.release_api = Some(value.into()),
                _ => {}
            }
        }

        // Environment variables name host homes explicitly; they override
        // remembered directories, and flags override them.
        if let Some(dir) = &env.claude_config_dir {
            s.claude_dir = dir.clone();
        }
        if let Some(dir) = &env.codex_home {
            s.codex_dir = dir.clone();
        }
        s.plugin_hosts = Hosts {
            claude: env.has_command("claude"),
            codex: env.has_command("codex"),
        };

        // Flags.
        let mut plugin = args.plugin.clone();
        if args.plugin_order.is_none() && args.all {
            plugin.push("both".into());
        }
        let plugin = args.plugin_order.as_ref().unwrap_or(&plugin);
        if !plugin.is_empty() {
            s.plugin_hosts = Hosts::fold("plugin", plugin)?;
        }
        if !args.skills.is_empty() {
            s.skill_hosts = Hosts::fold("skills", &args.skills)?;
            s.skill_hosts_chosen = true;
        }
        if !args.agents.is_empty() {
            s.agent_hosts = Hosts::fold("agents", &args.agents)?;
            s.agent_hosts_chosen = true;
        }
        if !args.skill.is_empty() {
            s.selected_skills = SkillSelection::fold(&args.skill)?;
            s.skill_explicit = true;
            s.skills_chosen = true;
        }
        let cli = args.cli_request.or(if args.no_cli {
            Some(false)
        } else if args.cli || args.all {
            Some(true)
        } else {
            None
        });
        if let Some(cli) = cli {
            s.cli = cli;
            s.cli_explicit = true;
        }
        if args.mcp || args.no_mcp {
            s.mcp_enabled = args.mcp;
        }
        if args.add_to_path || args.no_add_to_path {
            s.add_to_path = args.add_to_path;
            s.path_explicit = true;
        }
        for (list, values) in [
            (&mut s.extra_skills, &args.extra_skills_dir),
            (&mut s.extra_claude_agents, &args.extra_claude_agents_dir),
            (&mut s.extra_codex_agents, &args.extra_codex_agents_dir),
        ] {
            for value in values {
                push_location(list, path(value)?);
            }
        }
        if let Some(dir) = &args.bin_dir {
            s.bin_dir = path(dir)?;
            s.bin_dir_explicit = true;
        } else {
            s.bin_dir = s.standalone_bin_dir.clone();
        }
        for (target, value) in [
            (&mut s.data_dir, &args.data_dir),
            (&mut s.claude_dir, &args.claude_dir),
            (&mut s.codex_dir, &args.codex_dir),
            (&mut s.codex_skills, &args.codex_skills_dir),
            (&mut s.fish_completions_dir, &args.fish_completions_dir),
        ] {
            if let Some(value) = value {
                *target = path(value)?;
            }
        }
        if let Some(dir) = &args.claude_skills_dir {
            let dir = path(dir)?;
            s.claude_skills_dir = (dir != s.claude_dir.join("skills")).then_some(dir);
        }
        if let Some(dir) = &args.claude_agents_dir {
            let dir = path(dir)?;
            s.claude_agents_dir = (dir != s.claude_dir.join("agents/leanck")).then_some(dir);
        }
        if let Some(dir) = &args.codex_agents_dir {
            s.codex_agents_dir = Some(path(dir)?);
        }
        if let Some(dir) = &args.completions_dir {
            s.completions_dir = Some(path(dir)?);
        }
        if let Some(file) = &args.path_file {
            s.path_file = path(file)?;
            s.path_file_explicit = true;
        }
        if let Some(value) = &args.completions {
            completions = value.clone();
        }
        if let Some(shell) = &args.shell {
            // A new shell re-derives the completion default and the profile.
            s.shell = basename(shell).to_owned();
            if args.completions.is_none() {
                completions = CompletionShell::for_shell(&s.shell).as_str().into();
            }
            if !s.path_file_explicit {
                s.path_file = default_profile(&s.shell, &s.home, &s.config_home);
            }
        }
        if let Some(url) = &args.release_url {
            s.release_url = url.clone();
        }
        if let Some(timeout) = &args.timeout {
            s.timeout = parse_timeout(timeout)?;
        }
        if let Some(version) = &args.version {
            s.version = version.clone();
            s.version_explicit = true;
        }
        if let Some(repo) = &args.from_repo {
            ensure!(
                !s.version_explicit,
                "--version selects a published release; omit it with --from-repo"
            );
            s.from_repo = Some(path(repo)?);
        }
        s.mode = match args.mode_request {
            Some(mode) => mode,
            None if !args.skills.is_empty()
                || !args.agents.is_empty()
                || s.skill_explicit
                || (s.cli_explicit && !s.cli) =>
            {
                Mode::Standalone
            }
            None => Mode::Plugin,
        };
        if s.mode == Mode::Plugin && !s.cli_explicit {
            s.cli = true;
        }

        s.apply_path_default();

        // Value validation.
        s.completions = CompletionShell::parse(&completions).ok_or_else(|| {
            anyhow::anyhow!("--completions expects bash, zsh, fish, powershell, elvish, or none")
        })?;
        if s.add_to_path {
            ensure_path_shell(&s.shell)?;
        }
        ensure!(
            s.release_url.starts_with("https://"),
            "--release-url must use HTTPS"
        );
        if let Some(stripped) = s.release_url.strip_suffix('/') {
            s.release_url = stripped.to_owned();
        }
        if s.version != "latest" {
            s.version = s.version.trim_start_matches('v').to_owned();
            ensure!(
                !s.version.is_empty()
                    && s.version
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-')),
                "invalid release version"
            );
        }
        Ok(s)
    }

    /// Validate choices a non-interactive run cannot ask about.
    pub fn validate_noninteractive(&self) -> Result<()> {
        match self.mode {
            Mode::Plugin => ensure!(
                self.cli,
                "plugin mode includes the executable; omit --no-cli or use --standalone"
            ),
            Mode::Standalone => ensure!(
                self.has_components(),
                "nothing selected; omit --no-cli or select --skills or --agents"
            ),
        }
        Ok(())
    }

    /// Apply the rules that follow from choosing `mode`: plugin mode bundles
    /// skills and agents, includes the executable, and adds it to PATH
    /// unless a PATH choice was given or saved; standalone mode registers no
    /// plugins.
    pub fn apply_mode(&mut self, mode: Mode) -> Result<()> {
        self.mode = mode;
        match mode {
            Mode::Plugin => {
                ensure!(
                    self.cli,
                    "plugin mode includes the executable; omit --no-cli or use --standalone"
                );
                ensure!(
                    !self.skill_explicit || self.selected_skills == SkillSelection::All,
                    "plugin mode includes all skills; use --standalone with --skills HOST to select individual skills"
                );
                self.skill_hosts = Hosts::NONE;
                self.agent_hosts = Hosts::NONE;
                if !self.bin_dir_explicit {
                    self.bin_dir = self
                        .plugin_bin_dir
                        .clone()
                        .unwrap_or_else(|| self.data_dir.join("bin"));
                }
                if !self.path_explicit {
                    self.add_to_path = true;
                }
            }
            Mode::Standalone => {
                self.plugin_hosts = Hosts::NONE;
                if !self.bin_dir_explicit {
                    self.bin_dir = self.standalone_bin_dir.clone();
                }
                ensure!(
                    self.has_components(),
                    "nothing selected; omit --no-cli or select --skills or --agents"
                );
            }
        }
        self.apply_path_default();
        Ok(())
    }

    /// Whether the executable directory was on the original PATH.
    pub fn bin_on_path(&self) -> bool {
        self.path_entries.iter().any(|entry| entry == &self.bin_dir)
    }

    /// An executable already on PATH needs no implicit profile update.
    pub fn apply_path_default(&mut self) {
        if !self.path_explicit && self.bin_on_path() {
            self.add_to_path = false;
        }
    }

    /// Change the data directory; an executable directory that followed the
    /// old default (`OLD/bin`) follows the new one.
    pub fn set_data_dir(&mut self, data_dir: PathBuf) {
        if self.bin_dir == self.data_dir.join("bin") {
            self.bin_dir = data_dir.join("bin");
        }
        self.data_dir = data_dir;
    }

    /// Change the PATH shell; a profile that was the old shell's default
    /// becomes the new shell's default.
    pub fn set_shell(&mut self, shell: &str) {
        let shell = basename(shell).to_owned();
        if self.path_file == self.default_profile(&self.shell) {
            self.path_file = self.default_profile(&shell);
        }
        self.shell = shell;
    }

    /// Whether anything is selected for installation.
    pub fn has_components(&self) -> bool {
        self.cli
            || self.agent_hosts.any()
            || !self.extra_claude_agents.is_empty()
            || !self.extra_codex_agents.is_empty()
            || (self.selected_skills != SkillSelection::None
                && (self.skill_hosts.any() || !self.extra_skills.is_empty()))
    }

    /// Whether `name` is installed: plugin mode always bundles every skill.
    pub fn skill_selected(&self, name: &str) -> bool {
        self.mode == Mode::Plugin || self.selected_skills.includes(name)
    }

    pub fn bundle_dir(&self) -> PathBuf {
        self.data_dir.join("bundle")
    }
    pub fn binary(&self) -> PathBuf {
        self.bin_dir.join("leanck")
    }
    pub fn claude_skills(&self) -> PathBuf {
        self.claude_skills_dir
            .clone()
            .unwrap_or_else(|| self.claude_dir.join("skills"))
    }
    pub fn claude_agents(&self) -> PathBuf {
        self.claude_agents_dir
            .clone()
            .unwrap_or_else(|| self.claude_dir.join("agents/leanck"))
    }
    pub fn codex_agent_dir(&self) -> PathBuf {
        self.codex_agents_dir
            .clone()
            .unwrap_or_else(|| self.codex_dir.join("agents/leanck"))
    }
    /// The default completion directory for `shell`.
    pub fn default_completion_dir(&self, shell: CompletionShell) -> Option<PathBuf> {
        Some(match shell {
            CompletionShell::None => return None,
            CompletionShell::Fish => self.fish_completions_dir.clone(),
            CompletionShell::Bash => self.data_home.join("bash-completion/completions"),
            CompletionShell::Zsh => self.data_home.join("zsh/site-functions"),
            CompletionShell::Powershell => self.config_home.join("powershell/Completions"),
            CompletionShell::Elvish => self.config_home.join("elvish/lib"),
        })
    }
    /// The completion directory, when completions are selected.
    pub fn completion_dir(&self) -> Option<PathBuf> {
        if self.completions == CompletionShell::None {
            return None;
        }
        self.completions_dir
            .clone()
            .or_else(|| self.default_completion_dir(self.completions))
    }
    pub fn completion_path(&self) -> Option<PathBuf> {
        Some(self.completion_dir()?.join(self.completions.file_name()?))
    }
    /// The default shell profile for `shell`.
    pub fn default_profile(&self, shell: &str) -> PathBuf {
        default_profile(shell, &self.home, &self.config_home)
    }
    /// The line the managed PATH block contains.
    pub fn profile_line(&self) -> String {
        let dir = self.bin_dir.to_string_lossy();
        if self.shell == "fish" {
            format!(
                "fish_add_path -- '{}'",
                dir.replace('\\', "\\\\").replace('\'', "\\'")
            )
        } else {
            format!("export PATH='{}':\"$PATH\"", dir.replace('\'', "'\\''"))
        }
    }
}

pub fn ensure_path_shell(shell: &str) -> Result<()> {
    ensure!(
        matches!(shell, "fish" | "bash" | "zsh"),
        "--add-to-path needs --shell fish, bash, or zsh"
    );
    Ok(())
}

fn default_profile(shell: &str, home: &Path, config_home: &Path) -> PathBuf {
    match shell {
        "fish" => config_home.join("fish/config.fish"),
        "zsh" => home.join(".zshrc"),
        _ => home.join(".bashrc"),
    }
}

fn basename(value: &str) -> &str {
    value.rsplit('/').next().unwrap_or(value)
}

fn need_value(flag: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.starts_with("--") {
        bail!("{flag} needs a value; see --help");
    }
    Ok(())
}

fn parse_timeout(value: &str) -> Result<u64> {
    match value.parse::<u64>() {
        Ok(seconds) if seconds > 0 && value.bytes().all(|b| b.is_ascii_digit()) => Ok(seconds),
        _ => bail!("--timeout must be a positive number of seconds"),
    }
}

fn push_location(list: &mut Vec<PathBuf>, path: PathBuf) {
    if !list.contains(&path) {
        list.push(path);
    }
}

fn set_path(target: &mut PathBuf, value: Option<PathBuf>) {
    if let Some(value) = value {
        *target = value;
    }
}
