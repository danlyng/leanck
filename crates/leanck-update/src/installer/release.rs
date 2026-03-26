//! The contents of an unpacked release directory (`leanck/` in the archive).
use anyhow::{Context, Result, ensure};
use serde::Serialize;
use std::{
    fs,
    path::{Path, PathBuf},
};

/// The components a release directory offers.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ReleaseInventory {
    pub root: PathBuf,
    /// The `.leanck-release` tag, such as `v1.2.3`.
    pub tag: String,
    /// The first line of `.leanck-update-protocol`; 0 when missing or not a number.
    pub protocol: u32,
    /// Whether `docs/leanck.example.toml` exists.
    pub config_example: bool,
    /// Skill directory names under `plugin/skills`, in byte order.
    pub skills: Vec<String>,
    /// Claude agent file names (`NAME.md`) under `plugin/agents`.
    pub claude_agents: Vec<String>,
    /// Codex agent file names (`NAME.toml`) under `plugin/codex/agents`.
    pub codex_agents: Vec<String>,
}

impl ReleaseInventory {
    /// Read and validate an unpacked release directory.
    pub fn read(root: &Path) -> Result<Self> {
        let plugin = root.join("plugin");
        let complete = root.join(".leanck-release").is_file()
            && is_executable(&plugin.join("bin/leanck"))
            && plugin.join(".claude-plugin/plugin.json").is_file()
            && plugin.join(".codex-plugin/plugin.json").is_file();
        ensure!(complete, "incomplete release bundle");
        let tag = fs::read_to_string(root.join(".leanck-release"))?
            .trim()
            .to_owned();
        ensure!(
            tag.starts_with('v')
                && tag.len() > 1
                && tag
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'+' | b'-')),
            "invalid release version"
        );
        let protocol = match fs::read_to_string(root.join(".leanck-update-protocol")) {
            Ok(text) => text
                .lines()
                .next()
                .and_then(|line| line.parse().ok())
                .unwrap_or(0),
            Err(_) => 0,
        };
        Ok(Self {
            root: root.to_owned(),
            protocol,
            config_example: root.join("docs/leanck.example.toml").is_file(),
            skills: names(&plugin.join("skills"), |ty, _| ty.is_dir())?,
            claude_agents: names(&plugin.join("agents"), |ty, name| {
                ty.is_file() && name.ends_with(".md")
            })?,
            codex_agents: names(&plugin.join("codex/agents"), |ty, name| {
                ty.is_file() && name.ends_with(".toml")
            })?,
            tag,
        })
    }

    /// Check that the release is the requested `version` (`latest` or a tag without `v`).
    pub fn ensure_version(&self, version: &str) -> Result<()> {
        ensure!(
            version == "latest" || self.tag == format!("v{version}"),
            "release bundle version does not match requested version"
        );
        Ok(())
    }

    /// The release's semantic version without `v`.
    pub fn version(&self) -> &str {
        self.tag.trim_start_matches('v')
    }
}

fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Entry names in byte order. Names must be single-line text.
fn names(dir: &Path, keep: impl Fn(fs::FileType, &str) -> bool) -> Result<Vec<String>> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(vec![]),
        Err(e) => return Err(e).with_context(|| format!("cannot read {}", dir.display())),
    };
    let mut names = vec![];
    for entry in entries {
        let entry = entry?;
        let Ok(name) = entry.file_name().into_string() else {
            continue;
        };
        // Follow links like the shell glob tests did; skip hidden entries.
        let ty = fs::metadata(entry.path())?.file_type();
        if name.starts_with('.') || !keep(ty, &name) {
            continue;
        }
        crate::paths::valid_text(&name)?;
        names.push(name);
    }
    names.sort_by(|a, b| a.as_bytes().cmp(b.as_bytes()));
    Ok(names)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    /// A minimal unpacked release with the current skill and agent names.
    pub(crate) fn release(root: &Path, tag: &str) -> PathBuf {
        let release = root.join("leanck");
        let plugin = release.join("plugin");
        for dir in [
            ".claude-plugin",
            ".codex-plugin",
            "bin",
            "agents",
            "codex/agents",
        ] {
            fs::create_dir_all(plugin.join(dir)).unwrap();
        }
        fs::write(release.join(".leanck-release"), format!("{tag}\n")).unwrap();
        fs::write(release.join(".leanck-update-protocol"), "3\n").unwrap();
        fs::write(plugin.join(".claude-plugin/plugin.json"), "{}").unwrap();
        fs::write(plugin.join(".codex-plugin/plugin.json"), "{}").unwrap();
        fs::write(plugin.join("bin/leanck"), "#!/bin/sh\n").unwrap();
        fs::set_permissions(plugin.join("bin/leanck"), fs::Permissions::from_mode(0o755)).unwrap();
        for skill in ["leanck", "lean-proving", "lean-style"] {
            fs::create_dir_all(plugin.join("skills").join(skill)).unwrap();
            fs::write(plugin.join("skills").join(skill).join("SKILL.md"), skill).unwrap();
        }
        for agent in ["lean-expert", "lean-grind", "lean-style-worker"] {
            fs::write(plugin.join(format!("agents/{agent}.md")), agent).unwrap();
            fs::write(plugin.join(format!("codex/agents/{agent}.toml")), agent).unwrap();
        }
        release
    }

    #[test]
    fn inventory_lists_components_in_byte_order_and_reads_markers() {
        let (_dir, root) = crate::test_support::scratch();
        let dir = release(&root, "v1.2.3");
        fs::create_dir_all(dir.join("plugin/skills/leanck-x")).unwrap();
        fs::create_dir_all(dir.join("plugin/skills/.hidden")).unwrap();
        fs::write(dir.join("plugin/agents/notes.txt"), "").unwrap();
        let inventory = ReleaseInventory::read(&dir).unwrap();
        assert_eq!(inventory.tag, "v1.2.3");
        assert_eq!(inventory.protocol, 3);
        assert!(!inventory.config_example);
        assert_eq!(
            inventory.skills,
            ["lean-proving", "lean-style", "leanck", "leanck-x"]
        );
        assert_eq!(
            inventory.claude_agents,
            ["lean-expert.md", "lean-grind.md", "lean-style-worker.md"]
        );
        assert!(inventory.ensure_version("latest").is_ok());
        assert!(inventory.ensure_version("1.2.3").is_ok());
        assert_eq!(
            inventory.ensure_version("9.0.0").unwrap_err().to_string(),
            "release bundle version does not match requested version"
        );
        fs::write(dir.join(".leanck-update-protocol"), "two\n").unwrap();
        assert_eq!(ReleaseInventory::read(&dir).unwrap().protocol, 0);
        fs::remove_file(dir.join("plugin/.codex-plugin/plugin.json")).unwrap();
        assert_eq!(
            ReleaseInventory::read(&dir).unwrap_err().to_string(),
            "incomplete release bundle"
        );
    }
}
