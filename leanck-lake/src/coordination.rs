//! Advisory coordination for leanck-initiated builds and cache fetches.
use crate::LakeError;
use std::{
    fs::{File, TryLockError},
    path::Path,
};

/// Keep the inode in place. Closing the descriptor releases ownership, including on crash.
pub(crate) async fn acquire(project: &Path) -> Result<File, LakeError> {
    let directory = project.join(".lake");
    let path = directory.join("leanck-execution.lock");
    tokio::fs::create_dir_all(&directory)
        .await
        .map_err(|source| LakeError::Coordination {
            path: path.clone(),
            source,
        })?;
    let file = tokio::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(&path)
        .await
        .map_err(|source| LakeError::Coordination {
            path: path.clone(),
            source,
        })?
        .into_std()
        .await;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) => {
                tokio::time::sleep(std::time::Duration::from_millis(25)).await
            }
            Err(TryLockError::Error(source)) => {
                return Err(LakeError::Coordination { path, source });
            }
        }
    }
}
