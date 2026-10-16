//! UI-layer tests: key sequences in, rendered `TestBackend` frames and the
//! resulting settings, plans, and effects out. Planning uses an in-memory
//! filesystem snapshot, so nothing touches the disk or a terminal.
//!
//! Each test is named after the `test_installer.py` test whose terminal
//! behavior it covers (installer spec §10, layer U). Terminal restoration
//! after Ctrl-C (31) and real-terminal byte sequences (25) are end-to-end
//! concerns; here they are covered as `Interrupted` effects and styled cells.
//!
//! | §10 | Python test (`test_installer.py`)                                                 | Rust test |
//! |-----|------------------------------------------------------------------------------------|-----------|
//! | 9   | test_local_repo_build_waits_for_confirmation_and_cancel_never_executes_source      | local_repo_build_waits_for_confirmation_and_cancel_never_executes_source |
//! | 10  | test_offline_terminal_plugin_preview_can_open_editor_without_changes               | offline_terminal_plugin_preview_can_open_editor_without_changes |
//! | 11  | test_offline_terminal_preview_can_handle_conflicts_without_changes                 | offline_terminal_preview_can_handle_conflicts_without_changes |
//! | 12  | test_online_terminal_dry_run_also_prompts_but_never_installs                       | online_terminal_dry_run_also_prompts_but_never_installs |
//! | 18  | test_incompatible_daemon_offer_accepts_or_declines_in_terminal                     | incompatible_daemon_offer_accepts_or_declines_in_terminal |
//! | 22  | test_terminal_configuration_and_explicit_approval_before_writes                    | terminal_configuration_and_explicit_approval_before_writes |
//! | 23  | test_terminal_default_decline_and_ctrl_c_leave_installation_unchanged              | terminal_default_decline_and_ctrl_c_leave_installation_unchanged |
//! | 24  | test_arrow_keys_switch_modes_and_open_editor_without_approving                     | arrow_keys_switch_modes_and_open_editor_without_approving |
//! | 25  | test_terminal_color_controls_and_piped_stdout                                      | terminal_color_controls_and_piped_stdout |
//! | 26  | test_advanced_plugin_edits_apply_only_after_confirmation_and_recheck_conflicts     | advanced_plugin_edits_apply_only_after_confirmation_and_recheck_conflicts |
//! | 27  | test_advanced_data_directory_edit_moves_default_binary_without_writing_on_cancel   | advanced_data_directory_edit_moves_default_binary_without_writing_on_cancel |
//! | 28  | test_advanced_cancel_confirms_dirty_draft_and_discards_only_after_approval         | advanced_cancel_confirms_dirty_draft_and_discards_only_after_approval |
//! | 29  | test_advanced_enum_cycles_keep_selection_and_restore_clean_state                   | advanced_enum_cycles_keep_selection_and_restore_clean_state |
//! | 30  | test_advanced_path_escape_and_paste_do_not_apply_unintended_input                  | advanced_path_escape_and_paste_do_not_apply_unintended_input |
//! | 31  | test_advanced_ctrl_c_restores_terminal_during_navigation_and_editing               | advanced_ctrl_c_restores_terminal_during_navigation_and_editing |
//! | 32  | test_advanced_form_scrolls_and_preserves_selection_on_small_terminals              | advanced_form_scrolls_and_preserves_selection_on_small_terminals |
//! | 33  | test_advanced_plugin_additional_locations_and_completion_destination               | advanced_plugin_additional_locations_and_completion_destination |
//! | 34  | test_plain_advanced_config_can_change_completion_and_profile_paths                 | plain_advanced_config_can_change_completion_and_profile_paths |
//! | 35  | test_advanced_removing_new_location_restores_clean_state                           | advanced_removing_new_location_restores_clean_state |
//! | 36  | test_advanced_standalone_component_enums_install_selected_hosts                    | advanced_standalone_component_enums_install_selected_hosts |
//! | 42  | test_plugin_prompts_default_to_current_shell_and_adding_to_path                    | plugin_prompts_default_to_current_shell_and_adding_to_path |
//! | 43  | test_missing_default_shell_paths_warn_and_skip_without_prompting_or_creating       | missing_default_shell_paths_warn_and_skip_without_prompting_or_creating |
//! | 45  | test_confirmation_repeats_skipped_option_warnings_and_clears_corrected_paths       | confirmation_repeats_skipped_option_warnings_and_clears_corrected_paths |
//! | 48  | test_plugin_prompt_defaults_respect_explicit_opt_outs                              | plugin_prompt_defaults_respect_explicit_opt_outs |
//! | 49  | test_plugin_unknown_shell_asks_for_supported_path_shell                            | plugin_unknown_shell_asks_for_supported_path_shell |
//! | 60  | test_terminal_can_preserve_a_conflicting_skill                                     | terminal_can_preserve_a_conflicting_skill |
//! | 70  | test_plain_advanced_config_can_enable_mcp                                          | plain_advanced_config_can_enable_mcp |
//! | 72  | test_no_path_warning_refreshes_after_advanced_config_edit                          | no_path_warning_refreshes_after_advanced_config_edit |
//! | 84  | test_terminal_selects_skills_with_descriptions_and_edits_individual_choices        | terminal_selects_skills_with_descriptions_and_edits_individual_choices |
//! | 85  | test_plain_terminal_can_select_only_the_minimal_skill                              | plain_terminal_can_select_only_the_minimal_skill |
//!
//! Tests after the table cover the approved fixes to `install.sh` quirks.
use super::*;
use crate::installer::{
    Answers, Environment, FileSnapshot, HostName, InstallArgs, Mode, Node, Plan, PlanFailure,
    ReleaseInventory, SavedState, Settings, SkillSelection, Step, Style, plan_settings,
};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{Terminal, TerminalOptions, Viewport, backend::TestBackend, style::Color};
use std::{
    collections::BTreeMap,
    ffi::OsString,
    io::Cursor,
    path::{Path, PathBuf},
};

const HOME: &str = "/home/user";
const PLUGIN_PROMPT: &str = "Installing `leanck` as a plugin. Continue?";
const STANDALONE_PROMPT: &str = "Installing `leanck` as standalone. Continue?";
const NO_PATH_WARNING: &str = "agents may not be able to execute leanck";

fn home(relative: &str) -> PathBuf {
    Path::new(HOME).join(relative)
}
fn strings(names: &[&str]) -> Vec<String> {
    names.iter().map(|s| s.to_string()).collect()
}

/// The Python suite's setup: XDG bases under HOME, `SHELL=/bin/zsh`, host
/// homes from the environment, both host CLIs on PATH, nothing on disk.
struct Fixture {
    vars: BTreeMap<String, OsString>,
    files: FileSnapshot,
    saved: SavedState,
    inventory: ReleaseInventory,
}

