//! Installer state files, compatible with the shell installer's:
//! `selections.tsv` (remembered choices) and `uninstall.tsv` (the cumulative
//! uninstall inventory).
//!
//! Loading reads the files directly; saving only renders bytes, because every
//! state write goes through the installation journal.
use super::{Mode, Settings};
use crate::safe_path;
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
};

/// `selections.tsv` keys in the order the installer writes them.
pub const SELECTION_KEYS: [&str; 33] = [
    "mode",
    "plugin_claude",
    "plugin_codex",
    "from_repo",
    "release_url",
    "release_api",
    "installed_version",
    "installed_target",
    "version",
    "last_binary_path",
    "last_binary_digest",
    "last_completions_path",
    "last_completions_digest",
    "cli",
    "skills_claude",
    "skills_codex",
    "selected_skills",
    "agents_claude",
    "agents_codex",
    "add_to_path",
    "mcp_enabled",
    "standalone_bin_dir",
    "plugin_bin_dir",
    "data_dir",
    "claude_dir",
    "codex_dir",
    "codex_skills",
    "codex_agents_dir",
    "completions_dir",
    "fish_completions_dir",
    "path_file",
    "completions",
    "shell",
];

/// Remembered choices from `selections.tsv`, in file order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SavedState {
    entries: Vec<(String, String)>,
}

impl SavedState {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("selections.tsv")
    }

    /// Load `STATE/selections.tsv`; a missing file is empty state.
    pub fn load(state_dir: &Path) -> Result<Self> {
        let path = Self::path(state_dir);
        safe_path(&path)?;
        match fs::read(&path) {
            Ok(bytes) => Self::parse(
                &String::from_utf8(bytes)
                    .with_context(|| format!("{} is not UTF-8 text", path.display()))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e).with_context(|| format!("cannot read {}", path.display())),
        }
    }

    /// Parse `key<TAB>value` lines. Lines without a tab are keys with empty values.
    pub fn parse(text: &str) -> Result<Self> {
        let entries = text
            .lines()
            .filter(|line| !line.is_empty())
            .map(|line| {
                let (key, value) = line.split_once('\t').unwrap_or((line, ""));
                (key.to_owned(), value.to_owned())
            })
            .collect();
        Ok(Self { entries })
    }

    /// Every entry in file order; later entries override earlier ones.
    pub fn entries(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries.iter().map(|(k, v)| (k.as_str(), v.as_str()))
    }

    /// The last value recorded for `key`.
    pub fn get(&self, key: &str) -> Option<&str> {
        self.entries
            .iter()
            .rev()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    }

    /// The last recorded non-empty value for `key`.
    pub fn value(&self, key: &str) -> Option<&str> {
        self.get(key).filter(|v| !v.is_empty())
    }

    /// Render `selections.tsv` for `settings`. Fields the run does not
    /// produce (fingerprints of skipped components) keep their saved values.
    pub fn render(&self, settings: &Settings, record: &Recorded) -> String {
        let flag = |b: bool| if b { "1" } else { "0" }.to_owned();
        let path = |p: &Path| p.to_string_lossy().into_owned();
        let saved = |key: &str| self.get(key).unwrap_or_default().to_owned();
        let (standalone_bin_dir, plugin_bin_dir) = match settings.mode {
            Mode::Standalone => (
                path(&settings.bin_dir),
                settings
                    .plugin_bin_dir
                    .as_deref()
                    .map(path)
                    .unwrap_or_default(),
            ),
            Mode::Plugin => (
                path(&settings.standalone_bin_dir),
                if settings.bin_dir == settings.data_dir.join("bin") {
                    String::new()
                } else {
                    path(&settings.bin_dir)
                },
            ),
        };
        let (binary_path, binary_digest) = match &record.binary {
            Some((p, d)) => (path(p), d.clone()),
            None => (saved("last_binary_path"), saved("last_binary_digest")),
        };
        let (completion_path, completion_digest) = match &record.completions {
            Some((p, d)) => (path(p), d.clone()),
            None => (
                saved("last_completions_path"),
                saved("last_completions_digest"),
            ),
        };
        let mut out = String::new();
        for key in SELECTION_KEYS {
            let value = match key {
                "mode" => settings.mode.as_str().to_owned(),
                "plugin_claude" => flag(settings.plugin_hosts.claude),
                "plugin_codex" => flag(settings.plugin_hosts.codex),
                "from_repo" => settings.from_repo.as_deref().map(path).unwrap_or_default(),
                "release_url" => settings.release_url.clone(),
                "release_api" => match &settings.release_api {
                    Some(api) => api.clone(),
                    // Keep the shell installer's layout when no custom API was saved.
                    None => continue,
                },
                "installed_version" => record.installed_version.clone(),
                "installed_target" => record.installed_target.clone(),
                "version" => settings.version.clone(),
                "last_binary_path" => binary_path.clone(),
                "last_binary_digest" => binary_digest.clone(),
                "last_completions_path" => completion_path.clone(),
                "last_completions_digest" => completion_digest.clone(),
                "cli" => flag(settings.cli),
                "skills_claude" => flag(settings.skill_hosts.claude),
                "skills_codex" => flag(settings.skill_hosts.codex),
                "selected_skills" => settings.selected_skills.render(),
                "agents_claude" => flag(settings.agent_hosts.claude),
                "agents_codex" => flag(settings.agent_hosts.codex),
                "add_to_path" => flag(settings.add_to_path),
                "mcp_enabled" => flag(settings.mcp_enabled),
                "standalone_bin_dir" => standalone_bin_dir.clone(),
                "plugin_bin_dir" => plugin_bin_dir.clone(),
                "data_dir" => path(&settings.data_dir),
                "claude_dir" => path(&settings.claude_dir),
                "codex_dir" => path(&settings.codex_dir),
                "codex_skills" => path(&settings.codex_skills),
                "codex_agents_dir" => settings
                    .codex_agents_dir
                    .as_deref()
                    .map(path)
                    .unwrap_or_default(),
                "completions_dir" => settings
                    .completions_dir
                    .as_deref()
                    .map(path)
                    .unwrap_or_default(),
                "fish_completions_dir" => path(&settings.fish_completions_dir),
                "path_file" => path(&settings.path_file),
                "completions" => settings.completions.as_str().to_owned(),
                "shell" => settings.shell.clone(),
                _ => unreachable!(),
            };
            out.push_str(&format!("{key}\t{value}\n"));
        }
        for (key, list) in [
            ("extra_skills", &settings.extra_skills),
            ("extra_claude_agents", &settings.extra_claude_agents),
            ("extra_codex_agents", &settings.extra_codex_agents),
        ] {
            for location in list {
                out.push_str(&format!("{key}\t{}\n", location.display()));
            }
        }
        out
    }
}

