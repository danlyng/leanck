use crate::{ReleaseSource, atomic_json, safe_path};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Component {
    pub kind: String,
    pub destination: PathBuf,
    pub source: String,
    pub fingerprint: String,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Host {
    pub name: String,
    pub home: PathBuf,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Receipt {
    pub schema: u32,
    pub generation: u64,
    pub version: String,
    pub provenance: String,
    pub target: String,
    pub mode: String,
    pub binary: PathBuf,
    pub state_dir: PathBuf,
    pub bundle: Option<PathBuf>,
    pub source: ReleaseSource,
    pub pin: Option<String>,
    pub hosts: Vec<Host>,
    pub components: Vec<Component>,
    #[serde(default)]
    pub registration_pending: bool,
}
#[derive(Serialize, Deserialize)]
struct Marker {
    state_dir: PathBuf,
}

impl Receipt {
    pub fn path(&self) -> PathBuf {
        self.state_dir.join("receipt.json")
    }
    pub fn marker(binary: &Path) -> PathBuf {
        binary.with_file_name(".leanck-installation.json")
    }
    pub fn recorded_state(binary: &Path) -> Result<Option<PathBuf>> {
        let path = Self::marker(binary);
        if !path.try_exists()? {
            return Ok(None);
        }
        safe_path(&path)?;
        Ok(Some(
            serde_json::from_slice::<Marker>(&fs::read(path)?)?.state_dir,
        ))
    }
    pub fn discover(binary: &Path, state: Option<&Path>) -> Result<Option<Self>> {
        let state = match state {
            Some(path) => path.to_owned(),
            None => {
                let marker = Self::marker(binary);
                if !marker.try_exists()? {
                    return Ok(None);
                }
                safe_path(&marker)?;
                serde_json::from_slice::<Marker>(&fs::read(marker)?)?.state_dir
            }
        };
        safe_path(&state)?;
        let path = state.join("receipt.json");
        safe_path(&path)?;
        let receipt: Self = serde_json::from_slice(&fs::read(&path).with_context(|| {
            format!(
                "cannot read {}; rerun the installer to record ownership",
                path.display()
            )
        })?)?;
        receipt.validate()?;
        ensure!(
            receipt.state_dir == state,
            "receipt state directory mismatch"
        );
        let bundled = receipt.bundle.as_ref().map(|p| p.join("plugin/bin/leanck"));
        let direct = receipt.binary == binary
            || bundled.as_deref() == Some(binary)
            || receipt
                .binary
                .canonicalize()
                .ok()
                .zip(binary.canonicalize().ok())
                .is_some_and(|(a, b)| a == b);
        // Hosts cache plugin copies, including the discovery marker. Bind a
        // relocated copy by content, without hashing ordinary direct launches.
        let cached_copy = !direct
            && binary.file_name().is_some_and(|name| name == "leanck")
            && receipt
                .components
                .iter()
                .find(|c| c.destination == receipt.binary && c.kind == "file")
                .is_some_and(|c| digest_file(binary).is_ok_and(|digest| digest == c.fingerprint));
        ensure!(
            direct || cached_copy,
            "receipt belongs to a different executable, or this plugin cache is stale; restart the host using {}",
            receipt.binary.display()
        );
        Ok(Some(receipt))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema == 1,
            "unsupported installation receipt {}; rerun a compatible installer",
            self.schema
        );
        semver::Version::parse(&self.version)?;
        self.source.validate()?;
        safe_path(&self.state_dir)?;
        safe_path(&self.binary)?;
        for host in &self.hosts {
            ensure!(
                matches!(host.name.as_str(), "claude" | "codex"),
                "invalid installation host"
            );
            safe_path(&host.home)?;
        }
        for component in &self.components {
            ensure!(
                component
                    .destination
                    .to_str()
                    .is_some_and(|s| !s.chars().any(char::is_control))
                    && !component.source.chars().any(char::is_control),
                "managed path contains unsupported characters"
            );
            ensure!(
                matches!(
                    component.kind.as_str(),
                    "file" | "agent" | "tree" | "link" | "completion"
                ),
                "unknown managed component kind"
            );
            let source = Path::new(&component.source);
            if component.kind != "completion" && component.kind != "link" {
                ensure!(
                    !source.is_absolute()
                        && !source
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir)),
                    "unsafe archive component source"
                );
            }
            // Leaf symlinks are allowed only for explicitly recorded agent links.
            safe_path(
                component
                    .destination
                    .parent()
                    .context("component parent missing")?,
            )?;
            if component.kind != "link" {
                safe_path(&component.destination)?;
            }
        }
        ensure!(
            self.components
                .iter()
                .any(|c| c.destination == self.binary && c.kind == "file"),
            "receipt does not own the executable"
        );
        Ok(())
    }
    pub fn from_installer(state: &Path, binary: &Path) -> Result<Self> {
        safe_path(state)?;
        let selections = fs::read_to_string(state.join("selections.tsv"))?;
        let values: BTreeMap<_, _> = selections
            .lines()
            .filter_map(|l| l.split_once('\t'))
            .collect();
        let value = |key| values.get(key).copied().unwrap_or("");
        ensure!(
            !value("mode").is_empty(),
            "legacy selections lack mode/host information; rerun the installer to adopt this installation"
        );
        let components = fs::read_to_string(state.join("sources.tsv"))?
            .lines()
            .map(|line| {
                let parts: Vec<_> = line.split('\t').collect();
                ensure!(parts.len() == 4, "malformed component source record");
                Ok(Component {
                    kind: parts[0].into(),
                    fingerprint: parts[1].into(),
                    destination: parts[2].into(),
                    source: parts[3].into(),
                })
            })
            .collect::<Result<Vec<_>>>()?;
        let version = value("installed_version")
            .trim_start_matches('v')
            .split_whitespace()
            .next()
            .context("missing installed version")?
            .to_owned();
        let mode = value("mode").to_owned();
        let mut hosts = vec![];
        for (name, enabled, home) in [
            ("claude", "plugin_claude", "claude_dir"),
            ("codex", "plugin_codex", "codex_dir"),
        ] {
            if value(enabled) == "1" {
                hosts.push(Host {
                    name: name.into(),
                    home: value(home).into(),
                });
            }
        }
        let mut source = ReleaseSource::default();
        if !value("release_api").is_empty() {
            source.api = value("release_api").into();
        }
        if !value("release_url").is_empty() {
            source.releases = value("release_url").into();
        }
        let registration_pending = !hosts.is_empty();
        let receipt = Self {
            schema: 1,
            generation: crate::now(),
            version,
            provenance: if value("from_repo").is_empty() {
                "release"
            } else {
                "source"
            }
            .into(),
            target: value("installed_target").into(),
            mode: mode.clone(),
            binary: binary.into(),
            state_dir: state.into(),
            bundle: (mode == "plugin").then(|| PathBuf::from(value("data_dir")).join("bundle")),
            source,
            pin: (!matches!(value("version"), "" | "latest"))
                .then(|| value("version").trim_start_matches('v').to_owned()),
            hosts,
            components,
            registration_pending,
        };
        receipt.validate()?;
        Ok(receipt)
    }
    pub fn write(&self) -> Result<()> {
        self.validate()?;
        atomic_json(&self.path(), self)?;
        self.write_marker(&Self::marker(&self.binary))?;
        if let Some(bundle) = &self.bundle {
            self.write_marker(&Self::marker(&bundle.join("plugin/bin/leanck")))?;
        }
        Ok(())
    }
    fn write_marker(&self, path: &Path) -> Result<()> {
        safe_path(path)?;
        if path.try_exists()? {
            let marker: Marker = serde_json::from_slice(&fs::read(path)?)?;
            ensure!(
                marker.state_dir == self.state_dir,
                "installation marker points at different state"
            );
            return Ok(()); // Preserve installer fingerprints, including JSON whitespace.
        }
        atomic_json(
            path,
            &Marker {
                state_dir: self.state_dir.clone(),
            },
        )
    }
}

