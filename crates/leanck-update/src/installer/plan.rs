//! Pure installation planning: settings, the environment snapshot, saved
//! state, and a release inventory in; the exact preview lines and the
//! structured steps that `apply` performs out.
use super::{
    Environment, InstallArgs, Mode, Node, Probe, ReleaseInventory, SavedState, Settings,
    settings::{CompletionShell, SkillSelection, ensure_path_shell, valid_skill_name},
};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    path::{Path, PathBuf},
};

/// Content of every `.leanck-installed` ownership marker, without its newline.
pub const OWNERSHIP_MARKER: &str = "leanck installer v1";

/// A user's answer to a conflict prompt.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Answer {
    /// Leave the path untouched and skip what depends on it.
    Keep,
    /// Back the path up and replace it.
    Replace,
}

/// Conflict answers by path. The interactive flow keeps them across re-plans.
pub type Answers = BTreeMap<PathBuf, Answer>;

/// Why planning stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PlanError {
    /// The path exists or has local edits and needs `--force` or an [`Answer`].
    Conflict(PathBuf),
    /// Planning failed with this message.
    Invalid(String),
    /// These observations are needed first; see [`super::plan_observed`].
    Unobserved(Vec<Probe>),
}
impl fmt::Display for PlanError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            PlanError::Conflict(path) => write!(
                f,
                "{} already exists or has local edits; use --force to back up and replace it, or rerun interactively",
                path.display()
            ),
            PlanError::Invalid(message) => f.write_str(message),
            PlanError::Unobserved(probes) => {
                write!(
                    f,
                    "internal error: {} paths were not observed",
                    probes.len()
                )
            }
        }
    }
}

/// A failed plan, with the lines and warnings produced before it stopped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlanFailure {
    pub error: PlanError,
    pub actions: Vec<String>,
    pub warnings: Vec<String>,
}
impl fmt::Display for PlanFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.error.fmt(f)
    }
}
impl std::error::Error for PlanFailure {}
impl From<anyhow::Error> for PlanFailure {
    fn from(error: anyhow::Error) -> Self {
        PlanFailure {
            error: PlanError::Invalid(format!("{error:#}")),
            actions: vec![],
            warnings: vec![],
        }
    }
}

/// Where the configuration example comes from.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ExampleSource {
    /// A path relative to the release directory.
    Release(PathBuf),
    /// An absolute path, such as a source checkout's example.
    Path(PathBuf),
    /// A download URL, for releases that do not include the example.
    Url(String),
}

/// One file-system change, in the order `apply` performs them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// Remove an unchanged flat-layout Codex agent and its sidecars.
    MigrateCodexAgent { legacy: PathBuf },
    /// Install `plugin/bin/leanck` at `destination`.
    Executable { destination: PathBuf },
    /// Install a managed tree from `source`, relative to the release directory.
    Tree {
        source: String,
        destination: PathBuf,
    },
    /// Install the release, configured for `binary`, as the plugin bundle.
    Bundle {
        destination: PathBuf,
        binary: PathBuf,
        mcp_enabled: bool,
    },
    /// Install an agent copy from `source`, or a link to `link` with sidecars.
    Agent {
        source: String,
        destination: PathBuf,
        link: Option<PathBuf>,
    },
    ConfigExample {
        destination: PathBuf,
        source: ExampleSource,
    },
    /// Generate the executable's completion script for `shell`.
    Completions {
        shell: CompletionShell,
        destination: PathBuf,
    },
    /// Rewrite the managed PATH block of `file` to contain `line`.
    Profile { file: PathBuf, line: String },
}

/// The host whose plugin registration a plan includes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostName {
    Claude,
    Codex,
}

/// A host plugin registration, run after files are committed.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Registration {
    pub host: HostName,
    /// `CLAUDE_CONFIG_DIR` or `CODEX_HOME` for the host CLI.
    pub home: PathBuf,
    pub bundle: PathBuf,
}

/// A reviewed installation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Plan {
    pub settings: Settings,
    /// The version shown to the user: `v1.2.3` or `v1.2.3 (local source)`.
    pub version: String,
    /// Preview lines, exactly as printed and reported in JSON `actions`.
    pub actions: Vec<String>,
    /// Skipped-option warnings.
    pub warnings: Vec<String>,
    /// Paths left untouched: kept conflicts and missing shell destinations.
    pub skipped: BTreeSet<PathBuf>,
    /// Conflicting paths that will be backed up and replaced.
    pub replaced: BTreeSet<PathBuf>,
    pub steps: Vec<Step>,
    pub registrations: Vec<Registration>,
}

