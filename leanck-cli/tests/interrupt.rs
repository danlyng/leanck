use leanck_core::{api::Response, error::ErrorKind};
use std::{process::Stdio, time::Duration};
#[path = "../../test-support/fake_daemon.rs"]
mod fake_daemon;
use fake_daemon::{Scratch, request, respond};

#[tokio::test]
async fn sigint_cancels_only_the_cli_operation_and_exits_promptly() {
    let scratch = Scratch::new();
    let listener = scratch.listener();
    let mut cli = tokio::process::Command::new(env!("CARGO_BIN_EXE_leanck"))
        .args(["list", "Basic.lean", "--workspace"])
        .arg(&scratch.0)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let (_stream, original) = tokio::time::timeout(Duration::from_secs(3), request(&listener))
        .await
        .unwrap();
    assert_eq!(original.op, "list");
    assert!(
        std::process::Command::new("kill")
            .args(["-INT", &cli.id().unwrap().to_string()])
            .status()
            .unwrap()
            .success()
    );
    let (mut stream, cancel) = tokio::time::timeout(Duration::from_secs(2), request(&listener))
        .await
        .unwrap();
    assert_eq!(cancel.op, "cancel");
    assert_eq!(cancel.params["target_id"], original.id);
    respond(
        &mut stream,
        Response::ok(cancel.id, serde_json::json!({"cancelled":original.id})),
    )
    .await;
    let status = tokio::time::timeout(Duration::from_secs(2), cli.wait())
        .await
        .unwrap()
        .unwrap();
    assert!(!status.success());
    // The shared peer remains usable by the next client.
    let client = leanck_client::Client::from_path(&scratch.0).unwrap();
    let query = tokio::spawn(async move {
        client
            .list(
                &leanck_core::api::ListParams {
                    file: "Basic.lean".into(),
                    kind: None,
                },
                Default::default(),
            )
            .await
    });
    let (mut stream, next) = request(&listener).await;
    assert_eq!(next.op, "list");
    respond(
        &mut stream,
        Response::err(
            next.id,
            leanck_core::error::WireError {
                kind: ErrorKind::NotFound,
                message: "still serving".into(),
            },
        ),
    )
    .await;
    assert!(matches!(
        query.await.unwrap(),
        Err(leanck_client::ClientError::Daemon {
            kind: ErrorKind::NotFound,
            ..
        })
    ));
}
