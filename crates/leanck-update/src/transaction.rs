//! Durable, journaled replacement of installed paths.
//!
//! Every replaced or removed path is first copied (links as links) into
//! `STATE/backups/<YYYYmmddTHHMMSS>-<pid>/<seq>`, and `manifest.tsv` in that
//! directory records `destination<TAB>backup`. Backups never land beside
//! destinations, where hosts would load them as skills or agents, and they are
//! kept after both commit and rollback.
use crate::{atomic_json, safe_path};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::{Component, Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    destination: PathBuf,
    /// A complete backup copy, when `existed`. Older journals named a sibling
    /// `.leanck-backup-*` path here; recovery still accepts those.
    #[serde(default)]
    backup: Option<PathBuf>,
    existed: bool,
    staged: Option<PathBuf>,
}
#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    schema: u32,
    committed: bool,
    entries: Vec<Entry>,
    /// The backup directory, once the first backup is taken.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    backups: Option<PathBuf>,
}

/// The journal is durable before each mutation, and each backup is complete
/// before its destination changes.
pub(crate) struct Transaction {
    state: PathBuf,
    path: PathBuf,
    journal: Journal,
}
impl Transaction {
    pub fn recover(state: &Path) -> Result<()> {
        let path = state.join("update-journal.json");
        if !path.try_exists()? {
            return Ok(());
        }
        safe_path(&path)?;
        let journal: Journal = serde_json::from_slice(&fs::read(&path)?)?;
        ensure!(
            journal.schema == 1,
            "unsupported update journal; manual recovery required"
        );
        let mut tx = Self {
            state: state.to_owned(),
            path,
            journal,
        };
        if tx.journal.committed {
            tx.cleanup()?;
        } else {
            tx.rollback()?;
        }
        Ok(())
    }
    pub fn new(state: &Path) -> Result<Self> {
        Self::recover(state)?;
        let tx = Self {
            state: state.to_owned(),
            path: state.join("update-journal.json"),
            journal: Journal {
                schema: 1,
                committed: false,
                entries: vec![],
                backups: None,
            },
        };
        atomic_json(&tx.path, &tx.journal)?;
        Ok(tx)
    }
    /// The directory holding this transaction's backups, once one was taken.
    // The installer's apply layer reports it as "Replacement backups: DIR".
    #[allow(dead_code)]
    pub fn backup_directory(&self) -> Option<&Path> {
        self.journal.backups.as_deref()
    }
    /// Move `staged` into place at `destination`, or remove `destination` when
    /// `staged` is `None`. An existing destination is backed up first.
    pub fn replace(&mut self, staged: Option<PathBuf>, destination: PathBuf) -> Result<()> {
        let parent = destination
            .parent()
            .context("destination parent missing")?
            .to_owned();
        safe_path(&parent)?;
        fs::create_dir_all(&parent)?;
        let existed = fs::symlink_metadata(&destination).is_ok();
        let backup = if existed {
            let root = self.backup_root()?;
            Some(root.join(self.journal.entries.len().to_string()))
        } else {
            None
        };
        if let Some(backup) = &backup {
            ensure!(
                fs::symlink_metadata(backup).is_err(),
                "backup already exists: {}",
                backup.display()
            );
        }
        self.journal.entries.push(Entry {
            destination: destination.clone(),
            backup: backup.clone(),
            existed,
            staged: staged.clone(),
        });
        atomic_json(&self.path, &self.journal)?;
        if let Some(backup) = &backup {
            // Copy under a temporary name, then rename: recovery trusts only
            // complete backups, and the destination is untouched until then.
            let root = backup.parent().context("backup parent missing")?;
            let partial = root.join(format!(
                ".{}.partial",
                backup.file_name().unwrap().to_string_lossy()
            ));
            remove(&partial)?;
            copy_preserving(&destination, &partial)?;
            sync_tree(&partial)?;
            fs::rename(&partial, backup)?;
            let mut manifest = OpenOptions::new()
                .create(true)
                .append(true)
                .open(root.join("manifest.tsv"))?;
            manifest.write_all(
                format!("{}\t{}\n", destination.display(), backup.display()).as_bytes(),
            )?;
            manifest.sync_all()?;
            File::open(root)?.sync_all()?;
        }
        if let Some(staged) = staged {
            // Directories cannot be renamed over; files and links can, atomically.
            let replace_in_place = !existed
                || !(fs::symlink_metadata(&destination)?.is_dir()
                    || fs::symlink_metadata(&staged)?.is_dir());
            if !replace_in_place {
                remove(&destination)?;
            }
            fs::rename(staged, &destination)?;
        } else if existed {
            remove(&destination)?;
        }
        File::open(&parent)?.sync_all()?;
        Ok(())
    }
    pub fn commit(&mut self) -> Result<()> {
        self.journal.committed = true;
        atomic_json(&self.path, &self.journal)?;
        self.cleanup()
    }
    fn cleanup(&mut self) -> Result<()> {
        // Backups and their manifest stay under STATE/backups for repair.
        fs::remove_file(&self.path)?;
        File::open(&self.state)?.sync_all()?;
        Ok(())
    }
    pub fn rollback(&mut self) -> Result<()> {
        for (index, entry) in self.journal.entries.iter().enumerate().rev() {
            let parent = entry
                .destination
                .parent()
                .context("journal parent missing")?;
            safe_path(parent)?;
            match &entry.backup {
                Some(backup) if fs::symlink_metadata(backup).is_ok() => {
                    if self.legacy_backup(entry, backup)? {
                        remove(&entry.destination)?;
                        fs::rename(backup, &entry.destination)?;
                    } else {
                        // Restore a copy, keeping the backup itself.
                        let restoring = parent.join(format!(".leanck-restore-{index}"));
                        remove(&restoring)?;
                        copy_preserving(backup, &restoring)?;
                        sync_tree(&restoring)?;
                        remove(&entry.destination)?;
                        fs::rename(&restoring, &entry.destination)?;
                    }
                }
                // The backup never completed, so the destination never changed.
                Some(_) => {}
                None if !entry.existed => remove(&entry.destination)?,
                None => bail!("invalid backup path in recovery journal"),
            }
            File::open(parent)?.sync_all()?;
        }
        fs::remove_file(&self.path)?;
        File::open(&self.state)?.sync_all()?;
        Ok(())
    }

