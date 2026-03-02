//! JSON-RPC 2.0 client over the Lean server's stdio.
//!
//! One reader task owns the server's stdout (routes responses to the pending-id
//! map; routes notifications to a sink; replies `null` to server→client
//! requests so the server never blocks). One writer task owns stdin. Every
//! [`JsonRpc::request`] is bounded by a timeout and a [`CancellationToken`];
//! on either, it removes the pending entry and fires `$/cancelRequest`.

use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{json, Value};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{mpsc, oneshot};
use tokio_util::sync::CancellationToken;

use super::transport;
use crate::error::{Error, Result};

/// Callback invoked for every server notification (`method`, `params`).
pub type NotifySink = Arc<dyn Fn(String, Value) + Send + Sync>;

type Reply = std::result::Result<Value, Error>;

pub struct JsonRpc {
    next_id: AtomicI64,
    pending: Mutex<HashMap<i64, oneshot::Sender<Reply>>>,
    outgoing: mpsc::UnboundedSender<Vec<u8>>,
    /// Cancelled when the reader task observes EOF/error on the server stdout.
    dead: CancellationToken,
}

impl JsonRpc {
    /// Spawn reader + writer tasks over the given stdio halves and return a
    /// handle. `on_notify` receives every server notification.
    pub fn spawn<R, W>(reader: R, writer: W, on_notify: NotifySink) -> Arc<JsonRpc>
    where
        R: AsyncRead + Unpin + Send + 'static,
        W: AsyncWrite + Unpin + Send + 'static,
    {
        let (tx, rx) = mpsc::unbounded_channel::<Vec<u8>>();
        let rpc = Arc::new(JsonRpc {
            next_id: AtomicI64::new(1),
            pending: Mutex::new(HashMap::new()),
            outgoing: tx,
            dead: CancellationToken::new(),
        });

        tokio::spawn(writer_loop(writer, rx));
        tokio::spawn(reader_loop(reader, rpc.clone(), on_notify));
        rpc
    }

    /// Send a request and await the response, bounded by `timeout` and `cancel`.
    pub async fn request(
        &self,
        method: &str,
        params: Value,
        timeout: Duration,
        cancel: &CancellationToken,
    ) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let req = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let frame = transport::encode_message(&serde_json::to_vec(&req)?);
        if self.outgoing.send(frame).is_err() {
            self.pending.lock().unwrap().remove(&id);
            return Err(Error::ServerDown("writer closed".into()));
        }

        let result = tokio::select! {
            biased;
            r = rx => r.unwrap_or_else(|_| Err(Error::ServerDown("response channel dropped".into()))),
            _ = tokio::time::sleep(timeout) => Err(Error::Timeout),
            _ = cancel.cancelled() => Err(Error::Cancelled),
            _ = self.dead.cancelled() => Err(Error::ServerDown("server died".into())),
        };

        if matches!(result, Err(Error::Timeout) | Err(Error::Cancelled)) {
            self.pending.lock().unwrap().remove(&id);
            // Best-effort cancellation; the server may still send a late reply,
            // which the reader drops because the pending entry is gone.
            let _ = self.send_notification("$/cancelRequest", json!({ "id": id }));
        }
        result
    }

    /// Send a request with no external cancellation.
    pub async fn request_simple(&self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        let never = CancellationToken::new();
        self.request(method, params, timeout, &never).await
    }

    /// Send a fire-and-forget notification.
    pub fn send_notification(&self, method: &str, params: Value) -> Result<()> {
        let n = json!({"jsonrpc": "2.0", "method": method, "params": params});
        let frame = transport::encode_message(&serde_json::to_vec(&n)?);
        self.outgoing
            .send(frame)
            .map_err(|_| Error::ServerDown("writer closed".into()))
    }

    /// A clone of the token that fires when the server stdout closes.
    pub fn dead_token(&self) -> CancellationToken {
        self.dead.clone()
    }

    pub fn is_dead(&self) -> bool {
        self.dead.is_cancelled()
    }

    fn handle_incoming(&self, body: &[u8], on_notify: &NotifySink) {
        let Ok(v) = serde_json::from_slice::<Value>(body) else {
            return;
        };
        let has_id = v.get("id").is_some_and(|x| !x.is_null());
        let method = v.get("method").and_then(|m| m.as_str());

        match (has_id, method) {
            // Server → client request: reply null so the server doesn't block
            // (covers client/registerCapability, window/workDoneProgress/create).
            (true, Some(_)) => {
                let id = v.get("id").cloned().unwrap_or(Value::Null);
                let resp = json!({"jsonrpc": "2.0", "id": id, "result": null});
                if let Ok(bytes) = serde_json::to_vec(&resp) {
                    let _ = self.outgoing.send(transport::encode_message(&bytes));
                }
            }
            // Notification.
            (false, Some(m)) => {
                let params = v.get("params").cloned().unwrap_or(Value::Null);
                on_notify(m.to_string(), params);
            }
            // Response.
            (true, None) => {
                if let Some(id) = v.get("id").and_then(|x| x.as_i64()) {
                    if let Some(tx) = self.pending.lock().unwrap().remove(&id) {
                        let reply = match v.get("error") {
                            Some(err) => Err(Error::Lsp {
                                code: err.get("code").and_then(|c| c.as_i64()).unwrap_or(0),
                                message: err
                                    .get("message")
                                    .and_then(|m| m.as_str())
                                    .unwrap_or_default()
                                    .to_string(),
                            }),
                            None => Ok(v.get("result").cloned().unwrap_or(Value::Null)),
                        };
                        let _ = tx.send(reply);
                    }
                }
            }
            (false, None) => { /* malformed; ignore */ }
        }
    }

    fn fail_all_pending(&self) {
        let mut p = self.pending.lock().unwrap();
        for (_, tx) in p.drain() {
            let _ = tx.send(Err(Error::ServerDown("server stdout closed".into())));
        }
    }
}

