use crate::Result;
use leanck_core::api::HealthResult;
use serde::{Deserialize, Serialize};
use std::path::Path;

/// Presentation wrapper preserves MCP's existing health JSON and compact text.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthReport {
    pub ok: bool,
    pub lsp: HealthStatus,
    pub issues: Vec<HealthIssue>,
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
        }
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
    fn issue_levels_are_closed_and_preserve_json_and_text() {
        for (level, name) in [
            (HealthIssueLevel::Error, "error"),
            (HealthIssueLevel::Warning, "warning"),
        ] {
            assert_eq!(level.to_string(), name);
            assert_eq!(serde_json::to_value(level).unwrap(), json!(name));
            let issue: HealthIssue =
                serde_json::from_value(json!({"level":name,"message":"detail"})).unwrap();
            assert_eq!(issue.level, level);
            assert_eq!(
                serde_json::to_value(issue).unwrap(),
                json!({"level":name,"message":"detail"})
            );
        }
        for level in [json!("fatal"), json!("Error"), json!(1), json!(null)] {
            assert!(
                serde_json::from_value::<HealthIssue>(json!({"level":level,"message":"detail"}))
                    .is_err()
            );
        }
    }
}
