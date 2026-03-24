use crate::error::{ClientError, Result};
use std::path::PathBuf;

/// Resolve `leanck` beside the host executable, then on PATH. Never assume
/// an arbitrary host application or test harness supports `serve daemon`.
/// Explicit binary selection is supplied through `client.daemon_bin` in settings.
pub fn resolve_binary() -> Result<PathBuf> {
    resolve_binary_from(std::env::current_exe().ok(), std::env::var_os("PATH"))
}

fn resolve_binary_from(
    current_exe: Option<PathBuf>,
    search_path: Option<std::ffi::OsString>,
) -> Result<PathBuf> {
    if let Some(exe) = current_exe
        && let Some(dir) = exe.parent()
    {
        let sibling = dir.join("leanck");
        if sibling.is_file() {
            return Ok(sibling);
        }
    }
    // Fall back to a `leanck` found on PATH.
    if let Some(found) = find_on_path("leanck", search_path) {
        return Ok(found);
    }
    Err(ClientError::BinaryNotFound)
}

/// Locate an executable named `name` by scanning `$PATH`. Returns the first
/// match that exists, so the caller can fail early with a clear message rather
/// than letting `spawn()` surface a bare "No such file or directory".
fn find_on_path(name: &str, search_path: Option<std::ffi::OsString>) -> Option<PathBuf> {
    let path = search_path?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(name))
        .find(|candidate| candidate.is_file())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tests::Scratch;

    #[test]
    fn executable_discovery_keeps_sibling_path_order() {
        let scratch = Scratch::new();
        let sibling_dir = scratch.0.join("sibling");
        let path_dir = scratch.0.join("path");
        std::fs::create_dir(&sibling_dir).unwrap();
        std::fs::create_dir(&path_dir).unwrap();
        let sibling = sibling_dir.join("leanck");
        let path_bin = path_dir.join("leanck");
        for bin in [&sibling, &path_bin] {
            std::fs::write(bin, "").unwrap();
        }
        let current_exe = Some(sibling_dir.join("host-application"));
        let search_path = Some(path_dir.into_os_string());
        assert_eq!(
            resolve_binary_from(current_exe.clone(), search_path.clone()).unwrap(),
            sibling
        );
        std::fs::remove_file(&sibling).unwrap();
        assert_eq!(
            resolve_binary_from(current_exe.clone(), search_path.clone()).unwrap(),
            path_bin
        );
        // A library host is never assumed to support daemon mode itself.
        std::fs::write(current_exe.as_ref().unwrap(), "").unwrap();
        assert!(matches!(
            resolve_binary_from(current_exe, None),
            Err(ClientError::BinaryNotFound)
        ));
    }
}
