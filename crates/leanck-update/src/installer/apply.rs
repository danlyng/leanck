//! Applying a reviewed [`Plan`]: every file change goes through the durable
//! installation journal ([`Transaction`]), so a failure, or a crash followed by
//! recovery, restores every replaced path. Host registration runs after the
//! journal commits and reports `partial` instead of rolling back.
//!
//! Order of operations:
//! 1. Probe `codex plugin add --help` when registering Codex.
//! 2. Offer to stop daemons incompatible with the release being installed.
//! 3. Take the maintenance lock (as `_update lock-fd` did) and refuse while
//!    `leanck` processes that predate usage leases run either executable.
//! 4. Stage and commit each [`Step`]; then commit `uninstall.tsv`,
//!    `selections.tsv`, and, when this run installs the executable,
//!    `sources.tsv`, `receipt.json`, and the discovery markers.
//! 5. Register host plugins under a shared usage lease.
//! 6. Print the final messages.
use super::{
    Console, Environment, HostName, InventoryRecord, Mode, OWNERSHIP_MARKER, Plan, Recorded,
    ReleaseInventory, SavedState, Settings, Status, Step, UninstallInventory,
    environment::{PATH_BLOCK_END, PATH_BLOCK_START, balanced_markers, observe_node},
    hosts::{DaemonCheck, Daemons, HostCall, HostCli, relay, relay_captured, run_bounded},
    plan::{ExampleSource, sidecar},
    settings::CompletionShell,
};
use crate::{
    gate::{Lease, Maintenance, ensure_no_legacy_users},
    receipt::{Receipt, digest_file, tree_digest},
    safe_path,
    transaction::Transaction,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use std::{
    fs,
    io::Write,
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

const BUSY: &str =
    "installation is busy; close leanck/MCP clients and stop daemons before retrying";
const PARTIAL: &str = "Host plugin registration may be partial. Rerun the same installer selections to finish registration.";

/// External services `apply` uses, and the one question it may ask.
pub struct Runtime<'a> {
    pub hosts: &'a dyn HostCli,
    pub daemons: &'a dyn Daemons,
    /// Asked with the number of incompatible daemons; `true` stops them.
    /// Non-interactive runs (which require `--yes`) approve.
    pub confirm_daemon_stop: &'a mut dyn FnMut(usize) -> bool,
}

/// A committed installation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Applied {
    /// [`Status::Installed`], or [`Status::Partial`] when a host registration failed.
    pub status: Status,
    /// The directory holding this run's replacement backups, if any.
    pub backups: Option<PathBuf>,
    /// One message per host whose registration failed.
    pub registration_errors: Vec<String>,
}
impl Applied {
    /// The JSON `message`: registration errors, joined.
    pub fn message(&self) -> String {
        self.registration_errors.join("; ")
    }
}

/// Recover an interrupted install or update before planning, so the plan
/// sees restored files. Returns whether a journal was recovered.
pub fn recover_interrupted(state_dir: &Path) -> Result<bool> {
    let journal = state_dir.join("update-journal.json");
    if fs::symlink_metadata(&journal).is_err() {
        return Ok(false);
    }
    crate::install::recover(state_dir)
        .context("an interrupted installation could not be recovered")?;
    Ok(true)
}

/// Apply `plan`, made from `release` and `env`. On error, every replaced
/// path has been restored (or the journal is kept for recovery).
pub fn apply<W: Write>(
    plan: &Plan,
    release: &ReleaseInventory,
    env: &Environment,
    runtime: Runtime<'_>,
    console: &mut Console<W>,
) -> Result<Applied> {
    Installer::new(plan, release, env, console).run(runtime)
}

#[cfg(test)]
pub(crate) fn apply_failing_at<W: Write>(
    plan: &Plan,
    release: &ReleaseInventory,
    env: &Environment,
    runtime: Runtime<'_>,
    console: &mut Console<W>,
    replacement: usize,
) -> Result<Applied> {
    let mut installer = Installer::new(plan, release, env, console);
    installer.fail_at = Some(replacement);
    installer.run(runtime)
}

