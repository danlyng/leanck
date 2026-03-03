use leanck_loogle::LoogleQuery;
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Content, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::workspace::WorkspaceSelection;
use leanck_client::render::{HealthReport, format_health_human};
use leanck_client::{CancellationToken, ClientError, RequestOptions, TransportPhase};
use leanck_core::api::{
    DiagnosticsParams, GoalParams, ListParams, RestartFileParams, SeverityFilter, SorryParams,
};
use leanck_core::error::ErrorKind;

/// Wait for the elaborator to settle and return diagnostics for the file.
/// If `ident` is supplied, the response is restricted to diagnostics
/// overlapping the resolved declaration's range.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct DiagnosticsInput {
    /// File to query (workspace-relative path or `file://` URI).
    pub file: String,
    /// Optional declaration name. When set, diagnostics are restricted
    /// to the range of that declaration; otherwise the whole-file set
    /// is returned.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ident: Option<String>,
    /// Typed severity filter: {"mode":"all"} or
    /// {"mode":"only","severities":["error","warning","information","hint"]}.
    /// Omission uses configured diagnostics.severity (built-in: error-only); an empty selection matches nothing.
    /// Diagnostics with the message `Goals accomplished!` are always excluded.
    #[serde(default, deserialize_with = "deserialize_severity")]
    #[schemars(with = "SeverityFilter")]
    pub severity: Option<SeverityFilter>,
    /// Pagination offset into the filtered diagnostic list. Default `0`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub offset: Option<u32>,
    /// Maximum number of diagnostics to return. Default `5`. The
    /// response carries `total` (count after filters, before pagination)
    /// so callers know when to page; `limit`/`offset` are echoed back
    /// only when they actually trim the result.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    /// If true, return the full JSON response from the daemon.
    /// Default `false` returns compact human-readable text.
    #[serde(default)]
    pub verbose: Option<bool>,
    /// Operation budget in milliseconds, including queueing. Cold startup has
    /// a separate budget. Explicit values also override the elaboration cap.
    /// Defaults: 300s operation, 120s elaboration wait.
    /// Raise it for slow Mathlib-heavy files.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl From<&DiagnosticsInput> for DiagnosticsParams {
    fn from(input: &DiagnosticsInput) -> Self {
        Self {
            file: input.file.clone(),
            ident: input.ident.clone(),
            severity: input.severity.clone().unwrap_or_default(),
            offset: input.offset.map(u64::from),
            limit: input.limit.map(u64::from),
        }
    }
}

/// Look up the proof goal at a given file position.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct GoalInput {
    /// File (workspace-relative path or `file://` URI).
    pub file: String,
    /// 0-indexed line number.
    pub line: u32,
    /// 0-indexed character column.
    pub character: u32,
    /// Operation budget in milliseconds, including queueing, after cold startup.
    /// Defaults: 300s operation, 30s goal LSP call. An explicit value also
    /// overrides the goal-call cap, bounded by the remaining operation budget.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// List sorry locations under a file or directory path.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct SorryInput {
    /// Workspace-relative path, absolute path, or `file://` URI. If this
    /// points to a directory, `.lean` files are scanned recursively.
    pub path: String,
    /// Operation budget in milliseconds after cold startup. Defaults: 300s
    /// operation, 5s elaboration gate before text fallback. An explicit value
    /// overrides the brief gate, bounded by the remaining operation budget.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// List declarations in a file.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct ListInput {
    /// File to list (workspace-relative path or `file://` URI).
    pub file: String,
    /// Optional Lean keywords (`theorem`, `def`, `lemma`, `instance`, …) or
    /// LSP kind names (`namespace`, `class`, …) to restrict the result.
    /// Empty/omitted returns every declaration.
    #[serde(default)]
    pub kind: Vec<leanck_core::api::KindSelector>,
    /// Operation budget in milliseconds, including queueing. Cold startup has
    /// a separate budget. Explicit values also override the elaboration cap.
    /// Defaults: 300s operation, 120s elaboration wait.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Restart the Lean language server for a single file.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct RestartFileInput {
    /// File whose elaborator should be restarted. Workspace-relative path
    /// or `file://` URI.
    pub file: String,
    /// Operation budget in milliseconds after cold startup, including file-lock
    /// waits and restart submission. Default 300s. Dependency elaboration runs
    /// asynchronously; subsequent queries wait for it.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

