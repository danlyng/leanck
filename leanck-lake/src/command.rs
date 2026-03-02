//! Bounded Lake execution. The worker retains coordination until child cleanup finishes.
use serde::{Deserialize, Serialize};
use std::os::fd::AsRawFd;
use std::{
    path::{Path, PathBuf},
    process::Stdio,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt},
    time::Instant,
};
pub use tokio_util::sync::CancellationToken;

pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);
pub const DEFAULT_OUTPUT_LIMIT: usize = 8 * 1024 * 1024;
#[derive(Clone, Debug)]
pub struct ExecutionOptions {
    /// One budget covers coordination and execution. Cleanup completes before returning.
    pub timeout: Duration,
    pub cancellation: Option<CancellationToken>,
    /// Maximum bytes retained from each stream. Exceeding this fails and stops the job.
    pub max_output_bytes: usize,
}
impl Default for ExecutionOptions {
    fn default() -> Self {
        Self {
            timeout: DEFAULT_TIMEOUT,
            cancellation: None,
            max_output_bytes: DEFAULT_OUTPUT_LIMIT,
        }
    }
}
/// Captured output stream, named as in Lake's human-readable errors.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputStream {
    Stdout,
    Stderr,
}
impl std::fmt::Display for OutputStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Stdout => "stdout",
            Self::Stderr => "stderr",
        })
    }
}

/// Stable error categories for frontend JSON reports.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LakeErrorKind {
    Timeout,
    Cancelled,
    OutputLimit,
    InvalidOptions,
    Coordination,
    InvalidWorkspace,
    Spawn,
    Internal,
}

/// Invalid runner settings, checked before starting any work.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum ExecutionOptionsError {
    #[error("max_output_bytes must be positive")]
    ZeroOutputLimit,
    #[error("timeout is too large")]
    TimeoutTooLarge,
}