impl Fixture {
    fn new() -> Self {
        let mut vars = BTreeMap::new();
        for (key, value) in [
            ("HOME", HOME.to_owned()),
            ("XDG_DATA_HOME", format!("{HOME}/data")),
            ("XDG_STATE_HOME", format!("{HOME}/state")),
            ("XDG_CONFIG_HOME", format!("{HOME}/config")),
            ("SHELL", "/bin/zsh".to_owned()),
            ("CLAUDE_CONFIG_DIR", format!("{HOME}/claude")),
            ("CODEX_HOME", format!("{HOME}/codex")),
            ("PATH", "/usr/bin:/bin".to_owned()),
        ] {
            vars.insert(key.to_owned(), OsString::from(value));
        }
        Self {
            vars,
            files: FileSnapshot::assume_missing(),
            saved: SavedState::default(),
            inventory: ReleaseInventory {
                root: PathBuf::from("/tmp/release/leanck"),
                tag: "v1.2.3".into(),
                protocol: 3,
                config_example: true,
                skills: strings(&["lean-proving", "lean-style", "leanck"]),
                claude_agents: strings(&[
                    "lean-expert.md",
                    "lean-grind.md",
                    "lean-style-worker.md",
                ]),
                codex_agents: strings(&[
                    "lean-expert.toml",
                    "lean-grind.toml",
                    "lean-style-worker.toml",
                ]),
            },
        }
    }
    fn var(mut self, key: &str, value: &str) -> Self {
        self.vars.insert(key.into(), value.into());
        self
    }
    fn dir(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.insert(path, Node::Directory);
        self
    }
    fn file(mut self, path: impl Into<PathBuf>) -> Self {
        self.files.insert(
            path,
            Node::File {
                digest: "foreign".into(),
                text: Some("# existing\n".into()),
            },
        );
        self
    }
    /// A local source checkout at `/src` with version 9.8.7.
    fn checkout(mut self) -> Self {
        self = self
            .file("/src/crates/leanck-cli/Cargo.toml")
            .dir("/src/leanck-skills")
            .dir("/src/leanck-subagents");
        self.inventory.tag = "v9.8.7".into();
        self
    }
    fn env(&self) -> Environment {
        let mut env = Environment::from_vars(&self.vars, PathBuf::from("/work")).unwrap();
        env.commands = ["claude", "codex"].iter().map(|c| c.to_string()).collect();
        env.files = self.files.clone();
        env
    }
    fn settings(&self, argv: &[&str]) -> Settings {
        let args = InstallArgs::parse_from(argv).unwrap();
        Settings::resolve(&args, &self.env(), &self.saved).unwrap()
    }
    fn plan(&self, settings: &Settings, answers: &Answers) -> Result<Plan, PlanFailure> {
        plan_settings(settings, &self.env(), &self.saved, &self.inventory, answers)
    }
    fn ui(&self, argv: &[&str]) -> Harness<'_> {
        self.ui_colored(argv, false)
    }
    fn ui_colored(&self, argv: &[&str], color: bool) -> Harness<'_> {
        let flow = Flow::new(
            self.settings(argv),
            &self.inventory,
            PathBuf::from("/work"),
            false,
        );
        Harness {
            fixture: self,
            ui: Installer::new(flow, color),
            transcript: String::new(),
            effects: vec![],
            plans: 0,
        }
    }

    /// Run the line-mode flow over `replies`; returns the final effect and
    /// everything written.
    fn plain(&self, argv: &[&str], replies: &[&str], style: Style) -> (Effect, String) {
        let mut flow = Flow::new(
            self.settings(argv),
            &self.inventory,
            PathBuf::from("/work"),
            true,
        );
        let mut input = Cursor::new(replies.iter().map(|r| format!("{r}\n")).collect::<String>());
        let mut out = vec![];
        let effect = line::run(&mut flow, &mut input, &mut out, style, |s, a| {
            self.plan(s, a)
        })
        .unwrap();
        (effect, String::from_utf8(out).unwrap())
    }
}

fn key(code: KeyCode) -> Input {
    Input::Key(KeyEvent::new(code, KeyModifiers::NONE))
}
fn ctrl(c: char) -> Input {
    Input::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::CONTROL))
}

/// Drives an [`Installer`], planning against the fixture snapshot.
struct Harness<'a> {
    fixture: &'a Fixture,
    ui: Installer,
    /// Output printed incrementally between widgets.
    transcript: String,
    /// Every effect other than `None` and `Plan`, in order.
    effects: Vec<Effect>,
    /// How many plans were requested.
    plans: usize,
}

impl Harness<'_> {
    fn send(&mut self, input: Input) -> Effect {
        let mut effect = self.ui.update(input);
        self.transcript
            .push_str(&self.ui.take_transcript(Style::PLAIN));
        while effect == Effect::Plan {
            self.plans += 1;
            let result = self.fixture.plan(self.ui.settings(), self.ui.answers());
            effect = self.ui.planned(result);
            self.transcript
                .push_str(&self.ui.take_transcript(Style::PLAIN));
        }
        if effect != Effect::None {
            self.effects.push(effect.clone());
        }
        effect
    }
    fn press(&mut self, code: KeyCode) -> Effect {
        self.send(key(code))
    }
    fn enter(&mut self) -> Effect {
        self.press(KeyCode::Enter)
    }
    fn type_text(&mut self, text: &str) {
        for c in text.chars() {
            self.press(KeyCode::Char(c));
        }
    }

    fn question(&self) -> Question {
        self.ui.flow().question()
    }
    fn form(&self) -> &Form {
        self.ui.form().expect("the advanced form is open")
    }
    fn screen_sized(&self, width: u16, height: u16) -> String {
        let mut terminal = Terminal::with_options(
            TestBackend::new(width, height),
            TerminalOptions {
                viewport: Viewport::Inline(self.ui.height(width).min(height.saturating_sub(1))),
            },
        )
        .unwrap();
        terminal.draw(|frame| self.ui.render(frame)).unwrap();
        let buffer = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buffer[(x, y)].symbol().to_owned())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
    fn screen(&self) -> String {
        self.screen_sized(100, 40)
    }
    /// The transcript printed so far, outside the active widget.
    fn transcript(&self) -> String {
        self.transcript.clone()
    }
    fn count(&self, needle: &str) -> usize {
        self.transcript().matches(needle).count()
    }

    /// Choose `label` in the menu titled `title`.
    fn choose(&mut self, title: &str, label: &str) -> Effect {
        match self.question() {
            Question::Menu {
                title: t, options, ..
            } => {
                assert_eq!(t, title);
                assert!(
                    options.iter().any(|o| o.label == label),
                    "{label} in {options:?}"
                );
            }
            other => panic!("expected the menu {title:?}, got {other:?}"),
        }
        for _ in 0..8 {
            if self.ui.menu_selection() == Some(label) {
                return self.enter();
            }
            self.press(KeyCode::Down);
        }
        panic!("{label} not reachable");
    }
    /// Answer the prompt whose label starts with `prefix`.
    fn prompt(&mut self, prefix: &str, reply: &str) -> Effect {
        match self.question() {
            Question::Prompt { label, .. } => {
                assert!(
                    label.starts_with(prefix),
                    "expected {prefix:?}, got {label:?}"
                )
            }
            other => panic!("expected the prompt {prefix:?}, got {other:?}"),
        }
        self.type_text(reply);
        self.enter()
    }

    fn plugin_config(&mut self, completions: &str, path: &str) {
        self.choose("Install as plugin or standalone?", "Plugin (recommended)");
        self.prompt("Shell completions (", completions);
        self.prompt("Add leanck to PATH?", path);
    }
    fn open_plugin_config(&mut self) {
        self.plugin_config("none", "no");
        self.choose(PLUGIN_PROMPT, "Edit advanced config...");
    }
    /// The Python suite's standalone dialog; empty replies take defaults.
    fn standalone_config(
        &mut self,
        binary_dir: &str,
        skills: &str,
        choices: &[(&str, &str)],
        agents: &str,
        completions: &str,
        path: &str,
    ) {
        self.choose("Install as plugin or standalone?", "Standalone");
        self.prompt("Install the CLI?", "");
        self.prompt("Install location", binary_dir);
        self.prompt("Standalone skills", skills);
        if matches!(skills, "claude" | "codex" | "both") {
            for name in self.fixture.inventory.skills.clone() {
                let reply = choices
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map_or("", |(_, r)| r);
                self.prompt(&format!("Install {name} ("), reply);
            }
        }
        self.prompt("Custom agents", agents);
        if matches!(skills, "claude" | "both") {
            self.prompt("Claude skills directory", "");
        }
        if matches!(skills, "codex" | "both") {
            self.prompt("Codex skills directory", "");
        }
        if matches!(agents, "claude" | "both") {
            self.prompt("Claude agents directory", "");
        }
        if matches!(agents, "codex" | "both") {
            self.prompt("Codex agents directory", "");
        }
        self.prompt("Shell completions (", completions);
        self.prompt("Add leanck to PATH?", path);
    }

    fn focus(&mut self, label: &str) {
        for _ in 0..=self.form().fields().len() {
            let form = self.form();
            if form.label(form.selected()).trim() == label {
                return;
            }
            self.press(KeyCode::Down);
        }
        panic!("no form row labeled {label:?}");
    }
    fn value(&self) -> String {
        let form = self.form();
        form.value(form.selected())
    }
    fn set_enum(&mut self, label: &str, value: &str) {
        self.focus(label);
        for _ in 0..8 {
            if self.value() == value {
                return;
            }
            self.press(KeyCode::Right);
        }
        panic!("{label} never became {value}");
    }
    fn edit(&mut self, label: &str, value: &str) {
        self.focus(label);
        self.enter();
        assert!(self.form().is_editing(), "{label} opens an editor");
        self.send(ctrl('u'));
        self.type_text(value);
        self.enter();
    }
    fn action(&mut self, label: &str) -> Effect {
        self.focus(label);
        self.enter()
    }

    fn confirmed(&self) -> Plan {
        match self.effects.last() {
            Some(Effect::Confirmed(plan)) => (**plan).clone(),
            other => panic!("expected a confirmed plan, got {other:?}"),
        }
    }
    fn plan(&self) -> &Plan {
        self.ui.flow().plan().expect("a plan")
    }
}

