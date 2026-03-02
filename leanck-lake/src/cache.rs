//! Mathlib cache fetching reports outcomes; callers decide whether to proceed.
use crate::{CommandOutcome, CommandOutput, ExecutionOptions, Lake, LakeError};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Clone)]
pub struct CacheOptions {
    pub enabled: bool,
    pub execution: ExecutionOptions,
}
impl Default for CacheOptions {
    fn default() -> Self {
        Self {
            enabled: true,
            execution: ExecutionOptions {
                timeout: Duration::from_secs(20 * 60),
                ..Default::default()
            },
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    NotMathlib,
}
#[derive(Debug)]
pub enum CacheOutcome {
    Skipped(SkipReason),
    Completed(CommandOutput),
    Failed(CommandOutput),
    TimedOut,
}

pub async fn fetch(project: &Path, options: CacheOptions) -> Result<CacheOutcome, LakeError> {
    fetch_with(&Lake::default(), project, options).await
}
pub async fn fetch_with(
    lake: &Lake,
    project: &Path,
    options: CacheOptions,
) -> Result<CacheOutcome, LakeError> {
    if !options.enabled {
        return Ok(CacheOutcome::Skipped(SkipReason::Disabled));
    }
    if !depends_on_mathlib(project) {
        return Ok(CacheOutcome::Skipped(SkipReason::NotMathlib));
    }
    match lake
        .coordinated(
            project,
            &["exe".into(), "cache".into(), "get".into()],
            options.execution,
        )
        .await?
    {
        CommandOutcome::Finished(output) if output.success => Ok(CacheOutcome::Completed(output)),
        CommandOutcome::Finished(output) => Ok(CacheOutcome::Failed(output)),
        CommandOutcome::TimedOut => Ok(CacheOutcome::TimedOut),
    }
}

/// Read/parse failure remains a conservative skip, preserving startup policy.
pub fn depends_on_mathlib(project: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(project.join("lake-manifest.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    value
        .get("packages")
        .and_then(|packages| packages.as_array())
        .is_some_and(|packages| {
            packages.iter().any(|package| {
                package
                    .get("name")
                    .and_then(|name| name.as_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("mathlib"))
            })
        })
}
