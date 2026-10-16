//! Plan-layer tests: options, an environment snapshot, saved state, and a
//! release inventory in; preview lines, warnings, steps, and errors out.
//!
//! Each test is named after the `test-support/test_installer.py` test whose
//! planning behavior it covers (installer spec §10, layer P). Apply, UI,
//! bootstrap, and end-to-end parts of those tests belong to later layers.
//!
//! | §10 | Python test (`test_installer.py`)                                              | Rust test |
//! |-----|---------------------------------------------------------------------------------|-----------|
//! | 1   | test_local_repo_preview_uses_source_inventory_without_network_or_build          | local_repo_preview_uses_source_inventory_without_network_or_build |
//! | 2   | test_local_repo_requires_complete_checkout_and_rejects_release_version          | local_repo_requires_complete_checkout_and_rejects_release_version |
//! | 5   | test_local_repo_components_without_cli_skip_compilation                         | local_repo_components_without_cli_skip_compilation |
//! | 8   | test_local_repo_ignores_remembered_release_pin                                  | local_repo_ignores_remembered_release_pin |
//! | 11  | test_offline_terminal_preview_can_handle_conflicts_without_changes              | offline_terminal_preview_can_handle_conflicts_without_changes |
//! | 13  | test_noninteractive_requires_explicit_approval_even_with_force                  | noninteractive_requires_explicit_approval_even_with_force |
//! | 14  | test_json_preview_and_errors_are_valid_and_keep_stdout_clean                    | json_preview_and_errors_are_valid_and_keep_stdout_clean |
//! | 27  | test_advanced_data_directory_edit_moves_default_binary_without_writing_on_cancel | advanced_data_directory_edit_moves_default_binary_without_writing_on_cancel |
//! | 37  | test_saved_choices_and_flags_override_them                                      | saved_choices_and_flags_override_them |
//! | 38  | test_locally_edited_managed_files_require_force_and_are_backed_up               | locally_edited_managed_files_require_force_and_are_backed_up |
//! | 39  | test_completion_and_path_preview_and_idempotent_install                         | completion_and_path_preview_and_idempotent_install |
//! | 40  | test_all_supported_completion_shells_use_correct_destinations_and_arguments     | all_supported_completion_shells_use_correct_destinations_and_arguments |
//! | 41  | test_completion_defaults_follow_supported_current_shell_or_none                 | completion_defaults_follow_supported_current_shell_or_none |
//! | 42  | test_plugin_prompts_default_to_current_shell_and_adding_to_path                 | plugin_prompts_default_to_current_shell_and_adding_to_path |
//! | 43  | test_missing_default_shell_paths_warn_and_skip_without_prompting_or_creating    | missing_default_shell_paths_warn_and_skip_without_prompting_or_creating |
//! | 44  | test_missing_custom_shell_paths_are_skipped_in_preview_and_install              | missing_custom_shell_paths_are_skipped_in_preview_and_install |
//! | 46  | test_shell_updates_skip_only_the_missing_destination                            | shell_updates_skip_only_the_missing_destination |
//! | 47  | test_completion_directory_must_be_a_directory_before_installing                 | completion_directory_must_be_a_directory_before_installing |
//! | 50  | test_explicit_and_saved_completion_choices_override_detected_default            | explicit_and_saved_completion_choices_override_detected_default |
//! | 51  | test_custom_completion_directory_is_previewed_and_remembered                    | custom_completion_directory_is_previewed_and_remembered |
//! | 52  | test_additional_locations_are_repeatable_remembered_and_rolled_back             | additional_locations_are_repeatable_remembered_and_rolled_back |
//! | 54  | test_codex_home_and_explicit_config_override_remembered_destination             | codex_home_and_explicit_config_override_remembered_destination |
//! | 55  | test_custom_codex_agent_directory_is_independent_of_host_config                 | custom_codex_agent_directory_is_independent_of_host_config |
//! | 63  | test_multiline_values_and_malformed_profile_are_rejected                        | multiline_values_and_malformed_profile_are_rejected |
//! | 66  | test_plugin_mcp_flags_override_saved_choice_and_replace_definition_names        | plugin_mcp_flags_override_saved_choice_and_replace_definition_names |
//! | 71  | test_no_path_warning_in_preview_and_quiet_install                               | no_path_warning_in_preview_and_quiet_install |
//! | 72  | test_no_path_warning_refreshes_after_advanced_config_edit                       | no_path_warning_refreshes_after_advanced_config_edit |
//! | 73  | test_plugin_default_without_detected_hosts_uses_local_share                     | plugin_default_without_detected_hosts_uses_local_share |
//! | 81  | test_individual_skills_apply_to_hosts_extra_locations_and_saved_choices        | individual_skills_apply_to_hosts_extra_locations_and_saved_choices |
//! | 82  | test_skill_flags_override_saved_choices_and_can_restore_all                     | skill_flags_override_saved_choices_and_can_restore_all |
//! | 83  | test_invalid_skill_selections_fail_without_installing                           | invalid_skill_selections_fail_without_installing |
//! | 86  | test_dry_run_all_preserves_existing_files_and_never_executes_hosts_or_binary    | dry_run_all_preserves_existing_files_and_never_executes_hosts_or_binary |
//! | 87  | test_dry_run_standalone_components_with_force_preserves_foreign_files           | dry_run_standalone_components_with_force_preserves_foreign_files |
//! | 89  | test_dry_run_custom_destinations_are_not_created                                | dry_run_custom_destinations_are_not_created |
//! | 95  | test_dry_run_404_still_checks_conflicting_files                                 | dry_run_404_still_checks_conflicting_files |
//! | 96  | test_all_registers_plugins_and_installs_codex_agents_without_duplicate_skills   | all_registers_plugins_and_installs_codex_agents_without_duplicate_skills |
//! | 97  | test_conflicting_skill_aborts_before_installing_cli_and_force_replaces_it       | conflicting_skill_aborts_before_installing_cli_and_force_replaces_it |
//! | 98  | test_foreign_bundle_is_protected_even_with_force                                | foreign_bundle_is_protected_even_with_force |
//! | 99  | test_custom_agents_preserve_unrelated_files_and_require_force                   | custom_agents_preserve_unrelated_files_and_require_force |
//! | 102 | test_invalid_arguments_fail_without_network                                     | invalid_arguments_fail_without_network |
//!
//! Tests after the table cover the approved fixes to `install.sh` quirks.
use super::*;
use std::{
    collections::BTreeMap,
    ffi::OsString,
    path::{Path, PathBuf},
};

const HOME: &str = "/home/user with spaces";
const CWD: &str = "/work";
const ROOT: &str = "/tmp/case";

/// The Python suite's environment: XDG bases under HOME, `SHELL=/bin/zsh`,
/// host homes from the environment, and both host CLIs on PATH.
struct Fixture {
    vars: BTreeMap<String, OsString>,
    commands: Vec<&'static str>,
    files: FileSnapshot,
    saved: SavedState,
    inventory: ReleaseInventory,
}

