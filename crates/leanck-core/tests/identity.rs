use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use leanck_core::document::{DocumentError, DocumentPath, path_to_uri, uri_to_path};
use leanck_core::paths::{RuntimePaths, worktree_key};
use leanck_core::workspace::{PROJECT_MARKERS, WorkspaceError, WorkspaceRoot};

struct Scratch(PathBuf);
impl Scratch {
    fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = PathBuf::from("/tmp").join(format!(
            "leanck-core-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[test]
fn exact_validation_discovery_and_bare_fallback_are_distinct() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    let nested = root.join("a/b");
    std::fs::create_dir_all(&nested).unwrap();
    assert_eq!(WorkspaceRoot::discover(&nested).unwrap().as_path(), nested);
    assert!(matches!(
        WorkspaceRoot::validate_project(root),
        Err(WorkspaceError::NoProjectMarker(_))
    ));
    for marker in PROJECT_MARKERS {
        std::fs::write(root.join(marker), "").unwrap();
        assert_eq!(WorkspaceRoot::discover(&nested).unwrap().as_path(), root);
        assert_eq!(
            WorkspaceRoot::validate_project(root).unwrap().as_path(),
            root
        );
        assert!(matches!(
            WorkspaceRoot::validate_project(&nested),
            Err(WorkspaceError::NoProjectMarker(_))
        ));
        std::fs::remove_file(root.join(marker)).unwrap();
    }
    assert!(matches!(
        WorkspaceRoot::from_path(root.join("missing")),
        Err(WorkspaceError::Canonicalize { .. })
    ));
    std::fs::write(root.join("file"), "").unwrap();
    assert!(matches!(
        WorkspaceRoot::from_path(root.join("file")),
        Err(WorkspaceError::NotDirectory(_))
    ));
}

#[test]
fn equivalent_paths_and_symlinks_have_one_identity() {
    let scratch = Scratch::new();
    let root = &scratch.0;
    std::fs::create_dir(root.join("dir")).unwrap();
    std::fs::write(root.join("Basic.lean"), "").unwrap();
    let original = WorkspaceRoot::from_path(root).unwrap();
    let alternate = WorkspaceRoot::from_path(root.join("dir/..")).unwrap();
    assert_eq!(original, alternate);
    assert_eq!(
        worktree_key(original.as_path()),
        worktree_key(alternate.as_path())
    );
    let document = DocumentPath::resolve(root, "dir/../Basic.lean").unwrap();
    assert_eq!(
        document,
        DocumentPath::resolve(root, &document.uri()).unwrap()
    );
    assert_eq!(
        document,
        DocumentPath::from_path(root.join("Basic.lean")).unwrap()
    );
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(root, root.join("alias")).unwrap();
        assert_eq!(
            original,
            WorkspaceRoot::from_path(root.join("alias")).unwrap()
        );
        assert_eq!(worktree_key(root), worktree_key(&root.join("alias")));
        assert_eq!(
            document,
            DocumentPath::resolve(&root.join("alias"), "Basic.lean").unwrap()
        );
    }
}

#[test]
fn absolute_paths_are_resolved_without_uri_interpretation() {
    let scratch = Scratch::new();
    let workspace = scratch.0.join("workspace");
    std::fs::create_dir(&workspace).unwrap();
    let path = scratch.0.join("file:/space # ? % λ.lean");
    std::fs::create_dir(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "").unwrap();
    // Preserve doubled separators that resemble a URI in an absolute filename.
    let argument = path.to_str().unwrap().replace("file:/", "file://");
    assert_eq!(
        DocumentPath::resolve(&workspace, &argument).unwrap(),
        DocumentPath::from_path(&path).unwrap()
    );
}

#[test]
fn file_uris_round_trip_reserved_characters_and_unicode() {
    let scratch = Scratch::new();
    let path = scratch.0.join("space # ? % λ.lean");
    std::fs::write(&path, "").unwrap();
    let document = DocumentPath::from_path(&path).unwrap();
    let uri = document.uri();
    assert!(uri.contains("space%20%23%20%3F%20%25%20%CE%BB.lean"));
    assert_eq!(uri_to_path(&uri).unwrap(), path);
    assert_eq!(DocumentPath::resolve(&scratch.0, &uri).unwrap(), document);
    let missing = scratch.0.join("missing.lean");
    assert_eq!(
        uri_to_path(&path_to_uri(&missing).unwrap()).unwrap(),
        missing
    );
    assert!(matches!(
        DocumentPath::from_path(&missing),
        Err(DocumentError::Path { .. })
    ));
    assert!(matches!(
        path_to_uri(Path::new("relative.lean")),
        Err(DocumentError::RelativePath(_))
    ));
    for uri in [
        "https://example.com/file",
        "file://remote.example/file",
        "file:relative",
        "file:///tmp/file?query",
        "file:///tmp/file#fragment",
        "not a URI",
    ] {
        assert!(
            matches!(uri_to_path(uri), Err(DocumentError::InvalidUri(_))),
            "{uri}"
        );
    }
}

#[test]
fn runtime_calculation_is_pure_and_hash_and_filenames_stay_stable() {
    let scratch = Scratch::new();
    let absent = scratch.0.join("absent/runtime");
    let paths = RuntimePaths::in_directory(
        absent.clone(),
        Path::new("/__leanck_contract_missing__/workspace"),
    );
    // Independent BLAKE2b-48 vector; this differs from a truncated BLAKE2b-64 digest.
    assert_eq!(
        worktree_key(Path::new("/__leanck_contract_missing__/workspace")),
        "a3b694f716c7"
    );
    assert_eq!(paths.socket(), absent.join("leanck-a3b694f716c7.sock"));
    assert_eq!(paths.pid(), absent.join("leanck-a3b694f716c7.pid"));
    assert_eq!(paths.lock(), absent.join("leanck-a3b694f716c7.lock"));
    assert_eq!(paths.log(), absent.join("leanck-a3b694f716c7.log"));
    assert_eq!(
        paths.start_lock(),
        absent.join("leanck-a3b694f716c7.start.lock")
    );
    assert!(!absent.exists());
    let prepared = paths.clone().prepare().unwrap();
    assert_eq!(prepared, paths);
    assert!(absent.is_dir());
    assert!(!paths.socket().exists());
}

#[test]
fn runtime_preparation_falls_back_without_changing_the_workspace_key() {
    let scratch = Scratch::new();
    let blocked = scratch.0.join("file");
    std::fs::write(&blocked, "").unwrap();
    let original = RuntimePaths::in_directory(blocked.join("runtime"), &scratch.0);
    let prepared = original.clone().prepare().unwrap();
    assert!(
        prepared.directory().starts_with(std::env::temp_dir())
            || prepared.directory().starts_with("/tmp")
    );
    assert_ne!(prepared.directory(), std::env::temp_dir());
    assert_eq!(prepared.socket().file_name(), original.socket().file_name());
    // Existing fallback daemons must also remain visible to non-spawning probes.
    let listener = std::os::unix::net::UnixListener::bind(prepared.socket()).unwrap();
    let discovered = RuntimePaths::for_workspace(&scratch.0);
    drop(listener);
    std::fs::remove_file(prepared.socket()).unwrap();
    assert_eq!(discovered.directory(), prepared.directory());
}
