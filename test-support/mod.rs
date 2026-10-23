//! Shared, test-only live-project ownership. Never query or mutate the template.
#![allow(dead_code)] // Each integration suite uses a different subset.

use std::io::Write;
use std::path::Path;
use std::time::{Duration, Instant};

use leanck_client::{Client, RequestOptions};
use serde_json::Value;

pub const CLEAN: &str = "TestProject/ContractClean.lean";
pub const WARNINGS: &str = "TestProject/Contract/Warnings.lean";

pub fn fixture(name: &str) -> Value {
    serde_json::from_str::<Value>(include_str!("contracts/v4.json")).unwrap()[name].clone()
}

#[path = "lean_project.rs"]
mod lean_project;
pub use lean_project::LeanProject;

pub struct TestProject {
    project: LeanProject,
    lean_pid: Option<u32>,
}

impl TestProject {
    pub fn new() -> Self {
        Self {
            project: LeanProject::new(),
            lean_pid: None,
        }
    }

    pub fn path(&self) -> &Path {
        self.project.path()
    }

    pub fn track_lean(&mut self, pid: Option<u32>) {
        self.lean_pid = pid;
    }

    pub async fn shutdown_daemon(&mut self) {
        let runtime = leanck_core::paths::RuntimePaths::for_workspace(self.path());
        let socket = runtime.socket();
        let client = Client::from_path(self.path()).unwrap();
        tokio::time::timeout(
            Duration::from_secs(10),
            client.shutdown_existing(RequestOptions::default()),
        )
        .await
        .expect("shutdown bounded")
        .expect("shutdown acknowledged");
        let deadline = Instant::now() + Duration::from_secs(10);
        while socket.exists() || runtime.pid().exists() || !lock_released(&runtime.lock()) {
            assert!(
                Instant::now() < deadline,
                "daemon must remove socket/pid and release its stable lock"
            );
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.lean_pid = None;
    }
}

impl Drop for TestProject {
    fn drop(&mut self) {
        // Panic fallback: a synchronous, bounded shutdown also works while the
        // async test is unwinding. Only this uniquely owned workspace is touched.
        let runtime = leanck_core::paths::RuntimePaths::for_workspace(self.path());
        let socket = runtime.socket();
        let pid_file = runtime.pid();
        if let Ok(mut stream) = std::os::unix::net::UnixStream::connect(&socket) {
            let _ = stream.set_write_timeout(Some(Duration::from_millis(500)));
            let body = br#"{"id":"test-cleanup","op":"shutdown","params":{}}"#;
            let _ = stream.write_all(&(body.len() as u32).to_be_bytes());
            let _ = stream.write_all(body);
        }
        // Do not block an in-process daemon's executor during unwinding. If
        // graceful cleanup hasn't completed, terminate only known owned children.
        if let Some(pid) = self.lean_pid {
            terminate(pid);
        }
        if let Ok(contents) = std::fs::read_to_string(&pid_file)
            && let Ok(pid) = contents.trim().parse::<u32>()
            && pid > 1
            && pid != std::process::id()
        {
            // The auto-spawn client gives its daemon a separate process group.
            // Terminate that owned group, including children still warming up.
            let _ = std::process::Command::new("kill")
                .args(["-TERM", "--", &format!("-{pid}")])
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
            terminate(pid);
        }
        for path in [socket, pid_file, runtime.lock(), runtime.log()] {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn terminate(pid: u32) {
    if pid > 1 {
        let _ = std::process::Command::new("kill")
            .args(["-TERM", &pid.to_string()])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
    }
}

fn lock_released(path: &Path) -> bool {
    std::fs::File::open(path).is_ok_and(|file| file.try_lock().is_ok())
}
