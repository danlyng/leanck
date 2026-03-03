//! The per-worktree engine: owns the warm Lean server, the elaboration state
//! store, the open-document registry, and the concurrency primitives. All query
//! handlers run against a `Workspace`.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use tokio_util::sync::CancellationToken;

use tokio::sync::{OwnedSemaphorePermit, Semaphore};

use crate::documents::{Document, DocumentRegistry, SessionView};
use crate::error::{Error, Result};
use crate::request::RequestContext;
use crate::server::LeanSession;
use crate::server::lifecycle::LeanServer;
use crate::state::ElaborationStore;

/// Per-worktree engine. Cheap to share via `Arc`.
pub struct Workspace {
    worktree: PathBuf,
    pub(crate) settings: leanck_config::Daemon,
    state: Arc<ElaborationStore>,
    server: RwLock<Arc<LeanServer>>,
    sessions: Arc<RwLock<SessionView>>,
    documents: DocumentRegistry,
    inflight: Arc<Semaphore>,
    activity: Arc<crate::activity::Activity>,
    shutting_down: CancellationToken,
    supervisor: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

impl Workspace {
    /// Spawn the Lean server and build the engine. Starts the supervision task.
    #[cfg(test)]
    pub async fn start(worktree: &Path) -> Result<Arc<Workspace>> {
        Self::start_with_cancel(worktree, CancellationToken::new()).await
    }

    #[cfg(test)]
    pub async fn start_with_cancel(
        worktree: &Path,
        cancel: CancellationToken,
    ) -> Result<Arc<Workspace>> {
        let settings = leanck_config::ConfigArgs::default()
            .load(worktree)
            .map_err(|e| Error::BadRequest(format!("configuration: {e:#}")))?
            .settings;
        Self::start_configured(worktree, cancel, &settings).await
    }
    pub async fn start_configured(
        worktree: &Path,
        cancel: CancellationToken,
        settings: &leanck_config::Settings,
    ) -> Result<Arc<Workspace>> {
        let root = leanck_core::workspace::WorkspaceRoot::from_path(worktree)
            .map_err(|e| Error::InvalidPath(e.to_string()))?;
        let worktree = root.as_path();
        // Fetch the prebuilt Mathlib cache before the first `lake serve`, so a
        // fresh worktree doesn't build Mathlib from source. Best-effort.
        tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(Error::Cancelled),
            _ = warm_mathlib_cache(worktree, &settings.cache) => {}
        }
        let state = Arc::new(ElaborationStore::new());
        let server = LeanServer::spawn_with_cancel(worktree, state.clone(), &cancel).await?;
        let roots = tokio::select! {biased;_=cancel.cancelled()=>return Err(Error::Cancelled), roots=leanck_lake::source::discover(worktree)=>roots};
        let sessions = Arc::new(RwLock::new(SessionView {
            generation: state.generation(),
            session: server.session().clone(),
        }));
        let documents = DocumentRegistry::new(sessions.clone(), state.clone(), roots)
            .with_limits(
                settings.daemon.max_documents,
                settings.daemon.max_source_bytes,
            )
            .with_lsp_timeout(Duration::from_millis(settings.daemon.lsp_timeout_ms));
        let ws = Arc::new(Workspace {
            worktree: worktree.to_path_buf(),
            settings: settings.daemon.clone(),
            state,
            server: RwLock::new(server),
            sessions,
            documents,
            inflight: Arc::new(Semaphore::new(settings.daemon.max_inflight)),
            activity: crate::activity::Activity::new(),
            shutting_down: cancel,
            supervisor: Mutex::new(None),
        });
        *ws.supervisor.lock().unwrap() = Some(tokio::spawn(supervise(
            Arc::downgrade(&ws),
            ws.shutting_down.clone(),
        )));
        Ok(ws)
    }

    pub fn worktree(&self) -> &Path {
        &self.worktree
    }

    /// The current Lean server (cloned `Arc`, so callers don't hold the lock).
    pub fn server(&self) -> Arc<LeanServer> {
        self.server.read().unwrap().clone()
    }

    /// The typed session for the current Lean process.
    pub fn session(&self) -> Arc<LeanSession> {
        self.sessions.read().unwrap().session.clone()
    }

    pub fn default_elab_timeout(&self) -> Duration {
        Duration::from_millis(self.settings.elaboration_timeout_ms)
    }

    pub fn default_lsp_timeout(&self) -> Duration {
        Duration::from_millis(self.settings.lsp_timeout_ms)
    }

    pub fn mark_active(&self) {
        self.activity.touch();
    }
    pub fn idle_for(&self) -> Duration {
        self.activity.idle_for()
    }
    pub(crate) fn begin_request(&self) -> crate::activity::Lease {
        self.activity.begin()
    }
    pub fn active_requests(&self) -> u64 {
        self.activity.active()
    }
    pub fn is_shutting_down(&self) -> bool {
        self.shutting_down.is_cancelled()
    }

    #[cfg(test)]
    pub fn pid(&self) -> Option<u32> {
        self.server().pid()
    }

