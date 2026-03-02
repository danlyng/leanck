//! The per-worktree engine: owns the warm Lean server, the elaboration state
//! store, the open-document registry, and the concurrency primitives. All query
//! handlers run against a `Workspace`.

use std::collections::{HashMap, HashSet};
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime};

use serde_json::json;
use tokio::sync::{Mutex as AsyncMutex, OwnedSemaphorePermit, Semaphore};

use crate::error::{Error, Result};
use crate::server::lifecycle::{path_to_uri, LeanServer};
use crate::server::JsonRpc;
use crate::state::StateStore;

/// Default deadline for an elaboration-gated wait (diagnostics/list).
pub const DEFAULT_ELAB_TIMEOUT: Duration = Duration::from_secs(120);
/// Default deadline for a direct LSP request that runs after elaboration settled.
pub const DEFAULT_LSP_TIMEOUT: Duration = Duration::from_secs(30);
/// Cap on concurrent elaboration-blocking requests to the single server.
const MAX_INFLIGHT: usize = 8;
/// How long to wait for an in-flight permit before reporting `Overloaded`.
const PERMIT_TIMEOUT: Duration = Duration::from_secs(30);

struct DocEntry {
    /// Monotonic LSP document version. Never resets, even across respawns, so a
    /// re-`didOpen` after a server restart can't look stale to the state machine.
    version: i64,
    /// Hash of the last content synced to the server (skip no-op resyncs).
    hash: u64,
    /// When we last (re)opened this file, i.e. when its worker last (re)built its
    /// imports. A first-party import whose source mtime is newer than this means
    /// the open worker is elaborating against stale dependencies (see
    /// [`Workspace::imports_stale_since`]).
    opened_at: SystemTime,
}

/// Per-worktree engine. Cheap to share via `Arc`.
pub struct Workspace {
    worktree: PathBuf,
    state: Arc<StateStore>,
    server: RwLock<Arc<LeanServer>>,
    docs: Mutex<HashMap<String, DocEntry>>,
    ensure_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    restart_locks: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    inflight: Arc<Semaphore>,
    /// Wall-clock millis of the last serviced request, for the idle reaper.
    last_activity_ms: AtomicU64,
    shutting_down: tokio::sync::Notify,
}

impl Workspace {
    /// Spawn the Lean server and build the engine. Starts the supervision task.
    pub async fn start(worktree: &Path) -> Result<Arc<Workspace>> {
        // Fetch the prebuilt Mathlib cache before the first `lake serve`, so a
        // fresh worktree doesn't build Mathlib from source. Best-effort.
        crate::cache::ensure_mathlib_cache(worktree).await;
        let state = Arc::new(StateStore::new());
        let server = LeanServer::spawn(worktree, state.clone()).await?;
        let ws = Arc::new(Workspace {
            worktree: worktree.to_path_buf(),
            state,
            server: RwLock::new(server),
            docs: Mutex::new(HashMap::new()),
            ensure_locks: Mutex::new(HashMap::new()),
            restart_locks: Mutex::new(HashMap::new()),
            inflight: Arc::new(Semaphore::new(MAX_INFLIGHT)),
            last_activity_ms: AtomicU64::new(now_ms()),
            shutting_down: tokio::sync::Notify::new(),
        });
        tokio::spawn(supervise(ws.clone()));
        Ok(ws)
    }

    pub fn worktree(&self) -> &Path {
        &self.worktree
    }

    pub fn state(&self) -> &Arc<StateStore> {
        &self.state
    }

    /// The current Lean server (cloned `Arc`, so callers don't hold the lock).
    pub fn server(&self) -> Arc<LeanServer> {
        self.server.read().unwrap().clone()
    }

    /// The current JSON-RPC handle.
    pub fn rpc(&self) -> Arc<JsonRpc> {
        self.server().rpc().clone()
    }

    pub fn default_elab_timeout(&self) -> Duration {
        DEFAULT_ELAB_TIMEOUT
    }

    pub fn default_lsp_timeout(&self) -> Duration {
        DEFAULT_LSP_TIMEOUT
    }

    pub fn mark_active(&self) {
        self.last_activity_ms.store(now_ms(), Ordering::Relaxed);
    }