fn home(relative: &str) -> PathBuf {
    Path::new(HOME).join(relative)
}
fn root(relative: &str) -> PathBuf {
    Path::new(ROOT).join(relative)
}
fn strings(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|s| s.to_string()).collect()
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
            commands: vec!["claude", "codex"],
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
    fn var(&mut self, key: &str, value: Option<&str>) -> &mut Self {
        match value {
            Some(value) => self.vars.insert(key.into(), value.into()),
            None => self.vars.remove(key),
        };
        self
    }
    fn env(&self) -> Environment {
        let mut env = Environment::from_vars(&self.vars, PathBuf::from(CWD)).unwrap();
        env.commands = self.commands.iter().map(|c| c.to_string()).collect();
        env.target = "x86_64-unknown-linux-musl".into();
        env.files = self.files.clone();
        env
    }
    fn args(argv: &[&str]) -> InstallArgs {
        InstallArgs::parse_from(argv).unwrap_or_else(|e| panic!("{argv:?}: {e}"))
    }
    fn settings(&self, argv: &[&str]) -> Settings {
        let env = self.env();
        let mut settings = Settings::resolve(&Self::args(argv), &env, &self.saved).unwrap();
        settings.apply_mode(settings.mode).unwrap();
        settings
    }
    fn plan(&self, argv: &[&str]) -> Result<Plan, PlanFailure> {
        let args = InstallArgs::parse_from(argv).map_err(|e| PlanFailure {
            error: PlanError::Invalid(parse_error_message(&e).unwrap_or_default()),
            actions: vec![],
            warnings: vec![],
        })?;
        plan(&args, &self.env(), &self.saved, &self.inventory)
    }
    fn plan_settings(&self, settings: &Settings, answers: &Answers) -> Result<Plan, PlanFailure> {
        plan_settings(settings, &self.env(), &self.saved, &self.inventory, answers)
    }
    fn ok(&self, argv: &[&str]) -> Plan {
        self.plan(argv)
            .unwrap_or_else(|e| panic!("{argv:?}: {e}; actions: {:#?}", e.actions))
    }
    fn err(&self, argv: &[&str]) -> String {
        match self.plan(argv) {
            Ok(plan) => panic!("{argv:?} unexpectedly planned: {:#?}", plan.actions()),
            Err(failure) => failure.to_string(),
        }
    }

    // Snapshot builders.
    fn dir(&mut self, path: impl Into<PathBuf>) -> &mut Self {
        self.files.insert(path, Node::Directory);
        self
    }
    fn file(&mut self, path: impl Into<PathBuf>, digest: &str) -> &mut Self {
        self.files.insert(
            path,
            Node::File {
                digest: digest.into(),
                text: None,
            },
        );
        self
    }
    fn text(&mut self, path: impl Into<PathBuf>, text: &str) -> &mut Self {
        self.files.insert(
            path,
            Node::File {
                digest: format!("sha256({text})"),
                text: Some(text.into()),
            },
        );
        self
    }
    /// A tree with a valid marker whose stored digest is `stored`; its
    /// recomputed digest is `actual`.
    fn tree(&mut self, path: &Path, stored: &str, actual: &str) -> &mut Self {
        self.dir(path)
            .text(path.join(".leanck-installed"), "leanck installer v1\n")
            .text(path.join(".leanck-content-sha256"), &format!("{stored}\n"));
        self.files.insert_tree(path, Some(actual.into()));
        self
    }
    fn managed_tree(&mut self, path: &Path) -> &mut Self {
        self.tree(path, "tree", "tree")
    }
    /// An agent copy with sidecars recording `recorded`; its content digest is `actual`.
    fn agent(&mut self, path: &Path, recorded: &str, actual: &str) -> &mut Self {
        self.file(path, actual)
            .text(sidecar(path, "leanck-installed"), "leanck installer v1\n")
            .text(sidecar(path, "leanck-sha256"), &format!("{recorded}\n"))
    }
    fn managed_link(&mut self, path: &Path, target: &Path) -> &mut Self {
        self.files.insert(
            path,
            Node::Symlink {
                target: target.into(),
                resolved: Box::new(Node::File {
                    digest: "agent".into(),
                    text: None,
                }),
            },
        );
        self.text(sidecar(path, "leanck-installed"), "leanck installer v1\n")
            .text(sidecar(path, "leanck-sha256"), "agent\n")
    }

    /// Simulate a successful apply: record state and the installed files.
    fn install(&mut self, argv: &[&str]) -> Plan {
        let plan = self.ok(argv);
        self.remember(&plan);
        plan
    }
    fn remember(&mut self, plan: &Plan) {
        let mut recorded = Recorded {
            installed_version: plan.version.clone(),
            installed_target: "x86_64-unknown-linux-musl".into(),
            ..Recorded::default()
        };
        for step in &plan.steps {
            match step {
                Step::Executable { destination } => {
                    self.file(destination.clone(), "binary");
                    recorded.binary = Some((destination.clone(), "binary".into()));
                }
                Step::Tree { destination, .. } | Step::Bundle { destination, .. } => {
                    self.managed_tree(destination);
                }
                Step::Agent {
                    destination,
                    link: Some(target),
                    ..
                } => {
                    self.managed_link(destination, target);
                }
                Step::Agent { destination, .. } => {
                    self.agent(destination, "agent", "agent");
                }
                Step::Completions { destination, .. } => {
                    self.file(destination.clone(), "completions");
                    recorded.completions = Some((destination.clone(), "completions".into()));
                }
                Step::Profile { file, .. } => {
                    self.file(file.clone(), "profile");
                    self.files.insert_profile(file.clone(), true);
                }
                Step::ConfigExample { destination, .. } => {
                    self.file(destination.clone(), "example");
                }
                Step::MigrateCodexAgent { legacy } => {
                    self.files.insert(legacy.clone(), Node::Missing);
                }
            }
        }
        self.saved = SavedState::parse(&self.saved.render(&plan.settings, &recorded)).unwrap();
    }
}

fn has(plan: &Plan, needle: &str) -> bool {
    plan.actions().iter().any(|a| a.contains(needle))
}
fn installs(plan: &Plan, path: &Path) -> bool {
    plan.steps.iter().any(|step| match step {
        Step::Executable { destination }
        | Step::Tree { destination, .. }
        | Step::Bundle { destination, .. }
        | Step::Agent { destination, .. } => destination == path,
        _ => false,
    })
}

#[test]
fn local_repo_preview_uses_source_inventory_without_network_or_build() {
    let mut f = Fixture::new();
    let source = root("local source with spaces");
    f.text(source.join("crates/leanck-cli/Cargo.toml"), "")
        .dir(source.join("leanck-skills"))
        .dir(source.join("leanck-subagents"));
    f.inventory.tag = "v9.8.7".into();
    f.inventory.skills = strings(&["new-proof"]);
    f.inventory.claude_agents = strings(&["new-expert.md"]);
    f.inventory.codex_agents = strings(&["new-grind.toml"]);
    let source_arg = source.to_str().unwrap();
    let plan = f.ok(&[
        "--from-repo",
        source_arg,
        "--dry-run",
        "--skills",
        "both",
        "--agents",
        "both",
        "--json",
    ]);
    assert_eq!(plan.version, "v9.8.7 (local source)");
    assert_eq!(plan.settings.from_repo.as_ref(), Some(&source));
    for name in ["new-proof", "new-expert", "new-grind"] {
        assert!(has(&plan, name), "{name}: {:#?}", plan.actions());
    }
    let outcome = plan.outcome(Status::Preview, "");
    let json: serde_json::Value = serde_json::from_str(&outcome.to_json_line()).unwrap();
    assert_eq!(json["status"], "preview");
    assert!(json["version"].as_str().unwrap().contains("v9.8.7"));
}

#[test]
fn local_repo_requires_complete_checkout_and_rejects_release_version() {
    let mut f = Fixture::new();
    assert!(
        f.err(&["--from-repo", "/repo", "--version", "1.2.3"])
            .contains("--version selects a published release")
    );
    assert_eq!(
        f.err(&["--from-repo", ROOT, "--dry-run"]),
        format!("not a leanck source checkout: {ROOT}; pass --from-repo /path/to/leanck")
    );
    // A checkout needs its CLI manifest, skills, and agents.
    let source = root("checkout");
    f.text(source.join("crates/leanck-cli/Cargo.toml"), "")
        .dir(source.join("leanck-skills"));
    assert!(
        f.err(&["--from-repo", source.to_str().unwrap()])
            .contains("not a leanck source checkout")
    );
    f.dir(source.join("leanck-subagents"));
    f.ok(&["--from-repo", source.to_str().unwrap()]);
}

