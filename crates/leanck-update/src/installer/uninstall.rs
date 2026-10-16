//! `leanck _uninstall`: remove installer-managed files and host plugin
//! registrations recorded in `STATE/uninstall.tsv`, preserving anything
//! changed since installation.
//!
//! This replaces `uninstall.sh`'s logic and reads every state layout the
//! shell installer, the updater, and `_install` have written: inventories in
//! any record order, `completion` records written as `file`, discovery
//! markers in compact or pretty form, and state from installers old enough
//! to have no inventory at all (discovered from `selections.tsv`).
use super::{
    Environment, HostName, OWNERSHIP_MARKER, SavedState,
    apply::{set_mode, strip_profile_block, write_file},
    environment::balanced_markers,
    hosts::{Daemons, HostCall, HostCli, Ran, SystemDaemons, SystemHosts, is_executable},
    plan::sidecar,
    settings::DEFAULT_TIMEOUT,
};
use crate::{
    gate::{Maintenance, ensure_no_legacy_users},
    paths::absolute,
    receipt::{digest_file, tree_digest},
    safe_dir, safe_path,
    transaction::Transaction,
};
use anyhow::{Context, Result, bail, ensure};
use clap::{CommandFactory, FromArgMatches, Parser};
use std::{
    ffi::OsString,
    fs,
    io::{BufRead, Write},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
    time::Duration,
};

const AFTER_HELP: &str = "\
Modified files, user configuration, Lean projects, caches, and installer backups
are preserved. The recorded executable is used to stop leanck daemons before removal.

Examples:
  leanck _uninstall --dry-run
  leanck _uninstall --yes
  leanck _uninstall --state-dir '/custom state/leanck/installer'";

/// Flags of `leanck _uninstall`.
#[derive(Parser, Debug, Clone, Default)]
#[command(
    name = "leanck _uninstall",
    about = "Remove installer-managed leanck files and host plugin registrations.",
    after_help = AFTER_HELP,
    disable_version_flag = true,
    args_override_self = true
)]
pub struct UninstallArgs {
    /// Approve removal without prompting.
    #[arg(short = 'y', long)]
    pub yes: bool,
    /// Preview only; never run leanck or host commands.
    #[arg(long)]
    pub dry_run: bool,
    /// Installer state (default: $XDG_STATE_HOME/leanck/installer).
    #[arg(long, value_name = "DIR", allow_hyphen_values = true)]
    pub state_dir: Option<String>,
    /// Also unregister claude, codex, or both (for older installs).
    #[arg(long, value_name = "HOSTS", allow_hyphen_values = true)]
    pub plugin: Option<String>,
    /// Maximum time for each external command (default: 300).
    #[arg(long, value_name = "SECONDS", allow_hyphen_values = true)]
    pub timeout: Option<String>,
}

impl UninstallArgs {
    pub fn command() -> clap::Command {
        <Self as CommandFactory>::command()
    }
}

/// How removal was approved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    Approved,
    Declined,
    /// The terminal closed or was interrupted.
    Interrupted,
}

/// How an uninstall ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Uninstalled {
    /// No installer state exists.
    NothingFound,
    /// A dry run listed what would be removed.
    Preview,
    Cancelled,
    /// Every record was removed, with the state files.
    Complete,
    /// Changed paths were preserved and remain recorded.
    Incomplete,
}

/// Resolved `_uninstall` options.
#[derive(Clone, Debug)]
pub struct UninstallOptions {
    pub state_dir: PathBuf,
    pub dry_run: bool,
    /// Hosts to unregister for state that did not record them.
    pub plugin: Option<(bool, bool)>,
}

impl UninstallOptions {
    pub fn resolve(args: &UninstallArgs, env: &Environment) -> Result<(Self, Duration)> {
        let timeout = match &args.timeout {
            None => DEFAULT_TIMEOUT,
            Some(value) => match value.parse::<u64>() {
                Ok(n) if n > 0 && value.bytes().all(|b| b.is_ascii_digit()) => n,
                _ => bail!("--timeout expects a positive integer"),
            },
        };
        let plugin = match args.plugin.as_deref() {
            None | Some("none") => None,
            Some("claude") => Some((true, false)),
            Some("codex") => Some((false, true)),
            Some("both") => Some((true, true)),
            Some(_) => bail!("--plugin expects claude, codex, or both"),
        };
        let state_dir = match &args.state_dir {
            Some(dir) => {
                ensure!(
                    !dir.chars().any(char::is_control),
                    "control characters in path"
                );
                absolute(dir, &env.home, &env.cwd)?
            }
            None => env.state_home().join("leanck/installer"),
        };
        Ok((
            UninstallOptions {
                state_dir,
                dry_run: args.dry_run,
                plugin,
            },
            Duration::from_secs(timeout),
        ))
    }
}