struct Installer<'a, W: Write> {
    plan: &'a Plan,
    s: &'a Settings,
    release: &'a ReleaseInventory,
    env: &'a Environment,
    console: &'a mut Console<W>,
    timeout: Duration,
    tx: Option<Transaction>,
    replacements: usize,
    /// Fail the Nth replacement (tests).
    fail_at: Option<usize>,
    /// Whether this run installs the executable, and so owns the receipt.
    owns_binary: bool,
    records: Vec<InventoryRecord>,
    sources: String,
    binary: Option<(PathBuf, String)>,
    completions: Option<(PathBuf, String)>,
    receipt: Option<Receipt>,
}

impl<'a, W: Write> Installer<'a, W> {
    fn new(
        plan: &'a Plan,
        release: &'a ReleaseInventory,
        env: &'a Environment,
        console: &'a mut Console<W>,
    ) -> Self {
        Installer {
            plan,
            s: &plan.settings,
            release,
            env,
            console,
            timeout: Duration::from_secs(plan.settings.timeout),
            tx: None,
            replacements: 0,
            fail_at: None,
            owns_binary: plan
                .steps
                .iter()
                .any(|s| matches!(s, Step::Executable { .. })),
            records: vec![],
            sources: String::new(),
            binary: None,
            completions: None,
            receipt: None,
        }
    }

    fn staged_binary(&self) -> PathBuf {
        self.release.root.join("plugin/bin/leanck")
    }

    fn run(mut self, runtime: Runtime<'_>) -> Result<Applied> {
        ensure!(
            !self.s.dry_run,
            "internal error: a dry run never applies changes"
        );
        let s = self.s;
        if let Some(codex) = self
            .plan
            .registrations
            .iter()
            .find(|r| r.host == HostName::Codex)
        {
            let ran = runtime.hosts.run(&HostCall::new(
                HostName::Codex,
                &codex.home,
                &["plugin", "add", "--help"],
            ));
            ensure!(
                ran.success(),
                "update Codex to a version with codex plugin add"
            );
        }
        if s.cli && self.owns_binary {
            self.offer_daemon_stop(runtime.daemons, runtime.confirm_daemon_stop);
        }

        create_state_dir(&s.state_dir)?;
        let lock = s.state_dir.join("maintenance.lock");
        if fs::symlink_metadata(&lock).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("refusing symlink maintenance lock");
        }
        let maintenance =
            Maintenance::acquire(&s.state_dir).map_err(|e| anyhow!(e).context(BUSY))?;
        let mut users = vec![s.binary()];
        if s.mode == Mode::Plugin {
            users.push(s.bundle_dir().join("plugin/bin/leanck"));
        }
        for binary in &users {
            ensure_no_legacy_users(binary).map_err(|e| e.context(BUSY))?;
        }
        if fs::symlink_metadata(s.state_dir.join("update-journal.json")).is_ok() {
            Transaction::recover(&s.state_dir)?;
            bail!(
                "recovered an interrupted installation; rerun the installer to review the restored files"
            );
        }

