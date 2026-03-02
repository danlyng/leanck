use std::process::Command;

#[test]
fn doctor_accepts_all_lake_markers_without_registry_or_daemon() {
    let root = std::env::temp_dir().join(format!(
        "leanck-doctor-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir(&root).unwrap();
    let root = root.canonicalize().unwrap();
    for marker in ["lakefile.lean", "lakefile.toml", "lean-toolchain"] {
        std::fs::write(root.join(marker), "").unwrap();
        let output = Command::new(env!("CARGO_BIN_EXE_leanck"))
            .args(["doctor", "--json", "--workspace"])
            .arg(&root)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(body["ok"], true);
        assert_eq!(body["issues"], serde_json::json!([]));
        assert!(body.get("registry").is_none());
        assert!(!leanckd::paths::socket_path(&root).exists());
        assert_eq!(std::fs::read_dir(&root).unwrap().count(), 1);
        std::fs::remove_file(root.join(marker)).unwrap();
    }
    let output = Command::new(env!("CARGO_BIN_EXE_leanck"))
        .args(["doctor", "--json", "--workspace"])
        .arg(&root)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let body: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(body["ok"], false);
    std::fs::remove_dir(&root).unwrap();
}