fn has(plan: &Plan, needle: &str) -> bool {
    plan.actions().iter().any(|a| a.contains(needle))
}

#[test]
fn local_repo_build_waits_for_confirmation_and_cancel_never_executes_source() {
    let f = Fixture::new().checkout();
    let mut t = f.ui(&["--from-repo", "/src"]);
    t.plugin_config("none", "no");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    // Nothing is built, packaged, or installed before approval.
    assert!(t.effects.is_empty());
    assert_eq!(
        t.plan().settings.from_repo.as_deref(),
        Some(Path::new("/src"))
    );
    assert_eq!(t.choose(PLUGIN_PROMPT, "Cancel"), Effect::Cancelled);

    let mut t = f.ui(&["--from-repo", "/src"]);
    t.open_plugin_config();
    t.set_enum("Register plugins", "codex");
    t.action("Save changes");
    assert!(t.effects.is_empty());
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    t.choose(PLUGIN_PROMPT, "Continue");
    let plan = t.confirmed();
    assert_eq!(plan.version, "v9.8.7 (local source)");
    assert_eq!(
        plan.registrations
            .iter()
            .map(|r| r.host)
            .collect::<Vec<_>>(),
        [HostName::Codex]
    );
}

#[test]
fn offline_terminal_plugin_preview_can_open_editor_without_changes() {
    let f = Fixture::new().checkout();
    let mut t = f.ui(&["--from-repo", "/src", "--dry-run"]);
    assert!(t.transcript().is_empty());
    assert!(t.screen().starts_with("Install as plugin or standalone?"));
    t.open_plugin_config();
    assert!(t.screen().contains("[bundled with plugin]"));
    let plans = t.plans;
    t.action("Save changes");
    assert_eq!(t.plans, plans, "an unchanged form does not re-plan");
    t.choose(PLUGIN_PROMPT, "Continue");
    assert!(t.confirmed().settings.dry_run);
    let transcript = t.transcript();
    assert!(!transcript.contains("Standalone skills"));
    assert!(!transcript.contains("Install the CLI?"));
}

#[test]
fn offline_terminal_preview_can_handle_conflicts_without_changes() {
    let f = Fixture::new()
        .checkout()
        .dir(home("claude/skills/lean-proving"));
    let mut t = f.ui(&["--from-repo", "/src", "--dry-run", "--verbose"]);
    t.standalone_config("", "claude", &[], "", "none", "");
    let skill = home("claude/skills/lean-proving");
    assert!(t.transcript().contains(&format!(
        "[WARN] {} already exists or has local edits.",
        skill.display()
    )));
    t.prompt("Keep it, back up and replace", "replace");
    t.choose(STANDALONE_PROMPT, "Edit advanced config...");
    t.action("Save changes");
    assert!(
        t.transcript()
            .contains(&format!("Replace {} (backed up first)", skill.display()))
    );
    t.choose(STANDALONE_PROMPT, "Continue");
    assert!(t.confirmed().replaced.contains(&skill));
}

#[test]
fn online_terminal_dry_run_also_prompts_but_never_installs() {
    let f = Fixture::new();
    let mut t = f.ui(&["--dry-run"]);
    t.standalone_config("", "", &[], "", "", "");
    assert!(t.effects.is_empty());
    t.choose(STANDALONE_PROMPT, "Continue");
    let plan = t.confirmed();
    assert!(plan.settings.dry_run);
    assert_eq!(plan.settings.mode, Mode::Standalone);
}

#[test]
fn incompatible_daemon_offer_accepts_or_declines_in_terminal() {
    for (replies, expected) in [
        ("no\n", Some(false)),
        ("yes\n", Some(true)),
        ("\n", Some(true)),
        ("maybe\nY\n", Some(true)),
        ("", None),
    ] {
        let mut out = vec![];
        let answer = line::ask_yes_no(
            &mut Cursor::new(replies),
            &mut out,
            Style::PLAIN,
            "Stop incompatible leanck daemons now?",
            true,
        )
        .unwrap();
        assert_eq!(answer, expected, "{replies:?}");
        let out = String::from_utf8(out).unwrap();
        assert!(out.starts_with("Stop incompatible leanck daemons now? [yes]: "));
        assert_eq!(
            out.contains("[WARN] Enter yes or no."),
            replies.starts_with("maybe")
        );
    }
}

#[test]
fn terminal_configuration_and_explicit_approval_before_writes() {
    let f = Fixture::new();
    let mut t = f.ui_colored(&[], true);
    // Prompts are styled when color is on.
    let mut terminal = Terminal::with_options(
        TestBackend::new(100, 30),
        TerminalOptions {
            viewport: Viewport::Inline(t.ui.height(100).min(29)),
        },
    )
    .unwrap();
    terminal.draw(|frame| t.ui.render(frame)).unwrap();
    assert!(
        terminal
            .backend()
            .buffer()
            .content()
            .iter()
            .any(|cell| cell.fg == Color::Cyan)
    );
    t.standalone_config("/custom/chosen bin", "", &[], "", "", "");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    assert!(t.effects.is_empty(), "nothing runs before Continue");
    assert!(has(
        t.plan(),
        "Install executable: /custom/chosen bin/leanck"
    ));
    t.choose(STANDALONE_PROMPT, "Continue");
    assert_eq!(
        t.confirmed().steps[0],
        Step::Executable {
            destination: PathBuf::from("/custom/chosen bin/leanck")
        }
    );
}

#[test]
fn terminal_default_decline_and_ctrl_c_leave_installation_unchanged() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.plugin_config("none", "no");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    assert_eq!(t.enter(), Effect::Cancelled);
    assert_eq!(t.effects, [Effect::Cancelled]);

    for interrupt in ['c', 'd'] {
        let mut t = f.ui(&[]);
        assert!(matches!(t.question(), Question::Menu { .. }));
        assert_eq!(t.send(ctrl(interrupt)), Effect::Interrupted);
        assert_eq!(t.question(), Question::Done);
        assert_eq!(t.plans, 0);
    }
}

#[test]
fn arrow_keys_switch_modes_and_open_editor_without_approving() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    let title = "Install as plugin or standalone?";
    assert_eq!(t.ui.menu_selection(), Some("Plugin (recommended)"));
    assert!(t.screen().contains("> Plugin (recommended)"));
    t.press(KeyCode::Down);
    assert_eq!(t.ui.menu_selection(), Some("Standalone"));
    assert!(t.screen().contains("> Standalone"));
    t.press(KeyCode::Down);
    assert_eq!(
        t.ui.menu_selection(),
        Some("Plugin (recommended)"),
        "wraps around"
    );
    t.press(KeyCode::Up);
    t.press(KeyCode::Up);
    t.type_text("standalone");
    t.press(KeyCode::Esc);
    assert_eq!(
        t.ui.menu_selection(),
        Some("Plugin (recommended)"),
        "text and Escape are ignored"
    );
    t.enter();
    assert!(t.transcript().contains(title));
    t.prompt("Shell completions (", "");
    t.prompt("Add leanck to PATH?", "no");
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    t.focus("Plugin data directory");
    let form = t.screen();
    assert!(form.starts_with("Advanced configuration\nPlugin installation"));
    for label in [
        "Install skills",
        "Install agents",
        "Codex agents directory",
        "Shell completions",
        "Add leanck to PATH",
        "Register plugins",
        "Save changes",
        "Cancel",
    ] {
        assert!(form.contains(label), "{label}");
    }
    for label in [
        "Installer state directory",
        "Command timeout",
        "Back to confirmation",
    ] {
        assert!(!form.contains(label), "{label}");
    }
    t.action("Save changes");
    assert_eq!(t.choose(PLUGIN_PROMPT, "Cancel"), Effect::Cancelled);
    assert_eq!(t.effects, [Effect::Cancelled]);
}