impl Plan {
    pub fn skips(&self, path: &Path) -> bool {
        self.skipped.contains(path)
    }
}

/// Plan a non-interactive run from parsed options.
pub fn plan(
    args: &InstallArgs,
    env: &Environment,
    saved: &SavedState,
    inventory: &ReleaseInventory,
) -> Result<Plan, PlanFailure> {
    let mut settings = Settings::resolve(args, env, saved)?;
    settings.validate_noninteractive()?;
    settings.apply_mode(settings.mode)?;
    plan_settings(&settings, env, saved, inventory, &Answers::new())
}

/// Plan resolved settings, applying conflict answers.
pub fn plan_settings(
    settings: &Settings,
    env: &Environment,
    saved: &SavedState,
    inventory: &ReleaseInventory,
    answers: &Answers,
) -> Result<Plan, PlanFailure> {
    let mut planner = Planner {
        s: settings,
        env,
        saved,
        inventory,
        answers,
        version: match &settings.from_repo {
            Some(_) => format!("{} (local source)", inventory.tag),
            None => inventory.tag.clone(),
        },
        actions: vec![],
        warnings: vec![],
        skipped: BTreeSet::new(),
        replaced: BTreeSet::new(),
        steps: vec![],
        registrations: vec![],
        misses: BTreeSet::new(),
    };
    let result = planner.run();
    let failure = |error| PlanFailure {
        error,
        actions: planner.actions.clone(),
        warnings: planner.warnings.clone(),
    };
    if !planner.misses.is_empty() {
        return Err(failure(PlanError::Unobserved(
            planner.misses.iter().cloned().collect(),
        )));
    }
    if let Err(error) = result {
        return Err(failure(error));
    }
    Ok(Plan {
        settings: settings.clone(),
        version: planner.version,
        actions: planner.actions,
        warnings: planner.warnings,
        skipped: planner.skipped,
        replaced: planner.replaced,
        steps: planner.steps,
        registrations: planner.registrations,
    })
}

/// Plan, observing whatever the snapshot lacks until planning completes.
pub fn plan_observed(
    settings: &Settings,
    env: &mut Environment,
    saved: &SavedState,
    inventory: &ReleaseInventory,
    answers: &Answers,
) -> Result<Plan, PlanFailure> {
    // An unobserved marker reads as a conflict, which stops planning at that
    // path, so a reinstall needs about one round per managed destination.
    // Every round observes new paths, so this terminates.
    loop {
        match plan_settings(settings, env, saved, inventory, answers) {
            Err(PlanFailure {
                error: PlanError::Unobserved(probes),
                ..
            }) => {
                let known = |probe: &Probe| match probe {
                    Probe::Node(path) => env.files.node(path).is_some(),
                    Probe::Tree(path) => env.files.tree(path).is_some(),
                    Probe::Profile(path) => env.files.profile(path).is_some(),
                };
                if probes.iter().all(known) {
                    return Err(anyhow::anyhow!("planning did not converge").into());
                }
                env.files.observe(&probes)?
            }
            result => return result,
        }
    }
}

type Checked = Result<(), PlanError>;

fn invalid(message: impl Into<String>) -> Checked {
    Err(PlanError::Invalid(message.into()))
}

struct Planner<'a> {
    s: &'a Settings,
    env: &'a Environment,
    saved: &'a SavedState,
    inventory: &'a ReleaseInventory,
    answers: &'a Answers,
    version: String,
    actions: Vec<String>,
    warnings: Vec<String>,
    skipped: BTreeSet<PathBuf>,
    replaced: BTreeSet<PathBuf>,
    steps: Vec<Step>,
    registrations: Vec<Registration>,
    misses: BTreeSet<Probe>,
}

