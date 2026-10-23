use crate::{
    ReleaseSource,
    gate::Maintenance,
    receipt::{Component, Receipt, UNMANAGED_EXECUTABLE, digest_file, fingerprint, tree_digest},
    safe_dir, safe_path,
    transaction::Transaction,
};
use anyhow::{Context, Result, bail, ensure};
use semver::Version;
use serde::Serialize;
use std::{
    collections::BTreeSet,
    fs::{self, File},
    io::Read,
    path::{Path, PathBuf},
    time::Duration,
};

const MAX_ARCHIVE: u64 = 256 * 1024 * 1024;
const MAX_EXPANDED: u64 = 1024 * 1024 * 1024;

pub fn recover(state: &Path) -> Result<()> {
    if !state.join("update-journal.json").try_exists()? {
        return Ok(());
    }
    let _gate = Maintenance::acquire(state)?;
    Transaction::recover(state)
}

#[derive(Debug, Serialize)]
pub struct Plan {
    pub from_version: String,
    pub to_version: String,
    pub changes: Vec<PathBuf>,
    pub downgrade: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub recording_installer_ownership: Option<PathBuf>,
}
pub struct Prepared {
    _download: tempfile::TempDir,
    root: PathBuf,
    receipt: Receipt,
    components: Vec<Component>,
    pub plan: Plan,
    adoption: Option<InstallerAdoption>,
}

/// Release resolution can finish without downloading an installation payload.
pub enum Preparation {
    UpToDate(Outcome),
    Update(Box<Prepared>),
}

struct InstallerAdoption {
    receipt: Receipt,
    selections: Vec<u8>,
    sources: Vec<u8>,
}

impl InstallerAdoption {
    fn read(state: &Path, binary: &Path) -> Result<Self> {
        let read = |name: &str| -> Result<Vec<u8>> {
            let path = state.join(name);
            safe_path(&path)?;
            Ok(fs::read(path)?)
        };
        let selections = read("selections.tsv").context(UNMANAGED_EXECUTABLE)?;
        let sources = read("sources.tsv").context(UNMANAGED_EXECUTABLE)?;
        let receipt = Receipt::from_recorded_installer(state, binary)?;
        let adoption = Self {
            receipt,
            selections,
            sources,
        };
        adoption.validate()?;
        Ok(adoption)
    }

    fn validate(&self) -> Result<()> {
        let state = &self.receipt.state_dir;
        safe_path(&state.join("selections.tsv"))?;
        safe_path(&state.join("sources.tsv"))?;
        ensure!(
            fs::read(state.join("selections.tsv"))? == self.selections
                && fs::read(state.join("sources.tsv"))? == self.sources,
            "installer state changed during download; rerun update"
        );
        Receipt::from_recorded_installer(state, &self.receipt.binary)?;
        Ok(())
    }
}
#[derive(Debug, Serialize)]
pub struct Outcome {
    pub status: String,
    pub version: String,
    pub registration_errors: Vec<String>,
    pub restart_required: bool,
}

pub fn target() -> Result<String> {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "aarch64",
        "x86_64" => "x86_64",
        _ => bail!("unsupported update architecture"),
    };
    let os = match std::env::consts::OS {
        "macos" => "apple-darwin",
        "linux" => "unknown-linux-musl",
        _ => bail!("updates support macOS and Linux"),
    };
    Ok(format!("{arch}-{os}"))
}

pub async fn latest(source: &ReleaseSource, current: &str) -> Result<String> {
    source.validate()?;
    let updater = self_update::backends::github::Update::configure()
        .repo_owner("danlyng")
        .repo_name("leanck")
        .bin_name("leanck")
        .api_base_url(source.api.trim_end_matches('/'))
        .current_version(current)
        .timeout(Duration::from_secs(10))
        .no_confirm(true)
        .show_output(false)
        .build_async()?;
    let releases =
        tokio::time::timeout(Duration::from_secs(10), updater.get_latest_release_async()).await??;
    let version = Version::parse(
        releases
            .latest()
            .context("no published stable release")?
            .version(),
    )?;
    ensure!(
        version.pre.is_empty(),
        "latest published release is a prerelease"
    );
    Ok(version.to_string())
}

async fn download(url: &str, path: &Path, cap: u64) -> Result<()> {
    let mut download = self_update::Download::from_url(url);
    download
        .timeout(Duration::from_secs(120))
        .max_download_size(cap);
    let file = File::create(path)?;
    tokio::time::timeout(Duration::from_secs(120), download.download_to_async(file)).await??;
    Ok(())
}