#[test]
fn terminal_color_controls_and_piped_stdout() {
    let f = Fixture::new();
    // Without color, no cell carries a color or modifier.
    let mut t = f.ui(&[]);
    t.plugin_config("", "");
    let mut terminal = Terminal::with_options(
        TestBackend::new(100, 30),
        TerminalOptions {
            viewport: Viewport::Inline(t.ui.height(100).min(29)),
        },
    )
    .unwrap();
    terminal.draw(|frame| t.ui.render(frame)).unwrap();
    assert!(terminal.backend().buffer().content().iter().all(|cell| {
        cell.fg == Color::Reset && cell.bg == Color::Reset && cell.modifier.is_empty()
    }));
    assert_eq!(t.choose(PLUGIN_PROMPT, "Cancel"), Effect::Cancelled);

    // The plain flow under TERM=dumb has no escape sequences at all.
    let (effect, out) = f.plain(
        &[],
        &["plugin", "", "no", "edit", "save", "cancel"],
        Style::PLAIN,
    );
    assert_eq!(effect, Effect::Cancelled);
    assert!(!out.contains('\x1b'), "{out}");
    assert!(out.contains(
        "Install as plugin or standalone?\n  1) Plugin (recommended)\n  2) Standalone\nChoice [1]: "
    ));
    assert!(
        out.contains("Edit advanced config\n  1) Plugin data directory\n  2) Executable directory")
    );

    // Styled line prompts, as on a terminal whose stdout is piped.
    let (effect, out) = f.plain(
        &[],
        &["standalone", "", "", "", "", "", "no", "cancel"],
        Style::COLOR,
    );
    assert_eq!(effect, Effect::Cancelled);
    assert!(
        out.contains("\x1b[1m\x1b[36mAdd leanck to PATH?\x1b[0m [no]: "),
        "{out:?}"
    );
}

#[test]
fn advanced_plugin_edits_apply_only_after_confirmation_and_recheck_conflicts() {
    let foreign = "/root/edited codex config/agents/leanck/lean-expert.toml";
    let f = Fixture::new().file(foreign);
    let mut t = f.ui(&[]);
    t.open_plugin_config();
    t.edit("Plugin data directory", "/root/edited plugin data");
    t.edit("Executable directory", "/root/edited executable directory");
    t.set_enum("Register plugins", "codex");
    t.set_enum("Enable MCP", "yes");
    t.edit("Codex configuration", "/root/edited codex config");
    let plans = t.plans;
    t.action("Save changes");
    assert_eq!(t.plans, plans + 1, "saving a change re-plans");
    t.prompt("Keep it, back up and replace", "keep");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    assert!(t.effects.is_empty(), "nothing is applied before Continue");
    t.choose(PLUGIN_PROMPT, "Continue");
    let plan = t.confirmed();
    let s = &plan.settings;
    assert_eq!(s.data_dir, Path::new("/root/edited plugin data"));
    assert_eq!(s.bin_dir, Path::new("/root/edited executable directory"));
    assert!(s.mcp_enabled);
    assert!(
        plan.actions()
            .contains(&format!("Keep {foreign} unchanged"))
    );
    assert!(plan.steps.contains(&Step::Bundle {
        destination: PathBuf::from("/root/edited plugin data/bundle"),
        binary: PathBuf::from("/root/edited executable directory/leanck"),
        mcp_enabled: true,
    }));
    assert_eq!(plan.registrations.len(), 1);
    assert_eq!(plan.registrations[0].host, HostName::Codex);
    assert_eq!(
        plan.registrations[0].home,
        Path::new("/root/edited codex config")
    );
    assert!(plan.steps.contains(&Step::Agent {
        source: "plugin/codex/agents/lean-grind.toml".into(),
        destination: PathBuf::from("/root/edited codex config/agents/leanck/lean-grind.toml"),
        link: Some(PathBuf::from(
            "/root/edited plugin data/agents/lean-grind.toml"
        )),
    }));
}

#[test]
fn advanced_data_directory_edit_moves_default_binary_without_writing_on_cancel() {
    let f = Fixture::new();
    let mut t = f.ui(&["--verbose"]);
    t.open_plugin_config();
    t.edit("Plugin data directory", "/root/preview edited data");
    t.focus("Executable directory");
    assert_eq!(t.value(), "/root/preview edited data/bin");
    t.action("Save changes");
    assert!(
        t.transcript()
            .contains("Executable: /root/preview edited data/bin/leanck")
    );
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    let plans = t.plans;
    t.action("Save changes");
    assert_eq!(t.plans, plans);
    assert_eq!(t.choose(PLUGIN_PROMPT, "Cancel"), Effect::Cancelled);
    assert_eq!(t.effects, [Effect::Cancelled]);
}

#[test]
fn advanced_cancel_confirms_dirty_draft_and_discards_only_after_approval() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.open_plugin_config();
    t.focus("Plugin data directory");
    let original = t.value();
    t.edit("Plugin data directory", "/root/discarded plugin data");
    t.action("Cancel");
    assert_eq!(t.form().discard_selection(), Some("Keep editing"));
    let screen = t.screen();
    assert!(screen.contains("Discard advanced config changes?"));
    assert!(screen.contains("Your changes have not been saved."));
    t.enter();
    assert_eq!(t.form().discard_selection(), None);
    t.action("Cancel");
    t.press(KeyCode::Esc);
    assert_eq!(t.form().discard_selection(), None, "Escape keeps editing");
    t.action("Cancel");
    t.press(KeyCode::Down);
    assert_eq!(t.form().discard_selection(), Some("Discard changes"));
    t.enter();
    assert!(matches!(t.question(), Question::Menu { .. }));
    assert_eq!(t.ui.settings().data_dir, home("data/leanck"));
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    t.focus("Plugin data directory");
    assert_eq!(t.value(), original);
    t.action("Cancel");
    assert_eq!(t.choose(PLUGIN_PROMPT, "Cancel"), Effect::Cancelled);
}

#[test]
fn advanced_enum_cycles_keep_selection_and_restore_clean_state() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.open_plugin_config();
    t.focus("Enable MCP");
    assert_eq!(t.value(), "no");
    t.set_enum("Enable MCP", "yes");
    t.set_enum("Enable MCP", "no");
    for value in ["none", "claude", "codex", "both"] {
        t.set_enum("Register plugins", value);
    }
    for value in ["bash", "zsh", "fish", "powershell", "elvish", "none"] {
        t.set_enum("Shell completions", value);
        assert_eq!(t.form().label(t.form().selected()), "Shell completions");
    }
    t.press(KeyCode::Left);
    assert_eq!(t.value(), "elvish");
    t.press(KeyCode::Right);
    assert_eq!(t.value(), "none");
    t.set_enum("Add leanck to PATH", "yes");
    assert_eq!(t.form().label(t.form().selected()), "Add leanck to PATH");
    t.set_enum("Shell", "fish");
    t.focus("Shell profile");
    assert!(t.value().contains("fish/config.fish"));
    t.set_enum("Shell", "zsh");
    t.set_enum("Add leanck to PATH", "no");
    assert!(!t.form().fields().contains(&Field::Shell));
    assert!(!t.form().fields().contains(&Field::PathFile));
    assert!(!t.screen().contains("Shell profile"));
    assert!(!t.form().dirty());
    t.action("Cancel");
    assert!(
        matches!(t.question(), Question::Menu { .. }),
        "a clean form closes without asking"
    );
    assert!(!t.transcript().contains("Discard advanced config changes?"));
    t.choose(PLUGIN_PROMPT, "Cancel");
}

#[test]
fn advanced_path_escape_and_paste_do_not_apply_unintended_input() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.open_plugin_config();
    t.focus("Plugin data directory");
    let original = t.value();
    t.type_text("IGNORED-NAVIGATION-TEXT");
    assert!(!t.form().is_editing());
    assert!(!t.screen().contains("IGNORED"));
    t.send(Input::Paste("pasted while navigating".into()));
    assert!(!t.screen().contains("pasted"));
    t.enter();
    assert!(t.form().is_editing());
    t.send(ctrl('u'));
    t.type_text("discard-this-path");
    assert!(t.screen().contains("discard-this-path"));
    t.press(KeyCode::Esc);
    assert!(!t.form().is_editing());
    assert_eq!(t.value(), original);
    t.enter();
    t.send(Input::Paste("bad\npasted\ncommands".into()));
    assert_eq!(t.form().message(), Some("Paste a single-line value."));
    assert!(t.screen().contains("Paste a single-line value."));
    assert!(t.form().is_editing());
    assert_eq!(t.form().buffer(), Some(original.as_str()));
    t.press(KeyCode::Esc);
    assert!(!t.form().is_editing());
    assert!(!t.form().dirty());
    t.action("Cancel");
    assert!(matches!(t.question(), Question::Menu { .. }));
    t.choose(PLUGIN_PROMPT, "Cancel");
}

