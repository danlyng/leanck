use crate::{atomic_json, safe_path};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, File},
    path::{Path, PathBuf},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
struct Entry {
    destination: PathBuf,
    backup: PathBuf,
    existed: bool,
    staged: Option<PathBuf>,
}
#[derive(Debug, Serialize, Deserialize)]
struct Journal {
    schema: u32,
    committed: bool,
    entries: Vec<Entry>,
}

/// The journal is durable before each mutation, including before moving the old path.
pub(crate) struct Transaction {
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
        let mut tx = Self { path, journal };
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
            path: state.join("update-journal.json"),
            journal: Journal {
                schema: 1,
                committed: false,
                entries: vec![],
            },
        };
        atomic_json(&tx.path, &tx.journal)?;
        Ok(tx)
    }
    pub fn replace(&mut self, staged: Option<PathBuf>, destination: PathBuf) -> Result<()> {
        safe_path(destination.parent().context("destination parent missing")?)?;
        fs::create_dir_all(destination.parent().unwrap())?;
        let backup = destination.with_file_name(format!(
            ".leanck-backup-{}-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos(),
            self.journal.entries.len()
        ));
        ensure!(
            !backup.try_exists()?,
            "backup already exists: {}",
            backup.display()
        );
        let existed = fs::symlink_metadata(&destination).is_ok();
        self.journal.entries.push(Entry {
            destination: destination.clone(),
            backup: backup.clone(),
            existed,
            staged: staged.clone(),
        });
        atomic_json(&self.path, &self.journal)?;
        if existed {
            let meta = fs::symlink_metadata(&destination)?;
            if meta.is_dir() {
                fs::rename(&destination, &backup)?;
            } else if meta.file_type().is_symlink() {
                #[cfg(unix)]
                std::os::unix::fs::symlink(fs::read_link(&destination)?, &backup)?;
            } else {
                // Keep the executable at its canonical path until the single
                // atomic replacement. A crash must leave an executable that
                // can run recovery, never a gap between two renames.
                fs::hard_link(&destination, &backup)?;
            }
            File::open(destination.parent().unwrap())?.sync_all()?;
        }
        if let Some(staged) = staged {
            fs::rename(staged, &destination)?;
        } else if existed {
            remove(&destination)?;
        }
        File::open(destination.parent().unwrap())?.sync_all()?;
        Ok(())
    }
    pub fn commit(&mut self) -> Result<()> {
        self.journal.committed = true;
        atomic_json(&self.path, &self.journal)?;
        self.cleanup()
    }
    fn cleanup(&mut self) -> Result<()> {
        // Backups remain next to original destinations; retain the manifest for repair.
        let manifest = self.path.with_file_name(format!(
            "update-backups-{}.json",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_nanos()
        ));
        atomic_json(&manifest, &self.journal)?;
        fs::remove_file(&self.path)?;
        Ok(())
    }
    pub fn rollback(&mut self) -> Result<()> {
        for entry in self.journal.entries.iter().rev() {
            safe_path(
                entry
                    .destination
                    .parent()
                    .context("journal parent missing")?,
            )?;
            ensure!(
                entry.backup.parent() == entry.destination.parent()
                    && entry
                        .backup
                        .file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with(".leanck-backup-")),
                "invalid backup path in recovery journal"
            );
            safe_path(entry.backup.parent().context("backup parent missing")?)?;
            if fs::symlink_metadata(&entry.backup).is_ok() {
                remove(&entry.destination)?;
                fs::rename(&entry.backup, &entry.destination)?;
            } else if !entry.existed {
                remove(&entry.destination)?;
            }
            File::open(entry.destination.parent().unwrap())?.sync_all()?;
        }
        fs::remove_file(&self.path)?;
        File::open(self.path.parent().unwrap())?.sync_all()?;
        Ok(())
    }
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
        tx.commit().unwrap();
        Transaction::recover(&root).unwrap();
        assert_eq!(fs::read_to_string(a).unwrap(), "new");
        assert!(fs::read_dir(&root).unwrap().any(|e| {
            e.unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("update-backups-")
        }));
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
}
