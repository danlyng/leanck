use std::path::{Path, PathBuf};

use leanck_loogle::LoogleQuery;
use rmcp::{
    ErrorData, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{Content, Implementation, ServerCapabilities, ServerInfo},
    schemars, tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};

use crate::config::Config;
use crate::daemon::DaemonCache;
use crate::error::Error;

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
    /// Severity filter. Each entry is one of `error`, `warning`, `info`,
    /// `hint`, or `all`. Empty / omitted defaults to `error` only. Use
    /// `["all"]` to disable severity filtering. Diagnostics with the
    /// message `Goals accomplished!` are always excluded.
    #[serde(default)]
    pub severity: Vec<String>,
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
    pub verbose: bool,
    /// Per-request deadline in milliseconds. Also stretches the elaboration
    /// wait to this value. Defaults: 300s request, 120s elaboration wait.
    /// Raise it for slow Mathlib-heavy files.
    #[serde(default)]
    pub timeout_ms: Option<u64>,
}

/// Wire shape sent to the daemon; severity filters are comma-separated.
#[derive(Debug, Serialize)]
struct DiagnosticsQuery<'a> {
    file: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ident: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    severity: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    offset: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    limit: Option<u32>,
}

impl<'a> From<&'a DiagnosticsInput> for DiagnosticsQuery<'a> {
    fn from(input: &'a DiagnosticsInput) -> Self {
        let severity = if input.severity.is_empty() {
            None
        } else {
            Some(input.severity.join(","))
        };
        Self {
            file: &input.file,
            ident: input.ident.as_deref(),
            severity,
            offset: input.offset,
            limit: input.limit,
        }
    }
}

/// Look up the proof goal at a given file position.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct GoalInput {
    /// File (workspace-relative path or `file://` URI).
    pub file: String,
    /// 0-indexed line number.
    pub line: u32,
    /// 0-indexed character column.
    pub character: u32,
    /// Per-request deadline in milliseconds (also stretches the elaboration
    /// wait). Defaults to the daemon's 30s goal-query timeout.
    #[serde(default, skip_serializing)]
    pub timeout_ms: Option<u64>,
}

/// List sorry locations under a file or directory path.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct SorryInput {
    /// Workspace-relative path, absolute path, or `file://` URI. If this
    /// points to a directory, `.lean` files are scanned recursively.
    #[serde(alias = "file")]
    pub path: String,
    /// Per-request deadline in milliseconds (also stretches the brief
    /// elaboration wait before the text-scan fallback).
    #[serde(default, skip_serializing)]
    pub timeout_ms: Option<u64>,
}

/// List declarations in a file.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct ListInput {
    /// File to list (workspace-relative path or `file://` URI).
    pub file: String,
    /// Optional Lean keywords (`theorem`, `def`, `lemma`, `instance`, …) or
    /// LSP kind names (`namespace`, `class`, …) to restrict the result.
    /// Empty/omitted returns every declaration.
    #[serde(default)]
    pub kind: Vec<String>,
    /// Per-request deadline in milliseconds. Also stretches the elaboration
    /// wait to this value. Defaults: 300s request, 120s elaboration wait.
    #[serde(default, skip_serializing)]
    pub timeout_ms: Option<u64>,
}

/// Restart the Lean language server for a single file.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct RestartFileInput {
    /// File whose elaborator should be restarted. Workspace-relative path
    /// or `file://` URI.
    pub file: String,
    /// Per-request deadline in milliseconds (the restart itself plus the
    /// dependency rebuild it triggers). Default 300s.
    #[serde(default, skip_serializing)]
    pub timeout_ms: Option<u64>,
}

/// Run a full `lake build` of the current workspace and report the result.
#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct BuildInput {
    /// Also report build warnings, but only those from libraries/executables
    /// declared in the project's own `lakefile.toml` (dependency warnings such
    /// as Mathlib's are dropped). Default `false`.
    #[serde(default)]
    pub warnings: bool,
    /// If true, treat build warnings as failures and include them in the report.
    /// Default `false`.
    #[serde(default)]
    pub strict: bool,
    /// If true, return the full machine-readable JSON build report instead of
    /// compact text. Default `false`.
    #[serde(default)]
    pub json: bool,
}

