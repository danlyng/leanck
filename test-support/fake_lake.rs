//! Owned workspace/toolchain fixture for frontend build process tests.
#![allow(dead_code)]
use std::{
    path::{Path, PathBuf},
    sync::atomic::{AtomicU64, Ordering},
};
pub struct Project(pub PathBuf);
impl Project {
    pub fn new(script: &str) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "lck-build-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        std::fs::create_dir(root.join("bin")).unwrap();
        std::fs::write(root.join("lakefile.toml"), "name = \"test\"\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        let lake = root.join("bin/lake");
        std::fs::write(&lake, format!("#!/bin/sh\n{script}\n")).unwrap();
        std::fs::set_permissions(lake, std::fs::Permissions::from_mode(0o700)).unwrap();
        Self(root)
    }
    pub fn path_env(&self) -> String {
        format!(
            "{}:{}",
            self.0.join("bin").display(),
            std::env::var("PATH").unwrap_or_default()
        )
    }
    pub fn assert_no_daemon(&self) {
        let paths = leanck_core::paths::RuntimePaths::for_workspace(&self.0);
        assert!(!paths.socket().exists());
        assert!(!paths.pid().exists());
    }
}
impl Drop for Project {
    fn drop(&mut self) {
        if let Ok(pid) = std::fs::read_to_string(self.0.join("lake.pid")) {
            let _ = std::process::Command::new("kill")
                .args(["-KILL", &format!("-{}", pid.trim())])
                .stderr(std::process::Stdio::null())
                .status();
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}
pub async fn wait_file(path: &Path) {
    tokio::time::timeout(std::time::Duration::from_secs(10), async {
        while !path.exists() {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}
pub fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}
