//! lsp-server owns message parsing/framing and request correlation. This adapter
//! supplies cancellable child I/O and the daemon's pending-request lifetime.
use crate::error::{Error, Result};
use lsp_server::{Message, Notification, ReqQueue, RequestId};
use lsp_types::{notification::Notification as LspNotification, request::Request as LspRequest};
use serde_json::Value;
use std::{
    future::Future,
    io,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
    task::{Context, Poll},
};
use tokio::{
    io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader, ReadBuf},
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use tokio_util::{
    io::SyncIoBridge,
    sync::{CancellationToken, WaitForCancellationFutureOwned},
};

type Reply = oneshot::Sender<std::result::Result<Value, lsp_server::ResponseError>>;
pub(super) type NotificationHandler = Arc<dyn Fn(Notification) -> Result<()> + Send + Sync>;
pub(super) type RequestHandler =
    Arc<dyn Fn(lsp_server::Request) -> lsp_server::Response + Send + Sync>;

enum Write {
    Message(Message),
    Flush(oneshot::Sender<io::Result<()>>),
}
pub(super) struct SessionIo {
    pending: Mutex<ReqQueue<(), Reply>>,
    outgoing: mpsc::UnboundedSender<Write>,
    dead: CancellationToken,
    failure: Mutex<Option<String>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
}
impl SessionIo {
    pub fn spawn<R, W>(
        reader: R,
        writer: W,
        notifications: NotificationHandler,
        requests: RequestHandler,
    ) -> Arc<Self>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, rx) = mpsc::unbounded_channel();
        let io = Arc::new(Self {
            pending: Mutex::new(ReqQueue::default()),
            outgoing: tx,
            dead: CancellationToken::new(),
            failure: Mutex::new(None),
            tasks: Mutex::new(Vec::new()),
        });
        // One blocking parser per workspace, bridged to cancellable Tokio I/O.
        // Cancellation wakes a reader even if its child never writes again.
        let reader = SyncIoBridge::new(BufReader::new(StopRead::new(reader, io.dead.clone())));
        let weak = Arc::downgrade(&io);
        let dead = io.dead.clone();
        let reader = tokio::task::spawn_blocking(move || {
            read_loop(reader, weak, dead, notifications, requests)
        });
        let writer = tokio::spawn(write_loop(writer, rx, Arc::downgrade(&io), io.dead.clone()));
        *io.tasks.lock().unwrap() = vec![reader, writer];
        io
    }
    pub async fn request<R: LspRequest>(&self, params: R::Params) -> Result<R::Result> {
        let (tx, rx) = oneshot::channel();
        let request = {
            let mut pending = self.pending.lock().unwrap();
            if self.is_dead() {
                return Err(self.unavailable());
            }
            pending.outgoing.register(R::METHOD.into(), params, tx)
        };
        let mut guard = Pending {
            io: self,
            id: request.id.clone(),
            sent: false,
        };
        self.send(Message::Request(request))?;
        guard.sent = true;
        let result = rx.await.map_err(|_| self.unavailable())?;
        match result {
            Ok(value) => Ok(serde_json::from_value(value)?),
            Err(error) => {
                tracing::debug!(code = error.code, message = %error.message, data = ?error.data, "Lean request failed");
                Err(Error::Lsp {
                    code: i64::from(error.code),
                    message: error.message,
                    data: error.data,
                })
            }
        }
    }
    pub fn notify<N: LspNotification>(&self, params: N::Params) -> Result<()> {
        self.send(Notification::new(N::METHOD.into(), params).into())
    }
    fn send(&self, message: Message) -> Result<()> {
        if self.is_dead() {
            return Err(self.unavailable());
        }
        self.outgoing
            .send(Write::Message(message))
            .map_err(|_| self.unavailable())
    }
    pub async fn flush(&self) -> Result<()> {
        let (tx, rx) = oneshot::channel();
        self.outgoing
            .send(Write::Flush(tx))
            .map_err(|_| self.unavailable())?;
        rx.await.map_err(|_| self.unavailable())??;
        Ok(())
    }
    #[cfg(test)]
    pub fn has_pending(&self) -> bool {
        self.pending.lock().unwrap().has_pending()
    }
    pub fn dead_token(&self) -> CancellationToken {
        self.dead.clone()
    }
    pub fn is_dead(&self) -> bool {
        self.dead.is_cancelled()
    }
    fn unavailable(&self) -> Error {
        Error::ServerDown(
            self.failure
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| "Lean session stopped".into()),
        )
    }
    fn stop(&self, reason: String) {
        let mut failure = self.failure.lock().unwrap();
        if failure.is_none() {
            *failure = Some(reason);
        }
        drop(failure);
        self.dead.cancel();
        // Dropping library-owned response senders wakes every pending future.
        let _ = std::mem::take(&mut *self.pending.lock().unwrap());
    }
    pub async fn shutdown(&self) {
        self.stop("Lean session shut down".into());
        let tasks = std::mem::take(&mut *self.tasks.lock().unwrap());
        for task in tasks {
            let _ = task.await;
        }
    }
}
struct Pending<'a> {
    io: &'a SessionIo,
    id: RequestId,
    sent: bool,
}
impl Drop for Pending<'_> {
    fn drop(&mut self) {
        let removed = self
            .io
            .pending
            .lock()
            .unwrap()
            .outgoing
            .complete(self.id.clone())
            .is_some();
        if removed && self.sent {
            // The library supplies IDs; the guard is the Lean cancellation policy.
            #[derive(serde::Serialize)]
            struct Cancel {
                id: RequestId,
            }
            let _ = self.io.send(
                Notification::new(
                    lsp_types::notification::Cancel::METHOD.into(),
                    Cancel {
                        id: self.id.clone(),
                    },
                )
                .into(),
            );
        }
    }
}
impl Drop for SessionIo {
    fn drop(&mut self) {
        self.dead.cancel();
        for task in self.tasks.get_mut().unwrap().drain(..) {
            task.abort();
        }
    }
}
pub(super) struct StopRead<R> {
    inner: R,
    stop: Pin<Box<WaitForCancellationFutureOwned>>,
}
impl<R> StopRead<R> {
    pub fn new(inner: R, stop: CancellationToken) -> Self {
        Self {
            inner,
            stop: Box::pin(stop.cancelled_owned()),
        }
    }
}
impl<R: AsyncRead + Unpin> AsyncRead for StopRead<R> {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.stop.as_mut().poll(cx).is_ready() {
            return Poll::Ready(Err(io::ErrorKind::ConnectionAborted.into()));
        }
        Pin::new(&mut self.inner).poll_read(cx, buf)
    }
}
fn read_loop<R: io::BufRead>(
    mut reader: R,
    weak: Weak<SessionIo>,
    dead: CancellationToken,
    notifications: NotificationHandler,
    requests: RequestHandler,
) {
    while !dead.is_cancelled() {
        let message = Message::read(&mut reader);
        let Some(io) = weak.upgrade() else {
            return;
        };
        let result = match message {
            Ok(Some(Message::Response(response))) => {
                if let Some(tx) = io.pending.lock().unwrap().outgoing.complete(response.id) {
                    let _ = tx.send(response.response_result);
                }
                Ok(())
            }
            Ok(Some(Message::Notification(notification))) => notifications(notification),
            Ok(Some(Message::Request(request))) => io.send(requests(request).into()),
            Ok(None) => Err(Error::ServerDown("Lean stdout closed".into())),
            Err(error) => Err(Error::Io(error)),
        };
        if let Err(error) = result {
            io.stop(error.to_string());
            return;
        }
    }
}
async fn write_loop<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut rx: mpsc::UnboundedReceiver<Write>,
    weak: Weak<SessionIo>,
    dead: CancellationToken,
) {
    loop {
        let result = tokio::select! { biased;
            _ = dead.cancelled() => return,
            result = async {
                let Some(write) = rx.recv().await else { return Err(io::ErrorKind::BrokenPipe.into()); };
                match write {
                    Write::Message(message) => {
                        let mut bytes = Vec::new(); message.write(&mut bytes)?;
                        writer.write_all(&bytes).await?; writer.flush().await
                    }
                    Write::Flush(tx) => {
                        let result = writer.flush().await;
                        let failed = result.is_err(); let _ = tx.send(result);
                        if failed { Err(io::ErrorKind::BrokenPipe.into()) } else { Ok(()) }
                    }
                }
            } => result
        };
        if let Err(error) = result {
            if let Some(io) = weak.upgrade() {
                io.stop(format!("Lean write failed: {error}"));
            }
            return;
        }
    }
}