#[derive(Debug, Deserialize, Serialize, schemars::JsonSchema)]
pub struct HealthzInput {
    /// If true, return the full machine-readable JSON health payload.
    /// Default `false` returns compact text (`ok` when there are no issues).
    #[serde(default)]
    pub json: bool,
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
    /// Max results to show. Default `10`.
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
    daemon: DaemonCache,
    #[allow(dead_code)]
    tool_router: ToolRouter<Self>,
}

impl LeanckServer {
    pub fn new(config: &Config) -> Self {
        Self {
            daemon: DaemonCache::new(config.workspace.clone()),
            tool_router: Self::tool_router(),
        }
    }
}

/// Call a daemon method, retrying once after a refresh if the first attempt
/// failed with a transport error (e.g. the daemon was idle-reaped and needs
/// respawning). The optional fourth argument forwards a per-request deadline
/// (ms) in the request envelope.
macro_rules! forward {
    ($self:ident, $params:ident, $method:ident) => {
        forward!($self, $params, $method, None)
    };
    ($self:ident, $params:ident, $method:ident, $timeout_ms:expr) => {{
        let timeout_ms: Option<u64> = $timeout_ms;
        let up = $self
            .daemon
            .get()
            .await
            .map_err(to_mcp_error)?
            .with_timeout_ms(timeout_ms);
        let result = match up.$method(&$params).await {
            Ok(v) => Ok(v),
            Err(e) if DaemonCache::should_refresh(&e) => {
                let up = $self
                    .daemon
                    .refresh()
                    .await
                    .map_err(to_mcp_error)?
                    .with_timeout_ms(timeout_ms);
                up.$method(&$params).await
            }
            Err(e) => Err(e),
        };
        let body = result.map_err(to_mcp_error)?;
        Content::json(body)
    }};
}