impl<'a> Planner<'a> {
    fn node(&mut self, path: &Path) -> Node {
        match self.env.files.node(path) {
            Some(node) => node.clone(),
            None => {
                if !self.env.files.assumes_missing() {
                    self.misses.insert(Probe::Node(path.to_owned()));
                }
                Node::Missing
            }
        }
    }
    fn tree_digest(&mut self, path: &Path) -> Option<String> {
        match self.env.files.tree(path) {
            Some(digest) => digest.clone(),
            None => {
                if !self.env.files.assumes_missing() {
                    self.misses.insert(Probe::Tree(path.to_owned()));
                }
                None
            }
        }
    }
    fn profile_balanced(&mut self, path: &Path) -> bool {
        match self.env.files.profile(path) {
            Some(balanced) => balanced,
            None => {
                if !self.env.files.assumes_missing() {
                    self.misses.insert(Probe::Profile(path.to_owned()));
                }
                true
            }
        }
    }
    fn action(&mut self, line: impl Into<String>) {
        self.actions.push(line.into());
    }
    fn warn(&mut self, message: impl Into<String>) {
        self.warnings.push(message.into());
    }
    fn skipped(&self, path: &Path) -> bool {
        self.skipped.contains(path)
    }

    fn run(&mut self) -> Checked {
        let s = self.s;
        let inventory_names: &'a ReleaseInventory = self.inventory;
        if s.mode == Mode::Standalone
            && let SkillSelection::Names(names) = &s.selected_skills
        {
            for name in names {
                valid_skill_name(name).or_else(|e| invalid(e.to_string()))?;
                if !self.inventory.skills.contains(name) {
                    return invalid(format!(
                        "unknown skill: {name}; choose a skill included in this release, or use --skill all"
                    ));
                }
            }
        }
        if let Some(repo) = &s.from_repo {
            let checkout = self
                .node(&repo.join("crates/leanck-cli/Cargo.toml"))
                .is_file()
                & self.node(&repo.join("leanck-skills")).is_dir()
                & self.node(&repo.join("leanck-subagents")).is_dir();
            if !checkout {
                return invalid(format!(
                    "not a leanck source checkout: {}; pass --from-repo /path/to/leanck",
                    repo.display()
                ));
            }
            self.action(format!("Local source: {}", repo.display()));
        }
        let inventory = s.state_dir.join("uninstall.tsv");
        let node = self.node(&inventory);
        if node.is_symlink() {
            return invalid(format!("refusing symlink: {}", inventory.display()));
        }
        if node.present() && !matches!(node, Node::File { .. }) {
            return invalid(format!(
                "invalid uninstall inventory: {}",
                inventory.display()
            ));
        }
        if s.cli && !s.add_to_path {
            self.warn(
                "leanck will not be added to PATH; agents may not be able to execute leanck.",
            );
        }
        if !s.has_components() {
            return invalid("nothing selected; omit --no-cli or select --skills or --agents");
        }
        if s.plugin_hosts.claude && !self.env.has_command("claude") {
            return invalid("--plugin claude requires Claude Code on PATH");
        }
        if s.plugin_hosts.codex && !self.env.has_command("codex") {
            return invalid("--plugin codex requires Codex on PATH");
        }
        let bundle = s.bundle_dir();
        if s.mode == Mode::Plugin {
            // The marketplace directory must always belong to this installer, even with --force.
            let node = self.node(&bundle);
            if node.present()
                && (node.is_symlink() || !self.node(&bundle.join(".leanck-installed")).is_file())
            {
                return invalid(format!(
                    "{} already exists and is not installer-managed; choose another --data-dir",
                    bundle.display()
                ));
            }
            self.check_tree(&bundle)?;
        }
        for (enabled, dir) in self.skill_dirs() {
            if enabled {
                for skill in self.selected_skills() {
                    self.check_tree(&dir.join(skill))?;
                }
            }
        }
        let codex_agents = s.codex_agent_dir();
        if s.agent_hosts.claude {
            self.check_agents(
                &inventory_names.claude_agents,
                &s.claude_dir.join("agents"),
                None,
            )?;
        }
        if s.agent_hosts.codex {
            self.check_agents(&inventory_names.codex_agents, &codex_agents, None)?;
        }
        let data_agents = s.data_dir.join("agents");
        if s.mode == Mode::Plugin {
            self.check_tree(&data_agents)?;
            if s.plugin_hosts.codex {
                self.check_agents(
                    &inventory_names.codex_agents,
                    &codex_agents,
                    Some(&data_agents),
                )?;
            }
        }
        for location in &s.extra_skills {
            for skill in self.selected_skills() {
                self.check_tree(&location.join(skill))?;
            }
        }
        for location in &s.extra_claude_agents {
            self.check_agents(&inventory_names.claude_agents, location, None)?;
        }
        for location in &s.extra_codex_agents {
            self.check_agents(&inventory_names.codex_agents, location, None)?;
        }
        self.plan_codex_agent_migration(&bundle, &codex_agents);
        let binary = s.binary();
        if s.cli {
            let node = self.node(&binary);
            if node.is_dir() {
                return invalid(format!("{} is a directory", binary.display()));
            }
            // An executable this installer did not record is a conflict too.
            let recorded = self.saved.get("last_binary_path") == binary.to_str()
                && node.digest().is_some()
                && node.digest() == self.saved.get("last_binary_digest");
            if node.present() && !recorded {
                self.conflict(&binary)?;
            }
        }
        let completion = s.completion_path();
        if s.cli
            && let (Some(dir), Some(path)) = (s.completion_dir(), &completion)
        {
            let node = self.node(&dir);
            if node.present() {
                if !node.is_dir() {
                    return invalid(format!(
                        "completions directory is not a directory: {}; choose another path in advanced config or with --completions-dir",
                        dir.display()
                    ));
                }
            } else {
                self.warn(format!(
                    "Completions directory does not exist: {}; skipping shell completions.",
                    dir.display()
                ));
                self.skipped.insert(path.clone());
                self.action(format!(
                    "Skip shell completions (directory missing): {}",
                    dir.display()
                ));
            }
            let node = self.node(path);
            if node.present()
                && (self.saved.get("last_completions_path") != path.to_str()
                    || node.digest().is_none()
                    || node.digest() != self.saved.get("last_completions_digest"))
            {
                self.conflict(path)?;
            }
        }
        if s.cli && s.add_to_path {
            ensure_path_shell(&s.shell).or_else(|e| invalid(e.to_string()))?;
            let file = &s.path_file;
            let node = self.node(file);
            if node.is_symlink() {
                return invalid(format!("refusing symlink profile: {}", file.display()));
            }
            if node.present() {
                if !matches!(node, Node::File { .. }) {
                    return invalid(format!(
                        "shell profile is not a regular file: {}",
                        file.display()
                    ));
                }
                if !self.profile_balanced(file) {
                    return invalid(format!(
                        "unbalanced leanck PATH markers in {}; repair the block before retrying",
                        file.display()
                    ));
                }
            } else {
                self.warn(format!(
                    "Shell profile does not exist: {}; skipping PATH update. Choose an existing file in advanced config or with --path-file.",
                    file.display()
                ));
                self.skipped.insert(file.clone());
                self.action(format!(
                    "Skip PATH update (shell profile missing): {}",
                    file.display()
                ));
            }
        }

        // The preview, in the order `apply` performs each step.
        if s.cli && !self.skipped(&binary) {
            self.action(format!(
                "Would install {} ({})",
                binary.display(),
                self.version
            ));
            if self.node(&binary).present() {
                self.action("  Back up and replace existing executable");
            }
            self.steps.push(Step::Executable {
                destination: binary.clone(),
            });
        }
        for (enabled, dir) in self.skill_dirs() {
            if enabled {
                for skill in self.selected_skills() {
                    self.install_tree(&format!("plugin/skills/{skill}"), &dir.join(skill));
                }
            }
        }
        if s.agent_hosts.claude {
            self.install_agents(
                "plugin/agents",
                &inventory_names.claude_agents,
                &s.claude_dir.join("agents"),
                None,
            );
        }
        if s.agent_hosts.codex {
            self.install_agents(
                "plugin/codex/agents",
                &inventory_names.codex_agents,
                &codex_agents,
                None,
            );
        }
        if s.mode == Mode::Plugin && !self.skipped(&bundle) {
            self.action(format!("Would install {}", bundle.display()));
            self.steps.push(Step::Bundle {
                destination: bundle.clone(),
                binary: binary.clone(),
                mcp_enabled: s.mcp_enabled,
            });
            self.action(format!("Pin Codex MCP command to {}", binary.display()));
            self.action(if s.mcp_enabled {
                "Would enable plugin MCP definitions"
            } else {
                "Would disable MCP: save .mcp.disabled.json and .mcp.codex.disabled.json; remove Codex MCP manifest reference"
            });
            if s.plugin_hosts.claude {
                self.action(format!(
                    "Would run: claude plugin marketplace add \"{}\"",
                    bundle.display()
                ));
                self.action("Would run: claude plugin install leanck@leanck --scope user");
                self.registrations.push(Registration {
                    host: HostName::Claude,
                    home: s.claude_dir.clone(),
                    bundle: bundle.clone(),
                });
            }
            if s.plugin_hosts.codex {
                self.action(format!(
                    "Would run: codex plugin marketplace add \"{}\"",
                    bundle.display()
                ));
                self.action("Would run: codex plugin add leanck@leanck");
                self.registrations.push(Registration {
                    host: HostName::Codex,
                    home: s.codex_dir.clone(),
                    bundle: bundle.clone(),
                });
            }
            self.install_tree("plugin/codex/agents", &data_agents);
            if s.plugin_hosts.codex {
                self.install_agents(
                    "plugin/codex/agents",
                    &inventory_names.codex_agents,
                    &codex_agents,
                    Some(&data_agents),
                );
            }
        }
        for location in &s.extra_skills {
            for skill in self.selected_skills() {
                self.install_tree(&format!("plugin/skills/{skill}"), &location.join(skill));
            }
        }
        for location in &s.extra_claude_agents {
            self.install_agents(
                "plugin/agents",
                &inventory_names.claude_agents,
                location,
                None,
            );
        }
        for location in &s.extra_codex_agents {
            self.install_agents(
                "plugin/codex/agents",
                &inventory_names.codex_agents,
                location,
                None,
            );
        }
        let example = &s.config_example;
        if self.node(example).present() {
            self.action(format!(
                "Preserve existing configuration example: {}",
                example.display()
            ));
        } else {
            self.action(format!(
                "Would install configuration example: {}",
                example.display()
            ));
            let source = if self.inventory.config_example {
                ExampleSource::Release("docs/leanck.example.toml".into())
            } else if let Some(repo) = &s.from_repo {
                ExampleSource::Path(repo.join("docs/leanck.example.toml"))
            } else {
                ExampleSource::Url(format!(
                    "{}/download/{}/leanck.example.toml",
                    s.release_url, self.inventory.tag
                ))
            };
            self.steps.push(Step::ConfigExample {
                destination: example.clone(),
                source,
            });
        }
        if s.cli
            && let Some(path) = &completion
            && !self.skipped(path)
        {
            self.action(format!(
                "Would install {} completions: {}",
                s.completions.as_str(),
                path.display()
            ));
            self.steps.push(Step::Completions {
                shell: s.completions,
                destination: path.clone(),
            });
        }
        if s.cli && s.add_to_path && !self.skipped(&s.path_file) {
            let line = s.profile_line();
            self.action(format!(
                "Would update shell profile: {}",
                s.path_file.display()
            ));
            self.action(format!("  {line}"));
            self.steps.push(Step::Profile {
                file: s.path_file.clone(),
                line,
            });
        }
        self.action(format!(
            "Installer choices and replacement backups: {}",
            s.state_dir.display()
        ));
        Ok(())
    }