    pub fn idle_for(&self) -> Duration {
        Duration::from_millis(now_ms().saturating_sub(self.last_activity_ms.load(Ordering::Relaxed)))
    }

    pub fn pid(&self) -> Option<u32> {
        self.server().pid()
    }

    /// Acquire an in-flight permit (backpressure). Bounded so a flood of agents
    /// gets `Overloaded` instead of wedging the server.
    pub async fn acquire_inflight(&self) -> Result<OwnedSemaphorePermit> {
        match tokio::time::timeout(PERMIT_TIMEOUT, self.inflight.clone().acquire_owned()).await {
            Ok(Ok(permit)) => Ok(permit),
            Ok(Err(_)) => Err(Error::ServerDown("semaphore closed".into())),
            Err(_) => Err(Error::Overloaded),
        }
    }

    /// Begin shutdown: stop supervision and kill the server.
    pub async fn shutdown(&self) {
        self.shutting_down.notify_waiters();
        self.server().shutdown().await;
    }

    // ---- document sync ---------------------------------------------------

    /// Ensure `uri` is open in the server and in sync with its content on disk.
    /// Full-document sync; a content-hash check skips a no-op resync. Serialized
    /// per-URI so concurrent callers can't double-open or race versions.
    pub async fn sync_from_disk(&self, abs_path: &Path) -> Result<String> {
        let uri = path_to_uri(abs_path);
        let lock = self.lock_for(&self.ensure_locks, &uri);
        let _g = lock.lock().await;

        let content = tokio::fs::read_to_string(abs_path)
            .await
            .map_err(|e| Error::InvalidPath(format!("{}: {e}", abs_path.display())))?;
        let hash = hash_str(&content);

        let existing = {
            let docs = self.docs.lock().unwrap();
            docs.get(&uri).map(|e| (e.version, e.hash, e.opened_at))
        };

        match existing {
            Some((_, h, opened_at)) if h == hash => {
                // Content unchanged, so the open worker's elaboration is still
                // valid — *unless* a first-party dependency changed on disk since
                // we opened it. Lake's LSP serves the file from its already-loaded
                // (now stale) import environment, so a plain resync would return
                // cached diagnostics and miss the breakage. Rebuild its imports.
                if self.imports_stale_since(abs_path, opened_at) {
                    self.reopen_once(&uri, &content, hash)?;
                }
            }
            Some((version, _, _)) => {
                let next = version + 1;
                self.rpc().send_notification(
                    "textDocument/didChange",
                    json!({
                        "textDocument": { "uri": uri, "version": next },
                        "contentChanges": [{ "text": content }],
                    }),
                )?;
                self.docs
                    .lock()
                    .unwrap()
                    .insert(uri.clone(), DocEntry { version: next, hash, opened_at: SystemTime::now() });
                self.state.observe_doc_version(&uri, next);
            }
            None => {
                self.open_doc(&uri, &content, 1, None)?;
                self.docs
                    .lock()
                    .unwrap()
                    .insert(uri.clone(), DocEntry { version: 1, hash, opened_at: SystemTime::now() });
                self.state.touch(&uri);
                self.state.observe_doc_version(&uri, 1);
            }
        }
        Ok(uri)
    }

    /// Restart a file's worker the lightweight LSP way: `didClose` then `didOpen`
    /// with `dependencyBuildMode: "once"` (rebuilds stale/missing dependencies
    /// through the warm server — no `lake build`). Serialized per-URI.
    pub async fn restart_file(&self, abs_path: &Path) -> Result<String> {
        let uri = path_to_uri(abs_path);
        let lock = self.lock_for(&self.restart_locks, &uri);
        let _g = lock.lock().await;

        let content = tokio::fs::read_to_string(abs_path)
            .await
            .map_err(|e| Error::InvalidPath(format!("{}: {e}", abs_path.display())))?;
        let hash = hash_str(&content);
        self.reopen_once(&uri, &content, hash)?;
        Ok(uri)
    }

