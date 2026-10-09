//! `install.sh` and `uninstall.sh` end to end, against releases that the
//! `xtask` packager builds in process and a `file://` (or faked HTTPS) mirror
//! serves.
//!
//! Isolation: HOME, every XDG directory, CLAUDE_CONFIG_DIR, CODEX_HOME,
//! TMPDIR, and XDG_RUNTIME_DIR point into a temporary directory, and fake
//! `claude`, `codex`, `curl`, and `cargo` come first on PATH. The packaged
//! `leanck` is a script that forwards installer commands to the built
//! executable and answers `daemons` itself: daemon listing always considers
//! `/tmp/leanck-<uid>`, so a real executable could reach the developer's
//! daemons, and no test may list or stop them.
use leanck_update::{gate::Maintenance, receipt::Receipt};
use std::{
    fs,
    io::{Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::{fs::PermissionsExt, process::CommandExt},
    },
    path::{Path, PathBuf},
    process::{Command, Output, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

const VERSION: &str = env!("CARGO_PKG_VERSION");
/// A ceiling for the terminal tests, which wait on prompt text, not on time.
const CEILING: Duration = Duration::from_secs(120);

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

/// The archive target name `install.sh` derives from `uname`.
fn target() -> String {
    let platform = if cfg!(target_os = "macos") {
        "apple-darwin"
    } else {
        "unknown-linux-musl"
    };
    format!("{}-{platform}", std::env::consts::ARCH)
}

fn script(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, text).unwrap();
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_default()
}

struct World {
    _dir: tempfile::TempDir,
    root: PathBuf,
}

impl World {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        for sub in ["home", "tmp", "runtime"] {
            fs::create_dir_all(root.join(sub)).unwrap();
        }
        let log = |name: &str| root.join(name).display().to_string();
        for host in ["claude", "codex"] {
            script(
                &root.join("bin").join(host),
                &format!(
                    "#!/bin/sh\nprintf '{host} %s\\n' \"$*\" >> '{}'\n",
                    log("hosts.log")
                ),
            );
        }
        // HTTPS downloads come from ROOT/https; anything else is a 404.
        script(
            &root.join("bin/curl"),
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$*" >> '{curl}'
while [ "$#" -gt 1 ]; do case "$1" in --output) out=$2; shift 2 ;; *) shift ;; esac; done
file='{https}/'"${{1#https://example.test/}}"
if [ -f "$file" ]; then cp "$file" "$out"; printf 200; exit 0; fi
printf 404
exit 22
"#,
                curl = log("curl.log"),
                https = root.join("https").display(),
            ),
        );
        // `cargo build` stages the forwarding script; `cargo xtask` copies
        // the package that `World::package` assembled.
        script(
            &root.join("bin/cargo"),
            &format!(
                r#"#!/bin/sh
printf '%s\n' "$*" >> '{cargo}'
command=$1
while [ "$#" -gt 0 ]; do
    case "$1" in --target-dir) dir=$2; shift ;; --output) out=$2; shift ;; esac
    shift
done
case "$command" in
    build) mkdir -p "$dir/release" && cp '{staged}' "$dir/release/leanck" ;;
    xtask) cp -R '{package}' "$out" ;;
esac
"#,
                cargo = log("cargo.log"),
                staged = root.join("staged/leanck").display(),
                package = root.join("local-package").display(),
            ),
        );
        script(
            &root.join("staged/leanck"),
            &format!(
                r#"#!/bin/sh
case "$1" in
    daemons) [ "$2" = list ] && echo '{{"incompatible": 0}}'; exit 0 ;;
    _install|_uninstall|_completions|--version) exec '{}' "$@" ;;
