#[cfg(feature = "loader")]
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// Configuration sections that may only come from user or explicit layers.
/// Register new machine-wide sections here; project filtering and warnings are
/// driven by this list before validation, merging, or provenance recording.
#[cfg(feature = "loader")]
pub(crate) const MACHINE_WIDE_SECTIONS: &[&str] = &["instances", "updates"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Settings {
    pub version: u32,
    pub normalize: Normalize,
    pub loogle: Loogle,
    pub build: Build,
    pub client: Client,
    pub daemon: Daemon,
    pub cache: Cache,
    pub instances: Instances,
    pub diagnostics: Diagnostics,
    pub output: Output,
    pub updates: Updates,
}
impl Default for Settings {
    fn default() -> Self {
        Self {
            version: 1,
            normalize: Normalize::default(),
            loogle: Loogle::default(),
            build: Build::default(),
            client: Client::default(),
            daemon: Daemon::default(),
            cache: Cache::default(),
            instances: Instances::default(),
            diagnostics: Diagnostics::default(),
            output: Output::default(),
            updates: Updates::default(),
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Normalize {
    pub width: usize,
    pub rules: crate::NormalizeRules,
    pub frontmatter: bool,
    pub header: bool,
    pub sort_imports: bool,
    pub attributes: bool,
    pub headings: bool,
    pub comments: bool,
    pub comment_reflow: bool,
    pub linkify_urls: bool,
    pub spelling: Spelling,
    pub capitalize_after_colon: bool,
    pub collapse_blank_lines: bool,
    /// Globs relative to the requested directory; applies only to recursive traversal.
    pub exclude: Vec<String>,
}
impl Default for Normalize {
    fn default() -> Self {
        Self {
            width: 100,
            rules: crate::NormalizeRules::default(),
            frontmatter: true,
            header: true,
            sort_imports: true,
            attributes: true,
            headings: false,
            comments: true,
            comment_reflow: true,
            linkify_urls: false,
            spelling: Spelling::American,
            capitalize_after_colon: false,
            collapse_blank_lines: true,
            exclude: vec![],
        }
    }
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Spelling {
    American,
    British,
    Off,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Loogle {
    pub endpoints: Vec<String>,
    pub timeout_ms: u64,
    pub limit: usize,
}
impl Default for Loogle {
    fn default() -> Self {
        Self {
            endpoints: vec![
                "http://127.0.0.1:8118/loogle/json".into(),
                "https://loogle.lean-lang.org/json".into(),
            ],
            timeout_ms: 15_000,
            limit: 10,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Build {
    pub warnings: bool,
    pub strict: bool,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
    pub targets: Vec<String>,
}
impl Default for Build {
    fn default() -> Self {
        Self {
            warnings: false,
            strict: false,
            timeout_ms: 1_800_000,
            max_output_bytes: 8 * 1024 * 1024,
            targets: vec![],
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Client {
    pub daemon_bin: Option<PathBuf>,
    /// None preserves the daemon's separate operation and wait budgets.
    pub timeout_ms: Option<u64>,
    pub transport_timeout_ms: Option<u64>,
    pub startup_timeout_ms: u64,
    pub startup_grace_ms: u64,
    pub probe_timeout_ms: u64,
}
impl Default for Client {
    fn default() -> Self {
        Self {
            daemon_bin: None,
            timeout_ms: None,
            transport_timeout_ms: None,
            startup_timeout_ms: 1_800_000,
            startup_grace_ms: 30_000,
            probe_timeout_ms: 2_000,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Daemon {
    /// Zero disables idle shutdown; document eviction still runs.
    pub idle_timeout_ms: u64,
    pub max_inflight: usize,
    pub max_documents: usize,
    pub max_source_bytes: usize,
    pub request_timeout_ms: u64,
    pub elaboration_timeout_ms: u64,
    pub lsp_timeout_ms: u64,
    pub permit_timeout_ms: u64,
    pub sorry_timeout_ms: u64,
}
impl Default for Daemon {
    fn default() -> Self {
        Self {
            idle_timeout_ms: 3_600_000,
            max_inflight: 8,
            max_documents: 128,
            max_source_bytes: 64 * 1024 * 1024,
            request_timeout_ms: 300_000,
            elaboration_timeout_ms: 120_000,
            lsp_timeout_ms: 30_000,
            permit_timeout_ms: 30_000,
            sorry_timeout_ms: 5_000,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Cache {
    pub enabled: bool,
    pub timeout_ms: u64,
    pub max_output_bytes: usize,
}
impl Default for Cache {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_ms: 1_200_000,
            max_output_bytes: 8 * 1024 * 1024,
        }
    }
}
/// Machine-wide daemon cap. Read from user-level configuration only; project
/// `leanck.toml` and `.leanck.toml` files have this section ignored with a warning.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Instances {
    /// Maximum concurrently running daemons on this machine; 0 disables the cap.
    pub limit: u64,
    pub on_limit: OnLimit,
}
impl Instances {
    /// The largest accepted `limit`; `leanckd` asserts it equals the slot scan
    /// ceiling `leanck_core::instances::MAX_SLOTS`.
    pub const MAX_LIMIT: u64 = 4096;
}
impl Default for Instances {
    fn default() -> Self {
        Self {
            limit: 8,
            on_limit: OnLimit::EvictIdle,
        }
    }
}
/// What starting a daemon does when `instances.limit` daemons are already running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OnLimit {
    /// Start anyway and print a warning.
    Warn,
    /// Refuse to start another daemon.
    Refuse,
    /// Stop the longest-idle daemon, then start.
    EvictIdle,
}
impl OnLimit {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Warn => "warn",
            Self::Refuse => "refuse",
            Self::EvictIdle => "evict_idle",
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Diagnostics {
    pub limit: u64,
    pub severity: Vec<Severity>,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
    Error,
    Warning,
    Information,
    Hint,
}
impl Default for Diagnostics {
    fn default() -> Self {
        Self {
            limit: 5,
            severity: vec![Severity::Error],
        }
    }
}
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Output {
    pub json: bool,
}
/// User-level release checking and installation policy.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Updates {
    pub check: bool,
    pub check_interval_secs: u64,
    pub automatic: bool,
}
impl Default for Updates {
    fn default() -> Self {
        Self {
            check: true,
            check_interval_secs: 86400,
            automatic: false,
        }
    }
}
#[cfg(feature = "loader")]
impl Settings {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=31_536_000).contains(&self.updates.check_interval_secs),
            "updates.check_interval_secs must be between 1 and 31536000"
        );
        ensure!(
            self.version == 1,
            "unsupported configuration version {}; expected 1",
            self.version
        );
        ensure!(
            self.normalize.width > 0,
            "normalize.width must be greater than zero"
        );
        self.normalize.rules.validate()?;
        for (key, n) in [
            ("build.max_output_bytes", self.build.max_output_bytes),
            ("cache.max_output_bytes", self.cache.max_output_bytes),
            ("daemon.max_documents", self.daemon.max_documents),
            ("daemon.max_source_bytes", self.daemon.max_source_bytes),
        ] {
            ensure!(n > 0, "{key} must be greater than zero");
        }
        ensure!(
            (1..=65536).contains(&self.daemon.max_inflight),
            "daemon.max_inflight must be between 1 and 65536"
        );
        for (key, ms) in [
            ("loogle.timeout_ms", self.loogle.timeout_ms),
            ("build.timeout_ms", self.build.timeout_ms),
            ("cache.timeout_ms", self.cache.timeout_ms),
            ("client.startup_timeout_ms", self.client.startup_timeout_ms),
            ("client.startup_grace_ms", self.client.startup_grace_ms),
            ("client.probe_timeout_ms", self.client.probe_timeout_ms),
            ("daemon.request_timeout_ms", self.daemon.request_timeout_ms),
            (
                "daemon.elaboration_timeout_ms",
                self.daemon.elaboration_timeout_ms,
            ),
            ("daemon.lsp_timeout_ms", self.daemon.lsp_timeout_ms),
            ("daemon.permit_timeout_ms", self.daemon.permit_timeout_ms),
            ("daemon.sorry_timeout_ms", self.daemon.sorry_timeout_ms),
        ] {
            validate_duration(key, ms)?;
        }
        for (key, ms) in [
            ("client.timeout_ms", self.client.timeout_ms),
            (
                "client.transport_timeout_ms",
                self.client.transport_timeout_ms,
            ),
        ] {
            if let Some(ms) = ms {
                validate_duration(key, ms)?;
            }
        }
        ensure!(
            self.instances.limit <= Instances::MAX_LIMIT,
            "instances.limit must be at most {} (0 disables the limit)",
            Instances::MAX_LIMIT
        );
        if self.daemon.idle_timeout_ms != 0 {
            validate_duration("daemon.idle_timeout_ms", self.daemon.idle_timeout_ms)?;
        }
        if let Some(path) = &self.client.daemon_bin {
            ensure!(
                !path.as_os_str().is_empty(),
                "client.daemon_bin must not be empty"
            );
        }
        for pattern in &self.normalize.exclude {
            globset::Glob::new(pattern)
                .map_err(|e| anyhow::anyhow!("invalid normalize.exclude glob {pattern:?}: {e}"))?;
        }
        for endpoint in &self.loogle.endpoints {
            let url = url::Url::parse(endpoint)
                .map_err(|e| anyhow::anyhow!("invalid loogle.endpoints URL {endpoint:?}: {e}"))?;
            ensure!(
                matches!(url.scheme(), "http" | "https") && url.host_str().is_some(),
                "loogle.endpoints requires absolute HTTP(S) URLs"
            );
            ensure!(
                url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "loogle.endpoints URLs must not contain credentials, query strings, or fragments"
            );
        }
        Ok(())
    }
}
#[cfg(feature = "loader")]
fn validate_duration(key: &str, ms: u64) -> Result<()> {
    ensure!(
        (1..=31_536_000_000).contains(&ms),
        "{key} must be between 1 and 31536000000 milliseconds (one year)"
    );
    Ok(())
}