impl From<&GoalInput> for GoalParams {
    fn from(input: &GoalInput) -> Self {
        Self {
            file: input.file.clone(),
            line: input.line,
            character: input.character,
        }
    }
}
impl From<&SorryInput> for SorryParams {
    fn from(input: &SorryInput) -> Self {
        Self {
            path: input.path.clone(),
        }
    }
}
impl From<&ListInput> for ListParams {
    fn from(input: &ListInput) -> Self {
        Self {
            file: input.file.clone(),
            kind: Some(input.kind.clone()),
        }
    }
}
impl From<&RestartFileInput> for RestartFileParams {
    fn from(input: &RestartFileInput) -> Self {
        Self {
            file: input.file.clone(),
        }
    }
}

/// Run a full `lake build` of the current workspace and report the result.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct BuildInput {
    /// Also report warnings emitted by Lake, including dependency and configuration warnings. Default `false`.
    #[serde(default)]
    pub warnings: Option<bool>,
    /// If true, treat build warnings as failures and include them in the report.
    /// Default `false`.
    #[serde(default)]
    pub strict: Option<bool>,
    /// If true, return the full machine-readable JSON build report instead of
    /// compact text. Default `false`.
    #[serde(default)]
    pub json: Option<bool>,
    /// Targets forwarded to lake build; omission uses configured targets, [] builds the whole workspace.
    #[serde(default)]
    pub targets: Option<Vec<String>>,
    /// Budget including coordination, in milliseconds. Default: 1,800,000 (30 minutes).
    #[serde(default)]
    pub timeout_ms: Option<u64>,
    /// Maximum captured bytes per stream. Default: 8 MiB; must be positive.
    #[serde(default)]
    pub max_output_bytes: Option<usize>,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct HealthzInput {
    /// If true, return the full machine-readable JSON health payload.
    /// Default `false` returns compact text (`ok` when there are no issues).
    #[serde(default)]
    pub json: Option<bool>,
}

/// Repoint the server at a different Lake project root.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SetWorkspaceInput {
    /// New workspace root. Absolute path or a path interpretable from the
    /// server's working directory. Must contain a Lake project marker
    /// (`lakefile.lean`, `lakefile.toml`, or `lean-toolchain`).
    pub path: String,
}

/// Search Lean and Mathlib declarations through Loogle.
#[derive(Debug, Deserialize, schemars::JsonSchema)]
pub struct LoogleInput {
    /// Exact constants / declarations to search for, e.g. `Real.sin`.
    #[serde(default)]
    pub constant: Vec<String>,
    /// Name substrings to search for. Each value is quoted for Loogle, e.g.
    /// `"differ"` for declarations with `differ` in the name.
    #[serde(default)]
    pub substr: Vec<String>,
    /// Subexpressions to search for, e.g. `_ * (_ ^ _)`.
    #[serde(default)]
    pub subexpr: Vec<String>,
    /// Conclusion type patterns. A `|-` turnstile is added when the value does
    /// not already start with `|-` or `⊢`.
    #[serde(default)]
    pub conclusion: Vec<String>,
    /// Max results to show. Omission uses loogle.limit (built-in: 10).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limit: Option<usize>,
}

impl From<&LoogleInput> for LoogleQuery {
    fn from(input: &LoogleInput) -> Self {
        Self {
            constant: input.constant.clone(),
            substr: input.substr.clone(),
            subexpr: input.subexpr.clone(),
            conclusion: input.conclusion.clone(),
        }
    }
}