    /// Re-`didOpen` `uri` at the next version with `dependencyBuildMode: "once"`
    /// (after a best-effort `didClose`), so the worker rebuilds stale/missing
    /// dependencies. Updates the doc registry and wait-gate bookkeeping. Backs
    /// both [`Workspace::restart_file`] and the stale-dependency path in
    /// [`Workspace::sync_from_disk`].
    fn reopen_once(&self, uri: &str, content: &str, hash: u64) -> Result<()> {
        let prev_version = self.docs.lock().unwrap().get(uri).map(|e| e.version).unwrap_or(0);
        let next = prev_version + 1;

        let rpc = self.rpc();
        // Best-effort close (ignore if it was never opened).
        let _ = rpc.send_notification(
            "textDocument/didClose",
            json!({ "textDocument": { "uri": uri } }),
        );
        self.open_doc(uri, content, next, Some("once"))?;
        self.docs
            .lock()
            .unwrap()
            .insert(uri.to_string(), DocEntry { version: next, hash, opened_at: SystemTime::now() });
        self.state.touch(uri);
        self.state.observe_doc_version(uri, next);
        Ok(())
    }

    /// Whether any first-party source file transitively imported by `abs_path`
    /// has been modified since `since` (i.e. since the worker last built its
    /// imports). "First-party" means the module resolves to an existing `.lean`
    /// under the worktree (`A.B.C` → `<worktree>/A/B/C.lean`); prebuilt
    /// dependencies — Mathlib, batteries, the toolchain — resolve elsewhere and
    /// are skipped, since their oleans don't change mid-session. Best-effort and
    /// bounded: unreadable files and unresolved imports are treated as "fresh".
    fn imports_stale_since(&self, abs_path: &Path, since: SystemTime) -> bool {
        const MAX_FILES: usize = 4096;
        let mut seen: HashSet<PathBuf> = HashSet::new();
        let mut stack: Vec<PathBuf> = vec![abs_path.to_path_buf()];
        while let Some(path) = stack.pop() {
            if !seen.insert(path.clone()) || seen.len() > MAX_FILES {
                continue;
            }
            let Ok(source) = std::fs::read_to_string(&path) else {
                continue;
            };
            for module in parse_imports(&source) {
                let candidate = self.worktree.join(module_to_rel_path(&module));
                // Only first-party dependencies (resolving under the worktree)
                // can change during a session; skip everything else.
                if candidate == *abs_path || !candidate.is_file() {
                    continue;
                }
                if file_modified_after(&candidate, since) {
                    return true;
                }
                stack.push(candidate);
            }
        }
        false
    }

    /// The last LSP document version we synced for `uri` (0 if never opened).
    pub fn doc_version(&self, uri: &str) -> i64 {
        self.docs.lock().unwrap().get(uri).map(|e| e.version).unwrap_or(0)
    }

    /// Ask the server to confirm that every diagnostic for `uri` at or past
    /// `version` has been emitted (`textDocument/waitForDiagnostics`). The
    /// fileProgress gate alone is not authoritative: Lean can clear progress
    /// while proofs still elaborate in background tasks, and the final
    /// `publishDiagnostics` can land after the final progress notification.
    pub async fn wait_for_diagnostics(&self, uri: &str, version: i64, timeout: Duration) -> Result<()> {
        self.rpc()
            .request_simple(
                "textDocument/waitForDiagnostics",
                json!({ "uri": uri, "version": version }),
                timeout,
            )
            .await
            .map(|_| ())
    }

    fn open_doc(&self, uri: &str, content: &str, version: i64, dep_build_mode: Option<&str>) -> Result<()> {
        let mut params = json!({
            "textDocument": {
                "uri": uri,
                "languageId": "lean",
                "version": version,
                "text": content,
            },
        });
        if let Some(mode) = dep_build_mode {
            params["dependencyBuildMode"] = json!(mode);
        }
        self.rpc().send_notification("textDocument/didOpen", params)
    }

    /// Re-open every tracked document after a server respawn, continuing each
    /// URI's monotonic version so the new elaboration isn't seen as stale.
    async fn reopen_all(&self) {
        let uris: Vec<String> = self.docs.lock().unwrap().keys().cloned().collect();
        for uri in uris {
            let Some(path) = uri_to_path(&uri) else { continue };
            let Ok(content) = tokio::fs::read_to_string(&path).await else { continue };
            let hash = hash_str(&content);
            let next = self.docs.lock().unwrap().get(&uri).map(|e| e.version).unwrap_or(0) + 1;
            if self.open_doc(&uri, &content, next, Some("once")).is_ok() {
                self.docs.lock().unwrap().insert(uri.clone(), DocEntry { version: next, hash, opened_at: SystemTime::now() });
                self.state.observe_doc_version(&uri, next);
            }
        }
    }