#[test]
fn local_repo_components_without_cli_skip_compilation() {
    let mut f = Fixture::new();
    let source = root("source");
    f.text(source.join("crates/leanck-cli/Cargo.toml"), "")
        .dir(source.join("leanck-skills"))
        .dir(source.join("leanck-subagents"));
    let plan = f.ok(&[
        "--from-repo",
        source.to_str().unwrap(),
        "--no-cli",
        "--skills",
        "both",
        "--agents",
        "both",
    ]);
    assert!(
        !plan
            .steps
            .iter()
            .any(|s| matches!(s, Step::Executable { .. } | Step::Completions { .. }))
    );
    assert!(installs(&plan, &home("claude/skills/leanck")));
    assert!(installs(
        &plan,
        &home("codex/agents/leanck/lean-grind.toml")
    ));
    assert!(!has(&plan, "/leanck ("));
}

#[test]
fn local_repo_ignores_remembered_release_pin() {
    let mut f = Fixture::new();
    f.install(&["--standalone", "--version", "1.2.3"]);
    assert_eq!(f.saved.get("version"), Some("1.2.3"));
    let source = root("source");
    f.text(source.join("crates/leanck-cli/Cargo.toml"), "")
        .dir(source.join("leanck-skills"))
        .dir(source.join("leanck-subagents"));
    f.inventory.tag = "v9.8.7".into();
    let plan = f.install(&["--standalone", "--from-repo", source.to_str().unwrap()]);
    assert_eq!(plan.version, "v9.8.7 (local source)");
    assert_eq!(f.saved.get("version"), Some("1.2.3"));
    assert_eq!(f.saved.get("from_repo"), source.to_str());
}

#[test]
fn offline_terminal_preview_can_handle_conflicts_without_changes() {
    let mut f = Fixture::new();
    let skill = home("claude/skills/lean-proving");
    f.dir(&skill);
    let settings = f.settings(&["--standalone", "--skills", "claude", "--dry-run"]);
    let unanswered = f.plan_settings(&settings, &Answers::new()).unwrap_err();
    assert_eq!(unanswered.error, PlanError::Conflict(skill.clone()));
    for (answer, line, should_install) in [
        (
            Answer::Replace,
            format!("Replace {} (backed up first)", skill.display()),
            true,
        ),
        (
            Answer::Keep,
            format!("Keep {} unchanged", skill.display()),
            false,
        ),
    ] {
        let answers = Answers::from([(skill.clone(), answer)]);
        let plan = f.plan_settings(&settings, &answers).unwrap();
        assert!(plan.actions().contains(&line), "{:#?}", plan.actions());
        assert_eq!(installs(&plan, &skill), should_install);
        assert_eq!(plan.skips(&skill), !should_install);
        // Answers survive re-planning after an edit.
        let mut edited = settings.clone();
        edited.mcp_enabled = !edited.mcp_enabled;
        assert!(
            f.plan_settings(&edited, &answers)
                .unwrap()
                .actions()
                .contains(&line)
        );
    }
}

#[test]
fn noninteractive_requires_explicit_approval_even_with_force() {
    for argv in [&[][..], &["--force"], &["--json"]] {
        let args = Fixture::args(argv);
        let interactive = args.interactive(false);
        assert_eq!(
            args.require_approval(interactive).unwrap_err().to_string(),
            "approval is required; rerun in a terminal, use --yes for automation, or --dry-run to preview"
        );
    }
    // --json never prompts, even in a terminal.
    assert!(!Fixture::args(&["--json"]).interactive(true));
    for argv in [&["--yes"][..], &["--dry-run"], &["--json", "--dry-run"]] {
        let args = Fixture::args(argv);
        args.require_approval(args.interactive(false)).unwrap();
    }
    Fixture::args(&[]).require_approval(true).unwrap();
}

#[test]
fn json_preview_and_errors_are_valid_and_keep_stdout_clean() {
    let f = Fixture::new();
    let plan = f.ok(&["--all", "--dry-run", "--json"]);
    let json: serde_json::Value =
        serde_json::from_str(&plan.outcome(Status::Preview, "").to_json_line()).unwrap();
    assert_eq!(json["status"], "preview");
    assert_eq!(json["version"], "v1.2.3");
    assert!(
        json["actions"]
            .as_array()
            .unwrap()
            .iter()
            .any(|a| a.as_str().unwrap().contains("codex plugin add"))
    );
    let message = f.err(&["--json", "--wat"]);
    assert!(message.contains("unknown option"));
    let failed = Outcome {
        status: Status::Failed,
        version: "latest".into(),
        message,
        actions: vec![],
    }
    .to_json_line();
    let json: serde_json::Value = serde_json::from_str(&failed).unwrap();
    assert_eq!(json["status"], "failed");
    assert!(!failed.contains('\x1b'));
}

#[test]
fn advanced_data_directory_edit_moves_default_binary_without_writing_on_cancel() {
    let f = Fixture::new();
    let mut settings = f.settings(&[]);
    assert_eq!(settings.bin_dir, home("data/leanck/bin"));
    let data = root("preview edited data");
    settings.set_data_dir(data.clone());
    assert_eq!(settings.bin_dir, data.join("bin"));
    let plan = f.plan_settings(&settings, &Answers::new()).unwrap();
    assert!(plan.actions().contains(&format!(
        "  Executable: {}",
        data.join("bin/leanck").display()
    )));
    assert!(installs(&plan, &data.join("bundle")));
    // An executable directory chosen separately stays put.
    let mut custom = f.settings(&["--bin-dir", "/opt/bin"]);
    custom.set_data_dir(root("other data"));
    assert_eq!(custom.bin_dir, Path::new("/opt/bin"));
}

#[test]
fn saved_choices_and_flags_override_them() {
    let mut f = Fixture::new();
    let custom = root("saved bin");
    f.install(&[
        "--standalone",
        "--bin-dir",
        custom.to_str().unwrap(),
        "--skills",
        "claude",
    ]);
    let plan = f.ok(&["--standalone", "--dry-run"]);
    assert!(has(&plan, &custom.join("leanck").display().to_string()));
    assert!(installs(&plan, &home("claude/skills/lean-proving")));
    let plan = f.ok(&["--standalone", "--skills", "codex", "--dry-run"]);
    assert!(installs(&plan, &home(".agents/skills/lean-proving")));
    assert!(!installs(&plan, &home("claude/skills/lean-proving")));
}

#[test]
fn locally_edited_managed_files_require_force_and_are_backed_up() {
    let mut f = Fixture::new();
    f.install(&["--standalone", "--skills", "claude", "--agents", "codex"]);
    let skill = home("claude/skills/lean-proving");
    f.tree(&skill, "tree", "edited");
    assert_eq!(
        f.err(&["--standalone"]),
        format!(
            "{} already exists or has local edits; use --force to back up and replace it, or rerun interactively",
            skill.display()
        )
    );
    let plan = f.ok(&["--standalone", "--force"]);
    assert!(
        plan.actions()
            .contains(&format!("Replace {} (backed up first)", skill.display()))
    );
    assert!(plan.replaced.contains(&skill));
    f.remember(&plan);
    let agent = home("codex/agents/leanck/lean-expert.toml");
    f.agent(&agent, "agent", "edited agent");
    assert!(
        f.err(&["--standalone"])
            .starts_with(&agent.display().to_string())
    );
    let plan = f.ok(&["--standalone", "--force"]);
    assert!(plan.steps.iter().any(
        |s| matches!(s, Step::Agent { destination, link: None, .. } if *destination == agent)
    ));
}

#[test]
fn completion_and_path_preview_and_idempotent_install() {
    let mut f = Fixture::new();
    let completions = home("config/fish/completions");
    let profile = home("config/fish/config.fish");
    f.dir(&completions).file(&profile, "profile");
    f.files.insert_profile(&profile, true);
    let plan = f.install(&[
        "--standalone",
        "--shell",
        "fish",
        "--add-to-path",
        "--dry-run",
    ]);
    let script = completions.join("leanck.fish");
    assert!(
        plan.actions()
            .contains(&format!("Install fish completions: {}", script.display()))
    );
    assert!(
        plan.actions()
            .contains(&format!("Add leanck to PATH in {}", profile.display()))
    );
    assert!(plan.actions().contains(&format!(
        "  fish_add_path -- '{}'",
        home(".local/bin").display()
    )));
    // Re-running with remembered choices finds only its own files.
    let again = f.ok(&["--standalone"]);
    assert!(!has(&again, "conflict"));
    assert!(again.steps.contains(&Step::Profile {
        file: profile.clone(),
        line: format!("fish_add_path -- '{}'", home(".local/bin").display()),
    }));
}

