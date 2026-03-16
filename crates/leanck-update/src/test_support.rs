use std::sync::{
    Arc, Mutex,
    atomic::{AtomicUsize, Ordering},
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

pub struct Server {
    pub url: String,
    pub calls: Arc<AtomicUsize>,
    pub body: Arc<Mutex<(u16, Vec<u8>)>>,
    task: tokio::task::JoinHandle<()>,
}
impl Server {
    pub async fn new(body: impl Into<Vec<u8>>) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let calls = Arc::new(AtomicUsize::new(0));
        let response = Arc::new(Mutex::new((200, body.into())));
        let c = calls.clone();
        let b = response.clone();
        let task = tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else {
                    break;
                };
                let mut request = vec![0; 8192];
                let _ = socket.read(&mut request).await;
                c.fetch_add(1, Ordering::SeqCst);
                let (status, body) = b.lock().unwrap().clone();
                let header = format!(
                    "HTTP/1.1 {status} Response\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                let _ = socket.write_all(&body).await;
            }
        });
        Self {
            url,
            calls,
            body: response,
            task,
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

pub fn scratch() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().canonicalize().unwrap();
    (dir, path)
}
