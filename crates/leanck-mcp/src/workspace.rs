//! MCP session selection only. Connection and retry policy live in leanck-client.
use leanck_client::Client;
use leanck_core::workspace::WorkspaceRoot;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct WorkspaceSelection(Arc<RwLock<Selection>>);

struct Selection {
    client: Client,
    daemon_binary: Option<PathBuf>,
}

impl WorkspaceSelection {
    pub fn new(root: WorkspaceRoot) -> Self {
        Self(Arc::new(RwLock::new(Selection {
            client: Client::new(root),
            daemon_binary: None,
        })))
    }
    /// Keep the host executable across workspace changes and config reloads.
    pub fn with_daemon_binary(self, binary: PathBuf) -> Self {
        let mut selection = self.0.write().unwrap();
        selection.client = selection.client.clone().with_daemon_binary(binary.clone());
        selection.daemon_binary = Some(binary);
        drop(selection);
        self
    }
    /// Capture once, before awaiting startup, querying, or rendering.
    pub fn capture(&self) -> Client {
        self.0.read().unwrap().client.clone()
    }
    pub fn set_configured(&self, root: WorkspaceRoot, settings: leanck_config::Settings) {
        let mut client = Client::new(root).with_settings(settings);
        let mut selection = self.0.write().unwrap();
        if let Some(binary) = &selection.daemon_binary {
            client = client.with_daemon_binary(binary.clone());
        }
        selection.client = client;
    }
    pub fn set(&self, root: WorkspaceRoot) {
        self.set_configured(root, leanck_config::Settings::default());
    }
}