/// One inventory record.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Record {
    kind: String,
    expected: String,
    path: PathBuf,
}
impl Record {
    fn new(kind: &str, expected: impl Into<String>, path: impl Into<PathBuf>) -> Self {
        Record {
            kind: kind.into(),
            expected: expected.into(),
            path: path.into(),
        }
    }
    fn line(&self) -> String {
        format!(
            "{}\t{}\t{}\n",
            self.kind,
            self.expected,
            self.path.display()
        )
    }
    fn host(&self) -> Option<HostName> {
        matches!(self.kind.as_str(), "host" | "marketplace")
            .then(|| HostName::parse(&self.expected))
            .flatten()
    }
    /// Hosts first, then marketplaces, files, and trees last, so agents are
    /// unlinked before the trees they point into.
    fn rank(&self) -> u8 {
        match self.kind.as_str() {
            "host" => 0,
            "marketplace" => 1,
            "tree" => 3,
            _ => 2,
        }
    }
    /// A recorded `leanck` executable.
    fn is_executable_record(&self) -> bool {
        self.kind == "file"
            && self.path.file_name().is_some_and(|n| n == "leanck")
            && is_executable(&self.path)
    }
}

/// Run `leanck _uninstall`; returns the exit code.
pub fn main(argv: Vec<OsString>) -> i32 {
    let say = |message: &str| eprintln!("{message}");
    let die = |message: &str| {
        eprintln!("leanck-uninstall: {message}");
        1
    };
    let argv = std::iter::once(OsString::from("leanck _uninstall")).chain(argv);
    let args = match UninstallArgs::command()
        .try_get_matches_from(argv)
        .and_then(|m| UninstallArgs::from_arg_matches(&m))
    {
        Ok(args) => args,
        Err(error) => {
            use clap::error::ErrorKind;
            if matches!(error.kind(), ErrorKind::DisplayHelp) {
                print!("{}", UninstallArgs::command().render_help());
                return 0;
            }
            let message = super::parse_error_message(&error)
                .unwrap_or_else(|| error.to_string())
                .replace("needs a value", "requires a value");
            return die(&message);
        }
    };
    let result = (|| -> Result<Uninstalled> {
        let env = Environment::capture()?;
        let (options, timeout) = UninstallOptions::resolve(&args, &env)?;
        let hosts = SystemHosts::new(timeout);
        let daemons = SystemDaemons { timeout };
        let mut approve = || {
            if args.yes {
                return Ok(Approval::Approved);
            }
            prompt_terminal()
        };
        uninstall(
            &options,
            &env,
            &hosts,
            &daemons,
            &mut approve,
            &mut std::io::stderr(),
        )
    })();
    match result {
        Ok(Uninstalled::Incomplete) => 1,
        Ok(_) => 0,
        Err(error) if error.downcast_ref::<Interrupted>().is_some() => {
            say("");
            130
        }
        Err(error) => die(&format!("{error:#}")),
    }
}

#[derive(Debug)]
struct Interrupted;
impl std::fmt::Display for Interrupted {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("interrupted")
    }
}
impl std::error::Error for Interrupted {}

fn prompt_terminal() -> Result<Approval> {
    let Ok(tty) = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("/dev/tty")
    else {
        bail!("use --yes to approve removal without a terminal");
    };
    eprint!("Uninstall leanck? [y/N]: ");
    let mut line = String::new();
    match std::io::BufReader::new(tty).read_line(&mut line) {
        Ok(0) | Err(_) => Ok(Approval::Interrupted),
        Ok(_) => Ok(
            if matches!(
                line.trim_end_matches(['\n', '\r']),
                "y" | "Y" | "yes" | "YES"
            ) {
                Approval::Approved
            } else {
                Approval::Declined
            },
        ),
    }
}

