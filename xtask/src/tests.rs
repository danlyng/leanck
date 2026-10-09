use std::collections::BTreeSet;
use std::fs;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use super::*;

fn put(path: &Path, contents: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, contents).unwrap();
}

fn executable(path: &Path, contents: &str) {
    put(path, contents);
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

/// A minimal repository checkout with every packaging source.
fn fixture(root: &Path) -> PathBuf {
    let repo = root.join("repo");
    let overlay = repo.join("packaging/plugin");
    put(
        &overlay.join(".claude-plugin/plugin.json"),
        r#"{"name": "leanck", "version": "0.0.0", "description": "Lean — tools"}"#,
    );
    put(
        &overlay.join(".codex-plugin/plugin.json"),
        r#"{"name": "leanck", "version": "0.0.0", "mcpServers": "./.mcp.codex.json"}"#,
    );
    put(
        &overlay.join(".mcp.json"),
        r#"{"mcpServers": {"leanck": {"command": "leanck"}}}"#,
    );
    put(
        &overlay.join(".mcp.codex.json"),
        r#"{"mcpServers": {"leanck": {"command": "leanck"}}}"#,
    );
    put(&overlay.join("README.md"), "readme\n");
    put(&overlay.join(".DS_Store"), "junk");
    put(
        &repo.join("leanck-skills/proving/SKILL.md"),
        "---\nname: proving\ndescription: \"Prove things.\"\n---\n\nBody\n",
    );
    put(
        &repo.join("leanck-skills/proving/reference/notes.md"),
        "notes\n",
    );
    put(
        &repo.join("leanck-skills/proving/__pycache__/x.pyc"),
        "junk",
    );
    let agents = repo.join("leanck-subagents");
    put(&agents.join("both/instructions.md"), "Shared \"\"\" body\n");
    put(
        &agents.join("both/claude.yaml"),
        "model: opus\ndescription: Both hosts.\n",
    );
    put(
        &agents.join("both/codex.toml"),
        "description = \"Both hosts.\"\nmodel = \"m\"\n",
    );
    put(&agents.join("claude-only/instructions.md"), "Claude body\n");
    put(
        &agents.join("claude-only/claude.yaml"),
        "description: Claude only.\n",
    );
    put(&agents.join(".DS_Store"), "junk");
    put(&repo.join("leanck-hooks/hooks.json"), r#"{"hooks": {}}"#);
    put(&repo.join("leanck-hooks/hook.py"), "print()\n");
    executable(&repo.join("install.sh"), "#!/bin/sh\n");
    executable(&repo.join("uninstall.sh"), "#!/bin/sh\n");
    put(&repo.join("docs/leanck.example.toml"), "# example\n");
    repo
}

fn fake_binary(root: &Path, version: &str) -> PathBuf {
    let bin = root.join("bin");
    executable(
        &bin.join("leanck"),
        &format!("#!/bin/sh\necho 'leanck {version}'\n"),
    );
    bin
}

fn files(root: &Path) -> BTreeSet<String> {
    let mut found = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                stack.push(path);
            } else {
                found.insert(
                    path.strip_prefix(root)
                        .unwrap()
                        .to_string_lossy()
                        .into_owned(),
                );
            }
        }
    }
    found
}

fn error(result: Result<impl std::fmt::Debug>) -> String {
    format!("{:#}", result.unwrap_err())
}

#[test]
fn plugin_package_assembles_every_component() {
    let temp = tempfile::tempdir().unwrap();
    let repo = fixture(temp.path());
    let output = temp.path().join("out/plugin");
    let packaged = package_plugin(&repo, None, &output, false).unwrap();
    assert_eq!(packaged.skills, ["proving"]);
    assert_eq!(packaged.claude_agents, ["both", "claude-only"]);
    assert_eq!(packaged.codex_agents, ["both"]);
    assert_eq!(
        files(&output),
        BTreeSet::from(
            [
                ".claude-plugin/plugin.json",
                ".codex-plugin/plugin.json",
                ".leanck-package.json",
                ".mcp.codex.json",
                ".mcp.json",
                "README.md",
                "agents/both.md",
                "agents/claude-only.md",
                "codex/agents/both.toml",
                "hooks/hook.py",
                "hooks/hooks.json",
                "skills/proving/SKILL.md",
                "skills/proving/reference/notes.md",
            ]
            .map(String::from)
        )
    );
    assert_eq!(
        fs::read_to_string(output.join("agents/both.md")).unwrap(),
        "---\nname: both\nmodel: opus\ndescription: Both hosts.\n---\n\nShared \"\"\" body\n"
    );
    assert_eq!(
        fs::read_to_string(output.join("codex/agents/both.toml")).unwrap(),
        "name = \"both\"\ndescription = \"Both hosts.\"\nmodel = \"m\"\n\n\
         developer_instructions = \"\"\"\nShared \\\"\\\"\" body\n\"\"\"\n"
    );
    let marker: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(output.join(MARKER)).unwrap()).unwrap();
    let keys: Vec<&str> = marker
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        keys,
        [
            "generated_by",
            "source",
            "skills",
            "claude_agents",
            "codex_agents",
            "binaries"
        ]
    );
    assert_eq!(marker["generated_by"], "just package-plugin");
    assert_eq!(marker["binaries"], serde_json::json!([]));
    assert!(marker["source"].as_object().unwrap().contains_key("commit"));
    assert_eq!(
        fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o755
    );
    // Regenerating replaces a generated package and leaves no staging behind.
    put(&output.join("stale"), "");
    package_plugin(&repo, None, &output, false).unwrap();
    assert!(!output.join("stale").exists());
    let siblings: Vec<_> = fs::read_dir(output.parent().unwrap()).unwrap().collect();
    assert_eq!(siblings.len(), 1);
}