        self.tx = Some(Transaction::new(&s.state_dir)?);
        match self.install() {
            Ok(()) => {
                let tx = self.tx.as_mut().unwrap();
                let backups = tx.backup_directory().map(Path::to_owned);
                tx.commit()?;
                self.tx = None;
                let errors = self.register(runtime.hosts, maintenance);
                self.finish(backups.as_deref());
                Ok(Applied {
                    status: if errors.is_empty() {
                        Status::Installed
                    } else {
                        Status::Partial
                    },
                    backups,
                    registration_errors: errors,
                })
            }
            Err(error) => {
                let mut tx = self.tx.take().unwrap();
                let backups = tx.backup_directory().map(Path::to_owned);
                if self.replacements > 0 {
                    self.console
                        .warn("Restoring replaced files. Backups remain available.");
                }
                tx.rollback().with_context(|| {
                    format!(
                        "{error:#}; restoring replaced files also failed, so update-journal.json is kept for recovery"
                    )
                })?;
                if let Some(dir) = backups {
                    self.console
                        .info(&format!("Replacement backups: {}", dir.display()));
                }
                Err(error)
            }
        }
    }

    fn offer_daemon_stop(&mut self, daemons: &dyn Daemons, confirm: &mut dyn FnMut(usize) -> bool) {
        let binary = self.staged_binary();
        let count = match daemons.incompatible(&binary) {
            Ok(count) => count,
            Err(DaemonCheck::Failed) => {
                self.console
                    .warn("Could not check for incompatible leanck daemons.");
                return;
            }
            Err(DaemonCheck::Unreadable) => {
                self.console
                    .warn("Could not read the incompatible daemon count.");
                return;
            }
        };
        if count == 0 {
            return;
        }
        self.console.warn(&format!(
            "{count} incompatible leanck daemon(s) are running."
        ));
        if !confirm(count) {
            return;
        }
        let ran = daemons.stop_incompatible(&binary);
        relay(self.console, "", &ran);
        if !ran.success() {
            self.console.warn(
                "Could not stop all incompatible leanck daemons; retry with leanck daemons stop --incompatible.",
            );
        }
    }

    /// Stage and commit every step and the state files.
    fn install(&mut self) -> Result<()> {
        let steps = &self.plan.steps;
        for (index, step) in steps.iter().enumerate() {
            self.step(step)?;
            if let Step::Agent { destination, .. } = step {
                let dir = destination.parent().unwrap_or(Path::new("/"));
                let next = steps.get(index + 1).and_then(|next| match next {
                    Step::Agent { destination, .. } => destination.parent(),
                    _ => None,
                });
                if next != Some(dir) {
                    self.console
                        .info(&format!("Installed agents in {}", dir.display()));
                }
            }
        }
        self.save_state()
    }

    /// Commit `staged` at `destination` through the journal.
    fn replace(&mut self, staged: Option<PathBuf>, destination: &Path) -> Result<()> {
        self.replacements += 1;
        if self.fail_at == Some(self.replacements) {
            bail!("injected failure at replacement {}", self.replacements);
        }
        self.tx
            .as_mut()
            .context("internal error: no installation journal")?
            .replace(staged, destination.to_owned())
            .with_context(|| format!("could not install {}", destination.display()))
    }

    fn record(&mut self, kind: &str, fingerprint: &str, destination: &Path, source: Option<&str>) {
        self.records
            .push(InventoryRecord::new(kind, fingerprint, destination));
        if let Some(source) = source {
            self.sources.push_str(&format!(
                "{kind}\t{fingerprint}\t{}\t{source}\n",
                destination.display()
            ));
        }
    }

    fn step(&mut self, step: &Step) -> Result<()> {
        match step {
            Step::MigrateCodexAgent { legacy } => self.migrate_agent(legacy),
            Step::Executable { destination } => self.install_executable(destination),
            Step::Tree {
                source,
                destination,
            } => {
                let from = self.release.root.join(source);
                self.install_tree(&from, destination, source, None)
            }
            Step::Bundle {
                destination,
                binary,
                mcp_enabled,
            } => {
                let root = self.release.root.clone();
                self.install_tree(&root, destination, ".", Some((binary, *mcp_enabled)))
            }
            Step::Agent {
                source,
                destination,
                link,
            } => self.install_agent(source, destination, link.as_deref()),
            Step::ConfigExample {
                destination,
                source,
            } => self.install_config_example(destination, source),
            Step::Completions { shell, destination } => {
                self.install_completions(*shell, destination)
            }
            Step::Profile { file, line } => self.update_profile(file, line),
        }
    }

    fn migrate_agent(&mut self, legacy: &Path) -> Result<()> {
        ensure!(
            unchanged_managed_agent(legacy),
            "{} changed after review; rerun the installer",
            legacy.display()
        );
        self.replace(None, legacy)?;
        self.replace(None, &sidecar(legacy, "leanck-installed"))?;
        self.replace(None, &sidecar(legacy, "leanck-sha256"))
    }

    fn install_executable(&mut self, destination: &Path) -> Result<()> {
        ensure!(
            !destination.is_dir(),
            "{} is a directory",
            destination.display()
        );
        let stage = stage_beside(destination)?;
        let staged = stage.path().join("leanck");
        fs::copy(self.staged_binary(), &staged)?;
        set_mode(&staged, 0o755)?;
        let mut command = Command::new(&staged);
        command.arg("--version");
        let ran = run_bounded(command, self.timeout, "leanck --version");
        relay(self.console, "", &ran);
        let expected = format!("leanck {}", self.release.version());
        let printed = String::from_utf8_lossy(&ran.captured.stdout);
        ensure!(
            ran.success() && printed.trim() == expected,
            "the release executable failed its version check (expected {expected:?}, got {:?}); nothing was installed",
            printed.trim()
        );
        self.replace(Some(staged), destination)?;
        let digest = digest_file(destination)?;
        self.record("file", &digest, destination, Some("plugin/bin/leanck"));
        self.binary = Some((destination.to_owned(), digest));
        self.console.info(&format!(
            "Installed {} ({})",
            destination.display(),
            self.plan.version
        ));
        Ok(())
    }

    /// Install a managed tree; `bundle` configures the plugin for `binary`.
    fn install_tree(
        &mut self,
        from: &Path,
        destination: &Path,
        source: &str,
        bundle: Option<(&Path, bool)>,
    ) -> Result<()> {
        let stage = stage_beside(destination)?;
        let tree = stage.path().join("tree");
        copy_tree(from, &tree)?;
        if let Some((binary, mcp_enabled)) = bundle {
            crate::install::configure_plugin(&tree.join("plugin"), binary, mcp_enabled)
                .map_err(|e| anyhow!("plugin configuration failed: {e:#}"))?;
            if self.owns_binary {
                // Discovery markers are installation metadata, outside the tree digest.
                write_file(
                    &tree.join("plugin/bin/.leanck-installation.json"),
                    &Receipt::marker_bytes(&self.s.state_dir)?,
                    0o644,
                )?;
            }
        }
        write_file(
            &tree.join(".leanck-installed"),
            format!("{OWNERSHIP_MARKER}\n").as_bytes(),
            0o644,
        )?;
        let digest = tree_digest(&tree)?;
        write_file(
            &tree.join(".leanck-content-sha256"),
            format!("{digest}\n").as_bytes(),
            0o644,
        )?;
        self.replace(Some(tree), destination)?;
        self.record("tree", &digest, destination, Some(source));
        self.console
            .info(&format!("Installed {}", destination.display()));
        Ok(())
    }

    fn install_agent(
        &mut self,
        source: &str,
        destination: &Path,
        link: Option<&Path>,
    ) -> Result<()> {
        ensure!(
            !fs::symlink_metadata(destination).is_ok_and(|m| m.is_dir()),
            "{} is a directory",
            destination.display()
        );
        let stage = stage_beside(destination)?;
        let staged = stage.path().join("agent");
        match link {
            Some(target) => std::os::unix::fs::symlink(target, &staged)?,
            None => {
                fs::copy(self.release.root.join(source), &staged)?;
                set_mode(&staged, 0o644)?;
            }
        }
        self.replace(Some(staged), destination)?;
        // A link's sidecar records the digest of the file it resolves to.
        let digest = digest_file(destination)?;
        for (suffix, content) in [
            ("leanck-installed", OWNERSHIP_MARKER),
            ("leanck-sha256", digest.as_str()),
        ] {
            let path = stage.path().join(suffix);
            write_file(&path, format!("{content}\n").as_bytes(), 0o644)?;
            self.replace(Some(path), &sidecar(destination, suffix))?;
        }
        match link {
            Some(target) => {
                let target = target.to_string_lossy().into_owned();
                self.record("link", &target, destination, Some(&target));
            }
            None => self.record("agent", &digest, destination, Some(source)),
        }
        Ok(())
    }

    fn install_config_example(&mut self, destination: &Path, source: &ExampleSource) -> Result<()> {
        // Never replace a configuration example, even one created since review.
        if observe_node(destination)?.present() {
            return Ok(());
        }
        let bytes = match source {
            ExampleSource::Release(relative) => fs::read(self.release.root.join(relative))?,
            ExampleSource::Path(path) => fs::read(path)?,
            ExampleSource::Url(url) => download(url, self.timeout)?,
        };
        let stage = stage_beside(destination)?;
        let staged = stage.path().join("example");
        write_file(&staged, &bytes, 0o644)?;
        self.replace(Some(staged), destination)?;
        self.record("file", &digest_file(destination)?, destination, None);
        self.console.info(&format!(
            "Installed configuration example: {}",
            destination.display()
        ));
        Ok(())
    }

    fn install_completions(&mut self, shell: CompletionShell, destination: &Path) -> Result<()> {
        let mut command = Command::new(self.staged_binary());
        command.args(["_completions", shell.as_str()]);
        let ran = run_bounded(
            command,
            self.timeout,
            &format!("leanck _completions {}", shell.as_str()),
        );
        // Standard output is the script; only diagnostics are relayed.
        relay_captured(self.console, "", &ran.captured, false);
        if let Err(failure) = &ran.result
            && let Some(line) = failure.line()
        {
            self.console.error(&line);
        }
        ensure!(ran.success(), "completion generation failed");
        let stage = stage_beside(destination)?;
        let staged = stage.path().join("completions");
        write_file(&staged, &ran.captured.stdout, 0o644)?;
        self.replace(Some(staged), destination)?;
        let digest = digest_file(destination)?;
        self.record("completion", &digest, destination, Some(shell.as_str()));
        self.completions = Some((destination.to_owned(), digest));
        Ok(())
    }

    fn update_profile(&mut self, file: &Path, line: &str) -> Result<()> {
        let meta = fs::symlink_metadata(file)?;
        ensure!(
            !meta.file_type().is_symlink(),
            "refusing symlink profile: {}",
            file.display()
        );
        ensure!(
            meta.is_file(),
            "shell profile is not a regular file: {}",
            file.display()
        );
        let text = fs::read(file)?;
        ensure!(
            balanced_markers(&text),
            "unbalanced leanck PATH markers in {}; repair the block before retrying",
            file.display()
        );
        let stage = stage_beside(file)?;
        let staged = stage.path().join("profile");
        write_file(
            &staged,
            &rewrite_profile(&text, line),
            meta.permissions().mode() & 0o7777,
        )?;
        self.replace(Some(staged), file)?;
        self.record("profile", "-", file, None);
        Ok(())
    }

    /// Commit `uninstall.tsv`, `selections.tsv`, and ownership records.
    fn save_state(&mut self) -> Result<()> {
        let s = self.s;
        let state = &s.state_dir;
        let saved = SavedState::load(state)?;
        let mut inventory = UninstallInventory::load(state)?;
        inventory.merge(std::mem::take(&mut self.records));
        if self
            .plan
            .steps
            .iter()
            .any(|step| matches!(step, Step::Bundle { .. }))
        {
            inventory.merge(
                self.plan
                    .registrations
                    .iter()
                    .map(|r| InventoryRecord::new("host", r.host.command(), &r.home)),
            );
        }
        let binary = s.binary();
        let marker = Receipt::marker(&binary);
        let marker_bytes = Receipt::marker_bytes(state)?;
        if self.owns_binary {
            let digest = hex::encode(<sha2::Sha256 as sha2::Digest>::digest(&marker_bytes));
            inventory.merge([InventoryRecord::new("file", digest, &marker)]);
        }
        self.write_state("uninstall.tsv", inventory.render().as_bytes())?;
        let recorded = Recorded {
            installed_version: self.plan.version.clone(),
            installed_target: self.env.target.clone(),
            binary: self.binary.clone(),
            completions: self.completions.clone(),
        };
        self.write_state("selections.tsv", saved.render(s, &recorded).as_bytes())?;
        if !self.owns_binary {
            return Ok(());
        }
        let sources = std::mem::take(&mut self.sources);
        self.write_state("sources.tsv", sources.as_bytes())?;
        let previous = fs::read(state.join("receipt.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Receipt>(&bytes).ok());
        let mut receipt = Receipt::from_installer(state, &binary)
            .context("could not record native update ownership")?;
        if let Some(previous) = previous {
            // Running clients compare generations to notice a replacement.
            receipt.generation = receipt.generation.max(previous.generation + 1);
        }
        self.write_state("receipt.json", &serde_json::to_vec_pretty(&receipt)?)?;
        let stage = stage_beside(&marker)?;
        let staged = stage.path().join("marker");
        write_file(&staged, &marker_bytes, 0o644)?;
        self.replace(Some(staged), &marker)?;
        self.receipt = Some(receipt);
        Ok(())
    }

    fn write_state(&mut self, name: &str, bytes: &[u8]) -> Result<()> {
        let destination = self.s.state_dir.join(name);
        let stage = stage_beside(&destination)?;
        let staged = stage.path().join(name);
        write_file(&staged, bytes, 0o600)?;
        self.replace(Some(staged), &destination)
    }

    /// Register host plugins, continuing past a failing host.
    fn register(&mut self, hosts: &dyn HostCli, maintenance: Maintenance) -> Vec<String> {
        let registrations = &self.plan.registrations;
        if registrations.is_empty() {
            if self.s.mode == Mode::Plugin
                && self
                    .plan
                    .steps
                    .iter()
                    .any(|step| matches!(step, Step::Bundle { .. }))
            {
                self.console.info(&format!(
                    "Plugin files are ready in {} for registration when an agent host is installed.",
                    self.s.bundle_dir().display()
                ));
            }
            return vec![];
        }
        // Host hooks may run the installed leanck, which takes a shared usage
        // lease; hold one ourselves instead of the exclusive maintenance lock.
        drop(maintenance);
        let _lease = match &self.receipt {
            Some(receipt) => match Lease::acquire(receipt) {
                Ok(lease) => Some(lease),
                Err(error) => {
                    self.console.warn(PARTIAL);
                    return vec![format!("{error:#}")];
                }
            },
            None => None,
        };
        let registration = crate::lock_file(&self.s.state_dir.join("registration.lock"));
        if !registration
            .as_ref()
            .is_ok_and(|file| file.try_lock().is_ok())
        {
            self.console.warn(PARTIAL);
            return vec!["host registration is already in progress".into()];
        }
        let mut errors = vec![];
        for registration in registrations {
            let host = registration.host;
            let bundle = registration.bundle.to_string_lossy().into_owned();
            let install: &[&str] = match host {
                HostName::Claude => &["plugin", "install", "leanck@leanck", "--scope", "user"],
                HostName::Codex => &["plugin", "add", "leanck@leanck"],
            };
            let calls: [(&[&str], &str); 2] = [
                (
                    &["plugin", "marketplace", "add", &bundle],
                    "marketplace registration failed",
                ),
                (install, "plugin installation failed"),
            ];
            for (args, failure) in calls {
                let mut call = HostCall::new(host, &registration.home, args);
                if self.s.cli {
                    call.path_prefix = Some(self.s.bin_dir.clone());
                }
                let ran = hosts.run(&call);
                relay(self.console, host.command(), &ran);
                if !ran.success() {
                    errors.push(format!("{} {failure}; rerun to resume", host.title()));
                    break;
                }
            }
        }
        if !errors.is_empty() {
            self.console.warn(PARTIAL);
            return errors;
        }
        if let Some(receipt) = &mut self.receipt {
            receipt.registration_pending = false;
            if let Err(error) = crate::atomic_json(&receipt.path(), receipt) {
                self.console.warn(PARTIAL);
                return vec![format!(
                    "could not record completed plugin registration: {error:#}"
                )];
            }
        }
        self.console
            .info("✅Plugins registered. Restart agents to load the plugin.");
        errors
    }

    fn finish(&mut self, backups: Option<&Path>) {
        let s = self.s;
        if let Some(dir) = backups {
            self.console
                .info(&format!("Replacement backups: {}", dir.display()));
        }
        if s.cli {
            if !self.env.on_path(&s.bin_dir) {
                let profile_updated = self
                    .plan
                    .steps
                    .iter()
                    .any(|step| matches!(step, Step::Profile { .. }));
                if profile_updated {
                    self.console.info(
                        "Open a new shell to load the PATH change; restart the agent host as needed.",
                    );
                } else {
                    self.console.info(&format!(
                        "Add {} to PATH in your shell and agent host.",
                        s.bin_dir.display()
                    ));
                }
            }
            if let Some((path, _)) = &self.completions {
                self.console.info(&format!(
                    "Shell completions ({}): {}",
                    s.completions.as_str(),
                    path.display()
                ));
                let hint = match s.completions {
                    CompletionShell::Zsh => {
                        Some("Include the completions directory in fpath before calling compinit.")
                    }
                    CompletionShell::Powershell => {
                        Some("Load the completion script from your PowerShell profile.")
                    }
                    CompletionShell::Elvish => Some("Load the completion module with use leanck."),
                    _ => None,
                };
                if let Some(hint) = hint {
                    self.console.info(hint);
                }
            }
        } else if !self.env.has_command("leanck") {
            self.console
                .info("Standalone skills and agents require an existing leanck on PATH.");
        }
        if !self.console.quiet {
            let style = self.console.style;
            self.console
                .raw(&format!("{}Done.{}\n", style.bold, style.reset));
        }
        if !self.env.has_command("lake") {
            self.console
                .warn("`lake` is not detected. Install info: https://lean-lang.org/install/");
        }
    }
}

/// A flat-layout Codex agent this installer wrote and nobody edited.
pub(crate) fn unchanged_managed_agent(path: &Path) -> bool {
    let regular = |p: &Path| fs::symlink_metadata(p).is_ok_and(|m| m.is_file());
    let text = |p: &Path| fs::read_to_string(p).ok();
    let marker = sidecar(path, "leanck-installed");
    let digest = sidecar(path, "leanck-sha256");
    fs::metadata(path).is_ok_and(|m| m.is_file())
        && regular(&marker)
        && regular(&digest)
        && text(&marker).is_some_and(|t| t.trim_end_matches('\n') == OWNERSHIP_MARKER)
        && digest_file(path)
            .ok()
            .zip(text(&digest))
            .is_some_and(|(actual, recorded)| recorded.trim_end_matches('\n') == actual)
}

/// Create the state directory privately; parents get the usual 0755.
fn create_state_dir(state: &Path) -> Result<()> {
    safe_path(state)?;
    if let Some(parent) = state.parent() {
        create_dirs(parent)?;
    }
    if fs::symlink_metadata(state).is_err() {
        fs::create_dir(state)?;
        set_mode(state, 0o700)?;
    }
    Ok(())
}

/// Create missing directories with mode 0755, regardless of the umask.
pub(crate) fn create_dirs(path: &Path) -> Result<()> {
    let mut missing = vec![];
    for ancestor in path.ancestors() {
        if fs::symlink_metadata(ancestor).is_ok() {
            break;
        }
        missing.push(ancestor);
    }
    for dir in missing.into_iter().rev() {
        match fs::create_dir(dir) {
            Ok(()) => set_mode(dir, 0o755)?,
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e).with_context(|| format!("cannot create {}", dir.display())),
        }
    }
    Ok(())
}

