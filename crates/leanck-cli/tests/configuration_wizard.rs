//! Exercise prompts through a real terminal, including raw-mode cleanup.
#[test]
#[cfg(unix)]
fn guided_configuration_in_a_terminal() {
    let output = std::process::Command::new("python3")
        .arg(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../test-support/test_config_wizard.py"
        ))
        .arg(env!("CARGO_BIN_EXE_leanck"))
        .output()
        .expect("configuration terminal tests require python3");
    assert!(
        output.status.success(),
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}
