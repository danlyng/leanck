//! Spawn and initialize one warm Lean server (`lake serve`) for a worktree, and
//! route its notifications into the [`ElaborationStore`].

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use leanck_lake::ProcessGroup;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

use super::session::LeanSession;
use crate::error::{Error, Result};
use crate::request::RequestContext;
use crate::state::ElaborationStore;

/// How long to wait for the `initialize` response.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);

/// One running, initialized Lean server for a worktree.
pub struct LeanServer {
    session: Arc<LeanSession>,
    child: Mutex<Child>,
    pid: Option<u32>,
    worktree: PathBuf,
    group: ProcessGroup,
    stderr: std::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
    stopped: Mutex<bool>,
}

impl LeanServer {
    /// Spawn `lake serve --` (or `lean --server` for a non-Lake file tree) in
    /// `worktree`, perform the LSP initialize handshake, and wire notifications
    /// into `state`.
    pub async fn spawn_with_cancel(
        worktree: &Path,
        state: Arc<ElaborationStore>,
        cancel: &CancellationToken,
    ) -> Result<Arc<LeanServer>> {
        if cancel.is_cancelled() {
            return Err(Error::Cancelled);
        }
        let (program, args) = launch_command(worktree);
        tracing::info!(worktree = %worktree.display(), %program, ?args, "spawning lean server");

        let mut child = Command::new(&program)
            .args(&args)
            .current_dir(worktree)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .process_group(0)
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::Spawn(format!("{program}: {e}")))?;

        let group = ProcessGroup::from_child(&child);
        let pid = child.id();
        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| Error::Spawn("no stdin".into()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| Error::Spawn("no stdout".into()))?;
        let stderr = child
            .stderr
            .take()
            .map(|stderr| tokio::spawn(drain_stderr(stderr)));

        let session = LeanSession::spawn(stdout, stdin, state);

        let server = Arc::new(LeanServer {
            session,
            child: Mutex::new(child),
            pid,
            worktree: worktree.to_path_buf(),
            group,
            stderr: std::sync::Mutex::new(stderr),
            stopped: Mutex::new(false),
        });

        if let Err(error) = server.initialize(cancel).await {
            server.force_stop().await;
            return Err(error);
        }
        Ok(server)
    }

    async fn initialize(&self, cancel: &CancellationToken) -> Result<()> {
        let ctx = RequestContext::new(Some(INITIALIZE_TIMEOUT.as_millis() as u64), cancel.clone())?;
        self.session
            .initialize(&self.worktree, &ctx)
            .await
            .map(|_| ())
    }

    pub fn session(&self) -> &Arc<LeanSession> {
        &self.session
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn is_dead(&self) -> bool {
        self.session.is_dead()
    }

    /// Cooperative shutdown: ask the server to exit, then ensure it's killed.
    pub async fn shutdown(&self) {
        let mut stopped = self.stopped.lock().await;
        if *stopped {
            return;
        }
        if let Ok(ctx) = RequestContext::new(Some(2000), CancellationToken::new()) {
            let _ = self.session.cooperative_shutdown(&ctx).await;
        }
        self.force_stop().await;
        *stopped = true;
    }
    async fn force_stop(&self) {
        self.group.terminate();
        self.session.shutdown().await;
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
        let stderr = self.stderr.lock().unwrap().take();
        if let Some(task) = stderr {
            task.abort();
            let _ = task.await;
        }
    }
}
impl Drop for LeanServer {
    fn drop(&mut self) {
        self.group.terminate();
        if let Some(task) = self.stderr.get_mut().unwrap().take() {
            task.abort();
        }
    }
}

/// Decide how to launch the server. Mirrors the lean4 extension's
/// `willUseLakeServer`: a Lake project (lakefile.lean / lakefile.toml) uses
/// `lake serve --`; otherwise fall back to `lean --server`.
fn launch_command(worktree: &Path) -> (String, Vec<String>) {
    let has_lakefile =
        worktree.join("lakefile.lean").exists() || worktree.join("lakefile.toml").exists();
    if has_lakefile {
        (
            "lake".to_string(),
            vec!["serve".to_string(), "--".to_string()],
        )
    } else {
        ("lean".to_string(), vec!["--server".to_string()])
    }
}

async fn drain_stderr(stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "lean_server_stderr", "{line}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lake_project_uses_lake_serve() {
        let dir = std::env::temp_dir().join(format!("leanckd-lc-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("lakefile.toml"), b"").unwrap();
        let (prog, args) = launch_command(&dir);
        assert_eq!(prog, "lake");
        assert_eq!(args, vec!["serve".to_string(), "--".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn bare_tree_uses_lean_server() {
        let dir = std::env::temp_dir().join(format!("leanckd-lc-bare-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let (prog, args) = launch_command(&dir);
        assert_eq!(prog, "lean");
        assert_eq!(args, vec!["--server".to_string()]);
        std::fs::remove_dir_all(&dir).ok();
    }
}