#[derive(Clone)]
pub struct LeanckServer {
    workspace: WorkspaceSelection,
    config_args: leanck_config::ConfigArgs,
    loogle: leanck_loogle::Client,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl LeanckServer {
    pub fn new(config: &Config) -> Self {
        let workspace = WorkspaceSelection::new(config.workspace.clone());
        workspace.set_configured(config.workspace.clone(), config.settings.clone());
        Self {
            workspace,
            config_args: config.args.clone(),
            loogle: leanck_loogle::Client::from_config(&config.settings.loogle),
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl LeanckServer {
    #[tool(
        description = "Inspect the current workspace's effective leanck configuration without starting Lean. File changes are loaded by set_workspace (including the current workspace); daemon settings take effect on its next startup.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn configuration(&self) -> Result<Content, ErrorData> {
        let client = self.workspace.capture();
        Content::json(
            serde_json::json!({"workspace": client.workspace(), "settings": client.settings(), "warnings": client.settings().warnings()}),
        )
    }

    #[tool(
        description = "Get diagnostics for `file`. Optionally restrict to a single declaration with `ident`. Returns compact human-readable text; pass `verbose: true` for the full JSON response.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn diagnostics(
        &self,
        Parameters(p): Parameters<DiagnosticsInput>,
        cancel: CancellationToken,
    ) -> Result<Content, ErrorData> {
        let client = self.workspace.capture();
        let mut query = DiagnosticsParams::from(&p);
        if p.severity.is_none() {
            query.severity = configured_severity(&client.settings().diagnostics);
        }
        query.limit = query.limit.or(Some(client.settings().diagnostics.limit));
        let body = client
            .diagnostics(
                &query,
                RequestOptions {
                    timeout_ms: p.timeout_ms,
                    cancellation: Some(cancel),
                    ..Default::default()
                },
            )
            .await
            .map_err(to_mcp_error)?;
        if p.verbose.unwrap_or(client.settings().output.json) {
            Content::json(body)
        } else {
            Ok(Content::text(
                leanck_client::render::format_diagnostics_human(&body, &p.file),
            ))
        }
    }

    #[tool(
        description = "Return the Lean proof goal at the given file position (0-indexed line/character).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn goal(
        &self,
        Parameters(p): Parameters<GoalInput>,
        cancel: CancellationToken,
    ) -> Result<Content, ErrorData> {
        let client = self.workspace.capture();
        let body = client
            .goal(
                &GoalParams::from(&p),
                RequestOptions {
                    timeout_ms: p.timeout_ms,
                    cancellation: Some(cancel),
                    ..Default::default()
                },
            )
            .await
            .map_err(to_mcp_error)?;
        Content::json(body)
    }

    #[tool(
        description = "Show declarations that rely on `sorry` under `path`.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn sorry(
        &self,
        Parameters(p): Parameters<SorryInput>,
        cancel: CancellationToken,
    ) -> Result<Content, ErrorData> {
        let client = self.workspace.capture();
        let body = client
            .sorries(
                &SorryParams::from(&p),
                RequestOptions {
                    timeout_ms: p.timeout_ms,
                    cancellation: Some(cancel),
                    ..Default::default()
                },
            )
            .await
            .map_err(to_mcp_error)?;
        Content::json(body)
    }

    #[tool(
        description = "List declarations in `file`. Optionally filter by `kind` (Lean keywords like `theorem`, `def`, `lemma`, `instance`, or LSP kinds like `namespace`, `class`).",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list(
        &self,
        Parameters(p): Parameters<ListInput>,
        cancel: CancellationToken,
    ) -> Result<Content, ErrorData> {
        let client = self.workspace.capture();
        let body = client
            .list(
                &ListParams::from(&p),
                RequestOptions {
                    timeout_ms: p.timeout_ms,
                    cancellation: Some(cancel),
                    ..Default::default()
                },
            )
            .await
            .map_err(to_mcp_error)?;
        Content::json(body)
    }

