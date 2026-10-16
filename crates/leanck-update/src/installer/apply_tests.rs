//! Apply-layer tests: real plans applied to temporary directories, with fake
//! host CLIs and daemon maintenance, and fault injection into the journal.
//!
//! Each test is named after the `test-support/test_installer.py` test whose
//! apply behavior it covers (installer spec §10, layer A), or after the
//! uninstall test whose `uninstall.sh` behavior `_uninstall` now owns. The
//! release fixture's `leanck` is a shell script that logs its arguments, so
//! the staged `--version` check and completion generation really run.
//!
//! | §10 | Python test (`test_installer.py`)                                                 | Rust test |
//! |-----|------------------------------------------------------------------------------------|-----------|
//! | 3   | test_local_repo_plugin_builds_and_registers_from_source_without_release_downloads  | local_repo_plugin_builds_and_registers_from_source_without_release_downloads |
//! | 4   | test_local_repo_standalone_installs_built_cli_and_selected_components              | local_repo_standalone_installs_built_cli_and_selected_components |
//! | 5   | test_local_repo_components_without_cli_skip_compilation                            | local_repo_components_without_cli_skip_compilation |
//! | 6   | test_local_repo_build_failure_and_timeout_preserve_existing_installation           | — building moved to the bootstrap (3d); apply failures: fail_on_the_nth_replacement_rolls_back_everything |
//! | 7   | test_local_repo_version_or_inventory_changes_fail_before_installation              | — packaging moved to the bootstrap (3d); the staged version check: release_executable_version_mismatch_installs_nothing |
//! | 15  | test_subcommand_output_has_severity_and_source_with_clean_json                     | subcommand_output_has_severity_and_source_with_clean_json |
//! | 16  | test_no_incompatible_daemons_produces_no_offer_or_static_reminder                  | no_incompatible_daemons_produces_no_offer_or_static_reminder |
//! | 17  | test_yes_stops_only_incompatible_daemons_and_keeps_json_clean                      | yes_stops_only_incompatible_daemons_and_keeps_json_clean |
//! | 18  | test_incompatible_daemon_offer_accepts_or_declines_in_terminal                     | incompatible_daemon_offer_accepts_or_declines_in_terminal |
//! | 19  | test_daemon_maintenance_failures_warn_without_failing_install                      | daemon_maintenance_failures_warn_without_failing_install |
//! | 20  | test_quiet_subcommands_keep_warnings_errors_and_failure_status                     | quiet_subcommands_keep_warnings_errors_and_failure_status |
//! | 21  | test_completion_stdout_is_preserved_while_diagnostics_are_prefixed                 | completion_stdout_is_preserved_while_diagnostics_are_prefixed |
//! | 26  | test_advanced_plugin_edits_apply_only_after_confirmation_and_recheck_conflicts     | advanced_plugin_edits_apply_only_after_confirmation_and_recheck_conflicts |
//! | 33  | test_advanced_plugin_additional_locations_and_completion_destination               | advanced_plugin_additional_locations_and_completion_destination |
//! | 34  | test_plain_advanced_config_can_change_completion_and_profile_paths                 | plain_advanced_config_can_change_completion_and_profile_paths |
//! | 36  | test_advanced_standalone_component_enums_install_selected_hosts                    | advanced_standalone_component_enums_install_selected_hosts |
//! | 38  | test_locally_edited_managed_files_require_force_and_are_backed_up                  | locally_edited_managed_files_require_force_and_are_backed_up |
//! | 39  | test_completion_and_path_preview_and_idempotent_install                            | completion_and_path_preview_and_idempotent_install |
//! | 40  | test_all_supported_completion_shells_use_correct_destinations_and_arguments        | all_supported_completion_shells_use_correct_destinations_and_arguments |
//! | 44  | test_missing_custom_shell_paths_are_skipped_in_preview_and_install                 | missing_custom_shell_paths_are_skipped_in_preview_and_install |
//! | 51  | test_custom_completion_directory_is_previewed_and_remembered                       | custom_completion_directory_is_previewed_and_remembered |
//! | 52  | test_additional_locations_are_repeatable_remembered_and_rolled_back                | additional_locations_are_repeatable_remembered_and_rolled_back |
//! | 53  | test_additional_skills_can_be_installed_without_executable_or_host                 | additional_skills_can_be_installed_without_executable_or_host |
//! | 55  | test_custom_codex_agent_directory_is_independent_of_host_config                    | custom_codex_agent_directory_is_independent_of_host_config |
//! | 56  | test_flat_managed_codex_agents_migrate_but_local_edits_and_unrelated_files_stay    | flat_managed_codex_agents_migrate_but_local_edits_and_unrelated_files_stay |
//! | 57  | test_flat_managed_agent_links_migrate_to_custom_destination_before_source_replacement | flat_managed_agent_links_migrate_to_custom_destination_before_source_replacement |
//! | 58  | test_failed_file_install_rolls_back_prior_replacements                             | failed_file_install_rolls_back_prior_replacements |
//! | 59  | test_force_replaces_a_symlink_without_touching_its_target                          | force_replaces_a_symlink_without_touching_its_target |
//! | 60  | test_terminal_can_preserve_a_conflicting_skill                                     | terminal_can_preserve_a_conflicting_skill |
//! | 61  | test_partial_host_failure_preserves_files_and_reports_json                         | partial_host_failure_preserves_files_and_reports_json |
//! | 62  | test_host_commands_time_out_and_report_partial_installation                        | host_commands_time_out_and_report_partial_installation |
//! | 64  | test_profile_quoting_handles_metacharacters_as_literal_paths                       | profile_quoting_handles_metacharacters_as_literal_paths |
//! | 65  | test_default_plugin_disables_mcp_and_links_bundled_codex_agents                    | default_plugin_disables_mcp_and_links_bundled_codex_agents |
//! | 66  | test_plugin_mcp_flags_override_saved_choice_and_replace_definition_names           | plugin_mcp_flags_override_saved_choice_and_replace_definition_names |
//! | 67  | test_plugin_configuration_runs_bundled_binary_without_python                       | — configuration is the in-process `configure_plugin` |
//! | 68  | test_plugin_configuration_failure_installs_nothing                                 | plugin_configuration_failure_installs_nothing |
//! | 73  | test_plugin_default_without_detected_hosts_uses_local_share                        | plugin_default_without_detected_hosts_uses_local_share |
//! | 74  | test_plugin_can_migrate_managed_standalone_codex_agents                            | plugin_can_migrate_managed_standalone_codex_agents |
//! | 75  | test_plugin_failure_rolls_back_binary_bundle_and_agent_links                       | plugin_failure_rolls_back_binary_bundle_and_agent_links |
//! | 76  | test_configuration_example_honors_xdg_and_preserves_existing_configuration         | configuration_example_honors_xdg_and_preserves_existing_configuration |
//! | 78  | test_configuration_example_preserves_existing_symlink                              | configuration_example_preserves_existing_symlink |
//! | 79  | test_standalone_installs_only_cli_and_can_upgrade                                  | standalone_installs_only_cli_and_can_upgrade |
//! | 80  | test_skills_both_without_cli_and_pinned_version                                    | skills_both_without_cli_and_pinned_version |
//! | 81  | test_individual_skills_apply_to_hosts_extra_locations_and_saved_choices            | individual_skills_apply_to_hosts_extra_locations_and_saved_choices |
//! | 96  | test_all_registers_plugins_and_installs_codex_agents_without_duplicate_skills      | all_registers_plugins_and_installs_codex_agents_without_duplicate_skills |
//! | 97  | test_conflicting_skill_aborts_before_installing_cli_and_force_replaces_it          | conflicting_skill_aborts_before_installing_cli_and_force_replaces_it |
//! | 99  | test_custom_agents_preserve_unrelated_files_and_require_force                      | custom_agents_preserve_unrelated_files_and_require_force |
//! | 104 | test_custom_paths_and_mirror                                                       | custom_paths_and_mirror |
//! | 107 | test_uninstall_standalone_preserves_user_files_and_backups                         | uninstall_standalone_preserves_user_files_and_backups |
//! | 108 | test_uninstall_dry_run_executes_nothing_and_needs_no_approval                      | uninstall_dry_run_executes_nothing_and_needs_no_approval |
//! | 109 | test_uninstall_plugin_unregisters_only_selected_host_and_custom_home               | uninstall_plugin_unregisters_only_selected_host_and_custom_home |
//! | 110 | test_uninstall_preserves_modified_components_and_can_resume                        | uninstall_preserves_modified_components_and_can_resume |
//! | 111 | test_uninstall_tracks_old_and_extra_destinations                                   | uninstall_tracks_old_and_extra_destinations |
//! | 112 | test_uninstall_host_failure_preserves_files_and_retry_receipts                     | uninstall_host_failure_preserves_files_and_retry_receipts |
//! | 113 | test_uninstall_rejects_symlink_parents_and_tree_links                              | uninstall_follows_owned_symlink_parents_and_preserves_tree_links (user-owned parent links are now followed) |
//! | 114 | test_uninstall_legacy_state_and_custom_state_directory                             | uninstall_legacy_state_and_custom_state_directory |
//! | 115 | test_uninstall_invalid_arguments_do_not_change_files                               | uninstall_invalid_arguments_do_not_change_files |
//! | 116 | test_uninstall_cancellation_and_confirmation_use_terminal_not_script_stdin         | uninstall_cancellation_and_confirmation_use_terminal_not_script_stdin (approval only; the terminal is 3d's) |
//! | 117 | test_uninstall_host_timeout_preserves_files                                        | uninstall_host_timeout_preserves_files |
//! | 118 | test_uninstall_legacy_plugins_require_explicit_host_selection                      | uninstall_legacy_plugins_require_explicit_host_selection |
//! | 119 | test_uninstall_legacy_plugins_use_saved_custom_host_directory                      | uninstall_legacy_plugins_use_saved_custom_host_directory |
//! | 120 | test_uninstall_legacy_preserves_retargeted_agent_link                              | uninstall_legacy_preserves_retargeted_agent_link |
//!
//! Tests after the table cover journal fault injection, recovery, and state
//! written by the shell installer.
use super::{apply::apply_failing_at, *};
use crate::{
    gate::Maintenance,
    receipt::{Receipt, digest_file, tree_digest},
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    ffi::OsString,
    fs,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

// Fakes.

#[derive(Default)]
struct FakeHosts {
    calls: RefCell<Vec<HostCall>>,
    /// Hosts whose commands fail (except `--help` probes).
    failing: RefCell<Vec<HostName>>,
    /// Hosts whose commands time out.
    slow: RefCell<Vec<HostName>>,
    /// Hosts that are not on PATH.
    missing: Vec<HostName>,
    /// Print sample output on every call.
    output: bool,
}
impl FakeHosts {
    /// Calls as argv, without `--help` probes.
    fn argv(&self) -> Vec<Vec<String>> {
        self.calls
            .borrow()
            .iter()
            .filter(|c| !c.args.iter().any(|a| a == "--help"))
            .map(|c| {
                std::iter::once(c.host.command().to_owned())
                    .chain(c.args.iter().cloned())
                    .collect()
            })
            .collect()
    }
    fn clear(&self) {
        self.calls.borrow_mut().clear();
    }
}
impl HostCli for FakeHosts {
    fn available(&self, host: HostName) -> bool {
        !self.missing.contains(&host)
    }
    fn run(&self, call: &HostCall) -> Ran {
        self.calls.borrow_mut().push(call.clone());
        let captured = if self.output {
            Captured {
                stdout: b"host stdout\n".to_vec(),
                stderr: b"host stderr\nWarning: host warning\nerror: host error".to_vec(),
            }
        } else {
            Captured::default()
        };
        let probe = call.args.iter().any(|a| a == "--help");
        let result = if self.slow.borrow().contains(&call.host) && !probe {
            Err(Failure::TimedOut {
                seconds: 1,
                command: call.command_line(),
            })
        } else if self.failing.borrow().contains(&call.host) && !probe {
            Err(Failure::Status(Some(1)))
        } else {
            Ok(())
        };
        Ran { captured, result }
    }
}

struct FakeDaemons {
    incompatible: RefCell<Result<usize, DaemonCheck>>,
    stop_fails: RefCell<bool>,
    calls: RefCell<Vec<String>>,
}
impl Default for FakeDaemons {
    fn default() -> Self {
        FakeDaemons {
            incompatible: RefCell::new(Ok(0)),
            stop_fails: RefCell::new(false),
            calls: RefCell::default(),
        }
    }
}
impl Daemons for FakeDaemons {
    fn incompatible(&self, binary: &Path) -> Result<usize, DaemonCheck> {
        self.calls
            .borrow_mut()
            .push(format!("{} daemons list", binary.display()));
        *self.incompatible.borrow()
    }
    fn stop_incompatible(&self, binary: &Path) -> Ran {
        self.calls
            .borrow_mut()
            .push(format!("{} daemons stop --incompatible", binary.display()));
        if *self.stop_fails.borrow() {
            Ran::failed(1)
        } else {
            Ran::ok("")
        }
    }
    fn stop_all(&self, binary: &Path) -> Ran {
        self.calls
            .borrow_mut()
            .push(format!("{} daemons stop --all", binary.display()));
        if *self.stop_fails.borrow() {
            Ran::failed(1)
        } else {
            Ran::ok("")
        }
    }
}

// The release fixture.

fn executable(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// An unpacked release whose `leanck` logs every execution to
/// `ROOT/binary-executions` and fails completion generation while
/// `ROOT/fail-completions` exists.
fn write_release(root: &Path, name: &str, version: &str) -> PathBuf {
    let release = root.join(name).join("leanck");
    let plugin = release.join("plugin");
    let write = |relative: &str, text: &str| {
        let path = release.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, text).unwrap();
    };
    write(".leanck-release", &format!("v{version}\n"));
    write(".leanck-update-protocol", "3\n");
    write("docs/leanck.example.toml", "# Example configuration.\n");
    write(".agents/plugins/marketplace.json", "{}\n");
    write(
        "plugin/.claude-plugin/plugin.json",
        &format!("{{\n  \"name\": \"leanck\",\n  \"version\": \"{version}\"\n}}\n"),
    );
    write(
        "plugin/.codex-plugin/plugin.json",
        &format!(
            "{{\n  \"name\": \"leanck\",\n  \"version\": \"{version}\",\n  \"mcpServers\": \"./.mcp.codex.json\"\n}}\n"
        ),
    );
    let mcp = "{\n  \"mcpServers\": {\n    \"leanck\": {\n      \"command\": \"leanck\",\n      \"args\": [\"serve\", \"mcp\"]\n    }\n  }\n}\n";
    write("plugin/.mcp.json", mcp);
    write("plugin/.mcp.codex.json", mcp);
    for skill in ["lean-proving", "lean-style", "leanck"] {
        write(
            &format!("plugin/skills/{skill}/SKILL.md"),
            &format!("{skill} {version}\n"),
        );
    }
    write(
        "plugin/skills/lean-proving/reference/search-queries.md",
        "queries\n",
    );
    for agent in ["lean-expert", "lean-grind", "lean-style-worker"] {
        write(
            &format!("plugin/agents/{agent}.md"),
            &format!("---\nname: {agent}\n---\n{version}\n"),
        );
        write(
            &format!("plugin/codex/agents/{agent}.toml"),
            &format!("name = \"{agent}\"\ndeveloper_instructions = \"{version}\"\n"),
        );
    }
    executable(&plugin.join("hooks/check.sh"), "#!/bin/sh\nexit 0\n");
    let log = root.join("binary-executions");
    let fail = root.join("fail-completions");
    let diagnostics = root.join("completion-diagnostics");
    executable(
        &plugin.join("bin/leanck"),
        &format!(
            r#"#!/bin/sh
printf 'leanck %s\n' "$*" >> '{log}'
case "$1" in
  --version) echo 'leanck {version}' ;;
  _completions)
    if [ -e '{fail}' ]; then echo 'error: simulated completion failure' >&2; exit 1; fi
    if [ -e '{diagnostics}' ]; then echo 'warning: completion diagnostic' >&2; fi
    if [ "$2" = zsh ]; then printf '#compdef leanck\n'; else printf '# completions for %s\ncomplete -c leanck\n' "$2"; fi ;;
  *) exit 2 ;;
esac
"#,
            log = log.display(),
            fail = fail.display(),
            diagnostics = diagnostics.display(),
        ),
    );
    release
}

