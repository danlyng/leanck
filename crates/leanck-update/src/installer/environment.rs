//! A snapshot of everything planning reads from the outside world:
//! environment variables, `PATH` lookups, and the state of candidate paths.
//!
//! Planning is pure. When it needs a path that has not been observed yet, it
//! reports the missing [`Probe`]s instead of touching the filesystem;
//! [`FileSnapshot::observe`] gathers them and planning runs again
//! (see [`super::plan_observed`]).
use crate::{
    paths::absolute,
    receipt::{digest_file, tree_digest},
};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use std::{
    collections::{BTreeMap, BTreeSet},
    ffi::OsString,
    fs,
    io::Read,
    path::{Path, PathBuf},
};

/// Files at most this large keep their text, for ownership markers and sidecars.
const TEXT_LIMIT: u64 = 4096;

/// Environment inputs to planning, captured once.
#[derive(Clone, Debug, Default)]
pub struct Environment {
    pub home: PathBuf,
    pub cwd: PathBuf,
    /// Normalized XDG bases, when set and non-empty.
    pub xdg_data_home: Option<PathBuf>,
    pub xdg_state_home: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    /// `CLAUDE_CONFIG_DIR` and `CODEX_HOME`, normalized.
    pub claude_config_dir: Option<PathBuf>,
    pub codex_home: Option<PathBuf>,
    /// The inherited `SHELL`, verbatim.
    pub shell: Option<String>,
    /// The original `PATH` entries.
    pub path: Vec<PathBuf>,
    /// Commands found on `PATH` among [`Environment::COMMANDS`].
    pub commands: BTreeSet<String>,
    /// The `<arch>-<platform>` target this executable was built for.
    pub target: String,
    /// Observed paths.
    pub files: FileSnapshot,
}

impl Environment {
    /// Commands whose presence affects planning or final messages.
    pub const COMMANDS: [&'static str; 4] = ["claude", "codex", "leanck", "lake"];

    /// Capture the process environment.
    pub fn capture() -> Result<Self> {
        let vars: BTreeMap<String, OsString> = std::env::vars_os()
            .filter_map(|(k, v)| Some((k.into_string().ok()?, v)))
            .collect();
        let cwd = std::env::current_dir().context("cannot read the working directory")?;
        let mut env = Self::from_vars(&vars, cwd)?;
        env.target = crate::install::target()
            .unwrap_or_else(|_| format!("{}-{}", std::env::consts::ARCH, std::env::consts::OS));
        for name in Self::COMMANDS {
            if env.path.iter().any(|dir| is_executable(&dir.join(name))) {
                env.commands.insert(name.into());
            }
        }
        Ok(env)
    }

    /// Build an environment from variables; commands and files start empty.
    pub fn from_vars(vars: &BTreeMap<String, OsString>, cwd: PathBuf) -> Result<Self> {
        let text = |key: &str| -> Result<Option<String>> {
            match vars.get(key) {
                None => Ok(None),
                Some(value) if value.is_empty() => Ok(None),
                Some(value) => Ok(Some(
                    value
                        .to_str()
                        .with_context(|| format!("{key} must be valid UTF-8"))?
                        .to_owned(),
                )),
            }
        };
        let Some(home) = text("HOME")? else {
            bail!("HOME must be set");
        };
        let home = absolute(&home, Path::new("/"), &cwd)?;
        let dir = |key: &str| -> Result<Option<PathBuf>> {
            text(key)?
                .map(|value| absolute(&value, &home, &cwd))
                .transpose()
        };
        Ok(Self {
            xdg_data_home: dir("XDG_DATA_HOME")?,
            xdg_state_home: dir("XDG_STATE_HOME")?,
            xdg_config_home: dir("XDG_CONFIG_HOME")?,
            claude_config_dir: dir("CLAUDE_CONFIG_DIR")?,
            codex_home: dir("CODEX_HOME")?,
            shell: text("SHELL")?,
            path: vars
                .get("PATH")
                .map(|p| std::env::split_paths(p).collect())
                .unwrap_or_default(),
            commands: BTreeSet::new(),
            target: String::new(),
            files: FileSnapshot::default(),
            home,
            cwd,
        })
    }

    pub fn data_home(&self) -> PathBuf {
        self.xdg_data_home
            .clone()
            .unwrap_or_else(|| self.home.join(".local/share"))
    }
    pub fn state_home(&self) -> PathBuf {
        self.xdg_state_home
            .clone()
            .unwrap_or_else(|| self.home.join(".local/state"))
    }
    pub fn config_home(&self) -> PathBuf {
        self.xdg_config_home
            .clone()
            .unwrap_or_else(|| self.home.join(".config"))
    }
    pub fn has_command(&self, name: &str) -> bool {
        self.commands.contains(name)
    }
    /// Whether `dir` was on the original `PATH`.
    pub fn on_path(&self, dir: &Path) -> bool {
        self.path.iter().any(|entry| entry == dir)
    }
    /// `Claude Code, Codex`, `Claude Code`, `Codex`, or `none`.
    pub fn detected_agents(&self) -> &'static str {
        match (self.has_command("claude"), self.has_command("codex")) {
            (true, true) => "Claude Code, Codex",
            (true, false) => "Claude Code",
            (false, true) => "Codex",
            (false, false) => "none",
        }
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// What exists at a path, without following a final symlink.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Node {
    Missing,
    /// A regular file, its SHA-256, and its text when small and UTF-8.
    File {
        digest: String,
        text: Option<String>,
    },
    Directory,
    /// A symlink, its target, and what it resolves to (never itself a link).
    Symlink {
        target: PathBuf,
        resolved: Box<Node>,
    },
    /// A socket, FIFO, or device.
    Other,
}
impl Node {
    /// `[ -e P ] || [ -L P ]`
    pub fn present(&self) -> bool {
        !matches!(self, Node::Missing)
    }
    /// `[ -L P ]`
    pub fn is_symlink(&self) -> bool {
        matches!(self, Node::Symlink { .. })
    }
    fn followed(&self) -> &Node {
        match self {
            Node::Symlink { resolved, .. } => resolved,
            node => node,
        }
    }
    /// `[ -e P ]`, following links.
    pub fn exists(&self) -> bool {
        self.followed().present()
    }
    /// `[ -f P ]`, following links.
    pub fn is_file(&self) -> bool {
        matches!(self.followed(), Node::File { .. })
    }
    /// `[ -d P ]`, following links.
    pub fn is_dir(&self) -> bool {
        matches!(self.followed(), Node::Directory)
    }
    /// The digest of the regular file this path resolves to.
    pub fn digest(&self) -> Option<&str> {
        match self.followed() {
            Node::File { digest, .. } => Some(digest),
            _ => None,
        }
    }
    /// The file text, without trailing newlines, as `$(cat P)` reads it.
    pub fn text(&self) -> Option<&str> {
        match self.followed() {
            Node::File {
                text: Some(text), ..
            } => Some(text.trim_end_matches('\n')),
            _ => None,
        }
    }
    pub fn link_target(&self) -> Option<&Path> {
        match self {
            Node::Symlink { target, .. } => Some(target),
            _ => None,
        }
    }
}

/// One observation planning needs.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Probe {
    /// The node at a path.
    Node(PathBuf),
    /// A managed tree's recomputed digest.
    Tree(PathBuf),
    /// Whether a shell profile's managed PATH markers are balanced.
    Profile(PathBuf),
}

