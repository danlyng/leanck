//! Shared fake Unix-socket daemon fixtures. No Lean server or external daemon is launched.
#![allow(dead_code)]
use leanck_core::{
    api::{Request, Response},
    paths::RuntimePaths,
    wire,
};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use tokio::net::{UnixListener, UnixStream};

pub struct Scratch(pub PathBuf);
impl Scratch {
    pub fn new() -> Self {
        static NEXT: AtomicU32 = AtomicU32::new(0);
        let path = PathBuf::from("/tmp").join(format!(
            "lcc-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.canonicalize().unwrap())
    }
    pub fn listener(&self) -> UnixListener {
        let runtime = RuntimePaths::for_workspace(&self.0).prepare().unwrap();
        UnixListener::bind(runtime.socket()).unwrap()
    }
}
impl Drop for Scratch {
    fn drop(&mut self) {
        let runtime = RuntimePaths::for_workspace(&self.0);
        for path in [
            runtime.socket(),
            runtime.pid(),
            runtime.lock(),
            runtime.log(),
        ] {
            let _ = std::fs::remove_file(path);
        }
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub async fn request(listener: &UnixListener) -> (UnixStream, Request) {
    loop {
        let (mut stream, _) = listener.accept().await.unwrap();
        // Readiness probes connect and close without sending a frame.
        if let Some(frame) = wire::read_frame(&mut wire::reader(&mut stream))
            .await
            .unwrap()
        {
            let request: Request = serde_json::from_slice(&frame).unwrap();
            if request.id.starts_with("compatibility-") && request.op == "health" {
                let fixtures: serde_json::Value =
                    serde_json::from_str(include_str!("contracts/v4.json")).unwrap();
                respond(
                    &mut stream,
                    Response::ok(request.id, fixtures["health"].clone()),
                )
                .await;
                continue;
            }
            return (stream, request);
        }
    }
}

pub async fn respond(stream: &mut UnixStream, response: Response) {
    wire::write_frame(
        &mut wire::writer(stream),
        &serde_json::to_vec(&response).unwrap(),
    )
    .await
    .unwrap();
}