#[test]
fn all_supported_completion_shells_use_correct_destinations_and_arguments() {
    for (shell, relative) in [
        ("bash", "data/bash-completion/completions/leanck"),
        ("zsh", "data/zsh/site-functions/_leanck"),
        ("fish", "config/fish/completions/leanck.fish"),
        ("powershell", "config/powershell/Completions/leanck.ps1"),
        ("elvish", "config/elvish/lib/leanck.elv"),
    ] {
        let mut f = Fixture::new();
        let script = home(relative);
        f.dir(script.parent().unwrap());
        let plan = f.install(&["--standalone", "--completions", shell]);
        assert!(plan.steps.contains(&Step::Completions {
            shell: CompletionShell::parse(shell).unwrap(),
            destination: script.clone(),
        }));
        // The default shell (zsh) profile is not created or edited.
        assert!(!plan.steps.iter().any(|s| matches!(s, Step::Profile { .. })));
        let again = f.ok(&["--standalone", "--completions", shell]);
        assert!(!has(&again, "conflict"), "{shell}");
    }
    assert_eq!(
        Fixture::new().err(&["--standalone", "--completions", "unsupported"]),
        "--completions expects bash, zsh, fish, powershell, elvish, or none"
    );
}

#[test]
fn completion_defaults_follow_supported_current_shell_or_none() {
    for (shell, expected) in [
        (Some("/bin/bash"), "bash"),
        (Some("/usr/local/bin/fish"), "fish"),
        (Some("/opt/bin/pwsh"), "powershell"),
        (Some("/bin/tcsh"), "none"),
        (None, "none"),
    ] {
        let mut f = Fixture::new();
        f.var("SHELL", shell);
        let plan = f.ok(&["--standalone", "--dry-run"]);
        assert!(
            config_summary(&plan.settings).contains(&format!("- Shell completions: {expected}\n")),
            "{shell:?}"
        );
    }
}

#[test]
fn plugin_prompts_default_to_current_shell_and_adding_to_path() {
    let mut f = Fixture::new();
    f.dir(home("data/zsh/site-functions"))
        .file(home(".zshrc"), "profile");
    let plan = f.ok(&[]);
    assert_eq!(plan.settings.completions, CompletionShell::Zsh);
    assert!(plan.settings.add_to_path);
    assert!(plan.steps.contains(&Step::Completions {
        shell: CompletionShell::Zsh,
        destination: home("data/zsh/site-functions/_leanck"),
    }));
    assert!(plan.steps.contains(&Step::Profile {
        file: home(".zshrc"),
        line: format!(
            "export PATH='{}':\"$PATH\"",
            home("data/leanck/bin").display()
        ),
    }));
    assert!(plan.warnings.is_empty(), "{:?}", plan.warnings);
}

#[test]
fn missing_default_shell_paths_warn_and_skip_without_prompting_or_creating() {
    for mode in [&["--plugin", "both"][..], &["--standalone"]] {
        let f = Fixture::new();
        let plan = f.ok(&[mode, &["--completions", "zsh", "--add-to-path"]].concat());
        let warnings = plan.warnings.join("\n");
        assert!(warnings.contains(&format!(
            "Completions directory does not exist: {}; skipping shell completions.",
            home("data/zsh/site-functions").display()
        )));
        assert!(warnings.contains(&format!(
            "Shell profile does not exist: {}; skipping PATH update.",
            home(".zshrc").display()
        )));
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
fn missing_custom_shell_paths_are_skipped_in_preview_and_install() {
    let f = Fixture::new();
    let directory = root("missing completions parent/completions");
    let profile = root("missing profile parent/config.fish");
    let plan = f.ok(&[
        "--standalone",
        "--shell",
        "fish",
        "--completions-dir",
        directory.to_str().unwrap(),
        "--path-file",
        profile.to_str().unwrap(),
        "--add-to-path",
        "--dry-run",
    ]);
    assert!(plan.actions().contains(&format!(
        "Skip shell completions: directory missing ({})",
        directory.display()
    )));
    assert!(plan.actions().contains(&format!(
        "Skip PATH update: shell profile missing ({})",
        profile.display()
    )));
    assert!(!has(&plan, "Install fish completions"));
    assert!(!has(&plan, "Add leanck to PATH in"));
}

#[test]
fn shell_updates_skip_only_the_missing_destination() {
    let mut f = Fixture::new();
    let directory = home("config/fish/completions");
    let profile = home("config/fish/config.fish");
    f.dir(&directory);
    let plan = f.install(&["--standalone", "--shell", "fish", "--add-to-path"]);
    assert!(
        plan.steps
            .iter()
            .any(|s| matches!(s, Step::Completions { .. }))
    );
    assert!(
        !plan
            .warnings
            .iter()
            .any(|w| w.contains("skipping shell completions"))
    );
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("skipping PATH update"))
    );
    f.file(&profile, "profile");
    let missing = root("missing completions");
    let plan = f.ok(&[
        "--standalone",
        "--completions-dir",
        missing.to_str().unwrap(),
    ]);
    assert!(plan.steps.iter().any(|s| matches!(s, Step::Profile { .. })));
    assert!(
        plan.warnings
            .iter()
            .any(|w| w.contains("skipping shell completions"))
    );
    assert!(
        !plan
            .warnings
            .iter()
            .any(|w| w.contains("skipping PATH update"))
    );
}

#[test]
fn completion_directory_must_be_a_directory_before_installing() {
    let mut f = Fixture::new();
    let directory = root("completion file");
    f.file(&directory, "keep this file");
    assert_eq!(
        f.err(&[
            "--standalone",
            "--completions-dir",
            directory.to_str().unwrap()
        ]),
        format!(
            "completions directory is not a directory: {}; choose another path in advanced config or with --completions-dir",
            directory.display()
        )
    );
}

#[test]
fn explicit_and_saved_completion_choices_override_detected_default() {
    let mut f = Fixture::new();
    f.install(&["--standalone", "--completions", "none"]);
    let summary = |plan: Plan| config_summary(&plan.settings);
    assert!(summary(f.ok(&["--standalone", "--dry-run"])).contains("- Shell completions: none\n"));
    assert!(
        summary(f.ok(&["--standalone", "--dry-run", "--shell", "bash"]))
            .contains("- Shell completions: bash\n")
    );
    assert!(
        summary(f.ok(&[
            "--standalone",
            "--dry-run",
            "--shell",
            "bash",
            "--completions",
            "fish"
        ]))
        .contains("- Shell completions: fish\n")
    );
    let plugin = summary(f.ok(&[
        "--plugin",
        "codex",
        "--dry-run",
        "--completions",
        "none",
        "--no-add-to-path",
    ]));
    assert!(plugin.contains("- Shell completions: none\n"));
    assert!(plugin.contains("- Add leanck to PATH: no\n"));
}

#[test]
fn custom_completion_directory_is_previewed_and_remembered() {
    let mut f = Fixture::new();
    let directory = root("chosen completions");
    let dir = directory.to_str().unwrap();
    let plan = f.ok(&[
        "--standalone",
        "--completions",
        "bash",
        "--completions-dir",
        dir,
        "--dry-run",
    ]);
    assert!(plan.actions().contains(&format!(
        "Skip shell completions: directory missing ({})",
        directory.display()
    )));
    f.dir(&directory);
    f.install(&[
        "--standalone",
        "--completions",
        "bash",
        "--completions-dir",
        dir,
    ]);
    assert_eq!(f.saved.get("completions_dir"), Some(dir));
    let script = directory.join("leanck");
    f.file(&script, "local completion edits");
    assert!(
        f.err(&["--standalone"])
            .starts_with(&script.display().to_string())
    );
    let plan = f.ok(&["--standalone", "--force"]);
    assert!(
        plan.actions()
            .contains(&format!("Replace {} (backed up first)", script.display()))
    );
    assert!(plan.steps.contains(&Step::Completions {
        shell: CompletionShell::Bash,
        destination: script,
    }));
}

