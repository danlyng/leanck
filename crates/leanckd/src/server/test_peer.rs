//! Shared library-framed fake peer for document/session concurrency tests.
use crate::{
    server::{LeanSession, io::StopRead},
    state::ElaborationStore,
};
use lsp_server::{Message, Notification};
use std::{sync::Arc, time::Duration};
use tokio::{
    io::{AsyncWriteExt, BufReader},
    sync::mpsc,
};
use tokio_util::{io::SyncIoBridge, sync::CancellationToken};
pub(crate) struct Peer {
    pub(crate) writer: tokio::io::WriteHalf<tokio::io::DuplexStream>,
    rx: mpsc::UnboundedReceiver<Message>,
    stop: CancellationToken,
    reader: Option<tokio::task::JoinHandle<()>>,
}
impl Peer {
    pub(crate) async fn next(&mut self) -> Message {
        tokio::time::timeout(Duration::from_secs(2), self.rx.recv())
            .await
            .unwrap()
            .unwrap()
    }
    pub(crate) async fn request(&mut self, method: &str) -> lsp_server::Request {
        let Message::Request(request) = self.next().await else {
            panic!("expected request {method}")
        };
        assert_eq!(request.method, method);
        request
    }
    pub(crate) async fn notify(&mut self, method: &str) -> Notification {
        let Message::Notification(notification) = self.next().await else {
            panic!("expected notification {method}")
        };
        assert_eq!(notification.method, method);
        notification
    }
    pub(crate) async fn send(&mut self, message: impl Into<Message>) {
        let mut bytes = Vec::new();
        message.into().write(&mut bytes).unwrap();
        self.writer.write_all(&bytes).await.unwrap();
    }
    pub(crate) async fn finish(&mut self) {
        self.stop.cancel();
        if let Some(reader) = self.reader.take() {
            reader.await.unwrap();
        }
    }
}
impl Drop for Peer {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}
pub(crate) fn pair(state: Arc<ElaborationStore>) -> (Arc<LeanSession>, Peer) {
    let (client, peer) = tokio::io::duplex(65536);
    let (r, w) = tokio::io::split(client);
    let session = LeanSession::spawn(r, w, state.clone());
    let (r, writer) = tokio::io::split(peer);
    let stop = CancellationToken::new();
    let mut reader = SyncIoBridge::new(BufReader::new(StopRead::new(r, stop.clone())));
    let (tx, rx) = mpsc::unbounded_channel();
    let reader = tokio::task::spawn_blocking(move || {
        while let Ok(Some(msg)) = Message::read(&mut reader) {
            if tx.send(msg).is_err() {
                return;
            }
        }
    });
    (
        session,
        Peer {
            writer,
            rx,
            stop,
            reader: Some(reader),
        },
    )
}