/// Observed paths, keyed by absolute path.
#[derive(Clone, Debug, Default)]
pub struct FileSnapshot {
    nodes: BTreeMap<PathBuf, Node>,
    /// `None` when the tree contains links or special files, or cannot be read.
    trees: BTreeMap<PathBuf, Option<String>>,
    profiles: BTreeMap<PathBuf, bool>,
    /// Treat unobserved paths as missing instead of reporting them (tests).
    assume_missing: bool,
}

impl FileSnapshot {
    /// A snapshot in which every unrecorded path is missing.
    pub fn assume_missing() -> Self {
        Self {
            assume_missing: true,
            ..Self::default()
        }
    }
    pub fn assumes_missing(&self) -> bool {
        self.assume_missing
    }

    pub fn node(&self, path: &Path) -> Option<&Node> {
        self.nodes.get(path)
    }
    pub fn tree(&self, path: &Path) -> Option<&Option<String>> {
        self.trees.get(path)
    }
    pub fn profile(&self, path: &Path) -> Option<bool> {
        self.profiles.get(path).copied()
    }

    /// Record observations directly (tests and callers with prior knowledge).
    pub fn insert(&mut self, path: impl Into<PathBuf>, node: Node) -> &mut Self {
        self.nodes.insert(path.into(), node);
        self
    }
    pub fn insert_tree(&mut self, path: impl Into<PathBuf>, digest: Option<String>) -> &mut Self {
        self.trees.insert(path.into(), digest);
        self
    }
    pub fn insert_profile(&mut self, path: impl Into<PathBuf>, balanced: bool) -> &mut Self {
        self.profiles.insert(path.into(), balanced);
        self
    }

    /// Observe each probe on the filesystem.
    pub fn observe(&mut self, probes: &[Probe]) -> Result<()> {
        for probe in probes {
            match probe {
                Probe::Node(path) => {
                    let node = observe_node(path)?;
                    self.nodes.insert(path.clone(), node);
                }
                Probe::Tree(path) => {
                    let digest = match fs::symlink_metadata(path) {
                        Ok(meta) if meta.is_dir() => tree_digest(path).ok(),
                        _ => None,
                    };
                    self.trees.insert(path.clone(), digest);
                }
                Probe::Profile(path) => {
                    let balanced = match fs::read(path) {
                        Ok(bytes) => balanced_markers(&bytes),
                        Err(_) => false,
                    };
                    self.profiles.insert(path.clone(), balanced);
                }
            }
        }
        Ok(())
    }
}