    fn skill_dirs(&self) -> [(bool, PathBuf); 2] {
        [
            (self.s.skill_hosts.claude, self.s.claude_dir.join("skills")),
            (self.s.skill_hosts.codex, self.s.codex_skills.clone()),
        ]
    }

    fn selected_skills(&self) -> Vec<&'a str> {
        let inventory: &'a ReleaseInventory = self.inventory;
        inventory
            .skills
            .iter()
            .filter(|skill| self.s.skill_selected(skill))
            .map(String::as_str)
            .collect()
    }

    fn conflict(&mut self, path: &Path) -> Checked {
        match (self.s.force, self.answers.get(path)) {
            (true, _) | (false, Some(Answer::Replace)) => {
                self.action(format!("Back up and replace conflict: {}", path.display()));
                self.replaced.insert(path.to_owned());
                Ok(())
            }
            (false, Some(Answer::Keep)) => {
                self.skipped.insert(path.to_owned());
                self.action(format!("Preserve {}", path.display()));
                Ok(())
            }
            (false, None) => Err(PlanError::Conflict(path.to_owned())),
        }
    }

    /// A managed, unchanged tree is replaced silently; anything else conflicts.
    fn check_tree(&mut self, path: &Path) -> Checked {
        let node = self.node(path);
        if !node.present() {
            return Ok(());
        }
        let marker = self.node(&path.join(".leanck-installed"));
        if node.is_symlink() || !marker.is_file() || marker.text() != Some(OWNERSHIP_MARKER) {
            return self.conflict(path);
        }
        let recorded = self.node(&path.join(".leanck-content-sha256"));
        let actual = self.tree_digest(path);
        if !recorded.is_file() || actual.is_none() || recorded.text() != actual.as_deref() {
            return self.conflict(path);
        }
        Ok(())
    }

    fn check_agents(&mut self, names: &[String], dir: &Path, link: Option<&Path>) -> Checked {
        for name in names {
            let dest = dir.join(name);
            let node = self.node(&dest);
            if node.is_dir() {
                return invalid(format!("{} is a directory", dest.display()));
            }
            let marker_path = sidecar(&dest, "leanck-installed");
            let marker = self.node(&marker_path);
            if node.present() {
                let expected_link = link.map(|dir| dir.join(name));
                if (node.is_symlink() && expected_link.as_deref() != node.link_target())
                    || !marker.is_file()
                    || marker.text() != Some(OWNERSHIP_MARKER)
                {
                    self.conflict(&dest)?;
                } else {
                    let digest = self.node(&sidecar(&dest, "leanck-sha256"));
                    if !node.is_file()
                        || !digest.is_file()
                        || node.digest().is_none()
                        || digest.text() != node.digest()
                    {
                        self.conflict(&dest)?;
                    }
                }
            }
            if marker.is_symlink() {
                return invalid(format!("refusing symlink: {}", marker_path.display()));
            }
            if marker.exists() && !marker.is_file() {
                return invalid(format!(
                    "invalid ownership marker: {}",
                    marker_path.display()
                ));
            }
        }
        Ok(())
    }

    /// Move unchanged agents from the old flat `CODEX_DIR/agents` layout.
    fn plan_codex_agent_migration(&mut self, bundle: &Path, destination: &Path) {
        let s = self.s;
        let installs_codex_agents = s.agent_hosts.codex
            || (s.mode == Mode::Plugin && s.plugin_hosts.codex && !self.skipped(bundle));
        if !installs_codex_agents {
            return;
        }
        let inventory_names: &'a ReleaseInventory = self.inventory;
        for name in &inventory_names.codex_agents {
            let legacy = s.codex_dir.join("agents").join(name);
            if self.skipped(&destination.join(name)) || legacy == destination.join(name) {
                continue;
            }
            if self.unchanged_managed_agent(&legacy) {
                self.action(format!(
                    "Move managed Codex agent {} into {}/",
                    legacy.display(),
                    destination.display()
                ));
                self.steps.push(Step::MigrateCodexAgent { legacy });
            }
        }
    }

    fn unchanged_managed_agent(&mut self, path: &Path) -> bool {
        let node = self.node(path);
        let marker = self.node(&sidecar(path, "leanck-installed"));
        let digest = self.node(&sidecar(path, "leanck-sha256"));
        node.is_file()
            && marker.is_file()
            && !marker.is_symlink()
            && digest.is_file()
            && !digest.is_symlink()
            && marker.text() == Some(OWNERSHIP_MARKER)
            && node.digest().is_some()
            && digest.text() == node.digest()
    }

    fn install_tree(&mut self, source: &str, destination: &Path) {
        if self.skipped(destination) {
            return;
        }
        self.action(format!("Would install {}", destination.display()));
        self.steps.push(Step::Tree {
            source: source.to_owned(),
            destination: destination.to_owned(),
        });
    }

    fn install_agents(&mut self, source: &str, names: &[String], dir: &Path, link: Option<&Path>) {
        for name in names {
            let destination = dir.join(name);
            if self.skipped(&destination) {
                continue;
            }
            self.action(format!("Would install {}", destination.display()));
            self.steps.push(Step::Agent {
                source: format!("{source}/{name}"),
                destination,
                link: link.map(|dir| dir.join(name)),
            });
        }
    }
}

/// `AGENT.leanck-installed` and `AGENT.leanck-sha256` sit beside each agent.
pub fn sidecar(agent: &Path, suffix: &str) -> PathBuf {
    let mut name = agent.as_os_str().to_owned();
    name.push(".");
    name.push(suffix);
    PathBuf::from(name)
}