    /// Acquire an in-flight permit (backpressure). Bounded so a flood of agents
    /// gets `Overloaded` instead of wedging the server.
    pub async fn acquire_inflight(&self, ctx: &RequestContext) -> Result<OwnedSemaphorePermit> {
        ctx.run(async {
            match tokio::time::timeout(
                Duration::from_millis(self.settings.permit_timeout_ms),
                self.inflight.clone().acquire_owned(),
            )
            .await
            {
                Ok(Ok(permit)) => Ok(permit),
                Ok(Err(_)) => Err(Error::ServerDown("semaphore closed".into())),
                Err(_) => Err(Error::Overloaded),
            }
        })
        .await
    }

    /// Begin shutdown: stop supervision and kill the server.
    pub async fn shutdown(&self) {
        self.shutting_down.cancel();
        self.inflight.close();
        let task = self.supervisor.lock().unwrap().take();
        if let Some(mut task) = task {
            if tokio::time::timeout(Duration::from_secs(5), &mut task)
                .await
                .is_err()
            {
                task.abort();
                let _ = task.await;
            }
        }
        self.documents.close_all();
        self.server().shutdown().await;
    }

    pub async fn document(&self, path: &Path, ctx: &RequestContext) -> Result<Document> {
        self.documents.pin(path, false, ctx).await
    }
    pub async fn restart_file(&self, path: &Path, ctx: &RequestContext) -> Result<String> {
        let document = self.documents.pin(path, true, ctx).await?;
        Ok(document.uri.clone())
    }
    pub fn evict_documents(&self) {
        self.documents.evict();
    }
}

/// Warmup failure is daemon policy; the Lake utility returns an inspectable outcome.
async fn warm_mathlib_cache(worktree: &Path, settings: &leanck_config::Cache) {
    use leanck_lake::cache::{CacheOptions, CacheOutcome, SkipReason};
    let options = CacheOptions {
        enabled: settings.enabled,
        execution: leanck_lake::ExecutionOptions {
            timeout: Duration::from_millis(settings.timeout_ms),
            max_output_bytes: settings.max_output_bytes,
            ..Default::default()
        },
    };
    if options.enabled && leanck_lake::cache::depends_on_mathlib(worktree) {
        tracing::info!(worktree = %worktree.display(), "fetching Mathlib cache (`lake exe cache get`) before warmup");
    }
    match leanck_lake::cache::fetch(worktree, options.clone()).await {
        Ok(CacheOutcome::Skipped(SkipReason::Disabled)) => {
            tracing::debug!("cache fetching disabled; skipping cache fetch")
        }
        Ok(CacheOutcome::Skipped(SkipReason::NotMathlib)) => {}
        Ok(CacheOutcome::Completed(_)) => tracing::info!("`lake exe cache get` completed"),
        Ok(CacheOutcome::Failed(output)) => {
            tracing::warn!(exit_code = ?output.exit_code, "`lake exe cache get` failed; falling back to building from source")
        }
        Ok(CacheOutcome::TimedOut) => tracing::warn!(
            timeout_secs = options.execution.timeout.as_secs(),
            "`lake exe cache get` timed out; falling back to building from source"
        ),
        Err(error) => {
            tracing::warn!(%error, "could not fetch Mathlib cache; falling back to building from source")
        }
    }
}

/// Supervision loop: when the server dies (stdout EOF), respawn it, re-initialize,
/// and re-open tracked docs. Per-request timeouts already prevent freezes; this
/// recovers the server so subsequent requests succeed.
async fn supervise(weak: std::sync::Weak<Workspace>, shutdown: CancellationToken) {
    loop {
        let Some(ws) = weak.upgrade() else {
            return;
        };
        let dead = ws.session().dead_token();
        drop(ws);
        tokio::select! { biased; _ = shutdown.cancelled() => return, _ = dead.cancelled() => {} }
        let Some(ws) = weak.upgrade() else {
            return;
        };
        let previous = ws.server();
        let worktree = ws.worktree.clone();
        let state = ws.state.clone();
        drop(ws);
        previous.shutdown().await;
        drop(previous);
        loop {
            if shutdown.is_cancelled() {
                return;
            }
            let generation = state.generation() + 1;
            state.begin_generation(generation);
            match LeanServer::spawn_with_cancel(&worktree, state.clone(), &shutdown).await {
                Ok(new) => {
                    let Some(ws) = weak.upgrade() else {
                        new.shutdown().await;
                        return;
                    };
                    let installed = {
                        let mut current = ws.server.write().unwrap();
                        if shutdown.is_cancelled() {
                            false
                        } else {
                            *current = new.clone();
                            *ws.sessions.write().unwrap() = SessionView {
                                generation,
                                session: new.session().clone(),
                            };
                            true
                        }
                    };
                    if !installed {
                        new.shutdown().await;
                        return;
                    }
                    tokio::select! { biased; _ = shutdown.cancelled() => return, _ = async {if let Ok(ctx)=RequestContext::new(None,shutdown.clone()) {ws.documents.recover(&ctx).await;}} => {} }
                    break;
                }
                Err(Error::Cancelled) => return,
                Err(e) => {
                    tracing::error!(error = %e, "respawn failed; retrying in 3s");
                    tokio::select! { biased; _ = shutdown.cancelled() => return, _ = tokio::time::sleep(Duration::from_secs(3)) => {} }
                }
            }
        }
    }
}
impl Drop for Workspace {
    fn drop(&mut self) {
        self.shutting_down.cancel();
        if let Some(task) = self.supervisor.get_mut().unwrap().take() {
            task.abort();
        }
    }
}