/// A private staging directory beside `destination`, so commits are renames.
fn stage_beside(destination: &Path) -> Result<tempfile::TempDir> {
    let parent = destination
        .parent()
        .context("destination has no parent directory")?;
    safe_path(parent)?;
    create_dirs(parent)?;
    Ok(tempfile::Builder::new()
        .prefix(".leanck-install.")
        .tempdir_in(parent)?)
}

pub(crate) fn set_mode(path: &Path, mode: u32) -> Result<()> {
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
        .with_context(|| format!("cannot set permissions on {}", path.display()))
}

pub(crate) fn write_file(path: &Path, bytes: &[u8], mode: u32) -> Result<()> {
    let mut file = fs::File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    set_mode(path, mode)
}

/// Copy a release tree: directories 0755, files 0644, executables 0755.
/// Links and special files are refused; managed trees contain neither.
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        let target = destination.join(entry.file_name());
        if ty.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else if ty.is_file() {
            let executable = entry.metadata()?.permissions().mode() & 0o111 != 0;
            fs::copy(entry.path(), &target)?;
            set_mode(&target, if executable { 0o755 } else { 0o644 })?;
        } else {
            bail!(
                "release tree contains a link or special file: {}",
                entry.path().display()
            );
        }
    }
    set_mode(destination, 0o755)
}

