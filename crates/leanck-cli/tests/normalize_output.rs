#[test]
fn normalizer_commands_emit_source_warnings_on_stderr_and_linkify_without_reflow() {
    use std::{
        io::Write,
        process::{Command, Stdio},
    };
    for (extra, source, expected, warning) in [
        (
            vec!["--no-comment-reflow", "--linkify-urls", "--width", "10"],
            "/-- See https://example.test\non another line. -/\n",
            "/-- See [https://example.test](https://example.test)\non another line. -/\n",
            None,
        ),
        (
            vec![
                "--set",
                "normalize.rules.attributes.overrides={ext='above'}",
            ],
            "@[simp, ext] theorem demo : True := trivial\n",
            "@[simp, ext] theorem demo : True := trivial\n",
            Some("stdin:1: conflicting placement policies"),
        ),
        (
            vec![],
            "-- leanck: on headings\n",
            "-- leanck: on headings\n",
            Some("stdin:1: on headings cannot enable"),
        ),
    ] {
        let mut child = Command::new(env!("CARGO_BIN_EXE_leanck"))
            .arg("normalize")
            .args(["--no-config", "--no-header"])
            .args(extra)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(source.as_bytes())
            .unwrap();
        let output = child.wait_with_output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(String::from_utf8(output.stdout).unwrap(), expected);
        match warning {
            Some(message) => assert!(
                String::from_utf8_lossy(&output.stderr).contains(message),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            ),
            None => assert!(
                output.stderr.is_empty(),
                "{}",
                String::from_utf8_lossy(&output.stderr)
            ),
        }
    }
}
