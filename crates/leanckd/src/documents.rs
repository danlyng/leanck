//! One registry owns document mutation, pinned query leases, and retention.
use crate::{
    error::{Error, Result},
    request::RequestContext,
    server::{LeanSession, extensions::DependencyBuildMode},
    state::{DocState, ElaborationStore, WireRange, wait_for_version},
};
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicI64, Ordering},
    },
    time::SystemTime,
};
use tokio::{
    sync::{OwnedRwLockReadGuard, RwLock as AsyncRwLock},
    time::Instant,
};

#[derive(Clone)]
pub struct SessionView {
    pub generation: u64,
    pub session: Arc<LeanSession>,
}
struct Content {
    source: Arc<str>,
    version: i64,
    generation: u64,
    opened_at: SystemTime,
}
struct Entry {
    path: PathBuf,
    gate: Arc<AsyncRwLock<Content>>,
    used: Instant,
}
pub struct DocumentRegistry {
    entries: Mutex<HashMap<String, Entry>>,
    session: Arc<RwLock<SessionView>>,
    state: Arc<ElaborationStore>,
    roots: leanck_lake::source::SourceRoots,
    next_version: AtomicI64,
    capacity: usize,
    max_bytes: usize,
    lsp_timeout: std::time::Duration,
}
pub struct Document {
    pub lsp_timeout: std::time::Duration,
    pub uri: String,
    pub session: Arc<LeanSession>,
    pub generation: u64,
    pub notes: Vec<String>,
    guard: OwnedRwLockReadGuard<Content>,
    state: Arc<ElaborationStore>,
}
impl Document {
    pub fn source(&self) -> &str {
        &self.guard.source
    }
    pub fn version(&self) -> i64 {
        self.guard.version
    }
    pub fn state(&self) -> Result<DocState> {
        self.state
            .get_version(self.generation, &self.uri, self.version())
    }
    pub fn validate(&self) -> Result<()> {
        if self.session.is_dead() {
            return Err(Error::ServerDown(
                "pinned Lean session stopped; retry the query".into(),
            ));
        }
        self.state
            .verify_version(self.generation, &self.uri, self.version())
    }
    pub async fn wait(
        &self,
        range: Option<&WireRange>,
        timeout: std::time::Duration,
        ctx: &RequestContext,
    ) -> Result<()> {
        let dead = self.session.dead_token();
        ctx.run(async {tokio::select! {biased;_=dead.cancelled()=>Err(Error::ServerDown("pinned Lean session stopped".into())),result=wait_for_version(&self.state,self.generation,&self.uri,self.version(),range,timeout)=>result}}).await
    }
}
impl DocumentRegistry {
    pub fn new(
        session: Arc<RwLock<SessionView>>,
        state: Arc<ElaborationStore>,
        roots: leanck_lake::source::SourceRoots,
    ) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            session,
            state,
            roots,
            next_version: AtomicI64::new(1),
            capacity: 128,
            max_bytes: 64 * 1024 * 1024,
            lsp_timeout: std::time::Duration::from_secs(30),
        }
    }
    pub fn with_lsp_timeout(mut self, timeout: std::time::Duration) -> Self {
        self.lsp_timeout = timeout;
        self
    }
    pub fn with_limits(mut self, capacity: usize, max_bytes: usize) -> Self {
        self.capacity = capacity;
        self.max_bytes = max_bytes;
        self
    }
    pub async fn pin(&self, path: &Path, restart: bool, ctx: &RequestContext) -> Result<Document> {
        let document = leanck_core::document::DocumentPath::from_path(path)?;
        let path = document.into_path_buf();
        let uri = leanck_core::document::path_to_uri(&path)?;
        let gate = {
            let mut entries = self.entries.lock().unwrap();
            let entry = entries.entry(uri.clone()).or_insert_with(|| Entry {
                path: path.clone(),
                gate: Arc::new(AsyncRwLock::new(Content {
                    source: Arc::from(""),
                    version: 0,
                    generation: u64::MAX,
                    opened_at: SystemTime::UNIX_EPOCH,
                })),
                used: Instant::now(),
            });
            entry.used = Instant::now();
            entry.gate.clone()
        };
        let read = ctx
            .run(async { Ok(gate.clone().read_owned().await) })
            .await?;
        let view = self.session.read().unwrap().clone();
        if view.session.is_dead() {
            return Err(Error::ServerDown("Lean server is recovering".into()));
        }
        let content = ctx.run(read_source(&path)).await?;
        let fresh = !restart
            && read.version > 0
            && read.generation == view.generation
            && read.source.as_ref() == content;
        if fresh && !self.stale(&path, read.opened_at, ctx).await? {
            return Ok(Document {
                lsp_timeout: self.lsp_timeout,
                uri,
                session: view.session,
                generation: view.generation,
                notes: self.notes(),
                guard: read,
                state: self.state.clone(),
            });
        }
        drop(read);
        let mut write = ctx
            .run(async { Ok(gate.clone().write_owned().await) })
            .await?;
        let view = self.session.read().unwrap().clone();
        if view.session.is_dead() {
            return Err(Error::ServerDown("Lean server is recovering".into()));
        }
        let mutation_started = SystemTime::now();
        let content = ctx.run(read_source(&path)).await?;
        let same = write.version > 0
            && write.generation == view.generation
            && write.source.as_ref() == content;
        let reopen = restart || (same && self.stale(&path, write.opened_at, ctx).await?);
        if !same || reopen {
            let version = self.next_version.fetch_add(1, Ordering::Relaxed);
            if version > i64::from(i32::MAX) {
                return Err(Error::ServerDown("document version space exhausted".into()));
            }
            self.state.begin_document(view.generation, &uri, version)?;
            let result = if write.generation != view.generation || write.version == 0 {
                view.session
                    .open(&uri, &content, version, Some(DependencyBuildMode::Once))
            } else if reopen {
                view.session.close(&uri).and_then(|_| {
                    view.session
                        .open(&uri, &content, version, Some(DependencyBuildMode::Once))
                })
            } else {
                view.session.change(&uri, &content, version)
            };
            if let Err(error) = result {
                self.state.forget(&uri);
                write.generation = u64::MAX;
                return Err(error);
            }
            write.source = Arc::from(content);
            write.version = version;
            write.generation = view.generation;
            write.opened_at = mutation_started;
        }
        let guard = tokio::sync::OwnedRwLockWriteGuard::downgrade(write);
        let doc = Document {
            lsp_timeout: self.lsp_timeout,
            uri,
            session: view.session,
            generation: view.generation,
            notes: self.notes(),
            guard,
            state: self.state.clone(),
        };
        self.evict();
        doc.validate()?;
        Ok(doc)
    }
    fn notes(&self) -> Vec<String> {
        if self.roots.resolved {
            Vec::new()
        } else {
            vec!["project source roots unavailable; dependency freshness uses conservative reopening".into()]
        }
    }
    async fn stale(&self, path: &Path, since: SystemTime, ctx: &RequestContext) -> Result<bool> {
        if !self.roots.resolved {
            return Ok(true);
        }
        let roots = self.roots.paths.clone();
        let path = path.to_owned();
        let context = ctx.clone();
        ctx.run(async {
            tokio::task::spawn_blocking(move || imports_stale(&path, &roots, since, &context))
                .await
                .map_err(|e| Error::ServerDown(e.to_string()))?
        })
        .await
    }
    pub async fn recover(&self, ctx: &RequestContext) {
        let paths: Vec<_> = self
            .entries
            .lock()
            .unwrap()
            .values()
            .map(|entry| entry.path.clone())
            .collect();
        for path in paths {
            if ctx.check().is_err() {
                return;
            }
            let _ = self.pin(&path, false, ctx).await;
        }
    }
    pub fn evict(&self) {
        let view = self.session.read().unwrap().clone();
        let mut entries = self.entries.lock().unwrap();
        let mut bytes = entries
            .values()
            .filter_map(|e| e.gate.try_read().ok().map(|c| c.source.len()))
            .sum::<usize>();
        let mut candidates: Vec<_> = entries
            .iter()
            .filter(|(_, e)| Arc::strong_count(&e.gate) == 1)
            .map(|(uri, e)| (e.used, uri.clone()))
            .collect();
        candidates.sort();
        for (_, uri) in candidates {
            if entries.len() <= self.capacity && bytes <= self.max_bytes {
                break;
            }
            let entry = entries.get(&uri).unwrap();
            if Arc::strong_count(&entry.gate) != 1 {
                continue;
            }
            let Ok(content) = entry.gate.try_write() else {
                continue;
            };
            bytes = bytes.saturating_sub(content.source.len());
            if content.generation == view.generation {
                let _ = view.session.close(&uri);
            }
            self.state.forget(&uri);
            drop(content);
            entries.remove(&uri);
        }
    }
    pub fn close_all(&self) {
        let view = self.session.read().unwrap().clone();
        for uri in self.entries.lock().unwrap().keys() {
            let _ = view.session.close(uri);
            self.state.forget(uri);
        }
        self.entries.lock().unwrap().clear();
    }
}
async fn read_source(path: &Path) -> Result<String> {
    tokio::fs::read_to_string(path)
        .await
        .map_err(|error| Error::InvalidPath(format!("{}: {error}", path.display())))
}
fn imports_stale(
    path: &Path,
    roots: &[PathBuf],
    since: SystemTime,
    ctx: &RequestContext,
) -> Result<bool> {
    let mut seen = HashSet::new();
    let mut stack = vec![path.to_owned()];
    while let Some(file) = stack.pop() {
        ctx.check()?;
        if !seen.insert(file.clone()) {
            continue;
        }
        if seen.len() > 4096 {
            return Ok(true);
        }
        let Ok(source) = std::fs::read_to_string(file) else {
            return Ok(true);
        };
        for module in leanck_core::scan::imports(&source) {
            let rel = PathBuf::from(module.replace('.', "/")).with_extension("lean");
            if let Some(candidate) = roots
                .iter()
                .map(|root| root.join(&rel))
                .find(|p| p.is_file())
            {
                if candidate != path
                    && std::fs::metadata(&candidate)
                        .and_then(|m| m.modified())
                        .map_or(true, |mtime| mtime > since)
                {
                    return Ok(true);
                }
                stack.push(candidate);
            }
        }
    }
    Ok(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::server::test_peer::{self, Peer};
    use lsp_server::{Notification, Response};
    use serde_json::json;
    struct Fixture {
        root: PathBuf,
        registry: Arc<DocumentRegistry>,
        peer: Peer,
    }
    impl Fixture {
        fn new() -> Self {
            static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
            let root = std::env::temp_dir().join(format!(
                "leanck-docs-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&root).unwrap();
            let root = root.canonicalize().unwrap();
            let state = Arc::new(ElaborationStore::new());
            let (session, peer) = test_peer::pair(state.clone());
            let mut registry = DocumentRegistry::new(
                Arc::new(RwLock::new(SessionView {
                    generation: 0,
                    session,
                })),
                state,
                leanck_lake::source::SourceRoots {
                    paths: vec![root.clone()],
                    resolved: true,
                },
            );
            registry.capacity = 2;
            Self {
                root,
                registry: Arc::new(registry),
                peer,
            }
        }
        fn file(&self, name: &str, text: &str) -> PathBuf {
            let path = self.root.join(name);
            std::fs::write(&path, text).unwrap();
            path
        }
        async fn finish(mut self) {
            let session = self.registry.session.read().unwrap().session.clone();
            session.shutdown().await;
            self.peer.finish().await;
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }
    fn ctx() -> RequestContext {
        RequestContext::new(Some(2000), Default::default()).unwrap()
    }
    #[tokio::test]
    async fn unknown_source_layout_reopens_conservatively_and_reports_quality() {
        let mut f = Fixture::new();
        Arc::get_mut(&mut f.registry).unwrap().roots.resolved = false;
        let path = f.file("A.lean", "def a := 1");
        let first = f.registry.pin(&path, false, &ctx()).await.unwrap();
        assert!(!first.notes.is_empty());
        let version = first.version();
        drop(first);
        f.peer.notify("textDocument/didOpen").await;
        let again = f.registry.pin(&path, false, &ctx()).await.unwrap();
        assert!(again.version() > version);
        f.peer.notify("textDocument/didClose").await;
        f.peer.notify("textDocument/didOpen").await;
        drop(again);
        f.finish().await;
    }
    #[tokio::test]
    async fn read_lease_serializes_edits_and_restart_and_pins_source() {
        let mut f = Fixture::new();
        let path = f.file("A.lean", "def α := 1\n");
        let doc = f.registry.pin(&path, false, &ctx()).await.unwrap();
        let first = doc.version();
        f.peer.notify("textDocument/didOpen").await;
        std::fs::write(&path, "def α := 2\n").unwrap();
        let registry = f.registry.clone();
        let file = path.clone();
        let change = tokio::spawn(async move { registry.pin(&file, false, &ctx()).await });
        tokio::task::yield_now().await;
        assert!(!change.is_finished());
        assert_eq!(doc.source(), "def α := 1\n");
        drop(doc);
        let changed = change.await.unwrap().unwrap();
        assert_eq!(changed.source(), "def α := 2\n");
        assert!(changed.version() > first);
        let notification = f.peer.notify("textDocument/didChange").await;
        assert_eq!(
            notification.params["textDocument"]["version"],
            changed.version()
        );
        let registry = f.registry.clone();
        let file = path.clone();
        let restart = tokio::spawn(async move { registry.pin(&file, true, &ctx()).await });
        tokio::task::yield_now().await;
        assert!(!restart.is_finished());
        let second = changed.version();
        drop(changed);
        let restarted = restart.await.unwrap().unwrap();
        assert!(restarted.version() > second);
        f.peer.notify("textDocument/didClose").await;
        f.peer.notify("textDocument/didOpen").await;
        drop(restarted);
        f.finish().await;
    }
    #[tokio::test]
    async fn aliases_share_identity_and_cancelled_mutations_do_not_advance_version() {
        let mut f = Fixture::new();
        let path = f.file("A #.lean", "def a := 1\n");
        let alias = f.root.join("alias.lean");
        std::os::unix::fs::symlink(&path, &alias).unwrap();
        let doc = f.registry.pin(&path, false, &ctx()).await.unwrap();
        f.peer.notify("textDocument/didOpen").await;
        let same = f.registry.pin(&alias, false, &ctx()).await.unwrap();
        assert_eq!(doc.uri, same.uri);
        assert_eq!(doc.version(), same.version());
        let short = RequestContext::new(Some(20), Default::default()).unwrap();
        assert!(matches!(
            f.registry.pin(&path, true, &short).await,
            Err(Error::Timeout)
        ));
        assert_eq!(doc.version(), same.version());
        drop(same);
        drop(doc);
        f.finish().await;
    }
    #[tokio::test]
    async fn eviction_protects_leases_and_queued_waiters_and_never_reuses_versions() {
        let mut f = Fixture::new();
        let a = f.file("A.lean", "def a := 1");
        let b = f.file("B.lean", "def b := 1");
        let c = f.file("C.lean", "def c := 1");
        let first = f.registry.pin(&a, false, &ctx()).await.unwrap();
        let first_version = first.version();
        f.peer.notify("textDocument/didOpen").await;
        let waiting = first.uri.clone();
        let registry = f.registry.clone();
        let path = a.clone();
        let queued = tokio::spawn(async move { registry.pin(&path, true, &ctx()).await });
        tokio::task::yield_now().await;
        let second = f.registry.pin(&b, false, &ctx()).await.unwrap();
        f.peer.notify("textDocument/didOpen").await;
        drop(second);
        let third = f.registry.pin(&c, false, &ctx()).await.unwrap();
        f.peer.notify("textDocument/didOpen").await;
        let closed = f.peer.notify("textDocument/didClose").await;
        assert_ne!(closed.params["textDocument"]["uri"], waiting);
        assert!(f.registry.state.get(&waiting).is_some());
        drop(first);
        let restarted = queued.await.unwrap().unwrap();
        f.peer.notify("textDocument/didClose").await;
        f.peer.notify("textDocument/didOpen").await;
        assert!(restarted.version() > first_version);
        drop(restarted);
        drop(third);
        Arc::get_mut(&mut f.registry).unwrap().capacity = 0;
        f.registry.evict();
        assert!(f.registry.entries.lock().unwrap().is_empty());
        let reopened = f.registry.pin(&a, false, &ctx()).await.unwrap();
        assert!(reopened.version() > first_version);
        f.registry
            .state
            .progress_for(0, &reopened.uri, first_version, vec![]);
        assert!(!reopened.state().unwrap().seen_progress);
        drop(reopened);
        f.finish().await;
    }
    #[tokio::test]
    async fn recovery_reopens_once_and_old_session_notifications_are_isolated() {
        let mut f = Fixture::new();
        let path = f.file("A.lean", "def a := 1");
        let old = f.registry.pin(&path, false, &ctx()).await.unwrap();
        f.peer.notify("textDocument/didOpen").await;
        let uri = old.uri.clone();
        let old_version = old.version();
        drop(old);
        f.registry.state.begin_generation(1);
        let (new, mut new_peer) = test_peer::pair(f.registry.state.clone());
        let previous = f.registry.session.read().unwrap().session.clone();
        *f.registry.session.write().unwrap() = SessionView {
            generation: 1,
            session: new.clone(),
        };
        // A foreground request can synchronize before the recovery walk.
        let current = f.registry.pin(&path, false, &ctx()).await.unwrap();
        let version = current.version();
        new_peer.notify("textDocument/didOpen").await;
        drop(current);
        f.registry.recover(&ctx()).await;
        assert_eq!(
            f.registry.entries.lock().unwrap()[&uri]
                .gate
                .try_read()
                .unwrap()
                .version,
            version
        );
        f.peer
            .send(Notification::new(
                "$/lean/fileProgress".into(),
                json!({"textDocument":{"uri":uri,"version":version},"processing":[]}),
            ))
            .await;
        f.peer
            .send(Notification::new(
                "textDocument/publishDiagnostics".into(),
                json!({"uri":uri,"version":version,"diagnostics":[]}),
            ))
            .await;
        // A request/reply barrier proves the preceding notifications were processed.
        let session = previous.clone();
        let barrier = tokio::spawn(async move {
            session
                .document_symbols(&uri, &ctx(), std::time::Duration::from_secs(1))
                .await
        });
        let request = f.peer.request("textDocument/documentSymbol").await;
        f.peer.send(Response::new_ok(request.id, json!([]))).await;
        barrier.await.unwrap().unwrap();
        let st = f
            .registry
            .state
            .get_version(1, &current_uri(&path), version)
            .unwrap();
        assert!(!st.seen_progress);
        assert!(version > old_version);
        previous.shutdown().await;
        new.shutdown().await;
        new_peer.finish().await;
        f.finish().await;
    }
    fn current_uri(path: &Path) -> String {
        leanck_core::document::path_to_uri(path).unwrap()
    }
    #[tokio::test]
    async fn local_import_freshness_uses_custom_roots_and_transitive_modules() {
        let f = Fixture::new();
        let root = f.root.join("src");
        std::fs::create_dir(&root).unwrap();
        let main = f.file("Main.lean", "import A\n");
        std::fs::write(root.join("A.lean"), "import B\n").unwrap();
        std::fs::write(root.join("B.lean"), "def b := 1\n").unwrap();
        assert!(imports_stale(&main, &[root.clone()], SystemTime::UNIX_EPOCH, &ctx()).unwrap());
        assert!(
            !imports_stale(
                &main,
                &[root],
                SystemTime::now() + std::time::Duration::from_secs(1),
                &ctx()
            )
            .unwrap()
        );
        f.finish().await;
    }
}
