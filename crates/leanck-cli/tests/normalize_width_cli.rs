use std::{fs, path::PathBuf, process::Command};

struct Scratch(PathBuf);
impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn standalone_unlimited_width_config_flags_and_overrides() {
    let dir = Scratch(
        std::env::temp_dir().join(format!("leanck-normalize-width-{}", std::process::id())),
    );
    fs::create_dir_all(&dir.0).unwrap();
    let path = dir.0.join("Example.lean");
    let source = "/-- The first line\ncontinues with a deliberately long explanation. -/\n";
    for (configured, flags, unlimited) in [
        ("'unlimited'", vec![], true),
        ("'unlimited'", vec!["--width", "20"], false),
        ("20", vec!["--width", "unlimited"], true),
        ("20", vec!["--set", "normalize.width='unlimited'"], true),
        ("'unlimited'", vec!["--set", "normalize.width=20"], false),
    ] {
        fs::write(
            dir.0.join("leanck.toml"),
            format!("[normalize]\nwidth={configured}\nfrontmatter=false\nspelling='off'\n"),
        )
        .unwrap();
        fs::write(&path, source).unwrap();
        let output = normalizer()
            .current_dir(&dir.0)
            .env("XDG_CONFIG_HOME", dir.0.join("user"))
            .env_remove("LEANCK_CONFIG")
            .arg("Example.lean")
            .args(flags)
            .output()
            .unwrap();
        assert!(output.status.success(), "{:?}", output);
        let actual = fs::read_to_string(&path).unwrap();
        assert_eq!(
            actual.contains("The first line continues"),
            unlimited,
            "{actual}"
        );
    }
}

fn normalizer() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_leanck"));
    command.arg("normalize");
    command
}
