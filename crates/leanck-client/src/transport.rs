use crate::error::{ClientError, Result, TransportPhase};
use crate::{CancellationToken, PreparedRequest};
use leanck_core::{
    api::{CancelParams, Command, Response},
    error::ProtocolError,
    wire,
};
use serde_json::Value;
use std::{
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::net::UnixStream;

#[cfg(test)]
pub(crate) async fn call(socket: &Path, request: &PreparedRequest) -> Result<Value> {
    call_with_options(socket, request, Duration::from_secs(301), None, true).await
}
pub(crate) async fn call_with_options(
    socket: &Path,
    request: &PreparedRequest,
    budget: Duration,
    cancel: Option<&CancellationToken>,
    cancel_on_drop: bool,
) -> Result<Value> {
    let never = CancellationToken::new();
    let cancel = cancel.unwrap_or(&never);
    let mut phase = TransportPhase::Connect;
    let mut cleanup = CancelOnDrop {
        socket: socket.into(),
        id: request.id.clone(),
        armed: false,
    };
    let work = async {
        let stream =
            UnixStream::connect(socket)
                .await
                .map_err(|source| ClientError::Transport {
                    phase: TransportPhase::Connect,
                    source,
                })?;
        let mut stream = wire::framed(stream);
        phase = TransportPhase::Write;
        cleanup.armed = cancel_on_drop;
        wire::write_frame(&mut stream, &request.bytes)
            .await
            .map_err(|source| ClientError::Transport {
                phase: TransportPhase::Write,
                source,
            })?;
        phase = TransportPhase::Read;
        let frame = wire::read_frame(&mut stream)
            .await
            .map_err(|source| ClientError::Transport {
                phase: TransportPhase::Read,
                source,
            })?
            .ok_or_else(|| ClientError::Transport {
                phase: TransportPhase::Read,
                source: std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "daemon closed the connection",
                ),
            })?;
        let response: Response = serde_json::from_slice(&frame)
            .map_err(|source| ClientError::Protocol(ProtocolError::Envelope(source)))?;
        response
            .into_result(&request.id)
            .map_err(ClientError::Protocol)?
            .map_err(|error| ClientError::Daemon {
                kind: error.kind,
                message: error.message,
            })
    };
    // The clock bounds connect, a partial write, and an unresponsive daemon.
    let result = tokio::select! {
        biased;
        _ = cancel.cancelled() => None,
        result = tokio::time::timeout(budget, work) => Some(result),
    };
    match result {
        Some(Ok(result)) => {
            cleanup.armed = false;
            result
        }
        timed_or_cancelled => {
            // Explicit cancellation awaits its bounded control message. Drop
            // cancellation is scheduled by the same guard on a live runtime.
            if cleanup.armed {
                cleanup.armed = false;
                send_cancel(&cleanup.socket, &cleanup.id).await;
            }
            match timed_or_cancelled {
                None => Err(ClientError::Cancelled {
                    delivery_uncertain: phase != TransportPhase::Connect,
                }),
                _ => Err(ClientError::TransportTimeout { phase }),
            }
        }
    }
}
struct CancelOnDrop {
    socket: PathBuf,
    id: String,
    armed: bool,
}
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        if self.armed
            && let Ok(runtime) = tokio::runtime::Handle::try_current()
        {
            let socket = self.socket.clone();
            let id = self.id.clone();
            runtime.spawn(async move {
                send_cancel(&socket, &id).await;
            });
        }
    }
}
async fn send_cancel(socket: &Path, id: &str) {
    let _ = tokio::time::timeout(Duration::from_secs(1), async {
        let Ok(request) = Command::Cancel(CancelParams {
            target_id: id.into(),
        })
        .into_request(uuid::Uuid::new_v4().to_string(), None) else {
            return;
        };
        let Ok(bytes) = serde_json::to_vec(&request) else {
            return;
        };
        let Ok(stream) = UnixStream::connect(socket).await else {
            return;
        };
        let mut stream = wire::framed(stream);
        if wire::write_frame(&mut stream, &bytes).await.is_ok() {
            let _ = wire::read_frame(&mut stream).await;
        }
    })
    .await;
}
pub(crate) async fn can_connect(socket: &Path) -> bool {
    matches!(
        tokio::time::timeout(Duration::from_secs(2), UnixStream::connect(socket)).await,
        Ok(Ok(_))
    )
}