struct Case {
    _dir: tempfile::TempDir,
    root: PathBuf,
    home: PathBuf,
    release: PathBuf,
    vars: BTreeMap<String, OsString>,
    commands: Vec<&'static str>,
    hosts: FakeHosts,
    daemons: FakeDaemons,
    /// The answer to the incompatible daemon offer.
    stop_daemons: bool,
    answers: Answers,
}

/// A finished install: the applied result or error, and the console text.
struct Run {
    result: anyhow::Result<Applied>,
    output: String,
}
impl Run {
    fn ok(self) -> String {
        match self.result {
            Ok(_) => self.output,
            Err(e) => panic!("install failed: {e:#}\n{}", self.output),
        }
    }
    fn err(self) -> (String, String) {
        match self.result {
            Ok(applied) => panic!(
                "install unexpectedly succeeded: {applied:?}\n{}",
                self.output
            ),
            Err(e) => (format!("{e:#}"), self.output),
        }
    }
}

impl Case {
    fn new() -> Self {
        // Throwaway directories need no fsync; see `durability`.
        crate::durability::relax_for_this_thread();
        let (dir, root) = crate::test_support::scratch();
        let home = root.join("home");
        fs::create_dir(&home).unwrap();
        let mut vars = BTreeMap::new();
        for (key, value) in [
            ("HOME", home.display().to_string()),
            ("XDG_DATA_HOME", format!("{}/data", home.display())),
            ("XDG_STATE_HOME", format!("{}/state", home.display())),
            ("XDG_CONFIG_HOME", format!("{}/config", home.display())),
            ("SHELL", "/bin/zsh".into()),
            ("CLAUDE_CONFIG_DIR", format!("{}/claude", home.display())),
            ("CODEX_HOME", format!("{}/codex", home.display())),
            ("PATH", "/usr/bin:/bin".into()),
        ] {
            vars.insert(key.to_owned(), OsString::from(value));
        }
        let release = write_release(&root, "release", "1.2.3");
        Case {
            _dir: dir,
            root,
            home,
            release,
            vars,
            commands: vec!["claude", "codex"],
            hosts: FakeHosts::default(),
            daemons: FakeDaemons::default(),
            stop_daemons: true,
            answers: Answers::new(),
        }
    }
    fn home(&self, relative: &str) -> PathBuf {
        self.home.join(relative)
    }
    fn state(&self) -> PathBuf {
        self.home("state/leanck/installer")
    }
    fn state_file(&self, name: &str) -> String {
        fs::read_to_string(self.state().join(name)).unwrap()
    }
    fn executions(&self) -> String {
        fs::read_to_string(self.root.join("binary-executions")).unwrap_or_default()
    }
    fn completion_dir(&self, shell: &str) -> PathBuf {
        let dir = match shell {
            "bash" => self.home("data/bash-completion/completions"),
            "zsh" => self.home("data/zsh/site-functions"),
            "fish" => self.home("config/fish/completions"),
            "powershell" => self.home("config/powershell/Completions"),
            _ => self.home("config/elvish/lib"),
        };
        fs::create_dir_all(&dir).unwrap();
        dir
    }
    fn env(&self) -> Environment {
        let mut env = Environment::from_vars(&self.vars, self.root.clone()).unwrap();
        env.commands = self.commands.iter().map(|c| c.to_string()).collect();
        env.target = "x86_64-unknown-linux-musl".into();
        env
    }
    /// Plan like a non-interactive `_install`, observing the real filesystem.
    fn plan_settings(
        &self,
        argv: &[&str],
        edit: impl FnOnce(&mut Settings),
    ) -> Result<(Plan, ReleaseInventory, Environment), String> {
        let args = InstallArgs::parse_from(argv).map_err(|e| e.to_string())?;
        let mut env = self.env();
        let state = Settings::state_dir(&args, &env).map_err(|e| e.to_string())?;
        let saved = SavedState::load(&state).map_err(|e| e.to_string())?;
        let mut settings = Settings::resolve(&args, &env, &saved).map_err(|e| format!("{e:#}"))?;
        settings
            .validate_noninteractive()
            .map_err(|e| e.to_string())?;
        settings
            .apply_mode(settings.mode)
            .map_err(|e| e.to_string())?;
        edit(&mut settings);
        let inventory = ReleaseInventory::read(&self.release).map_err(|e| e.to_string())?;
        let plan = plan_observed(&settings, &mut env, &saved, &inventory, &self.answers)
            .map_err(|e| e.to_string())?;
        Ok((plan, inventory, env))
    }
    fn run_with(
        &self,
        argv: &[&str],
        edit: impl FnOnce(&mut Settings),
        fault: Option<usize>,
    ) -> Run {
        let (plan, inventory, env) = match self.plan_settings(argv, edit) {
            Ok(planned) => planned,
            Err(message) => {
                return Run {
                    result: Err(anyhow::anyhow!(message)),
                    output: String::new(),
                };
            }
        };
        let quiet = argv.iter().any(|a| *a == "--quiet" || *a == "-q");
        let mut console = Console::new(vec![], Style::PLAIN, quiet);
        let stop = self.stop_daemons;
        let mut confirm = |_| stop;
        let runtime = Runtime {
            hosts: &self.hosts,
            daemons: &self.daemons,
            confirm_daemon_stop: &mut confirm,
        };
        let result = match fault {
            None => apply(&plan, &inventory, &env, runtime, &mut console),
            Some(n) => apply_failing_at(&plan, &inventory, &env, runtime, &mut console, n),
        };
        Run {
            result,
            output: String::from_utf8(console.into_inner()).unwrap(),
        }
    }
    /// Install in standalone mode unless a plugin flag is given, as the
    /// Python suite's `run_install` does.
    fn install(&self, argv: &[&str]) -> Run {
        let plugin = argv
            .iter()
            .any(|a| matches!(*a, "--plugin" | "--all" | "--from-repo"));
        let mut args = vec![];
        if !plugin {
            args.push("--standalone");
        }
        args.extend_from_slice(argv);
        self.run_with(&args, |_| {}, None)
    }
    /// Install exactly `argv` (plugin mode by default).
    fn plugin(&self, argv: &[&str]) -> Run {
        self.run_with(argv, |_| {}, None)
    }
    fn uninstall_args(&self, argv: &[&str]) -> anyhow::Result<UninstallOptions> {
        let args = <UninstallArgs as clap::Parser>::try_parse_from(
            std::iter::once("leanck _uninstall").chain(argv.iter().copied()),
        )?;
        Ok(UninstallOptions::resolve(&args, &self.env())?.0)
    }
    fn uninstall_with(
        &self,
        argv: &[&str],
        approval: Approval,
    ) -> (anyhow::Result<Uninstalled>, String) {
        let options = match self.uninstall_args(argv) {
            Ok(options) => options,
            Err(e) => return (Err(e), String::new()),
        };
        let mut out = vec![];
        let mut approve = || Ok(approval);
        let result = uninstall(
            &options,
            &self.env(),
            &self.hosts,
            &self.daemons,
            &mut approve,
            &mut out,
        );
        (result, String::from_utf8(out).unwrap())
    }
    fn uninstall(&self, argv: &[&str]) -> String {
        let (result, output) = self.uninstall_with(argv, Approval::Approved);
        match result {
            Ok(Uninstalled::Complete | Uninstalled::NothingFound | Uninstalled::Preview) => output,
            other => panic!("uninstall: {other:?}\n{output}"),
        }
    }
    /// Every file and link under the case root, except the release, backups,
    /// and lock files, with content and mode.
    fn snapshot(&self) -> BTreeMap<PathBuf, String> {
        let mut out = BTreeMap::new();
        snapshot(&self.root, &self.root, &mut out);
        out
    }
}

fn snapshot(root: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, String>) {
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        let relative = path.strip_prefix(root).unwrap().to_owned();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        assert!(
            !name.starts_with(".leanck-install.") && !name.starts_with(".leanck-restore-"),
            "staging leftover: {}",
            path.display()
        );
        if relative.starts_with("release")
            || name == "backups"
            || name.ends_with(".lock")
            || name == "binary-executions"
        {
            continue;
        }
        let meta = fs::symlink_metadata(&path).unwrap();
        let mode = meta.permissions().mode() & 0o7777;
        if meta.file_type().is_symlink() {
            out.insert(
                relative,
                format!("link {}", fs::read_link(&path).unwrap().display()),
            );
        } else if meta.is_dir() {
            snapshot(root, &path, out);
        } else {
            out.insert(
                relative,
                format!(
                    "{mode:o} {}",
                    String::from_utf8_lossy(&fs::read(&path).unwrap())
                ),
            );
        }
    }
}

fn mode(path: &Path) -> u32 {
    fs::symlink_metadata(path).unwrap().permissions().mode() & 0o7777
}

fn json(path: &Path) -> serde_json::Value {
    serde_json::from_slice(&fs::read(path).unwrap()).unwrap()
}

fn backup_manifests(case: &Case) -> String {
    let mut text = String::new();
    if let Ok(entries) = fs::read_dir(case.state().join("backups")) {
        for entry in entries {
            text.push_str(
                &fs::read_to_string(entry.unwrap().path().join("manifest.tsv")).unwrap_or_default(),
            );
        }
    }
    text
}