#[test]
fn additional_locations_are_repeatable_remembered_and_rolled_back() {
    let mut f = Fixture::new();
    let [skills, skills2, claude, codex] =
        ["skills one", "skills two", "Claude extras", "Codex extras"].map(root);
    let argv = [
        "--plugin",
        "codex",
        "--extra-skills-dir",
        skills.to_str().unwrap(),
        "--extra-skills-dir",
        skills2.to_str().unwrap(),
        "--extra-skills-dir",
        skills.to_str().unwrap(),
        "--extra-claude-agents-dir",
        claude.to_str().unwrap(),
        "--extra-codex-agents-dir",
        codex.to_str().unwrap(),
    ];
    let plan = f.ok(&[&argv[..], &["--dry-run"]].concat());
    assert_eq!(
        plan.settings.extra_skills,
        [skills.clone(), skills2.clone()]
    );
    for path in [
        skills.join("lean-proving"),
        skills2.join("lean-proving"),
        claude.join("lean-expert.md"),
        codex.join("lean-expert.toml"),
    ] {
        assert!(installs(&plan, &path), "{}", path.display());
    }
    f.install(&argv);
    let plan = f.install(&["--plugin", "codex"]);
    assert_eq!(
        plan.settings.extra_skills,
        [skills.clone(), skills2.clone()]
    );
    let saved = f.saved.render(&plan.settings, &Recorded::default());
    assert_eq!(saved.matches("extra_skills\t").count(), 2);
    // Edited copies are conflicts that --force replaces.
    f.tree(&skills.join("lean-proving"), "tree", "first local edit");
    f.agent(
        &claude.join("lean-expert.md"),
        "agent",
        "custom Claude agent",
    );
    assert!(
        f.err(&["--plugin", "codex"])
            .contains("already exists or has local edits")
    );
    let plan = f.ok(&["--plugin", "codex", "--force"]);
    assert!(plan.replaced.contains(&skills.join("lean-proving")));
    assert!(plan.replaced.contains(&claude.join("lean-expert.md")));
}

#[test]
fn codex_home_and_explicit_config_override_remembered_destination() {
    let mut f = Fixture::new();
    let [remembered, configured, explicit] =
        ["remembered Codex", "environment Codex", "explicit Codex"].map(root);
    f.var("CODEX_HOME", None);
    let link = |plan: &Plan, dir: &Path| {
        plan.steps.iter().any(|s| {
            matches!(s, Step::Agent { destination, link: Some(_), .. }
                if *destination == dir.join("agents/leanck/lean-expert.toml"))
        })
    };
    let plan = f.install(&[
        "--plugin",
        "codex",
        "--codex-dir",
        remembered.to_str().unwrap(),
    ]);
    assert!(link(&plan, &remembered));
    assert!(link(&f.ok(&["--plugin", "codex"]), &remembered));
    f.var("CODEX_HOME", configured.to_str());
    let plan = f.install(&["--plugin", "codex"]);
    assert!(link(&plan, &configured));
    let plan = f.ok(&[
        "--plugin",
        "codex",
        "--codex-dir",
        explicit.to_str().unwrap(),
    ]);
    assert!(link(&plan, &explicit));
}

#[test]
fn custom_codex_agent_directory_is_independent_of_host_config() {
    let mut f = Fixture::new();
    let destination = root("custom Codex agents");
    let plan = f.install(&[
        "--plugin",
        "codex",
        "--codex-agents-dir",
        destination.to_str().unwrap(),
    ]);
    let expected = Step::Agent {
        source: "plugin/codex/agents/lean-expert.toml".into(),
        destination: destination.join("lean-expert.toml"),
        link: Some(home("data/leanck/agents/lean-expert.toml")),
    };
    assert!(plan.steps.contains(&expected));
    assert!(!has(&plan, &home("codex/agents").display().to_string()));
    let again = f.ok(&["--plugin", "codex"]);
    assert!(again.steps.contains(&expected));
    assert!(!has(&again, "conflict"));
}

#[test]
fn multiline_values_and_malformed_profile_are_rejected() {
    let mut f = Fixture::new();
    assert_eq!(
        f.err(&["--standalone", "--bin-dir", "bad\npath"]),
        "values must be single-line text"
    );
    assert_eq!(
        f.err(&["--standalone", "--bin-dir", "bad\tpath"]),
        "values must be single-line text without control characters"
    );
    f.file(home(".zshrc"), "profile");
    f.files.insert_profile(home(".zshrc"), false);
    assert_eq!(
        f.err(&["--standalone", "--add-to-path"]),
        format!(
            "unbalanced leanck PATH markers in {}; repair the block before retrying",
            home(".zshrc").display()
        )
    );
}

#[test]
fn plugin_mcp_flags_override_saved_choice_and_replace_definition_names() {
    let mut f = Fixture::new();
    for (argv, enabled) in [
        (&["--mcp"][..], true),
        (&[], true),
        (&["--no-mcp"], false),
        (&[], false),
    ] {
        let plan = f.install(argv);
        assert!(plan.steps.contains(&Step::Bundle {
            destination: home("data/leanck/bundle"),
            binary: home("data/leanck/bin/leanck"),
            mcp_enabled: enabled,
        }));
        assert!(!has(&plan, "MCP"));
        assert_eq!(
            f.saved.get("mcp_enabled"),
            Some(if enabled { "1" } else { "0" })
        );
    }
}

#[test]
fn no_path_warning_in_preview_and_quiet_install() {
    let warning = "leanck will not be added to PATH; agents may not be able to execute leanck.";
    for mode in ["--plugin", "--standalone"] {
        let mut argv = vec!["--no-add-to-path", "--dry-run"];
        if mode == "--plugin" {
            argv.extend(["--plugin", "both"]);
        } else {
            argv.push(mode);
        }
        let plan = Fixture::new().ok(&argv);
        assert!(plan.warnings.iter().any(|w| w == warning), "{mode}");
    }
    let mut f = Fixture::new();
    f.file(home(".zshrc"), "");
    let profile = home(".zshrc");
    let plan = f.ok(&[
        "--standalone",
        "--add-to-path",
        "--path-file",
        profile.to_str().unwrap(),
        "--dry-run",
    ]);
    assert!(!plan.warnings.iter().any(|w| w == warning));
}

#[test]
fn no_path_warning_refreshes_after_advanced_config_edit() {
    let mut f = Fixture::new();
    f.file(home(".zshrc"), "");
    let path_warnings = |plan: &Plan| {
        plan.warnings
            .iter()
            .filter(|w| w.contains("agents may not be able to execute leanck"))
            .count()
    };
    let mut settings = f.settings(&["--no-add-to-path"]);
    let plan = f.plan_settings(&settings, &Answers::new()).unwrap();
    assert_eq!(path_warnings(&plan), 1);
    settings.add_to_path = true;
    let plan = f.plan_settings(&settings, &Answers::new()).unwrap();
    assert_eq!(path_warnings(&plan), 0, "{:?}", plan.warnings);
}

#[test]
fn plugin_default_without_detected_hosts_uses_local_share() {
    let mut f = Fixture::new();
    f.commands.clear();
    f.var("XDG_DATA_HOME", None);
    let data = home(".local/share/leanck");
    let plan = f.ok(&[]);
    assert!(plan.steps.contains(&Step::Executable {
        destination: data.join("bin/leanck"),
    }));
    assert!(plan.steps.contains(&Step::Bundle {
        destination: data.join("bundle"),
        binary: data.join("bin/leanck"),
        mcp_enabled: false,
    }));
    assert!(plan.steps.contains(&Step::Tree {
        source: "plugin/codex/agents".into(),
        destination: data.join("agents"),
    }));
    assert!(plan.registrations.is_empty());
    assert_eq!(f.env().detected_agents(), "none");
    assert!(
        banner(&plan.version, f.env().detected_agents(), Style::PLAIN)
            .contains("Detected coding agents: none\n")
    );
}