/// Values a completed installation records in `selections.tsv`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recorded {
    /// `v1.2.3` or `v1.2.3 (local source)`.
    pub installed_version: String,
    /// `<arch>-<platform>`.
    pub installed_target: String,
    /// The installed executable and its digest, when this run installed it.
    pub binary: Option<(PathBuf, String)>,
    /// The installed completion script and its digest, when this run installed it.
    pub completions: Option<(PathBuf, String)>,
}

/// One `uninstall.tsv` record: `kind<TAB>expected<TAB>path`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryRecord {
    /// `tree`, `agent`, `link`, `file`, `profile`, or `host`.
    pub kind: String,
    /// A digest, a link target, `-` for profiles, or the host name.
    pub expected: String,
    /// The path, or the host home for `host` records.
    pub path: String,
}
impl InventoryRecord {
    pub fn new(kind: &str, expected: impl Into<String>, path: impl AsRef<Path>) -> Self {
        Self {
            kind: if kind == "completion" { "file" } else { kind }.to_owned(),
            expected: expected.into(),
            path: path.as_ref().to_string_lossy().into_owned(),
        }
    }
    /// Host records are scoped to the host home; others to the path.
    fn key(&self) -> (String, String, String) {
        if self.kind == "host" {
            (self.kind.clone(), self.expected.clone(), self.path.clone())
        } else {
            (String::new(), String::new(), self.path.clone())
        }
    }
}

/// The cumulative uninstall inventory, in first-recorded order.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UninstallInventory {
    pub records: Vec<InventoryRecord>,
}
impl UninstallInventory {
    pub fn path(state_dir: &Path) -> PathBuf {
        state_dir.join("uninstall.tsv")
    }