/// A fake source checkout for `--from-repo`.
fn checkout(case: &Case) -> PathBuf {
    let repo = case.root.join("checkout");
    for dir in ["leanck-skills", "leanck-subagents", "crates/leanck-cli"] {
        fs::create_dir_all(repo.join(dir)).unwrap();
    }
    fs::write(
        repo.join("crates/leanck-cli/Cargo.toml"),
        "version = \"9.8.7\"\n",
    )
    .unwrap();
    repo
}

// Tests named after the Python suite.

#[test]
fn local_repo_plugin_builds_and_registers_from_source_without_release_downloads() {
    let mut case = Case::new();
    let repo = checkout(&case);
    // The bootstrap packaged the checkout; `_install` sees a local package.
    case.release = write_release(&case.root, "local-install", "9.8.7");
    let repo_arg = repo.display().to_string();
    let output = case
        .plugin(&["--from-repo", &repo_arg, "--plugin", "codex"])
        .ok();
    assert!(output.contains("(v9.8.7 (local source))"), "{output}");
    let binary = case.home("data/leanck/bin/leanck");
    assert!(
        fs::read_to_string(&binary)
            .unwrap()
            .contains("leanck 9.8.7")
    );
    let bundle = case.home("data/leanck/bundle");
    let mcp = json(&bundle.join("plugin/.mcp.codex.disabled.json"));
    assert_eq!(
        mcp["mcpServers"]["leanck"]["command"],
        binary.to_str().unwrap()
    );
    assert!(
        fs::symlink_metadata(case.home("codex/agents/leanck/lean-grind.toml"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        json(&bundle.join("plugin/.codex-plugin/plugin.json"))["version"],
        "9.8.7"
    );
    assert!(case.hosts.argv().contains(&vec![
        "codex".into(),
        "plugin".into(),
        "add".into(),
        "leanck@leanck".into()
    ]));
    let receipt = json(&case.state().join("receipt.json"));
    assert_eq!(receipt["provenance"], "source");
    assert_eq!(receipt["version"], "9.8.7");
    assert!(
        case.state_file("selections.tsv")
            .contains(&format!("from_repo\t{repo_arg}\n"))
    );
}

#[test]
fn local_repo_standalone_installs_built_cli_and_selected_components() {
    let mut case = Case::new();
    let repo = checkout(&case);
    case.release = write_release(&case.root, "local-install", "9.8.7");
    fs::write(
        case.release.join("docs/leanck.example.toml"),
        "# Local source example.\n",
    )
    .unwrap();
    fs::write(case.root.join("completion-diagnostics"), "").unwrap();
    case.completion_dir("bash");
    let repo_arg = repo.display().to_string();
    let output = case
        .install(&[
            "--from-repo",
            &repo_arg,
            "--standalone",
            "--skills",
            "both",
            "--agents",
            "both",
            "--completions",
            "bash",
        ])
        .ok();
    assert!(output.contains("local source"));
    assert!(output.contains("[INFO] leanck 9.8.7\n"), "{output}");
    assert!(output.contains("[WARN] warning: completion diagnostic\n"));
    assert!(!output.contains("(leanck)"));
    for path in [
        ".local/bin/leanck",
        "claude/skills/lean-proving/SKILL.md",
        ".agents/skills/lean-proving/SKILL.md",
        "claude/agents/lean-expert.md",
        "codex/agents/leanck/lean-grind.toml",
        "data/bash-completion/completions/leanck",
    ] {
        assert!(case.home(path).is_file(), "{path}");
    }
    assert!(!case.home("data/leanck/bundle").exists());
    assert!(case.hosts.calls.borrow().is_empty());
    assert_eq!(
        fs::read_to_string(case.home("config/leanck/config.example.toml")).unwrap(),
        "# Local source example.\n"
    );
}

#[test]
fn local_repo_components_without_cli_skip_compilation() {
    let mut case = Case::new();
    let repo = checkout(&case);
    case.release = write_release(&case.root, "local-install", "9.8.7");
    let repo_arg = repo.display().to_string();
    case.install(&[
        "--from-repo",
        &repo_arg,
        "--standalone",
        "--no-cli",
        "--skills",
        "both",
        "--agents",
        "both",
    ])
    .ok();
    assert!(case.home("claude/skills/leanck/SKILL.md").is_file());
    assert!(case.home("codex/agents/leanck/lean-grind.toml").is_file());
    assert!(!case.home(".local/bin/leanck").exists());
    // Without the executable nothing from the release runs, and no receipt claims it.
    assert_eq!(case.executions(), "");
    assert!(case.daemons.calls.borrow().is_empty());
    assert!(!case.state().join("receipt.json").exists());
    assert!(!case.state().join("sources.tsv").exists());
    assert!(case.hosts.calls.borrow().is_empty());
}

#[test]
fn release_executable_version_mismatch_installs_nothing() {
    let mut case = Case::new();
    case.release = write_release(&case.root, "mismatch", "1.2.3");
    executable(
        &case.release.join("plugin/bin/leanck"),
        "#!/bin/sh\necho 'leanck 0.0.1'\n",
    );
    let (message, _) = case.install(&[]).err();
    assert!(message.contains("failed its version check"), "{message}");
    assert!(!case.home(".local/bin/leanck").exists());
    assert!(!case.state().join("selections.tsv").exists());
}

#[test]
fn subcommand_output_has_severity_and_source_with_clean_json() {
    let mut case = Case::new();
    case.hosts.output = true;
    let output = case.plugin(&["--all"]).ok();
    for host in ["claude", "codex"] {
        for (level, message) in [
            ("INFO", "host stdout"),
            ("INFO", "host stderr"),
            ("WARN", "Warning: host warning"),
            ("ERR", "error: host error"),
        ] {
            let line = format!("[{level}] ({host}) {message}\n");
            assert!(output.contains(&line), "{line}\n{output}");
        }
    }
    assert!(output.contains("[INFO] leanck 1.2.3\n"));
    assert!(!output.contains("(leanck)"));
    assert!(!output.contains('\x1b'));
}

#[test]
fn no_incompatible_daemons_produces_no_offer_or_static_reminder() {
    let case = Case::new();
    let output = case.install(&[]).ok();
    let calls = case.daemons.calls.borrow().clone();
    assert_eq!(calls.len(), 1);
    assert!(
        calls[0].ends_with("release/leanck/plugin/bin/leanck daemons list"),
        "{calls:?}"
    );
    assert!(!output.contains("incompatible leanck daemon"));
}

#[test]
fn yes_stops_only_incompatible_daemons_and_keeps_json_clean() {
    let case = Case::new();
    *case.daemons.incompatible.borrow_mut() = Ok(2);
    let output = case.install(&[]).ok();
    assert!(output.contains("[WARN] 2 incompatible leanck daemon(s) are running.\n"));
    let calls = case.daemons.calls.borrow().clone();
    assert!(
        calls[1].ends_with("daemons stop --incompatible"),
        "{calls:?}"
    );
    assert!(!calls.iter().any(|c| c.contains("--all")));
}

#[test]
fn incompatible_daemon_offer_accepts_or_declines_in_terminal() {
    for answer in [false, true] {
        let mut case = Case::new();
        *case.daemons.incompatible.borrow_mut() = Ok(1);
        case.stop_daemons = answer;
        case.install(&[]).ok();
        let stopped = case
            .daemons
            .calls
            .borrow()
            .iter()
            .any(|c| c.contains("stop --incompatible"));
        assert_eq!(stopped, answer);
    }
}

#[test]
fn daemon_maintenance_failures_warn_without_failing_install() {
    let case = Case::new();
    *case.daemons.incompatible.borrow_mut() = Err(DaemonCheck::Failed);
    let output = case.install(&[]).ok();
    assert!(output.contains("Could not check for incompatible leanck daemons."));
    *case.daemons.incompatible.borrow_mut() = Err(DaemonCheck::Unreadable);
    let output = case.install(&[]).ok();
    assert!(output.contains("Could not read the incompatible daemon count."));
    *case.daemons.incompatible.borrow_mut() = Ok(1);
    *case.daemons.stop_fails.borrow_mut() = true;
    let output = case.install(&[]).ok();
    assert!(output.contains("Could not stop all incompatible leanck daemons"));
}

#[test]
fn quiet_subcommands_keep_warnings_errors_and_failure_status() {
    let mut case = Case::new();
    case.hosts.output = true;
    case.hosts.failing.borrow_mut().push(HostName::Claude);
    let run = case.plugin(&["--plugin", "claude", "--quiet"]);
    let applied = run.result.as_ref().unwrap();
    assert_eq!(applied.status, Status::Partial);
    assert!(!run.output.contains("[INFO]"), "{}", run.output);
    assert!(
        run.output
            .contains("[WARN] (claude) Warning: host warning\n")
    );
    assert!(run.output.contains("[ERR] (claude) error: host error\n"));
}

#[test]
fn completion_stdout_is_preserved_while_diagnostics_are_prefixed() {
    let case = Case::new();
    fs::write(case.root.join("completion-diagnostics"), "").unwrap();
    let dir = case.completion_dir("zsh");
    let output = case.install(&["--completions", "zsh"]).ok();
    assert_eq!(
        fs::read_to_string(dir.join("_leanck")).unwrap(),
        "#compdef leanck\n"
    );
    assert_eq!(mode(&dir.join("_leanck")), 0o644);
    assert!(
        output.contains("[WARN] warning: completion diagnostic\n"),
        "{output}"
    );
    assert!(!output.contains("(leanck)"));
}

#[test]
fn advanced_plugin_edits_apply_only_after_confirmation_and_recheck_conflicts() {
    let case = Case::new();
    let data = case.root.join("edited data");
    // The editor moves the default executable with the data directory.
    let run = case.run_with(
        &["--plugin", "codex"],
        |s| s.set_data_dir(data.clone()),
        None,
    );
    run.ok();
    assert!(data.join("bin/leanck").is_file());
    assert!(data.join("bundle/plugin/bin/leanck").is_file());
    assert!(!case.home("data/leanck").exists());
    let saved = case.state_file("selections.tsv");
    assert!(saved.contains(&format!("data_dir\t{}\n", data.display())));
    assert!(saved.contains("plugin_bin_dir\t\n"));
    // A foreign file at an edited destination is a conflict on re-plan.
    let other = case.root.join("other data");
    fs::create_dir_all(other.join("agents")).unwrap();
    fs::write(other.join("agents/foreign.txt"), "foreign").unwrap();
    let (message, _) = case
        .run_with(
            &["--plugin", "codex"],
            |s| s.set_data_dir(other.clone()),
            None,
        )
        .err();
    assert!(
        message.contains("already exists or has local edits"),
        "{message}"
    );
    assert_eq!(
        fs::read_to_string(other.join("agents/foreign.txt")).unwrap(),
        "foreign"
    );
}

#[test]
fn advanced_plugin_additional_locations_and_completion_destination() {
    let case = Case::new();
    let skills = case.root.join("extra skills");
    let claude = case.root.join("extra Claude agents");
    let codex = case.root.join("extra Codex agents");
    let completions = case.root.join("custom completions");
    fs::create_dir(&completions).unwrap();
    let profile = case.root.join("custom profile");
    fs::write(&profile, "# existing profile\n").unwrap();
    let paths: Vec<String> = [&skills, &claude, &codex, &completions, &profile]
        .iter()
        .map(|p| p.display().to_string())
        .collect();
    case.plugin(&[
        "--extra-skills-dir",
        &paths[0],
        "--extra-claude-agents-dir",
        &paths[1],
        "--extra-codex-agents-dir",
        &paths[2],
        "--completions",
        "bash",
        "--completions-dir",
        &paths[3],
        "--add-to-path",
        "--shell",
        "bash",
        "--path-file",
        &paths[4],
    ])
    .ok();
    assert!(skills.join("lean-proving/SKILL.md").is_file());
    assert!(claude.join("lean-expert.md").is_file());
    assert!(codex.join("lean-expert.toml").is_file());
    assert!(
        fs::read_to_string(completions.join("leanck"))
            .unwrap()
            .contains("completions for bash")
    );
    assert!(
        fs::read_to_string(&profile)
            .unwrap()
            .contains("# >>> leanck PATH >>>")
    );
    let state = case.state_file("selections.tsv");
    for (key, path) in [
        ("extra_skills", &skills),
        ("extra_claude_agents", &claude),
        ("extra_codex_agents", &codex),
    ] {
        assert!(
            state.contains(&format!("{key}\t{}\n", path.display())),
            "{key}"
        );
    }
}

#[test]
fn plain_advanced_config_can_change_completion_and_profile_paths() {
    let case = Case::new();
    let directory = case.root.join("existing completions");
    fs::create_dir(&directory).unwrap();
    let profile = case.root.join("existing profile");
    fs::write(&profile, "# existing profile\n").unwrap();
    let (dir_arg, profile_arg) = (
        directory.display().to_string(),
        profile.display().to_string(),
    );
    case.plugin(&[
        "--completions",
        "fish",
        "--completions-dir",
        &dir_arg,
        "--shell",
        "fish",
        "--path-file",
        &profile_arg,
    ])
    .ok();
    assert!(directory.join("leanck.fish").is_file());
    assert!(
        fs::read_to_string(&profile)
            .unwrap()
            .contains("fish_add_path")
    );
    assert!(!case.home("data/zsh").exists());
    assert!(!case.home(".zshrc").exists());
}

#[test]
fn advanced_standalone_component_enums_install_selected_hosts() {
    let case = Case::new();
    case.install(&["--skills", "both", "--agents", "codex"])
        .ok();
    for path in [
        ".local/bin/leanck",
        "claude/skills/lean-proving/SKILL.md",
        ".agents/skills/lean-proving/SKILL.md",
        "codex/agents/leanck/lean-expert.toml",
    ] {
        assert!(case.home(path).is_file(), "{path}");
    }
    assert!(!case.home("claude/agents").exists());
    assert!(case.hosts.calls.borrow().is_empty());
    // Directories 0755, files 0644, executables 0755.
    assert_eq!(mode(&case.home(".local/bin/leanck")), 0o755);
    assert_eq!(mode(&case.home("claude/skills/lean-proving")), 0o755);
    assert_eq!(
        mode(&case.home("claude/skills/lean-proving/SKILL.md")),
        0o644
    );
    assert_eq!(
        mode(&case.home("codex/agents/leanck/lean-expert.toml")),
        0o644
    );
    assert_eq!(
        mode(&case.home("codex/agents/leanck/lean-expert.toml.leanck-sha256")),
        0o644
    );
    assert_eq!(mode(&case.state()), 0o700);
}

#[test]
fn locally_edited_managed_files_require_force_and_are_backed_up() {
    let case = Case::new();
    case.install(&["--skills", "claude", "--agents", "codex"])
        .ok();
    let skill = case.home("claude/skills/lean-proving/SKILL.md");
    fs::write(&skill, "local edit").unwrap();
    let (message, _) = case.install(&[]).err();
    assert!(message.contains("local edits"), "{message}");
    assert_eq!(fs::read_to_string(&skill).unwrap(), "local edit");
    let output = case.install(&["--force"]).ok();
    assert!(output.contains("Replacement backups: "), "{output}");
    let backups = case.state().join("backups");
    let manifests = backup_manifests(&case);
    let line = manifests
        .lines()
        .find(|l| l.starts_with(&format!("{}\t", skill.parent().unwrap().display())))
        .expect("manifest records the skill");
    let backup = PathBuf::from(line.split('\t').nth(1).unwrap());
    assert!(backup.starts_with(&backups));
    assert_eq!(
        fs::read_to_string(backup.join("SKILL.md")).unwrap(),
        "local edit"
    );
    let agent = case.home("codex/agents/leanck/lean-expert.toml");
    fs::write(&agent, "edited agent").unwrap();
    case.install(&[]).err();
    assert_eq!(fs::read_to_string(&agent).unwrap(), "edited agent");
    case.install(&["--force"]).ok();
    assert!(backup_manifests(&case).contains(&format!("{}\t", agent.display())));
    // Backups never land beside destinations.
    for dir in [case.home("claude/skills"), case.home("codex/agents/leanck")] {
        for entry in fs::read_dir(dir).unwrap() {
            let name = entry.unwrap().file_name().to_string_lossy().into_owned();
            assert!(!name.contains("backup"), "{name}");
        }
    }
}

#[test]
fn completion_and_path_preview_and_idempotent_install() {
    let case = Case::new();
    case.completion_dir("fish");
    let profile = case.home("config/fish/config.fish");
    fs::write(&profile, "# existing profile\n").unwrap();
    fs::set_permissions(&profile, fs::Permissions::from_mode(0o644)).unwrap();
    case.install(&["--shell", "fish", "--add-to-path"]).ok();
    let completion = case.home("config/fish/completions/leanck.fish");
    assert!(
        fs::read_to_string(completion)
            .unwrap()
            .contains("complete -c leanck")
    );
    let first = fs::read_to_string(&profile).unwrap();
    assert!(first.starts_with("# existing profile\n"));
    case.install(&[]).ok();
    let second = fs::read_to_string(&profile).unwrap();
    assert_eq!(second.matches("# >>> leanck PATH >>>").count(), 1);
    assert_eq!(second.matches("fish_add_path").count(), 1);
    // Rewrites do not accumulate blank lines.
    assert_eq!(second, first);
    assert_eq!(mode(&profile), 0o644);
}

#[test]
fn all_supported_completion_shells_use_correct_destinations_and_arguments() {
    let case = Case::new();
    for (shell, relative) in [
        ("bash", "data/bash-completion/completions/leanck"),
        ("zsh", "data/zsh/site-functions/_leanck"),
        ("fish", "config/fish/completions/leanck.fish"),
        ("powershell", "config/powershell/Completions/leanck.ps1"),
        ("elvish", "config/elvish/lib/leanck.elv"),
    ] {
        case.completion_dir(shell);
        case.install(&["--completions", shell]).ok();
        let script = case.home(relative);
        assert!(script.is_file(), "{shell}");
        assert!(
            case.executions()
                .contains(&format!("leanck _completions {shell}\n"))
        );
        // A recorded script is replaced without a conflict.
        case.install(&["--completions", shell]).ok();
        if shell == "zsh" {
            assert!(
                fs::read_to_string(&script)
                    .unwrap()
                    .starts_with("#compdef leanck\n")
            );
        }
    }
    for profile in [".zshrc", ".bashrc", "config/fish/config.fish"] {
        assert!(!case.home(profile).exists());
    }
}

#[test]
fn missing_custom_shell_paths_are_skipped_in_preview_and_install() {
    let case = Case::new();
    let directory = case.root.join("missing completions parent/completions");
    let profile = case.root.join("missing profile parent/config.fish");
    let (dir_arg, profile_arg) = (
        directory.display().to_string(),
        profile.display().to_string(),
    );
    let output = case
        .install(&[
            "--shell",
            "fish",
            "--completions-dir",
            &dir_arg,
            "--path-file",
            &profile_arg,
            "--add-to-path",
        ])
        .ok();
    assert!(!directory.parent().unwrap().exists());
    assert!(!profile.parent().unwrap().exists());
    assert!(!case.executions().contains("_completions"));
    assert!(output.contains(&format!(
        "Add {} to PATH",
        case.home(".local/bin").display()
    )));
}

#[test]
fn custom_completion_directory_is_previewed_and_remembered() {
    let case = Case::new();
    let directory = case.root.join("chosen completions");
    fs::create_dir(&directory).unwrap();
    let arg = directory.display().to_string();
    case.install(&["--completions", "bash", "--completions-dir", &arg])
        .ok();
    assert!(
        case.state_file("selections.tsv")
            .contains(&format!("completions_dir\t{arg}\n"))
    );
    let script = directory.join("leanck");
    fs::write(&script, "local completion edits").unwrap();
    case.install(&[]).err();
    assert_eq!(
        fs::read_to_string(&script).unwrap(),
        "local completion edits"
    );
    case.install(&["--force"]).ok();
    assert!(
        fs::read_to_string(&script)
            .unwrap()
            .contains("completions for bash")
    );
}

#[test]
fn additional_locations_are_repeatable_remembered_and_rolled_back() {
    let case = Case::new();
    let [skills, skills2, claude, codex] =
        ["skills one", "skills two", "Claude extras", "Codex extras"].map(|n| case.root.join(n));
    let s = |p: &PathBuf| p.display().to_string();
    let (a, b, c, d) = (s(&skills), s(&skills2), s(&claude), s(&codex));
    let args = [
        "--plugin",
        "codex",
        "--extra-skills-dir",
        &a,
        "--extra-skills-dir",
        &b,
        "--extra-skills-dir",
        &a,
        "--extra-claude-agents-dir",
        &c,
        "--extra-codex-agents-dir",
        &d,
    ];
    case.plugin(&args).ok();
    for path in [
        skills.join("lean-proving/SKILL.md"),
        skills2.join("lean-proving/SKILL.md"),
        claude.join("lean-expert.md"),
        codex.join("lean-expert.toml"),
    ] {
        assert!(path.is_file(), "{}", path.display());
    }
    case.plugin(&["--plugin", "codex"]).ok();
    let state = case.state_file("selections.tsv");
    assert_eq!(state.matches("extra_skills\t").count(), 2);
    let originals = [
        (skills.join("lean-proving/SKILL.md"), "first local edit"),
        (skills2.join("lean-proving/SKILL.md"), "second local edit"),
        (claude.join("lean-expert.md"), "custom Claude agent"),
    ];
    for (path, content) in &originals {
        fs::write(path, content).unwrap();
    }
    fs::write(case.root.join("fail-completions"), "").unwrap();
    case.completion_dir("fish");
    let run = case.plugin(&["--plugin", "codex", "--force", "--completions", "fish"]);
    let (message, output) = run.err();
    assert_eq!(message, "completion generation failed");
    assert!(output.contains("Restoring replaced files"), "{output}");
    for (path, content) in &originals {
        assert_eq!(&fs::read_to_string(path).unwrap(), content);
    }
    assert_eq!(case.state_file("selections.tsv"), state);
    assert!(!case.state().join("update-journal.json").exists());
}

#[test]
fn additional_skills_can_be_installed_without_executable_or_host() {
    let case = Case::new();
    let directory = case.root.join("skills only");
    let arg = directory.display().to_string();
    let output = case.install(&["--no-cli", "--extra-skills-dir", &arg]).ok();
    assert!(directory.join("lean-proving/SKILL.md").is_file());
    assert!(!case.home(".local/bin/leanck").exists());
    assert!(case.hosts.calls.borrow().is_empty());
    assert!(output.contains("Standalone skills and agents require an existing leanck on PATH."));
}

#[test]
fn custom_codex_agent_directory_is_independent_of_host_config() {
    let case = Case::new();
    let destination = case.root.join("custom Codex agents");
    let arg = destination.display().to_string();
    case.plugin(&["--plugin", "codex", "--codex-agents-dir", &arg])
        .ok();
    let agent = destination.join("lean-expert.toml");
    assert!(
        fs::symlink_metadata(&agent)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        agent.canonicalize().unwrap(),
        case.home("data/leanck/agents/lean-expert.toml")
            .canonicalize()
            .unwrap()
    );
    assert!(!case.home("codex/agents").exists());
    case.plugin(&["--plugin", "codex"]).ok();
    assert!(
        fs::symlink_metadata(&agent)
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn flat_managed_codex_agents_migrate_but_local_edits_and_unrelated_files_stay() {
    let case = Case::new();
    case.install(&["--no-cli", "--agents", "codex"]).ok();
    let parent = case.home("codex/agents");
    let namespace = parent.join("leanck");
    for entry in fs::read_dir(&namespace).unwrap() {
        let path = entry.unwrap().path();
        fs::rename(&path, parent.join(path.file_name().unwrap())).unwrap();
    }
    fs::remove_dir(&namespace).unwrap();
    let edited = parent.join("lean-expert.toml");
    fs::write(&edited, "local legacy edit").unwrap();
    let unrelated = parent.join("other.toml");
    fs::write(&unrelated, "unrelated agent").unwrap();
    case.plugin(&["--plugin", "codex"]).ok();
    assert_eq!(fs::read_to_string(&edited).unwrap(), "local legacy edit");
    assert_eq!(fs::read_to_string(&unrelated).unwrap(), "unrelated agent");
    assert!(
        fs::symlink_metadata(namespace.join("lean-expert.toml"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    for name in ["lean-grind", "lean-style-worker"] {
        assert!(
            fs::symlink_metadata(namespace.join(format!("{name}.toml")))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        for suffix in ["", ".leanck-installed", ".leanck-sha256"] {
            assert!(!parent.join(format!("{name}.toml{suffix}")).exists());
        }
    }
    assert!(
        backup_manifests(&case).contains(&parent.join("lean-grind.toml").display().to_string())
    );
}

#[test]
fn flat_managed_agent_links_migrate_to_custom_destination_before_source_replacement() {
    let case = Case::new();
    case.plugin(&["--plugin", "codex"]).ok();
    let parent = case.home("codex/agents");
    let namespace = parent.join("leanck");
    for entry in fs::read_dir(&namespace).unwrap() {
        let path = entry.unwrap().path();
        fs::rename(&path, parent.join(path.file_name().unwrap())).unwrap();
    }
    fs::remove_dir(&namespace).unwrap();
    let destination = case.root.join("migrated agents");
    let arg = destination.display().to_string();
    case.plugin(&["--plugin", "codex", "--codex-agents-dir", &arg])
        .ok();
    assert!(
        fs::symlink_metadata(destination.join("lean-expert.toml"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(fs::symlink_metadata(parent.join("lean-expert.toml")).is_err());
    assert!(!parent.join("lean-expert.toml.leanck-installed").exists());
}

#[test]
fn failed_file_install_rolls_back_prior_replacements() {
    let case = Case::new();
    case.completion_dir("fish");
    let binary = case.home(".local/bin/leanck");
    fs::create_dir_all(binary.parent().unwrap()).unwrap();
    fs::write(&binary, "previous binary").unwrap();
    fs::write(case.root.join("fail-completions"), "").unwrap();
    // A foreign executable is a conflict; --force backs it up first.
    let (message, _) = case.install(&["--completions", "fish"]).err();
    assert!(
        message.contains("already exists or has local edits"),
        "{message}"
    );
    let (_, output) = case.install(&["--completions", "fish", "--force"]).err();
    assert!(output.contains("Restoring replaced files"), "{output}");
    assert!(output.contains("Replacement backups: "));
    assert_eq!(fs::read_to_string(&binary).unwrap(), "previous binary");
    assert!(!case.state().join("selections.tsv").exists());
}

#[test]
fn force_replaces_a_symlink_without_touching_its_target() {
    let case = Case::new();
    let foreign = case.root.join("foreign skill");
    fs::create_dir(&foreign).unwrap();
    fs::write(foreign.join("keep.txt"), "keep").unwrap();
    let skill = case.home("claude/skills/lean-proving");
    fs::create_dir_all(skill.parent().unwrap()).unwrap();
    std::os::unix::fs::symlink(&foreign, &skill).unwrap();
    case.install(&["--skills", "claude", "--force"]).ok();
    assert!(
        !fs::symlink_metadata(&skill)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(skill.join("SKILL.md").is_file());
    assert_eq!(
        fs::read_to_string(foreign.join("keep.txt")).unwrap(),
        "keep"
    );
    assert_eq!(fs::read_dir(&foreign).unwrap().count(), 1);
}

#[test]
fn terminal_can_preserve_a_conflicting_skill() {
    let mut case = Case::new();
    let skill = case.home("claude/skills/lean-proving");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("custom.txt"), "keep").unwrap();
    case.answers.insert(skill.clone(), Answer::Keep);
    case.install(&["--skills", "claude"]).ok();
    assert_eq!(
        fs::read_to_string(skill.join("custom.txt")).unwrap(),
        "keep"
    );
    assert!(!skill.join("SKILL.md").exists());
    assert!(case.home("claude/skills/lean-style/SKILL.md").is_file());
    assert!(
        !case
            .state_file("uninstall.tsv")
            .contains(&format!("{}\n", skill.display()))
    );
}

#[test]
fn partial_host_failure_preserves_files_and_reports_json() {
    let case = Case::new();
    case.hosts.failing.borrow_mut().push(HostName::Claude);
    let run = case.plugin(&["--plugin", "both"]);
    let applied = run.result.unwrap();
    assert_eq!(applied.status, Status::Partial);
    assert_eq!(
        applied.message(),
        "Claude marketplace registration failed; rerun to resume"
    );
    assert!(run.output.contains("registration may be partial"));
    // A failing host does not stop the next one.
    assert!(
        case.hosts
            .argv()
            .iter()
            .any(|c| c[..3] == ["codex", "plugin", "add"])
    );
    assert!(case.home("data/leanck/bin/leanck").is_file());
    assert!(case.home("data/leanck/bundle/plugin/bin/leanck").is_file());
    assert_eq!(
        json(&case.state().join("receipt.json"))["registration_pending"],
        true
    );
    case.hosts.failing.borrow_mut().clear();
    let run = case.plugin(&[]);
    assert_eq!(run.result.unwrap().status, Status::Installed);
    assert_eq!(
        json(&case.state().join("receipt.json"))["registration_pending"],
        false
    );
}

#[test]
fn host_commands_time_out_and_report_partial_installation() {
    let case = Case::new();
    let bin = case.root.join("host bin");
    executable(&bin.join("claude"), "#!/bin/sh\nsleep 30\n");
    let (plan, inventory, env) = case.plan_settings(&["--plugin", "claude"], |_| {}).unwrap();
    // Only the host CLI gets the short limit: a loaded machine may take more
    // than a second to run the staged executable's version check.
    let hosts = SystemHosts {
        path: format!("{}:/usr/bin:/bin", bin.display()).into(),
        timeout: Duration::from_secs(1),
    };
    let mut console = Console::new(vec![], Style::PLAIN, false);
    let mut confirm = |_| true;
    let started = std::time::Instant::now();
    let applied = apply(
        &plan,
        &inventory,
        &env,
        Runtime {
            hosts: &hosts,
            daemons: &case.daemons,
            confirm_daemon_stop: &mut confirm,
        },
        &mut console,
    )
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(8));
    let output = String::from_utf8(console.into_inner()).unwrap();
    let bundle = case.home("data/leanck/bundle");
    assert!(
        output.contains(&format!(
            "[ERR] (claude) Command timed out after 1 seconds: claude plugin marketplace add {}\n",
            bundle.display()
        )),
        "{output}"
    );
    assert_eq!(applied.status, Status::Partial);
    assert!(case.home("data/leanck/bin/leanck").is_file());
}

#[test]
fn profile_quoting_handles_metacharacters_as_literal_paths() {
    let case = Case::new();
    let custom = case.root.join("a'b\\c $(touch NEVER)");
    let arg = custom.display().to_string();
    fs::write(case.home(".bashrc"), "").unwrap();
    case.install(&["--bin-dir", &arg, "--shell", "bash", "--add-to-path"])
        .ok();
    let output = std::process::Command::new("bash")
        .args([
            "--noprofile",
            "--norc",
            "-c",
            ". \"$1\"; printf \"%s\" \"$PATH\"",
            "profile-test",
        ])
        .arg(case.home(".bashrc"))
        .current_dir(&case.root)
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout)
            .unwrap()
            .split(':')
            .next()
            .unwrap(),
        arg
    );
    assert!(!case.root.join("NEVER").exists());
    if std::process::Command::new("fish")
        .arg("--version")
        .output()
        .is_ok()
    {
        let profile = case.home("config/fish/config.fish");
        fs::create_dir_all(profile.parent().unwrap()).unwrap();
        fs::write(&profile, "").unwrap();
        case.install(&["--bin-dir", &arg, "--shell", "fish", "--add-to-path"])
            .ok();
        let output = std::process::Command::new("fish")
            .args([
                "--no-config",
                "-c",
                "function fish_add_path; printf \"%s\" \"$argv[2]\"; end; source \"$argv[1]\"",
            ])
            .arg(&profile)
            .current_dir(&case.root)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(output.stdout).unwrap(), arg);
    }
}

#[test]
fn default_plugin_disables_mcp_and_links_bundled_codex_agents() {
    let case = Case::new();
    case.plugin(&[]).ok();
    let data = case.home("data/leanck");
    let binary = data.join("bin/leanck");
    assert_eq!(mode(&binary), 0o755);
    let plugin = data.join("bundle/plugin");
    for name in [".mcp.json", ".mcp.codex.json"] {
        assert!(!plugin.join(name).exists());
        assert!(
            plugin
                .join(name.replace(".json", ".disabled.json"))
                .is_file()
        );
    }
    assert!(
        json(&plugin.join(".codex-plugin/plugin.json"))
            .get("mcpServers")
            .is_none()
    );
    let mcp = json(&plugin.join(".mcp.codex.disabled.json"));
    assert_eq!(
        mcp["mcpServers"]["leanck"]["command"],
        binary.to_str().unwrap()
    );
    assert_eq!(
        mcp["mcpServers"]["leanck"]["args"],
        serde_json::json!(["serve", "mcp"])
    );
    // Executable bits survive the copy; the bundle carries a discovery marker.
    assert_eq!(mode(&plugin.join("hooks/check.sh")), 0o755);
    assert_eq!(
        fs::read(plugin.join("bin/.leanck-installation.json")).unwrap(),
        Receipt::marker_bytes(&case.state()).unwrap()
    );
    for entry in fs::read_dir(data.join("agents")).unwrap() {
        let agent = entry.unwrap().path();
        if agent.extension().is_some_and(|e| e == "toml") {
            let link = case
                .home("codex/agents/leanck")
                .join(agent.file_name().unwrap());
            assert_eq!(fs::read_link(&link).unwrap(), agent);
        }
    }
    for path in [
        ".local/bin",
        "claude/skills",
        "claude/agents",
        ".agents/skills",
    ] {
        assert!(!case.home(path).exists(), "{path}");
    }
    case.plugin(&[]).ok();
}

#[test]
fn plugin_mcp_flags_override_saved_choice_and_replace_definition_names() {
    let case = Case::new();
    let plugin = case.home("data/leanck/bundle/plugin");
    for (args, enabled) in [
        (&["--mcp"][..], true),
        (&[][..], true),
        (&["--no-mcp"][..], false),
        (&[][..], false),
    ] {
        case.plugin(args).ok();
        for name in [".mcp.json", ".mcp.codex.json"] {
            assert_eq!(plugin.join(name).is_file(), enabled);
            assert_eq!(
                plugin
                    .join(name.replace(".json", ".disabled.json"))
                    .is_file(),
                !enabled
            );
        }
        let manifest = json(&plugin.join(".codex-plugin/plugin.json"));
        assert_eq!(manifest.get("mcpServers").is_some(), enabled);
        if enabled {
            assert_eq!(manifest["mcpServers"], "./.mcp.codex.json");
        }
        assert!(
            case.state_file("selections.tsv")
                .contains(&format!("mcp_enabled\t{}\n", enabled as u8))
        );
    }
}

#[test]
fn plugin_configuration_failure_installs_nothing() {
    let case = Case::new();
    fs::write(case.release.join("plugin/.mcp.codex.json"), "not json").unwrap();
    let (message, _) = case.plugin(&["--plugin", "codex"]).err();
    assert!(
        message.starts_with("plugin configuration failed"),
        "{message}"
    );
    for path in [
        "data/leanck/bin/leanck",
        "data/leanck/bundle",
        "data/leanck/agents",
    ] {
        assert!(fs::symlink_metadata(case.home(path)).is_err(), "{path}");
    }
    assert!(!case.state().join("selections.tsv").exists());
    assert!(case.hosts.argv().is_empty());
}

#[test]
fn plugin_default_without_detected_hosts_uses_local_share() {
    let mut case = Case::new();
    case.commands.clear();
    case.vars.remove("XDG_DATA_HOME");
    let output = case.plugin(&[]).ok();
    let data = case.home(".local/share/leanck");
    assert!(data.join("bin/leanck").is_file());
    assert!(
        data.join("bundle/plugin/.claude-plugin/plugin.json")
            .is_file()
    );
    assert!(
        data.join("bundle/plugin/.codex-plugin/plugin.json")
            .is_file()
    );
    assert!(data.join("agents/lean-expert.toml").is_file());
    assert!(output.contains("Plugin files are ready in"));
    assert!(case.hosts.calls.borrow().is_empty());
}

#[test]
fn plugin_can_migrate_managed_standalone_codex_agents() {
    let case = Case::new();
    case.install(&["--no-cli", "--agents", "codex"]).ok();
    case.plugin(&[]).ok();
    let agent = case.home("codex/agents/leanck/lean-expert.toml");
    assert!(
        fs::symlink_metadata(&agent)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        agent.canonicalize().unwrap(),
        case.home("data/leanck/agents/lean-expert.toml")
            .canonicalize()
            .unwrap()
    );
}

#[test]
fn plugin_failure_rolls_back_binary_bundle_and_agent_links() {
    let case = Case::new();
    case.completion_dir("fish");
    fs::write(case.root.join("fail-completions"), "").unwrap();
    case.plugin(&["--plugin", "codex", "--completions", "fish"])
        .err();
    for path in [
        "data/leanck/bin/leanck",
        "data/leanck/bundle",
        "data/leanck/agents",
        "codex/agents/leanck/lean-expert.toml",
        "state/leanck/installer/selections.tsv",
    ] {
        assert!(fs::symlink_metadata(case.home(path)).is_err(), "{path}");
    }
    assert!(case.hosts.argv().is_empty());
}

#[test]
fn configuration_example_honors_xdg_and_preserves_existing_configuration() {
    let case = Case::new();
    let config = case.home("config/leanck/config.toml");
    fs::create_dir_all(config.parent().unwrap()).unwrap();
    fs::write(&config, "# User configuration.\n").unwrap();
    let example = config.with_file_name("config.example.toml");
    case.install(&[]).ok();
    assert_eq!(
        fs::read(&example).unwrap(),
        fs::read(case.release.join("docs/leanck.example.toml")).unwrap()
    );
    assert_eq!(mode(&example), 0o644);
    fs::write(&example, "# Edited example.\n").unwrap();
    case.install(&["--force"]).ok();
    assert_eq!(
        fs::read(&example).unwrap(),
        fs::read(case.release.join("docs/leanck.example.toml")).unwrap()
    );
    assert_eq!(
        fs::read_to_string(&config).unwrap(),
        "# User configuration.\n"
    );
}

#[test]
fn configuration_example_refuses_existing_symlink() {
    let case = Case::new();
    let example = case.home("config/leanck/config.example.toml");
    fs::create_dir_all(example.parent().unwrap()).unwrap();
    let target = case.root.join("existing example.toml");
    fs::write(&target, "# Existing example.\n").unwrap();
    std::os::unix::fs::symlink(&target, &example).unwrap();
    assert!(
        case.install(&["--force"])
            .err()
            .0
            .contains("refusing symlink:")
    );
    assert!(
        fs::symlink_metadata(&example)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert_eq!(
        fs::read_to_string(&target).unwrap(),
        "# Existing example.\n"
    );
}

#[test]
fn standalone_installs_only_cli_and_can_upgrade() {
    let mut case = Case::new();
    let output = case.install(&["--completions", "none"]).ok();
    let binary = case.home(".local/bin/leanck");
    assert_eq!(mode(&binary), 0o755);
    assert!(!case.home("data").exists());
    assert!(!case.home("claude").exists());
    assert!(output.contains("Add "));
    let warning = "[WARN] `lake` is not detected. Install info: https://lean-lang.org/install/\n";
    assert!(output.contains(warning));
    assert!(output.ends_with(&format!("Done.\n{warning}")));
    case.commands.push("lake");
    case.release = write_release(&case.root, "release-1.3.0", "1.3.0");
    let output = case.install(&[]).ok();
    assert!(!output.contains(warning));
    let mut names: Vec<_> = fs::read_dir(binary.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(names, [".leanck-installation.json", "leanck"]);
    assert!(
        fs::read_to_string(&binary)
            .unwrap()
            .contains("leanck 1.3.0")
    );
    let receipt = Receipt::discover(&binary, None).unwrap().unwrap();
    assert_eq!(receipt.version, "1.3.0");
    assert!(
        case.state_file("selections.tsv")
            .contains("installed_version\tv1.3.0\n")
    );
}

#[test]
fn skills_both_without_cli_and_pinned_version() {
    let case = Case::new();
    case.install(&["--no-cli", "--skills", "both", "--version", "1.2.3"])
        .ok();
    for directory in [case.home("claude/skills"), case.home(".agents/skills")] {
        let mut names: Vec<_> = fs::read_dir(&directory)
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        assert_eq!(names, ["lean-proving", "lean-style", "leanck"]);
        assert!(
            directory
                .join("lean-proving/reference/search-queries.md")
                .is_file()
        );
    }
    assert!(!case.home(".local/bin/leanck").exists());
    assert!(
        case.state_file("selections.tsv")
            .contains("version\t1.2.3\n")
    );
}

#[test]
fn individual_skills_apply_to_hosts_extra_locations_and_saved_choices() {
    let case = Case::new();
    let extra = case.root.join("extra skills");
    let omitted = case.home("claude/skills/lean-proving/SKILL.md");
    fs::create_dir_all(omitted.parent().unwrap()).unwrap();
    fs::write(&omitted, "keep this unselected skill").unwrap();
    let arg = extra.display().to_string();
    case.install(&[
        "--no-cli",
        "--skills",
        "both",
        "--skill",
        "leanck",
        "--skill",
        "lean-style",
        "--extra-skills-dir",
        &arg,
    ])
    .ok();
    for directory in [
        case.home("claude/skills"),
        case.home(".agents/skills"),
        extra.clone(),
    ] {
        for name in ["leanck", "lean-style"] {
            assert!(directory.join(name).join("SKILL.md").is_file());
        }
    }
    assert_eq!(
        fs::read_to_string(&omitted).unwrap(),
        "keep this unselected skill"
    );
    assert!(!extra.join("lean-proving").exists());
    assert!(
        case.state_file("selections.tsv")
            .contains("selected_skills\tleanck lean-style\n")
    );
}

#[test]
fn all_registers_plugins_and_installs_codex_agents_without_duplicate_skills() {
    let case = Case::new();
    fs::write(case.home(".zshrc"), "").unwrap();
    case.plugin(&["--all", "--skills", "both", "--agents", "claude"])
        .ok();
    let bundle = case.home("data/leanck/bundle");
    assert!(bundle.join("plugin/bin/leanck").is_file());
    assert!(bundle.join(".agents/plugins/marketplace.json").is_file());
    for path in [".agents/skills", "claude/skills", "claude/agents"] {
        assert!(!case.home(path).exists(), "{path}");
    }
    assert_eq!(
        fs::read_dir(case.home("codex/agents/leanck"))
            .unwrap()
            .filter(|e| e
                .as_ref()
                .unwrap()
                .path()
                .extension()
                .is_some_and(|x| x == "toml"))
            .count(),
        3
    );
    let b = bundle.display().to_string();
    let expected: Vec<Vec<String>> = [
        vec!["claude", "plugin", "marketplace", "add", &b],
        vec![
            "claude",
            "plugin",
            "install",
            "leanck@leanck",
            "--scope",
            "user",
        ],
        vec!["codex", "plugin", "marketplace", "add", &b],
        vec!["codex", "plugin", "add", "leanck@leanck"],
    ]
    .into_iter()
    .map(|c| c.into_iter().map(String::from).collect())
    .collect();
    assert_eq!(case.hosts.argv(), expected);
    let calls = case.hosts.calls.borrow().clone();
    // The Codex probe runs first; host hooks find the installed leanck.
    assert_eq!(calls[0].args, ["plugin", "add", "--help"]);
    for call in &calls[1..] {
        assert_eq!(
            call.path_prefix.as_deref(),
            Some(case.home("data/leanck/bin").as_path())
        );
        let home = if call.host == HostName::Claude {
            "claude"
        } else {
            "codex"
        };
        assert_eq!(call.home, case.home(home));
    }
    assert!(
        fs::read_to_string(case.home(".zshrc"))
            .unwrap()
            .contains("data/leanck/bin")
    );
    let inventory = case.state_file("uninstall.tsv");
    assert!(inventory.contains(&format!(
        "host\tclaude\t{}\n",
        case.home("claude").display()
    )));
    assert!(inventory.contains(&format!("host\tcodex\t{}\n", case.home("codex").display())));
    case.plugin(&["--all"]).ok();
}

#[test]
fn conflicting_skill_aborts_before_installing_cli_and_force_replaces_it() {
    let case = Case::new();
    let skill = case.home("claude/skills/lean-proving");
    fs::create_dir_all(&skill).unwrap();
    fs::write(skill.join("custom.txt"), "keep").unwrap();
    case.install(&["--skills", "claude"]).err();
    assert_eq!(
        fs::read_to_string(skill.join("custom.txt")).unwrap(),
        "keep"
    );
    assert!(!case.home(".local/bin").exists());
    case.install(&["--skills", "claude", "--force"]).ok();
    assert!(!skill.join("custom.txt").exists());
    case.install(&["--skills", "claude"]).ok();
}

#[test]
fn custom_agents_preserve_unrelated_files_and_require_force() {
    let case = Case::new();
    let agents = case.home("codex/agents/leanck");
    fs::create_dir_all(&agents).unwrap();
    let custom = agents.join("lean-expert.toml");
    fs::write(&custom, "custom").unwrap();
    let unrelated = agents.join("other.toml");
    fs::write(&unrelated, "unrelated").unwrap();
    case.install(&["--agents", "codex"]).err();
    assert_eq!(fs::read_to_string(&custom).unwrap(), "custom");
    case.install(&["--agents", "both", "--force"]).ok();
    assert!(
        fs::read_to_string(&custom)
            .unwrap()
            .contains("developer_instructions")
    );
    assert_eq!(fs::read_to_string(&unrelated).unwrap(), "unrelated");
    assert!(case.home("claude/agents/lean-expert.md").is_file());
    case.install(&["--agents", "both"]).ok();
}

#[test]
fn custom_paths_and_mirror() {
    let case = Case::new();
    let binary_dir = case.root.join("custom bin");
    let data_dir = case.root.join("custom data");
    let (b, d) = (
        binary_dir.display().to_string(),
        data_dir.display().to_string(),
    );
    case.plugin(&[
        "--bin-dir",
        &b,
        "--data-dir",
        &d,
        "--release-url",
        "https://mirror.example/releases/",
        "--plugin",
        "claude",
    ])
    .ok();
    assert!(binary_dir.join("leanck").is_file());
    assert!(data_dir.join("bundle/plugin/bin/leanck").is_file());
    let state = case.state_file("selections.tsv");
    assert!(state.contains("release_url\thttps://mirror.example/releases\n"));
    assert!(state.contains(&format!("plugin_bin_dir\t{b}\n")));
    assert_eq!(
        json(&case.state().join("receipt.json"))["source"]["releases"],
        "https://mirror.example/releases"
    );
}

// Uninstall.

#[test]
fn uninstall_standalone_preserves_user_files_and_backups() {
    let case = Case::new();
    case.completion_dir("fish");
    let profile = case.home(".zshrc");
    fs::write(&profile, "# existing profile\n").unwrap();
    case.install(&[
        "--skills",
        "both",
        "--agents",
        "both",
        "--completions",
        "fish",
        "--add-to-path",
    ])
    .ok();
    fs::write(
        &profile,
        fs::read_to_string(&profile).unwrap() + "# added later\n",
    )
    .unwrap();
    let unrelated = case.home("codex/agents/leanck/custom.toml");
    fs::write(&unrelated, "keep").unwrap();
    case.uninstall(&[]);
    for path in [
        ".local/bin/leanck",
        ".local/bin/.leanck-installation.json",
        "claude/skills/lean-proving",
        ".agents/skills/lean-proving",
        "claude/agents/lean-expert.md",
        "claude/agents/lean-expert.md.leanck-installed",
        "codex/agents/leanck/lean-expert.toml",
        "config/fish/completions/leanck.fish",
        "config/leanck/config.example.toml",
        "state/leanck/installer/selections.tsv",
        "state/leanck/installer/uninstall.tsv",
        "state/leanck/installer/receipt.json",
        "state/leanck/installer/sources.tsv",
    ] {
        assert!(fs::symlink_metadata(case.home(path)).is_err(), "{path}");
    }
    assert_eq!(fs::read_to_string(&unrelated).unwrap(), "keep");
    assert_eq!(
        fs::read_to_string(&profile).unwrap(),
        "# existing profile\n# added later\n"
    );
    assert!(
        case.daemons
            .calls
            .borrow()
            .iter()
            .any(|c| c.ends_with(".local/bin/leanck daemons stop --all"))
    );
    assert!(case.uninstall(&[]).contains("nothing to remove"));
}

#[test]
fn uninstall_dry_run_executes_nothing_and_needs_no_approval() {
    let case = Case::new();
    case.completion_dir("fish");
    fs::write(case.home(".zshrc"), "").unwrap();
    case.plugin(&["--all", "--add-to-path", "--completions", "fish"])
        .ok();
    let before = case.snapshot();
    case.hosts.clear();
    case.daemons.calls.borrow_mut().clear();
    let (result, output) = case.uninstall_with(&["--dry-run"], Approval::Declined);
    assert_eq!(result.unwrap(), Uninstalled::Preview);
    assert!(output.contains("Would remove host: "));
    assert!(output.contains("Would stop leanck daemons"));
    assert_eq!(case.snapshot(), before);
    assert!(case.hosts.calls.borrow().is_empty());
    assert!(case.daemons.calls.borrow().is_empty());
}

#[test]
fn uninstall_plugin_unregisters_only_selected_host_and_custom_home() {
    let case = Case::new();
    case.plugin(&["--plugin", "codex"]).ok();
    case.hosts.clear();
    case.uninstall(&[]);
    let expected: Vec<Vec<String>> = [
        vec!["codex", "plugin", "remove", "leanck@leanck"],
        vec!["codex", "plugin", "marketplace", "remove", "leanck"],
    ]
    .into_iter()
    .map(|c| c.into_iter().map(String::from).collect())
    .collect();
    assert_eq!(case.hosts.argv(), expected);
    assert!(
        case.hosts
            .calls
            .borrow()
            .iter()
            .all(|c| c.home == case.home("codex"))
    );
    for path in [
        "data/leanck/bundle",
        "data/leanck/agents",
        "data/leanck/bin/leanck",
    ] {
        assert!(!case.home(path).exists(), "{path}");
    }
    assert_eq!(
        fs::read_dir(case.home("codex/agents/leanck"))
            .unwrap()
            .count(),
        0
    );
}

#[test]
fn uninstall_preserves_modified_components_and_can_resume() {
    let case = Case::new();
    case.completion_dir("fish");
    case.install(&[
        "--skills",
        "claude",
        "--agents",
        "both",
        "--completions",
        "fish",
    ])
    .ok();
    let changed: Vec<PathBuf> = [
        ".local/bin/leanck",
        "claude/skills/lean-proving/SKILL.md",
        "claude/agents/lean-expert.md",
        "codex/agents/leanck/lean-grind.toml",
        "config/fish/completions/leanck.fish",
    ]
    .iter()
    .map(|p| case.home(p))
    .collect();
    let originals: Vec<Vec<u8>> = changed.iter().map(|p| fs::read(p).unwrap()).collect();
    for path in &changed {
        let mut bytes = fs::read(path).unwrap();
        bytes.extend_from_slice(b"\n# edited\n");
        fs::write(path, bytes).unwrap();
    }
    let (result, output) = case.uninstall_with(&[], Approval::Approved);
    assert_eq!(result.unwrap(), Uninstalled::Incomplete);
    assert!(output.contains("Uninstall incomplete"));
    for path in &changed {
        assert!(fs::read(path).unwrap().ends_with(b"# edited\n"));
    }
    assert!(
        case.home("claude/agents/lean-expert.md.leanck-installed")
            .exists()
    );
    // The edited executable is never run.
    assert!(
        case.daemons
            .calls
            .borrow()
            .iter()
            .all(|c| !c.contains("stop --all"))
    );
    for (path, bytes) in changed.iter().zip(&originals) {
        fs::write(path, bytes).unwrap();
    }
    case.uninstall(&[]);
    assert!(changed.iter().all(|p| !p.exists()));
}

#[test]
fn uninstall_tracks_old_and_extra_destinations() {
    let case = Case::new();
    let binary_a = case.root.join("first bin");
    let binary_b = case.root.join("second bin");
    let extra = case.root.join("extra skills");
    let (a, b, e) = (
        binary_a.display().to_string(),
        binary_b.display().to_string(),
        extra.display().to_string(),
    );
    case.install(&["--bin-dir", &a, "--extra-skills-dir", &e])
        .ok();
    case.install(&["--bin-dir", &b]).ok();
    let inventory = case.state_file("uninstall.tsv");
    assert!(
        inventory.contains(&format!("{a}/leanck\n"))
            && inventory.contains(&format!("{b}/leanck\n"))
    );
    case.uninstall(&[]);
    assert!(!binary_a.join("leanck").exists());
    assert!(!binary_b.join("leanck").exists());
    assert!(!extra.join("lean-proving").exists());
}

#[test]
fn uninstall_host_failure_preserves_files_and_retry_receipts() {
    let case = Case::new();
    case.plugin(&["--all"]).ok();
    case.hosts.failing.borrow_mut().push(HostName::Codex);
    let (result, _) = case.uninstall_with(&[], Approval::Approved);
    assert!(format!("{:#}", result.unwrap_err()).contains("removal failed"));
    assert!(case.home("data/leanck/bundle").exists());
    let receipts = case.state_file("uninstall.tsv");
    assert!(!receipts.contains("host\tclaude\t"));
    assert!(receipts.contains("marketplace\tclaude\t"));
    case.hosts.failing.borrow_mut().clear();
    case.hosts.clear();
    case.uninstall(&[]);
    assert!(
        !case
            .hosts
            .argv()
            .iter()
            .any(|c| c[..3] == ["claude", "plugin", "uninstall"])
    );
}

#[test]
fn uninstall_follows_owned_symlink_parents_and_preserves_tree_links() {
    let case = Case::new();
    case.install(&["--skills", "claude", "--agents", "codex"])
        .ok();
    // The user's own link above a destination, as dotfile managers make,
    // is followed: the agents are removed where the link points.
    let directory = case.home("codex/agents/leanck");
    let moved = case.home("moved agents");
    fs::rename(&directory, &moved).unwrap();
    std::os::unix::fs::symlink(&moved, &directory).unwrap();
    // A link inside a managed tree is a local change: the tree is preserved.
    let skill = case.home("claude/skills/lean-proving");
    std::os::unix::fs::symlink(case.home("outside"), skill.join("user-link")).unwrap();
    let (result, output) = case.uninstall_with(&[], Approval::Approved);
    assert_eq!(result.unwrap(), Uninstalled::Incomplete, "{output}");
    assert!(!moved.join("lean-expert.toml").exists(), "{output}");
    assert!(!moved.join("lean-expert.toml.leanck-installed").exists());
    assert!(
        fs::symlink_metadata(&directory)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(output.contains(&format!(
        "Preserved changed or unsafe path: {}",
        skill.display()
    )));
    assert!(
        fs::symlink_metadata(skill.join("user-link"))
            .unwrap()
            .file_type()
            .is_symlink()
    );
}

#[test]
fn dotfile_links_above_destinations_install_update_and_uninstall() {
    let case = Case::new();
    // ~/.claude (here CLAUDE_CONFIG_DIR) and the state's parent are the user's links.
    let dotfiles = case.root.join("dotfiles");
    fs::create_dir_all(dotfiles.join("claude")).unwrap();
    fs::create_dir_all(dotfiles.join("state")).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("claude"), case.home("claude")).unwrap();
    std::os::unix::fs::symlink(dotfiles.join("state"), case.home("state")).unwrap();
    case.install(&["--skills", "claude", "--agents", "claude"])
        .ok();
    assert!(dotfiles.join("claude/skills/leanck/SKILL.md").is_file());
    assert!(dotfiles.join("claude/agents/lean-expert.md").is_file());
    assert!(
        dotfiles
            .join("state/leanck/installer/uninstall.tsv")
            .is_file()
    );
    // Reinstalling replaces through the links and keeps them.
    case.install(&["--skills", "claude", "--agents", "claude", "--force"])
        .ok();
    for link in ["claude", "state"] {
        assert!(
            fs::symlink_metadata(case.home(link))
                .unwrap()
                .file_type()
                .is_symlink()
        );
    }
    case.uninstall(&[]);
    assert!(!dotfiles.join("claude/skills/leanck").exists());
    assert!(!dotfiles.join("claude/agents/lean-expert.md").exists());
    assert!(dotfiles.join("claude/skills").is_dir());
    // A link at a destination itself is never followed: it is a conflict.
    let elsewhere = dotfiles.join("elsewhere");
    fs::create_dir_all(&elsewhere).unwrap();
    std::os::unix::fs::symlink(&elsewhere, case.home("claude/skills/leanck")).unwrap();
    let (error, _) = case
        .install(&["--skills", "claude", "--skill", "leanck"])
        .err();
    assert!(error.contains("already exists"), "{error}");
    assert!(elsewhere.read_dir().unwrap().next().is_none());
}

#[test]
fn uninstall_legacy_state_and_custom_state_directory() {
    let case = Case::new();
    let state = case.root.join("custom state");
    let arg = state.display().to_string();
    case.install(&["--state-dir", &arg, "--skills", "both", "--agents", "both"])
        .ok();
    fs::remove_file(state.join("uninstall.tsv")).unwrap();
    case.uninstall(&["--state-dir", &arg]);
    for path in [
        ".local/bin/leanck",
        "claude/skills/lean-proving",
        "codex/agents/leanck/lean-expert.toml",
    ] {
        assert!(!case.home(path).exists(), "{path}");
    }
}

#[test]
fn uninstall_invalid_arguments_do_not_change_files() {
    let case = Case::new();
    case.install(&[]).ok();
    let before = case.snapshot();
    for (args, message) in [
        (&["--unknown"][..], "unexpected argument"),
        (&["--state-dir"][..], "a value is required"),
        (
            &["--timeout", "0"][..],
            "--timeout expects a positive integer",
        ),
        (
            &["--plugin", "invalid"][..],
            "--plugin expects claude, codex, or both",
        ),
        (&["--state-dir", "a\tb"][..], "control characters in path"),
    ] {
        let (result, _) = case.uninstall_with(args, Approval::Approved);
        let error = format!("{:#}", result.unwrap_err());
        assert!(error.contains(message), "{args:?}: {error}");
        assert_eq!(case.snapshot(), before);
    }
}

#[test]
fn uninstall_cancellation_and_confirmation_use_terminal_not_script_stdin() {
    let case = Case::new();
    case.install(&[]).ok();
    let before = case.snapshot();
    let (result, output) = case.uninstall_with(&[], Approval::Declined);
    assert_eq!(result.unwrap(), Uninstalled::Cancelled);
    assert!(output.ends_with("Cancelled.\n"));
    assert_eq!(case.snapshot(), before);
    let (result, _) = case.uninstall_with(&[], Approval::Interrupted);
    assert!(result.is_err());
    assert_eq!(case.snapshot(), before);
}

#[test]
fn uninstall_host_timeout_preserves_files() {
    let case = Case::new();
    case.plugin(&["--plugin", "codex"]).ok();
    case.hosts.slow.borrow_mut().push(HostName::Codex);
    let (result, output) = case.uninstall_with(&["--timeout", "1"], Approval::Approved);
    assert!(result.is_err());
    assert!(
        output.contains("Command timed out after 1 seconds: codex plugin remove leanck@leanck")
    );
    assert!(case.home("data/leanck/bundle").exists());
}

#[test]
fn uninstall_legacy_plugins_require_explicit_host_selection() {
    let mut case = Case::new();
    case.plugin(&["--plugin", "codex"]).ok();
    fs::remove_file(case.state().join("uninstall.tsv")).unwrap();
    let before = case.snapshot();
    let (result, _) = case.uninstall_with(&[], Approval::Approved);
    assert!(
        format!("{:#}", result.unwrap_err())
            .contains("older installer did not record plugin hosts")
    );
    assert_eq!(case.snapshot(), before);
    case.vars.insert(
        "CLAUDE_CONFIG_DIR".into(),
        case.home("unrelated claude").into(),
    );
    case.uninstall(&["--plugin", "codex"]);
    assert!(!case.home("data/leanck/bundle").exists());
}

#[test]
fn uninstall_legacy_plugins_use_saved_custom_host_directory() {
    let case = Case::new();
    let custom = case.root.join("custom codex");
    let arg = custom.display().to_string();
    case.plugin(&["--plugin", "codex", "--codex-dir", &arg])
        .ok();
    fs::remove_file(case.state().join("uninstall.tsv")).unwrap();
    case.hosts.clear();
    case.uninstall(&["--plugin", "codex"]);
    let calls = case.hosts.calls.borrow().clone();
    assert_eq!(calls.len(), 2);
    assert!(
        calls
            .iter()
            .all(|c| c.host == HostName::Codex && c.home == custom)
    );
}

#[test]
fn uninstall_legacy_preserves_retargeted_agent_link() {
    let case = Case::new();
    case.plugin(&["--plugin", "codex"]).ok();
    fs::remove_file(case.state().join("uninstall.tsv")).unwrap();
    let link = case.home("codex/agents/leanck/lean-expert.toml");
    let user_agent = case.home("user-agent.toml");
    fs::write(&user_agent, "keep my agent").unwrap();
    fs::remove_file(&link).unwrap();
    std::os::unix::fs::symlink(&user_agent, &link).unwrap();
    let (result, _) = case.uninstall_with(&["--plugin", "codex"], Approval::Approved);
    assert_eq!(result.unwrap(), Uninstalled::Incomplete);
    assert_eq!(fs::read_link(&link).unwrap(), user_agent);
    assert_eq!(fs::read_to_string(&user_agent).unwrap(), "keep my agent");
}

// Journal faults, recovery, and shell-installer state.

/// Start from an installation, then fail a reinstall that replaces nearly
/// everything at each replacement in turn: every failure restores every
/// file, so the same starting state serves each attempt.
fn fail_each_replacement(first: &[&str], second: &[&str], edited: &str) {
    let case = Case::new();
    case.completion_dir("fish");
    fs::write(case.home("config/fish/config.fish"), "# profile\n").unwrap();
    case.run_with(first, |_| {}, None).ok();
    fs::write(case.home(edited), "edited").unwrap();
    let before = case.snapshot();
    let mut n = 1;
    loop {
        let run = case.run_with(second, |_| {}, Some(n));
        match run.result {
            Ok(_) => {
                assert!(n > 15, "{second:?} made only {} replacements", n - 1);
                return;
            }
            Err(error) => {
                assert!(
                    format!("{error:#}").contains(&format!("injected failure at replacement {n}")),
                    "{error:#}"
                );
                assert_eq!(
                    case.snapshot(),
                    before,
                    "{second:?} failing at replacement {n}"
                );
                assert!(!case.state().join("update-journal.json").exists());
            }
        }
        n += 1;
    }
}

#[test]
fn fail_on_the_nth_replacement_rolls_back_everything() {
    fail_each_replacement(
        &[
            "--plugin",
            "both",
            "--completions",
            "fish",
            "--add-to-path",
            "--shell",
            "fish",
        ],
        &["--plugin", "both", "--mcp", "--force"],
        "data/leanck/bundle/plugin/skills/lean-style/SKILL.md",
    );
}

#[test]
fn fail_on_the_nth_standalone_replacement_rolls_back_everything() {
    fail_each_replacement(
        &[
            "--standalone",
            "--skills",
            "both",
            "--agents",
            "both",
            "--completions",
            "fish",
        ],
        &["--standalone", "--force"],
        "claude/skills/lean-style/SKILL.md",
    );
}

#[test]
fn interrupted_installations_are_recovered_before_planning_and_apply() {
    let case = Case::new();
    case.install(&["--skills", "claude"]).ok();
    let skill = case.home("claude/skills/lean-style");
    let before = case.snapshot();
    // Simulate a crash mid-install: the journal names a backup of the old tree.
    let backups = case.state().join("backups/crash");
    fs::create_dir_all(&backups).unwrap();
    crate::transaction::copy_preserving(&skill, &backups.join("0")).unwrap();
    fs::write(skill.join("SKILL.md"), "half installed").unwrap();
    let journal = serde_json::json!({
        "schema": 1,
        "committed": false,
        "entries": [{"destination": skill, "backup": backups.join("0"), "existed": true, "staged": null}],
    });
    fs::write(
        case.state().join("update-journal.json"),
        journal.to_string(),
    )
    .unwrap();
    assert!(recover_interrupted(&case.state()).unwrap());
    assert!(!recover_interrupted(&case.state()).unwrap());
    let mut after = case.snapshot();
    after.retain(|path, _| !path.starts_with("home/state/leanck/installer/backups"));
    assert_eq!(after, before);
    // A journal that appears after planning stops apply before any change.
    let (plan, inventory, env) = case.plan_settings(&["--standalone"], |_| {}).unwrap();
    fs::write(
        case.state().join("update-journal.json"),
        serde_json::json!({"schema": 1, "committed": false, "entries": []}).to_string(),
    )
    .unwrap();
    let mut console = Console::new(vec![], Style::PLAIN, false);
    let mut confirm = |_| true;
    let error = apply(
        &plan,
        &inventory,
        &env,
        Runtime {
            hosts: &case.hosts,
            daemons: &case.daemons,
            confirm_daemon_stop: &mut confirm,
        },
        &mut console,
    )
    .unwrap_err();
    assert!(
        error
            .to_string()
            .contains("recovered an interrupted installation"),
        "{error:#}"
    );
    assert!(!case.state().join("update-journal.json").exists());
}

#[test]
fn a_busy_installation_is_refused_before_any_change() {
    let case = Case::new();
    case.install(&[]).ok();
    let before = case.snapshot();
    let _lease = Maintenance::acquire(&case.state()).unwrap();
    let (message, _) = case.install(&["--force"]).err();
    assert!(message.starts_with("installation is busy"), "{message}");
    assert_eq!(case.snapshot(), before);
}

/// Write the state a shell-installer plugin run left behind: umask 077
/// modes, compact markers, an inventory in arbitrary order, shell-ordered
/// selections without `release_api`, and the `_update receipt` output.
fn shell_installed(case: &Case) -> (PathBuf, String) {
    let release = &case.release;
    let data = case.home("data/leanck");
    let bin = data.join("bin");
    let state = case.state();
    let private_tree = |from: &Path, to: &Path| {
        crate::transaction::copy_preserving(from, to).unwrap();
        for entry in walk(to) {
            let meta = fs::symlink_metadata(&entry).unwrap();
            let mode = if meta.is_dir() { 0o700 } else { 0o600 };
            fs::set_permissions(&entry, fs::Permissions::from_mode(mode)).unwrap();
        }
        fs::write(to.join(".leanck-installed"), "leanck installer v1\n").unwrap();
    };
    for dir in [&bin, &state, &case.home("codex/agents/leanck")] {
        fs::create_dir_all(dir).unwrap();
    }
    let binary = bin.join("leanck");
    fs::copy(release.join("plugin/bin/leanck"), &binary).unwrap();
    let marker = format!("{{\"state_dir\":\"{}\"}}\n", state.display());
    fs::write(bin.join(".leanck-installation.json"), &marker).unwrap();
    let bundle = data.join("bundle");
    private_tree(release, &bundle);
    crate::install::configure_plugin(&bundle.join("plugin"), &binary, false).unwrap();
    fs::write(bundle.join("plugin/bin/.leanck-installation.json"), &marker).unwrap();
    let bundle_digest = tree_digest(&bundle).unwrap();
    fs::write(
        bundle.join(".leanck-content-sha256"),
        format!("{bundle_digest}\n"),
    )
    .unwrap();
    let agents = data.join("agents");
    private_tree(&release.join("plugin/codex/agents"), &agents);
    let agents_digest = tree_digest(&agents).unwrap();
    fs::write(
        agents.join(".leanck-content-sha256"),
        format!("{agents_digest}\n"),
    )
    .unwrap();
    let mut links = vec![];
    for name in [
        "lean-expert.toml",
        "lean-grind.toml",
        "lean-style-worker.toml",
    ] {
        let link = case.home("codex/agents/leanck").join(name);
        std::os::unix::fs::symlink(agents.join(name), &link).unwrap();
        fs::write(sidecar(&link, "leanck-installed"), "leanck installer v1\n").unwrap();
        fs::write(
            sidecar(&link, "leanck-sha256"),
            format!("{}\n", digest_file(&link).unwrap()),
        )
        .unwrap();
        links.push(link);
    }
    let example = case.home("config/leanck/config.example.toml");
    fs::create_dir_all(example.parent().unwrap()).unwrap();
    fs::copy(release.join("docs/leanck.example.toml"), &example).unwrap();
    let completion = case.completion_dir("fish").join("leanck.fish");
    fs::write(&completion, "# completions for fish\ncomplete -c leanck\n").unwrap();
    let profile = case.home(".zshrc");
    let original_profile = "# my zshrc\n".to_owned();
    fs::write(
        &profile,
        format!(
            "{original_profile}\n# >>> leanck PATH >>>\nexport PATH='{}':\"$PATH\"\n# <<< leanck PATH <<<\n",
            bin.display()
        ),
    )
    .unwrap();
    let digest = |p: &Path| digest_file(p).unwrap();
    let mut inventory = vec![
        format!("tree\t{agents_digest}\t{}", agents.display()),
        format!("host\tcodex\t{}", case.home("codex").display()),
        format!("file\t{}\t{}", digest(&completion), completion.display()),
        format!("tree\t{bundle_digest}\t{}", bundle.display()),
        format!("profile\t-\t{}", profile.display()),
        format!("file\t{}\t{}", digest(&example), example.display()),
        format!("file\t{}\t{}", digest(&binary), binary.display()),
        format!(
            "file\t{}\t{}",
            digest(&bin.join(".leanck-installation.json")),
            bin.join(".leanck-installation.json").display()
        ),
    ];
    for link in &links {
        inventory.push(format!(
            "link\t{}\t{}",
            fs::read_link(link).unwrap().display(),
            link.display()
        ));
    }
    inventory.reverse();
    fs::write(state.join("uninstall.tsv"), inventory.join("\n") + "\n").unwrap();
    let mut sources = format!(
        "file\t{}\t{}\tplugin/bin/leanck\n",
        digest(&binary),
        binary.display()
    );
    sources.push_str(&format!("tree\t{bundle_digest}\t{}\t.\n", bundle.display()));
    sources.push_str(&format!(
        "tree\t{agents_digest}\t{}\tplugin/codex/agents\n",
        agents.display()
    ));
    for link in &links {
        let target = fs::read_link(link).unwrap();
        sources.push_str(&format!(
            "link\t{0}\t{1}\t{0}\n",
            target.display(),
            link.display()
        ));
    }
    sources.push_str(&format!(
        "completion\t{}\t{}\tfish\n",
        digest(&completion),
        completion.display()
    ));
    fs::write(state.join("sources.tsv"), sources).unwrap();
    let h = |p: &str| case.home(p).display().to_string();
    let selections = [
        ("mode", "plugin".to_owned()),
        ("plugin_claude", "0".into()),
        ("plugin_codex", "1".into()),
        ("from_repo", "".into()),
        ("release_url", DEFAULT_RELEASE_URL.into()),
        ("installed_version", "v1.2.3".into()),
        ("installed_target", "x86_64-unknown-linux-musl".into()),
        ("version", "latest".into()),
        ("last_binary_path", binary.display().to_string()),
        ("last_binary_digest", digest(&binary)),
        ("last_completions_path", completion.display().to_string()),
        ("last_completions_digest", digest(&completion)),
        ("cli", "1".into()),
        ("skills_claude", "0".into()),
        ("skills_codex", "0".into()),
        ("selected_skills", "all".into()),
        ("agents_claude", "0".into()),
        ("agents_codex", "0".into()),
        ("add_to_path", "1".into()),
        ("mcp_enabled", "0".into()),
        ("standalone_bin_dir", h(".local/bin")),
        ("plugin_bin_dir", "".into()),
        ("data_dir", data.display().to_string()),
        ("claude_dir", h("claude")),
        ("codex_dir", h("codex")),
        ("codex_skills", h(".agents/skills")),
        ("codex_agents_dir", "".into()),
        ("completions_dir", "".into()),
        ("fish_completions_dir", h("config/fish/completions")),
        ("path_file", profile.display().to_string()),
        ("completions", "fish".into()),
        ("shell", "zsh".into()),
    ];
    let text: String = selections
        .iter()
        .map(|(k, v)| format!("{k}\t{v}\n"))
        .collect();
    fs::write(state.join("selections.tsv"), text).unwrap();
    let receipt = Receipt::from_installer(&state, &binary).unwrap();
    fs::write(
        state.join("receipt.json"),
        serde_json::to_string_pretty(&receipt).unwrap() + "\n",
    )
    .unwrap();
    (profile, original_profile)
}

fn walk(root: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_owned()];
    if fs::symlink_metadata(root).unwrap().is_dir() {
        for entry in fs::read_dir(root).unwrap() {
            out.extend(walk(&entry.unwrap().path()));
        }
    }
    out
}

#[test]
fn an_existing_install_written_by_the_shell_installer_is_updated_and_then_uninstalled_cleanly() {
    let mut case = Case::new();
    let (profile, original_profile) = shell_installed(&case);
    let receipt = json(&case.state().join("receipt.json"));
    // A newer release over the shell installer's state: nothing is a conflict.
    case.release = write_release(&case.root, "release-1.3.0", "1.3.0");
    let output = case.plugin(&["--plugin", "codex"]).ok();
    assert!(!output.contains("Back up and replace conflict"), "{output}");
    let binary = case.home("data/leanck/bin/leanck");
    assert!(
        fs::read_to_string(&binary)
            .unwrap()
            .contains("leanck 1.3.0")
    );
    let updated = Receipt::discover(&binary, None).unwrap().unwrap();
    assert_eq!(updated.version, "1.3.0");
    assert!(updated.generation > receipt["generation"].as_u64().unwrap());
    let selections = case.state_file("selections.tsv");
    assert!(selections.contains("installed_version\tv1.3.0\n"));
    assert!(selections.contains("completions\tfish\n"));
    // One record per path; the old ones were updated in place.
    let inventory = case.state_file("uninstall.tsv");
    let paths: Vec<&str> = inventory
        .lines()
        .map(|l| l.rsplit('\t').next().unwrap())
        .collect();
    let mut unique = paths.clone();
    unique.sort();
    unique.dedup();
    assert_eq!(unique.len(), paths.len(), "{inventory}");
    assert_eq!(
        fs::read_to_string(&profile)
            .unwrap()
            .matches("# >>> leanck PATH >>>")
            .count(),
        1
    );
    case.hosts.clear();
    case.uninstall(&[]);
    assert_eq!(fs::read_to_string(&profile).unwrap(), original_profile);
    for path in [
        "data/leanck/bin/leanck",
        "data/leanck/bin/.leanck-installation.json",
        "data/leanck/bundle",
        "data/leanck/agents",
        "codex/agents/leanck/lean-expert.toml",
        "codex/agents/leanck/lean-expert.toml.leanck-sha256",
        "config/leanck/config.example.toml",
        "config/fish/completions/leanck.fish",
        "state/leanck/installer/receipt.json",
    ] {
        assert!(fs::symlink_metadata(case.home(path)).is_err(), "{path}");
    }
    assert_eq!(case.hosts.argv().len(), 2);
}

#[test]
fn a_shell_installer_inventory_with_old_marker_bytes_uninstalls_cleanly() {
    let case = Case::new();
    let (profile, original_profile) = shell_installed(&case);
    // An older updater rewrote the marker as pretty JSON without a newline.
    let marker = case.home("data/leanck/bin/.leanck-installation.json");
    fs::write(
        &marker,
        serde_json::to_vec_pretty(&serde_json::json!({"state_dir": case.state()})).unwrap(),
    )
    .unwrap();
    case.uninstall(&[]);
    assert!(!marker.exists());
    assert!(!case.home("data/leanck/bin/leanck").exists());
    assert_eq!(fs::read_to_string(&profile).unwrap(), original_profile);
    assert!(!case.state().join("uninstall.tsv").exists());
    // A marker naming other state is someone else's and stays.
    let case = Case::new();
    shell_installed(&case);
    let marker = case.home("data/leanck/bin/.leanck-installation.json");
    fs::write(&marker, "{\"state_dir\":\"/elsewhere\"}\n").unwrap();
    let (result, _) = case.uninstall_with(&[], Approval::Approved);
    assert_eq!(result.unwrap(), Uninstalled::Incomplete);
    assert!(marker.exists());
}