fn download(url: &str, timeout: Duration) -> Result<Vec<u8>> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    runtime.block_on(async {
        let response = tokio::time::timeout(timeout, reqwest::get(url))
            .await
            .with_context(|| format!("download timed out: {url}"))??;
        ensure!(
            response.status().is_success(),
            "download failed: {url}; check the release/tag and network connection"
        );
        Ok(tokio::time::timeout(timeout, response.bytes())
            .await
            .with_context(|| format!("download timed out: {url}"))??
            .to_vec())
    })
}

/// Remove the managed PATH block, and the blank line written before it.
pub(crate) fn strip_profile_block(text: &[u8]) -> Vec<u8> {
    let mut lines: Vec<&[u8]> = text.split_inclusive(|&b| b == b'\n').collect();
    let trim = |line: &[u8]| -> Vec<u8> { line.strip_suffix(b"\n").unwrap_or(line).to_vec() };
    let mut out: Vec<&[u8]> = vec![];
    let mut skipping = false;
    for line in lines.drain(..) {
        let bare = trim(line);
        if bare == PATH_BLOCK_START.as_bytes() {
            if out.last().is_some_and(|last| trim(last).is_empty()) {
                out.pop();
            }
            skipping = true;
        } else if bare == PATH_BLOCK_END.as_bytes() {
            skipping = false;
        } else if !skipping {
            out.push(line);
        }
    }
    out.concat()
}