    /// Validate a journal backup path, reporting whether it uses the older
    /// layout beside its destination.
    fn legacy_backup(&self, entry: &Entry, backup: &Path) -> Result<bool> {
        if backup.parent() == entry.destination.parent()
            && backup
                .file_name()
                .is_some_and(|n| n.to_string_lossy().starts_with(".leanck-backup-"))
        {
            safe_path(backup.parent().context("backup parent missing")?)?;
            return Ok(true);
        }
        ensure!(
            backup.starts_with(self.state.join("backups"))
                && !backup
                    .components()
                    .any(|c| matches!(c, Component::ParentDir)),
            "invalid backup path in recovery journal"
        );
        // The backup itself may be a link copied from a link destination.
        safe_path(backup.parent().context("backup parent missing")?)?;
        Ok(false)
    }

    fn backup_root(&mut self) -> Result<PathBuf> {
        if let Some(root) = &self.journal.backups {
            return Ok(root.clone());
        }
        let parent = self.state.join("backups");
        safe_path(&parent)?;
        fs::create_dir_all(&parent)?;
        let name = format!("{}-{}", timestamp(), std::process::id());
        let mut attempt = 0;
        let root = loop {
            let candidate = if attempt == 0 {
                parent.join(&name)
            } else {
                parent.join(format!("{name}.{attempt}"))
            };
            match fs::create_dir(&candidate) {
                Ok(()) => break candidate,
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && attempt < 1000 => {
                    attempt += 1;
                }
                Err(e) => return Err(e.into()),
            }
        };
        File::open(&parent)?.sync_all()?;
        self.journal.backups = Some(root.clone());
        atomic_json(&self.path, &self.journal)?;
        Ok(root)
    }
}

/// Local time as `YYYYmmddTHHMMSS`, matching `date +%Y%m%dT%H%M%S`.
fn timestamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as libc::time_t;
    // SAFETY: localtime_r writes only into the provided, zero-initialized tm.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    let ok = unsafe { !libc::localtime_r(&now, &mut tm).is_null() };
    if !ok {
        return format!("{now}");
    }
    format!(
        "{:04}{:02}{:02}T{:02}{:02}{:02}",
        tm.tm_year + 1900,
        tm.tm_mon + 1,
        tm.tm_mday,
        tm.tm_hour,
        tm.tm_min,
        tm.tm_sec
    )
}