esac
echo "unexpected: leanck $*" >&2
exit 2
"#,
                env!("CARGO_BIN_EXE_leanck")
            ),
        );
        Self { _dir: dir, root }
    }

    fn path(&self, relative: &str) -> PathBuf {
        self.root.join(relative)
    }

    /// Assemble an unpacked release with the forwarding executable.
    fn release(&self, name: &str) -> PathBuf {
        let out = self.path("dist").join(name).join("leanck");
        fs::create_dir_all(out.parent().unwrap()).unwrap();
        xtask::assemble_distribution(&repo(), Some(&self.path("staged")), &out, VERSION, false)
            .unwrap();
        out
    }

    /// Archive `release` as both the pinned and the latest release under `mirror`.
    fn publish(&self, release: &Path, mirror: &Path) {
        for dir in [format!("download/v{VERSION}"), "latest/download".into()] {
            xtask::write_release_archive(release, &mirror.join(dir), &target()).unwrap();
        }
    }

    /// A published release and its `file://` mirror URL.
    fn mirror(&self) -> String {
        let release = self.release("release");
        self.publish(&release, &self.path("mirror"));
        format!("file://{}", self.path("mirror").display())
    }

    fn command(&self, script: &str) -> Command {
        let mut command = Command::new("sh");
        command
            .arg(repo().join(script))
            .env_clear()
            .env("HOME", self.path("home"))
            .env(
                "PATH",
                format!("{}:/usr/bin:/bin", self.path("bin").display()),
            )
            .env("SHELL", "/bin/zsh")
            .env("TERM", "dumb")
            .env("TMPDIR", self.path("tmp"))
            .env("XDG_RUNTIME_DIR", self.path("runtime"))
            .env("XDG_CACHE_HOME", self.path("xdg/cache"))
            .env("XDG_CONFIG_HOME", self.path("xdg/config"))
            .env("XDG_DATA_HOME", self.path("xdg/data"))
            .env("XDG_STATE_HOME", self.path("xdg/state"))
            .env("CLAUDE_CONFIG_DIR", self.path("claude"))
            .env("CODEX_HOME", self.path("codex"))
            .current_dir(&self.root)
            .stdin(Stdio::null());
        command
    }

    fn run(&self, script: &str, args: &[&str]) -> Output {
        self.command(script).args(args).output().unwrap()
    }

    fn assert_tmp_is_empty(&self) {
        let left: Vec<_> = fs::read_dir(self.path("tmp"))
            .unwrap()
            .map(|e| e.unwrap().path())
            .collect();
        assert!(left.is_empty(), "work directories left behind: {left:?}");
    }

    fn state(&self) -> PathBuf {
        self.path("xdg/state/leanck/installer")
    }

    /// The files and directories that make a leanck source checkout; the
    /// fake `cargo` builds nothing from them.
    fn checkout(&self) -> PathBuf {
        let checkout = self.path("checkout");
        for dir in [
            "crates/leanck-cli",
            "xtask",
            "leanck-skills",
            "leanck-subagents",
        ] {
            fs::create_dir_all(checkout.join(dir)).unwrap();
        }
        for file in [
            "Cargo.toml",
            "Cargo.lock",
            "crates/leanck-cli/Cargo.toml",
            "xtask/Cargo.toml",
        ] {
            fs::write(checkout.join(file), "").unwrap();
        }
        checkout
    }
}

#[test]
fn plugin_install_registers_both_hosts_and_uninstall_sh_removes_everything() {
    let world = World::new();
    let mirror = world.mirror();
    fs::write(world.path("home/.zshrc"), "# profile\n").unwrap();
    fs::create_dir_all(world.path("completions")).unwrap();
    let completions = world.path("completions").display().to_string();
    let output = world.run(
        "install.sh",
        &[
            "--release-url",
            &mirror,
            "--yes",
            "--all",
            "--completions",
            "zsh",
            "--completions-dir",
            &completions,
            "--shell",
            "zsh",
        ],
    );
    let log = stderr(&output);
    assert!(output.status.success(), "{log}");
    assert!(output.stdout.is_empty());
    let data = world.path("xdg/data/leanck");
    let bundle = data.join("bundle");
    assert!(data.join("bin/leanck").is_file(), "{log}");
    assert!(bundle.join("plugin/.claude-plugin/plugin.json").is_file());
    assert!(read(&world.path("completions/_leanck")).starts_with("#compdef leanck"));
    assert!(read(&world.path("home/.zshrc")).contains("# >>> leanck PATH >>>"));
    let hosts = read(&world.path("hosts.log"));
    for call in [
        format!("claude plugin marketplace add {}", bundle.display()),
        "claude plugin install leanck@leanck --scope user".into(),
        format!("codex plugin marketplace add {}", bundle.display()),
        "codex plugin add leanck@leanck".into(),
    ] {
        assert!(hosts.contains(&call), "missing {call:?} in:\n{hosts}");
    }
    assert!(!world.path("curl.log").exists());
    world.assert_tmp_is_empty();

    let output = world.run("uninstall.sh", &["--yes"]);
    let log = stderr(&output);
    assert!(output.status.success(), "{log}");
    assert!(log.contains("Uninstalled leanck."), "{log}");
    assert!(!data.join("bin/leanck").exists());
    assert!(!bundle.exists());
    assert!(!world.path("completions/_leanck").exists());
    assert_eq!(read(&world.path("home/.zshrc")), "# profile\n");
    let hosts = read(&world.path("hosts.log"));
    for call in [
        "claude plugin uninstall leanck@leanck --scope user",
        "codex plugin remove leanck@leanck",
    ] {
        assert!(hosts.contains(call), "missing {call:?} in:\n{hosts}");
    }
    assert!(!world.state().join("selections.tsv").exists());
    assert!(!world.state().join("uninstall.tsv").exists());
    assert!(
        !world.path("xdg/state/leanck/updates").exists(),
        "installing and uninstalling must not check for releases"
    );
}