#[test]
fn plugin_package_copies_the_staged_binary() {
    let temp = tempfile::tempdir().unwrap();
    let repo = fixture(temp.path());
    let output = temp.path().join("plugin");
    let missing = package_plugin(&repo, Some(&temp.path().join("none")), &output, false);
    assert_eq!(
        error(missing),
        format!(
            "{}: staged executable is missing",
            temp.path().join("none/leanck").display()
        )
    );
    assert!(!output.exists());
    let bin = fake_binary(temp.path(), "1.0.0");
    package_plugin(&repo, Some(&bin), &output, false).unwrap();
    assert_eq!(
        fs::metadata(output.join("bin/leanck"))
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o755
    );
    let marker = fs::read_to_string(output.join(MARKER)).unwrap();
    assert!(
        marker.contains("\"binaries\": [\n    \"leanck\"\n  ]"),
        "{marker}"
    );
}

#[test]
fn foreign_outputs_are_never_replaced() {
    let temp = tempfile::tempdir().unwrap();
    let repo = fixture(temp.path());
    let output = temp.path().join("mine");
    put(&output.join("keep"), "user data");
    assert_eq!(
        error(package_plugin(&repo, None, &output, false)),
        format!(
            "{} exists and is not a generated package; remove it manually",
            output.display()
        )
    );
    assert!(output.join("keep").exists());
    let link = temp.path().join("link");
    std::os::unix::fs::symlink(&output, &link).unwrap();
    put(&output.join(MARKER), "{}");
    assert!(
        error(package_plugin(&repo, None, &link, false)).contains("is not a generated package")
    );
    let names: Vec<_> = fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect();
    assert_eq!(
        names.len(),
        3,
        "staging directories were removed: {names:?}"
    );
}

#[test]
fn source_validation_reports_the_offending_file() {
    let temp = tempfile::tempdir().unwrap();
    let repo = fixture(temp.path());
    let output = temp.path().join("plugin");
    let skill = repo.join("leanck-skills/proving/SKILL.md");
    for (text, message) in [
        ("no frontmatter\n", "missing YAML frontmatter".to_owned()),
        (
            "---\nname: proving\n",
            "unterminated YAML frontmatter".to_owned(),
        ),
        (
            "---\nname: proving\n odd\nnot a key\n---\n",
            "malformed frontmatter line 'not a key'".to_owned(),
        ),
        (
            "---\nname: proving\ndescription: ''\n---\n",
            "frontmatter lacks 'description'".to_owned(),
        ),
        (
            "---\nname: other\ndescription: d\n---\n",
            "name 'other' differs from directory".to_owned(),
        ),
    ] {
        put(&skill, text);
        assert_eq!(
            error(package_plugin(&repo, None, &output, false)),
            format!("{}: {message}", skill.display())
        );
    }
    put(&skill, "---\nname: proving\ndescription: d\n---\n");
    let manifest = repo.join("packaging/plugin/.codex-plugin/plugin.json");
    put(&manifest, "{");
    let message = error(package_plugin(&repo, None, &output, false));
    assert!(
        message.starts_with(&format!("{}: invalid JSON: ", manifest.display())),
        "{message}"
    );
    put(&manifest, "{}");
    put(&repo.join("leanck-hooks/hooks.json"), "[");
    assert!(
        error(package_plugin(&repo, None, &output, false)).contains("hooks.json: invalid JSON: ")
    );
    put(&repo.join("leanck-hooks/hooks.json"), "{}");
    let agent = repo.join("leanck-subagents/empty");
    fs::create_dir_all(&agent).unwrap();
    assert_eq!(
        error(package_plugin(&repo, None, &output, false)),
        format!("{}: missing instructions.md", agent.display())
    );
    put(&agent.join("instructions.md"), "body\n");
    assert_eq!(
        error(package_plugin(&repo, None, &output, false)),
        format!(
            "{}: needs claude.yaml, codex.toml, or both",
            agent.display()
        )
    );
    assert!(!output.exists());
}