#[test]
fn advanced_ctrl_c_restores_terminal_during_navigation_and_editing() {
    let f = Fixture::new();
    for editing in [false, true] {
        let mut t = f.ui(&[]);
        t.open_plugin_config();
        t.focus("Plugin data directory");
        if editing {
            t.enter();
            assert!(t.form().is_editing());
        }
        assert_eq!(t.send(ctrl('c')), Effect::Interrupted);
        assert_eq!(t.effects, [Effect::Interrupted]);
        assert!(t.ui.form().is_none());
    }
}

#[test]
fn advanced_form_scrolls_and_preserves_selection_on_small_terminals() {
    let f = Fixture::new();
    let mut t = f.ui(&["--verbose"]);
    t.open_plugin_config();
    let custom = format!("/root/{}", "long executable directory ".repeat(4));
    let custom = custom.trim_end();
    let visible = |t: &Harness| {
        let screen = t.screen_sized(64, 12);
        let label = t.form().label(t.form().selected());
        assert!(
            screen
                .lines()
                .any(|l| l.starts_with(&format!("> {}", label.trim_end()))),
            "{label:?} is visible in\n{screen}"
        );
        assert_eq!(screen.lines().count(), 11, "one terminal row stays free");
    };
    t.set_enum("Register plugins", "codex");
    visible(&t);
    t.set_enum("Add leanck to PATH", "yes");
    visible(&t);
    t.set_enum("Shell", "bash");
    visible(&t);
    t.focus("Executable directory");
    visible(&t);
    t.edit("Executable directory", custom);
    visible(&t);
    t.focus("Cancel");
    visible(&t);
    t.action("Save changes");
    // Saving needs an existing profile for bash; the plan reports it skipped.
    assert!(matches!(t.question(), Question::Menu { .. }));
    assert!(
        t.transcript()
            .contains(&format!("Install executable: {custom}/leanck"))
    );
    assert_eq!(t.choose(PLUGIN_PROMPT, "Cancel"), Effect::Cancelled);
}

#[test]
fn advanced_plugin_additional_locations_and_completion_destination() {
    let f = Fixture::new()
        .dir("/root/custom completions")
        .file("/root/custom profile");
    let mut t = f.ui(&[]);
    t.open_plugin_config();
    for (label, path) in [
        ("Add skill location...", "/root/extra skills"),
        ("Add Claude location...", "/root/extra Claude agents"),
        ("Add Codex location...", "/root/extra Codex agents"),
    ] {
        t.focus(label);
        t.enter();
        assert!(t.form().is_editing());
        assert_eq!(t.form().buffer(), Some(""));
        t.type_text(path);
        t.enter();
    }
    let screen = t.screen();
    assert!(screen.contains("Additional location"));
    assert!(screen.contains("/root/extra Claude agents"));
    t.set_enum("Shell completions", "bash");
    t.edit("Completions directory", "/root/custom completions");
    t.set_enum("Add leanck to PATH", "yes");
    t.edit("Shell profile", "/root/custom profile");
    t.action("Save changes");
    assert!(t.effects.is_empty());
    t.choose(PLUGIN_PROMPT, "Continue");
    let plan = t.confirmed();
    let s = &plan.settings;
    assert_eq!(s.extra_skills, [PathBuf::from("/root/extra skills")]);
    assert_eq!(
        s.extra_claude_agents,
        [PathBuf::from("/root/extra Claude agents")]
    );
    assert_eq!(
        s.extra_codex_agents,
        [PathBuf::from("/root/extra Codex agents")]
    );
    for action in [
        "Install skills into /root/extra skills",
        "Install Claude Code agents into /root/extra Claude agents",
        "Install Codex agents into /root/extra Codex agents",
        "Install bash completions: /root/custom completions/leanck",
        "Add leanck to PATH in /root/custom profile",
    ] {
        assert!(plan.actions().contains(&action.to_owned()), "{action}");
    }
}

#[test]
fn plain_advanced_config_can_change_completion_and_profile_paths() {
    let f = Fixture::new()
        .dir("/root/existing completions")
        .file("/root/existing profile");
    let (effect, out) = f.plain(
        &[],
        &[
            "plugin",
            "",
            "",
            "edit",
            "completions",
            "fish",
            "/root/existing completions",
            "path",
            "",
            "fish",
            "/root/existing profile",
            "save",
            "continue",
        ],
        Style::PLAIN,
    );
    let Effect::Confirmed(plan) = effect else {
        panic!("{effect:?}\n{out}");
    };
    assert!(out.contains("Completions directory ["));
    assert!(out.contains("Shell profile to update ["));
    assert!(has(
        &plan,
        "Install fish completions: /root/existing completions/leanck.fish"
    ));
    assert!(
        plan.actions()
            .contains(&"Add leanck to PATH in /root/existing profile".to_owned())
    );
    assert!(has(&plan, "  fish_add_path -- "));
    assert!(!has(&plan, "zsh"));
}

#[test]
fn advanced_removing_new_location_restores_clean_state() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.open_plugin_config();
    t.edit("Add skill location...", "/root/removed location");
    assert!(t.form().dirty());
    t.edit("Additional location", "");
    assert!(!t.form().dirty());
    assert_eq!(
        t.form().label(t.form().selected()).trim(),
        "Add skill location..."
    );
    // An abandoned new row disappears too.
    t.focus("Add Claude location...");
    t.enter();
    t.type_text("/root/abandoned");
    t.press(KeyCode::Esc);
    assert!(!t.form().dirty());
    assert!(!t.screen().contains("Claude location "));
    t.action("Cancel");
    assert!(matches!(t.question(), Question::Menu { .. }));
    assert!(!t.transcript().contains("Discard advanced config changes?"));
}

#[test]
fn advanced_standalone_component_enums_install_selected_hosts() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.standalone_config("", "", &[], "", "", "");
    t.choose(STANDALONE_PROMPT, "Edit advanced config...");
    assert!(t.screen().contains("Standalone installation"));
    t.set_enum("Install executable", "no");
    t.set_enum("Install executable", "yes");
    t.set_enum("Install skills", "both");
    t.set_enum("Install agents", "codex");
    t.action("Save changes");
    t.choose(STANDALONE_PROMPT, "Continue");
    let plan = t.confirmed();
    for path in [
        home(".local/bin/leanck"),
        home("claude/skills/lean-proving"),
        home(".agents/skills/lean-proving"),
        home("codex/agents/leanck/lean-expert.toml"),
    ] {
        assert!(
            plan.steps.iter().any(
                |step| matches!(step, Step::Executable { destination } | Step::Tree { destination, .. } | Step::Agent { destination, .. } if destination == &path)
            ),
            "{}",
            path.display()
        );
    }
    assert!(!has(&plan, &home("claude/agents").display().to_string()));
    assert!(plan.registrations.is_empty());
}

#[test]
fn plugin_prompts_default_to_current_shell_and_adding_to_path() {
    let f = Fixture::new()
        .dir(home("data/zsh/site-functions"))
        .file(home(".zshrc"));
    let mut t = f.ui(&[]);
    t.choose("Install as plugin or standalone?", "Plugin (recommended)");
    assert!(
        t.screen()
            .contains("Shell completions (none / bash / zsh / fish / powershell / elvish) [zsh]:")
    );
    t.prompt("Shell completions (", "");
    assert!(t.screen().contains("Add leanck to PATH? [yes]:"));
    t.prompt("Add leanck to PATH?", "");
    assert!(t.effects.is_empty());
    t.choose(PLUGIN_PROMPT, "Continue");
    let transcript = t.transcript();
    assert!(
        transcript
            .contains("Shell completions (none / bash / zsh / fish / powershell / elvish) [zsh]: ")
    );
    assert!(transcript.contains("Add leanck to PATH? [yes]: "));
    assert!(!transcript.contains("Completions directory ["));
    assert!(!transcript.contains("Shell profile to update"));
    assert!(!transcript.contains("does not exist"));
    let plan = t.confirmed();
    assert!(plan.actions().contains(&format!(
        "Install zsh completions: {}",
        home("data/zsh/site-functions/_leanck").display()
    )));
    assert!(has(
        &plan,
        &format!("Add leanck to PATH in {}", home(".zshrc").display())
    ));
    assert!(has(&plan, &home("data/leanck/bin").display().to_string()));
}

