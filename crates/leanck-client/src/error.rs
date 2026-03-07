use leanck_core::{
    error::{ErrorKind, ProtocolError},
    workspace::WorkspaceError,
};
use std::path::PathBuf;
use thiserror::Error;

/// A write may have been partial, so every write/read failure has uncertain delivery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportPhase {
    Connect,
    Write,
    Read,
}

impl std::fmt::Display for TransportPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Connect => "connection (request not sent)",
            Self::Write => "request write (delivery uncertain)",
            Self::Read => "response read (delivery uncertain)",
        })
    }
}

#[derive(Debug, Error)]
pub enum ClientError {
    #[error(transparent)]
    Workspace(#[from] WorkspaceError),
    #[error("leanckd transport error during {phase}: {source}")]
    Transport {
        phase: TransportPhase,
        #[source]
        source: std::io::Error,
    },
    #[error("leanckd transport timed out during {phase}")]
    TransportTimeout { phase: TransportPhase },
    #[error("daemon operation cancelled (delivery uncertain: {delivery_uncertain})")]
    Cancelled { delivery_uncertain: bool },
    #[error(
        "incompatible leanckd protocol: expected {expected}, found {actual}; shut down the daemon explicitly to replace it"
    )]
    IncompatibleProtocol { expected: u32, actual: u64 },
    #[error("daemon compatibility probe failed before sending the operation: {0}")]
    Handshake(#[source] Box<ClientError>),
    #[error("could not serialize daemon request: {0}")]
    Encode(#[source] serde_json::Error),
    #[error(transparent)]
    Protocol(#[from] ProtocolError),
    #[error("leanckd error [{kind:?}]: {message}")]
    Daemon { kind: ErrorKind, message: String },
    #[error("could not prepare daemon runtime directory {directory:?}: {source}")]
    Prepare {
        directory: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("cannot open daemon log {path:?}: {source}")]
    Log {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("could not spawn daemon {binary:?}: {source}")]
    Spawn {
        binary: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("configured daemon binary {0:?} is not a file")]
    InvalidBinary(PathBuf),
    #[error(
        "could not find leanck to run `serve daemon`; configure client.daemon_bin or put leanck on PATH"
    )]
    BinaryNotFound,
    #[error("leanckd did not start within the timeout")]
    StartupTimeout,
    #[error("leanck daemon instance limit reached ({running} running, limit {limit}){}; list them with `leanck daemons`, free a slot with `leanck daemons stop --idle`, or raise instances.limit in the user configuration", if *busy { ": too few idle daemons could be evicted" } else { "" })]
    InstanceLimit {
        running: u64,
        limit: u64,
        busy: bool,
    },
    /// `code` is the exit status, or 128 plus the signal number.
    #[error("leanckd exited with status {code} before serving; see {log}")]
    DaemonExited { code: i32, log: PathBuf },
}

impl ClientError {
    /// Only a failed connection proves that the daemon did not receive the request.
    pub fn is_pre_dispatch(&self) -> bool {
        matches!(
            self,
            Self::Transport {
                phase: TransportPhase::Connect,
                ..
            }
        )
    }
}

pub type Result<T> = std::result::Result<T, ClientError>;