#[test]
fn release_versions_are_semantic() {
    for valid in [
        "1.2.3",
        "v1.2.3",
        "0.50.0-rc.1",
        "1.0.0+build.5",
        "1.0.0-a+b",
    ] {
        assert_eq!(
            release_version(valid).unwrap(),
            valid.trim_start_matches('v')
        );
    }
    for invalid in [
        "", "v", "1.2", "1.2.3.4", "vv1.2.3", "1.2.3-", "1.2.3 ", "1.a.3", "1.2.3-é",
    ] {
        assert_eq!(
            error(release_version(invalid)),
            "version must be a semantic version, optionally prefixed with v",
            "{invalid}"
        );
    }
}

#[test]
fn json_matches_python_formatting() {
    let value: serde_json::Value =
        serde_json::from_str(r#"{"z": "é😀\u007f", "a": [], "m": {}, "n": [1, true, null]}"#)
            .unwrap();
    assert_eq!(
        json::to_string(&value).unwrap(),
        "{\n  \"z\": \"\\u00e9\\ud83d\\ude00\\u007f\",\n  \"a\": [],\n  \"m\": {},\n  \"n\": [\n    1,\n    true,\n    null\n  ]\n}\n"
    );
}

/// The checks `leanck update` applies when extracting a release archive.
fn archive_entries(archive: &Path) -> Vec<(String, tar::EntryType, u32, Vec<u8>)> {
    let decoder = flate2::read::GzDecoder::new(fs::File::open(archive).unwrap());
    let mut tar = tar::Archive::new(decoder);
    let mut entries = Vec::new();
    for entry in tar.entries().unwrap() {
        let mut entry = entry.unwrap();
        let path = entry.path().unwrap().into_owned();
        assert!(!path.is_absolute());
        assert!(
            !path
                .components()
                .any(|part| matches!(part, std::path::Component::ParentDir))
        );
        assert!(path.starts_with("leanck"), "{}", path.display());
        let kind = entry.header().entry_type();
        assert!(
            kind.is_file() || kind.is_dir(),
            "{} is {kind:?}",
            path.display()
        );
        let mode = entry.header().mode().unwrap();
        let mut data = Vec::new();
        entry.read_to_end(&mut data).unwrap();
        entries.push((path.to_str().unwrap().to_owned(), kind, mode, data));
    }
    entries
}

#[test]
fn release_archive_has_a_single_root_and_keeps_modes() {
    let temp = tempfile::tempdir().unwrap();
    let repo = fixture(temp.path());
    let bin = fake_binary(temp.path(), "1.2.3");
    let root = temp.path().join("dist/leanck");
    fs::create_dir(temp.path().join("dist")).unwrap();
    assert_eq!(
        error(assemble_distribution(
            &repo,
            Some(&bin),
            &root,
            "v1.2.4",
            false
        )),
        "release v1.2.4 does not match binary: 'leanck 1.2.3'"
    );
    assert!(!root.exists());
    assemble_distribution(&repo, Some(&bin), &root, "v1.2.3", false).unwrap();
    let output = temp.path().join("release");
    let archive = write_release_archive(&root, &output, "aarch64-apple-darwin").unwrap();
    assert_eq!(archive, output.join("leanck-aarch64-apple-darwin.tar.gz"));

    let entries = archive_entries(&archive);
    let names: Vec<&str> = entries.iter().map(|(name, ..)| name.as_str()).collect();
    let mut sorted = names.clone();
    sorted.sort();
    assert_eq!(names, sorted, "entries are sorted");
    assert_eq!(names[0], "leanck");
    let entry = |name: &str| {
        entries
            .iter()
            .find(|(n, ..)| n == name)
            .unwrap_or_else(|| panic!("{name}"))
    };
    for name in [
        "leanck/install.sh",
        "leanck/uninstall.sh",
        "leanck/plugin/bin/leanck",
    ] {
        assert_eq!(entry(name).2 & 0o777, 0o755, "{name}");
    }
    assert_eq!(entry("leanck/plugin/README.md").2 & 0o777, 0o644);
    assert_eq!(entry("leanck/.leanck-update-protocol").3, b"3\n");
    assert_eq!(entry("leanck/.leanck-release").3, b"v1.2.3\n");
    assert_eq!(entry("leanck/docs/leanck.example.toml").3, b"# example\n");
    assert_eq!(
        String::from_utf8_lossy(&entry("leanck/plugin/.claude-plugin/plugin.json").3),
        "{\n  \"name\": \"leanck\",\n  \"version\": \"1.2.3\",\n  \"description\": \"Lean \\u2014 tools\"\n}\n"
    );
    assert!(
        String::from_utf8_lossy(&entry("leanck/plugin/.codex-plugin/plugin.json").3)
            .contains("\"version\": \"1.2.3\"")
    );
    assert_eq!(
        String::from_utf8_lossy(&entry("leanck/.claude-plugin/marketplace.json").3),
        "{\n  \"name\": \"leanck\",\n  \"owner\": {\n    \"name\": \"Dan Lyng\"\n  },\n  \"plugins\": [\n    {\n      \"name\": \"leanck\",\n      \"source\": \"./plugin\"\n    }\n  ]\n}\n"
    );
    let codex: serde_json::Value =
        serde_json::from_slice(&entry("leanck/.agents/plugins/marketplace.json").3).unwrap();
    assert_eq!(codex["plugins"][0]["source"]["path"], "./plugin");
    assert_eq!(
        codex["plugins"][0]["policy"]["authentication"],
        "ON_INSTALL"
    );

    let digest = hex::encode(Sha256::digest(fs::read(&archive).unwrap()));
    assert_eq!(
        fs::read_to_string(output.join("leanck-aarch64-apple-darwin.tar.gz.sha256")).unwrap(),
        format!("{digest}  leanck-aarch64-apple-darwin.tar.gz\n")
    );
}

use sha2::{Digest, Sha256};

#[test]
fn long_names_use_pax_headers() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("leanck");
    let long = "d".repeat(120);
    put(&root.join(&long).join("é.txt"), "x");
    let archive =
        write_release_archive(&root, &temp.path().join("out"), "x86_64-apple-darwin").unwrap();
    let names: Vec<String> = archive_entries(&archive)
        .into_iter()
        .map(|entry| entry.0)
        .collect();
    assert_eq!(
        names,
        [
            "leanck".to_owned(),
            format!("leanck/{long}"),
            format!("leanck/{long}/é.txt")
        ]
    );
}