#[test]
fn missing_default_shell_paths_warn_and_skip_without_prompting_or_creating() {
    let f = Fixture::new();
    for (mode, confirmation) in [("plugin", PLUGIN_PROMPT), ("standalone", STANDALONE_PROMPT)] {
        let mut t = f.ui(&[]);
        if mode == "plugin" {
            t.plugin_config("", "yes");
        } else {
            t.standalone_config("", "", &[], "", "", "yes");
        }
        let transcript = t.transcript();
        for needle in [
            "Completions directory does not exist:",
            "skipping shell completions",
            "Shell profile does not exist:",
            "skipping PATH update",
        ] {
            assert!(transcript.contains(needle), "{mode}: {needle}");
        }
        assert!(!transcript.contains("Completions directory ["));
        assert!(!transcript.contains("Shell profile to update"));
        t.choose(confirmation, "Continue");
        let plan = t.confirmed();
        assert!(plan.skips(&home("data/zsh/site-functions/_leanck")));
        assert!(plan.skips(&home(".zshrc")));
        assert!(
            !plan
                .steps
                .iter()
                .any(|s| matches!(s, Step::Completions { .. } | Step::Profile { .. }))
        );
    }
}

#[test]
fn confirmation_repeats_skipped_option_warnings_and_clears_corrected_paths() {
    let f = Fixture::new()
        .dir("/root/existing completions")
        .file("/root/existing profile");
    let mut t = f.ui(&[]);
    t.plugin_config("", "yes");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    for warning in ["skipping shell completions", "skipping PATH update"] {
        assert_eq!(t.count(warning), 1);
    }
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    t.action("Cancel");
    for warning in ["skipping shell completions", "skipping PATH update"] {
        assert_eq!(t.count(warning), 2, "repeated before the menu");
    }
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    t.edit("Completions directory", "/root/existing completions");
    t.edit("Shell profile", "/root/existing profile");
    t.action("Save changes");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    for warning in ["skipping shell completions", "skipping PATH update"] {
        assert_eq!(t.count(warning), 2, "corrected paths clear the warning");
    }
    assert_eq!(t.choose(PLUGIN_PROMPT, "Cancel"), Effect::Cancelled);
}

#[test]
fn plugin_prompt_defaults_respect_explicit_opt_outs() {
    let f = Fixture::new();
    let mut t = f.ui(&["--completions", "none", "--no-add-to-path"]);
    t.choose("Install as plugin or standalone?", "Plugin (recommended)");
    t.prompt("Shell completions (", "");
    t.prompt("Add leanck to PATH?", "");
    t.choose(PLUGIN_PROMPT, "Continue");
    let transcript = t.transcript();
    assert!(transcript.contains("elvish) [none]: "));
    assert!(transcript.contains("Add leanck to PATH? [no]: "));
    assert!(!transcript.contains("does not exist"));
    let plan = t.confirmed();
    assert!(
        !plan
            .steps
            .iter()
            .any(|s| matches!(s, Step::Completions { .. } | Step::Profile { .. }))
    );
}

#[test]
fn plugin_unknown_shell_asks_for_supported_path_shell() {
    let f = Fixture::new()
        .var("SHELL", "/bin/tcsh")
        .file(home("config/fish/config.fish"));
    let mut t = f.ui(&[]);
    t.choose("Install as plugin or standalone?", "Plugin (recommended)");
    t.prompt("Shell completions (", "");
    t.prompt("Add leanck to PATH?", "");
    assert!(
        t.screen()
            .contains("Shell for PATH (fish / bash / zsh) [bash]:")
    );
    t.prompt("Shell for PATH (", "tcsh");
    assert!(t.transcript().contains("[WARN] Choose fish, bash, or zsh."));
    t.prompt("Shell for PATH (", "fish");
    t.choose(PLUGIN_PROMPT, "Continue");
    let plan = t.confirmed();
    assert_eq!(plan.settings.shell, "fish");
    assert!(plan.steps.contains(&Step::Profile {
        file: home("config/fish/config.fish"),
        line: format!("fish_add_path -- '{}'", home("data/leanck/bin").display()),
    }));
}

#[test]
fn terminal_can_preserve_a_conflicting_skill() {
    let skill = home("claude/skills/lean-proving");
    let f = Fixture::new().dir(&skill);
    let mut t = f.ui(&[]);
    t.standalone_config("", "claude", &[], "", "", "");
    t.prompt("Keep it, back up and replace", "keep");
    t.choose(STANDALONE_PROMPT, "Continue");
    let plan = t.confirmed();
    assert!(
        plan.actions()
            .contains(&format!("Keep {} unchanged", skill.display()))
    );
    assert!(
        !plan
            .steps
            .iter()
            .any(|step| matches!(step, Step::Tree { destination, .. } if destination == &skill))
    );
    assert!(plan.steps.iter().any(|step| matches!(step, Step::Tree { destination, .. } if destination == &home("claude/skills/lean-style"))));
}

#[test]
fn plain_advanced_config_can_enable_mcp() {
    let f = Fixture::new();
    let (effect, out) = f.plain(
        &[],
        &["plugin", "", "no", "edit", "mcp", "yes", "save", "continue"],
        Style::PLAIN,
    );
    let Effect::Confirmed(plan) = effect else {
        panic!("{effect:?}\n{out}");
    };
    assert!(out.contains("- MCP: disabled"));
    assert!(out.contains("- MCP: enabled"));
    assert!(out.contains("Enable MCP? [no]: "));
    assert!(plan.settings.mcp_enabled);
    assert!(!has(&plan, "MCP"));
}

#[test]
fn no_path_warning_refreshes_after_advanced_config_edit() {
    let f = Fixture::new().file(home(".zshrc"));
    let mut t = f.ui(&[]);
    t.plugin_config("none", "no");
    assert_eq!(t.count(NO_PATH_WARNING), 1);
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    t.set_enum("Add leanck to PATH", "yes");
    t.action("Save changes");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    assert_eq!(t.count(NO_PATH_WARNING), 1);
    t.choose(PLUGIN_PROMPT, "Cancel");
}

#[test]
fn terminal_selects_skills_with_descriptions_and_edits_individual_choices() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    assert!(t.screen().contains("> Plugin (recommended)"));
    t.standalone_config(
        "",
        "both",
        &[("lean-proving", "no"), ("lean-style", "no")],
        "",
        "",
        "no",
    );
    let transcript = t.transcript();
    for description in [
        "Minimal leanck command reference",
        "Opinionated Lean theorem-proving workflow",
        "Style cleanup using specialized agents",
    ] {
        assert!(transcript.contains(description), "{description}");
    }
    assert_eq!(
        t.ui.settings().selected_skills,
        SkillSelection::Names(strings(&["leanck"]))
    );
    t.choose(STANDALONE_PROMPT, "Edit advanced config...");
    t.focus("leanck");
    assert!(t.screen().contains("Minimal leanck command reference"));
    t.set_enum("lean-style", "yes");
    assert!(
        t.screen()
            .contains("Style cleanup using specialized agents")
    );
    t.action("Save changes");
    t.choose(STANDALONE_PROMPT, "Continue");
    let plan = t.confirmed();
    assert_eq!(
        plan.settings.selected_skills,
        SkillSelection::Names(strings(&["lean-style", "leanck"]))
    );
    for dir in [home("claude/skills"), home(".agents/skills")] {
        for (skill, installed) in [
            ("leanck", true),
            ("lean-style", true),
            ("lean-proving", false),
        ] {
            assert_eq!(
                plan.steps.iter().any(|step| matches!(step, Step::Tree { destination, .. } if destination == &dir.join(skill))),
                installed,
                "{skill}"
            );
        }
    }
}

