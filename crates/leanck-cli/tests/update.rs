#[path = "../../../test-support/mcp.rs"]
mod mcp;

use leanck_update::{
    ReleaseSource,
    gate::{Lease, Maintenance},
    receipt::{Component, Receipt, digest_file},
};
use serde_json::json;
use std::{
    fs,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    receipt: Receipt,
    calls: Arc<AtomicUsize>,
    latest: Arc<std::sync::Mutex<String>>,
    bad_checksum: Arc<std::sync::atomic::AtomicBool>,
    server: tokio::task::JoinHandle<()>,
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.server.abort();
    }
}
impl Fixture {
    async fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        fs::create_dir(root.join("bin")).unwrap();
        let binary = root.join("bin/leanck");
        fs::copy(env!("CARGO_BIN_EXE_leanck"), &binary).unwrap();
        #[cfg(target_os = "macos")]
        assert!(
            std::process::Command::new("/usr/bin/codesign")
                .args(["--force", "-s", "-"])
                .arg(&binary)
                .status()
                .unwrap()
                .success()
        );
        let package = root.join("package");
        fs::create_dir_all(package.join("plugin/bin")).unwrap();
        fs::copy(&binary, package.join("plugin/bin/leanck")).unwrap();
        fs::create_dir_all(package.join("plugin/skills/prove")).unwrap();
        fs::write(
            package.join("plugin/skills/prove/SKILL.md"),
            "updated proof skill",
        )
        .unwrap();
        fs::create_dir_all(package.join("plugin/codex/agents")).unwrap();
        fs::write(
            package.join("plugin/codex/agents/first.toml"),
            "updated first agent",
        )
        .unwrap();
        fs::write(
            package.join("plugin/codex/agents/second.toml"),
            "new second agent",
        )
        .unwrap();
        for host in ["claude", "codex"] {
            fs::create_dir_all(package.join(format!("plugin/.{host}-plugin"))).unwrap();
            let mut manifest = json!({"version":env!("CARGO_PKG_VERSION")});
            if host == "codex" {
                manifest["mcpServers"] = "./.mcp.codex.json".into();
            }
            fs::write(
                package.join(format!("plugin/.{host}-plugin/plugin.json")),
                serde_json::to_vec(&manifest).unwrap(),
            )
            .unwrap();
        }
        fs::write(
            package.join("plugin/.mcp.codex.json"),
            br#"{"mcpServers":{"leanck":{"command":"leanck"}}}"#,
        )
        .unwrap();
        fs::write(package.join("plugin/.mcp.json"), b"{}").unwrap();
        fs::write(
            package.join(".leanck-release"),
            format!("v{}\n", env!("CARGO_PKG_VERSION")),
        )
        .unwrap();
        fs::write(package.join(".leanck-update-protocol"), "1\n").unwrap();
        let archive = root.join("release.tar.gz");
        let encoder = flate2::write::GzEncoder::new(
            fs::File::create(&archive).unwrap(),
            flate2::Compression::fast(),
        );
        let mut builder = tar::Builder::new(encoder);
        builder.append_dir_all("leanck", &package).unwrap();
        builder.into_inner().unwrap().finish().unwrap();
        let checksum = format!(
            "{}  leanck-{}.tar.gz\n",
            digest_file(&archive).unwrap(),
            leanck_update::install::target().unwrap()
        );
        fs::write(root.join("release.tar.gz.sha256"), &checksum).unwrap();
        let archive = Arc::new(fs::read(&archive).unwrap());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let counts = calls.clone();
        let latest = Arc::new(std::sync::Mutex::new("1000.0.0".to_owned()));
        let reported_version = latest.clone();
        let bad_checksum = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let checksum_failure = bad_checksum.clone();
        let server = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut bytes = [0; 8192];
                let n = socket.read(&mut bytes).await.unwrap_or(0);
                counts.fetch_add(1, Ordering::SeqCst);
                let request = String::from_utf8_lossy(&bytes[..n]);
                let path = request.split_whitespace().nth(1).unwrap_or("");
                let data = if path.ends_with(".sha256") {
                    if checksum_failure.load(Ordering::SeqCst) {
                        format!(
                            "{}  leanck-{}.tar.gz\n",
                            "0".repeat(64),
                            leanck_update::install::target().unwrap()
                        )
                        .into_bytes()
                    } else {
                        checksum.as_bytes().to_vec()
                    }
                } else if path.ends_with(".tar.gz") {
                    archive.as_ref().clone()
                } else {
                    serde_json::to_vec(&json!({"tag_name":format!("v{}", reported_version.lock().unwrap()),"created_at":"2026-10-19","assets":[]})).unwrap()
                };
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    data.len()
                );
                if socket.write_all(header.as_bytes()).await.is_ok() {
                    let _ = socket.write_all(&data).await;
                }
            }
        });
        let skill = root.join("skills/prove");
        fs::create_dir_all(&skill).unwrap();
        fs::write(skill.join("SKILL.md"), "old skill").unwrap();
        let receipt = Receipt {
            schema: 1,
            generation: 1,
            version: "0.34.0".into(),
            provenance: "release".into(),
            target: leanck_update::install::target().unwrap(),
            mode: "standalone".into(),
            binary: binary.clone(),
            state_dir: root.join("installer"),
            bundle: None,
            source: ReleaseSource {
                api: url.clone(),
                releases: url.clone(),
            },
            pin: None,
            hosts: vec![],
            registration_pending: false,
            components: vec![
                Component {
                    kind: "file".into(),
                    destination: binary.clone(),
                    source: "plugin/bin/leanck".into(),
                    fingerprint: digest_file(&binary).unwrap(),
                },
                Component {
                    kind: "tree".into(),
                    destination: skill.clone(),
                    source: "plugin/skills/prove".into(),
                    fingerprint: leanck_update::receipt::tree_digest(&skill).unwrap(),
                },
            ],
        };
        receipt.write().unwrap();
        Self {
            _dir: dir,
            root,
            receipt,
            calls,
            latest,
            bad_checksum,
            server,
        }
    }
    fn command(&self) -> tokio::process::Command {
        let mut command = tokio::process::Command::new(&self.receipt.binary);
        command
            .current_dir(&self.root)
            .env("HOME", &self.root)
            .env("XDG_STATE_HOME", self.root.join("state"))
            .env("XDG_CONFIG_HOME", self.root.join("config"))
            .env("XDG_CACHE_HOME", self.root.join("cache"))
            .env("TMPDIR", &self.root)
            .args(["--no-config", "--set", "updates.check=false"]);
        command
    }
    async fn run(&self, args: &[&str]) -> std::process::Output {
        self.command().args(args).output().await.unwrap()
    }
}