    /// Load `STATE/uninstall.tsv`, refusing links and non-regular files.
    pub fn load(state_dir: &Path) -> Result<Self> {
        let path = Self::path(state_dir);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                bail!("refusing symlink: {}", path.display())
            }
            Ok(meta) if !meta.is_file() => {
                bail!("invalid uninstall inventory: {}", path.display())
            }
            Ok(_) => Self::parse(&fs::read_to_string(&path)?),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.into()),
        }
    }

    pub fn parse(text: &str) -> Result<Self> {
        let mut inventory = Self::default();
        for line in text.lines().filter(|l| !l.is_empty()) {
            let mut parts = line.splitn(3, '\t');
            let (Some(kind), Some(expected), Some(path)) =
                (parts.next(), parts.next(), parts.next())
            else {
                bail!("malformed uninstall inventory");
            };
            ensure!(!path.is_empty(), "malformed uninstall inventory");
            inventory.merge([InventoryRecord {
                kind: kind.into(),
                expected: expected.into(),
                path: path.into(),
            }]);
        }
        Ok(inventory)
    }

    /// Add records; a later record replaces an earlier one for the same key
    /// in place, so the order stays deterministic.
    pub fn merge(&mut self, records: impl IntoIterator<Item = InventoryRecord>) {
        let mut index: HashMap<_, _> = self
            .records
            .iter()
            .enumerate()
            .map(|(i, r)| (r.key(), i))
            .collect();
        for record in records {
            match index.get(&record.key()) {
                Some(&i) => self.records[i] = record,
                None => {
                    index.insert(record.key(), self.records.len());
                    self.records.push(record);
                }
            }
        }
    }

    pub fn render(&self) -> String {
        self.records
            .iter()
            .map(|r| format!("{}\t{}\t{}\n", r.kind, r.expected, r.path))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_installer_selections_parse_with_later_values_winning() {
        let state = SavedState::parse(
            "mode\tplugin\nversion\t1.2.3\nextra_skills\t/a\nextra_skills\t/b\nlast_binary_path\t\nversion\tlatest\n",
        )
        .unwrap();
        assert_eq!(state.get("version"), Some("latest"));
        assert_eq!(state.get("last_binary_path"), Some(""));
        assert_eq!(state.value("last_binary_path"), None);
        assert_eq!(
            state
                .entries()
                .filter(|(k, _)| *k == "extra_skills")
                .map(|(_, v)| v)
                .collect::<Vec<_>>(),
            ["/a", "/b"]
        );
    }

    #[test]
    fn inventory_merges_last_wins_in_first_seen_order_and_scopes_hosts_by_home() {
        let mut inventory = UninstallInventory::parse(
            "tree\taaa\t/s/one\nfile\tbbb\t/bin/leanck\nhost\tclaude\t/h1\n",
        )
        .unwrap();
        inventory.merge([
            InventoryRecord::new("tree", "ccc", "/s/one"),
            InventoryRecord::new("host", "claude", "/h2"),
            InventoryRecord::new("completion", "ddd", "/c/_leanck"),
            InventoryRecord::new("host", "claude", "/h1"),
        ]);
        assert_eq!(
            inventory.render(),
            "tree\tccc\t/s/one\nfile\tbbb\t/bin/leanck\nhost\tclaude\t/h1\nhost\tclaude\t/h2\nfile\tddd\t/c/_leanck\n"
        );
        assert!(UninstallInventory::parse("tree\tonly-two\n").is_err());
    }

    #[test]
    fn inventory_refuses_links_and_non_regular_files() {
        let (_dir, root) = crate::test_support::scratch();
        std::os::unix::fs::symlink("/etc/hosts", root.join("uninstall.tsv")).unwrap();
        assert_eq!(
            UninstallInventory::load(&root).unwrap_err().to_string(),
            format!("refusing symlink: {}", root.join("uninstall.tsv").display())
        );
        fs::remove_file(root.join("uninstall.tsv")).unwrap();
        fs::create_dir(root.join("uninstall.tsv")).unwrap();
        assert!(
            UninstallInventory::load(&root)
                .unwrap_err()
                .to_string()
                .starts_with("invalid uninstall inventory")
        );
    }
}