#[test]
fn plain_terminal_can_select_only_the_minimal_skill() {
    let f = Fixture::new();
    let (effect, out) = f.plain(
        &[],
        &[
            "standalone",
            "no",
            "codex",
            "no",
            "no",
            "yes",
            "none",
            "",
            "continue",
        ],
        Style::PLAIN,
    );
    let Effect::Confirmed(plan) = effect else {
        panic!("{effect:?}\n{out}");
    };
    assert!(!out.contains('\x1b'));
    assert!(!plan.settings.cli);
    let actions = plan.actions();
    let installs: Vec<&String> = actions
        .iter()
        .filter(|a| a.contains(".agents/skills"))
        .collect();
    assert_eq!(
        installs,
        [&format!(
            "Install skills into {}",
            home(".agents/skills").display()
        )]
    );
}

// Approved fixes to `install.sh` quirks.

#[test]
fn the_standalone_path_dialog_matches_plugin_mode() {
    let f = Fixture::new()
        .var("SHELL", "/bin/tcsh")
        .file(home("config/fish/config.fish"));
    let mut t = f.ui(&[]);
    t.choose("Install as plugin or standalone?", "Standalone");
    t.prompt("Install the CLI?", "");
    t.prompt("Install location", "");
    t.prompt("Standalone skills", "");
    t.prompt("Custom agents", "");
    t.prompt("Shell completions (", "");
    t.prompt("Add leanck to PATH?", "yes");
    t.prompt("Shell for PATH (", "fish");
    t.choose(STANDALONE_PROMPT, "Continue");
    let plan = t.confirmed();
    assert_eq!(plan.settings.path_file, home("config/fish/config.fish"));
    assert!(plan.steps.iter().any(|s| matches!(s, Step::Profile { .. })));
}

#[test]
fn conflict_answers_are_remembered_across_replans() {
    let skill = home("claude/skills/lean-proving");
    let f = Fixture::new().dir(&skill);
    let mut t = f.ui(&[]);
    t.standalone_config("", "claude", &[], "", "none", "");
    t.prompt("Keep it, back up and replace", "keep");
    t.choose(STANDALONE_PROMPT, "Edit advanced config...");
    t.set_enum("Install agents", "claude");
    t.action("Save changes");
    // The re-plan reuses the earlier answer instead of asking again.
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    assert_eq!(t.count("already exists or has local edits"), 1);
    assert!(
        t.plan()
            .actions()
            .contains(&format!("Keep {} unchanged", skill.display()))
    );
    assert!(has(
        t.plan(),
        &format!(
            "Install Claude Code agents into {}",
            home("claude/agents").display()
        )
    ));
}

#[test]
fn invalid_text_reprompts_instead_of_aborting() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.choose("Install as plugin or standalone?", "Standalone");
    t.prompt("Install the CLI?", "maybe");
    assert!(t.transcript().contains("[WARN] Enter yes or no."));
    assert_eq!(t.ui.buffer(), "", "the prompt starts over");
    t.prompt("Install the CLI?", "yes");
    t.prompt("Install location", "");
    t.prompt("Standalone skills", "all of them");
    assert!(
        t.transcript()
            .contains("[WARN] Choose none, claude, codex, or both.")
    );
    t.prompt("Standalone skills", "");
    t.prompt("Custom agents", "");
    t.prompt("Shell completions (", "tcsh");
    assert!(
        t.transcript()
            .contains("[WARN] Choose none, bash, zsh, fish, powershell, or elvish.")
    );
    t.prompt("Shell completions (", "none");
    t.prompt("Add leanck to PATH?", "");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));

    // Control characters cannot be typed in the terminal UI; line mode re-asks.
    let (effect, out) = f.plain(
        &[],
        &[
            "standalone",
            "",
            "bad\u{1}dir",
            "/opt/bin",
            "",
            "",
            "",
            "",
            "cancel",
        ],
        Style::PLAIN,
    );
    assert_eq!(effect, Effect::Cancelled);
    assert!(out.contains("[WARN] values must be single-line text without control characters"));
    assert_eq!(out.matches("Install location [").count(), 2);
    let (effect, out) = f.plain(&[], &["sideways", "plugin"], Style::PLAIN);
    assert_eq!(effect, Effect::Interrupted, "input ends after the dialog");
    assert!(out.contains("[WARN] Enter 1-2 or plugin or standalone."));
}

#[test]
fn the_advanced_form_keeps_all_when_every_skill_is_selected() {
    let f = Fixture::new();
    let mut t = f.ui(&[
        "--skills",
        "claude",
        "--skill",
        "lean-proving",
        "--skill",
        "lean-style",
    ]);
    t.standalone_config("", "claude", &[], "", "none", "");
    assert_eq!(
        t.ui.settings().selected_skills,
        SkillSelection::Names(strings(&["lean-proving", "lean-style"]))
    );
    t.choose(STANDALONE_PROMPT, "Edit advanced config...");
    t.set_enum("leanck", "yes");
    assert_eq!(t.form().draft().selected_skills, SkillSelection::All);
    t.set_enum("lean-style", "no");
    t.set_enum("lean-style", "yes");
    assert_eq!(t.form().draft().selected_skills, SkillSelection::All);
    t.action("Save changes");
    assert_eq!(t.ui.settings().selected_skills, SkillSelection::All);
}

#[test]
fn skill_selection_with_an_interactive_plugin_choice_is_an_error() {
    let f = Fixture::new();
    let mut t = f.ui(&["--skill", "leanck"]);
    assert_eq!(t.ui.menu_selection(), Some("Standalone"));
    let effect = t.choose("Install as plugin or standalone?", "Plugin (recommended)");
    assert_eq!(
        effect,
        Effect::Failed(
            "plugin mode includes all skills; use --standalone with --skills HOST to select individual skills"
                .into()
        )
    );
}

#[test]
fn standalone_mode_asks_again_until_something_is_selected() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.choose("Install as plugin or standalone?", "Standalone");
    t.prompt("Install the CLI?", "no");
    t.prompt("Standalone skills", "");
    t.prompt("Custom agents", "");
    assert!(
        t.transcript()
            .contains("[WARN] Select an executable, skills, or custom agents before continuing.")
    );
    assert_eq!(t.plans, 0);
    t.prompt("Install the CLI?", "no");
    t.prompt("Standalone skills", "codex");
    for _ in 0..3 {
        t.prompt("Install ", "");
    }
    t.prompt("Custom agents", "");
    t.prompt("Codex skills directory", "");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));

    // The form refuses to save an empty selection.
    t.choose(STANDALONE_PROMPT, "Edit advanced config...");
    t.set_enum("Install skills", "none");
    t.action("Save changes");
    assert_eq!(
        t.form().message(),
        Some("Select an executable, skills, or custom agents before continuing.")
    );
}

#[test]
fn a_failed_replan_keeps_the_last_good_configuration() {
    let f = Fixture::new().file("/root/a file");
    let mut t = f.ui(&[]);
    t.plugin_config("bash", "no");
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    t.edit("Completions directory", "/root/a file");
    t.action("Save changes");
    assert!(t.transcript().contains(
        " [ERR]: completions directory is not a directory: /root/a file; choose another path in advanced config or with --completions-dir; the advanced config changes were not applied"
    ));
    assert_eq!(t.ui.settings().completions_dir, None);
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    t.choose(PLUGIN_PROMPT, "Continue");
    assert_eq!(t.confirmed().settings.completions_dir, None);
}

#[test]
fn a_cancelled_conflict_prompt_interrupts() {
    let skill = home("claude/skills/lean-proving");
    let f = Fixture::new().dir(&skill);
    let mut t = f.ui(&[]);
    t.standalone_config("", "claude", &[], "", "none", "");
    t.prompt("Keep it, back up and replace", "sure");
    assert_eq!(
        t.count("[WARN]"),
        1,
        "invalid conflict replies re-ask silently"
    );
    assert_eq!(
        t.prompt("Keep it, back up and replace", "cancel"),
        Effect::Interrupted
    );
}

