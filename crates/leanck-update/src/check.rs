use crate::{atomic_json, lock_file, now, receipt::Receipt};
use anyhow::{Context, Result, ensure};
use leanck_config::Updates;
use self_update::check_interval::UpdateCheckGuard;
use semver::Version;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, path::PathBuf, time::Duration};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ReleaseSource {
    pub api: String,
    pub releases: String,
}
impl Default for ReleaseSource {
    fn default() -> Self {
        Self {
            api: "https://api.github.com".into(),
            releases: "https://github.com/danlyng/leanck/releases".into(),
        }
    }
}
impl ReleaseSource {
    pub fn validate(&self) -> Result<()> {
        for value in [&self.api, &self.releases] {
            let url = reqwest::Url::parse(value)?;
            let loopback = matches!(url.host_str(), Some("127.0.0.1" | "localhost" | "[::1]"));
            ensure!(
                url.scheme() == "https" || (url.scheme() == "http" && loopback),
                "release source requires HTTPS (HTTP is allowed only on loopback)"
            );
            ensure!(
                url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "invalid release source"
            );
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateStatus {
    pub state: String,
    pub running_version: String,
    pub installed_version: Option<String>,
    pub latest_version: Option<String>,
    pub eligible_version: Option<String>,
    pub last_attempt_at: Option<u64>,
    pub last_success_at: Option<u64>,
    pub stale: bool,
    pub pinned: bool,
    pub installable: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(default)]
struct Cache {
    schema: u32,
    source: Option<ReleaseSource>,
    latest: Option<String>,
    last_attempt_at: Option<u64>,
    last_success_at: Option<u64>,
    error: Option<String>,
}

#[derive(Clone)]
pub struct Checker {
    version: String,
    state: PathBuf,
    source: ReleaseSource,
    receipt: Option<Receipt>,
}
impl Checker {
    pub fn new(version: impl Into<String>, state: PathBuf, source: ReleaseSource) -> Self {
        Self {
            version: version.into(),
            state,
            source,
            receipt: None,
        }
    }
    pub fn for_current(version: &str) -> Result<Self> {
        // An unsupported/legacy receipt cannot authorize installation, but it
        // must not prevent release notices for the running version.
        let receipt = Receipt::discover(&std::env::current_exe()?, None)
            .ok()
            .flatten();
        let source = receipt
            .as_ref()
            .map(|r| r.source.clone())
            .unwrap_or_default();
        Ok(Self {
            version: version.into(),
            state: crate::state_directory()?.join("updates"),
            source,
            receipt,
        })
    }
    pub fn with_receipt(mut self, receipt: Receipt) -> Self {
        self.source = receipt.source.clone();
        self.receipt = Some(receipt);
        self
    }
    fn directory(&self) -> PathBuf {
        let key = hex::encode(Sha256::digest(format!(
            "{}\n{}\nstable",
            self.source.api, self.source.releases
        )));
        self.state.join(&key[..16])
    }
    pub async fn check(&self, policy: &Updates, force: bool) -> UpdateStatus {
        if !policy.check && !force {
            return self.status(&Cache::default(), true, policy);
        }
        let directory = self.directory();
        let read = || {
            fs::read(directory.join("check.json"))
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Cache>(&bytes).ok())
                .filter(|c| c.schema == 1 && c.source.as_ref() == Some(&self.source))
                .filter(|c| {
                    c.latest
                        .as_ref()
                        .is_none_or(|v| Version::parse(v).is_ok_and(|v| v.pre.is_empty()))
                })
                .unwrap_or_default()
        };
        let mut cache = read();
        let result: Result<()> = async {
            self.source.validate()?;
            let file = lock_file(&directory.join("check.lock"))?;
            match file.try_lock() {
                Ok(()) => {}
                Err(std::fs::TryLockError::WouldBlock) => return Ok(()),
                Err(e) => return Err(e.into()),
            }
            cache = read();
            let guard = UpdateCheckGuard::new(
                directory.join("last-check"),
                Duration::from_secs(policy.check_interval_secs),
            );
            if !force && !guard.should_check()? {
                return Ok(());
            }
            guard.record_check()?;
            cache.schema = 1;
            cache.source = Some(self.source.clone());
            cache.last_attempt_at = Some(now());
            atomic_json(&directory.join("check.json"), &cache)?;
            let query = async {
                let updater = self_update::backends::github::Update::configure()
                    .repo_owner("danlyng")
                    .repo_name("leanck")
                    .bin_name("leanck")
                    .api_base_url(self.source.api.trim_end_matches('/'))
                    .current_version(&self.version)
                    .timeout(Duration::from_secs(2))
                    .no_confirm(true)
                    .show_output(false)
                    .build_async()?;
                let releases = updater.get_latest_release_async().await?;
                let release = releases.latest().context("no published stable release")?;
                let version = Version::parse(release.version())?;
                ensure!(version.pre.is_empty(), "latest release is a prerelease");
                Ok::<_, anyhow::Error>(version.to_string())
            };
            match tokio::time::timeout(Duration::from_secs(2), query).await {
                Ok(Ok(version)) => {
                    cache.latest = Some(version);
                    cache.last_success_at = Some(now());
                    cache.error = None;
                }
                Ok(Err(e)) => cache.error = Some(format!("{e:#}").chars().take(512).collect()),
                Err(_) => cache.error = Some("release check timed out after 2 seconds".into()),
            }
            atomic_json(&directory.join("check.json"), &cache)?;
            Ok(())
        }
        .await;
        if let Err(e) = result {
            cache.error = Some(format!("{e:#}").chars().take(512).collect());
        }
        self.status(&cache, false, policy)
    }
    fn status(&self, cache: &Cache, disabled: bool, policy: &Updates) -> UpdateStatus {
        let receipt = self.receipt.as_ref();
        let pinned = receipt.is_some_and(|r| r.pin.is_some());
        let installed = receipt.map(|r| r.version.clone());
        let current = installed.as_deref().unwrap_or(&self.version);
        let newer = cache
            .latest
            .as_deref()
            .and_then(|v| Some(Version::parse(v).ok()? > Version::parse(current).ok()?))
            .unwrap_or(false);
        let installable = receipt.is_some_and(|r| r.provenance == "release");
        let stale = cache.error.is_some()
            || cache
                .last_success_at
                .is_none_or(|t| now().saturating_sub(t) >= policy.check_interval_secs || t > now());
        let mut notice = if !disabled && newer {
            Some(format!(
                "leanck {} is available (installed {}); {}",
                cache.latest.as_deref().unwrap(),
                current,
                if pinned {
                    "installation is pinned; use `leanck update --to-version latest` to unpin"
                } else if installable {
                    "run `leanck update`"
                } else {
                    "update through your installation method or adopt this installation"
                }
            ))
        } else {
            None
        };
        if !disabled && installed.as_ref().is_some_and(|v| v != &self.version) {
            let message = format!(
                "running leanck {} differs from installed {}; restart this process",
                self.version, current
            );
            notice = Some(notice.map(|n| format!("{n}; {message}")).unwrap_or(message));
        }
        if !disabled && receipt.is_some_and(|r| r.registration_pending) {
            let message =
                "host registration is incomplete; run `leanck update --retry-registration --yes`";
            notice = Some(
                notice
                    .map(|n| format!("{n}; {message}"))
                    .unwrap_or_else(|| message.into()),
            );
        }
        UpdateStatus {
            state: if disabled {
                "disabled"
            } else if newer {
                if pinned { "pinned" } else { "available" }
            } else if cache.error.is_some() {
                "check_failed"
            } else if cache.latest.is_some() {
                "current"
            } else {
                "unknown"
            }
            .into(),
            running_version: self.version.clone(),
            installed_version: installed,
            latest_version: if disabled { None } else { cache.latest.clone() },
            eligible_version: if disabled || pinned || !newer {
                None
            } else {
                cache.latest.clone()
            },
            last_attempt_at: cache.last_attempt_at,
            last_success_at: cache.last_success_at,
            stale,
            pinned,
            installable,
            notice,
            error: if disabled { None } else { cache.error.clone() },
        }
    }
}

pub async fn current_status(version: &str, policy: &Updates) -> UpdateStatus {
    match Checker::for_current(version) {
        Ok(checker) => checker.check(policy, false).await,
        Err(error) => {
            let checker = Checker::new(version, PathBuf::new(), ReleaseSource::default());
            checker.status(
                &Cache {
                    error: Some(error.to_string()),
                    ..Default::default()
                },
                !policy.check,
                policy,
            )
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{Server, scratch};
    use std::sync::atomic::Ordering;

    #[tokio::test]
    async fn persistent_notices_disabled_checks_failures_and_clock_skew() {
        let (_dir, root) = scratch();
        let server =
            Server::new(br#"{"tag_name":"v9.0.0","created_at":"2026-01-01","assets":[]}"#.to_vec())
                .await;
        let source = ReleaseSource {
            api: server.url.clone(),
            releases: server.url.clone(),
        };
        let checker = Checker::new("1.0.0", root.join("state"), source);
        let policy = Updates::default();
        let first = checker.check(&policy, false).await;
        assert_eq!(first.state, "available", "{first:?}");
        assert!(first.notice.is_some());
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
        let cached = checker.check(&policy, false).await;
        assert_eq!(cached.notice, first.notice);
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
        let disabled = checker
            .check(
                &Updates {
                    check: false,
                    ..policy.clone()
                },
                false,
            )
            .await;
        assert_eq!(disabled.state, "disabled");
        assert!(disabled.notice.is_none());
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
        *server.body.lock().unwrap() = (503, b"unavailable".to_vec());
        let failed = checker.check(&policy, true).await;
        assert_eq!(failed.notice, first.notice);
        assert!(failed.stale);
        assert!(failed.error.is_some());
        let count = server.calls.load(Ordering::SeqCst);
        checker.check(&policy, false).await;
        assert_eq!(
            server.calls.load(Ordering::SeqCst),
            count,
            "failures must be throttled"
        );
        for content in ["99999999999", "not a timestamp", "1"] {
            fs::write(checker.directory().join("last-check"), content).unwrap();
            checker.check(&policy, false).await;
        }
        assert_eq!(server.calls.load(Ordering::SeqCst), count + 3);
    }

    #[tokio::test]
    async fn concurrent_checks_and_source_isolation() {
        let (_dir, root) = scratch();
        let server =
            Server::new(br#"{"tag_name":"v2.0.0","created_at":"2026-01-01","assets":[]}"#.to_vec())
                .await;
        let checker = Checker::new(
            "1.0.0",
            root.clone(),
            ReleaseSource {
                api: server.url.clone(),
                releases: server.url.clone(),
            },
        );
        let policy = Updates::default();
        tokio::join!(checker.check(&policy, false), checker.check(&policy, false));
        assert_eq!(server.calls.load(Ordering::SeqCst), 1);
        let other = Checker::new(
            "1.0.0",
            root,
            ReleaseSource {
                api: server.url.clone(),
                releases: format!("{}/mirror", server.url),
            },
        );
        assert_ne!(other.directory(), checker.directory());
        other.check(&policy, false).await;
        assert_eq!(server.calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn corrupt_metadata_does_not_look_current_or_bypass_recent_attempt() {
        let (_dir, root) = scratch();
        let checker = Checker::new("1.0.0", root, ReleaseSource::default());
        fs::create_dir_all(checker.directory()).unwrap();
        fs::write(checker.directory().join("check.json"), "broken").unwrap();
        UpdateCheckGuard::new(
            checker.directory().join("last-check"),
            Duration::from_secs(86400),
        )
        .record_check()
        .unwrap();
        assert_eq!(
            checker.check(&Updates::default(), false).await.state,
            "unknown"
        );
    }

    #[tokio::test]
    async fn prereleases_are_not_offered() {
        let (_dir, root) = scratch();
        let server = Server::new(
            br#"{"tag_name":"v9.0.0-beta.1","created_at":"2026-01-01","assets":[]}"#.to_vec(),
        )
        .await;
        let checker = Checker::new(
            "1.0.0",
            root,
            ReleaseSource {
                api: server.url.clone(),
                releases: server.url.clone(),
            },
        );
        let status = checker.check(&Updates::default(), false).await;
        assert_eq!(status.state, "check_failed");
        assert!(status.notice.is_none());
    }
}
