//! MCP session selection only. Connection and retry policy live in leanck-client.
use leanck_client::Client;
use leanck_core::workspace::WorkspaceRoot;
use std::sync::{Arc, RwLock};

#[derive(Clone)]
pub struct WorkspaceSelection(Arc<RwLock<Client>>);

impl WorkspaceSelection {
    pub fn new(root: WorkspaceRoot) -> Self {
        Self(Arc::new(RwLock::new(Client::new(root))))
    }
    /// Capture once, before awaiting startup, querying, or rendering.
    pub fn capture(&self) -> Client {
        self.0.read().unwrap().clone()
    }
    pub fn set_configured(&self, root: WorkspaceRoot, settings: leanck_config::Settings) {
        *self.0.write().unwrap() = Client::new(root).with_settings(settings);
    }
    pub fn set(&self, root: WorkspaceRoot) {
        *self.0.write().unwrap() = Client::new(root);
    }
}