#[test]
fn standalone_install_from_an_https_mirror_reports_json_and_keeps_its_pin() {
    let world = World::new();
    let release = world.release("release");
    // Only the pinned path exists, so a later run must use the saved pin.
    xtask::write_release_archive(
        &release,
        &world.path(&format!("https/releases/download/v{VERSION}")),
        &target(),
    )
    .unwrap();
    // Checksum sidecars may use upper case.
    let sidecar = world.path(&format!(
        "https/releases/download/v{VERSION}/leanck-{}.tar.gz.sha256",
        target()
    ));
    fs::write(&sidecar, read(&sidecar).to_uppercase()).unwrap();
    let bin = world.path("installed-bin");
    let state = world.path("custom state");
    let args = |pin: bool| {
        let mut args = vec![
            "--release-url".to_owned(),
            "https://example.test/releases".into(),
            "--yes".into(),
            "--json".into(),
            "--standalone".into(),
            "--skills".into(),
            "codex".into(),
            "--skill".into(),
            "leanck".into(),
            "--completions".into(),
            "none".into(),
            "--no-add-to-path".into(),
            "--bin-dir".into(),
            bin.display().to_string(),
            "--state-dir".into(),
            state.display().to_string(),
        ];
        if pin {
            args.extend(["--version".into(), format!("v{VERSION}")]);
        }
        args
    };
    for pin in [true, false] {
        let output = world
            .command("install.sh")
            .args(args(pin))
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", stderr(&output));
        let stdout = String::from_utf8(output.stdout).unwrap();
        assert_eq!(stdout.lines().count(), 1, "{stdout}");
        let json: serde_json::Value = serde_json::from_str(&stdout).unwrap();
        assert_eq!(json["status"], "installed", "{json}");
        assert_eq!(json["version"], format!("v{VERSION}"));
    }
    let downloads = read(&world.path("curl.log"));
    assert_eq!(downloads.lines().count(), 4, "{downloads}");
    assert!(
        downloads.lines().all(|line| line.contains(&format!(
            "https://example.test/releases/download/v{VERSION}/"
        ))),
        "{downloads}"
    );
    let receipt = Receipt::discover(&bin.join("leanck"), None)
        .unwrap()
        .unwrap();
    assert_eq!(receipt.mode, "standalone");
    assert_eq!(receipt.version, VERSION);
    assert_eq!(receipt.selected_skills, Some(vec!["leanck".to_owned()]));
    assert_eq!(receipt.components[0].source, "plugin/bin/leanck");
    let selections = read(&state.join("selections.tsv"));
    assert!(
        selections.contains(&format!("version\t{VERSION}\n")),
        "{selections}"
    );
    assert!(selections.contains("release_url\thttps://example.test/releases\n"));
    assert!(world.path("home/.agents/skills/leanck/SKILL.md").is_file());
    assert!(!world.path("home/.agents/skills/lean-proving").exists());
    assert!(
        Maintenance::acquire(&state).is_ok(),
        "the installer must release its lock"
    );
    world.assert_tmp_is_empty();

    // Without the installed executable, a standalone install has no other
    // copy; the uninstall.sh in a release archive uses the archive's own.
    fs::remove_file(bin.join("leanck")).unwrap();
    let state_arg = state.display().to_string();
    let output = world.run("uninstall.sh", &["--yes", "--state-dir", &state_arg]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no leanck executable with _uninstall found"));
    let archived = release.join("uninstall.sh").display().to_string();
    let output = world.run(&archived, &["--yes", "--state-dir", &state_arg]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(!world.path("home/.agents/skills/leanck").exists());
    assert!(!state.join("selections.tsv").exists());
}

#[test]
fn bootstrap_checks_fail_before_any_change_and_dry_runs_preview() {
    let world = World::new();
    let mirror = world.mirror();
    let home = world.path("home");
    let untouched = || {
        let entries: Vec<_> = fs::read_dir(&home).unwrap().collect();
        assert!(entries.is_empty(), "{entries:?}");
        assert!(!world.path("xdg/data").exists() && !world.path("xdg/state").exists());
        world.assert_tmp_is_empty();
    };
    let json = |output: &Output| -> serde_json::Value {
        serde_json::from_slice(&output.stdout)
            .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&output.stdout)))
    };

    // Help needs neither a network nor a release.
    let output = world.run("install.sh", &["--plugin", "both", "--help"]);
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("Bootstrap options:") && help.contains("leanck _install --help"));

    // Approval comes before any download, also with --force and --json.
    let https = ["--release-url", "https://example.test/releases"];
    for extra in [&["--force"][..], &["--json"][..]] {
        let output = world.run("install.sh", &[&https[..], extra].concat());
        assert_eq!(output.status.code(), Some(1));
        assert!(stderr(&output).contains("approval is required; rerun in a terminal"));
        if extra.contains(&"--json") {
            assert_eq!(json(&output)["status"], "failed");
        }
    }
    assert!(!world.path("curl.log").exists());
    for (args, message) in [
        (
            &["--timeout", "0"][..],
            "--timeout must be a positive number of seconds",
        ),
        (&["--version", "1.2/3"][..], "invalid release version"),
        (&["--skill"][..], "--skill needs a value; see --help"),
        (
            &["--release-url", "http://example.test"][..],
            "--release-url must use HTTPS",
        ),
    ] {
        let output = world.run("install.sh", &[&["--yes", "--json"][..], args].concat());
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(
            json(&output)["message"]
                .as_str()
                .unwrap()
                .starts_with(message),
            "{args:?}"
        );
    }
    assert!(!world.path("curl.log").exists());

    // A dry run previews without approval and changes nothing.
    let output = world.run(
        "install.sh",
        &["--release-url", &mirror, "--dry-run", "--json"],
    );
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(json(&output)["status"], "preview");
    assert!(stderr(&output).contains("Dry run complete"));
    untouched();

    // A missing pinned release fails, in dry runs too, over HTTPS and file://.
    for url in [&mirror, "https://example.test/releases"] {
        let output = world.run(
            "install.sh",
            &[
                "--release-url",
                url,
                "--version",
                "v9.9.9",
                "--dry-run",
                "--json",
            ],
        );
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(json(&output)["message"], "release v9.9.9 not found");
    }

    // Releases without `_install` belong to their own install.sh.
    let old = world.release("old");
    fs::write(old.join(".leanck-update-protocol"), "2\n").unwrap();
    world.publish(&old, &world.path("old-mirror"));
    let old_mirror = format!("file://{}", world.path("old-mirror").display());
    let output = world.run("install.sh", &["--release-url", &old_mirror, "--dry-run"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains(&format!(
        "v{VERSION} predates this installer; use the install.sh attached to that release"
    )));

    // A corrupted archive is refused.
    let sidecar = world.path(&format!(
        "old-mirror/latest/download/leanck-{}.tar.gz.sha256",
        target()
    ));
    fs::write(&sidecar, format!("{}  x\n", "0".repeat(64))).unwrap();
    let output = world.run("install.sh", &["--release-url", &old_mirror, "--yes"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("SHA-256 checksum mismatch; nothing installed"));

    // Options the bootstrap does not know fail in the release's installer.
    let output = world.run(
        "install.sh",
        &["--release-url", &mirror, "--yes", "--json", "--wat"],
    );
    assert_eq!(output.status.code(), Some(1));
    assert_eq!(
        json(&output)["message"],
        "unknown option: --wat; see --help"
    );
    untouched();
}

#[test]
fn from_repo_builds_with_cargo_then_installs_the_local_package() {
    let world = World::new();
    xtask::assemble_distribution(
        &repo(),
        Some(&world.path("staged")),
        &world.path("local-package"),
        VERSION,
        false,
    )
    .unwrap();
    let checkout = world.checkout();
    let repo_arg = checkout.display().to_string();
    // Nothing from the checkout runs without approval or with a release pin.
    for (args, message) in [
        (
            &["--dry-run"][..],
            "--from-repo builds before previewing; add --yes to approve the build",
        ),
        (
            &["--yes", "--version", "v1.0.0"][..],
            "--version selects a published release; omit it with --from-repo",
        ),
    ] {
        let output = world.run(
            "install.sh",
            &[&["--from-repo", &repo_arg][..], args].concat(),
        );
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(stderr(&output).contains(message), "{}", stderr(&output));
    }
    let output = world.run("install.sh", &["--from-repo", "/nonexistent", "--yes"]);
    assert!(stderr(&output).contains("not a leanck source checkout: /nonexistent"));
    assert!(!world.path("cargo.log").exists());

    let output = world.run(
        "install.sh",
        &[
            "--from-repo",
            &repo_arg,
            "--yes",
            "--standalone",
            "--skills",
            "claude",
            "--completions",
            "none",
            "--no-add-to-path",
        ],
    );
    let log = stderr(&output);
    assert!(output.status.success(), "{log}");
    let target_dir = checkout.join("target");
    let calls: Vec<String> = read(&world.path("cargo.log"))
        .lines()
        .map(Into::into)
        .collect();
    assert_eq!(calls.len(), 2, "{calls:?}");
    assert_eq!(
        calls[0],
        format!(
            "build --release --locked -p leanck-cli --target-dir {}",
            target_dir.display()
        )
    );
    assert!(
        calls[1].starts_with(&format!(
            "xtask package-local --bin-dir {} --output ",
            target_dir.join("release").display()
        )) && calls[1].ends_with(&format!(" --version {VERSION}")),
        "{calls:?}"
    );
    assert!(log.contains(&format!("v{VERSION} (local source)")), "{log}");
    let binary = world.path("home/.local/bin/leanck");
    let receipt = Receipt::discover(&binary, None).unwrap().unwrap();
    assert_eq!(receipt.provenance, "source");
    assert!(world.path("claude/skills/leanck/SKILL.md").is_file());
    world.assert_tmp_is_empty();
}

#[test]
fn scripts_pass_shellcheck() {
    for script in ["install.sh", "uninstall.sh"] {
        let output = Command::new("sh")
            .arg("-n")
            .arg(repo().join(script))
            .output()
            .unwrap();
        assert!(output.status.success(), "{script}: {}", stderr(&output));
    }
    if Command::new("shellcheck")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("notice: shellcheck is not installed; skipping the shellcheck check");
        return;
    }
    let output = Command::new("shellcheck")
        .args(["--shell=sh", "--severity=style"])
        .arg(repo().join("install.sh"))
        .arg(repo().join("uninstall.sh"))
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stdout)
    );
}