    #[tool(
        description = "Restart the Lean language server worker for a single file.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn restart_file(
        &self,
        Parameters(p): Parameters<RestartFileInput>,
        cancel: CancellationToken,
    ) -> Result<Content, ErrorData> {
        let client = self.workspace.capture();
        let body = client
            .restart_file(
                &RestartFileParams::from(&p),
                RequestOptions {
                    timeout_ms: p.timeout_ms,
                    cancellation: Some(cancel),
                    ..Default::default()
                },
            )
            .await
            .map_err(to_mcp_error)?;
        Content::json(body)
    }

    #[tool(
        description = "Run `lake build` in the current workspace and report a compact pass/fail log: build success and, on failure, the failing modules with their error messages. Pass `warnings: true` to also include warnings emitted by Lake, including dependency and configuration warnings, `strict: true` to treat warnings as failures, or `json: true` for the full machine-readable report. Optional `targets` scope the build; `timeout_ms` covers queueing and execution (default 30 minutes).",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn build(
        &self,
        Parameters(p): Parameters<BuildInput>,
        cancel: CancellationToken,
    ) -> Result<Content, ErrorData> {
        let client = self.workspace.capture();
        let settings = &client.settings().build;
        let warnings = p.warnings.unwrap_or(settings.warnings);
        let strict = p.strict.unwrap_or(settings.strict);
        let report = leanck_lake::build::run_with(
            client.workspace(),
            leanck_lake::build::BuildOptions {
                warnings,
                strict,
                targets: p.targets.unwrap_or_else(|| settings.targets.clone()),
            },
            leanck_lake::ExecutionOptions {
                timeout: p
                    .timeout_ms
                    .map(std::time::Duration::from_millis)
                    .unwrap_or(std::time::Duration::from_millis(settings.timeout_ms)),
                cancellation: Some(cancel),
                max_output_bytes: p.max_output_bytes.unwrap_or(settings.max_output_bytes),
            },
        )
        .await
        .map_err(|e| {
            let data = Some(serde_json::json!({"kind":e.kind()}));
            if matches!(e, leanck_lake::LakeError::InvalidOptions(_)) {
                ErrorData::invalid_params(e.to_string(), data)
            } else {
                ErrorData::internal_error(e.to_string(), data)
            }
        })?;
        if p.json.unwrap_or(client.settings().output.json) {
            Content::json(report)
        } else {
            Ok(Content::text(leanck_lake::build::format_human(
                &report,
                warnings || strict,
            )))
        }
    }

    #[tool(
        description = "Report Lean server (leanckd) reachability and status. Returns compact text; pass `json: true` for the full payload, which includes `instances`: the machine-wide count of running daemons against `instances.limit`.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn health(
        &self,
        Parameters(p): Parameters<HealthzInput>,
        cancel: CancellationToken,
    ) -> Result<Content, ErrorData> {
        let client = self.workspace.capture();
        let report = HealthReport::from_result(
            client
                .health(RequestOptions {
                    cancellation: Some(cancel),
                    ..Default::default()
                })
                .await,
            &client.socket(),
        )
        .with_instances(leanck_client::instances::summary(client.settings()));
        if p.json.unwrap_or(client.settings().output.json) {
            Content::json(report)
        } else {
            Ok(Content::text(format_health_human(&report)))
        }
    }

    #[tool(
        description = "Search Lean and Mathlib declarations through Loogle. Structured parameters mirror lean-tool: `constant` searches exact constants, `substr` searches quoted name substrings, `subexpr` searches expression patterns, and `conclusion` searches result types with `|-` added when omitted. Returns compact human-readable results.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = true
        )
    )]
    async fn loogle(&self, Parameters(p): Parameters<LoogleInput>) -> Result<Content, ErrorData> {
        let client = self.workspace.capture();
        let loogle_query = LoogleQuery::from(&p);
        let outcome = self
            .loogle
            .configured(&client.settings().loogle)
            .search(
                &loogle_query,
                leanck_loogle::SearchOptions {
                    limit: p.limit.unwrap_or(client.settings().loogle.limit),
                },
            )
            .await
            .map_err(|error| match error {
                leanck_loogle::SearchError::EmptyQuery => {
                    ErrorData::invalid_params(error.to_string(), None)
                }
                _ => ErrorData::internal_error(error.to_string(), None),
            })?;
        Ok(Content::text(leanck_loogle::format_human(&outcome)))
    }

    #[tool(
        description = "Repoint the server at a different Lake project root. Subsequent tool calls run against the new workspace.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn set_workspace(
        &self,
        Parameters(p): Parameters<SetWorkspaceInput>,
    ) -> Result<Content, ErrorData> {
        let candidate = leanck_core::workspace::WorkspaceRoot::validate_project(&p.path)
            .map_err(|e| ErrorData::invalid_params(e.to_string(), None))?;

        let settings = self
            .config_args
            .load(candidate.as_path())
            .map_err(|e| ErrorData::invalid_params(format!("configuration: {e:#}"), None))?
            .settings;
        self.workspace.set_configured(candidate.clone(), settings);
        Content::json(serde_json::json!({
            "ok": true,
            "workspace": candidate.as_path(),
            "socket_path": leanck_core::paths::socket_path(candidate.as_path()),
        }))
    }
}

#[tool_handler]
impl ServerHandler for LeanckServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("leanck-mcp", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "Talks to a Lean 4 worktree via the leanckd daemon, which runs \
                 its own `lake serve` (auto-started on first use; no editor \
                 required). Use the tools to query diagnostics, goals, sorry \
                 locations, and declaration lists against the live elaborator.",
            )
    }
}