pub fn digest_file(path: &Path) -> Result<String> {
    let mut file = fs::File::open(path)?;
    let mut hash = Sha256::new();
    let mut buffer = [0; 64 * 1024];
    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hash.update(&buffer[..n]);
    }
    Ok(hex::encode(hash.finalize()))
}

pub(crate) fn files(root: &Path) -> Result<Vec<PathBuf>> {
    let mut paths = vec![];
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let ty = entry.file_type()?;
        ensure!(
            !ty.is_symlink(),
            "unexpected link in managed tree: {}",
            entry.path().display()
        );
        if ty.is_dir() {
            paths.extend(files(&entry.path())?);
        } else {
            ensure!(ty.is_file(), "special file in managed tree");
            paths.push(entry.path());
        }
    }
    paths.sort();
    Ok(paths)
}

pub fn tree_digest(path: &Path) -> Result<String> {
    let mut records = String::new();
    for file in files(path)? {
        let relative = file.strip_prefix(path)?;
        if relative == Path::new(".leanck-content-sha256")
            || relative
                .file_name()
                .is_some_and(|n| n == ".leanck-installation.json")
        {
            continue;
        }
        records.push_str(&format!(
            "{} ./{}\n",
            digest_file(&file)?,
            relative.to_string_lossy()
        ));
    }
    Ok(hex::encode(Sha256::digest(records)))
}

pub(crate) fn fingerprint(component: &Component) -> Result<String> {
    match component.kind.as_str() {
        "tree" => tree_digest(&component.destination),
        "link" => Ok(fs::read_link(&component.destination)?
            .to_string_lossy()
            .into_owned()),
        _ => digest_file(&component.destination),
    }
}
