//! `leanck config init` without guided setup. The prompts themselves are
//! covered by `TestBackend` tests in `leanck-config` (`wizard/screens.rs`).
use std::process::Command;

/// `--json`, `--no-interactive`, and `TERM=dumb` write the starter file
/// without prompting.
#[test]
fn automation_flags_and_dumb_term_skip_the_wizard() {
    let scratch = tempfile::tempdir().unwrap();
    let root = scratch.path();
    let project = root.join("project");
    std::fs::create_dir(&project).unwrap();
    std::fs::write(project.join("lean-toolchain"), "").unwrap();
    for (args, term) in [
        (&["--json"][..], "xterm-256color"),
        (&["--no-interactive"][..], "xterm-256color"),
        (&[][..], "dumb"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_leanck"))
            .args(["config", "init"])
            .args(args)
            .current_dir(&project)
            .env("HOME", root)
            .env("XDG_CONFIG_HOME", root.join("config"))
            .env("XDG_RUNTIME_DIR", root.join("runtime"))
            .env("XDG_CACHE_HOME", root.join("cache"))
            .env("TMPDIR", root)
            .env("TERM", term)
            .env("NO_COLOR", "1")
            .env_remove("LEANCK_CONFIG")
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(output.status.success(), "{args:?}: {stdout}\n{stderr}");
        assert!(!stdout.contains("What would you like"), "{args:?}");
        assert!(!stderr.contains("What would you like"), "{args:?}");
        if args == ["--json"] {
            assert!(stdout.contains("\"created\""), "{stdout}");
        }
        let path = project.join("leanck.toml");
        assert!(path.exists(), "{args:?}");
        std::fs::remove_file(path).unwrap();
    }
}