fn to_mcp_error(error: ClientError) -> ErrorData {
    let message = error.to_string();
    match error {
        ClientError::Workspace(_) => ErrorData::invalid_params(message, None),
        ClientError::Daemon { kind, .. } => {
            let data = Some(serde_json::json!({ "kind": kind }));
            match kind {
                ErrorKind::BadRequest | ErrorKind::NotFound => {
                    ErrorData::invalid_params(message, data)
                }
                _ => ErrorData::internal_error(message, data),
            }
        }
        ClientError::Cancelled { delivery_uncertain } => ErrorData::internal_error(
            message,
            Some(
                serde_json::json!({"kind": "cancelled", "delivery_uncertain": delivery_uncertain}),
            ),
        ),
        ClientError::Handshake(_) | ClientError::IncompatibleProtocol { .. } => {
            ErrorData::internal_error(
                message,
                Some(serde_json::json!({"kind": "compatibility", "delivery_uncertain": false})),
            )
        }
        ClientError::TransportTimeout { phase } => ErrorData::internal_error(
            message,
            Some(
                serde_json::json!({"kind": "transport_timeout", "phase": phase.to_string(), "delivery_uncertain": phase != TransportPhase::Connect}),
            ),
        ),
        ClientError::Protocol(_) => ErrorData::internal_error(
            message,
            Some(serde_json::json!({ "kind": "protocol_error", "delivery_uncertain": true })),
        ),
        ClientError::Transport { phase, .. } => ErrorData::internal_error(
            message,
            Some(serde_json::json!({
                "phase": match phase { TransportPhase::Connect => "connect", TransportPhase::Write => "write", TransportPhase::Read => "read" },
                "delivery_uncertain": phase != TransportPhase::Connect,
            })),
        ),
        ClientError::InstanceLimit { running, limit, .. } => ErrorData::internal_error(
            message,
            Some(serde_json::json!({"kind": "instance_limit", "running": running, "limit": limit})),
        ),
        ClientError::DaemonExited { code, log } => ErrorData::internal_error(
            message,
            Some(serde_json::json!({"kind": "daemon_exited", "code": code, "log": log})),
        ),
        _ => ErrorData::internal_error(message, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_tool_surface() {
        let server = LeanckServer::new(&Config::defaults(
            leanck_core::workspace::WorkspaceRoot::from_path(std::env::temp_dir()).unwrap(),
        ));
        let mut names: Vec<_> = server
            .tool_router
            .list_all()
            .into_iter()
            .map(|tool| tool.name.into_owned())
            .collect();
        names.sort();
        assert_eq!(
            names,
            [
                "build",
                "configuration",
                "diagnostics",
                "goal",
                "health",
                "list",
                "loogle",
                "restart_file",
                "set_workspace",
                "sorry"
            ]
        );
    }

    #[tokio::test]
    async fn workspace_switch_only_requires_a_lake_marker() {
        let root = std::env::temp_dir().join(format!(
            "leanck-mcp-switch-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let original = std::env::temp_dir().canonicalize().unwrap();
        let server = LeanckServer::new(&Config::defaults(
            leanck_core::workspace::WorkspaceRoot::from_path(&original).unwrap(),
        ));
        let input = || {
            Parameters(SetWorkspaceInput {
                path: root.to_string_lossy().into_owned(),
            })
        };
        assert!(server.set_workspace(input()).await.is_err());
        assert_eq!(server.workspace.capture().workspace(), original);
        for marker in ["lakefile.lean", "lakefile.toml", "lean-toolchain"] {
            std::fs::write(root.join(marker), "").unwrap();
            let response = server.set_workspace(input()).await.unwrap();
            let body: serde_json::Value =
                serde_json::from_str(&response.as_text().unwrap().text).unwrap();
            assert_eq!(body["ok"], true);
            assert_eq!(body["workspace"], root.to_string_lossy().as_ref());
            assert!(body.get("registry").is_none());
            assert_eq!(server.workspace.capture().workspace(), root);
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
            std::fs::remove_file(root.join(marker)).unwrap();
        }
        std::fs::remove_dir(&root).unwrap();
    }
}

#[cfg(test)]
#[path = "../../../test-support/fake_daemon.rs"]
mod fake_daemon;

#[cfg(test)]
#[path = "mcp_client_tests.rs"]
mod client_tests;

fn deserialize_severity<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<SeverityFilter>, D::Error> {
    SeverityFilter::deserialize(deserializer).map(Some)
}

fn configured_severity(settings: &leanck_config::Diagnostics) -> SeverityFilter {
    use leanck_config::Severity as S;
    use leanck_core::api::DiagnosticSeverity as D;
    SeverityFilter::Only {
        severities: settings
            .severity
            .iter()
            .map(|v| match v {
                S::Error => D::Error,
                S::Warning => D::Warning,
                S::Information => D::Information,
                S::Hint => D::Hint,
            })
            .collect(),
    }
}