#[tokio::test]
async fn checksum_failure_preserves_every_installed_component() {
    let fixture = Fixture::new().await;
    fixture.bad_checksum.store(true, Ordering::SeqCst);
    let before = digest_file(&fixture.receipt.binary).unwrap();
    let result = fixture
        .run(&[
            "update",
            "--version",
            env!("CARGO_PKG_VERSION"),
            "--yes",
            "--json",
        ])
        .await;
    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("checksum mismatch"));
    assert_eq!(digest_file(&fixture.receipt.binary).unwrap(), before);
    assert_eq!(
        Receipt::discover(&fixture.receipt.binary, None)
            .unwrap()
            .unwrap()
            .generation,
        1
    );
    assert_eq!(
        fs::read_to_string(fixture.root.join("skills/prove/SKILL.md")).unwrap(),
        "old skill"
    );
}

#[tokio::test]
async fn partial_host_registration_is_recorded_and_can_be_retried() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new().await;
    let commands = fixture.root.join("host-commands");
    fs::create_dir(&commands).unwrap();
    let stub = commands.join("codex");
    fs::write(
        &stub,
        "#!/bin/sh\necho simulated registration failure >&2\nexit 1\n",
    )
    .unwrap();
    fs::set_permissions(&stub, fs::Permissions::from_mode(0o755)).unwrap();
    let bundle = fixture.root.join("data/bundle");
    fs::create_dir_all(bundle.join("plugin/bin")).unwrap();
    fs::copy(&fixture.receipt.binary, bundle.join("plugin/bin/leanck")).unwrap();
    let mut receipt = fixture.receipt.clone();
    receipt.mode = "plugin".into();
    receipt.bundle = Some(bundle.clone());
    receipt.hosts.push(leanck_update::receipt::Host {
        name: "codex".into(),
        home: fixture.root.join("codex-home"),
    });
    receipt.components.push(Component {
        kind: "tree".into(),
        destination: bundle.clone(),
        source: ".".into(),
        fingerprint: leanck_update::receipt::tree_digest(&bundle).unwrap(),
    });
    receipt.write().unwrap();
    let path = format!("{}:{}", commands.display(), std::env::var("PATH").unwrap());
    let result = fixture
        .command()
        .env("PATH", &path)
        .args([
            "update",
            "--version",
            env!("CARGO_PKG_VERSION"),
            "--yes",
            "--json",
        ])
        .output()
        .await
        .unwrap();
    assert!(!result.status.success());
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(value["status"], "partial");
    let updated = Receipt::discover(&fixture.receipt.binary, None)
        .unwrap()
        .unwrap();
    assert!(updated.registration_pending);
    assert_eq!(updated.version, env!("CARGO_PKG_VERSION"));
    fs::write(&stub, "#!/bin/sh\nexit 0\n").unwrap();
    let result = fixture
        .command()
        .env("PATH", &path)
        .args(["update", "--retry-registration", "--yes", "--json"])
        .output()
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(
        !Receipt::discover(&fixture.receipt.binary, None)
            .unwrap()
            .unwrap()
            .registration_pending
    );
}

