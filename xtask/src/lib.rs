//! Repository tasks for leanck, run as `cargo xtask <task>`.
//!
//! [`package_plugin`] assembles the Claude Code and Codex plugin from
//! repository sources. Its output directory is generated: it is replaced as a
//! whole and must not be edited by hand. Sources are:
//!
//! - `packaging/plugin/`: manifests, MCP configurations, and README
//! - `leanck-skills/`: skills (shared by Claude Code and Codex)
//! - `leanck-subagents/<name>/`: `instructions.md`, shared by both hosts, plus
//!   `claude.yaml` (Claude Code frontmatter) and/or `codex.toml` (Codex header)
//! - `leanck-hooks/`: Claude Code hooks
//! - a staged `leanck` executable
//!
//! [`assemble_distribution`] wraps that plugin in the unpacked layout shared by
//! release archives ([`write_release_archive`]) and local installs.

mod agents;
mod archive;
mod json;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use anyhow::{Context, Result, anyhow, bail, ensure};
use serde_json::json;

pub use archive::write_release_archive;

pub const BINARIES: &[&str] = &["leanck"];
pub const MARKER: &str = ".leanck-package.json";
/// Protocol 3: the bundled binary supports `leanck _install` and `leanck _uninstall`,
/// which `install.sh` and `uninstall.sh` run. (Protocol 2 added
/// `leanck _update configure-plugin`.) `install.sh` refuses lower protocols.
pub const UPDATE_PROTOCOL: u32 = 3;
pub const TARGETS: &[&str] = &[
    "aarch64-apple-darwin",
    "x86_64-apple-darwin",
    "aarch64-unknown-linux-musl",
    "x86_64-unknown-linux-musl",
];

/// The repository checkout this task was built from.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives inside the repository")
        .to_path_buf()
}

/// Component names recorded in a package's marker.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Packaged {
    pub output: PathBuf,
    pub skills: Vec<String>,
    pub claude_agents: Vec<String>,
    pub codex_agents: Vec<String>,
    pub dirty: bool,
}

impl Packaged {
    /// The summary printed after packaging.
    pub fn summary(&self) -> String {
        let mut text = format!(
            "packaged {}\n  skills: {}\n  claude agents: {}\n  codex agents: {}\n",
            self.output.display(),
            self.skills.join(", "),
            self.claude_agents.join(", "),
            self.codex_agents.join(", "),
        );
        if self.dirty {
            text.push_str("  note: built from a working tree with uncommitted changes\n");
        }
        text
    }
}

/// Python-style `repr` of a string, for error messages.
pub(crate) fn repr(text: &str) -> String {
    let quote = if text.contains('\'') && !text.contains('"') {
        '"'
    } else {
        '\''
    };
    let mut out = String::from(quote);
    for c in text.chars() {
        match c {
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c == quote => {
                out.push('\\');
                out.push(c);
            }
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\x{:02x}", c as u32))
            }
            c => out.push(c),
        }
    }
    out.push(quote);
    out
}

/// Split lines like Python's `str.splitlines` for `\n`, `\r\n`, and `\r`.
fn split_lines(text: &str) -> Vec<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let mut lines: Vec<String> = normalized.split('\n').map(str::to_owned).collect();
    if lines.last().is_some_and(String::is_empty) {
        lines.pop();
    }
    lines
}

/// Parse top-level `key: value` lines of a YAML header, as the hosts read it.
pub(crate) fn header_keys(path: &Path, lines: &[String]) -> Result<HashMap<String, String>> {
    let mut keys = HashMap::new();
    for line in lines {
        if line.is_empty() || line.starts_with([' ', '\t', '-', '#']) {
            continue;
        }
        let (key, value) = line.split_once(':').unwrap_or((line, ""));
        let bare: String = key.chars().filter(|c| !matches!(c, '-' | '_')).collect();
        if !line.contains(':') || bare.is_empty() || !bare.chars().all(char::is_alphanumeric) {
            bail!(
                "{}: malformed frontmatter line {}",
                path.display(),
                repr(line)
            );
        }
        keys.insert(
            key.to_owned(),
            value.trim().trim_matches(['"', '\'']).to_owned(),
        );
    }
    Ok(keys)
}

/// Return the top-level keys of Markdown text's YAML frontmatter.
pub(crate) fn frontmatter_text(path: &Path, text: &str) -> Result<HashMap<String, String>> {
    let lines = split_lines(text);
    if lines.first().map(String::as_str) != Some("---") {
        bail!("{}: missing YAML frontmatter", path.display());
    }
    let end = lines[1..]
        .iter()
        .position(|line| line == "---")
        .map(|index| index + 1)
        .ok_or_else(|| anyhow!("{}: unterminated YAML frontmatter", path.display()))?;
    let keys = header_keys(path, &lines[1..end])?;
    for required in ["name", "description"] {
        if keys.get(required).is_none_or(String::is_empty) {
            bail!("{}: frontmatter lacks {}", path.display(), repr(required));
        }
    }
    Ok(keys)
}

