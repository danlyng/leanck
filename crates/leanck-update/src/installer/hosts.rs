//! The installer's external commands: host CLIs (`claude`, `codex`) behind
//! [`HostCli`], daemon maintenance behind [`Daemons`], and the bounded runner
//! that executes them with the configured `--timeout`.
//!
//! Dry runs never reach these traits: only `apply` and `_uninstall` call them,
//! so tests substitute in-process fakes and assert exactly what would run.
use super::{Console, HostName};
use std::{
    ffi::OsString,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

/// Output kept from one command; larger output is truncated.
const CAPTURE_LIMIT: u64 = 8 * 1024 * 1024;

impl HostName {
    /// The host CLI's command name.
    pub fn command(self) -> &'static str {
        match self {
            HostName::Claude => "claude",
            HostName::Codex => "codex",
        }
    }
    /// The variable that selects the host's configuration home.
    pub fn home_variable(self) -> &'static str {
        match self {
            HostName::Claude => "CLAUDE_CONFIG_DIR",
            HostName::Codex => "CODEX_HOME",
        }
    }
    /// `Claude` or `Codex`, for messages.
    pub fn title(self) -> &'static str {
        match self {
            HostName::Claude => "Claude",
            HostName::Codex => "Codex",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "claude" => Some(HostName::Claude),
            "codex" => Some(HostName::Codex),
            _ => None,
        }
    }
}

/// Captured standard output and error.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Captured {
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

/// Why a command did not succeed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The command is not on `PATH`.
    Missing(String),
    /// The command could not be started.
    Spawn(String),
    /// The command exited unsuccessfully (`None` when killed by a signal).
    Status(Option<i32>),
    /// The command ran longer than the timeout and was stopped.
    TimedOut { seconds: u64, command: String },
}
impl Failure {
    /// The message relayed for this failure, if it has its own line.
    pub fn line(&self) -> Option<String> {
        match self {
            Failure::Missing(command) => Some(format!("{command} is not on PATH")),
            Failure::Spawn(message) => Some(message.clone()),
            Failure::Status(_) => None,
            Failure::TimedOut { seconds, command } => Some(format!(
                "Command timed out after {seconds} seconds: {command}"
            )),
        }
    }
}

/// A finished command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ran {
    pub captured: Captured,
    pub result: Result<(), Failure>,
}
impl Ran {
    /// A successful command with this standard output.
    pub fn ok(stdout: impl Into<Vec<u8>>) -> Self {
        Ran {
            captured: Captured {
                stdout: stdout.into(),
                stderr: vec![],
            },
            result: Ok(()),
        }
    }
    /// A command that exited with `code`.
    pub fn failed(code: i32) -> Self {
        Ran {
            captured: Captured::default(),
            result: Err(Failure::Status(Some(code))),
        }
    }
    pub fn success(&self) -> bool {
        self.result.is_ok()
    }
}