#[tokio::test]
async fn native_updates_preserve_disabled_and_enabled_plugin_mcp() {
    for enabled in [false, true] {
        let fixture = Fixture::new().await;
        let bundle = fixture.root.join("data/bundle");
        fs::create_dir_all(bundle.join("plugin")).unwrap();
        let name = if enabled {
            ".mcp.codex.json"
        } else {
            ".mcp.codex.disabled.json"
        };
        fs::write(bundle.join("plugin").join(name), b"{}").unwrap();
        let mut receipt = fixture.receipt.clone();
        receipt.mode = "plugin".into();
        receipt.bundle = Some(bundle.clone());
        receipt.components.push(Component {
            kind: "tree".into(),
            destination: bundle.clone(),
            source: ".".into(),
            fingerprint: leanck_update::receipt::tree_digest(&bundle).unwrap(),
        });
        receipt.write().unwrap();
        let result = fixture
            .run(&[
                "update",
                "--version",
                env!("CARGO_PKG_VERSION"),
                "--yes",
                "--json",
            ])
            .await;
        assert!(
            result.status.success(),
            "{}",
            String::from_utf8_lossy(&result.stderr)
        );
        let plugin = bundle.join("plugin");
        for name in [".mcp.json", ".mcp.codex.json"] {
            assert_eq!(plugin.join(name).is_file(), enabled);
            assert_eq!(
                plugin
                    .join(name.replace(".json", ".disabled.json"))
                    .is_file(),
                !enabled
            );
        }
        let config: serde_json::Value =
            serde_json::from_slice(&fs::read(plugin.join(name)).unwrap()).unwrap();
        assert_eq!(
            config["mcpServers"]["leanck"]["command"],
            receipt.binary.to_string_lossy().as_ref()
        );
        let manifest: serde_json::Value =
            serde_json::from_slice(&fs::read(plugin.join(".codex-plugin/plugin.json")).unwrap())
                .unwrap();
        assert_eq!(manifest.get("mcpServers").is_some(), enabled);
    }
}

