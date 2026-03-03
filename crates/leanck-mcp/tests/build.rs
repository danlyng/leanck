//! Exercise actual MCP stdio cancellation and the next build's lock reuse.
#[path = "../../../test-support/fake_lake.rs"]
mod fake_lake;
use fake_lake::{Project, alive, wait_file};
use serde_json::{Value, json};
use std::{process::Stdio, time::Duration};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
async fn send(stdin: &mut tokio::process::ChildStdin, value: Value) {
    let mut bytes = serde_json::to_vec(&value).unwrap();
    bytes.push(b'\n');
    stdin.write_all(&bytes).await.unwrap();
}
async fn response(
    lines: &mut tokio::io::Lines<BufReader<tokio::process::ChildStdout>>,
    id: u64,
) -> Value {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let line = lines
                .next_line()
                .await
                .unwrap()
                .expect("MCP stdout remained open");
            let value: Value = serde_json::from_str(&line).unwrap();
            if value["id"] == id {
                return value;
            }
        }
    })
    .await
    .unwrap()
}
#[tokio::test]
async fn mcp_build_cancellation_stops_lake_and_allows_the_next_standalone_build() {
    let project = Project::new(
        "if [ ! -f attempted ]; then touch attempted; printf '%s' \"$$\" > lake.pid.tmp; mv lake.pid.tmp lake.pid; exec sleep 30; fi\nprintf '%s' \"$$\" > lake.pid.tmp\nmv lake.pid.tmp lake.pid\nprintf '%s\\n' \"$@\" > args\nprintf 'warning: configuration warning\\n'",
    );
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_leanck-mcp"))
        .arg("--workspace")
        .arg(&project.0)
        .env("PATH", project.path_env())
        .arg("--set")
        .arg(format!(
            "client.daemon_bin={}",
            serde_json::to_string(&project.0.join("missing-daemon")).unwrap()
        ))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    let mut lines = BufReader::new(child.stdout.take().unwrap()).lines();
    send(&mut stdin,json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"build-test","version":"1"}}})).await;
    assert!(response(&mut lines, 1).await.get("error").is_none());
    send(
        &mut stdin,
        json!({"jsonrpc":"2.0","method":"notifications/initialized"}),
    )
    .await;
    send(&mut stdin,json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"build","arguments":{"json":true}}})).await;
    wait_file(&project.0.join("lake.pid")).await;
    let pid = std::fs::read_to_string(project.0.join("lake.pid")).unwrap();
    send(&mut stdin,json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2,"reason":"cancel build"}})).await;
    tokio::time::timeout(Duration::from_secs(3), async {
        while alive(&pid) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("MCP cancellation reaped Lake");
    send(&mut stdin,json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"build","arguments":{"json":true,"strict":true,"targets":["Target"],"timeout_ms":2000,"max_output_bytes":1024}}})).await;
    let result = response(&mut lines, 3).await;
    assert!(result.get("error").is_none(), "{result}");
    let report: Value =
        serde_json::from_str(result["result"]["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(report["success"], false);
    assert_eq!(report["provenance"]["exit_code"], 0);
    assert_eq!(
        report["provenance"]["execution"]["args"],
        json!(["build", "Target"])
    );
    send(&mut stdin,json!({"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"build","arguments":{"max_output_bytes":1}}})).await;
    assert_eq!(
        response(&mut lines, 4).await["error"]["data"]["kind"],
        "output_limit"
    );
    send(&mut stdin,json!({"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"build","arguments":{"timeout_ms":0}}})).await;
    assert_eq!(
        response(&mut lines, 5).await["error"]["data"]["kind"],
        "timeout"
    );
    project.assert_no_daemon();
    stdin.shutdown().await.unwrap();
    drop(stdin);
    tokio::time::timeout(Duration::from_secs(3), child.wait())
        .await
        .unwrap()
        .unwrap();
}