    fn lock_for(&self, map: &Mutex<HashMap<String, Arc<AsyncMutex<()>>>>, uri: &str) -> Arc<AsyncMutex<()>> {
        map.lock()
            .unwrap()
            .entry(uri.to_string())
            .or_insert_with(|| Arc::new(AsyncMutex::new(())))
            .clone()
    }
}

/// Supervision loop: when the server dies (stdout EOF), respawn it, re-initialize,
/// and re-open tracked docs. Per-request timeouts already prevent freezes; this
/// recovers the server so subsequent requests succeed.
async fn supervise(ws: Arc<Workspace>) {
    loop {
        let server = ws.server();
        let dead = server.rpc().dead_token();
        tokio::select! {
            _ = dead.cancelled() => {}
            _ = ws.shutting_down.notified() => return,
        }
        tracing::warn!(worktree = %ws.worktree.display(), "lean server died; respawning");

        loop {
            match LeanServer::spawn(&ws.worktree, ws.state.clone()).await {
                Ok(new) => {
                    *ws.server.write().unwrap() = new;
                    ws.reopen_all().await;
                    tracing::info!("lean server respawned");
                    break;
                }
                Err(e) => {
                    tracing::error!(error = %e, "respawn failed; retrying in 3s");
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(3)) => {}
                        _ = ws.shutting_down.notified() => return,
                    }
                }
            }
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn hash_str(s: &str) -> u64 {
    let mut h = std::collections::hash_map::DefaultHasher::new();
    s.hash(&mut h);
    h.finish()
}

/// Recover a filesystem path from a `file://` URI.
pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    uri.strip_prefix("file://").map(PathBuf::from)
}

/// Module names from a Lean file's `import` lines. Lean requires imports at the
/// top of the file, but a leading copyright/doc block comment can precede them,
/// so we scan the whole file for `import …` lines rather than stopping at the
/// first non-import line; a stray match inside a comment just resolves to a
/// non-existent path and is ignored downstream. Handles `import all Mod`.
fn parse_imports(source: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in source.lines() {
        let Some(rest) = line.trim_start().strip_prefix("import ") else {
            continue;
        };
        let mut tokens = rest.split_whitespace();
        let mut module = tokens.next().unwrap_or("");
        if module == "all" {
            module = tokens.next().unwrap_or("");
        }
        if !module.is_empty() && !module.starts_with("--") {
            out.push(module.to_string());
        }
    }
    out
}

/// Map a dotted Lean module name to its source path relative to a library root
/// (`A.B.C` → `A/B/C.lean`).
fn module_to_rel_path(module: &str) -> PathBuf {
    let mut rel = PathBuf::new();
    for seg in module.split('.') {
        rel.push(seg);
    }
    rel.set_extension("lean");
    rel
}

/// Whether `path`'s mtime is strictly after `since`. Unreadable → `false`.
fn file_modified_after(path: &Path, since: SystemTime) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|mtime| mtime > since)
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uri_path_round_trip() {
        let p = Path::new("/x/y.lean");
        let uri = path_to_uri(p);
        assert_eq!(uri_to_path(&uri).unwrap(), p);
    }

    #[test]
    fn hash_detects_change() {
        assert_eq!(hash_str("a"), hash_str("a"));
        assert_ne!(hash_str("a"), hash_str("b"));
    }

    #[test]
    fn parses_import_header() {
        let src = "/- copyright -/\nimport TestProject.Dep\nimport all Mathlib.Tactic\n\n-- import Commented.Out\ntheorem t : True := by\n  import_oops";
        assert_eq!(
            parse_imports(src),
            vec!["TestProject.Dep".to_string(), "Mathlib.Tactic".to_string()]
        );
    }

    #[test]
    fn module_path_is_dotted_to_slashed() {
        assert_eq!(module_to_rel_path("A.B.C"), PathBuf::from("A/B/C.lean"));
        assert_eq!(module_to_rel_path("Solo"), PathBuf::from("Solo.lean"));
    }
}