fn extract(archive: &Path, destination: &Path) -> Result<()> {
    let reader = flate2::read::GzDecoder::new(File::open(archive)?).take(MAX_EXPANDED + 1);
    let mut archive = tar::Archive::new(reader);
    let mut total = 0_u64;
    let mut names = BTreeSet::new();
    for entry in archive.entries()? {
        let mut entry = entry?;
        let path = entry.path()?.into_owned();
        ensure!(
            path.to_str()
                .is_some_and(|s| !s.chars().any(char::is_control)),
            "archive path contains unsupported characters"
        );
        ensure!(names.len() < 100_000, "too many archive entries");
        ensure!(names.insert(path.clone()), "duplicate archive member");
        ensure!(
            !path.is_absolute()
                && !path
                    .components()
                    .any(|p| matches!(p, std::path::Component::ParentDir)),
            "unsafe archive path"
        );
        let path = path
            .strip_prefix("leanck")
            .context("archive must have a single leanck/ root")?;
        let kind = entry.header().entry_type();
        ensure!(
            kind.is_file() || kind.is_dir(),
            "archive contains a link or special file"
        );
        total = total
            .checked_add(entry.size())
            .context("archive size overflow")?;
        ensure!(total <= MAX_EXPANDED, "expanded archive exceeds 1 GiB");
        let out = destination.join(path);
        if kind.is_dir() {
            fs::create_dir_all(&out)?;
        } else {
            fs::create_dir_all(out.parent().context("archive entry missing parent")?)?;
            let mut file = File::create(&out)?;
            std::io::copy(&mut entry, &mut file)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                file.set_permissions(fs::Permissions::from_mode(entry.header().mode()? & 0o777))?;
            }
        }
    }
    Ok(())
}

/// Configure an unpacked `plugin/` directory in place for an installation.
///
/// Points the Codex MCP definition at `binary`. When `mcp_enabled` is false,
/// renames both MCP definitions to `*.disabled.json` and drops `mcpServers`
/// from the Codex manifest. The installer and updater share this, so their
/// configured trees, and thus fingerprints, match byte for byte.
pub fn configure_plugin(plugin: &Path, binary: &Path, mcp_enabled: bool) -> Result<()> {
    let config = plugin.join(".mcp.codex.json");
    let mut value = read_json(&config)?;
    value["mcpServers"]["leanck"]["command"] = binary.to_string_lossy().into_owned().into();
    write_json(&config, &value)?;
    if !mcp_enabled {
        for name in [".mcp.json", ".mcp.codex.json"] {
            fs::rename(
                plugin.join(name),
                plugin.join(name.replace(".json", ".disabled.json")),
            )?;
        }
        let manifest = plugin.join(".codex-plugin/plugin.json");
        let mut value = read_json(&manifest)?;
        value
            .as_object_mut()
            .context("invalid Codex plugin manifest")?
            .shift_remove("mcpServers");
        write_json(&manifest, &value)?;
    }
    Ok(())
}

fn read_json(path: &Path) -> Result<serde_json::Value> {
    serde_json::from_slice(&fs::read(path).with_context(|| format!("reading {}", path.display()))?)
        .with_context(|| format!("parsing {}", path.display()))
}

// Two-space indentation, original key order, and a trailing newline.
fn write_json(path: &Path, value: &serde_json::Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    fs::write(path, bytes).with_context(|| format!("writing {}", path.display()))
}

