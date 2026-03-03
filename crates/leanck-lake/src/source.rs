//! Authoritative project source roots from Lake's own environment, with explicit fallback.
use crate::{CommandOutcome, Lake};
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Debug, Clone)]
pub struct SourceRoots {
    pub paths: Vec<PathBuf>,
    pub resolved: bool,
}
pub async fn discover(project: &Path) -> SourceRoots {
    discover_with(&Lake::default(), project).await
}
pub async fn discover_with(lake: &Lake, project: &Path) -> SourceRoots {
    let project_path = tokio::fs::canonicalize(project)
        .await
        .unwrap_or_else(|_| project.to_owned());
    let project = project_path.as_path();
    let result = lake
        .run(
            project,
            &["env".into(), "printenv".into(), "LEAN_SRC_PATH".into()],
            Some(Duration::from_secs(5)),
        )
        .await;
    let mut paths = Vec::new();
    let mut resolved = false;
    if let Ok(CommandOutcome::Finished(output)) = result {
        if output.success {
            for path in std::env::split_paths(output.stdout.trim()) {
                if path.as_os_str().is_empty() {
                    continue;
                }
                let path = if path.is_absolute() {
                    path
                } else {
                    project.join(path)
                };
                if let Ok(path) = path.canonicalize() {
                    if path.starts_with(project)
                        && !path.components().any(|c| c.as_os_str() == ".lake")
                        && !paths.contains(&path)
                    {
                        paths.push(path);
                    }
                }
            }
            resolved = !paths.is_empty();
        }
    }
    if paths.is_empty() {
        paths.push(project.into());
    }
    SourceRoots { paths, resolved }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        os::unix::fs::PermissionsExt,
        sync::atomic::{AtomicU64, Ordering},
    };
    struct Scratch(PathBuf);
    impl Scratch {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = std::env::temp_dir().join(format!(
                "leanck-roots-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir(&path).unwrap();
            Self(path.canonicalize().unwrap())
        }
        fn lake(&self, script: &str) -> Lake {
            let path = self.0.join("lake");
            std::fs::write(&path, script).unwrap();
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
            Lake::new(path)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    #[tokio::test]
    async fn uses_lake_environment_and_custom_source_roots() {
        let s = Scratch::new();
        std::fs::create_dir(s.0.join("src")).unwrap();
        std::fs::create_dir(s.0.join(".lake")).unwrap();
        let lake=s.lake("#!/bin/sh\n[ \"$1\" = env ] && [ \"$2\" = printenv ] && [ \"$3\" = LEAN_SRC_PATH ] || exit 2\nprintf '%s\\n' \"$PWD/src:$PWD/.lake:/tmp:$PWD/src\"\n");
        let roots = discover_with(&lake, &s.0).await;
        assert!(roots.resolved);
        assert_eq!(roots.paths, [s.0.join("src")]);
    }
    #[tokio::test]
    async fn failure_missing_paths_and_spawn_errors_preserve_conservative_fallback() {
        let s = Scratch::new();
        for script in [
            "#!/bin/sh\nexit 1\n",
            "#!/bin/sh\nprintf '%s\\n' missing\n",
            "#!/bin/sh\nprintf '\\n'\n",
        ] {
            let roots = discover_with(&s.lake(script), &s.0).await;
            assert!(!roots.resolved);
            assert_eq!(roots.paths, [s.0.clone()]);
        }
        let roots = discover_with(&Lake::new(s.0.join("absent")), &s.0).await;
        assert!(!roots.resolved);
    }
}