/// Uninstall per `options`. Messages go to `out`, one per line.
pub fn uninstall(
    options: &UninstallOptions,
    env: &Environment,
    hosts: &dyn HostCli,
    daemons: &dyn Daemons,
    approve: &mut dyn FnMut() -> Result<Approval>,
    out: &mut dyn Write,
) -> Result<Uninstalled> {
    let mut say = |message: &str| {
        let _ = writeln!(out, "{message}");
    };
    let state = &options.state_dir;
    valid_path(state)?;
    safe_path(&state.join("selections.tsv"))
        .map_err(|_| anyhow::anyhow!("refusing symlink installer state directory"))?;
    for name in ["selections.tsv", "uninstall.tsv"] {
        let path = state.join(name);
        if fs::symlink_metadata(&path).is_ok_and(|m| m.file_type().is_symlink()) {
            bail!("refusing symlink: {}", path.display());
        }
    }
    let inventory_path = state.join("uninstall.tsv");
    let selections_path = state.join("selections.tsv");
    if !inventory_path.is_file() && !selections_path.is_file() {
        say(&format!(
            "No leanck installer state found at {}; nothing to remove.",
            state.display()
        ));
        return Ok(Uninstalled::NothingFound);
    }
    let saved = SavedState::load(state)?;
    let mut records = if inventory_path.is_file() {
        parse_inventory(&fs::read_to_string(&inventory_path)?)?
    } else {
        legacy_inventory(&saved, env, options.plugin.is_some())?
    };
    if let Some((claude, codex)) = options.plugin {
        let home = |key: &str, from_env: &Option<PathBuf>, default: &str| -> PathBuf {
            saved
                .value(key)
                .map(PathBuf::from)
                .or_else(|| from_env.clone())
                .unwrap_or_else(|| env.home.join(default))
        };
        if claude {
            records.push(Record::new(
                "host",
                "claude",
                home("claude_dir", &env.claude_config_dir, ".claude"),
            ));
        }
        if codex {
            records.push(Record::new(
                "host",
                "codex",
                home("codex_dir", &env.codex_home, ".codex"),
            ));
        }
    }
    // Host removal and marketplace removal have separate records, so a retry
    // never repeats a completed host step.
    let mut expanded: Vec<Record> = vec![];
    for record in records {
        let marketplace = (record.kind == "host")
            .then(|| Record::new("marketplace", &record.expected, &record.path));
        for record in std::iter::once(record).chain(marketplace) {
            if !expanded.contains(&record) {
                expanded.push(record);
            }
        }
    }
    expanded.sort_by_key(Record::rank);
    let mut remaining = expanded.clone();
    for record in &expanded {
        valid_path(&record.path)?;
        match record.kind.as_str() {
            "host" | "marketplace" => ensure!(record.host().is_some(), "invalid host receipt"),
            "tree" | "file" | "agent" | "link" | "profile" => {}
            kind => bail!("invalid uninstall inventory kind: {kind}"),
        }
        if record.host().is_none() && present(&record.path) && !unchanged(record, state) {
            say(&format!(
                "Preserve changed or unsafe path: {}",
                record.path.display()
            ));
        } else {
            say(&format!(
                "Would remove {}: {}",
                record.kind,
                record.path.display()
            ));
            if record.is_executable_record() {
                say(&format!(
                    "Would stop leanck daemons using {} daemons stop --all",
                    record.path.display()
                ));
            }
        }
    }
    if options.dry_run {
        say("Dry run complete; no installed files or host configuration changed.");
        return Ok(Uninstalled::Preview);
    }
    match approve()? {
        Approval::Approved => {}
        Approval::Declined => {
            say("Cancelled.");
            return Ok(Uninstalled::Cancelled);
        }
        Approval::Interrupted => return Err(Interrupted.into()),
    }
    save_remaining(state, &remaining)?;

    // Stop daemons before any removal, but only run an unchanged, recorded binary.
    let executable = expanded
        .iter()
        .find(|r| r.is_executable_record() && unchanged(r, state));
    if let Some(record) = executable {
        let ran = daemons.stop_all(&record.path);
        relay_raw(&mut say, &ran);
        ensure!(
            ran.success(),
            "could not stop leanck daemons; files preserved"
        );
    }
    // Remaining MCP/CLI users make removal defer without mutation.
    let in_use = "installation is in use; close leanck/MCP clients before retrying";
    let lock = state.join("maintenance.lock");
    ensure!(
        !fs::symlink_metadata(&lock).is_ok_and(|m| m.file_type().is_symlink()),
        "refusing symlink maintenance lock"
    );
    let _maintenance = Maintenance::acquire(state).context(in_use)?;
    if let Some(record) = executable {
        ensure_no_legacy_users(&record.path).context(in_use)?;
    }
    // An interrupted install or update restores its files before removal.
    if fs::symlink_metadata(state.join("update-journal.json")).is_ok() {
        Transaction::recover(state)?;
    }

    for record in &expanded {
        if !remove(record, state, hosts, &mut say)? {
            continue;
        }
        if let Some(index) = remaining.iter().position(|r| r == record) {
            remaining.remove(index);
        }
        save_remaining(state, &remaining)?;
    }
    if !remaining.is_empty() {
        say(&format!(
            "Uninstall incomplete: edited files were preserved. Remaining paths are recorded in {}.",
            inventory_path.display()
        ));
        return Ok(Uninstalled::Incomplete);
    }
    for name in [
        "uninstall.tsv",
        "selections.tsv",
        "receipt.json",
        "sources.tsv",
    ] {
        match fs::remove_file(state.join(name)) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    say(&format!(
        "Uninstalled leanck. Installer backups remain in {}. Restart your agent hosts and shell.",
        state.join("backups").display()
    ));
    Ok(Uninstalled::Complete)
}

fn relay_raw(say: &mut impl FnMut(&str), ran: &Ran) {
    for stream in [&ran.captured.stdout, &ran.captured.stderr] {
        for line in String::from_utf8_lossy(stream).lines() {
            say(line);
        }
    }
    if let Err(failure) = &ran.result
        && let Some(line) = failure.line()
    {
        say(&line);
    }
}

/// Remove one record. `Ok(false)` keeps a changed path recorded.
fn remove(
    record: &Record,
    state: &Path,
    hosts: &dyn HostCli,
    say: &mut impl FnMut(&str),
) -> Result<bool> {
    let failed = "removal failed; remaining files and receipts preserved; rerun to resume";
    if let Some(host) = record.host() {
        ensure!(
            hosts.available(host),
            "{} is unavailable; restore it to unregister the plugin, then rerun",
            host.command()
        );
        let args: &[&str] = match (host, record.kind.as_str()) {
            (HostName::Claude, "host") => {
                &["plugin", "uninstall", "leanck@leanck", "--scope", "user"]
            }
            (HostName::Claude, _) => &[
                "plugin",
                "marketplace",
                "remove",
                "leanck",
                "--scope",
                "user",
            ],
            (HostName::Codex, "host") => &["plugin", "remove", "leanck@leanck"],
            (HostName::Codex, _) => &["plugin", "marketplace", "remove", "leanck"],
        };
        let ran = hosts.run(&HostCall::new(host, &record.path, args));
        relay_raw(say, &ran);
        ensure!(ran.success(), failed);
        return Ok(true);
    }
    let path = &record.path;
    if !present(path) {
        return Ok(true);
    }
    if !unchanged(record, state) {
        say(&format!(
            "Preserved changed or unsafe path: {}",
            path.display()
        ));
        return Ok(false);
    }
    let removed = (|| -> Result<()> {
        match record.kind.as_str() {
            "profile" => {
                let text = fs::read(path)?;
                let mode = fs::metadata(path)?.permissions().mode() & 0o7777;
                let parent = path.parent().context("profile has no parent")?;
                let stage = tempfile::Builder::new()
                    .prefix(".leanck-uninstall.")
                    .tempdir_in(parent)?;
                let staged = stage.path().join("profile");
                write_file(&staged, &strip_profile_block(&text), mode)?;
                fs::rename(&staged, path)?;
            }
            "tree" => fs::remove_dir_all(path)?,
            "file" => fs::remove_file(path)?,
            _ => {
                fs::remove_file(path)?;
                // Sidecars belong to this agent; never remove the host's directory.
                for suffix in ["leanck-installed", "leanck-sha256"] {
                    let sidecar = sidecar(path, suffix);
                    if !fs::symlink_metadata(&sidecar).is_ok_and(|m| m.is_dir()) {
                        match fs::remove_file(&sidecar) {
                            Ok(()) => {}
                            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
                            Err(e) => return Err(e.into()),
                        }
                    }
                }
            }
        }
        Ok(())
    })();
    removed.context(failed)?;
    say(&format!("Removed {}: {}", record.kind, path.display()));
    Ok(true)
}

fn save_remaining(state: &Path, remaining: &[Record]) -> Result<()> {
    let text: String = remaining.iter().map(Record::line).collect();
    crate::atomic_bytes(&state.join("uninstall.tsv"), text.as_bytes())?;
    set_mode(&state.join("uninstall.tsv"), 0o600)
}

fn present(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

/// Absolute, without `..`, and single-line.
fn valid_path(path: &Path) -> Result<()> {
    let text = path.to_string_lossy();
    ensure!(
        path.is_absolute(),
        "expected an absolute path: {}",
        path.display()
    );
    ensure!(
        !text.chars().any(char::is_control),
        "control characters in path"
    );
    ensure!(
        path != Path::new("/")
            && !path
                .components()
                .any(|c| matches!(c, std::path::Component::ParentDir)),
        "unsafe path: {}",
        path.display()
    );
    Ok(())
}

fn parse_inventory(text: &str) -> Result<Vec<Record>> {
    let mut records = vec![];
    for line in text.lines().filter(|l| !l.is_empty()) {
        let fields: Vec<&str> = line.split('\t').collect();
        ensure!(fields.len() == 3, "malformed uninstall inventory");
        records.push(Record::new(fields[0], fields[1], fields[2]));
    }
    Ok(records)
}

/// Discover what installers without an inventory installed.
fn legacy_inventory(saved: &SavedState, env: &Environment, plugin: bool) -> Result<Vec<Record>> {
    let value = |key: &str| saved.value(key).map(PathBuf::from);
    let data_dir = value("data_dir").unwrap_or_else(|| env.data_home().join("leanck"));
    let claude_dir = value("claude_dir")
        .or_else(|| env.claude_config_dir.clone())
        .unwrap_or_else(|| env.home.join(".claude"));
    let codex_dir = value("codex_dir")
        .or_else(|| env.codex_home.clone())
        .unwrap_or_else(|| env.home.join(".codex"));
    let claude_skills = value("claude_skills_dir").unwrap_or_else(|| claude_dir.join("skills"));
    let claude_agents = value("claude_agents_dir").unwrap_or_else(|| claude_dir.join("agents"));
    let codex_skills = value("codex_skills").unwrap_or_else(|| env.home.join(".agents/skills"));
    let codex_agents = value("codex_agents_dir").unwrap_or_else(|| codex_dir.join("agents/leanck"));
    let mut records = vec![];
    for (key, location) in saved.entries() {
        let location = Path::new(location);
        match key {
            "extra_skills" => {
                valid_path(location)?;
                for skill in sorted_entries(location) {
                    legacy_tree(&skill, &mut records);
                }
            }
            "extra_claude_agents" | "extra_codex_agents" => {
                valid_path(location)?;
                legacy_agents(location, &data_dir, &mut records);
            }
            _ => {}
        }
    }
    for directory in [
        &data_dir,
        &claude_dir,
        &claude_skills,
        &claude_agents,
        &codex_dir,
        &codex_skills,
        &codex_agents,
    ] {
        valid_path(directory)?;
    }
    for directory in [claude_skills, codex_skills] {
        for skill in sorted_entries(&directory) {
            legacy_tree(&skill, &mut records);
        }
    }
    legacy_agents(&claude_agents, &data_dir, &mut records);
    legacy_agents(&codex_dir.join("agents"), &data_dir, &mut records);
    legacy_agents(&codex_agents, &data_dir, &mut records);
    legacy_tree(&data_dir.join("agents"), &mut records);
    legacy_tree(&data_dir.join("bundle"), &mut records);
    for (path, digest) in [
        ("last_binary_path", "last_binary_digest"),
        ("last_completions_path", "last_completions_digest"),
    ] {
        if let Some(path) = saved.value(path) {
            records.push(Record::new(
                "file",
                saved.get(digest).unwrap_or_default(),
                path,
            ));
        }
    }
    if let Some(profile) = saved.value("path_file") {
        records.push(Record::new("profile", "-", profile));
    }
    ensure!(
        !data_dir.join("bundle").is_dir() || plugin,
        "older installer did not record plugin hosts; rerun with --plugin claude, codex, or both to remove registrations"
    );
    Ok(records)
}

fn sorted_entries(dir: &Path) -> Vec<PathBuf> {
    let mut entries: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| entries.filter_map(|e| Some(e.ok()?.path())).collect())
        .unwrap_or_default();
    use std::os::unix::ffi::OsStrExt;
    entries.sort_by(|a, b| a.as_os_str().as_bytes().cmp(b.as_os_str().as_bytes()));
    entries
}

fn text(path: &Path) -> Option<String> {
    fs::read_to_string(path)
        .ok()
        .map(|t| t.trim_end_matches('\n').to_owned())
}

fn legacy_tree(path: &Path, records: &mut Vec<Record>) {
    let digest = path.join(".leanck-content-sha256");
    if fs::symlink_metadata(path).is_ok_and(|m| !m.file_type().is_symlink())
        && digest.is_file()
        && let Some(expected) = text(&digest)
    {
        records.push(Record::new("tree", expected, path));
    }
}

fn legacy_agents(dir: &Path, data_dir: &Path, records: &mut Vec<Record>) {
    for marker in sorted_entries(dir) {
        let Some(name) = marker
            .to_str()
            .and_then(|m| m.strip_suffix(".leanck-installed"))
        else {
            continue;
        };
        if !fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file()) {
            continue;
        }
        let target = PathBuf::from(name);
        let digest = sidecar(&target, "leanck-sha256");
        if !digest.is_file() {
            continue;
        }
        let file_name = target.file_name().unwrap_or_default();
        let link = data_dir.join("agents").join(file_name);
        if fs::read_link(&target).is_ok_and(|t| t == link) {
            records.push(Record::new("link", link.to_string_lossy(), &target));
        } else {
            records.push(Record::new(
                "agent",
                text(&digest).unwrap_or_default(),
                &target,
            ));
        }
    }
}

