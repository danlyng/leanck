//! Workspace-bound daemon client shared by CLI and MCP.
//!
//! A fresh framed connection is opened per request. Startup and the one allowed
//! reconnect use the same canonical workspace. Requests whose delivery is
//! uncertain are never automatically replayed, including read-only operations.
mod discovery;
mod error;
pub mod instances;
pub mod render;
mod startup;
mod transport;

pub use discovery::resolve_binary;
pub use error::{ClientError, Result, TransportPhase};
use leanck_core::{api::*, error::ProtocolError, paths::RuntimePaths, workspace::WorkspaceRoot};
use serde::de::DeserializeOwned;
use serde_json::Value;
use std::path::Path;
use std::time::Duration;
pub use tokio_util::sync::CancellationToken;
use uuid::Uuid;

/// Options for one operation; no deadline is stored in the client or MCP session.
#[derive(Debug, Default, Clone)]
pub struct RequestOptions {
    pub timeout_ms: Option<u64>,
    /// Independent socket budget. Defaults to the operation budget plus 1s;
    /// health and compatibility probes default to 2s. Excludes cold startup.
    pub transport_timeout_ms: Option<u64>,
    pub cancellation: Option<CancellationToken>,
}

#[derive(Debug, Clone)]
pub struct Client {
    workspace: WorkspaceRoot,
    settings: leanck_config::Settings,
    daemon_binary: Option<std::path::PathBuf>,
}

