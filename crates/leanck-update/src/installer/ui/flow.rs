//! The interactive dialog as a pure question-and-answer state machine.
//!
//! [`Flow`] holds the settings being configured and the current
//! [`Question`]. Front ends show the question, pass the reply back with
//! [`Flow::answer`] (text, or a menu's value word) or [`Flow::choose`] (a menu
//! index), and act on the returned [`Effect`]. Planning is the front end's
//! job: [`Effect::Plan`] asks it to plan [`Flow::settings`] with
//! [`Flow::answers`] and to report the result through [`Flow::planned`].
//! Nothing here reads the terminal or the filesystem.
use super::super::{
    Answer, Answers, CompletionShell, Hosts, Mode, Plan, PlanError, PlanFailure, ReleaseInventory,
    Settings, SkillSelection, config_summary, ensure_path_shell,
};
use crate::paths::{absolute, display, valid_text};
use std::{
    borrow::Cow,
    collections::VecDeque,
    path::{Path, PathBuf},
};

/// The fixed description shown with each skill.
pub fn skill_description(name: &str) -> &'static str {
    match name {
        "leanck" => "leanck command reference",
        "lean-proving" => "opinionated workflow for proving theorems",
        "lean-style" => "opinionated workflow for code and documentation style cleanup",
        _ => "Lean development skill",
    }
}

/// The confirmation menu's title for `mode`.
pub fn confirmation_title(mode: Mode) -> &'static str {
    match mode {
        Mode::Plugin => "Installing `leanck` as a plugin. Continue?",
        Mode::Standalone => "Installing `leanck` as standalone. Continue?",
    }
}

/// One menu option: the word line mode accepts, and the label shown.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub value: Cow<'static, str>,
    pub label: Cow<'static, str>,
}
fn choice(value: impl Into<Cow<'static, str>>, label: impl Into<Cow<'static, str>>) -> Choice {
    Choice {
        value: value.into(),
        label: label.into(),
    }
}

/// What the user is being asked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Question {
    /// Pick one option. Terminals use the arrow keys; line mode accepts a
    /// value word, and an empty reply takes `default`.
    Menu {
        title: String,
        options: Vec<Choice>,
        default: usize,
    },
    /// Pick any number of options; an empty line takes `selected`.
    Checklist {
        title: String,
        note: Option<String>,
        options: Vec<Choice>,
        selected: Vec<bool>,
    },
    /// A line of text. An empty reply takes `default`.
    Prompt { label: String, default: String },
    /// The advanced-config form (terminal UI only); see [`Flow::form_closed`].
    Form,
    /// Waiting for the plan requested by [`Effect::Plan`].
    Planning,
    /// The dialog has ended.
    Done,
}

/// Messages for the front end, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Output {
    /// A prompt and the reply it received. Line mode echoes replies itself
    /// and ignores these; the terminal UI keeps them as a transcript.
    Answer { prompt: String, reply: String },
    /// `[WARN] MESSAGE`
    Warn(String),
    /// ` [ERR]: MESSAGE`
    Error(String),
    /// Unprefixed text, such as the configuration summary or the plan.
    Text(String),
    /// A plan rendered with the front end's style.
    Plan(Box<Plan>),
}

/// What the front end must do next.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Effect {
    /// Show the next question.
    None,
    /// Plan [`Flow::settings`] with [`Flow::answers`], then call [`Flow::planned`].
    Plan,
    /// The user approved this plan.
    Confirmed(Box<Plan>),
    /// The user chose Cancel at the confirmation menu: nothing is installed (exit 0).
    Cancelled,
    /// Ctrl-C, Ctrl-D, end of input, or `cancel` at a conflict prompt (exit 130).
    Interrupted,
    /// Configuration cannot continue (exit 1).
    Failed(String),
}

