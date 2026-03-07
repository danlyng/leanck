//! Small stdio MCP client for executable integration tests. Every stdout line
//! must be protocol JSON, including during startup and configuration warnings.
#![allow(dead_code)]
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines},
    process::{Child, ChildStdin, ChildStdout, Command},
};

pub struct Mcp {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Lines<BufReader<ChildStdout>>,
    id: u64,
}

impl Mcp {
    pub async fn start(mut command: Command) -> Self {
        let mut child = command
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap();
        let stdin = child.stdin.take();
        let lines = BufReader::new(child.stdout.take().unwrap()).lines();
        let mut client = Self {
            child,
            stdin,
            lines,
            id: 0,
        };
        client
            .call(
                "initialize",
                json!({
                    "protocolVersion": "2024-11-05", "capabilities": {},
                    "clientInfo": {"name": "serve-test", "version": "1"},
                }),
            )
            .await;
        client
            .send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))
            .await;
        client
    }

    async fn send(&mut self, value: Value) {
        let mut bytes = serde_json::to_vec(&value).unwrap();
        bytes.push(b'\n');
        let stdin = self.stdin.as_mut().unwrap();
        stdin.write_all(&bytes).await.unwrap();
        stdin.flush().await.unwrap();
    }

    pub async fn call(&mut self, method: &str, params: Value) -> Value {
        self.id += 1;
        self.send(json!({"jsonrpc": "2.0", "id": self.id, "method": method, "params": params}))
            .await;
        tokio::time::timeout(Duration::from_secs(20), async {
            loop {
                let line = self
                    .lines
                    .next_line()
                    .await
                    .unwrap()
                    .expect("MCP stdout open");
                let response: Value =
                    serde_json::from_str(&line).expect("only protocol JSON on stdout");
                if response["id"] == self.id {
                    assert!(response.get("error").is_none(), "{response}");
                    return response["result"].clone();
                }
            }
        })
        .await
        .expect("bounded MCP response")
    }

    pub async fn tool(&mut self, name: &str, arguments: Value) -> Value {
        let result = self
            .call("tools/call", json!({"name": name, "arguments": arguments}))
            .await;
        assert_ne!(result["isError"], true, "{result}");
        serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap()
    }

    pub async fn close(mut self) {
        drop(self.stdin.take());
        let status = tokio::time::timeout(Duration::from_secs(5), self.child.wait())
            .await
            .unwrap()
            .unwrap();
        assert!(status.success(), "{status}");
    }
}