/// Skip an unchanged version unless forced or explicitly requested.
pub async fn prepare(
    receipt: Receipt,
    requested: Option<&str>,
    force: bool,
) -> Result<Preparation> {
    receipt.validate()?;
    ensure!(
        receipt.provenance == "release",
        "this installation was built from local source.\n\
         `leanck update` only supports published releases.\n\n\
         To rebuild and reinstall, run from your leanck checkout:\n  sh ./install.sh --from-repo ."
    );
    ensure!(
        receipt.target == target()?,
        "installation target differs from this machine"
    );
    let selection = requested.or(receipt.pin.as_deref());
    let version = match selection {
        Some("latest") | None => latest(&receipt.source, &receipt.version).await?,
        Some(version) => Version::parse(version.trim_start_matches('v'))?.to_string(),
    };
    ensure!(
        Version::parse(&version)? >= Version::parse(&receipt.version)?
            || requested.is_some_and(|s| s != "latest"),
        "the published release is older than this installation; downgrades require an explicit --to-version TAG"
    );
    if !force
        && requested.is_none()
        && Version::parse(&version)? == Version::parse(&receipt.version)?
    {
        return Ok(Preparation::UpToDate(Outcome {
            status: "up_to_date".into(),
            version,
            registration_errors: vec![],
            restart_required: false,
        }));
    }
    let download_dir = tempfile::tempdir()?;
    let name = format!("leanck-{}.tar.gz", receipt.target);
    let url = format!(
        "{}/download/v{version}/{name}",
        receipt.source.releases.trim_end_matches('/')
    );
    let archive = download_dir.path().join(&name);
    let sums_path = download_dir.path().join("checksum");
    download(&format!("{url}.sha256"), &sums_path, 16 * 1024).await?;
    let checksum = self_update::Checksum::from_sums_file(&fs::read_to_string(sums_path)?, &name)?;
    let expected = match checksum {
        self_update::Checksum::Sha256(digest) => digest,
        _ => bail!("expected a SHA-256 sidecar"),
    };
    download(&url, &archive, MAX_ARCHIVE).await?;
    ensure!(
        crate::checksum_matches(&expected, &digest_file(&archive)?),
        "SHA-256 checksum mismatch; nothing installed"
    );
    let root = download_dir.path().join("extracted");
    fs::create_dir(&root)?;
    extract(&archive, &root)?;
    ensure!(
        fs::read_to_string(root.join(".leanck-release"))?.trim() == format!("v{version}"),
        "release marker differs from requested version"
    );
    for host in ["claude", "codex"] {
        let manifest: serde_json::Value = serde_json::from_slice(&fs::read(
            root.join(format!("plugin/.{host}-plugin/plugin.json")),
        )?)?;
        ensure!(
            manifest["version"].as_str() == Some(version.as_str()),
            "plugin version differs from requested version"
        );
    }
    let executable = root.join("plugin/bin/leanck");
    let output = command(&executable, &["--version".into()], None).await?;
    ensure!(
        output.trim() == format!("leanck {version}"),
        "staged executable version mismatch"
    );
    #[cfg(target_os = "macos")]
    command(
        Path::new("/usr/bin/codesign"),
        &["--verify".into(), executable.to_string_lossy().into_owned()],
        None,
    )
    .await?;
    // Match the installer transformations before computing managed tree fingerprints.
    // Older installations have active definitions; preserve their opt-in too.
    let mcp_enabled = !receipt
        .bundle
        .as_ref()
        .is_some_and(|bundle| bundle.join("plugin/.mcp.codex.disabled.json").is_file());
    configure_plugin(&root.join("plugin"), &receipt.binary, mcp_enabled)?;
    let mut components = receipt.components.clone();
    // Discover new members of previously selected skill/agent collections.
    let mut added = vec![];
    for component in &components {
        if component.kind == "link" {
            let target = Path::new(&component.source);
            if let Some(tree) = components
                .iter()
                .find(|c| c.kind == "tree" && target.parent() == Some(c.destination.as_path()))
            {
                for entry in fs::read_dir(root.join(&tree.source))? {
                    let entry = entry?;
                    if entry.path().extension() != target.extension() {
                        continue;
                    }
                    let destination = component
                        .destination
                        .parent()
                        .unwrap()
                        .join(entry.file_name());
                    if !components
                        .iter()
                        .chain(added.iter())
                        .any(|c: &Component| c.destination == destination)
                    {
                        ensure!(
                            fs::symlink_metadata(&destination).is_err(),
                            "new agent link conflicts with an existing path"
                        );
                        added.push(Component {
                            kind: "link".into(),
                            destination,
                            source: tree
                                .destination
                                .join(entry.file_name())
                                .to_string_lossy()
                                .into_owned(),
                            fingerprint: String::new(),
                        });
                    }
                }
            }
            continue;
        }
        let source = Path::new(&component.source);
        let Some(parent) = source.parent() else {
            continue;
        };
        let selected = (component.kind == "tree" && parent == Path::new("plugin/skills"))
            || (component.kind == "agent"
                && matches!(
                    parent.to_str(),
                    Some("plugin/agents" | "plugin/codex/agents")
                ));
        if !selected {
            continue;
        }
        for entry in fs::read_dir(root.join(parent))? {
            let entry = entry?;
            if component.kind == "tree" && !entry.file_type()?.is_dir() {
                continue;
            }
            if parent == Path::new("plugin/skills")
                && receipt.selected_skills.as_ref().is_some_and(|names| {
                    !names.iter().any(|name| entry.file_name() == name.as_str())
                })
            {
                continue;
            }
            if component.kind == "agent" && entry.path().extension() != source.extension() {
                continue;
            }
            let relative = parent
                .join(entry.file_name())
                .to_string_lossy()
                .into_owned();
            let destination = component
                .destination
                .parent()
                .unwrap()
                .join(entry.file_name());
            if !components
                .iter()
                .chain(added.iter())
                .any(|c: &Component| c.destination == destination)
            {
                ensure!(
                    !destination.try_exists()?,
                    "new component conflicts with an existing path: {}",
                    destination.display()
                );
                added.push(Component {
                    source: relative,
                    destination,
                    kind: component.kind.clone(),
                    fingerprint: String::new(),
                });
            }
        }
    }
    components.extend(added);
    let changes = components.iter().map(|c| c.destination.clone()).collect();
    let plan = Plan {
        from_version: receipt.version.clone(),
        to_version: version.clone(),
        changes,
        downgrade: Version::parse(&version)? < Version::parse(&receipt.version)?,
        recording_installer_ownership: None,
    };
    let mut receipt = receipt;
    if let Some(requested) = requested {
        receipt.pin = (requested != "latest").then(|| version.clone());
    }
    Ok(Preparation::Update(Box::new(Prepared {
        _download: download_dir,
        root,
        receipt,
        components,
        plan,
        adoption: None,
    })))
}

/// Prepare an update without writing ownership until the approved transaction is applied.
pub async fn prepare_from_installer(
    state: &Path,
    binary: &Path,
    requested: Option<&str>,
    force: bool,
) -> Result<Preparation> {
    let adoption = InstallerAdoption::read(state, binary)?;
    let mut preparation = prepare(adoption.receipt.clone(), requested, force).await?;
    if let Preparation::Update(prepared) = &mut preparation {
        prepared.plan.recording_installer_ownership = Some(binary.to_owned());
        prepared.adoption = Some(adoption);
    }
    Ok(preparation)
}

fn copy_tree(source: &Path, destination: &Path) -> Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let target = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&entry.path(), &target)?;
        } else {
            ensure!(
                entry.file_type()?.is_file(),
                "unexpected link in staging tree"
            );
            fs::copy(entry.path(), target)?;
        }
    }
    Ok(())
}
fn sync_tree(root: &Path) -> Result<()> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            sync_tree(&entry.path())?;
        } else if entry.file_type()?.is_file() {
            crate::durability::sync_path(&entry.path())?;
        }
    }
    crate::durability::sync_path(root)?;
    Ok(())
}