/// Additional-location lists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Skills,
    ClaudeAgents,
    CodexAgents,
}
impl Group {
    pub fn list(self, settings: &Settings) -> &Vec<PathBuf> {
        match self {
            Group::Skills => &settings.extra_skills,
            Group::ClaudeAgents => &settings.extra_claude_agents,
            Group::CodexAgents => &settings.extra_codex_agents,
        }
    }
    pub fn list_mut(self, settings: &mut Settings) -> &mut Vec<PathBuf> {
        match self {
            Group::Skills => &mut settings.extra_skills,
            Group::ClaudeAgents => &mut settings.extra_claude_agents,
            Group::CodexAgents => &mut settings.extra_codex_agents,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum Ask {
    Mode,
    Completions,
    CompletionsDir,
    AddToPath,
    /// The dialog's fallback when `$SHELL` cannot take a PATH block.
    PathShell,
    /// The plain editor's shell choice.
    Shell,
    Profile,
    Cli,
    BinDir(&'static str),
    SkillHosts(&'static str),
    Skills,
    AgentHosts(&'static str),
    PluginHosts,
    ClaudeDir,
    ClaudeSkillsDir,
    ClaudeAgentsDir,
    CodexSkills,
    CodexDir,
    CodexAgentsDir,
    DataDir,
    Mcp,
    Extra(Group),
    Conflict(PathBuf),
    Confirm,
    Editor,
    Discard,
    Form,
    Planning,
    Done,
}

/// A queued prompt is asked only if its condition holds when it comes up.
type When = fn(&Settings) -> bool;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    /// The mode menu and the plugin or standalone prompts.
    Configure,
    /// Planning, conflicts, and the confirmation menu.
    Review,
    /// The plain (line-mode) advanced editor.
    Editor,
}

/// The interactive configuration dialog.
#[derive(Clone, Debug)]
pub struct Flow {
    settings: Settings,
    skills: Vec<String>,
    cwd: PathBuf,
    /// Line mode: "Edit advanced config..." opens the plain editor, not the form.
    plain: bool,
    answers: Answers,
    plan: Option<Plan>,
    ask: Ask,
    queue: VecDeque<(Ask, When)>,
    phase: Phase,
    /// Settings when the plain editor opened.
    editor_initial: Option<Settings>,
    /// Settings before the advanced edit that the pending plan checks.
    before_edit: Option<Settings>,
    output: Vec<Output>,
    serial: u64,
}

fn always(_: &Settings) -> bool {
    true
}
fn yes_no(value: bool) -> &'static str {
    if value { "yes" } else { "no" }
}
fn parse_yes_no(reply: &str) -> Option<bool> {
    match reply {
        "y" | "Y" | "yes" | "YES" => Some(true),
        "n" | "N" | "no" | "NO" => Some(false),
        _ => None,
    }
}
/// `a`, `a or b`, `a, b, or c`.
fn or_list(words: &[&str]) -> String {
    match words {
        [] => String::new(),
        [one] => (*one).to_owned(),
        [a, b] => format!("{a} or {b}"),
        [rest @ .., last] => format!("{}, or {last}", rest.join(", ")),
    }
}

impl Flow {
    /// Start at the mode menu. `settings` come from [`Settings::resolve`];
    /// the chosen mode is applied here.
    pub fn new(
        settings: Settings,
        inventory: &ReleaseInventory,
        cwd: PathBuf,
        plain: bool,
    ) -> Self {
        Self {
            settings,
            // The command reference comes first; the workflows follow.
            skills: {
                let mut skills = inventory.skills.clone();
                skills.sort_by_key(|name| name != "leanck");
                skills
            },
            cwd,
            plain,
            answers: Answers::new(),
            plan: None,
            ask: Ask::Mode,
            queue: VecDeque::new(),
            phase: Phase::Configure,
            editor_initial: None,
            before_edit: None,
            output: vec![],
            serial: 0,
        }
    }

    /// The settings as configured so far.
    pub fn settings(&self) -> &Settings {
        &self.settings
    }
    /// Conflict answers, kept across re-plans.
    pub fn answers(&self) -> &Answers {
        &self.answers
    }
    /// The last successful plan.
    pub fn plan(&self) -> Option<&Plan> {
        self.plan.as_ref()
    }
    /// The release's skills, in inventory order.
    pub fn skills(&self) -> &[String] {
        &self.skills
    }
    /// The working directory relative paths resolve against.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }
    /// Changes whenever a question is asked again or replaced, so front
    /// ends know to reset their input state.
    pub fn serial(&self) -> u64 {
        self.serial
    }
    /// Messages produced since the last call.
    pub fn take_output(&mut self) -> Vec<Output> {
        std::mem::take(&mut self.output)
    }

    /// The current question.
    pub fn question(&self) -> Question {
        let s = &self.settings;
        let menu = |title: &str, options: &[Choice], default: usize| Question::Menu {
            title: title.to_owned(),
            options: options.to_vec(),
            default,
        };
        let prompt = |label: String, default: String| Question::Prompt { label, default };
        let path = |path: &Path| display(path, &s.home);
        match &self.ask {
            Ask::Mode => menu(
                "Install as plugin or standalone?",
                &[
                    choice("plugin", "Plugin (recommended)"),
                    choice("standalone", "Standalone"),
                ],
                usize::from(s.mode == Mode::Standalone),
            ),
            Ask::Confirm => menu(
                confirmation_title(s.mode),
                &[
                    choice("cancel", "Cancel"),
                    choice("continue", "Continue"),
                    choice("edit", "Edit advanced config..."),
                ],
                0,
            ),
            Ask::Editor => match s.mode {
                Mode::Plugin => menu(
                    "Edit advanced config",
                    &[
                        choice("data", "Plugin data directory"),
                        choice("binary", "Executable directory"),
                        choice("skills", "Install skills"),
                        choice("agents", "Install agents"),
                        choice("completions", "Shell completions"),
                        choice("path", "Add leanck to PATH"),
                        choice("mcp", "Enable MCP"),
                        choice("hosts", "Register plugins"),
                        choice("save", "Save changes"),
                        choice("cancel", "Cancel"),
                    ],
                    0,
                ),
                Mode::Standalone => menu(
                    "Edit advanced config",
                    &[
                        choice("binary", "Executable"),
                        choice("skills", "Install skills"),
                        choice("agents", "Install agents"),
                        choice("completions", "Shell completions"),
                        choice("path", "Add leanck to PATH"),
                        choice("save", "Save changes"),
                        choice("cancel", "Cancel"),
                    ],
                    0,
                ),
            },
            Ask::Form => Question::Form,
            Ask::Planning => Question::Planning,
            Ask::Done => Question::Done,
            Ask::Completions => prompt(
                "Shell completions (none / bash / zsh / fish / powershell / elvish)".into(),
                s.completions.as_str().into(),
            ),
            Ask::CompletionsDir => prompt(
                "Completions directory".into(),
                s.completion_dir().map(|d| path(&d)).unwrap_or_default(),
            ),
            Ask::AddToPath => prompt("Add leanck to PATH?".into(), yes_no(s.add_to_path).into()),
            Ask::PathShell => prompt("Shell for PATH (fish / bash / zsh)".into(), "bash".into()),
            Ask::Shell => prompt("Shell (fish / bash / zsh)".into(), s.shell.clone()),
            Ask::Profile => prompt("Shell profile to update".into(), path(&s.path_file)),
            Ask::Cli => prompt("Install the CLI?".into(), yes_no(s.cli).into()),
            Ask::BinDir(label) => prompt((*label).into(), path(&s.bin_dir)),
            Ask::Skills => Question::Checklist {
                title: "Install skills:".into(),
                note: None,
                options: self.skills.iter().map(|name| choice(
                    name.clone(), format!("{name} ({})", skill_description(name)),
                )).collect(),
                selected: self.skills.iter().map(|name| {
                    if s.skills_chosen { s.selected_skills.includes(name) } else { name == "leanck" }
                }).collect(),
            },
            Ask::SkillHosts(label) => host_checklist(label, if s.skill_hosts_chosen {
                s.skill_hosts
            } else { s.detected_hosts }, None),
            Ask::AgentHosts(label) => host_checklist(label, if s.agent_hosts_chosen {
                s.agent_hosts
            } else { s.detected_hosts },
                self.skills.iter().any(|name| matches!(name.as_str(), "lean-proving" | "lean-style") && s.selected_skills.includes(name))
                    .then(|| "Note: Custom agents are strongly recommended if `lean-proving` or `lean-style` are installed".into())),
            Ask::PluginHosts => host_checklist("Register plugins for:", s.plugin_hosts, None),
            Ask::ClaudeDir => prompt("Claude configuration directory".into(), path(&s.claude_dir)),
            Ask::ClaudeSkillsDir => {
                prompt("Claude skills directory".into(), path(&s.claude_skills()))
            }
            Ask::ClaudeAgentsDir => {
                prompt("Claude agents directory".into(), path(&s.claude_agents()))
            }
            Ask::CodexSkills => prompt("Codex skills directory".into(), path(&s.codex_skills)),
            Ask::CodexDir => prompt("Codex configuration directory".into(), path(&s.codex_dir)),
            Ask::CodexAgentsDir => {
                prompt("Codex agents directory".into(), path(&s.codex_agent_dir()))
            }
            Ask::DataDir => prompt("Plugin data directory".into(), path(&s.data_dir)),
            Ask::Mcp => prompt("Enable MCP?".into(), yes_no(s.mcp_enabled).into()),
            Ask::Extra(group) => prompt(
                format!(
                    "Additional {} directory (keep / none / path)",
                    match group {
                        Group::Skills => "skills",
                        Group::ClaudeAgents => "Claude agents",
                        Group::CodexAgents => "Codex agents",
                    }
                ),
                "keep".into(),
            ),
            Ask::Conflict(_) => prompt(
                "Keep it, back up and replace it, or cancel? (keep / replace / cancel)".into(),
                "keep".into(),
            ),
            Ask::Discard => prompt("Discard advanced config changes?".into(), "no".into()),
        }
    }

    /// Answer the current question with a line of text. Menus accept their
    /// value words. Invalid replies leave the question in place with a warning.
    pub fn answer(&mut self, reply: &str) -> Effect {
        self.serial += 1;
        match self.question() {
            Question::Menu {
                title,
                options,
                default,
            } => {
                let value = if reply.is_empty() {
                    options[default].value.as_ref()
                } else {
                    reply
                };
                match options.iter().position(|o| o.value == value) {
                    Some(index) => self.select(&title, &options, index),
                    None => {
                        let values: Vec<&str> = options.iter().map(|o| o.value.as_ref()).collect();
                        self.warn(format!("Enter 1-{} or {}.", values.len(), or_list(&values)));
                        Effect::None
                    }
                }
            }
            Question::Checklist {
                options, selected, ..
            } => {
                let selected = if reply.is_empty() {
                    selected
                } else if reply == "none" {
                    vec![false; options.len()]
                } else {
                    let mut selected = vec![false; options.len()];
                    let tokens: Vec<_> = reply
                        .split(|c: char| c == ',' || c.is_whitespace())
                        .filter(|token| !token.is_empty())
                        .collect();
                    if tokens.is_empty() {
                        self.warn("Enter numbers or names separated by spaces, or none.");
                        return Effect::None;
                    }
                    for token in tokens {
                        let index = token
                            .parse::<usize>()
                            .ok()
                            .filter(|n| (1..=options.len()).contains(n))
                            .map(|n| n - 1)
                            .or_else(|| options.iter().position(|o| o.value == token));
                        let Some(index) = index else {
                            self.warn("Enter numbers or names separated by spaces, or none.");
                            return Effect::None;
                        };
                        selected[index] = true;
                    }
                    selected
                };
                self.check(selected)
            }
            Question::Prompt { label, default } => {
                self.output.push(Output::Answer {
                    prompt: format!("{label} [{default}]:"),
                    reply: reply.to_owned(),
                });
                if let Err(error) = valid_text(reply) {
                    self.warn(error.to_string());
                    return Effect::None;
                }
                let reply = if reply.is_empty() {
                    default.as_str()
                } else {
                    reply
                };
                self.accept(reply)
            }
            _ => Effect::None,
        }
    }

    /// Choose a menu option by index.
    pub fn choose(&mut self, index: usize) -> Effect {
        self.serial += 1;
        match self.question() {
            Question::Menu { title, options, .. } if index < options.len() => {
                self.select(&title, &options, index)
            }
            _ => Effect::None,
        }
    }

    /// Confirm a checklist; malformed selections leave it unchanged.
    pub fn check(&mut self, selected: Vec<bool>) -> Effect {
        let Question::Checklist { title, options, .. } = self.question() else {
            return Effect::None;
        };
        if selected.len() != options.len() {
            return Effect::None;
        }
        self.serial += 1;
        let labels: Vec<_> = options
            .iter()
            .zip(&selected)
            .filter(|(_, checked)| **checked)
            // The answer line names skills without their descriptions.
            .map(|(o, _)| o.label.split(" (").next().unwrap_or_default())
            .collect();
        self.output.push(Output::Answer {
            prompt: title,
            reply: if labels.is_empty() {
                "none".into()
            } else {
                labels.join(", ")
            },
        });
        let hosts = || Hosts {
            claude: selected[0],
            codex: selected[1],
        };
        match self.ask {
            Ask::Skills => {
                self.settings.selected_skills = selection(&self.skills, &selected);
                self.settings.skills_chosen = true;
                if !selected.iter().any(|checked| *checked) {
                    self.settings.skill_hosts = Hosts::NONE;
                    self.settings.skill_hosts_chosen = true;
                }
            }
            Ask::SkillHosts(_) => {
                self.settings.skill_hosts = hosts();
                self.settings.skill_hosts_chosen = true;
            }
            Ask::AgentHosts(_) => {
                self.settings.agent_hosts = hosts();
                self.settings.agent_hosts_chosen = true;
            }
            Ask::PluginHosts => self.settings.plugin_hosts = hosts(),
            _ => return Effect::None,
        }
        self.next()
    }

    /// Stop: Ctrl-C, Ctrl-D, or the end of input.
    pub fn interrupt(&mut self) -> Effect {
        self.serial += 1;
        self.ask = Ask::Done;
        Effect::Interrupted
    }

    /// Report the result of the plan [`Effect::Plan`] requested.
    pub fn planned(&mut self, result: Result<Plan, PlanFailure>) -> Effect {
        self.serial += 1;
        match result {
            Ok(plan) => {
                self.before_edit = None;
                // Show what Continue approves; an unchanged plan stays above.
                self.output.push(Output::Plan(Box::new(plan.clone())));
                self.plan = Some(plan);
                self.enter_confirm()
            }
            Err(PlanFailure {
                error: PlanError::Conflict(path),
                ..
            }) => {
                self.warn(format!(
                    "{} already exists or has local edits.",
                    path.display()
                ));
                self.ask = Ask::Conflict(path);
                Effect::None
            }
            Err(failure) => {
                let message = failure.error.to_string();
                match self.before_edit.take() {
                    // An edit made the plan fail: keep the last good configuration.
                    Some(previous) if self.plan.is_some() => {
                        self.settings = previous;
                        self.error(format!(
                            "{message}; the advanced config changes were not applied"
                        ));
                        self.enter_confirm()
                    }
                    _ => {
                        for warning in failure.warnings {
                            self.warn(warning);
                        }
                        self.ask = Ask::Done;
                        Effect::Failed(message)
                    }
                }
            }
        }
    }

    /// Close the advanced-config form with its saved settings, or `None`
    /// when the form was cancelled or its changes discarded.
    pub fn form_closed(&mut self, saved: Option<Settings>) -> Effect {
        self.serial += 1;
        match saved {
            Some(settings) if settings != self.settings => {
                self.before_edit = Some(std::mem::replace(&mut self.settings, settings));
                self.replan_after_edit()
            }
            _ => self.return_to_confirm(),
        }
    }

    fn replan_after_edit(&mut self) -> Effect {
        self.request_plan()
    }

    fn return_to_confirm(&mut self) -> Effect {
        self.enter_confirm()
    }

    fn warn(&mut self, message: impl Into<String>) {
        self.output.push(Output::Warn(message.into()));
    }
    fn error(&mut self, message: impl Into<String>) {
        self.output.push(Output::Error(message.into()));
    }

    fn select(&mut self, title: &str, options: &[Choice], index: usize) -> Effect {
        let option = &options[index];
        self.output.push(Output::Answer {
            prompt: title.to_owned(),
            reply: option.label.to_string(),
        });
        match (&self.ask, option.value.as_ref()) {
            (Ask::Mode, "plugin") => self.start(Mode::Plugin),
            (Ask::Mode, _) => self.start(Mode::Standalone),
            (Ask::Confirm, "continue") => {
                self.ask = Ask::Done;
                match &self.plan {
                    Some(plan) => Effect::Confirmed(Box::new(plan.clone())),
                    None => Effect::Failed("no installation plan to confirm".into()),
                }
            }
            (Ask::Confirm, "edit") => {
                if self.plain {
                    self.phase = Phase::Editor;
                    self.editor_initial = Some(self.settings.clone());
                    self.enter_editor()
                } else {
                    self.ask = Ask::Form;
                    Effect::None
                }
            }
            (Ask::Confirm, _) => {
                self.ask = Ask::Done;
                Effect::Cancelled
            }
            (Ask::Editor, value) => self.editor_choice(value),
            _ => Effect::None,
        }
    }

    fn start(&mut self, mode: Mode) -> Effect {
        // Standalone mode reports an empty selection, which the standalone
        // prompts are about to fill, after applying its other rules.
        if let Err(error) = self.settings.apply_mode(mode)
            && mode == Mode::Plugin
        {
            self.ask = Ask::Done;
            return Effect::Failed(format!("{error:#}"));
        }
        self.phase = Phase::Configure;
        self.queue.clear();
        match mode {
            Mode::Plugin => self.queue_path_dialog(),
            Mode::Standalone => self.queue_standalone(),
        }
        self.next()
    }

    fn push(&mut self, ask: Ask, when: When) {
        self.queue.push_back((ask, when));
    }

    /// Completions, then the PATH dialog; the same in both modes.
    fn queue_path_dialog(&mut self) {
        self.push(Ask::Completions, |s| s.cli);
        self.push(Ask::AddToPath, |s| s.cli && !s.bin_on_path());
        self.push(Ask::PathShell, |s| {
            s.cli && !s.bin_on_path() && s.add_to_path && ensure_path_shell(&s.shell).is_err()
        });
    }

    fn queue_standalone(&mut self) {
        self.push(Ask::Cli, always);
        self.push(Ask::BinDir("Install location"), |s| s.cli);
        self.push(Ask::Skills, always);
        self.push(Ask::SkillHosts("Install skills for:"), |s| {
            s.selected_skills != SkillSelection::None
        });
        self.push(Ask::AgentHosts("Install custom agents for:"), always);
        self.push(Ask::ClaudeSkillsDir, |s| s.skill_hosts.claude);
        self.push(Ask::CodexSkills, |s| s.skill_hosts.codex);
        self.push(Ask::ClaudeAgentsDir, |s| s.agent_hosts.claude);
        self.push(Ask::CodexAgentsDir, |s| s.agent_hosts.codex);
        self.queue_path_dialog();
    }

    /// Ask the next queued question whose condition holds.
    fn next(&mut self) -> Effect {
        while let Some((ask, when)) = self.queue.pop_front() {
            if ask == Ask::AddToPath {
                self.settings.apply_path_default();
            }
            if when(&self.settings) {
                self.ask = ask;
                return Effect::None;
            }
        }
        match self.phase {
            Phase::Configure => {
                if self.settings.mode == Mode::Standalone && !self.settings.has_components() {
                    self.warn("Select an executable, skills, or custom agents before continuing.");
                    self.queue_standalone();
                    return self.next();
                }
                self.phase = Phase::Review;
                self.request_plan()
            }
            Phase::Editor => self.enter_editor(),
            Phase::Review => self.enter_confirm(),
        }
    }

    fn request_plan(&mut self) -> Effect {
        self.phase = Phase::Review;
        self.ask = Ask::Planning;
        Effect::Plan
    }

    fn enter_confirm(&mut self) -> Effect {
        self.phase = Phase::Review;
        let warnings = self
            .plan
            .as_ref()
            .map(|plan| plan.warnings.clone())
            .unwrap_or_default();
        for warning in warnings {
            self.warn(warning);
        }
        self.ask = Ask::Confirm;
        Effect::None
    }

    fn enter_editor(&mut self) -> Effect {
        self.output
            .push(Output::Text(config_summary(&self.settings)));
        self.ask = Ask::Editor;
        Effect::None
    }

    fn editor_choice(&mut self, value: &str) -> Effect {
        let standalone = self.settings.mode == Mode::Standalone;
        match value {
            "data" => self.push(Ask::DataDir, always),
            "binary" => {
                if standalone {
                    self.push(Ask::Cli, always);
                }
                self.push(Ask::BinDir("Executable directory"), |s| s.cli);
            }
            "hosts" => {
                self.push(Ask::PluginHosts, always);
                self.push(Ask::ClaudeDir, |s| s.plugin_hosts.claude);
                self.push(Ask::CodexDir, |s| s.plugin_hosts.codex);
            }
            "skills" => {
                if standalone {
                    self.push(Ask::Skills, always);
                    self.push(Ask::SkillHosts("Install skills for:"), |s| {
                        s.selected_skills != SkillSelection::None
                    });
                    self.push(Ask::ClaudeSkillsDir, |s| s.skill_hosts.claude);
                    self.push(Ask::CodexSkills, |s| s.skill_hosts.codex);
                }
                self.push(Ask::Extra(Group::Skills), always);
            }
            "agents" => {
                if standalone {
                    self.push(Ask::AgentHosts("Install agents for:"), always);
                    self.push(Ask::ClaudeAgentsDir, |s| s.agent_hosts.claude);
                }
                self.push(Ask::CodexAgentsDir, |s| {
                    s.agent_hosts.codex || (s.mode == Mode::Plugin && s.plugin_hosts.codex)
                });
                self.push(Ask::Extra(Group::ClaudeAgents), always);
                self.push(Ask::Extra(Group::CodexAgents), always);
            }
            "completions" => {
                self.push(Ask::Completions, always);
                self.push(Ask::CompletionsDir, |s| {
                    s.completions != CompletionShell::None
                });
            }
            "path" => {
                self.push(Ask::AddToPath, always);
                self.push(Ask::Shell, |s| s.add_to_path);
                self.push(Ask::Profile, |s| s.add_to_path);
            }
            "mcp" => self.push(Ask::Mcp, always),
            "save" => {
                let initial = self
                    .editor_initial
                    .take()
                    .unwrap_or_else(|| self.settings.clone());
                return if initial != self.settings {
                    self.before_edit = Some(initial);
                    self.replan_after_edit()
                } else {
                    self.return_to_confirm()
                };
            }
            _ => {
                if self.editor_initial.as_ref() != Some(&self.settings) {
                    self.ask = Ask::Discard;
                    return Effect::None;
                }
                self.editor_initial = None;
                return self.return_to_confirm();
            }
        }
        self.next()
    }

    fn path(&mut self, reply: &str) -> Option<PathBuf> {
        match absolute(reply, &self.settings.home, &self.cwd) {
            Ok(path) => Some(path),
            Err(error) => {
                self.warn(error.to_string());
                None
            }
        }
    }

    fn boolean(&mut self, reply: &str) -> Option<bool> {
        let value = parse_yes_no(reply);
        if value.is_none() {
            self.warn("Enter yes or no.");
        }
        value
    }

    fn shell(&mut self, reply: &str) -> Option<String> {
        if ensure_path_shell(reply).is_ok() {
            Some(reply.to_owned())
        } else {
            self.warn("Choose fish, bash, or zsh.");
            None
        }
    }

    /// Apply a reply to the current prompt; `None` from a parser re-asks.
    fn accept(&mut self, reply: &str) -> Effect {
        macro_rules! parse {
            ($value:expr) => {
                match $value {
                    Some(value) => value,
                    None => return Effect::None,
                }
            };
        }
        match self.ask.clone() {
            Ask::Completions => match CompletionShell::parse(reply) {
                Some(shell) => self.settings.completions = shell,
                None => {
                    self.warn("Choose none, bash, zsh, fish, powershell, or elvish.");
                    return Effect::None;
                }
            },
            Ask::CompletionsDir => {
                let dir = parse!(self.path(reply));
                let s = &mut self.settings;
                s.completions_dir =
                    (s.default_completion_dir(s.completions).as_ref() != Some(&dir)).then_some(dir);
            }
            Ask::AddToPath => {
                self.settings.add_to_path = parse!(self.boolean(reply));
                self.settings.path_explicit = true;
            }
            Ask::PathShell => {
                let shell = parse!(self.shell(reply));
                let s = &mut self.settings;
                if s.path_file_explicit {
                    s.shell = shell;
                } else {
                    s.path_file = s.default_profile(&shell);
                    s.shell = shell;
                }
            }
            Ask::Shell => {
                let shell = parse!(self.shell(reply));
                let s = &mut self.settings;
                if s.shell != shell {
                    s.path_file = s.default_profile(&shell);
                    s.shell = shell;
                }
            }
            Ask::Profile => self.settings.path_file = parse!(self.path(reply)),
            Ask::Cli => self.settings.cli = parse!(self.boolean(reply)),
            Ask::BinDir(_) => self.settings.bin_dir = parse!(self.path(reply)),
            Ask::ClaudeDir => self.settings.claude_dir = parse!(self.path(reply)),
            Ask::ClaudeSkillsDir => {
                let dir = parse!(self.path(reply));
                let s = &mut self.settings;
                s.claude_skills_dir = (dir != s.claude_dir.join("skills")).then_some(dir);
            }
            Ask::ClaudeAgentsDir => {
                let dir = parse!(self.path(reply));
                let s = &mut self.settings;
                s.claude_agents_dir = (dir != s.claude_dir.join("agents/leanck")).then_some(dir);
            }
            Ask::CodexSkills => self.settings.codex_skills = parse!(self.path(reply)),
            Ask::CodexDir => self.settings.codex_dir = parse!(self.path(reply)),
            Ask::CodexAgentsDir => {
                let dir = parse!(self.path(reply));
                let s = &mut self.settings;
                s.codex_agents_dir = (dir != s.codex_dir.join("agents/leanck")).then_some(dir);
            }
            Ask::DataDir => {
                let dir = parse!(self.path(reply));
                self.settings.set_data_dir(dir);
            }
            Ask::Mcp => self.settings.mcp_enabled = parse!(self.boolean(reply)),
            Ask::Extra(group) => match reply {
                "keep" => {}
                "none" => group.list_mut(&mut self.settings).clear(),
                _ => {
                    let dir = parse!(self.path(reply));
                    let list = group.list_mut(&mut self.settings);
                    if !list.contains(&dir) {
                        list.push(dir);
                    }
                }
            },
            Ask::Conflict(path) => {
                let answer = match reply {
                    "keep" => Answer::Keep,
                    "replace" => Answer::Replace,
                    "cancel" => return self.interrupt(),
                    // Anything else asks again, without a warning.
                    _ => return Effect::None,
                };
                self.answers.insert(path, answer);
                return self.request_plan();
            }
            Ask::Discard => {
                if parse!(self.boolean(reply)) {
                    if let Some(initial) = self.editor_initial.take() {
                        self.settings = initial;
                    }
                    return self.return_to_confirm();
                }
                return self.enter_editor();
            }
            _ => return Effect::None,
        }
        self.next()
    }
}

fn host_checklist(title: &str, hosts: Hosts, note: Option<String>) -> Question {
    Question::Checklist {
        title: title.into(),
        note,
        options: vec![choice("claude", "Claude"), choice("codex", "Codex")],
        selected: vec![hosts.claude, hosts.codex],
    }
}

/// `all` when every skill is chosen, `none` when none is, else the chosen
/// names in inventory order.
pub fn selection(skills: &[String], chosen: &[bool]) -> SkillSelection {
    let names: Vec<String> = skills
        .iter()
        .zip(chosen)
        .filter(|(_, yes)| **yes)
        .map(|(name, _)| name.clone())
        .collect();
    if names.is_empty() {
        SkillSelection::None
    } else if names.len() == skills.len() {
        SkillSelection::All
    } else {
        SkillSelection::Names(names)
    }
}