#[test]
fn individual_skills_apply_to_hosts_extra_locations_and_saved_choices() {
    let mut f = Fixture::new();
    let extra = root("extra skills");
    let omitted = home("claude/skills/lean-proving");
    f.dir(&omitted);
    let plan = f.install(&[
        "--no-cli",
        "--skills",
        "both",
        "--skill",
        "leanck",
        "--skill",
        "lean-style",
        "--extra-skills-dir",
        extra.to_str().unwrap(),
    ]);
    for directory in [home("claude/skills"), home(".agents/skills"), extra.clone()] {
        for name in ["leanck", "lean-style"] {
            assert!(installs(&plan, &directory.join(name)));
        }
        assert!(!installs(&plan, &directory.join("lean-proving")));
    }
    assert_eq!(f.saved.get("selected_skills"), Some("leanck lean-style"));
    let plan = f.ok(&["--no-cli", "--skills", "both", "--dry-run"]);
    assert!(config_summary(&plan.settings).contains("- Selected skills: leanck lean-style\n"));
    assert!(!has(&plan, &omitted.display().to_string()));
}

#[test]
fn skill_flags_override_saved_choices_and_can_restore_all() {
    let mut f = Fixture::new();
    let skills = home(".agents/skills");
    let plan = f.install(&["--no-cli", "--skills", "codex", "--skill", "leanck"]);
    let trees: Vec<_> = plan
        .steps
        .iter()
        .filter_map(|s| match s {
            Step::Tree { destination, .. } => Some(destination.clone()),
            _ => None,
        })
        .collect();
    assert_eq!(trees, [skills.join("leanck")]);
    let plan = f.ok(&[
        "--no-cli",
        "--skills",
        "codex",
        "--skill",
        "lean-style",
        "--skill",
        "lean-style",
        "--dry-run",
    ]);
    assert!(config_summary(&plan.settings).contains("- Selected skills: lean-style\n"));
    assert!(!installs(&plan, &skills.join("leanck")));
    let plan = f.ok(&["--no-cli", "--skills", "codex", "--skill", "all"]);
    for name in ["lean-proving", "lean-style", "leanck"] {
        assert!(installs(&plan, &skills.join(name)));
    }
    // `--skill NAME --skill all` selects all; `--skill all --skill NAME` selects NAME.
    assert_eq!(
        f.settings(&["--skills", "codex", "--skill", "leanck", "--skill", "all"])
            .selected_skills,
        SkillSelection::All
    );
    assert_eq!(
        f.settings(&["--skills", "codex", "--skill", "all", "--skill", "leanck"])
            .selected_skills,
        SkillSelection::Names(vec!["leanck".into()])
    );
}

#[test]
fn invalid_skill_selections_fail_without_installing() {
    let f = Fixture::new();
    for (argv, message) in [
        (
            &["--standalone", "--skills", "codex", "--skill", "missing"][..],
            "unknown skill: missing; choose a skill included in this release, or use --skill all",
        ),
        (
            &["--standalone", "--skill", "../leanck"],
            "invalid skill name: ../leanck; use --skill NAME (repeatable), all, or none",
        ),
        (
            &[
                "--standalone",
                "--no-cli",
                "--skills",
                "both",
                "--skill",
                "none",
            ],
            "nothing selected; omit --no-cli or select --skills or --agents",
        ),
        (
            &["--plugin", "codex", "--skill", "leanck"],
            "plugin mode includes all skills; use --standalone with --skills HOST to select individual skills",
        ),
    ] {
        assert_eq!(f.err(argv), message, "{argv:?}");
    }
}

#[test]
fn dry_run_all_preserves_existing_files_and_never_executes_hosts_or_binary() {
    let mut f = Fixture::new();
    let binary = home("data/leanck/bin/leanck");
    f.file(&binary, "previous executable");
    let argv = ["--all", "--skills", "both", "--agents", "both", "--dry-run"];
    // An executable the installer never recorded is a conflict.
    assert!(f.err(&argv).starts_with(&binary.display().to_string()));
    let plan = f.ok(&[&argv[..], &["--force"]].concat());
    assert!(
        plan.actions()
            .contains(&format!("  Executable: {}", binary.display()))
    );
    assert!(
        plan.actions()
            .contains(&"  Replaces the existing executable (backed up first)".to_owned())
    );
    assert!(has(&plan, "  $ claude plugin install"));
    assert!(has(&plan, "  $ codex plugin add"));
    assert!(has(&plan, "Link Codex agents into"));
    assert!(has(&plan, "lean-expert, lean-grind, lean-style-worker"));
    assert!(!has(&plan, "claude/skills/"));
    assert!(!has(&plan, ".agents/skills/"));
    assert!(!has(
        &plan,
        &format!(
            "Install Claude Code agents into {}",
            home("claude/agents").display()
        )
    ));
    assert!(!has(&plan, "Would sign "));
    assert!(plan.actions().contains(&format!(
        "Install configuration example: {}",
        home("config/leanck/config.example.toml").display()
    )));
}

#[test]
fn dry_run_standalone_components_with_force_preserves_foreign_files() {
    let mut f = Fixture::new();
    f.dir(home("claude/skills/lean-proving"))
        .file(home("codex/agents/leanck/lean-expert.toml"), "custom agent");
    let plan = f.ok(&[
        "--no-cli",
        "--skills",
        "both",
        "--agents",
        "both",
        "--force",
        "--dry-run",
        "--version",
        "v1.2.3",
    ]);
    for destination in [
        "claude/skills/lean-proving",
        ".agents/skills/lean-proving",
        "claude/agents/lean-expert.md",
        "codex/agents/leanck/lean-expert.toml",
    ] {
        assert!(installs(&plan, &home(destination)), "{destination}");
    }
    assert!(!has(&plan, "/.local/bin/leanck"));
    assert_eq!(plan.settings.version, "1.2.3");
}

#[test]
fn dry_run_custom_destinations_are_not_created() {
    let f = Fixture::new();
    let [binary_dir, data_dir] = ["custom bin", "custom data"].map(root);
    let plan = f.ok(&[
        "--dry-run",
        "--plugin",
        "claude",
        "--bin-dir",
        binary_dir.to_str().unwrap(),
        "--data-dir",
        data_dir.to_str().unwrap(),
    ]);
    assert!(has(
        &plan,
        &format!(
            "Install executable: {}",
            binary_dir.join("leanck").display()
        )
    ));
    assert!(installs(&plan, &data_dir.join("bundle")));
}

#[test]
fn dry_run_404_still_checks_conflicting_files() {
    let mut f = Fixture::new();
    f.dir(home("claude/skills/lean-proving"));
    assert!(
        f.err(&["--standalone", "--dry-run", "--skills", "claude"])
            .contains("already exists")
    );
    f.ok(&["--standalone", "--dry-run", "--skills", "claude", "--force"]);
}

#[test]
fn all_registers_plugins_and_installs_codex_agents_without_duplicate_skills() {
    let mut f = Fixture::new();
    f.file(home(".zshrc"), "");
    let bundle = home("data/leanck/bundle");
    let plan = f.install(&["--all", "--skills", "both", "--agents", "claude"]);
    assert_eq!(
        plan.registrations,
        [
            Registration {
                host: HostName::Claude,
                home: home("claude"),
                bundle: bundle.clone(),
            },
            Registration {
                host: HostName::Codex,
                home: home("codex"),
                bundle: bundle.clone(),
            },
        ]
    );
    let actions = plan.actions();
    let order: Vec<_> = actions.iter().filter(|a| a.starts_with("  $")).collect();
    assert_eq!(
        order,
        [
            &format!("  $ claude plugin marketplace add \"{}\"", bundle.display()),
            "  $ claude plugin install leanck@leanck --scope user",
            &format!("  $ codex plugin marketplace add \"{}\"", bundle.display()),
            "  $ codex plugin add leanck@leanck",
        ]
    );
    assert!(!has(&plan, &home(".agents/skills").display().to_string()));
    assert!(!has(&plan, &home("claude/skills").display().to_string()));
    assert!(!has(&plan, &home("claude/agents").display().to_string()));
    let links = plan
        .steps
        .iter()
        .filter(|s| matches!(s, Step::Agent { link: Some(_), .. }))
        .count();
    assert_eq!(links, 3);
    assert!(has(&plan, "data/leanck/bin"));
    assert!(plan.steps.iter().any(|s| matches!(s, Step::Profile { .. })));
    assert!(!has(&f.ok(&["--all"]), "conflict"));
}