/// Return the top-level keys of a Markdown file's YAML frontmatter.
pub fn frontmatter(path: &Path) -> Result<HashMap<String, String>> {
    frontmatter_text(path, &read_text(path)?)
}

pub(crate) fn read_text(path: &Path) -> Result<String> {
    fs::read_to_string(path).with_context(|| format!("{}: cannot read", path.display()))
}

fn check_json(path: &Path) -> Result<()> {
    serde_json::from_str::<serde_json::Value>(&read_text(path)?)
        .map_err(|error| anyhow!("{}: invalid JSON: {error}", path.display()))?;
    Ok(())
}

pub(crate) fn write(path: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    fs::write(path, contents).with_context(|| format!("{}: cannot write", path.display()))
}

pub(crate) fn create_dir_all(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("{}: cannot create directory", path.display()))
}

/// Directory entries sorted by name, following links like Python's `is_dir`.
pub(crate) fn sorted_entries(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut entries = fs::read_dir(dir)
        .with_context(|| format!("{}: cannot list directory", dir.display()))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<std::io::Result<Vec<_>>>()
        .with_context(|| format!("{}: cannot list directory", dir.display()))?;
    entries.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
    Ok(entries)
}

pub(crate) fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(str::to_owned)
        .ok_or_else(|| anyhow!("{}: name is not valid UTF-8", path.display()))
}

/// Copy a file with its permissions and modification time, like `shutil.copy2`.
fn copy_file(source: &Path, destination: &Path) -> Result<()> {
    fs::copy(source, destination).with_context(|| {
        format!(
            "cannot copy {} to {}",
            source.display(),
            destination.display()
        )
    })?;
    let modified = fs::metadata(source).and_then(|meta| meta.modified());
    if let Ok(modified) = modified {
        fs::File::open(destination)
            .and_then(|file| file.set_modified(modified))
            .with_context(|| format!("{}: cannot set modification time", destination.display()))?;
    }
    Ok(())
}

/// Merge `source` into `destination`, skipping `.DS_Store` and `__pycache__`.
/// Links are followed, so the copy holds only regular files and directories.
fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    create_dir_all(destination)?;
    for path in sorted_entries(source)? {
        let name = path.file_name().expect("directory entries have names");
        if name == ".DS_Store" || name == "__pycache__" {
            continue;
        }
        let target = destination.join(name);
        if path.is_dir() {
            copy_tree(&path, &target)?;
        } else {
            copy_file(&path, &target)?;
        }
    }
    let meta = fs::metadata(source)
        .with_context(|| format!("{}: cannot read metadata", source.display()))?;
    fs::set_permissions(destination, meta.permissions())
        .with_context(|| format!("{}: cannot set permissions", destination.display()))?;
    if let Ok(modified) = meta.modified() {
        fs::File::open(destination)
            .and_then(|directory| directory.set_modified(modified))
            .with_context(|| format!("{}: cannot set modification time", destination.display()))?;
    }
    Ok(())
}

fn add_skills(repo: &Path, out: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    for skill in sorted_entries(&repo.join("leanck-skills"))? {
        if !skill.is_dir() {
            continue;
        }
        let name = file_name(&skill)?;
        let metadata = skill.join("SKILL.md");
        let keys = frontmatter(&metadata)?;
        if keys["name"] != name {
            bail!(
                "{}: name {} differs from directory",
                metadata.display(),
                repr(&keys["name"])
            );
        }
        copy_tree(&skill, &out.join("skills").join(&name))?;
        names.push(name);
    }
    Ok(names)
}

fn add_hooks(repo: &Path, out: &Path) -> Result<()> {
    let source = repo.join("leanck-hooks");
    check_json(&source.join("hooks.json"))?;
    copy_tree(&source, &out.join("hooks"))
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|meta| meta.is_file() && meta.permissions().mode() & 0o111 != 0)
}

