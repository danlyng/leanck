use crate::{Result, instances::Summary};
use leanck_core::api::HealthResult;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Presentation wrapper preserves MCP's existing health JSON and compact text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub ok: bool,
    pub lsp: HealthStatus,
    pub issues: Vec<HealthIssue>,
    /// Machine-wide daemon count against `instances.limit`, when the caller attaches it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instances: Option<Summary>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum HealthStatus {
    Reachable {
        #[serde(flatten)]
        health: HealthResult,
        socket_path: String,
    },
    Unreachable {
        ok: bool,
        reason: String,
        socket_path: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HealthIssueLevel {
    Error,
    Warning,
}

impl std::fmt::Display for HealthIssueLevel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Error => "error",
            Self::Warning => "warning",
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthIssue {
    pub level: HealthIssueLevel,
    pub message: String,
}

impl HealthReport {
    pub fn from_result(result: Result<HealthResult>, socket: &Path) -> Self {
        let socket_path = socket.display().to_string();
        let (ok, lsp, reason) = match result {
            Ok(health) => {
                let ok = health.ok;
                (
                    ok,
                    HealthStatus::Reachable {
                        health,
                        socket_path,
                    },
                    "not reachable".into(),
                )
            }
            Err(error) => {
                let reason = error.to_string();
                (
                    false,
                    HealthStatus::Unreachable {
                        ok: false,
                        reason: reason.clone(),
                        socket_path,
                    },
                    reason,
                )
            }
        };
        Self {
            ok,
            lsp,
            issues: if ok {
                vec![]
            } else {
                vec![HealthIssue {
                    level: HealthIssueLevel::Error,
                    message: format!("lsp: {reason}"),
                }]
            },
            instances: None,
        }
    }

    pub fn with_instances(mut self, instances: Summary) -> Self {
        self.instances = Some(instances);
        self
    }
}

pub fn format_health_human(report: &HealthReport) -> String {
    if report.issues.is_empty() {
        if report.ok {
            "ok".into()
        } else {
            "error: health checks failed".into()
        }
    } else {
        report
            .issues
            .iter()
            .map(|issue| format!("{}: {}", issue.level, issue.message))
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn instances_are_optional_and_omitted_until_attached() {
        let report = HealthReport::from_result(
            Err(crate::ClientError::StartupTimeout),
            Path::new("/run/leanck.sock"),
        );
        assert!(report.instances.is_none());
        let encoded = serde_json::to_value(&report).unwrap();
        assert!(encoded.get("instances").is_none());
        let decoded: HealthReport = serde_json::from_value(encoded).unwrap();
        assert!(decoded.instances.is_none());

        let report = report.with_instances(Summary {
            running: 2,
            limit: 8,
            on_limit: leanck_config::OnLimit::EvictIdle,
        });
        let human = format_health_human(&report);
        let encoded = serde_json::to_value(&report).unwrap();
        assert_eq!(
            encoded["instances"],
            json!({"running": 2, "limit": 8, "on_limit": "evict_idle"})
        );
        let decoded: HealthReport = serde_json::from_value(encoded).unwrap();
        assert_eq!(decoded.instances, report.instances);
        assert_eq!(
            human,
            "error: lsp: leanckd did not start within the timeout"
        );
    }
}
