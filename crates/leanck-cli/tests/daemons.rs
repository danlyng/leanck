use leanck_core::api::{
    HealthResult, LifecycleHealth, LifecycleState, PROTOCOL_VERSION, Request, Response,
    ServerHealth,
};
use std::io::{Read, Write};
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::thread::JoinHandle;

/// A scratch directory removed on drop; `cache` is `XDG_CACHE_HOME` for the child.
struct Scratch(PathBuf);

impl Scratch {
    fn new(name: &str) -> Self {
        // Under /tmp so socket paths stay within the Unix socket length limit.
        let root = PathBuf::from("/tmp").join(format!(
            "leanck-daemons-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir(&root).unwrap();
        Self(root.canonicalize().unwrap())
    }

    fn cache(&self) -> PathBuf {
        self.0.join("cache")
    }
}

impl Drop for Scratch {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn leanck(cache: &Path, cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_leanck"))
        .env_remove("XDG_RUNTIME_DIR")
        .env("XDG_CACHE_HOME", cache)
        .env_remove("LEANCK_CONFIG")
        .current_dir(cwd)
        .args(["--no-config"])
        .args(args)
        .output()
        .unwrap()
}

fn json(output: &Output) -> serde_json::Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

fn terminal_listing(cache: &Path, cwd: &Path, verbose: bool, columns: u16) -> String {
    use std::fs::File;
    use std::os::fd::FromRawFd;
    let mut master = -1;
    let mut slave = -1;
    let mut size = libc::winsize {
        ws_row: 40,
        ws_col: columns,
        ws_xpixel: 0,
        ws_ypixel: 0,
    };
    // SAFETY: openpty receives valid writable fd pointers and a valid window size.
    assert_eq!(
        unsafe {
            libc::openpty(
                &mut master,
                &mut slave,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut size,
            )
        },
        0
    );
    // SAFETY: a successful openpty returned descriptors owned by this scope.
    let mut master = unsafe { File::from_raw_fd(master) };
    // SAFETY: the slave descriptor is distinct from the master and is owned here.
    let slave = unsafe { File::from_raw_fd(slave) };
    let mut command = Command::new(env!("CARGO_BIN_EXE_leanck"));
    command
        .env_remove("XDG_RUNTIME_DIR")
        .env("XDG_CACHE_HOME", cache)
        .env("TERM", "xterm-256color")
        .env("NO_COLOR", "1")
        .env_remove("LEANCK_CONFIG")
        .current_dir(cwd)
        .args(["--no-config", "daemons", "list"])
        .stdin(std::process::Stdio::null())
        .stdout(slave)
        .stderr(std::process::Stdio::null());
    if verbose {
        command.arg("--verbose");
    }
    let mut child = command.spawn().unwrap();
    drop(command);
    let mut output = Vec::new();
    let mut chunk = [0_u8; 4096];
    loop {
        match master.read(&mut chunk) {
            Ok(0) => break,
            Ok(length) => output.extend_from_slice(&chunk[..length]),
            Err(error) if error.raw_os_error() == Some(libc::EIO) => break,
            Err(error) => panic!("read terminal: {error}"),
        }
    }
    assert!(child.wait().unwrap().success());
    String::from_utf8(output).unwrap().replace('\r', "")
}

#[test]
fn listing_with_no_slot_directory_is_empty_and_creates_nothing() {
    let scratch = Scratch::new("empty");
    let cache = scratch.cache();
    for args in [&["daemons", "--json"][..], &["daemons", "list", "--json"]] {
        let body = json(&leanck(&cache, &scratch.0, args));
        assert_eq!(
            body,
            serde_json::json!({"daemons": [], "running": 0, "limit": 8, "on_limit": "evict_idle"})
        );
    }
    let output = leanck(&cache, &scratch.0, &["daemons"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "no leanck daemons running\n"
    );
    assert!(
        !cache.exists(),
        "listing must not create the cache directory"
    );
}

#[test]
fn dead_slot_records_are_not_listed() {
    let scratch = Scratch::new("dead");
    let slots = scratch.cache().join("leanck");
    std::fs::create_dir_all(&slots).unwrap();
    let mut child = Command::new("true").spawn().unwrap();
    let dead = child.id();
    child.wait().unwrap();
    let record = serde_json::json!({
        "pid": dead,
        "worktree": scratch.0.join("gone"),
        "socket": slots.join("leanck-gone.sock"),
        "started_at": 1,
    });
    std::fs::write(slots.join("leanck-slot-0.lock"), format!("{record}\n")).unwrap();
    let body = json(&leanck(
        &scratch.cache(),
        &scratch.0,
        &["daemons", "--json"],
    ));
    assert_eq!(body["daemons"], serde_json::json!([]));
    assert_eq!(body["running"], 0);
}

#[test]
fn stopping_with_nothing_running() {
    let scratch = Scratch::new("stop");
    let cache = scratch.cache();
    for target in ["--idle", "--all"] {
        let body = json(&leanck(
            &cache,
            &scratch.0,
            &["daemons", "stop", target, "--json"],
        ));
        assert_eq!(
            body,
            serde_json::json!({"stopped": [], "skipped": [], "failed": []})
        );
    }
    let output = leanck(&cache, &scratch.0, &["daemons", "stop", "--idle"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "no idle leanck daemons to stop\n"
    );

    // A worktree with no daemon is an error that points at the listing.
    let output = leanck(&cache, &scratch.0, &["daemons", "stop", "."]);
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("no leanck daemon is running for") && stderr.contains("`leanck daemons`"),
        "{stderr}"
    );
    assert!(output.stdout.is_empty());
    assert!(!cache.exists(), "stop must not create the cache directory");
}

fn read_request(stream: &mut UnixStream) -> Option<Request> {
    let mut length = [0; 4];
    stream.read_exact(&mut length).ok()?;
    let mut body = vec![0; u32::from_be_bytes(length) as usize];
    stream.read_exact(&mut body).ok()?;
    Some(serde_json::from_slice(&body).unwrap())
}

fn write_response(stream: &mut UnixStream, response: &Response) {
    let body = serde_json::to_vec(response).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}

/// A slot record whose pid is a `sleep` child and whose socket answers health
/// with `active` requests. A conditional shutdown is served as
/// `shutdown(if_idle)` and refused with `shutting_down: false` while `active >
/// 0` or when `busy_at_shutdown` simulates work admitted after the listing. An
/// accepted shutdown kills and reaps the child, removes the socket, and ends
/// the thread, which returns the operations it served.
fn fake_daemon(
    slots: &Path,
    slot: u32,
    worktree: &Path,
    active: u64,
    busy_at_shutdown: bool,
) -> (u32, JoinHandle<Vec<String>>) {
    fake_daemon_version(
        slots,
        slot,
        worktree,
        active,
        busy_at_shutdown,
        Some(env!("CARGO_PKG_VERSION")),
    )
}

fn fake_daemon_version(
    slots: &Path,
    slot: u32,
    worktree: &Path,
    active: u64,
    busy_at_shutdown: bool,
    version: Option<&str>,
) -> (u32, JoinHandle<Vec<String>>) {
    std::fs::create_dir_all(worktree).unwrap();
    let mut child: Child = Command::new("sleep").arg("60").spawn().unwrap();
    let pid = child.id();
    let socket = slots.join(format!("fake-{slot}.sock"));
    let listener = UnixListener::bind(&socket).unwrap();
    // Written after the child starts, so the record is live (not a reused pid).
    let record = serde_json::json!({
        "pid": pid,
        "worktree": worktree,
        "socket": socket,
        "started_at": leanck_core::instances::now_unix_secs(),
        "version": version,
    });
    std::fs::write(
        slots.join(format!("leanck-slot-{slot}.lock")),
        format!("{record}\n"),
    )
    .unwrap();
    let health = HealthResult {
        ok: true,
        workspace: worktree.display().to_string(),
        protocol_version: PROTOCOL_VERSION,
        server: ServerHealth {
            running: true,
            pid: Some(pid),
        },
        idle_secs: 90,
        lifecycle: LifecycleHealth {
            state: LifecycleState::Ready,
            active_requests: active,
        },
    };
    let server = std::thread::spawn(move || {
        let mut served = Vec::new();
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            let Some(request) = read_request(&mut stream) else {
                continue;
            };
            let if_idle = request.params["if_idle"].as_bool().unwrap_or(false);
            let shutdown = request.op == "shutdown";
            served.push(if shutdown && if_idle {
                "shutdown(if_idle)".to_owned()
            } else {
                request.op.clone()
            });
            let accepted = !(if_idle && (active > 0 || busy_at_shutdown));
            let result = if shutdown {
                serde_json::json!({"shutting_down": accepted})
            } else {
                serde_json::to_value(&health).unwrap()
            };
            write_response(&mut stream, &Response::ok(request.id, result));
            if shutdown && accepted {
                break;
            }
        }
        // Reap the child so the stopping client sees its pid disappear.
        let _ = child.kill();
        child.wait().unwrap();
        std::fs::remove_file(&socket).unwrap();
        served
    });
    (pid, server)
}

#[test]
fn verbose_inventory_and_incompatible_stop_use_semver_not_executable_paths() {
    let scratch = Scratch::new("versions");
    let cache = scratch.cache();
    let slots = cache.join("leanck");
    std::fs::create_dir_all(&slots).unwrap();
    let mut compatible = semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap();
    compatible.patch += 1;
    let compatible = compatible.to_string();
    let incompatible = format!(
        "{}.0.0",
        semver::Version::parse(env!("CARGO_PKG_VERSION"))
            .unwrap()
            .major
            + 1
    );
    let versions = [
        Some(compatible.as_str()),
        Some(incompatible.as_str()),
        None,
        Some("development"),
    ];
    let mut servers = Vec::new();
    // Every process uses /bin/sleep, at a different path than the CLI. Only
    // the version recorded by the running daemon determines compatibility.
    for (slot, version) in versions.into_iter().enumerate() {
        servers.push(
            fake_daemon_version(
                &slots,
                slot as u32,
                &scratch.0.join(format!("wt{slot}")),
                0,
                false,
                version,
            )
            .1,
        );
    }
    let plain = json(&leanck(&cache, &scratch.0, &["daemons", "list", "--json"]));
    let verbose = json(&leanck(
        &cache,
        &scratch.0,
        &["daemons", "list", "--verbose", "--json"],
    ));
    let wide = terminal_listing(&cache, &scratch.0, false, 160);
    let narrow = terminal_listing(&cache, &scratch.0, true, 40);
    let stopped = json(&leanck(
        &cache,
        &scratch.0,
        &["daemons", "stop", "--incompatible", "--json"],
    ));
    let remaining = json(&leanck(&cache, &scratch.0, &["daemons", "list", "--json"]));
    // Clean up every child before checking assertions.
    let _ = leanck(&cache, &scratch.0, &["daemons", "stop", "--all"]);
    for server in servers {
        assert_eq!(server.join().unwrap().last().unwrap(), "shutdown");
    }
    for (text, width) in [(&wide, 160), (&narrow, 40)] {
        assert!(text.contains('│'), "{text}");
        assert!(text.contains("Worktree"), "{text}");
        assert!(
            text.lines().all(|line| line.chars().count() <= width),
            "{text}"
        );
        assert!(
            !text.contains('\u{1b}'),
            "NO_COLOR must suppress ANSI escapes"
        );
    }
    assert!(!wide.contains("Version"));
    assert!(!wide.contains("Compatible"));
    assert!(narrow.contains("Version"));
    assert!(narrow.contains("Compatible"));
    assert!(plain.get("incompatible").is_none());
    assert!(plain.get("cli_version").is_none());
    assert!(plain["daemons"][0].get("version").is_none());
    assert_eq!(verbose["incompatible"], 1, "{verbose}");
    assert_eq!(verbose["cli_version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(verbose["daemons"][0]["version"], compatible);
    assert_eq!(verbose["daemons"][0]["compatible"], true);
    assert_eq!(verbose["daemons"][1]["version"], incompatible);
    assert_eq!(verbose["daemons"][1]["compatible"], false);
    assert!(verbose["daemons"][2]["compatible"].is_null());
    assert!(verbose["daemons"][3]["compatible"].is_null());
    assert_eq!(stopped["stopped"].as_array().unwrap().len(), 1, "{stopped}");
    assert_eq!(stopped["skipped"].as_array().unwrap().len(), 3);
    assert_eq!(remaining["running"], 3);
}

#[test]
fn stop_idle_skips_busy_daemons_and_a_worktree_stops_its_own() {
    let scratch = Scratch::new("fake");
    let cache = scratch.cache();
    let slots = cache.join("leanck");
    std::fs::create_dir_all(&slots).unwrap();
    let idle_tree = scratch.0.join("idle");
    let busy_tree = scratch.0.join("busy");
    let (idle_pid, idle) = fake_daemon(&slots, 0, &idle_tree, 0, false);
    let (busy_pid, busy) = fake_daemon(&slots, 1, &busy_tree, 2, false);

    let body = json(&leanck(&cache, &scratch.0, &["daemons", "--json"]));
    assert_eq!(body["running"], 2);
    let daemons = body["daemons"].as_array().unwrap();
    assert_eq!(
        daemons
            .iter()
            .map(|d| (d["slot"].clone(), d["pid"].clone(), d["state"].clone()))
            .collect::<Vec<_>>(),
        [
            (0.into(), idle_pid.into(), "ready".into()),
            (1.into(), busy_pid.into(), "ready".into()),
        ]
    );
    assert_eq!(daemons[1]["health"]["lifecycle"]["active_requests"], 2);
    let output = leanck(&cache, &scratch.0, &["daemons"]);
    let text = String::from_utf8_lossy(&output.stdout);
    let lines: Vec<_> = text.lines().collect();
    assert_eq!(lines.len(), 3, "{text}");
    assert!(lines[0].starts_with(idle_tree.to_str().unwrap()), "{text}");
    assert!(lines[0].contains(&format!("pid {idle_pid}")) && lines[0].ends_with("active 0"));
    assert!(lines[1].contains("idle 1m  ready") && lines[1].ends_with("active 2"));
    assert_eq!(lines[2], "2 running, limit 8 (on_limit=evict_idle)");

    let body = json(&leanck(
        &cache,
        &scratch.0,
        &["daemons", "stop", "--idle", "--wait", "5", "--json"],
    ));
    assert_eq!(
        body,
        serde_json::json!({
            "stopped": [{"pid": idle_pid, "worktree": idle_tree, "accepted": true, "exited": true}],
            "skipped": [{"pid": busy_pid, "worktree": busy_tree, "reason": "busy (2 active requests)"}],
            "failed": [],
        })
    );
    // Two listings and the stop each probed health once; `--idle` stops conditionally.
    assert_eq!(
        idle.join().unwrap(),
        ["health", "health", "health", "shutdown(if_idle)"]
    );

    let output = leanck(
        &cache,
        &scratch.0,
        &["daemons", "stop", "busy", "--wait", "5"],
    );
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("stopped {} (pid {busy_pid})\n", busy_tree.display())
    );
    assert_eq!(
        busy.join().unwrap(),
        ["health", "health", "health", "health", "shutdown"]
    );
    let body = json(&leanck(&cache, &scratch.0, &["daemons", "--json"]));
    assert_eq!(body["running"], 0);
}

#[test]
fn stop_idle_skips_a_daemon_that_is_busy_at_shutdown_and_all_stops_it() {
    let scratch = Scratch::new("raced");
    let cache = scratch.cache();
    let slots = cache.join("leanck");
    std::fs::create_dir_all(&slots).unwrap();
    let tree = scratch.0.join("raced");
    // Listed as idle, but it admits work before the stop request arrives.
    let (pid, daemon) = fake_daemon(&slots, 0, &tree, 0, true);

    let body = json(&leanck(
        &cache,
        &scratch.0,
        &["daemons", "stop", "--idle", "--wait", "5", "--json"],
    ));
    assert_eq!(
        body,
        serde_json::json!({
            "stopped": [],
            "skipped": [{"pid": pid, "worktree": tree, "reason": "busy at shutdown"}],
            "failed": [],
        })
    );
    let output = leanck(&cache, &scratch.0, &["daemons", "stop", "--idle"]);
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("skipped {} (pid {pid}): busy at shutdown\n", tree.display())
    );

    // `--all` is unconditional.
    let body = json(&leanck(
        &cache,
        &scratch.0,
        &["daemons", "stop", "--all", "--wait", "5", "--json"],
    ));
    assert_eq!(
        body["stopped"],
        serde_json::json!([{"pid": pid, "worktree": tree, "accepted": true, "exited": true}])
    );
    assert_eq!(
        daemon.join().unwrap(),
        [
            "health",
            "shutdown(if_idle)",
            "health",
            "shutdown(if_idle)",
            "health",
            "shutdown",
        ]
    );
}

#[test]
fn a_removed_worktree_is_stopped_by_relative_or_symlinked_name() {
    let scratch = Scratch::new("removed");
    let cache = scratch.cache();
    let slots = cache.join("leanck");
    std::fs::create_dir_all(&slots).unwrap();
    let relative = scratch.0.join("relative");
    let linked = scratch.0.join("linked");
    let (relative_pid, relative_server) = fake_daemon(&slots, 0, &relative, 0, false);
    let (linked_pid, linked_server) = fake_daemon(&slots, 1, &linked, 0, false);
    std::fs::remove_dir(&relative).unwrap();
    std::fs::remove_dir(&linked).unwrap();
    // The scratch root is canonical; on macOS `/tmp` is a symlink to `/private/tmp`.
    let spelled = Path::new("/tmp")
        .join(scratch.0.file_name().unwrap())
        .join("linked");
    for (name, worktree, pid) in [
        (Path::new("relative"), &relative, relative_pid),
        (spelled.as_path(), &linked, linked_pid),
    ] {
        let output = leanck(
            &cache,
            &scratch.0,
            &["daemons", "stop", name.to_str().unwrap(), "--wait", "5"],
        );
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&output.stdout),
            format!("stopped {} (pid {pid})\n", worktree.display())
        );
    }
    assert_eq!(relative_server.join().unwrap().last().unwrap(), "shutdown");
    assert_eq!(linked_server.join().unwrap().last().unwrap(), "shutdown");
}