async fn writer_loop<W: AsyncWrite + Unpin>(mut w: W, mut rx: mpsc::UnboundedReceiver<Vec<u8>>) {
    while let Some(frame) = rx.recv().await {
        if w.write_all(&frame).await.is_err() {
            break;
        }
        if w.flush().await.is_err() {
            break;
        }
    }
}

async fn reader_loop<R: AsyncRead + Unpin>(reader: R, rpc: Arc<JsonRpc>, on_notify: NotifySink) {
    let mut buf = BufReader::new(reader);
    loop {
        match transport::read_message(&mut buf).await {
            Ok(Some(body)) => rpc.handle_incoming(&body, &on_notify),
            Ok(None) | Err(_) => break,
        }
    }
    rpc.dead.cancel();
    rpc.fail_all_pending();
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::BufReader;

    /// A tiny in-process "server" over one end of a duplex pipe.
    async fn read_one(br: &mut BufReader<tokio::io::ReadHalf<tokio::io::DuplexStream>>) -> Value {
        let body = transport::read_message(br).await.unwrap().unwrap();
        serde_json::from_slice(&body).unwrap()
    }

    #[tokio::test]
    async fn request_gets_a_response() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (cr, cw) = tokio::io::split(client_io);
        let rpc = JsonRpc::spawn(cr, cw, Arc::new(|_, _| {}));

        let server = tokio::spawn(async move {
            let (sr, mut sw) = tokio::io::split(server_io);
            let mut br = BufReader::new(sr);
            let req = read_one(&mut br).await;
            let resp = json!({"jsonrpc": "2.0", "id": req["id"], "result": {"echo": req["params"]}});
            sw.write_all(&transport::encode_message(&serde_json::to_vec(&resp).unwrap()))
                .await
                .unwrap();
            sw.flush().await.unwrap();
        });

        let token = CancellationToken::new();
        let res = rpc
            .request("ping", json!({"x": 1}), Duration::from_secs(2), &token)
            .await
            .unwrap();
        assert_eq!(res["echo"]["x"], 1);
        server.await.unwrap();
    }

    #[tokio::test]
    async fn notification_reaches_the_sink() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (cr, cw) = tokio::io::split(client_io);
        let (tx, mut rx) = mpsc::unbounded_channel::<(String, Value)>();
        let _rpc = JsonRpc::spawn(
            cr,
            cw,
            Arc::new(move |m, p| {
                let _ = tx.send((m, p));
            }),
        );

        let (_sr, mut sw) = tokio::io::split(server_io);
        let note = json!({"jsonrpc": "2.0", "method": "$/lean/fileProgress", "params": {"v": 7}});
        sw.write_all(&transport::encode_message(&serde_json::to_vec(&note).unwrap()))
            .await
            .unwrap();
        sw.flush().await.unwrap();

        let (method, params) = tokio::time::timeout(Duration::from_secs(2), rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(method, "$/lean/fileProgress");
        assert_eq!(params["v"], 7);
    }

    #[tokio::test]
    async fn timeout_fires_cancel_request() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (cr, cw) = tokio::io::split(client_io);
        let rpc = JsonRpc::spawn(cr, cw, Arc::new(|_, _| {}));

        // Server reads the request but never replies; it should then see a
        // `$/cancelRequest` for the same id.
        let server = tokio::spawn(async move {
            let (sr, _sw) = tokio::io::split(server_io);
            let mut br = BufReader::new(sr);
            let req = read_one(&mut br).await;
            let cancel = read_one(&mut br).await;
            (req, cancel)
        });

        let token = CancellationToken::new();
        let res = rpc
            .request("slow", json!(null), Duration::from_millis(80), &token)
            .await;
        assert!(matches!(res, Err(Error::Timeout)));

        let (req, cancel) = server.await.unwrap();
        assert_eq!(cancel["method"], "$/cancelRequest");
        assert_eq!(cancel["params"]["id"], req["id"]);
    }

    #[tokio::test]
    async fn server_death_fails_request() {
        let (client_io, server_io) = tokio::io::duplex(64 * 1024);
        let (cr, cw) = tokio::io::split(client_io);
        let rpc = JsonRpc::spawn(cr, cw, Arc::new(|_, _| {}));

        // Drop the server end without replying → reader hits EOF.
        drop(server_io);

        let token = CancellationToken::new();
        let res = rpc
            .request("x", json!(null), Duration::from_secs(5), &token)
            .await;
        assert!(matches!(res, Err(Error::ServerDown(_))));
    }
}