impl Prepared {
    pub fn preserve_pin(mut self, pin: Option<String>) -> Self {
        self.receipt.pin = pin;
        self
    }
    pub async fn apply(mut self) -> Result<Outcome> {
        let _maintenance = Maintenance::acquire(&self.receipt.state_dir)?;
        crate::gate::ensure_no_legacy_users(&self.receipt.binary)?;
        if let Some(bundle) = &self.receipt.bundle {
            crate::gate::ensure_no_legacy_users(&bundle.join("plugin/bin/leanck"))?;
        }
        Transaction::recover(&self.receipt.state_dir)?;
        let current = if let Some(adoption) = &self.adoption {
            ensure!(
                !self.receipt.path().try_exists()?,
                "installation ownership changed during download; rerun update"
            );
            adoption.validate()?;
            adoption.receipt.clone()
        } else {
            Receipt::discover(&self.receipt.binary, Some(&self.receipt.state_dir))?
                .context("installation receipt disappeared")?
        };
        ensure!(
            current.generation == self.receipt.generation,
            "installation changed during download; rerun update"
        );
        for component in &self.components {
            if component.fingerprint.is_empty() {
                ensure!(
                    !component.destination.try_exists()?,
                    "new component destination appeared during update"
                );
            } else {
                ensure!(
                    fingerprint(component)? == component.fingerprint,
                    "managed component has local edits: {}; restore it or rerun the installer with reviewed replacement",
                    component.destination.display()
                );
                if matches!(component.kind.as_str(), "agent" | "link") {
                    for (suffix, expected) in [
                        ("leanck-installed", "leanck installer v1".to_owned()),
                        (
                            "leanck-sha256",
                            if component.kind == "link" {
                                digest_file(&component.destination)?
                            } else {
                                component.fingerprint.clone()
                            },
                        ),
                    ] {
                        let path = PathBuf::from(format!(
                            "{}.{}",
                            component.destination.display(),
                            suffix
                        ));
                        safe_path(&path)?;
                        if path.try_exists()? {
                            ensure!(
                                fs::read_to_string(&path)?.trim() == expected,
                                "managed agent sidecar has local edits: {}",
                                path.display()
                            );
                        }
                    }
                }
            }
        }
        let mut stages = vec![];
        let mut replacements = vec![];
        let mut next = vec![];
        let original_components = self.components.clone();
        for mut component in self.components {
            safe_dir(
                component
                    .destination
                    .parent()
                    .context("missing component parent")?,
            )?;
            fs::create_dir_all(component.destination.parent().unwrap())?;
            let stage = tempfile::Builder::new()
                .prefix(".leanck-update-")
                .tempdir_in(component.destination.parent().unwrap())?;
            let out = stage.path().join("replacement");
            let source = if component.kind == "link" {
                let target = Path::new(&component.source);
                let tree = original_components
                    .iter()
                    .find(|c| c.kind == "tree" && target.parent() == Some(c.destination.as_path()))
                    .context("agent link source is not owned by a managed tree")?;
                self.root
                    .join(&tree.source)
                    .join(target.file_name().context("agent link missing filename")?)
            } else {
                self.root.join(&component.source)
            };
            if component.kind != "completion" && !source.try_exists()? {
                if matches!(component.kind.as_str(), "agent" | "link") {
                    for suffix in ["leanck-installed", "leanck-sha256"] {
                        replacements.push((
                            None,
                            PathBuf::from(format!(
                                "{}.{}",
                                component.destination.display(),
                                suffix
                            )),
                        ));
                    }
                }
                replacements.push((None, component.destination));
                stages.push(stage);
                continue;
            }
            match component.kind.as_str() {
                "tree" => {
                    copy_tree(&source, &out)?;
                    fs::write(out.join(".leanck-installed"), "leanck installer v1\n")?;
                    component.fingerprint = tree_digest(&out)?;
                    fs::write(
                        out.join(".leanck-content-sha256"),
                        format!("{}\n", component.fingerprint),
                    )?;
                }
                "link" => {
                    #[cfg(unix)]
                    std::os::unix::fs::symlink(&component.source, &out)?;
                    component.fingerprint = component.source.clone();
                }
                "completion" => {
                    let bytes = command(
                        &self.root.join("plugin/bin/leanck"),
                        &["_completions".into(), component.source.clone()],
                        None,
                    )
                    .await?;
                    fs::write(&out, bytes)?;
                    component.fingerprint = digest_file(&out)?;
                }
                _ => {
                    fs::copy(&source, &out)?;
                    component.fingerprint = digest_file(&out)?;
                }
            }
            replacements.push((Some(out), component.destination.clone()));
            if matches!(component.kind.as_str(), "agent" | "link") {
                for (suffix, content) in [
                    ("leanck-installed", "leanck installer v1".to_owned()),
                    (
                        "leanck-sha256",
                        if component.kind == "link" {
                            digest_file(&source)?
                        } else {
                            component.fingerprint.clone()
                        },
                    ),
                ] {
                    let sidecar =
                        PathBuf::from(format!("{}.{}", component.destination.display(), suffix));
                    safe_path(&sidecar)?;
                    let path = stage.path().join(suffix);
                    fs::write(&path, format!("{content}\n"))?;
                    replacements.push((Some(path), sidecar));
                }
            }
            stages.push(stage);
            next.push(component);
        }
        self.receipt.components = next;
        self.receipt.version = self.plan.to_version.clone();
        self.receipt.generation = self
            .receipt
            .generation
            .checked_add(1)
            .context("installation generation overflow")?;
        self.receipt.registration_pending = !self.receipt.hosts.is_empty();
        let metadata = tempfile::tempdir_in(&self.receipt.state_dir)?;
        let receipt_path = metadata.path().join("receipt.json");
        fs::write(&receipt_path, serde_json::to_vec_pretty(&self.receipt)?)?;
        // Keep the cumulative shell uninstall inventory, replacing only updated records.
        let inventory_path = self.receipt.state_dir.join("uninstall.tsv");
        let inventory = match fs::read_to_string(&inventory_path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let destinations: BTreeSet<_> = current
            .components
            .iter()
            .map(|c| c.destination.to_string_lossy().into_owned())
            .collect();
        let mut inventory = inventory
            .lines()
            .filter(|l| {
                l.split('\t')
                    .nth(2)
                    .is_none_or(|p| !destinations.contains(p))
            })
            .map(|l| format!("{l}\n"))
            .collect::<String>();
        for c in &self.receipt.components {
            inventory.push_str(&format!(
                "{}\t{}\t{}\n",
                if c.kind == "completion" {
                    "file"
                } else {
                    &c.kind
                },
                c.fingerprint,
                c.destination.display()
            ));
        }
        let staged_inventory = metadata.path().join("inventory");
        fs::write(&staged_inventory, inventory)?;
        replacements.push((Some(staged_inventory), inventory_path));
        // Keep bootstrap installer selections and adoption sources current too.
        // Otherwise a later installer run would mistake our managed executable
        // for a local edit and adoption would resurrect stale fingerprints.
        let selections_path = self.receipt.state_dir.join("selections.tsv");
        let selections = match fs::read_to_string(&selections_path) {
            Ok(text) => text,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
            Err(e) => return Err(e.into()),
        };
        let mut values = std::collections::BTreeMap::from([
            (
                "version",
                self.receipt.pin.clone().unwrap_or_else(|| "latest".into()),
            ),
            ("installed_version", format!("v{}", self.receipt.version)),
            ("installed_target", self.receipt.target.clone()),
            ("mode", self.receipt.mode.clone()),
            ("release_url", self.receipt.source.releases.clone()),
            ("release_api", self.receipt.source.api.clone()),
            (
                "last_binary_path",
                self.receipt.binary.to_string_lossy().into_owned(),
            ),
        ]);
        if let Some(binary) = self
            .receipt
            .components
            .iter()
            .find(|c| c.destination == self.receipt.binary)
        {
            values.insert("last_binary_digest", binary.fingerprint.clone());
        }
        if let Some(completion) = self
            .receipt
            .components
            .iter()
            .find(|c| c.kind == "completion")
        {
            values.insert(
                "last_completions_path",
                completion.destination.to_string_lossy().into_owned(),
            );
            values.insert("last_completions_digest", completion.fingerprint.clone());
        }
        let mut selections = selections
            .lines()
            .map(|line| {
                if let Some((key, _)) = line.split_once('\t')
                    && let Some(value) = values.remove(key)
                {
                    format!("{key}\t{value}\n")
                } else {
                    format!("{line}\n")
                }
            })
            .collect::<String>();
        for (key, value) in values {
            selections.push_str(&format!("{key}\t{value}\n"));
        }
        let staged_selections = metadata.path().join("selections");
        fs::write(&staged_selections, selections)?;
        replacements.push((Some(staged_selections), selections_path));
        let sources = self
            .receipt
            .components
            .iter()
            .map(|c| {
                format!(
                    "{}\t{}\t{}\t{}\n",
                    c.kind,
                    c.fingerprint,
                    c.destination.display(),
                    c.source
                )
            })
            .collect::<String>();
        let staged_sources = metadata.path().join("sources");
        fs::write(&staged_sources, sources)?;
        replacements.push((
            Some(staged_sources),
            self.receipt.state_dir.join("sources.tsv"),
        ));
        replacements.push((Some(receipt_path), self.receipt.path()));
        for stage in &stages {
            sync_tree(stage.path())?;
        }
        sync_tree(metadata.path())?;
        let mut tx = Transaction::new(&self.receipt.state_dir)?;
        for (source, destination) in replacements {
            if let Err(error) = tx.replace(source, destination) {
                tx.rollback().context(
                    "update failed and rollback needs repair; retain update-journal.json",
                )?;
                return Err(error.context("update rolled back"));
            }
        }
        tx.commit()?;
        self.receipt.write()?;
        // Registration hooks can start MCP/CLI children. Keep a shared usage
        // lease while refreshing hosts, so those children can enter and other
        // installers still cannot replace files beneath the refresh.
        drop(_maintenance);
        let _lease = crate::gate::Lease::acquire(&self.receipt)?;
        ensure!(
            Receipt::discover(&self.receipt.binary, Some(&self.receipt.state_dir))?
                .is_some_and(|r| r.generation == self.receipt.generation),
            "installation changed before host registration; run `leanck update --retry-registration --yes`"
        );
        register(&mut self.receipt).await
    }
}

/// Direct subprocesses, bounded in duration and captured output; no shell evaluation.
async fn command(exe: &Path, args: &[String], env: Option<(&str, &Path)>) -> Result<String> {
    let stdout = tempfile::tempfile()?;
    let stderr = tempfile::tempfile()?;
    let mut command = tokio::process::Command::new(exe);
    command
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(stdout.try_clone()?)
        .stderr(stderr.try_clone()?)
        .kill_on_drop(true);
    if let Some((key, value)) = env {
        command.env(key, value);
    }
    let status = tokio::time::timeout(Duration::from_secs(30), command.status())
        .await
        .context("update subprocess exceeded 30 seconds")??;
    use std::io::{Seek, SeekFrom};
    let read = |mut file: File| -> Result<String> {
        file.seek(SeekFrom::Start(0))?;
        let mut bytes = vec![];
        file.take(1024 * 1024).read_to_end(&mut bytes)?;
        Ok(String::from_utf8_lossy(&bytes).into_owned())
    };
    let out = read(stdout)?;
    ensure!(
        status.success(),
        "{} failed: {}",
        exe.display(),
        read(stderr)?
    );
    Ok(out)
}

pub async fn register(receipt: &mut Receipt) -> Result<Outcome> {
    let registration = crate::lock_file(&receipt.state_dir.join("registration.lock"))?;
    registration
        .try_lock()
        .context("host registration is already in progress")?;
    let mut errors = vec![];
    if let Some(bundle) = &receipt.bundle {
        for host in &receipt.hosts {
            let key = match host.name.as_str() {
                "claude" => "CLAUDE_CONFIG_DIR",
                "codex" => "CODEX_HOME",
                _ => bail!("invalid receipt host"),
            };
            let calls = [
                vec![
                    "plugin".into(),
                    "marketplace".into(),
                    "add".into(),
                    bundle.to_string_lossy().into_owned(),
                ],
                if host.name == "claude" {
                    vec![
                        "plugin".into(),
                        "install".into(),
                        "leanck@leanck".into(),
                        "--scope".into(),
                        "user".into(),
                    ]
                } else {
                    vec!["plugin".into(), "add".into(), "leanck@leanck".into()]
                },
            ];
            for args in calls {
                if let Err(e) = command(Path::new(&host.name), &args, Some((key, &host.home))).await
                {
                    errors.push(format!("{}: {e:#}", host.name));
                    break;
                }
            }
        }
    }
    receipt.registration_pending = !errors.is_empty();
    receipt.write()?;
    Ok(Outcome {
        status: if errors.is_empty() {
            "updated"
        } else {
            "partial"
        }
        .into(),
        version: receipt.version.clone(),
        registration_errors: errors,
        restart_required: !receipt.hosts.is_empty(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{gate::Lease, receipt, test_support::scratch};
    use std::os::unix::fs::PermissionsExt;

    fn fixture() -> (tempfile::TempDir, Prepared) {
        let (dir, path) = scratch();
        let installed = path.join("bin/leanck");
        fs::create_dir(path.join("bin")).unwrap();
        fs::write(&installed, "old binary").unwrap();
        let download = tempfile::tempdir_in(&path).unwrap();
        let root = download.path().join("release");
        fs::create_dir_all(root.join("plugin/bin")).unwrap();
        fs::write(
            root.join("plugin/bin/leanck"),
            "#!/bin/sh\nprintf 'new completions\\n'\n",
        )
        .unwrap();
        fs::set_permissions(
            root.join("plugin/bin/leanck"),
            fs::Permissions::from_mode(0o755),
        )
        .unwrap();
        fs::create_dir_all(root.join("plugin/skills/prove")).unwrap();
        fs::write(root.join("plugin/skills/prove/SKILL.md"), "new skill").unwrap();
        let skill = path.join("skills/prove");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "old skill").unwrap();
        fs::write(skill.join(".leanck-installed"), "leanck installer v1\n").unwrap();
        let completion = path.join("completions.fish");
        fs::write(&completion, "old completions").unwrap();
        let components = vec![
            Component {
                kind: "file".into(),
                destination: installed.clone(),
                source: "plugin/bin/leanck".into(),
                fingerprint: digest_file(&installed).unwrap(),
            },
            Component {
                kind: "tree".into(),
                destination: skill.clone(),
                source: "plugin/skills/prove".into(),
                fingerprint: tree_digest(&skill).unwrap(),
            },
            Component {
                kind: "completion".into(),
                destination: completion.clone(),
                source: "fish".into(),
                fingerprint: digest_file(&completion).unwrap(),
            },
        ];
        let receipt = Receipt {
            schema: 1,
            generation: 1,
            version: "1.0.0".into(),
            provenance: "release".into(),
            target: target().unwrap(),
            mode: "standalone".into(),
            binary: installed,
            state_dir: path.join("state"),
            bundle: None,
            source: ReleaseSource::default(),
            pin: None,
            hosts: vec![],
            selected_skills: None,
            components: components.clone(),
            registration_pending: false,
        };
        receipt.write().unwrap();
        fs::write(
            receipt.state_dir.join("uninstall.tsv"),
            format!("profile\t-\t{}\n", path.join("profile").display()),
        )
        .unwrap();
        let plan = Plan {
            from_version: "1.0.0".into(),
            to_version: "2.0.0".into(),
            changes: components.iter().map(|c| c.destination.clone()).collect(),
            downgrade: false,
            recording_installer_ownership: None,
        };
        (
            dir,
            Prepared {
                _download: download,
                root,
                receipt,
                components,
                plan,
                adoption: None,
            },
        )
    }

    #[tokio::test]
    async fn native_update_keeps_choices_updates_inventory_and_retains_backups() {
        let (_dir, prepared) = fixture();
        let receipt = prepared.receipt.clone();
        let result = prepared.apply().await.unwrap();
        assert_eq!(result.status, "updated");
        let updated = Receipt::discover(&receipt.binary, None).unwrap().unwrap();
        assert_eq!(updated.version, "2.0.0");
        assert_eq!(updated.generation, 2);
        assert!(updated.pin.is_none());
        assert_eq!(
            fs::read_to_string(updated.components[1].destination.join("SKILL.md")).unwrap(),
            "new skill"
        );
        assert_eq!(
            fs::read_to_string(&updated.components[2].destination).unwrap(),
            "new completions\n"
        );
        for c in &updated.components {
            assert_eq!(receipt::fingerprint(c).unwrap(), c.fingerprint);
        }
        let inventory = fs::read_to_string(updated.state_dir.join("uninstall.tsv")).unwrap();
        assert!(inventory.contains("profile\t-\t"));
        assert!(!inventory.contains("completion\t"));
        // Backups live under the state directory with a manifest, never in
        // host directories, where hosts would load them as skills or agents.
        let backups: Vec<_> = fs::read_dir(updated.state_dir.join("backups"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert_eq!(backups.len(), 1);
        let manifest = fs::read_to_string(backups[0].join("manifest.tsv")).unwrap();
        for c in &updated.components {
            assert!(manifest.contains(&format!("{}\t", c.destination.display())));
            let siblings: Vec<_> = fs::read_dir(c.destination.parent().unwrap())
                .unwrap()
                .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
                .collect();
            assert!(
                siblings
                    .iter()
                    .all(|n| !n.starts_with(".leanck-backup") && !n.starts_with(".leanck-update")),
                "{siblings:?}"
            );
        }
    }

    #[test]
    fn installer_receipts_record_skill_choices_and_keep_plugins_bundled() {
        let (_dir, prepared) = fixture();
        let receipt = prepared.receipt;
        fs::write(
            receipt.state_dir.join("sources.tsv"),
            receipt
                .components
                .iter()
                .map(|c| {
                    format!(
                        "{}\t{}\t{}\t{}\n",
                        c.kind,
                        c.fingerprint,
                        c.destination.display(),
                        c.source
                    )
                })
                .collect::<String>(),
        )
        .unwrap();
        for (mode, selection, expected) in [
            (
                "standalone",
                "prove reference",
                Some(vec!["prove".into(), "reference".into()]),
            ),
            ("standalone", "none", Some(vec![])),
            ("standalone", "all", None),
            ("standalone", "", None),
            ("plugin", "prove", None),
        ] {
            fs::write(
                receipt.state_dir.join("selections.tsv"),
                format!("mode\t{mode}\ninstalled_version\t1.0.0\ninstalled_target\t{}\ndata_dir\t{}\nselected_skills\t{selection}\n", receipt.target, receipt.state_dir.display()),
            )
            .unwrap();
            let adopted = Receipt::from_installer(&receipt.state_dir, &receipt.binary).unwrap();
            assert_eq!(adopted.selected_skills, expected);
        }
    }

    #[tokio::test]
    async fn active_users_and_local_edits_block_before_any_replacement() {
        let (_dir, prepared) = fixture();
        let receipt = prepared.receipt.clone();
        let lease = Lease::acquire(&receipt).unwrap();
        assert!(
            prepared
                .apply()
                .await
                .unwrap_err()
                .to_string()
                .contains("in use")
        );
        assert_eq!(fs::read_to_string(&receipt.binary).unwrap(), "old binary");
        drop(lease);
        let (_dir, prepared) = fixture();
        let binary = prepared.receipt.binary.clone();
        fs::write(
            prepared.components[1].destination.join("SKILL.md"),
            "local edits",
        )
        .unwrap();
        assert!(
            prepared
                .apply()
                .await
                .unwrap_err()
                .to_string()
                .contains("local edits")
        );
        assert_eq!(fs::read_to_string(binary).unwrap(), "old binary");
    }

    fn plugin_fixture() -> (tempfile::TempDir, PathBuf) {
        let (dir, path) = scratch();
        let plugin = path.join("plugin");
        fs::create_dir_all(plugin.join(".codex-plugin")).unwrap();
        fs::write(
            plugin.join(".mcp.codex.json"),
            r#"{"mcpServers": {"leanck": {"command": "leanck", "args": ["serve", "mcp"]}}, "after": {}}"#,
        )
        .unwrap();
        fs::write(plugin.join(".mcp.json"), "{\"claude\": true}\n").unwrap();
        fs::write(
            plugin.join(".codex-plugin/plugin.json"),
            r#"{"name": "leanck", "version": "1.0.0", "mcpServers": "./.mcp.codex.json", "hooks": "./hooks/hooks.json", "keywords": []}"#,
        )
        .unwrap();
        (dir, plugin)
    }

    const CONFIGURED_CODEX_MCP: &str = r#"{
  "mcpServers": {
    "leanck": {
      "command": "/opt/leanck/bin/leanck",
      "args": [
        "serve",
        "mcp"
      ]
    }
  },
  "after": {}
}
"#;

    #[test]
    fn plugin_configuration_keeps_enabled_mcp_and_key_order() {
        let (_dir, plugin) = plugin_fixture();
        let manifest = fs::read(plugin.join(".codex-plugin/plugin.json")).unwrap();
        configure_plugin(&plugin, Path::new("/opt/leanck/bin/leanck"), true).unwrap();
        assert_eq!(
            fs::read_to_string(plugin.join(".mcp.codex.json")).unwrap(),
            CONFIGURED_CODEX_MCP
        );
        assert_eq!(
            fs::read_to_string(plugin.join(".mcp.json")).unwrap(),
            "{\"claude\": true}\n"
        );
        assert_eq!(
            fs::read(plugin.join(".codex-plugin/plugin.json")).unwrap(),
            manifest
        );
        assert!(!plugin.join(".mcp.disabled.json").exists());
        assert!(!plugin.join(".mcp.codex.disabled.json").exists());
    }

    #[test]
    fn plugin_configuration_disables_mcp_definitions_and_manifest_entry() {
        let (_dir, plugin) = plugin_fixture();
        configure_plugin(&plugin, Path::new("/opt/leanck/bin/leanck"), false).unwrap();
        assert!(!plugin.join(".mcp.json").exists());
        assert!(!plugin.join(".mcp.codex.json").exists());
        assert_eq!(
            fs::read_to_string(plugin.join(".mcp.disabled.json")).unwrap(),
            "{\"claude\": true}\n"
        );
        assert_eq!(
            fs::read_to_string(plugin.join(".mcp.codex.disabled.json")).unwrap(),
            CONFIGURED_CODEX_MCP
        );
        assert_eq!(
            fs::read_to_string(plugin.join(".codex-plugin/plugin.json")).unwrap(),
            r#"{
  "name": "leanck",
  "version": "1.0.0",
  "hooks": "./hooks/hooks.json",
  "keywords": []
}
"#
        );
    }

    #[tokio::test]
    async fn updates_follow_owned_parent_links_but_never_a_linked_destination() {
        crate::durability::relax_for_this_thread();
        // A dotfiles link above a managed skill is followed.
        let (_dir, prepared) = fixture();
        let skill = prepared.receipt.components[1].destination.clone();
        let skills = skill.parent().unwrap().to_owned();
        let dotfiles = skills.with_file_name("dotfiles-skills");
        fs::rename(&skills, &dotfiles).unwrap();
        std::os::unix::fs::symlink(&dotfiles, &skills).unwrap();
        let result = prepared.apply().await.unwrap();
        assert_eq!(result.status, "updated");
        assert_eq!(
            fs::read_to_string(dotfiles.join("prove/SKILL.md")).unwrap(),
            "new skill"
        );
        assert!(
            fs::symlink_metadata(&skills)
                .unwrap()
                .file_type()
                .is_symlink()
        );

        // A link at the managed path itself is refused before any change.
        let (_dir, prepared) = fixture();
        let skill = prepared.receipt.components[1].destination.clone();
        let moved = skill.with_file_name("moved");
        fs::rename(&skill, &moved).unwrap();
        std::os::unix::fs::symlink(&moved, &skill).unwrap();
        let binary = prepared.receipt.binary.clone();
        let error = format!("{:#}", prepared.apply().await.unwrap_err());
        assert!(error.contains("refusing symlink"), "{error}");
        assert_eq!(fs::read_to_string(&binary).unwrap(), "old binary");
        assert_eq!(
            fs::read_to_string(moved.join("SKILL.md")).unwrap(),
            "old skill"
        );
    }

    #[test]
    fn extraction_rejects_links_and_wrong_roots() {
        let (_dir, root) = scratch();
        for (name, link) in [("leanck/escape", true), ("foreign/file", false)] {
            let archive = root.join("archive.tar.gz");
            let encoder = flate2::write::GzEncoder::new(
                File::create(&archive).unwrap(),
                flate2::Compression::fast(),
            );
            let mut tar = tar::Builder::new(encoder);
            let mut header = tar::Header::new_gnu();
            header.set_mode(0o644);
            header.set_size(0);
            if link {
                header.set_entry_type(tar::EntryType::Symlink);
                header.set_link_name("/outside").unwrap();
            }
            header.set_cksum();
            tar.append_data(&mut header, name, std::io::empty())
                .unwrap();
            tar.into_inner().unwrap().finish().unwrap();
            let out = root.join("out");
            fs::create_dir_all(&out).unwrap();
            assert!(extract(&archive, &out).is_err());
        }
    }
}