/// Observe a path without following a final symlink.
pub fn observe_node(path: &Path) -> Result<Node> {
    let meta = match fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if is_absent(&e) => return Ok(Node::Missing),
        Err(e) => return Err(e).with_context(|| format!("cannot inspect {}", path.display())),
    };
    let ty = meta.file_type();
    if ty.is_symlink() {
        let target = fs::read_link(path)?;
        let resolved = match fs::metadata(path) {
            Ok(meta) => describe(path, &meta)?,
            Err(_) => Node::Missing,
        };
        return Ok(Node::Symlink {
            target,
            resolved: Box::new(resolved),
        });
    }
    describe(path, &meta)
}

fn describe(path: &Path, meta: &fs::Metadata) -> Result<Node> {
    Ok(if meta.is_dir() {
        Node::Directory
    } else if meta.is_file() {
        let text = if meta.len() <= TEXT_LIMIT {
            let mut bytes = vec![];
            fs::File::open(path)?
                .take(TEXT_LIMIT)
                .read_to_end(&mut bytes)?;
            String::from_utf8(bytes).ok()
        } else {
            None
        };
        Node::File {
            digest: digest_file(path)?,
            text,
        }
    } else {
        Node::Other
    })
}

fn is_absent(error: &std::io::Error) -> bool {
    error.kind() == std::io::ErrorKind::NotFound || error.raw_os_error() == Some(libc::ENOTDIR)
}

pub const PATH_BLOCK_START: &str = "# >>> leanck PATH >>>";
pub const PATH_BLOCK_END: &str = "# <<< leanck PATH <<<";

/// At most one managed PATH block, properly opened and closed.
pub fn balanced_markers(profile: &[u8]) -> bool {
    let (mut open, mut count) = (false, 0);
    for line in profile.split(|&b| b == b'\n') {
        if line == PATH_BLOCK_START.as_bytes() {
            if open || count > 0 {
                return false;
            }
            open = true;
            count += 1;
        } else if line == PATH_BLOCK_END.as_bytes() {
            if !open {
                return false;
            }
            open = false;
        }
    }
    !open
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> BTreeMap<String, OsString> {
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), OsString::from(v)))
            .collect()
    }

    #[test]
    fn relative_and_parent_xdg_values_are_normalized_against_the_working_directory() {
        let env = Environment::from_vars(
            &vars(&[
                ("HOME", "/home/u"),
                ("XDG_STATE_HOME", "state/../st"),
                ("XDG_DATA_HOME", ""),
                ("CODEX_HOME", "~/codex"),
            ]),
            PathBuf::from("/work"),
        )
        .unwrap();
        assert_eq!(env.state_home(), Path::new("/work/st"));
        assert_eq!(env.data_home(), Path::new("/home/u/.local/share"));
        assert_eq!(env.codex_home.as_deref(), Some(Path::new("/home/u/codex")));
        assert!(
            Environment::from_vars(&vars(&[("HOME", "")]), PathBuf::from("/"))
                .unwrap_err()
                .to_string()
                .contains("HOME must be set")
        );
    }

    #[test]
    fn markers_balance_like_the_shell_installer_checks_them() {
        for (text, expected) in [
            ("", true),
            ("# >>> leanck PATH >>>\nx\n# <<< leanck PATH <<<\n", true),
            ("# >>> leanck PATH >>>\nkeep this text\n", false),
            ("# <<< leanck PATH <<<\n", false),
            (
                "# >>> leanck PATH >>>\n# <<< leanck PATH <<<\n# >>> leanck PATH >>>\n# <<< leanck PATH <<<\n",
                false,
            ),
            ("  # >>> leanck PATH >>>\n", true),
        ] {
            assert_eq!(balanced_markers(text.as_bytes()), expected, "{text:?}");
        }
    }

    #[test]
    fn nodes_describe_links_without_following_them_for_identity() {
        let (_dir, root) = crate::test_support::scratch();
        fs::write(root.join("file"), "leanck installer v1\n").unwrap();
        std::os::unix::fs::symlink(root.join("file"), root.join("link")).unwrap();
        std::os::unix::fs::symlink(root.join("gone"), root.join("dangling")).unwrap();
        let file = observe_node(&root.join("file")).unwrap();
        assert_eq!(file.text(), Some("leanck installer v1"));
        let link = observe_node(&root.join("link")).unwrap();
        assert!(link.is_symlink() && link.is_file() && link.exists());
        assert_eq!(link.digest(), file.digest());
        let dangling = observe_node(&root.join("dangling")).unwrap();
        assert!(dangling.present() && !dangling.exists() && dangling.digest().is_none());
        assert_eq!(
            observe_node(&root.join("file/child")).unwrap(),
            Node::Missing
        );
    }
}