#[test]
fn conflicting_skill_aborts_before_installing_cli_and_force_replaces_it() {
    let mut f = Fixture::new();
    let skill = home("claude/skills/lean-proving");
    f.dir(&skill);
    let failure = f.plan(&["--standalone", "--skills", "claude"]).unwrap_err();
    assert_eq!(failure.error, PlanError::Conflict(skill.clone()));
    assert!(!failure.actions.iter().any(|a| a.contains("leanck (")));
    f.install(&["--standalone", "--skills", "claude", "--force"]);
    assert!(!has(
        &f.ok(&["--standalone", "--skills", "claude"]),
        "conflict"
    ));
}

#[test]
fn foreign_bundle_is_protected_even_with_force() {
    let mut f = Fixture::new();
    let bundle = home("data/leanck/bundle");
    f.dir(&bundle);
    for argv in [
        &["--plugin", "claude"][..],
        &["--plugin", "claude", "--force"],
    ] {
        assert_eq!(
            f.err(argv),
            format!(
                "{} already exists and is not installer-managed; choose another --data-dir",
                bundle.display()
            )
        );
    }
}

#[test]
fn custom_agents_preserve_unrelated_files_and_require_force() {
    let mut f = Fixture::new();
    let agents = home("codex/agents/leanck");
    f.file(agents.join("lean-expert.toml"), "custom")
        .file(agents.join("other.toml"), "unrelated");
    assert!(
        f.err(&["--standalone", "--agents", "codex"])
            .starts_with(&agents.join("lean-expert.toml").display().to_string())
    );
    let plan = f.install(&["--standalone", "--agents", "both", "--force"]);
    assert!(plan.replaced.contains(&agents.join("lean-expert.toml")));
    assert!(!has(&plan, "other.toml"));
    assert!(installs(&plan, &home("claude/agents/lean-expert.md")));
    assert!(!has(
        &f.ok(&["--standalone", "--agents", "both"]),
        "conflict"
    ));
}

#[test]
fn invalid_arguments_fail_without_network() {
    let f = Fixture::new();
    for (argv, message) in [
        (&["--wat"][..], "unknown option: --wat; see --help"),
        (&["--skills"], "--skills needs a value; see --help"),
        (
            &["--skills", "bad"],
            "skills: expected none, claude, codex, or both (got bad)",
        ),
        (
            &["--no-cli"],
            "nothing selected; omit --no-cli or select --skills or --agents",
        ),
        (&["--version", "../../bad"], "invalid release version"),
        (
            &["--release-url", "http://example.com"],
            "--release-url must use HTTPS",
        ),
        (
            &["--signing-identity", "AAAA"],
            "unknown option: --signing-identity; see --help",
        ),
        (
            &["--bin-dir", "--yes"],
            "--bin-dir needs a value; see --help",
        ),
        (
            &["--timeout", "0"],
            "--timeout must be a positive number of seconds",
        ),
    ] {
        assert_eq!(f.err(argv), message, "{argv:?}");
    }
}

// Approved fixes to install.sh quirks (spec §11, items 17-33).

#[test]
fn claude_config_dir_overrides_a_remembered_claude_directory() {
    let mut f = Fixture::new();
    f.install(&[
        "--standalone",
        "--skills",
        "claude",
        "--claude-dir",
        "/remembered",
    ]);
    assert_eq!(f.saved.get("claude_dir"), Some("/remembered"));
    assert_eq!(f.settings(&["--standalone"]).claude_dir, home("claude"));
    f.var("CLAUDE_CONFIG_DIR", None);
    assert_eq!(
        f.settings(&["--standalone"]).claude_dir,
        Path::new("/remembered")
    );
    f.var("CLAUDE_CONFIG_DIR", Some("/environment"));
    assert_eq!(
        f.settings(&["--standalone", "--claude-dir", "/flag"])
            .claude_dir,
        Path::new("/flag")
    );
}

#[test]
fn a_saved_path_opt_out_is_respected_in_plugin_mode() {
    let mut f = Fixture::new();
    assert!(f.settings(&[]).add_to_path);
    f.install(&["--no-add-to-path"]);
    assert!(!f.settings(&[]).add_to_path);
    assert!(f.settings(&["--add-to-path"]).add_to_path);
}

#[test]
fn shell_flags_take_the_basename_and_rederive_defaults() {
    let f = Fixture::new();
    let settings = f.settings(&["--standalone", "--shell", "/usr/local/bin/fish"]);
    assert_eq!(settings.shell, "fish");
    assert_eq!(settings.completions, CompletionShell::Fish);
    assert_eq!(settings.path_file, home("config/fish/config.fish"));
    let mut settings = f.settings(&["--standalone"]);
    settings.set_shell("bash");
    assert_eq!(settings.path_file, home(".bashrc"));
    let mut custom = f.settings(&["--standalone", "--path-file", "/etc/profile.d/x"]);
    custom.set_shell("fish");
    assert_eq!(custom.path_file, Path::new("/etc/profile.d/x"));
}

#[test]
fn timeouts_are_validated_numerically() {
    let f = Fixture::new();
    for value in ["00", "0", "-1", "1.5", "x"] {
        assert_eq!(
            f.err(&["--standalone", "--timeout", value]),
            "--timeout must be a positive number of seconds",
            "{value}"
        );
    }
    assert_eq!(f.settings(&["--standalone", "--timeout", "007"]).timeout, 7);
}

#[test]
fn skill_selection_with_plugin_mode_is_an_error_even_after_choosing_plugin() {
    let f = Fixture::new();
    let env = f.env();
    let mut settings = Settings::resolve(
        &Fixture::args(&["--skills", "codex", "--skill", "leanck"]),
        &env,
        &f.saved,
    )
    .unwrap();
    assert_eq!(settings.mode, Mode::Standalone);
    assert!(
        settings
            .apply_mode(Mode::Plugin)
            .unwrap_err()
            .to_string()
            .contains("plugin mode includes all skills")
    );
}

#[test]
fn the_configuration_example_falls_back_to_the_release_mirror() {
    let mut f = Fixture::new();
    f.inventory.config_example = false;
    let plan = f.ok(&[
        "--standalone",
        "--release-url",
        "https://mirror.example/releases/",
    ]);
    assert!(plan.steps.contains(&Step::ConfigExample {
        destination: home("config/leanck/config.example.toml"),
        source: ExampleSource::Url(
            "https://mirror.example/releases/download/v1.2.3/leanck.example.toml".into()
        ),
    }));
}

#[test]
fn existing_configuration_example_is_replaced() {
    let mut f = Fixture::new();
    f.file(home("config/leanck/config.example.toml"), "mine");
    let plan = f.ok(&["--standalone", "--skills", "both", "--dry-run"]);
    assert!(plan.review(Style::PLAIN).contains(&format!(
        "Install configuration example: {}",
        home("config/leanck/config.example.toml").display()
    )));
    assert!(plan.steps.contains(&Step::ConfigExample {
        destination: home("config/leanck/config.example.toml"),
        source: ExampleSource::Release("docs/leanck.example.toml".into()),
    }));
}

#[test]
fn relative_and_parent_paths_are_normalized_once_at_input() {
    let mut f = Fixture::new();
    f.var("XDG_STATE_HOME", Some("state/../st"));
    let settings = f.settings(&[
        "--standalone",
        "--bin-dir",
        "../bin",
        "--data-dir",
        "~/d/./x/..",
    ]);
    assert_eq!(settings.state_dir, Path::new("/work/st/leanck/installer"));
    assert_eq!(settings.bin_dir, Path::new("/bin"));
    assert_eq!(settings.data_dir, home("d"));
}