/// Replace the managed PATH block of a profile with one containing `line`.
/// Rewriting is idempotent: blank lines do not accumulate.
pub(crate) fn rewrite_profile(text: &[u8], line: &str) -> Vec<u8> {
    let mut out = strip_profile_block(text);
    if !out.is_empty() {
        if !out.ends_with(b"\n") {
            out.push(b'\n');
        }
        out.push(b'\n');
    }
    out.extend_from_slice(format!("{PATH_BLOCK_START}\n{line}\n{PATH_BLOCK_END}\n").as_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn profile_rewrites_are_idempotent_and_keep_surrounding_text() {
        let line = "export PATH='/b':\"$PATH\"";
        let once = rewrite_profile(b"# user\nalias x=y", line);
        assert_eq!(
            String::from_utf8(once.clone()).unwrap(),
            format!("# user\nalias x=y\n\n{PATH_BLOCK_START}\n{line}\n{PATH_BLOCK_END}\n")
        );
        assert_eq!(rewrite_profile(&once, line), once);
        assert_eq!(strip_profile_block(&once), b"# user\nalias x=y\n");
        let empty = rewrite_profile(b"", line);
        assert!(empty.starts_with(PATH_BLOCK_START.as_bytes()));
        // A block in the middle keeps the text after it.
        let middle = format!("a\n\n{PATH_BLOCK_START}\nold\n{PATH_BLOCK_END}\nb\n");
        assert_eq!(strip_profile_block(middle.as_bytes()), b"a\nb\n");
    }
}