/// One host CLI invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostCall {
    pub host: HostName,
    /// `CLAUDE_CONFIG_DIR` or `CODEX_HOME` for the call.
    pub home: PathBuf,
    pub args: Vec<String>,
    /// Prepended to `PATH`, so host hooks can run the installed `leanck`.
    pub path_prefix: Option<PathBuf>,
}
impl HostCall {
    pub fn new(host: HostName, home: &Path, args: &[&str]) -> Self {
        HostCall {
            host,
            home: home.to_owned(),
            args: args.iter().map(|a| a.to_string()).collect(),
            path_prefix: None,
        }
    }
    /// `claude plugin marketplace add /path`, as messages name it.
    pub fn command_line(&self) -> String {
        std::iter::once(self.host.command())
            .chain(self.args.iter().map(String::as_str))
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// Host CLIs. The real implementation is [`SystemHosts`].
pub trait HostCli {
    /// Whether the host CLI is on `PATH`.
    fn available(&self, host: HostName) -> bool;
    /// Run a host command with its home variable set.
    fn run(&self, call: &HostCall) -> Ran;
}

/// Why the incompatible daemon count is unknown.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DaemonCheck {
    /// `daemons list` failed.
    Failed,
    /// `daemons list` printed no readable `incompatible` count.
    Unreadable,
}

/// Daemon maintenance through a `leanck` executable. The real
/// implementation is [`SystemDaemons`].
pub trait Daemons {
    /// Count running daemons incompatible with `binary`.
    fn incompatible(&self, binary: &Path) -> Result<usize, DaemonCheck>;
    /// Stop daemons incompatible with `binary`.
    fn stop_incompatible(&self, binary: &Path) -> Ran;
    /// Stop every daemon, before uninstalling `binary`.
    fn stop_all(&self, binary: &Path) -> Ran;
}

/// Host CLIs found on a fixed `PATH`, bounded by a timeout.
pub struct SystemHosts {
    pub path: OsString,
    pub timeout: Duration,
}
impl SystemHosts {
    /// Use the process `PATH`.
    pub fn new(timeout: Duration) -> Self {
        SystemHosts {
            path: std::env::var_os("PATH").unwrap_or_default(),
            timeout,
        }
    }
    fn find(&self, name: &str) -> Option<PathBuf> {
        std::env::split_paths(&self.path)
            .map(|dir| dir.join(name))
            .find(|candidate| is_executable(candidate))
    }
}
impl HostCli for SystemHosts {
    fn available(&self, host: HostName) -> bool {
        self.find(host.command()).is_some()
    }
    fn run(&self, call: &HostCall) -> Ran {
        let Some(program) = self.find(call.host.command()) else {
            return Ran {
                captured: Captured::default(),
                result: Err(Failure::Missing(call.host.command().into())),
            };
        };
        let mut command = Command::new(program);
        command
            .args(&call.args)
            .env(call.host.home_variable(), &call.home);
        let mut path = OsString::new();
        if let Some(prefix) = &call.path_prefix {
            path.push(prefix);
            if !self.path.is_empty() {
                path.push(":");
            }
        }
        path.push(&self.path);
        command.env("PATH", path);
        run_bounded(command, self.timeout, &call.command_line())
    }
}

/// Daemon maintenance by running a `leanck` executable.
pub struct SystemDaemons {
    pub timeout: Duration,
}
impl SystemDaemons {
    fn run(&self, binary: &Path, args: &[&str]) -> Ran {
        let mut command = Command::new(binary);
        command.args(args);
        let line = format!("leanck {}", args.join(" "));
        run_bounded(command, self.timeout, &line)
    }
}
impl Daemons for SystemDaemons {
    fn incompatible(&self, binary: &Path) -> Result<usize, DaemonCheck> {
        let ran = self.run(
            binary,
            &[
                "daemons",
                "list",
                "--verbose",
                "--json",
                "--no-config",
                "--set",
                "updates.check=false",
            ],
        );
        if !ran.success() {
            return Err(DaemonCheck::Failed);
        }
        let listing: serde_json::Value =
            serde_json::from_slice(&ran.captured.stdout).map_err(|_| DaemonCheck::Unreadable)?;
        listing["incompatible"]
            .as_u64()
            .map(|n| n as usize)
            .ok_or(DaemonCheck::Unreadable)
    }
    fn stop_incompatible(&self, binary: &Path) -> Ran {
        self.run(
            binary,
            &[
                "daemons",
                "stop",
                "--incompatible",
                "--no-config",
                "--set",
                "updates.check=false",
            ],
        )
    }
    fn stop_all(&self, binary: &Path) -> Ran {
        self.run(
            binary,
            &["daemons", "stop", "--all", "--set", "updates.check=false"],
        )
    }
}

pub(crate) fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

/// Run `command` with stdin closed and output captured. After `timeout` the
/// command's process group gets SIGTERM, then SIGKILL a second later.
/// `display` names the command in the timeout message.
pub fn run_bounded(mut command: Command, timeout: Duration, display: &str) -> Ran {
    use std::os::unix::process::CommandExt;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(error) => {
            return Ran {
                captured: Captured::default(),
                result: Err(Failure::Spawn(format!("could not run {display}: {error}"))),
            };
        }
    };
    let reader = |stream: Option<Box<dyn Read + Send>>| {
        let (sender, receiver) = mpsc::channel();
        if let Some(stream) = stream {
            std::thread::spawn(move || {
                let mut bytes = vec![];
                let _ = stream.take(CAPTURE_LIMIT).read_to_end(&mut bytes);
                let _ = sender.send(bytes);
            });
        }
        receiver
    };
    let stdout = reader(
        child
            .stdout
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let stderr = reader(
        child
            .stderr
            .take()
            .map(|s| Box::new(s) as Box<dyn Read + Send>),
    );
    let deadline = Instant::now() + timeout;
    let pid = child.id() as libc::pid_t;
    let mut timed_out = false;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                timed_out = true;
                // SAFETY: signalling the child's own process group.
                unsafe { libc::kill(-pid, libc::SIGTERM) };
                let grace = Instant::now() + Duration::from_secs(1);
                while Instant::now() < grace {
                    if let Ok(Some(_)) = child.try_wait() {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                // SAFETY: as above; the group may already be gone.
                unsafe { libc::kill(-pid, libc::SIGKILL) };
                let _ = child.wait();
                break None;
            }
            Err(_) => break None,
        }
    };
    // Descendants that keep a pipe open must not hold the installer hostage.
    let collect = |receiver: mpsc::Receiver<Vec<u8>>| {
        receiver
            .recv_timeout(Duration::from_secs(2))
            .unwrap_or_default()
    };
    let captured = Captured {
        stdout: collect(stdout),
        stderr: collect(stderr),
    };
    let result = if timed_out {
        Err(Failure::TimedOut {
            seconds: timeout.as_secs(),
            command: display.to_owned(),
        })
    } else {
        match status {
            Some(status) if status.success() => Ok(()),
            Some(status) => Err(Failure::Status(status.code())),
            None => Err(Failure::Status(None)),
        }
    };
    Ran { captured, result }
}