/// Copy like `cp -p -R -P`: links stay links and are never followed; modes are kept.
pub(crate) fn copy_preserving(source: &Path, destination: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(source)?;
    let ty = meta.file_type();
    if ty.is_symlink() {
        std::os::unix::fs::symlink(fs::read_link(source)?, destination)?;
    } else if ty.is_dir() {
        fs::create_dir(destination)?;
        for entry in fs::read_dir(source)? {
            let entry = entry?;
            copy_preserving(&entry.path(), &destination.join(entry.file_name()))?;
        }
        fs::set_permissions(destination, meta.permissions())?;
    } else if ty.is_file() {
        fs::copy(source, destination)?;
    } else {
        bail!("cannot back up special file: {}", source.display());
    }
    Ok(())
}

fn sync_tree(path: &Path) -> Result<()> {
    let meta = fs::symlink_metadata(path)?;
    if meta.is_dir() {
        for entry in fs::read_dir(path)? {
            sync_tree(&entry?.path())?;
        }
        File::open(path)?.sync_all()?;
    } else if meta.is_file() {
        File::open(path)?.sync_all()?;
    }
    Ok(())
}

pub(crate) fn remove(path: &Path) -> Result<()> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path)?,
        Ok(_) => fs::remove_file(path)?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::scratch;

    fn backup_dirs(state: &Path) -> Vec<PathBuf> {
        match fs::read_dir(state.join("backups")) {
            Ok(entries) => entries.map(|e| e.unwrap().path()).collect(),
            Err(_) => vec![],
        }
    }

    /// Every name in `dir` that does not belong to the transaction's own inputs.
    fn names(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn interrupted_update_is_recovered_and_recovery_is_idempotent() {
        let (_dir, root) = scratch();
        let dest = root.join("installed");
        let staged = root.join("new");
        fs::write(&dest, "old").unwrap();
        fs::write(&staged, "new").unwrap();
        let mut tx = Transaction::new(&root).unwrap();
        tx.replace(Some(staged), dest.clone()).unwrap();
        drop(tx); // Simulate process exit without commit or rollback.
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
        Transaction::recover(&root).unwrap();
        Transaction::recover(&root).unwrap();
        assert_eq!(fs::read_to_string(dest).unwrap(), "old");
    }

    #[test]
    fn failed_move_restores_all_paths_and_commits_retain_backups() {
        let (_dir, root) = scratch();
        let a = root.join("a");
        let b = root.join("b");
        let new = root.join("new");
        fs::write(&a, "old a").unwrap();
        fs::write(&b, "old b").unwrap();
        fs::write(&new, "new a").unwrap();
        let mut tx = Transaction::new(&root).unwrap();
        tx.replace(Some(new), a.clone()).unwrap();
        assert!(tx.replace(Some(root.join("missing")), b.clone()).is_err());
        tx.rollback().unwrap();
        assert_eq!(fs::read_to_string(&a).unwrap(), "old a");
        assert_eq!(fs::read_to_string(&b).unwrap(), "old b");
        fs::write(root.join("new"), "new").unwrap();
        let mut tx = Transaction::new(&root).unwrap();
        tx.replace(Some(root.join("new")), a.clone()).unwrap();
        let backups = tx.backup_directory().unwrap().to_owned();
        tx.commit().unwrap();
        Transaction::recover(&root).unwrap();
        assert_eq!(fs::read_to_string(a).unwrap(), "new");
        assert_eq!(fs::read_to_string(backups.join("0")).unwrap(), "old a");
        assert!(!root.join("update-journal.json").exists());
    }

    #[test]
    fn interrupted_agent_link_replacement_restores_the_original_link() {
        let (_dir, root) = scratch();
        let link = root.join("agent.toml");
        let replacement = root.join("replacement");
        std::os::unix::fs::symlink("old-target", &link).unwrap();
        std::os::unix::fs::symlink("new-target", &replacement).unwrap();
        let mut tx = Transaction::new(&root).unwrap();
        tx.replace(Some(replacement), link.clone()).unwrap();
        drop(tx);
        Transaction::recover(&root).unwrap();
        assert_eq!(fs::read_link(link).unwrap(), Path::new("old-target"));
    }

    #[test]
    fn backups_live_under_state_with_a_manifest_and_never_beside_destinations() {
        let (_dir, root) = scratch();
        let state = root.join("state");
        let skills = root.join("skills");
        let skill = skills.join("prove");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "old skill").unwrap();
        std::os::unix::fs::symlink("/nonexistent/target", skills.join("link")).unwrap();
        let file = skills.join("agent.md");
        fs::write(&file, "old agent").unwrap();
        let staged_tree = root.join("staged-tree");
        fs::create_dir(&staged_tree).unwrap();
        fs::write(staged_tree.join("SKILL.md"), "new skill").unwrap();
        let staged_file = root.join("staged-file");
        fs::write(&staged_file, "new agent").unwrap();
        let mut tx = Transaction::new(&state).unwrap();
        tx.replace(Some(staged_tree), skill.clone()).unwrap();
        tx.replace(Some(staged_file), file.clone()).unwrap();
        tx.replace(None, skills.join("link")).unwrap();
        tx.replace(None, skills.join("never-existed")).unwrap();
        tx.commit().unwrap();
        // Only the managed names remain in the host directory.
        assert_eq!(names(&skills), ["agent.md", "prove"]);
        assert_eq!(names(&skill), ["SKILL.md"]);
        let dirs = backup_dirs(&state);
        assert_eq!(dirs.len(), 1);
        let name = dirs[0].file_name().unwrap().to_string_lossy().into_owned();
        let (stamp, pid) = name.split_once('-').unwrap();
        assert_eq!(pid, std::process::id().to_string());
        assert!(stamp.len() == 15 && stamp.as_bytes()[8] == b'T', "{stamp}");
        let backup = &dirs[0];
        assert_eq!(
            fs::read_to_string(backup.join("0/SKILL.md")).unwrap(),
            "old skill"
        );
        assert_eq!(fs::read_to_string(backup.join("1")).unwrap(), "old agent");
        assert_eq!(
            fs::read_link(backup.join("2")).unwrap(),
            Path::new("/nonexistent/target")
        );
        assert_eq!(
            fs::read_to_string(backup.join("manifest.tsv")).unwrap(),
            format!(
                "{}\t{}\n{}\t{}\n{}\t{}\n",
                skill.display(),
                backup.join("0").display(),
                file.display(),
                backup.join("1").display(),
                skills.join("link").display(),
                backup.join("2").display(),
            )
        );
    }

    #[test]
    fn rollback_restores_trees_from_backups_and_keeps_the_backups() {
        let (_dir, root) = scratch();
        let state = root.join("state");
        let tree = root.join("tree");
        fs::create_dir(&tree).unwrap();
        fs::write(tree.join("a"), "old").unwrap();
        let staged = root.join("staged");
        fs::create_dir(&staged).unwrap();
        fs::write(staged.join("b"), "new").unwrap();
        let mut tx = Transaction::new(&state).unwrap();
        tx.replace(Some(staged), tree.clone()).unwrap();
        let backups = tx.backup_directory().unwrap().to_owned();
        drop(tx);
        Transaction::recover(&state).unwrap();
        assert_eq!(names(&tree), ["a"]);
        assert_eq!(fs::read_to_string(tree.join("a")).unwrap(), "old");
        assert_eq!(fs::read_to_string(backups.join("0/a")).unwrap(), "old");
        assert_eq!(names(&root), ["state", "tree"]);
    }

    #[test]
    fn legacy_sibling_backups_from_older_journals_still_recover() {
        let (_dir, root) = scratch();
        let dest = root.join("installed");
        let backup = root.join(".leanck-backup-1-0");
        fs::write(&dest, "new").unwrap();
        fs::write(&backup, "old").unwrap();
        fs::write(
            root.join("update-journal.json"),
            serde_json::json!({
                "schema": 1,
                "committed": false,
                "entries": [{"destination": dest, "backup": backup, "existed": true, "staged": null}],
            })
            .to_string(),
        )
        .unwrap();
        Transaction::recover(&root).unwrap();
        assert_eq!(fs::read_to_string(&dest).unwrap(), "old");
        assert!(!backup.exists());
    }

    #[test]
    fn journals_naming_foreign_backup_paths_are_refused() {
        let (_dir, root) = scratch();
        let dest = root.join("installed");
        let foreign = root.join("elsewhere/backup");
        fs::create_dir_all(foreign.parent().unwrap()).unwrap();
        fs::write(&dest, "new").unwrap();
        fs::write(&foreign, "old").unwrap();
        fs::write(
            root.join("update-journal.json"),
            serde_json::json!({
                "schema": 1,
                "committed": false,
                "entries": [{"destination": dest, "backup": foreign, "existed": true, "staged": null}],
            })
            .to_string(),
        )
        .unwrap();
        assert!(Transaction::recover(&root).is_err());
        assert_eq!(fs::read_to_string(&dest).unwrap(), "new");
    }
}