/// Whether a recorded path is exactly what the installer left there.
fn unchanged(record: &Record, state: &Path) -> bool {
    let path = &record.path;
    let Some(parent) = path.parent() else {
        return false;
    };
    if safe_dir(parent).is_err() {
        return false;
    }
    let meta = fs::symlink_metadata(path);
    let regular = meta.as_ref().is_ok_and(|m| m.is_file());
    match record.kind.as_str() {
        "tree" => {
            let marker = path.join(".leanck-installed");
            meta.as_ref().is_ok_and(|m| m.is_dir())
                && fs::symlink_metadata(&marker).is_ok_and(|m| m.is_file())
                && text(&marker).as_deref() == Some(OWNERSHIP_MARKER)
                && only_files_and_dirs(path)
                && tree_digest(path).is_ok_and(|digest| digest == record.expected)
        }
        "file" | "agent" => {
            regular
                && (digest_file(path).is_ok_and(|digest| digest == record.expected)
                    || (record.kind == "file" && marker_for(path, state)))
        }
        "link" => fs::read_link(path).is_ok_and(|t| t.to_string_lossy() == record.expected),
        "profile" => regular && fs::read(path).is_ok_and(|text| balanced_markers(&text)),
        _ => true,
    }
}

/// A discovery marker naming this state, in any format an installer or
/// updater has written. Older inventories recorded other marker bytes.
fn marker_for(path: &Path, state: &Path) -> bool {
    path.file_name()
        .is_some_and(|n| n == ".leanck-installation.json")
        && fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
            .is_some_and(|value| {
                value.as_object().is_some_and(|o| o.len() == 1)
                    && value["state_dir"].as_str() == state.to_str()
            })
}

fn only_files_and_dirs(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    entries.into_iter().all(|entry| {
        entry.is_ok_and(|entry| match entry.file_type() {
            Ok(ty) if ty.is_dir() => only_files_and_dirs(&entry.path()),
            Ok(ty) => ty.is_file(),
            Err(_) => false,
        })
    })
}