/// Message levels for relayed command output.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Info,
    Warn,
    Err,
}

/// Classify a line like the shell installer: a whole word `error`, `err`,
/// `fatal`, or `panic` is an error; `warning` or `warn` a warning.
pub fn classify(line: &str, default: Level) -> Level {
    let words: Vec<String> = line
        .split(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if words
        .iter()
        .any(|w| matches!(w.as_str(), "error" | "err" | "fatal" | "panic"))
    {
        Level::Err
    } else if words
        .iter()
        .any(|w| matches!(w.as_str(), "warning" | "warn"))
    {
        Level::Warn
    } else {
        default
    }
}

/// Write one relayed line: `[LEVEL] (SOURCE) TEXT`. `source` is empty for
/// `leanck` itself. Quiet output drops only `[INFO]` lines.
pub fn relay_line<W: Write>(console: &mut Console<W>, source: &str, level: Level, text: &str) {
    if console.quiet && level == Level::Info {
        return;
    }
    let style = console.style;
    let (color, name) = match level {
        Level::Info => (style.cyan, "INFO"),
        Level::Warn => (style.yellow, "WARN"),
        Level::Err => (style.red, "ERR"),
    };
    let label = if source.is_empty() {
        String::new()
    } else {
        format!(" ({source})")
    };
    console.raw(&format!("{color}[{name}]{}{label} {text}\n", style.reset));
}

/// Relay a command's output, then its failure line, if any.
pub fn relay<W: Write>(console: &mut Console<W>, source: &str, ran: &Ran) {
    relay_captured(console, source, &ran.captured, true);
    if let Err(failure) = &ran.result
        && let Some(line) = failure.line()
    {
        relay_line(console, source, Level::Err, &line);
    }
}

/// Relay captured output; standard output is skipped when it is data.
pub fn relay_captured<W: Write>(
    console: &mut Console<W>,
    source: &str,
    captured: &Captured,
    stdout: bool,
) {
    let streams: &[&[u8]] = if stdout {
        &[&captured.stdout, &captured.stderr]
    } else {
        &[&captured.stderr]
    };
    for stream in streams {
        for line in String::from_utf8_lossy(stream).lines() {
            if !line.is_empty() {
                relay_line(console, source, classify(line, Level::Info), line);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::installer::Style;

    #[test]
    fn lines_are_classified_by_whole_words() {
        for (line, level) in [
            ("error: host error", Level::Err),
            ("Warning: host warning", Level::Warn),
            ("host stdout", Level::Info),
            ("terror and warnings", Level::Info),
            ("thread panic", Level::Err),
            ("[warn] careful", Level::Warn),
        ] {
            assert_eq!(classify(line, Level::Info), level, "{line}");
        }
    }

    #[test]
    fn relayed_lines_have_level_and_source_and_quiet_keeps_problems() {
        let mut console = Console::new(vec![], Style::PLAIN, false);
        let ran = Ran {
            captured: Captured {
                stdout: b"host stdout\n".to_vec(),
                stderr: b"Warning: host warning\nerror: host error".to_vec(),
            },
            result: Err(Failure::TimedOut {
                seconds: 1,
                command: "claude plugin marketplace add /b".into(),
            }),
        };
        relay(&mut console, "claude", &ran);
        assert_eq!(
            String::from_utf8(console.into_inner()).unwrap(),
            "[INFO] (claude) host stdout\n[WARN] (claude) Warning: host warning\n\
             [ERR] (claude) error: host error\n\
             [ERR] (claude) Command timed out after 1 seconds: claude plugin marketplace add /b\n"
        );
        let mut console = Console::new(vec![], Style::PLAIN, true);
        relay(&mut console, "", &ran);
        let text = String::from_utf8(console.into_inner()).unwrap();
        assert!(!text.contains("[INFO]") && text.contains("[WARN] Warning: host warning\n"));
    }

    #[test]
    fn bounded_commands_capture_output_and_stop_at_the_timeout() {
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "echo out; echo err >&2; exit 3"]);
        let ran = run_bounded(command, Duration::from_secs(5), "sh");
        assert_eq!(ran.captured.stdout, b"out\n");
        assert_eq!(ran.captured.stderr, b"err\n");
        assert_eq!(ran.result, Err(Failure::Status(Some(3))));
        let started = Instant::now();
        let mut command = Command::new("/bin/sh");
        command.args(["-c", "sleep 30 & sleep 30"]);
        let ran = run_bounded(command, Duration::from_secs(1), "sh sleeper");
        assert!(started.elapsed() < Duration::from_secs(6));
        assert_eq!(
            ran.result,
            Err(Failure::TimedOut {
                seconds: 1,
                command: "sh sleeper".into()
            })
        );
    }

    #[test]
    fn system_hosts_search_their_own_path_and_set_the_home_variable() {
        let (_dir, root) = crate::test_support::scratch();
        let script = root.join("claude");
        std::fs::write(
            &script,
            "#!/bin/sh\nprintf '%s|%s|%s\\n' \"$CLAUDE_CONFIG_DIR\" \"$*\" \"${PATH%%:*}\"\n",
        )
        .unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
        let hosts = SystemHosts {
            path: format!("{}:/usr/bin:/bin", root.display()).into(),
            timeout: Duration::from_secs(10),
        };
        assert!(hosts.available(HostName::Claude));
        assert!(!hosts.available(HostName::Codex));
        let mut call = HostCall::new(HostName::Claude, Path::new("/h"), &["plugin", "list"]);
        call.path_prefix = Some("/bin dir".into());
        let ran = hosts.run(&call);
        assert_eq!(
            String::from_utf8(ran.captured.stdout).unwrap(),
            "/h|plugin list|/bin dir\n"
        );
        assert_eq!(
            hosts
                .run(&HostCall::new(HostName::Codex, Path::new("/h"), &[]))
                .result,
            Err(Failure::Missing("codex".into()))
        );
    }
}