// Terminal tests.

/// A pseudo-terminal: the parent keeps the controller and a follower
/// descriptor, so the follower's settings can be read after the child exits.
struct Pty {
    controller: fs::File,
    follower: OwnedFd,
    output: mpsc::Receiver<Vec<u8>>,
    seen: Vec<u8>,
}

impl Pty {
    fn open() -> Self {
        let (mut controller, mut follower) = (0, 0);
        let mut size = libc::winsize {
            ws_row: 40,
            ws_col: 120,
            ws_xpixel: 0,
            ws_ypixel: 0,
        };
        // SAFETY: the out-pointers are valid; name and settings may be null.
        let status = unsafe {
            libc::openpty(
                &mut controller,
                &mut follower,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        };
        assert_eq!(status, 0, "{}", std::io::Error::last_os_error());
        // SAFETY: openpty returned two new descriptors that nothing else owns.
        let (controller, follower) = unsafe {
            (
                fs::File::from_raw_fd(controller),
                OwnedFd::from_raw_fd(follower),
            )
        };
        let mut reader = controller.try_clone().unwrap();
        let (sender, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            while let Ok(n) = reader.read(&mut buffer) {
                if n == 0 || sender.send(buffer[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Pty {
            controller,
            follower,
            output,
            seen: vec![],
        }
    }

    fn termios(&self) -> libc::termios {
        // SAFETY: zeroed termios is a valid out-buffer for tcgetattr.
        let mut termios: libc::termios = unsafe { std::mem::zeroed() };
        // SAFETY: the follower descriptor is open.
        assert_eq!(
            unsafe { libc::tcgetattr(self.follower.as_raw_fd(), &mut termios) },
            0
        );
        termios
    }

    /// Run `command` as a session leader with this terminal as its controlling terminal.
    fn spawn(&self, mut command: Command) -> std::process::Child {
        let stdio = || Stdio::from(self.follower.try_clone().unwrap());
        command.stdin(stdio()).stdout(stdio()).stderr(stdio());
        // SAFETY: setsid and ioctl are async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                if libc::setsid() < 0 || libc::ioctl(0, libc::TIOCSCTTY as _, 0) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        command.spawn().unwrap()
    }

    /// Wait until the terminal shows `text`.
    fn expect(&mut self, text: &str) {
        let deadline = Instant::now() + CEILING;
        while !String::from_utf8_lossy(&self.seen).contains(text) {
            let left = deadline.saturating_duration_since(Instant::now());
            match self.output.recv_timeout(left) {
                Ok(bytes) => self.seen.extend(bytes),
                Err(_) => panic!(
                    "{text:?} never appeared; the terminal showed:\n{}",
                    String::from_utf8_lossy(&self.seen)
                ),
            }
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.controller.write_all(bytes).unwrap();
    }
}

/// Wait for `child` to exit, killing its session after the ceiling.
fn wait(child: std::process::Child) -> std::process::ExitStatus {
    let pid = child.id() as libc::pid_t;
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut child = child;
        let _ = sender.send(child.wait().unwrap());
    });
    receiver.recv_timeout(CEILING).unwrap_or_else(|_| {
        // SAFETY: the child leads its own session and process group.
        unsafe { libc::kill(-pid, libc::SIGKILL) };
        panic!("the installer did not exit");
    })
}

fn flags(termios: &libc::termios) -> (libc::tcflag_t, libc::tcflag_t, libc::tcflag_t) {
    (
        termios.c_lflag & (libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN),
        termios.c_iflag & (libc::ICRNL | libc::IXON),
        termios.c_oflag & libc::OPOST,
    )
}

#[test]
fn ctrl_c_at_the_first_prompt_restores_the_terminal_and_exits_130() {
    let world = World::new();
    let mirror = world.mirror();
    let mut pty = Pty::open();
    let before = flags(&pty.termios());
    // The session leader outlives the installer: some systems revoke a
    // terminal once its controlling process exits, which would hide its settings.
    let mut command = world.command("install.sh");
    command
        .args(["--release-url", &mirror])
        .env("TERM", "xterm-256color");
    let script = command.get_args().next().unwrap().to_owned();
    let mut leader = Command::new("sh");
    leader
        .arg("-c")
        .arg(r#"sh "$@"; printf '\nexited with %s\n' "$?"; read -r _"#)
        .arg("leader")
        .arg(script)
        .args(["--release-url", &mirror]);
    leader
        .env_clear()
        .envs(command.get_envs().filter_map(|(k, v)| Some((k, v?))));
    leader.current_dir(&world.root);
    let child = pty.spawn(leader);
    pty.expect("Install as plugin or standalone?");
    // Raw mode is on while the menu shows, so Ctrl-C arrives as a key.
    pty.send(b"\x03");
    pty.expect("exited with 130");
    assert_eq!(
        flags(&pty.termios()),
        before,
        "terminal settings were not restored"
    );
    pty.send(b"\n");
    assert!(wait(child).success());
    assert!(!world.path("xdg/data").exists() && !world.path("xdg/state").exists());
    world.assert_tmp_is_empty();

    // Inspect the UI after it takes over, excluding the banner printed to
    // stderr beforehand. Strip terminal commands to check visible text.
    let output = String::from_utf8_lossy(&pty.seen);
    let (_, screen) = output.split_once("\x1b[?1049h").unwrap();
    let mut chars = screen.chars();
    let mut visible = String::new();
    while let Some(c) = chars.next() {
        if c == '\x1b' && chars.next() == Some('[') {
            for c in chars.by_ref() {
                if ('@'..='~').contains(&c) {
                    break;
                }
            }
        } else {
            visible.push(c);
        }
    }
    assert!(
        visible.starts_with(&format!("Welcome to the leanck v{VERSION} installer")),
        "unexpected welcome banner: {visible:?}"
    );
}

#[test]
fn declining_the_from_repo_build_runs_nothing_from_the_checkout() {
    let world = World::new();
    let checkout = world.checkout();
    let mut pty = Pty::open();
    let mut command = world.command("install.sh");
    command.arg("--from-repo").arg(&checkout);
    let child = pty.spawn(command);
    pty.expect("This runs the repository's build scripts. [y/N]: ");
    pty.send(b"n\n");
    let status = wait(child);
    assert_eq!(
        status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&pty.seen)
    );
    pty.expect("Cancelled; nothing built or installed.");
    assert!(!world.path("cargo.log").exists());
    assert!(!checkout.join("target").exists());
    world.assert_tmp_is_empty();
}
