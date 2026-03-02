//! Spawn and initialize one warm Lean server (`lake serve`) for a worktree, and
//! route its notifications into the [`StateStore`].

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

use super::jsonrpc::JsonRpc;
use crate::error::{Error, Result};
use crate::state::{StateStore, WireDiagnostic, WireRange};

/// How long to wait for the `initialize` response.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(60);

/// One running, initialized Lean server for a worktree.
pub struct LeanServer {
    rpc: Arc<JsonRpc>,
    child: Mutex<Child>,
    pid: Option<u32>,
    worktree: PathBuf,
}

impl LeanServer {
    /// Spawn `lake serve --` (or `lean --server` for a non-Lake file tree) in
    /// `worktree`, perform the LSP initialize handshake, and wire notifications
    /// into `state`.
    pub async fn spawn(worktree: &Path, state: Arc<StateStore>) -> Result<Arc<LeanServer>> {
        let (program, args) = launch_command(worktree);
        tracing::info!(worktree = %worktree.display(), %program, ?args, "spawning lean server");

        let mut child = Command::new(&program)
            .args(&args)
            .current_dir(worktree)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .map_err(|e| Error::Spawn(format!("{program}: {e}")))?;

        let pid = child.id();
        let stdin = child.stdin.take().ok_or_else(|| Error::Spawn("no stdin".into()))?;
        let stdout = child.stdout.take().ok_or_else(|| Error::Spawn("no stdout".into()))?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(drain_stderr(stderr));
        }

        let sink_state = state.clone();
        let rpc = JsonRpc::spawn(
            stdout,
            stdin,
            Arc::new(move |method, params| route_notification(&sink_state, &method, params)),
        );

        let server = Arc::new(LeanServer {
            rpc,
            child: Mutex::new(child),
            pid,
            worktree: worktree.to_path_buf(),
        });

        server.initialize().await?;
        Ok(server)
    }

    async fn initialize(&self) -> Result<()> {
        let root_uri = path_to_uri(&self.worktree);
        let params = json!({
            "processId": std::process::id(),
            "rootUri": root_uri,
            "rootPath": self.worktree.to_string_lossy(),
            "capabilities": client_capabilities(),
            // `hasWidgets` enables the interactive RPC surface. We deliberately
            // do NOT advertise incrementalDiagnosticSupport, so Lean publishes
            // full diagnostic sets (which the state machine assumes).
            "initializationOptions": { "hasWidgets": true },
            "workspaceFolders": [{
                "uri": root_uri,
                "name": self.worktree.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
            }],
        });
        self.rpc
            .request_simple("initialize", params, INITIALIZE_TIMEOUT)
            .await?;
        self.rpc.send_notification("initialized", json!({}))?;
        Ok(())
    }

    pub fn rpc(&self) -> &Arc<JsonRpc> {
        &self.rpc
    }

    pub fn pid(&self) -> Option<u32> {
        self.pid
    }

    pub fn is_dead(&self) -> bool {
        self.rpc.is_dead()
    }

    /// Cooperative shutdown: ask the server to exit, then ensure it's killed.
    pub async fn shutdown(&self) {
        let _ = self
            .rpc
            .request_simple("shutdown", json!(null), Duration::from_secs(2))
            .await;
        let _ = self.rpc.send_notification("exit", json!(null));
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
        let _ = child.wait().await;
    }
}

/// Decide how to launch the server. Mirrors the lean4 extension's
/// `willUseLakeServer`: a Lake project (lakefile.lean / lakefile.toml) uses
/// `lake serve --`; otherwise fall back to `lean --server`.
fn launch_command(worktree: &Path) -> (String, Vec<String>) {
    let has_lakefile =
        worktree.join("lakefile.lean").exists() || worktree.join("lakefile.toml").exists();
    if has_lakefile {
        ("lake".to_string(), vec!["serve".to_string(), "--".to_string()])
    } else {
        ("lean".to_string(), vec!["--server".to_string()])
    }
}

/// Route a server notification into the state store. Handles
/// `textDocument/publishDiagnostics` and `$/lean/fileProgress`; ignores the rest.
fn route_notification(state: &StateStore, method: &str, params: Value) {
    match method {
        "textDocument/publishDiagnostics" => {
            let Some(uri) = params.get("uri").and_then(|u| u.as_str()) else {
                return;
            };
            let version = params.get("version").and_then(|v| v.as_i64());
            let diags = params
                .get("diagnostics")
                .and_then(|d| d.as_array())
                .map(|arr| arr.iter().filter_map(WireDiagnostic::from_lsp).collect())
                .unwrap_or_default();
            state.apply_diagnostics(uri, version, diags);
        }
        "$/lean/fileProgress" => {
            let Some(td) = params.get("textDocument") else {
                return;
            };
            let Some(uri) = td.get("uri").and_then(|u| u.as_str()) else {
                return;
            };
            let version = td.get("version").and_then(|v| v.as_i64()).unwrap_or(0);
            let processing: Vec<WireRange> = params
                .get("processing")
                .and_then(|p| p.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|e| serde_json::from_value(e.get("range")?.clone()).ok())
                        .collect()
                })
                .unwrap_or_default();
            state.apply_file_progress(uri, version, processing);
        }
        _ => {}
    }
}

async fn drain_stderr(stderr: tokio::process::ChildStderr) {
    let mut lines = BufReader::new(stderr).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "lean_server_stderr", "{line}");
    }
}

/// Minimal client capabilities sufficient for diagnostics, goals, document
/// symbols (hierarchical tree), and folding ranges.
fn client_capabilities() -> Value {
    json!({
        "textDocument": {
            "synchronization": { "didSave": true, "dynamicRegistration": false },
            "publishDiagnostics": { "relatedInformation": true },
            "documentSymbol": { "hierarchicalDocumentSymbolSupport": true },
            "foldingRange": { "lineFoldingOnly": false },
            "hover": { "contentFormat": ["plaintext"] },
        },
        "window": { "workDoneProgress": true },
    })
}

/// `file://` URI for an absolute path (no percent-encoding of `/`). Matches the
/// form Lean echoes back, which the state store keys on.
pub fn path_to_uri(path: &Path) -> String {
    format!("file://{}", path.to_string_lossy())
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

    #[test]
    fn uri_round_trips_path() {
        assert_eq!(path_to_uri(Path::new("/a/b.lean")), "file:///a/b.lean");
    }
}