#[derive(Debug, thiserror::Error)]
pub enum LakeError {
    #[error(
        "could not run {executable:?} in {project:?}; is Lake on PATH (elan installed)?: {source}"
    )]
    Spawn {
        executable: PathBuf,
        project: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not access Lake project {path:?}: {source}")]
    Project {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not acquire Lake execution lock {path:?}: {source}")]
    Coordination {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not collect Lake output: {0}")]
    Output(#[source] std::io::Error),
    #[error(
        "Lake {stream} exceeded the {limit}-byte output limit; narrow the build targets or raise the output limit"
    )]
    OutputLimit { stream: OutputStream, limit: usize },
    #[error("invalid Lake execution options: {0}")]
    InvalidOptions(#[source] ExecutionOptionsError),
    #[error(
        "Lake command timed out (including coordination); raise the timeout to allow more time"
    )]
    TimedOut,
    #[error("Lake command cancelled")]
    Cancelled,
    #[error("Lake execution worker stopped before returning a result")]
    WorkerStopped,
}
impl LakeError {
    pub fn kind(&self) -> LakeErrorKind {
        match self {
            Self::TimedOut => LakeErrorKind::Timeout,
            Self::Cancelled => LakeErrorKind::Cancelled,
            Self::OutputLimit { .. } => LakeErrorKind::OutputLimit,
            Self::InvalidOptions(_) => LakeErrorKind::InvalidOptions,
            Self::Coordination { .. } => LakeErrorKind::Coordination,
            Self::Project { .. } => LakeErrorKind::InvalidWorkspace,
            Self::Spawn { .. } => LakeErrorKind::Spawn,
            Self::Output(_) | Self::WorkerStopped => LakeErrorKind::Internal,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionProvenance {
    /// Requested executable; PATH/toolchain resolution remains the operating system's job.
    pub executable: String,
    pub workspace: String,
    pub args: Vec<String>,
    pub elapsed_ms: u64,
    pub queue_ms: u64,
    pub coordinated: bool,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CommandOutput {
    pub success: bool,
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub provenance: ExecutionProvenance,
}
#[derive(Debug, Clone)]
pub enum CommandOutcome {
    Finished(CommandOutput),
    TimedOut,
}
/// Uses the project's own Lake on PATH by default. Explicit executables support isolated tests.
#[derive(Debug, Clone)]
pub struct Lake {
    executable: PathBuf,
}
impl Default for Lake {
    fn default() -> Self {
        Self::new("lake")
    }
}
impl Lake {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }
    /// Generic finite command, without build/cache coordination. None uses the bounded default.
    pub async fn run(
        &self,
        project: &Path,
        args: &[String],
        timeout: Option<Duration>,
    ) -> Result<CommandOutcome, LakeError> {
        self.run_with(
            project,
            args,
            ExecutionOptions {
                timeout: timeout.unwrap_or(DEFAULT_TIMEOUT),
                ..Default::default()
            },
        )
        .await
    }
    pub async fn run_with(
        &self,
        project: &Path,
        args: &[String],
        options: ExecutionOptions,
    ) -> Result<CommandOutcome, LakeError> {
        self.start(project, args, options, false).await
    }
    pub(crate) async fn coordinated(
        &self,
        project: &Path,
        args: &[String],
        options: ExecutionOptions,
    ) -> Result<CommandOutcome, LakeError> {
        self.start(project, args, options, true).await
    }
    async fn start(
        &self,
        project: &Path,
        args: &[String],
        options: ExecutionOptions,
        coordinated: bool,
    ) -> Result<CommandOutcome, LakeError> {
        if options.max_output_bytes == 0 {
            return Err(LakeError::InvalidOptions(
                ExecutionOptionsError::ZeroOutputLimit,
            ));
        }
        let started = Instant::now();
        let deadline = started
            .checked_add(options.timeout)
            .ok_or(LakeError::InvalidOptions(
                ExecutionOptionsError::TimeoutTooLarge,
            ))?;
        let cancel = options
            .cancellation
            .as_ref()
            .map(CancellationToken::child_token)
            .unwrap_or_default();
        if cancel.is_cancelled() {
            return Err(LakeError::Cancelled);
        }
        let _cancel_on_drop = CancelOnDrop(cancel.clone());
        let project = project.to_owned();
        let args = args.to_vec();
        let executable = self.executable.clone();
        let (tx, rx) = tokio::sync::oneshot::channel();
        // A dropped caller signals cancellation. This worker keeps the lock until
        // it has killed the process group and reaped the child, then returns/releases.
        tokio::spawn(async move {
            let result = run_job(
                project,
                args,
                executable,
                options,
                coordinated,
                ExecutionBudget {
                    started,
                    deadline,
                    cancel,
                },
            )
            .await;
            let result = match result {
                Ok(output) => Ok(CommandOutcome::Finished(output)),
                Err(LakeError::TimedOut) => Ok(CommandOutcome::TimedOut),
                Err(error) => Err(error),
            };
            let _ = tx.send(result);
        });
        rx.await.map_err(|_| LakeError::WorkerStopped)?
    }
}
struct CancelOnDrop(CancellationToken);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}
async fn bounded<T>(
    cancel: &CancellationToken,
    deadline: Instant,
    work: impl std::future::Future<Output = Result<T, LakeError>>,
) -> Result<T, LakeError> {
    tokio::select! {biased;_=cancel.cancelled()=>Err(LakeError::Cancelled),_=tokio::time::sleep_until(deadline)=>Err(LakeError::TimedOut),result=work=>result}
}
struct ExecutionBudget {
    started: Instant,
    deadline: Instant,
    cancel: CancellationToken,
}
async fn run_job(
    project: PathBuf,
    args: Vec<String>,
    executable: PathBuf,
    options: ExecutionOptions,
    coordinated: bool,
    budget: ExecutionBudget,
) -> Result<CommandOutput, LakeError> {
    let ExecutionBudget {
        started,
        deadline,
        cancel,
    } = budget;
    let cancel = &cancel;
    let project = bounded(cancel, deadline, async {
        tokio::fs::canonicalize(&project)
            .await
            .map_err(|source| LakeError::Project {
                path: project,
                source,
            })
    })
    .await?;
    let queued = Instant::now();
    let _lock = if coordinated {
        Some(bounded(cancel, deadline, crate::coordination::acquire(&project)).await?)
    } else {
        None
    };
    let queue_ms = if coordinated {
        millis(queued.elapsed())
    } else {
        0
    };
    // Check again before spawn: an expired/cancelled waiter must never launch Lake.
    if cancel.is_cancelled() {
        return Err(LakeError::Cancelled);
    }
    if Instant::now() >= deadline {
        return Err(LakeError::TimedOut);
    }
    let mut command = tokio::process::Command::new(&executable);
    command
        .args(&args)
        .current_dir(&project)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .kill_on_drop(true);
    if let Some(lock) = &_lock {
        let fd = lock.as_raw_fd();
        // Keep the same locked open-file description alive in Lake. If this
        // caller crashes, the next job must wait for its surviving child too.
        // SAFETY: the descriptor is live through spawn; the child callback only
        // uses async-signal-safe fcntl and modifies its own descriptor flags.
        unsafe {
            command.pre_exec(move || {
                let flags = libc::fcntl(fd, libc::F_GETFD);
                if flags < 0 || libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    let mut child = command.spawn().map_err(|source| LakeError::Spawn {
        executable: executable.clone(),
        project: project.clone(),
        source,
    })?;
    let group = ProcessGroup::from_child(&child);
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let collected = bounded(cancel, deadline, async {
        tokio::try_join!(
            async { child.wait().await.map_err(LakeError::Output) },
            read_output(stdout, OutputStream::Stdout, options.max_output_bytes),
            read_output(stderr, OutputStream::Stderr, options.max_output_bytes)
        )
    })
    .await;
    group.terminate();
    // Also handles timeout/cancellation/output failure. Keeping _lock in this
    // worker prevents a new coordinated command from starting during cleanup.
    child.wait().await.map_err(LakeError::Output)?;
    let (status, stdout, stderr) = collected?;
    Ok(CommandOutput {
        success: status.success(),
        exit_code: status.code(),
        stdout: String::from_utf8_lossy(&stdout).into_owned(),
        stderr: String::from_utf8_lossy(&stderr).into_owned(),
        provenance: ExecutionProvenance {
            executable: executable.to_string_lossy().into_owned(),
            workspace: project.to_string_lossy().into_owned(),
            args,
            elapsed_ms: millis(started.elapsed()),
            queue_ms,
            coordinated,
        },
    })
}
fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}
async fn read_output<R: AsyncRead + Unpin>(
    mut reader: R,
    stream: OutputStream,
    limit: usize,
) -> Result<Vec<u8>, LakeError> {
    let mut output = Vec::new();
    let mut chunk = [0; 8192];
    loop {
        let read = reader.read(&mut chunk).await.map_err(LakeError::Output)?;
        if read == 0 {
            return Ok(output);
        }
        if read > limit.saturating_sub(output.len()) {
            return Err(LakeError::OutputLimit { stream, limit });
        }
        tracing::debug!(target:"lake_output",stream = %stream,"{}",String::from_utf8_lossy(&chunk[..read]).trim_end());
        output.extend_from_slice(&chunk[..read]);
    }
}
/// Own a newly spawned `process_group(0)` group; termination includes descendants.
/// Only construct this from a child launched in its own process group.
pub struct ProcessGroup(std::sync::atomic::AtomicI32);
impl ProcessGroup {
    pub fn from_child(child: &tokio::process::Child) -> Self {
        Self(std::sync::atomic::AtomicI32::new(
            child.id().map(|id| id as i32).unwrap_or(0),
        ))
    }
    pub fn terminate(&self) {
        let pid = self.0.swap(0, std::sync::atomic::Ordering::AcqRel);
        if pid > 1 {
            unsafe {
                libc::kill(-pid, libc::SIGKILL);
            }
        }
    }
}
impl Drop for ProcessGroup {
    fn drop(&mut self) {
        self.terminate();
    }
}