impl Client {
    /// Capture a workspace without connecting or creating runtime directories.
    pub fn new(workspace: WorkspaceRoot) -> Self {
        Self {
            workspace,
            settings: leanck_config::Settings::default(),
            daemon_binary: None,
        }
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self> {
        Ok(Self::new(WorkspaceRoot::from_path(path)?))
    }

    pub fn discover() -> Result<Self> {
        Ok(Self::new(WorkspaceRoot::discover_current()?))
    }

    pub fn with_settings(mut self, settings: leanck_config::Settings) -> Self {
        self.settings = settings;
        self
    }

    /// Default executable to invoke with `serve daemon`. The hosting `leanck`
    /// supplies itself; library callers otherwise use sibling/PATH discovery.
    /// An explicit `client.daemon_bin` setting takes precedence.
    pub fn with_daemon_binary(mut self, binary: std::path::PathBuf) -> Self {
        self.daemon_binary = Some(binary);
        self
    }

    pub fn settings(&self) -> &leanck_config::Settings {
        &self.settings
    }

    /// Nonfatal diagnostics for the effective per-request timeout settings.
    pub fn request_warnings(&self, options: &RequestOptions) -> Vec<leanck_config::Warning> {
        let effective = leanck_config::Client {
            timeout_ms: options.timeout_ms.or(self.settings.client.timeout_ms),
            transport_timeout_ms: options
                .transport_timeout_ms
                .or(self.settings.client.transport_timeout_ms),
            ..self.settings.client.clone()
        };
        effective.timeout_warnings(self.settings.daemon.request_timeout_ms)
    }

    pub fn workspace(&self) -> &Path {
        self.workspace.as_path()
    }
    pub fn socket(&self) -> std::path::PathBuf {
        RuntimePaths::for_workspace(self.workspace()).socket()
    }

    /// Explicitly allow startup, without issuing a query.
    pub async fn ensure_running(&self) -> Result<()> {
        startup::ensure_running(
            self.workspace(),
            RuntimePaths::for_workspace(self.workspace()),
            &self.settings,
            self.daemon_binary.as_deref(),
        )
        .await?;
        Ok(())
    }

    /// Query existing health only. Missing/refused sockets mean no daemon;
    /// other transport errors remain errors. This operation never spawns/retries.
    pub async fn probe_existing(&self, options: RequestOptions) -> Result<Option<HealthResult>> {
        // A missing socket is idle, even if its hypothetical pathname exceeds
        // the Unix socket length limit (e.g. an unused custom cache directory).
        if !self
            .socket()
            .try_exists()
            .map_err(|source| ClientError::Transport {
                phase: TransportPhase::Connect,
                source,
            })?
        {
            return Ok(None);
        }
        match self
            .execute::<HealthResult>(Command::Health, options, false)
            .await
        {
            Ok(value) => Ok(Some(value)),
            Err(ClientError::Transport {
                phase: TransportPhase::Connect,
                source,
            }) if matches!(
                source.kind(),
                std::io::ErrorKind::NotFound | std::io::ErrorKind::ConnectionRefused
            ) =>
            {
                Ok(None)
            }
            Err(error) => Err(error),
        }
    }

    /// Explicit cleanup never starts a daemon merely to stop it.
    pub async fn shutdown_existing(&self, options: RequestOptions) -> Result<ShutdownResult> {
        self.execute(Command::Shutdown(ShutdownParams::default()), options, false)
            .await
    }

    async fn execute<T: DeserializeOwned>(
        &self,
        command: Command,
        options: RequestOptions,
        allow_startup: bool,
    ) -> Result<T> {
        let operation = command.operation();
        let value = self.call(command, options, allow_startup).await?;
        serde_json::from_value(value).map_err(|source| {
            ClientError::Protocol(ProtocolError::InvalidResult { operation, source })
        })
    }

    async fn call(
        &self,
        command: Command,
        options: RequestOptions,
        allow_startup: bool,
    ) -> Result<Value> {
        let mut options = options;
        let control = matches!(
            &command,
            Command::Health | Command::Status | Command::Shutdown(_) | Command::Cancel(_)
        );
        if !control {
            for warning in self.request_warnings(&options) {
                eprintln!("warning: {}", warning.message);
            }
            options.timeout_ms = options.timeout_ms.or(self.settings.client.timeout_ms);
        }
        options.transport_timeout_ms = options
            .transport_timeout_ms
            .or(self.settings.client.transport_timeout_ms);
        let health = matches!(&command, Command::Health | Command::Status);
        let budget = Duration::from_millis(options.transport_timeout_ms.unwrap_or_else(|| {
            if control {
                self.settings.client.probe_timeout_ms
            } else {
                options
                    .timeout_ms
                    .unwrap_or(self.settings.daemon.request_timeout_ms)
                    .saturating_add(1_000)
            }
        }));
        let request = self.request_bytes(command, options.clone())?;
        let runtime = RuntimePaths::for_workspace(self.workspace());
        let attempt = self
            .attempt(&runtime, &request, &options, budget, !control)
            .await;
        let value = if allow_startup {
            self.finish_call(attempt, runtime, &request, &options, budget, !control)
                .await?
        } else {
            attempt?
        };
        if health {
            check_protocol(&value)?;
        }
        Ok(value)
    }

    async fn startup(&self, runtime: &RuntimePaths, options: &RequestOptions) -> Result<()> {
        let work = startup::ensure_running(
            self.workspace(),
            runtime.clone(),
            &self.settings,
            self.daemon_binary.as_deref(),
        );
        if let Some(token) = &options.cancellation {
            tokio::select! { biased; _ = token.cancelled() => Err(ClientError::Cancelled { delivery_uncertain: false }), result = work => result.map(|_| ()) }
        } else {
            work.await.map(|_| ())
        }
    }

    async fn attempt(
        &self,
        runtime: &RuntimePaths,
        request: &PreparedRequest,
        options: &RequestOptions,
        budget: Duration,
        negotiate: bool,
    ) -> Result<Value> {
        if negotiate {
            let probe = PreparedRequest::new(
                Command::Health,
                format!("compatibility-{}", Uuid::new_v4()),
                None,
            )?;
            let result = transport::call_with_options(
                &runtime.socket(),
                &probe,
                Duration::from_millis(self.settings.client.probe_timeout_ms),
                options.cancellation.as_ref(),
                false,
            )
            .await;
            let value = match result {
                Err(ClientError::Cancelled { .. }) => {
                    return Err(ClientError::Cancelled {
                        delivery_uncertain: false,
                    });
                }
                Err(error) if error.is_pre_dispatch() => return Err(error),
                Err(error) => return Err(ClientError::Handshake(Box::new(error))),
                Ok(value) => value,
            };
            if let Err(error) = check_protocol(&value) {
                return Err(match error {
                    incompatible @ ClientError::IncompatibleProtocol { .. } => incompatible,
                    other => ClientError::Handshake(Box::new(other)),
                });
            }
        }
        transport::call_with_options(
            &runtime.socket(),
            request,
            budget,
            options.cancellation.as_ref(),
            negotiate,
        )
        .await
    }

    async fn finish_call(
        &self,
        first: Result<Value>,
        runtime: RuntimePaths,
        request: &PreparedRequest,
        options: &RequestOptions,
        budget: Duration,
        negotiate: bool,
    ) -> Result<Value> {
        match first {
            Err(error) if error.is_pre_dispatch() => {
                self.startup(&runtime, options).await?;
                self.attempt(&runtime, request, options, budget, negotiate)
                    .await
            }
            result => result,
        }
    }

    fn request_bytes(&self, command: Command, options: RequestOptions) -> Result<PreparedRequest> {
        PreparedRequest::new(command, Uuid::new_v4().to_string(), options.timeout_ms)
    }

    /// Cancel an existing request without starting a daemon or negotiating away explicit recovery.
    pub async fn cancel_existing(
        &self,
        target_id: String,
        options: RequestOptions,
    ) -> Result<CancelResult> {
        self.execute(Command::Cancel(CancelParams { target_id }), options, false)
            .await
    }

    pub async fn health(&self, options: RequestOptions) -> Result<HealthResult> {
        self.execute(Command::Health, options, true).await
    }

    pub async fn diagnostics(
        &self,
        params: &DiagnosticsParams,
        options: RequestOptions,
    ) -> Result<DiagnosticsResult> {
        self.execute(Command::Diagnostics(params.clone()), options, true)
            .await
    }

    pub async fn goal(&self, params: &GoalParams, options: RequestOptions) -> Result<GoalResult> {
        self.execute(Command::Goal(params.clone()), options, true)
            .await
    }

    pub async fn sorries(
        &self,
        params: &SorryParams,
        options: RequestOptions,
    ) -> Result<SorryResult> {
        self.execute(Command::Sorry(params.clone()), options, true)
            .await
    }

    pub async fn list(&self, params: &ListParams, options: RequestOptions) -> Result<ListResult> {
        self.execute(Command::List(params.clone()), options, true)
            .await
    }

    pub async fn references(
        &self,
        params: &ReferencesParams,
        options: RequestOptions,
    ) -> Result<ReferencesResult> {
        self.execute(Command::References(params.clone()), options, true)
            .await
    }

    pub async fn restart_file(
        &self,
        params: &RestartFileParams,
        options: RequestOptions,
    ) -> Result<RestartFileResult> {
        self.execute(Command::RestartFile(params.clone()), options, true)
            .await
    }
}

fn check_protocol(value: &Value) -> Result<()> {
    #[derive(serde::Deserialize)]
    struct Header {
        protocol_version: u64,
    }
    let header: Header = serde_json::from_value(value.clone()).map_err(|source| {
        ClientError::Protocol(ProtocolError::InvalidResult {
            operation: Operation::Health,
            source,
        })
    })?;
    if header.protocol_version != u64::from(PROTOCOL_VERSION) {
        return Err(ClientError::IncompatibleProtocol {
            expected: PROTOCOL_VERSION,
            actual: header.protocol_version,
        });
    }
    Ok(())
}

struct PreparedRequest {
    id: String,
    bytes: Vec<u8>,
}

impl PreparedRequest {
    fn new(command: Command, id: String, timeout_ms: Option<u64>) -> Result<Self> {
        let request = command
            .into_request(id, timeout_ms)
            .map_err(ClientError::Encode)?;
        Ok(Self {
            id: request.id.clone(),
            bytes: serde_json::to_vec(&request).map_err(ClientError::Encode)?,
        })
    }
}

#[cfg(test)]
mod tests;
