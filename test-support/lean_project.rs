//! Disposable copies of the shared Lean project for live tests, ignored by default.
//!
//! ```sh
//! cargo test -p leanck-lake --test live --locked -- --ignored
//! cargo test -p leanck-cli --test live --locked -- --ignored
//! ```
//!
//! Set `LEANCK_TEST_PROJECT` to optionally override the in-repo template.
#![allow(dead_code)] // Each integration suite uses a different subset.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

pub struct LeanProject {
    root: PathBuf,
}

impl LeanProject {
    pub fn new() -> Self {
        let template = std::env::var_os("LEANCK_TEST_PROJECT").unwrap_or_else(|| {
            concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/../../test-support/lean-project"
            )
            .into()
        });
        let template = PathBuf::from(template)
            .canonicalize()
            .expect("valid test template");
        assert!(
            std::process::Command::new("lake")
                .arg("--version")
                .current_dir(&template)
                .output()
                .is_ok_and(|output| output.status.success()),
            "live tests need `lake` on PATH (install elan)"
        );
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let root = std::env::temp_dir().join(format!(
            "lck-test-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // Refuse an existing identity, rather than risk reusing someone else's daemon.
        std::fs::create_dir(&root).expect("unique test workspace");
        let project = Self {
            root: root.canonicalize().unwrap(),
        };
        copy_tree(&template, project.path());
        project
    }

    pub fn path(&self) -> &Path {
        &self.root
    }
}

impl Drop for LeanProject {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(self.path());
    }
}

fn copy_tree(from: &Path, to: &Path) {
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        if [".git", ".lake", "target", "build"]
            .iter()
            .any(|name| entry.file_name() == *name)
        {
            continue;
        }
        let dest = to.join(entry.file_name());
        let kind = entry.file_type().unwrap();
        if kind.is_dir() {
            std::fs::create_dir(&dest).unwrap();
            copy_tree(&entry.path(), &dest);
        } else if kind.is_file() {
            std::fs::copy(entry.path(), dest).unwrap();
        }
    }
}