#[test]
fn numeric_menu_reply_selects_an_option_and_invalid_numbers_reprompt() {
    let f = Fixture::new();
    let (effect, out) = f.plain(&[], &["9", "1", "none", "no", "2"], Style::PLAIN);
    let Effect::Confirmed(plan) = effect else {
        panic!("{effect:?}\n{out}");
    };
    assert_eq!(plan.settings.mode, Mode::Plugin);
    let menu = "Install as plugin or standalone?\n  1) Plugin (recommended)\n  2) Standalone\nChoice [1]: ";
    assert_eq!(out.matches(menu).count(), 2);
    assert!(out.contains("[WARN] Enter 1-2 or plugin or standalone."));
    let (effect, out) = f.plain(&[], &["2", "", "", "", "", "none", "no", "2"], Style::PLAIN);
    let Effect::Confirmed(plan) = effect else {
        panic!("{effect:?}\n{out}");
    };
    assert_eq!(plan.settings.mode, Mode::Standalone);
}

#[test]
fn inline_widgets_leave_answered_questions_in_the_incremental_transcript() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    assert_eq!(t.ui.height(100), 3);
    assert!(t.transcript().is_empty());
    t.choose("Install as plugin or standalone?", "Plugin (recommended)");
    assert_eq!(t.ui.height(100), 1);
    assert_eq!(
        t.transcript(),
        "Install as plugin or standalone? Plugin (recommended)\n"
    );
    assert!(!t.screen().contains("Install as plugin or standalone?"));
    assert!(t.ui.height(20) > 1, "long labels wrap in narrow viewports");
    t.prompt("Shell completions (", "none");
    assert!(t.transcript().contains("[zsh]: none\n"));
    assert!(!t.screen().contains("Shell completions"));
    assert!(
        t.ui.take_transcript(Style::PLAIN).is_empty(),
        "already printed entries are drained"
    );
    t.prompt("Add leanck to PATH?", "no");
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    assert_eq!(t.ui.height(100), t.form().fields().len() as u16 + 4);
    let height = t.ui.height(100);
    t.set_enum("Register plugins", "none");
    assert!(
        t.ui.height(100) < height,
        "form height follows visible fields"
    );
    let transcript = t.transcript();
    t.action("Save changes");
    let warnings: String = t
        .plan()
        .warnings
        .iter()
        .map(|w| format!("[WARN] {w}\n"))
        .collect();
    assert_eq!(
        &t.transcript()[transcript.len()..],
        format!("{}{warnings}", t.plan().review(Style::PLAIN)),
        "saving the form prints only the new plan and its warnings"
    );
    t.choose(PLUGIN_PROMPT, "Cancel");
    assert_eq!(t.ui.height(100), 0);
}

#[test]
fn the_plan_is_shown_before_confirmation_and_after_changed_edits_only() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.plugin_config("none", "no");
    assert_eq!(t.count("Install Plan: leanck v1.2.3"), 1);
    assert!(
        t.transcript()
            .contains("Install leanck into /home/user/data/leanck")
    );
    // Closing the form unchanged keeps the plan already shown above.
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    t.action("Cancel");
    assert_eq!(t.count("Install Plan: leanck v1.2.3"), 1);
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    t.set_enum("Enable MCP", "yes");
    t.action("Save changes");
    assert_eq!(t.count("Install Plan: leanck v1.2.3"), 2);
    t.choose(PLUGIN_PROMPT, "Cancel");

    let (effect, out) = f.plain(&[], &["", "none", "no", "cancel"], Style::PLAIN);
    assert_eq!(effect, Effect::Cancelled);
    let review = out.find("Install Plan: leanck v1.2.3").expect("plan");
    assert!(review < out.find(PLUGIN_PROMPT).unwrap());
}

#[test]
fn standalone_form_lists_component_directories_and_resets_derived_defaults() {
    let f = Fixture::new();
    let mut t = f.ui(&["--claude-agents-dir", "/custom/agents"]);
    t.standalone_config("", "both", &[], "both", "none", "no");
    t.choose(STANDALONE_PROMPT, "Edit advanced config...");
    let rows: Vec<_> = t
        .form()
        .fields()
        .iter()
        .map(|field| {
            (
                t.form().label(*field).trim().to_owned(),
                t.form().value(*field),
            )
        })
        .collect();
    for (label, value) in [
        ("Claude skills directory", "~/claude/skills"),
        ("Claude agents directory", "/custom/agents"),
        ("Codex skills directory", "~/.agents/skills"),
        ("Codex agents directory", "~/codex/agents/leanck"),
    ] {
        assert!(rows.contains(&(label.into(), value.into())), "{rows:?}");
    }
    assert!(
        !rows
            .iter()
            .any(|(label, _)| label == "Claude configuration")
    );
    t.edit(
        "Claude agents directory",
        &home("claude/agents").display().to_string(),
    );
    assert_eq!(t.form().draft().claude_agents_dir, None);
    t.edit("Claude skills directory", "/custom/skills");
    assert_eq!(
        t.form().draft().claude_skills_dir.as_deref(),
        Some(Path::new("/custom/skills"))
    );
    t.edit(
        "Claude skills directory",
        &home("claude/skills").display().to_string(),
    );
    assert_eq!(t.form().draft().claude_skills_dir, None);
}

#[test]
fn standalone_prompt_sequence_uses_claude_component_directories() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.choose("Install as plugin or standalone?", "Standalone");
    let prompts = [
        ("Install the CLI?", ""),
        ("Install location", ""),
        ("Standalone skills", "both"),
        ("Install lean-proving (", ""),
        ("Install lean-style (", ""),
        ("Install leanck (", ""),
        ("Custom agents", "both"),
        ("Claude skills directory", "/x/skills"),
        ("Codex skills directory", ""),
        ("Claude agents directory", "/y/agents"),
        ("Codex agents directory", ""),
        ("Shell completions (", "none"),
        ("Add leanck to PATH?", "no"),
    ];
    for (prefix, reply) in prompts {
        t.prompt(prefix, reply);
    }
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    assert_eq!(t.plan().settings.claude_skills(), Path::new("/x/skills"));
    assert_eq!(t.plan().settings.claude_agents(), Path::new("/y/agents"));
}

#[test]
fn standalone_skips_path_question_after_choosing_a_directory_on_path() {
    let f = Fixture::new().var("PATH", "/chosen/bin:/usr/bin:/bin");
    let mut t = f.ui(&[]);
    t.choose("Install as plugin or standalone?", "Standalone");
    t.prompt("Install the CLI?", "");
    t.prompt("Install location", "/chosen/bin");
    t.prompt("Standalone skills", "none");
    t.prompt("Custom agents", "none");
    t.prompt("Shell completions (", "none");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    assert!(!t.transcript().contains("Add leanck to PATH?"));
    assert!(!t.plan().settings.add_to_path);
    assert!(
        !t.plan()
            .steps
            .iter()
            .any(|step| matches!(step, Step::Profile { .. }))
    );
    assert!(
        !t.plan()
            .warnings
            .iter()
            .any(|warning| warning.contains(NO_PATH_WARNING))
    );
}

#[test]
fn plugin_skips_path_question_when_binary_directory_is_on_path() {
    let bin = home("data/leanck/bin").display().to_string();
    let f = Fixture::new().var("PATH", &format!("{bin}:/usr/bin:/bin"));
    let mut t = f.ui(&[]);
    t.choose("Install as plugin or standalone?", "Plugin (recommended)");
    t.prompt("Shell completions (", "none");
    assert_eq!(t.ui.menu_selection(), Some("Cancel"));
    assert!(!t.transcript().contains("Add leanck to PATH?"));
    assert!(!t.plan().settings.add_to_path);
    assert!(
        !t.plan()
            .steps
            .iter()
            .any(|step| matches!(step, Step::Profile { .. }))
    );
    assert!(
        !t.plan()
            .warnings
            .iter()
            .any(|warning| warning.contains(NO_PATH_WARNING))
    );
    t.choose(PLUGIN_PROMPT, "Edit advanced config...");
    assert!(t.form().fields().contains(&Field::AddToPath));
    t.set_enum("Add leanck to PATH", "yes");
    t.action("Save changes");
    assert!(t.plan().settings.add_to_path);
}

#[test]
fn path_question_is_asked_when_binary_directory_is_not_on_path() {
    let f = Fixture::new();
    let mut t = f.ui(&[]);
    t.choose("Install as plugin or standalone?", "Plugin (recommended)");
    t.prompt("Shell completions (", "none");
    assert!(
        matches!(t.question(), Question::Prompt { label, .. } if label == "Add leanck to PATH?")
    );
}
