//! Best-effort Mathlib cache warmup.
//!
//! On a fresh worktree the warm `lake serve` would build Mathlib from source on
//! the first elaboration. Fetching the prebuilt olean cache first
//! (`lake exe cache get`, the standard Mathlib workflow) avoids that. This is
//! strictly best-effort: any failure (offline, non-Mathlib project, missing
//! `cache` exe) is logged and ignored — the caller spawns the server regardless.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncRead, BufReader};
use tokio::process::Command;

/// Generous ceiling: a cold fetch downloads a lot of oleans. Far less than a
/// source build, but still minutes on a slow link.
const CACHE_GET_TIMEOUT: Duration = Duration::from_secs(20 * 60);

/// Set to any value to skip the `lake exe cache get` warmup (e.g. when the
/// cache is managed externally, or the machine is offline).
const OPT_OUT_ENV: &str = "LEANCKD_NO_CACHE_GET";

/// Run `lake exe cache get` once for `worktree` when it depends on Mathlib, so
/// the warm `lake serve` reuses the prebuilt oleans instead of rebuilding
/// Mathlib from source. Best-effort; never errors.
pub async fn ensure_mathlib_cache(worktree: &Path) {
    if std::env::var_os(OPT_OUT_ENV).is_some() {
        tracing::debug!("{OPT_OUT_ENV} set; skipping `lake exe cache get`");
        return;
    }
    if !depends_on_mathlib(worktree) {
        return;
    }

    tracing::info!(
        worktree = %worktree.display(),
        "fetching Mathlib cache (`lake exe cache get`) before warmup; this can take a while on a fresh checkout"
    );

    let spawn = Command::new("lake")
        .args(["exe", "cache", "get"])
        .current_dir(worktree)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn();

    let mut child = match spawn {
        Ok(child) => child,
        Err(e) => {
            tracing::warn!(
                error = %e,
                "could not run `lake exe cache get`; falling back to building from source"
            );
            return;
        }
    };

    if let Some(stdout) = child.stdout.take() {
        tokio::spawn(drain(stdout, "stdout"));
    }
    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(drain(stderr, "stderr"));
    }

    match tokio::time::timeout(CACHE_GET_TIMEOUT, child.wait()).await {
        Ok(Ok(status)) if status.success() => {
            tracing::info!("`lake exe cache get` completed");
        }
        Ok(Ok(status)) => {
            tracing::warn!(
                %status,
                "`lake exe cache get` failed; falling back to building from source"
            );
        }
        Ok(Err(e)) => {
            tracing::warn!(
                error = %e,
                "`lake exe cache get` could not be awaited; falling back to building from source"
            );
        }
        Err(_) => {
            // `child` is dropped at function exit; `kill_on_drop` reaps it.
            tracing::warn!(
                timeout_secs = CACHE_GET_TIMEOUT.as_secs(),
                "`lake exe cache get` timed out; falling back to building from source"
            );
        }
    }
}

/// Forward a child's output stream into the trace log at debug level so a long
/// download isn't silent.
async fn drain<R: AsyncRead + Unpin + Send + 'static>(reader: R, stream: &'static str) {
    let mut lines = BufReader::new(reader).lines();
    while let Ok(Some(line)) = lines.next_line().await {
        tracing::debug!(target: "lake_cache", stream, "{line}");
    }
}

/// Whether `worktree/lake-manifest.json` lists a `mathlib` package. Tolerant of
/// manifest shape; on any read/parse failure returns `false`, since we only
/// want to run `cache get` when we're confident Mathlib is present.
fn depends_on_mathlib(worktree: &Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(worktree.join("lake-manifest.json")) else {
        return false;
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return false;
    };
    value
        .get("packages")
        .and_then(|p| p.as_array())
        .is_some_and(|packages| {
            packages.iter().any(|pkg| {
                pkg.get("name")
                    .and_then(|n| n.as_str())
                    .is_some_and(|name| name.eq_ignore_ascii_case("mathlib"))
            })
        })
}

#[cfg(test)]
mod tests {
    use super::depends_on_mathlib;
    use std::fs;

    fn write_manifest(dir: &std::path::Path, contents: &str) {
        fs::write(dir.join("lake-manifest.json"), contents).unwrap();
    }

    #[test]
    fn detects_mathlib_package() {
        let dir = std::env::temp_dir().join(format!("leanckd-cache-test-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // No manifest → false.
        assert!(!depends_on_mathlib(&dir));

        write_manifest(
            &dir,
            r#"{"packages":[{"name":"batteries"},{"name":"Mathlib"}]}"#,
        );
        assert!(depends_on_mathlib(&dir), "case-insensitive mathlib match");

        write_manifest(&dir, r#"{"packages":[{"name":"batteries"}]}"#);
        assert!(!depends_on_mathlib(&dir));

        write_manifest(&dir, "not json");
        assert!(!depends_on_mathlib(&dir));

        fs::remove_dir_all(&dir).unwrap();
    }
}
