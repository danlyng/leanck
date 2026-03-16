//! Stable installation locks. Shared usage leases never unlink their lock files.
use crate::{lock_file, receipt::Receipt};
use anyhow::{Result, bail};
use std::{fs::File, path::Path};

pub struct Lease {
    _file: File,
}
impl Lease {
    pub fn acquire(receipt: &Receipt) -> Result<Self> {
        let file = lock_file(&receipt.state_dir.join("maintenance.lock"))?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
        let mut waiting = false;
        loop {
            match file.try_lock_shared() {
                Ok(()) => break,
                Err(std::fs::TryLockError::WouldBlock) => {
                    anyhow::ensure!(
                        std::time::Instant::now() < deadline,
                        "installation maintenance is still running; retry after it finishes"
                    );
                    if !waiting {
                        eprintln!("Waiting for leanck installation maintenance…");
                        waiting = true;
                    }
                    std::thread::sleep(std::time::Duration::from_millis(25));
                }
                Err(e) => return Err(e.into()),
            }
        }
        anyhow::ensure!(
            !receipt.state_dir.join("update-journal.json").try_exists()?,
            "an interrupted update needs recovery; run `leanck update` before using this installation"
        );
        Ok(Self { _file: file })
    }
}
pub struct Maintenance {
    _file: File,
}
impl Maintenance {
    pub fn acquire(state: &Path) -> Result<Self> {
        let file = lock_file(&state.join("maintenance.lock"))?;
        match file.try_lock() {
            Ok(()) => Ok(Self { _file: file }),
            Err(std::fs::TryLockError::WouldBlock) => bail!(
                "installation is in use; close leanck clients, stop its daemons, and restart MCP hosts before updating"
            ),
            Err(e) => Err(e.into()),
        }
    }
}

/// Detect older processes that predate usage leases. PIDs are checked against
/// their start identity both before and after inspecting the executable path.
pub fn ensure_no_legacy_users(binary: &Path) -> Result<()> {
    let wanted = binary.canonicalize().unwrap_or_else(|_| binary.to_owned());
    for pid in process_ids()? {
        if pid == std::process::id() {
            continue;
        }
        let before = leanck_core::instances::process_start_id(pid);
        if let Some(path) = process_path(pid) {
            let path = path.canonicalize().unwrap_or(path);
            if path == wanted
                && before.is_some()
                && before == leanck_core::instances::process_start_id(pid)
            {
                bail!(
                    "leanck process {pid} is still using {}; close its session before updating",
                    binary.display()
                );
            }
        }
    }
    Ok(())
}

#[cfg(target_os = "linux")]
fn process_ids() -> Result<Vec<u32>> {
    Ok(std::fs::read_dir("/proc")?
        .filter_map(|e| e.ok()?.file_name().to_str()?.parse().ok())
        .collect())
}
#[cfg(target_os = "linux")]
fn process_path(pid: u32) -> Option<std::path::PathBuf> {
    std::fs::read_link(format!("/proc/{pid}/exe")).ok()
}
#[cfg(target_os = "macos")]
fn process_ids() -> Result<Vec<u32>> {
    let mut pids = vec![0_u32; 65536];
    // SAFETY: the supplied buffer has exactly the advertised writable byte capacity.
    // PROC_ALL_PIDS from macOS libproc.h; libc exposes the function but not this constant.
    let bytes =
        unsafe { libc::proc_listpids(1, 0, pids.as_mut_ptr().cast(), (pids.len() * 4) as i32) };
    anyhow::ensure!(
        bytes >= 0 && (bytes as usize) < pids.len() * 4,
        "cannot enumerate processes safely"
    );
    pids.truncate(bytes as usize / 4);
    Ok(pids)
}
#[cfg(target_os = "macos")]
fn process_path(pid: u32) -> Option<std::path::PathBuf> {
    use std::os::unix::ffi::OsStringExt;
    let mut bytes = vec![0_u8; 4096];
    // SAFETY: proc_pidpath writes at most the supplied buffer length.
    let n =
        unsafe { libc::proc_pidpath(pid as i32, bytes.as_mut_ptr().cast(), bytes.len() as u32) };
    if n <= 0 {
        return None;
    }
    bytes.truncate(bytes.iter().position(|&b| b == 0).unwrap_or(n as usize));
    Some(std::ffi::OsString::from_vec(bytes).into())
}