#[test]
fn release_archives_refuse_links() {
    let temp = tempfile::tempdir().unwrap();
    let root = temp.path().join("leanck");
    fs::create_dir(&root).unwrap();
    std::os::unix::fs::symlink("/etc/hosts", root.join("link")).unwrap();
    let message = error(write_release_archive(
        &root,
        &temp.path().join("out"),
        "x86_64-apple-darwin",
    ));
    assert!(
        message.ends_with("archives may hold only regular files and directories"),
        "{message}"
    );
}

#[test]
fn local_packages_must_match_the_approved_inventory() {
    let temp = tempfile::tempdir().unwrap();
    let repo = fixture(temp.path());
    let inventory = temp.path().join("inventory");
    fs::create_dir_all(inventory.join("plugin/skills/proving")).unwrap();
    put(&inventory.join("plugin/agents/both.md"), "");
    put(&inventory.join("plugin/agents/claude-only.md"), "");
    put(&inventory.join("plugin/codex/agents/both.toml"), "");
    let packaged = package_local(
        &repo,
        None,
        &temp.path().join("local"),
        "1.0.0",
        Some(&inventory),
    )
    .unwrap();
    assert!(!temp.path().join("local/plugin/bin").exists());
    assert_eq!(packaged.codex_agents, ["both"]);
    fs::remove_file(inventory.join("plugin/agents/claude-only.md")).unwrap();
    assert_eq!(
        error(package_local(
            &repo,
            None,
            &temp.path().join("local2"),
            "1.0.0",
            Some(&inventory)
        )),
        "local component names changed after configuration; rerun the installer"
    );
    // Without an inventory, as install.sh --from-repo packages, nothing is compared.
    let packaged = package_local(&repo, None, &temp.path().join("local3"), "1.0.0", None).unwrap();
    assert_eq!(packaged.claude_agents, ["both", "claude-only"]);
    assert_eq!(
        fs::read_to_string(temp.path().join("local3/.leanck-update-protocol")).unwrap(),
        "3\n"
    );
}

#[test]
fn repository_sources_package_cleanly() {
    let temp = tempfile::tempdir().unwrap();
    let packaged = package_plugin(&repo_root(), None, &temp.path().join("plugin"), false).unwrap();
    assert!(!packaged.skills.is_empty());
    assert_eq!(packaged.claude_agents, packaged.codex_agents);
}