fn add_binaries(bin_dir: &Path, out: &Path, sign: bool) -> Result<()> {
    let bin = out.join("bin");
    fs::create_dir(&bin).with_context(|| format!("{}: cannot create directory", bin.display()))?;
    for name in BINARIES {
        let source = bin_dir.join(name);
        if !is_executable(&source) {
            bail!("{}: staged executable is missing", source.display());
        }
        let target = bin.join(name);
        copy_file(&source, &target)?;
        if sign && cfg!(target_os = "macos") {
            let output = Command::new("codesign")
                .args(["--force", "-s", "-"])
                .arg(&target)
                .output()
                .context("cannot run codesign")?;
            ensure!(
                output.status.success(),
                "{}: codesign failed: {}",
                target.display(),
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
    }
    Ok(())
}

struct Revision {
    commit: Option<String>,
    dirty: bool,
}

fn revision(repo: &Path) -> Revision {
    let git = |args: &[&str]| {
        let output = Command::new("git")
            .arg("-C")
            .arg(repo)
            .args(args)
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    Revision {
        commit: git(&["rev-parse", "HEAD"]),
        dirty: git(&["status", "--porcelain"]).is_some_and(|status| !status.is_empty()),
    }
}

/// Swap the assembled package into place, refusing to delete foreign data.
fn replace(staged: &Path, output: &Path) -> Result<()> {
    let rename = |from: &Path, to: &Path| {
        fs::rename(from, to)
            .with_context(|| format!("cannot rename {} to {}", from.display(), to.display()))
    };
    if output.symlink_metadata().is_ok() {
        if output.is_symlink() || !output.join(MARKER).is_file() {
            bail!(
                "{} exists and is not a generated package; remove it manually",
                output.display()
            );
        }
        let retired = output.with_file_name(format!(".{}.old", file_name(output)?));
        let _ = fs::remove_dir_all(&retired);
        rename(output, &retired)?;
        rename(staged, output)?;
        fs::remove_dir_all(&retired)
            .with_context(|| format!("{}: cannot remove", retired.display()))?;
    } else {
        rename(staged, output)?;
    }
    Ok(())
}

/// Assemble the plugin into `output`, replacing a previously generated package.
/// Without `bin_dir`, the package holds only skills, agents, and hooks.
pub fn package_plugin(
    repo: &Path,
    bin_dir: Option<&Path>,
    output: &Path,
    sign_binaries: bool,
) -> Result<Packaged> {
    let overlay = repo.join("packaging").join("plugin");
    check_overlay_json(&overlay)?;
    let name = file_name(output)?;
    let parent = output.parent().context("output must name a directory")?;
    create_dir_all(parent)?;
    let staged = tempfile::Builder::new()
        .prefix(&format!(".{name}."))
        .tempdir_in(parent)
        .with_context(|| format!("{}: cannot create a staging directory", parent.display()))?;
    copy_tree(&overlay, staged.path())?;
    let skills = add_skills(repo, staged.path())?;
    let (claude_agents, codex_agents) = agents::add_agents(repo, staged.path())?;
    add_hooks(repo, staged.path())?;
    if let Some(bin_dir) = bin_dir {
        add_binaries(bin_dir, staged.path(), sign_binaries)?;
    }
    let source = revision(repo);
    let marker = json!({
        "generated_by": "just package-plugin",
        "source": {"commit": source.commit, "dirty": source.dirty},
        "skills": skills,
        "claude_agents": claude_agents,
        "codex_agents": codex_agents,
        "binaries": if bin_dir.is_some() { BINARIES.to_vec() } else { Vec::new() },
    });
    json::write(&staged.path().join(MARKER), &marker)?;
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(staged.path(), fs::Permissions::from_mode(0o755))?;
    }
    let staged = staged.keep();
    if let Err(error) = replace(&staged, output) {
        let _ = fs::remove_dir_all(&staged);
        return Err(error);
    }
    Ok(Packaged {
        output: output.to_path_buf(),
        skills,
        claude_agents,
        codex_agents,
        dirty: source.dirty,
    })
}

fn check_overlay_json(dir: &Path) -> Result<()> {
    for path in sorted_entries(dir)? {
        if path.is_dir() {
            check_overlay_json(&path)?;
        } else if path
            .extension()
            .is_some_and(|extension| extension == "json")
        {
            check_json(&path)?;
        }
    }
    Ok(())
}

/// Validate a semantic version, optionally prefixed with `v`, and drop the prefix.
pub fn release_version(version: &str) -> Result<&str> {
    let version = version.strip_prefix('v').unwrap_or(version);
    let (core, suffix) = match version.find(['-', '+']) {
        Some(index) => (&version[..index], Some(&version[index + 1..])),
        None => (version, None),
    };
    let numeric = |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit());
    let parts: Vec<&str> = core.split('.').collect();
    let valid = parts.len() == 3
        && parts.iter().all(|part| numeric(part))
        && suffix.is_none_or(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-'))
        });
    ensure!(
        valid,
        "version must be a semantic version, optionally prefixed with v"
    );
    Ok(version)
}