#[tool_router]
impl LeanckServer {
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
    ) -> Result<Content, ErrorData> {
        let query = DiagnosticsQuery::from(&p);
        let up = self
            .daemon
            .get()
            .await
            .map_err(to_mcp_error)?
            .with_timeout_ms(p.timeout_ms);
        let result = match up.diagnostics(&query).await {
            Ok(v) => Ok(v),
            Err(e) if DaemonCache::should_refresh(&e) => {
                let up = self
                    .daemon
                    .refresh()
                    .await
                    .map_err(to_mcp_error)?
                    .with_timeout_ms(p.timeout_ms);
                up.diagnostics(&query).await
            }
            Err(e) => Err(e),
        };
        let body = result.map_err(to_mcp_error)?;
        if p.verbose {
            Content::json(body)
        } else {
            let workspace = self.daemon.workspace().await;
            Ok(Content::text(leanckd::format::format_diagnostics_human(
                &body, &p.file, &workspace,
            )))
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
    async fn goal(&self, Parameters(p): Parameters<GoalInput>) -> Result<Content, ErrorData> {
        forward!(self, p, goal, p.timeout_ms)
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
    async fn sorry(&self, Parameters(p): Parameters<SorryInput>) -> Result<Content, ErrorData> {
        forward!(self, p, sorries, p.timeout_ms)
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
    async fn list(&self, Parameters(p): Parameters<ListInput>) -> Result<Content, ErrorData> {
        forward!(self, p, list, p.timeout_ms)
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
    ) -> Result<Content, ErrorData> {
        forward!(self, p, restart_file, p.timeout_ms)
    }

    #[tool(
        description = "Run a full `lake build` of the current workspace and report a compact pass/fail log: build success and, on failure, the failing modules with their error messages. Pass `warnings: true` to also include warnings from locally-declared targets (dependency warnings are excluded), `strict: true` to treat warnings as failures, or `json: true` for the full machine-readable report.",
        annotations(
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = true
        )
    )]
    async fn build(&self, Parameters(p): Parameters<BuildInput>) -> Result<Content, ErrorData> {
        let workspace = self.daemon.workspace().await;
        let report = leanckd::build::run(
            &workspace,
            leanckd::build::BuildOptions {
                warnings: p.warnings,
                strict: p.strict,
                ..Default::default()
            },
        )
        .await
        .map_err(|e| ErrorData::internal_error(e.to_string(), None))?;
        if p.json {
            Content::json(report)
        } else {
            Ok(Content::text(leanckd::build::format_human(
                &report,
                p.warnings || p.strict,
            )))
        }
    }

    #[tool(
        description = "Report Lean server (leanckd) reachability and status. Returns compact text; pass `json: true` for the full payload.",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn health(&self, Parameters(p): Parameters<HealthzInput>) -> Result<Content, ErrorData> {
        let lsp_socket_path = self.daemon.discovery_path().await;
        let lsp = match self.daemon.get().await {
            Ok(up) => {
                let result = match up.healthz().await {
                    Ok(v) => Ok(v),
                    Err(e) if DaemonCache::should_refresh(&e) => {
                        let up = self.daemon.refresh().await;
                        match up {
                            Ok(up) => up.healthz().await,
                            Err(e) => Err(e),
                        }
                    }
                    Err(e) => Err(e),
                };
                match result {
                    Ok(v) => normalize_lsp_health(v),
                    Err(e) => serde_json::json!({
                        "ok": false,
                        "reason": e.to_string(),
                    }),
                }
            }
            Err(e) => serde_json::json!({
                "ok": false,
                "reason": e.to_string(),
            }),
        };
        let lsp = with_lsp_socket_path(lsp, &lsp_socket_path);

        let ok = lsp.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
        let mut body = serde_json::json!({
            "ok": ok,
            "lsp": lsp,
        });
        let issues = collect_health_issues(&body);
        if let Some(obj) = body.as_object_mut() {
            obj.insert(
                "issues".to_string(),
                serde_json::Value::Array(
                    issues
                        .iter()
                        .map(|issue| {
                            serde_json::json!({
                                "level": issue.level,
                                "message": issue.message,
                            })
                        })
                        .collect(),
                ),
            );
        }
        if p.json {
            Content::json(body)
        } else {
            Ok(Content::text(format_health_human(&body)))
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
        let loogle_query = LoogleQuery::from(&p);
        let has_multiple_terms = loogle_query.has_multiple_terms();
        let query = loogle_query.build().ok_or_else(|| {
            ErrorData::invalid_params(
                "no Loogle query provided. Use `constant`, `substr`, `subexpr`, or `conclusion`.",
                None,
            )
        })?;
        let limit = p.limit.unwrap_or(leanck_loogle::DEFAULT_LIMIT);
        let mut result = leanck_loogle::search(&query, limit)
            .await
            .map_err(|err| ErrorData::internal_error(err, None))?;
        result.add_empty_multi_term_note(has_multiple_terms);
        Ok(Content::text(leanck_loogle::format_human(&result)))
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
        let raw = PathBuf::from(&p.path);
        let candidate = raw.canonicalize().map_err(|e| {
            ErrorData::invalid_params(
                format!("could not canonicalize workspace path {:?}: {e}", p.path),
                None,
            )
        })?;
        if !candidate.is_dir() {
            return Err(ErrorData::invalid_params(
                format!(
                    "workspace path {:?} is not a directory",
                    candidate.display().to_string()
                ),
                None,
            ));
        }
        let marker = [
            candidate.join("lakefile.lean"),
            candidate.join("lakefile.toml"),
            candidate.join("lean-toolchain"),
        ];
        if !marker.iter().any(|m| m.exists()) {
            return Err(ErrorData::invalid_params(
                format!(
                    "workspace path {:?} has no Lake project marker (lakefile.lean, lakefile.toml, lean-toolchain)",
                    candidate.display().to_string()
                ),
                None,
            ));
        }

        self.daemon.set_workspace(candidate.clone()).await;
        Content::json(serde_json::json!({
            "ok": true,
            "workspace": candidate,
            "socket_path": leanckd::paths::socket_path(&candidate),
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

fn normalize_lsp_health(value: serde_json::Value) -> serde_json::Value {
    match value {
        serde_json::Value::Object(mut obj) => {
            obj.entry("ok").or_insert(serde_json::Value::Bool(true));
            serde_json::Value::Object(obj)
        }
        other => serde_json::json!({
            "ok": true,
            "response": other,
        }),
    }
}

fn with_lsp_socket_path(mut value: serde_json::Value, socket_path: &Path) -> serde_json::Value {
    if let serde_json::Value::Object(obj) = &mut value {
        obj.insert(
            "socket_path".to_string(),
            serde_json::Value::String(socket_path.display().to_string()),
        );
    }
    value
}

fn to_mcp_error(err: Error) -> ErrorData {
    let msg = err.to_string();
    match err {
        Error::Unresolved(_) | Error::InvalidPath(_) => ErrorData::invalid_params(msg, None),
        _ => ErrorData::internal_error(msg, None),
    }
}

#[derive(Debug)]
struct HealthIssue {
    level: &'static str,
    message: String,
}

fn collect_health_issues(body: &serde_json::Value) -> Vec<HealthIssue> {
    let mut issues = Vec::new();
    let lsp = body.get("lsp").unwrap_or(&serde_json::Value::Null);
    if lsp.get("ok").and_then(|v| v.as_bool()) != Some(true) {
        let reason = lsp
            .get("reason")
            .and_then(|v| v.as_str())
            .unwrap_or("not reachable");
        issues.push(HealthIssue {
            level: "error",
            message: format!("lsp: {reason}"),
        });
    }

    issues
}

fn format_health_human(body: &serde_json::Value) -> String {
    let ok = body.get("ok").and_then(|v| v.as_bool()).unwrap_or(false);
    let issues: Vec<String> = body
        .get("issues")
        .and_then(|v| v.as_array())
        .into_iter()
        .flatten()
        .filter_map(|issue| {
            let level = issue.get("level").and_then(|v| v.as_str())?;
            let message = issue.get("message").and_then(|v| v.as_str())?;
            Some(format!("{level}: {message}"))
        })
        .collect();
    if ok && issues.is_empty() {
        "ok".to_string()
    } else if issues.is_empty() {
        "error: health checks failed".to_string()
    } else {
        issues.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_tool_surface() {
        let server = LeanckServer::new(&Config {
            workspace: std::env::temp_dir(),
        });
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

    #[test]
    fn healthy_daemon_needs_no_registry() {
        assert!(collect_health_issues(&serde_json::json!({"lsp": {"ok": true}})).is_empty());
        let issues =
            collect_health_issues(&serde_json::json!({"lsp": {"ok": false, "reason": "offline"}}));
        assert_eq!(issues.len(), 1);
        assert_eq!(issues[0].message, "lsp: offline");
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
        let original = std::env::temp_dir();
        let server = LeanckServer::new(&Config {
            workspace: original.clone(),
        });
        let input = || {
            Parameters(SetWorkspaceInput {
                path: root.to_string_lossy().into_owned(),
            })
        };
        assert!(server.set_workspace(input()).await.is_err());
        assert_eq!(server.daemon.workspace().await, original);
        for marker in ["lakefile.lean", "lakefile.toml", "lean-toolchain"] {
            std::fs::write(root.join(marker), "").unwrap();
            let response = server.set_workspace(input()).await.unwrap();
            let body: serde_json::Value =
                serde_json::from_str(&response.as_text().unwrap().text).unwrap();
            assert_eq!(body["ok"], true);
            assert_eq!(body["workspace"], root.to_string_lossy().as_ref());
            assert!(body.get("registry").is_none());
            assert_eq!(server.daemon.workspace().await, root);
            assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
            std::fs::remove_file(root.join(marker)).unwrap();
        }
        std::fs::remove_dir(&root).unwrap();
    }

    #[test]
    fn health_format_is_minimal_when_clean() {
        let body = serde_json::json!({
            "ok": true,
            "issues": [],
        });

        assert_eq!(format_health_human(&body), "ok");
    }

    #[test]
    fn lsp_health_includes_socket_path_when_status_ok() {
        let lsp = normalize_lsp_health(serde_json::json!({
            "status": "ok",
            "workspace": "/workspace",
        }));
        let lsp = with_lsp_socket_path(lsp, Path::new("/tmp/leanckd-test.sock"));

        assert_eq!(lsp.get("ok").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(lsp.get("status").and_then(|v| v.as_str()), Some("ok"));
        assert_eq!(
            lsp.get("socket_path").and_then(|v| v.as_str()),
            Some("/tmp/leanckd-test.sock")
        );
    }

    #[test]
    fn health_format_lists_issues() {
        let body = serde_json::json!({
            "ok": true,
            "issues": [
                { "level": "warning", "message": "lsp: daemon protocol mismatch" },
                { "level": "warning", "message": "lsp: not reachable" }
            ],
        });

        assert_eq!(
            format_health_human(&body),
            "warning: lsp: daemon protocol mismatch\nwarning: lsp: not reachable"
        );
    }
}