#[tokio::test]
async fn automatic_update_reexecutes_once_and_preserves_an_unpinned_installation() {
    let fixture = Fixture::new().await;
    *fixture.latest.lock().unwrap() = env!("CARGO_PKG_VERSION").into();
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(30),
        fixture.run(&[
            "--set",
            "updates.check=true",
            "--set",
            "updates.automatic=true",
            "daemons",
            "--json",
        ]),
    )
    .await
    .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let _: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    let receipt = Receipt::discover(&fixture.receipt.binary, None)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(receipt.generation, 2);
    assert!(receipt.pin.is_none());
    assert_eq!(
        fixture.calls.load(Ordering::SeqCst),
        3,
        "one metadata request plus checksum and archive"
    );
}

#[tokio::test]
async fn native_command_downloads_verifies_previews_and_updates_all_components() {
    let fixture = Fixture::new().await;
    let before = digest_file(&fixture.receipt.binary).unwrap();
    let args = [
        "update",
        "--version",
        env!("CARGO_PKG_VERSION"),
        "--json",
        "--dry-run",
    ];
    let preview = fixture.run(&args).await;
    assert!(
        preview.status.success(),
        "{}",
        String::from_utf8_lossy(&preview.stderr)
    );
    assert_eq!(digest_file(&fixture.receipt.binary).unwrap(), before);
    assert_eq!(
        Receipt::discover(&fixture.receipt.binary, None)
            .unwrap()
            .unwrap()
            .generation,
        1
    );
    let lease = Lease::acquire(&fixture.receipt).unwrap();
    let blocked = fixture
        .run(&[
            "update",
            "--version",
            env!("CARGO_PKG_VERSION"),
            "--yes",
            "--json",
        ])
        .await;
    assert!(!blocked.status.success());
    drop(lease);
    let update = fixture
        .run(&[
            "update",
            "--version",
            env!("CARGO_PKG_VERSION"),
            "--yes",
            "--json",
        ])
        .await;
    assert!(
        update.status.success(),
        "{}",
        String::from_utf8_lossy(&update.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&update.stdout).unwrap();
    assert_eq!(value["status"], "updated");
    let receipt = Receipt::discover(&fixture.receipt.binary, None)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(receipt.pin.as_deref(), Some(env!("CARGO_PKG_VERSION")));
    assert_eq!(
        fs::read_to_string(fixture.root.join("skills/prove/SKILL.md")).unwrap(),
        "updated proof skill"
    );
    let selections = fs::read_to_string(receipt.state_dir.join("selections.tsv")).unwrap();
    assert!(selections.contains(&format!(
        "last_binary_digest\t{}",
        digest_file(&receipt.binary).unwrap()
    )));
    assert!(selections.contains(&format!(
        "installed_version\tv{}",
        env!("CARGO_PKG_VERSION")
    )));
}

#[tokio::test]
async fn native_updates_refresh_agent_links_and_add_new_selected_agents() {
    use std::os::unix::fs::symlink;
    let fixture = Fixture::new().await;
    let agents = fixture.root.join("data/agents");
    fs::create_dir_all(&agents).unwrap();
    fs::write(agents.join("first.toml"), "old agent").unwrap();
    let links = fixture.root.join("codex/agents");
    fs::create_dir_all(&links).unwrap();
    symlink(agents.join("first.toml"), links.join("first.toml")).unwrap();
    let mut receipt = fixture.receipt.clone();
    receipt.components.push(Component {
        kind: "tree".into(),
        destination: agents.clone(),
        source: "plugin/codex/agents".into(),
        fingerprint: leanck_update::receipt::tree_digest(&agents).unwrap(),
    });
    receipt.components.push(Component {
        kind: "link".into(),
        destination: links.join("first.toml"),
        source: agents.join("first.toml").to_string_lossy().into_owned(),
        fingerprint: agents.join("first.toml").to_string_lossy().into_owned(),
    });
    receipt.write().unwrap();
    let result = fixture
        .run(&[
            "update",
            "--version",
            env!("CARGO_PKG_VERSION"),
            "--yes",
            "--json",
        ])
        .await;
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    for (name, content) in [
        ("first.toml", "updated first agent"),
        ("second.toml", "new second agent"),
    ] {
        assert_eq!(fs::read_to_string(links.join(name)).unwrap(), content);
        assert_eq!(fs::read_link(links.join(name)).unwrap(), agents.join(name));
        assert_eq!(
            fs::read_to_string(links.join(format!("{name}.leanck-sha256")))
                .unwrap()
                .trim(),
            digest_file(&agents.join(name)).unwrap()
        );
    }
}

#[tokio::test]
async fn cli_and_mcp_doctor_always_show_cached_notices_and_disable_them() {
    let fixture = Fixture::new().await;
    // Use the running version here so ordinary doctor admission does not re-exec.
    let mut receipt = fixture.receipt.clone();
    receipt.version = env!("CARGO_PKG_VERSION").into();
    receipt.write().unwrap();
    let checker = leanck_update::Checker::new(
        env!("CARGO_PKG_VERSION"),
        fixture.root.join("state/leanck/updates"),
        receipt.source.clone(),
    )
    .with_receipt(receipt);
    let checked = checker
        .check(&leanck_config::Updates::default(), false)
        .await;
    assert!(checked.notice.is_some(), "{checked:?}");
    let calls = fixture.calls.load(Ordering::SeqCst);
    for args in [
        vec!["--set", "updates.check=true", "doctor"],
        vec!["--set", "updates.check=true", "doctor", "--json"],
    ] {
        let output = fixture.run(&args).await;
        assert!(String::from_utf8_lossy(&output.stdout).contains("1000.0.0"));
    }
    assert_eq!(fixture.calls.load(Ordering::SeqCst), calls);
    let disabled = fixture.run(&["doctor", "--json"]).await;
    let value: serde_json::Value = serde_json::from_slice(&disabled.stdout).unwrap();
    assert_eq!(value["updates"]["state"], "disabled");
    let mut command = fixture.command();
    command
        .args(["--set", "updates.check=true", "serve", "mcp", "--workspace"])
        .arg(&fixture.root);
    let mut server = mcp::Mcp::start(command).await;
    let report = server.tool("doctor", json!({"json":true})).await;
    assert_eq!(report["updates"]["latest_version"], "1000.0.0");
    assert!(
        !fixture.root.join("cache").exists(),
        "doctor must not start Lean"
    );
    assert!(
        Maintenance::acquire(&fixture.receipt.state_dir).is_err(),
        "MCP must retain a lifetime usage lease"
    );
    server.close().await;
    assert!(Maintenance::acquire(&fixture.receipt.state_dir).is_ok());
    assert_eq!(fixture.calls.load(Ordering::SeqCst), calls);
}

#[tokio::test]
async fn host_cached_plugin_executable_retains_the_installations_usage_lease() {
    let fixture = Fixture::new().await;
    let mut receipt = fixture.receipt.clone();
    receipt.version = env!("CARGO_PKG_VERSION").into();
    receipt.write().unwrap();
    let cached = fixture.root.join("host-cache/plugin/bin/leanck");
    fs::create_dir_all(cached.parent().unwrap()).unwrap();
    fs::copy(&receipt.binary, &cached).unwrap();
    fs::copy(Receipt::marker(&receipt.binary), Receipt::marker(&cached)).unwrap();
    let mut command = tokio::process::Command::new(&cached);
    command
        .args([
            "--no-config",
            "--set",
            "updates.check=false",
            "serve",
            "mcp",
            "--workspace",
        ])
        .arg(&fixture.root)
        .env("HOME", &fixture.root)
        .env("XDG_CACHE_HOME", fixture.root.join("runtime-cache"));
    let mut server = mcp::Mcp::start(command).await;
    let report = server.tool("doctor", json!({"json":true})).await;
    assert_eq!(
        report["updates"]["installed_version"],
        env!("CARGO_PKG_VERSION")
    );
    assert!(Maintenance::acquire(&receipt.state_dir).is_err());
    server.close().await;
    assert!(Maintenance::acquire(&receipt.state_dir).is_ok());
}

#[test]
fn inherited_shell_descriptor_holds_the_same_maintenance_lock() {
    use std::os::fd::AsRawFd;
    use std::os::unix::process::CommandExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    let state = root.join("installer");
    fs::create_dir(&state).unwrap();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create_new(true)
        .open(state.join("maintenance.lock"))
        .unwrap();
    let raw = file.as_raw_fd();
    let mut command = std::process::Command::new(env!("CARGO_BIN_EXE_leanck"));
    command.args(["_update", "lock-fd", "--fd", "9"]);
    // SAFETY: dup2 is async-signal-safe and raw remains open until the child exits.
    unsafe {
        command.pre_exec(move || {
            if libc::dup2(raw, 9) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
    assert!(command.status().unwrap().success());
    assert!(
        Maintenance::acquire(&state).is_err(),
        "the parent must retain the child's flock"
    );
    drop(file);
    assert!(Maintenance::acquire(&state).is_ok());
}

#[tokio::test]
async fn bootstrap_installer_emits_native_receipt_and_uninstall_removes_it() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new().await;
    let commands = fixture.root.join("commands");
    fs::create_dir(&commands).unwrap();
    let script = format!(
        r#"#!/bin/sh
while [ "$#" -gt 0 ]; do
    case "$1" in --output) output=$2; shift 2 ;; *) url=$1; shift ;; esac
done
case "$url" in *.sha256) cp '{root}/release.tar.gz.sha256' "$output" ;; *) cp '{root}/release.tar.gz' "$output" ;; esac
printf 200
"#,
        root = fixture.root.display()
    );
    fs::write(commands.join("curl"), script).unwrap();
    fs::set_permissions(commands.join("curl"), fs::Permissions::from_mode(0o755)).unwrap();
    let binary = fixture.root.join("bootstrap-bin/leanck");
    let state = fixture.root.join("bootstrap-state");
    let mut command = tokio::process::Command::new("sh");
    command
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../install.sh"))
        .args([
            "--yes",
            "--standalone",
            "--completions",
            "none",
            "--no-add-to-path",
            "--release-url",
            "https://example.test/releases",
            "--bin-dir",
        ])
        .arg(binary.parent().unwrap())
        .arg("--state-dir")
        .arg(&state)
        .env("HOME", &fixture.root)
        .env("XDG_STATE_HOME", fixture.root.join("state"))
        .env("XDG_CONFIG_HOME", fixture.root.join("config"))
        .env(
            "PATH",
            format!("{}:{}", commands.display(), std::env::var("PATH").unwrap()),
        )
        .env("SHELL", "/bin/sh");
    let result = command.output().await.unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let receipt = Receipt::discover(&binary, None).unwrap().unwrap();
    assert_eq!(receipt.mode, "standalone");
    assert_eq!(receipt.version, env!("CARGO_PKG_VERSION"));
    assert_eq!(receipt.components.len(), 1);
    assert_eq!(receipt.components[0].source, "plugin/bin/leanck");
    assert!(
        Maintenance::acquire(&state).is_ok(),
        "bootstrap shell must release its lock"
    );
    let result = tokio::process::Command::new("sh")
        .arg(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../uninstall.sh"))
        .args(["--yes", "--state-dir"])
        .arg(&state)
        .env("HOME", &fixture.root)
        .env("XDG_STATE_HOME", fixture.root.join("state"))
        .env("XDG_RUNTIME_DIR", fixture.root.join("runtime"))
        .env("XDG_CACHE_HOME", fixture.root.join("cache"))
        .env("TMPDIR", &fixture.root)
        .output()
        .await
        .unwrap();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    assert!(!binary.exists());
    assert!(!Receipt::marker(&binary).exists());
    assert!(!state.join("receipt.json").exists());
    assert!(
        !fixture.root.join("state/leanck/updates").exists(),
        "uninstall must not perform release checks"
    );
}