/// Assemble an unpacked distribution shared by releases and local installs.
pub fn assemble_distribution(
    repo: &Path,
    bin_dir: Option<&Path>,
    root: &Path,
    version: &str,
    sign_binaries: bool,
) -> Result<Packaged> {
    let version = release_version(version)?;
    if let Some(bin_dir) = bin_dir {
        let binary = bin_dir.join("leanck");
        let output = Command::new(&binary)
            .arg("--version")
            .output()
            .with_context(|| format!("cannot run {}", binary.display()))?;
        ensure!(
            output.status.success(),
            "{} --version failed: {}",
            binary.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        );
        let reported = String::from_utf8_lossy(&output.stdout);
        let reported = reported.trim();
        ensure!(
            reported == format!("leanck {version}"),
            "release v{version} does not match binary: {}",
            repr(reported)
        );
    }
    fs::create_dir(root).with_context(|| format!("{}: cannot create directory", root.display()))?;
    let packaged = package_plugin(repo, bin_dir, &root.join("plugin"), sign_binaries)?;
    // Host plugin caches use this version when refreshing their installed copy.
    for host in ["claude", "codex"] {
        let manifest = root
            .join("plugin")
            .join(format!(".{host}-plugin"))
            .join("plugin.json");
        let mut data: serde_json::Value = serde_json::from_str(&read_text(&manifest)?)
            .with_context(|| format!("{}: invalid JSON", manifest.display()))?;
        data.as_object_mut()
            .with_context(|| format!("{}: manifest is not an object", manifest.display()))?
            .insert("version".into(), version.into());
        json::write(&manifest, &data)?;
    }
    let catalogs = [
        (
            ".claude-plugin/marketplace.json",
            json!({
                "name": "leanck",
                "owner": {"name": "Dan Lyng"},
                "plugins": [{"name": "leanck", "source": "./plugin"}],
            }),
        ),
        (
            ".agents/plugins/marketplace.json",
            json!({
                "name": "leanck",
                "plugins": [{
                    "name": "leanck",
                    "source": {"source": "local", "path": "./plugin"},
                    "policy": {"installation": "AVAILABLE", "authentication": "ON_INSTALL"},
                    "category": "Engineering",
                }],
            }),
        ),
    ];
    for (name, catalog) in catalogs {
        let path = root.join(name);
        create_dir_all(path.parent().expect("catalogs live in directories"))?;
        json::write(&path, &catalog)?;
    }
    write(&root.join(".leanck-release"), format!("v{version}\n"))?;
    write(
        &root.join(".leanck-update-protocol"),
        format!("{UPDATE_PROTOCOL}\n"),
    )?;
    copy_file(&repo.join("install.sh"), &root.join("install.sh"))?;
    copy_file(&repo.join("uninstall.sh"), &root.join("uninstall.sh"))?;
    create_dir_all(&root.join("docs"))?;
    copy_file(
        &repo.join("docs").join("leanck.example.toml"),
        &root.join("docs").join("leanck.example.toml"),
    )?;
    Ok(packaged)
}

/// Package a staged binary plus integrations as `leanck-<target>.tar.gz` and
/// its `.sha256` sidecar in `output`, returning the archive path.
pub fn package_release(
    repo: &Path,
    bin_dir: &Path,
    output: &Path,
    target: &str,
    version: &str,
) -> Result<(Packaged, PathBuf)> {
    ensure!(
        TARGETS.contains(&target),
        "unsupported target {}",
        repr(target)
    );
    let temp = tempfile::Builder::new()
        .prefix("leanck-release-")
        .tempdir()
        .context("cannot create a temporary directory")?;
    let root = temp.path().join("leanck");
    let packaged = assemble_distribution(repo, Some(bin_dir), &root, version, true)?;
    let archive = write_release_archive(&root, output, target)?;
    Ok((packaged, archive))
}

/// Assemble local source for `install.sh --from-repo` after the user approves
/// building it. With `inventory`, confirm that component names still match an
/// approved inventory.
pub fn package_local(
    repo: &Path,
    bin_dir: Option<&Path>,
    output: &Path,
    version: &str,
    inventory: Option<&Path>,
) -> Result<Packaged> {
    let packaged = assemble_distribution(repo, bin_dir, output, version, false)?;
    let Some(inventory) = inventory else {
        return Ok(packaged);
    };
    let names = |dir: &str, suffix: Option<&str>| -> Result<Vec<String>> {
        let mut names = Vec::new();
        for path in sorted_entries(&inventory.join(dir))? {
            let name = file_name(&path)?;
            match suffix {
                None => names.push(name),
                Some(suffix) => {
                    if let Some(stem) = name.strip_suffix(suffix).filter(|stem| !stem.is_empty()) {
                        names.push(stem.to_owned());
                    }
                }
            }
        }
        names.sort();
        Ok(names)
    };
    let expected = [
        (&packaged.skills, names("plugin/skills", None)?),
        (
            &packaged.claude_agents,
            names("plugin/agents", Some(".md"))?,
        ),
        (
            &packaged.codex_agents,
            names("plugin/codex/agents", Some(".toml"))?,
        ),
    ];
    for (actual, expected) in expected {
        ensure!(
            *actual == expected,
            "local component names changed after configuration; rerun the installer"
        );
    }
    Ok(packaged)
}

#[cfg(test)]
mod tests;