#[test]
fn release_api_and_pins_survive_reinstalls_and_selections_keep_shell_keys() {
    let mut f = Fixture::new();
    f.saved = SavedState::parse(
        "mode\tstandalone\nrelease_api\thttps://api.example\nversion\t1.2.3\nbin_dir\t/legacy/bin\n",
    )
    .unwrap();
    let plan = f.install(&["--standalone"]);
    assert_eq!(plan.settings.bin_dir, Path::new("/legacy/bin"));
    assert_eq!(f.saved.get("release_api"), Some("https://api.example"));
    assert_eq!(f.saved.get("version"), Some("1.2.3"));
    assert_eq!(f.saved.get("standalone_bin_dir"), Some("/legacy/bin"));
    let rendered = f.saved.render(&plan.settings, &Recorded::default());
    let keys: Vec<_> = rendered
        .lines()
        .map(|line| line.split('\t').next().unwrap())
        .collect();
    let expected: Vec<_> = SELECTION_KEYS
        .into_iter()
        .filter(|key| !matches!(*key, "claude_skills_dir" | "claude_agents_dir"))
        .collect();
    assert_eq!(keys, expected);
}

#[test]
fn unobserved_paths_are_reported_instead_of_read() {
    let f = Fixture::new();
    let mut env = f.env();
    env.files = FileSnapshot::default();
    let settings = f.settings(&["--standalone", "--skills", "claude"]);
    let failure =
        plan_settings(&settings, &env, &f.saved, &f.inventory, &Answers::new()).unwrap_err();
    let PlanError::Unobserved(probes) = failure.error else {
        panic!("{failure}");
    };
    assert!(probes.contains(&Probe::Node(home("claude/skills/leanck"))));
    assert!(probes.contains(&Probe::Node(home("state/leanck/installer/uninstall.tsv"))));
}

#[test]
fn plans_from_an_observed_filesystem_match_the_snapshot_model() {
    let (_dir, base) = crate::test_support::scratch();
    let home = base.join("home");
    std::fs::create_dir_all(home.join("claude/skills/lean-proving")).unwrap();
    std::fs::write(home.join("claude/skills/lean-proving/custom.txt"), "keep").unwrap();
    let release = super::release::tests::release(&base, "v1.2.3");
    let inventory = ReleaseInventory::read(&release).unwrap();
    let vars: BTreeMap<String, OsString> = [
        ("HOME", home.to_str().unwrap()),
        ("CLAUDE_CONFIG_DIR", home.join("claude").to_str().unwrap()),
        ("SHELL", "/bin/zsh"),
    ]
    .into_iter()
    .map(|(k, v)| (k.into(), v.into()))
    .collect();
    let mut env = Environment::from_vars(&vars, base.clone()).unwrap();
    let saved = SavedState::default();
    let args = Fixture::args(&["--standalone", "--skills", "claude"]);
    let mut settings = Settings::resolve(&args, &env, &saved).unwrap();
    settings.apply_mode(Mode::Standalone).unwrap();
    let failure =
        plan_observed(&settings, &mut env, &saved, &inventory, &Answers::new()).unwrap_err();
    assert_eq!(
        failure.error,
        PlanError::Conflict(home.join("claude/skills/lean-proving"))
    );
    settings.force = true;
    let plan = plan_observed(&settings, &mut env, &saved, &inventory, &Answers::new()).unwrap();
    assert!(
        plan.replaced
            .contains(&home.join("claude/skills/lean-proving"))
    );
    assert!(plan.warnings.iter().any(|w| w.contains("PATH")));
}

#[test]
fn standalone_claude_component_directories_install_and_round_trip() {
    let mut f = Fixture::new();
    let plan = f.install(&[
        "--standalone",
        "--skills",
        "claude",
        "--agents",
        "claude",
        "--claude-skills-dir",
        "/x/skills",
        "--claude-agents-dir",
        "/y/agents",
    ]);
    for name in &f.inventory.skills {
        assert!(installs(&plan, &Path::new("/x/skills").join(name)));
    }
    for name in &f.inventory.claude_agents {
        assert!(installs(&plan, &Path::new("/y/agents").join(name)));
    }
    assert_eq!(f.saved.get("claude_skills_dir"), Some("/x/skills"));
    assert_eq!(f.saved.get("claude_agents_dir"), Some("/y/agents"));
    let restored = f.settings(&["--standalone"]);
    assert_eq!(
        restored.claude_skills_dir.as_deref(),
        Some(Path::new("/x/skills"))
    );
    assert_eq!(
        restored.claude_agents_dir.as_deref(),
        Some(Path::new("/y/agents"))
    );
    assert_eq!(restored.claude_dir, home("claude"));
    let summary = config_summary(&restored);
    assert!(summary.contains("- Claude skills directory: /x/skills\n"));
    assert!(summary.contains("- Claude agents directory: /y/agents\n"));
}

#[test]
fn claude_component_directories_default_to_the_current_parent() {
    let mut f = Fixture::new();
    f.saved = SavedState::parse("claude_dir\t/old\n").unwrap();
    let settings = f.settings(&["--standalone"]);
    assert_eq!(settings.claude_skills_dir, None);
    assert_eq!(settings.claude_agents_dir, None);
    assert_eq!(settings.claude_skills(), home("claude/skills"));
    assert_eq!(settings.claude_agents(), home("claude/agents"));
    let rendered = f.saved.render(&settings, &Recorded::default());
    assert!(!rendered.contains("claude_skills_dir\t"));
    assert!(!rendered.contains("claude_agents_dir\t"));
    let settings = f.settings(&[
        "--standalone",
        "--claude-dir",
        "/new",
        "--claude-skills-dir",
        "/new/skills",
        "--claude-agents-dir",
        "/new/agents",
    ]);
    assert_eq!(settings.claude_skills_dir, None);
    assert_eq!(settings.claude_agents_dir, None);
    assert_eq!(settings.claude_skills(), Path::new("/new/skills"));
    assert_eq!(settings.claude_agents(), Path::new("/new/agents"));
}

#[test]
fn bundled_claude_agents_detail_requires_a_codex_link_and_claude_host() {
    let f = Fixture::new();
    for (argv, expected) in [
        (vec!["--plugin", "both"], true),
        (vec!["--plugin", "codex"], false),
        (vec!["--plugin", "claude"], false),
        (vec!["--plugin", "none"], false),
        (vec!["--standalone", "--agents", "both"], false),
    ] {
        let plan = f.ok(&argv);
        let notice = "Claude Code agents are bundled with the plugin";
        let item = plan
            .items
            .iter()
            .find(|item| item.text.starts_with("Link Codex agents into "));
        assert_eq!(
            item.is_some_and(|item| item.details.iter().any(|detail| detail == notice)),
            expected
        );
        assert_eq!(plan.actions().contains(&format!("  {notice}")), expected);
        if expected {
            assert_eq!(
                item.unwrap().details,
                ["lean-expert, lean-grind, lean-style-worker", notice,]
            );
            assert!(plan.review(Style::PLAIN).contains(concat!(
                "│  ├─ lean-expert, lean-grind, lean-style-worker\n",
                "│  └─ Claude Code agents are bundled with the plugin\n",
            )));
        }
    }
}

#[test]
fn explicit_path_update_is_honored_when_binary_directory_is_on_path() {
    for mode in ["--standalone", "--all"] {
        let mut f = Fixture::new();
        f.var("PATH", Some("/already/bin:/usr/bin:/bin"));
        f.file(home(".zshrc"), "profile");
        let plan = f.ok(&[mode, "--bin-dir", "/already/bin", "--add-to-path"]);
        assert!(plan.settings.bin_on_path());
        assert!(plan.settings.add_to_path);
        assert!(
            plan.steps
                .iter()
                .any(|step| matches!(step, Step::Profile { .. }))
        );
        let rendered = f.saved.render(&plan.settings, &Recorded::default());
        assert!(!rendered.contains("path_entries"));
    }
}
