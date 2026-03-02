use super::*;
use lsp_server::{Message, Notification, ResponseError};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;

fn pair() -> (
    Arc<LeanSession>,
    crate::server::test_peer::Peer,
    Arc<ElaborationStore>,
) {
    let state = Arc::new(ElaborationStore::new());
    state.begin_document(0, URI, 2).unwrap();
    let (session, peer) = crate::server::test_peer::pair(state.clone());
    (session, peer, state)
}
fn ctx() -> RequestContext {
    RequestContext::new(Some(2000), CancellationToken::new()).unwrap()
}
const URI: &str = "file:///tmp/Lean%20Probe.lean";

#[tokio::test]
async fn typed_lean_surface_and_notifications_work_over_library_framing() {
    let (session, mut peer, state) = pair();
    let client = session.clone();
    let initialization =
        tokio::spawn(async move { client.initialize(Path::new("/tmp"), &ctx()).await });
    let initialize = peer.request("initialize").await;
    assert_eq!(
        initialize.params["initializationOptions"],
        json!({"hasWidgets":true})
    );
    assert_eq!(
        initialize.params["capabilities"]["textDocument"]["documentSymbol"]["hierarchicalDocumentSymbolSupport"],
        true
    );
    peer.send(Response::new_ok(initialize.id, json!({"capabilities":{}})))
        .await;
    initialization.await.unwrap().unwrap();
    peer.notify("initialized").await;
    session
        .open(
            URI,
            "theorem t : True := by sorry",
            1,
            Some(lean::DependencyBuildMode::Once),
        )
        .unwrap();
    let open = peer.notify("textDocument/didOpen").await;
    assert_eq!(open.params["dependencyBuildMode"], "once");
    assert_eq!(open.params["textDocument"]["version"], 1);
    session
        .change(URI, "theorem t : True := by trivial", 2)
        .unwrap();
    assert_eq!(
        peer.notify("textDocument/didChange").await.params["textDocument"]["version"],
        2
    );
    session.close(URI).unwrap();
    assert_eq!(
        peer.notify("textDocument/didClose").await.params["textDocument"]["uri"],
        URI
    );
    // A notification between an outgoing request and its response changes state immediately.
    let client = session.clone();
    let query = tokio::spawn(async move {
        client
            .goal(
                URI,
                Position {
                    line: 0,
                    character: 0,
                },
                &ctx(),
                Duration::from_secs(1),
            )
            .await
    });
    let goal = peer.request("$/lean/plainGoal").await;
    peer.send(Notification::new(
        "$/lean/fileProgress".into(),
        json!({"textDocument":{"uri":URI,"version":2},"processing":[]}),
    ))
    .await;
    peer.send(Notification::new("textDocument/publishDiagnostics".into(), json!({"uri":URI,"version":2,"diagnostics":[{"range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}},"message":"warning","code":{"value":"hasSorry"},"severity":2,"data":{"extension":true}}]}))).await;
    peer.send(Response::new_ok(
        goal.id,
        json!({"goals":[],"rendered":"duplicate","extra":7}),
    ))
    .await;
    let GoalResult::Goals(goals) = query.await.unwrap().unwrap() else {
        panic!("expected goals")
    };
    assert_eq!(goals.extensions.get("extra"), Some(&json!(7)));
    assert!(!goals.extensions.contains_key("rendered"));
    let document = state.get(URI).unwrap();
    assert!(document.seen_completion);
    assert_eq!(document.diagnostics[0].code, Some(json!("hasSorry")));
    assert_eq!(
        document.diagnostics[0].data,
        Some(json!({"extension":true}))
    );
    for (method, answer) in [
        ("textDocument/documentSymbol", json!([])),
        ("textDocument/foldingRange", json!([])),
        ("textDocument/waitForDiagnostics", Value::Null),
        ("textDocument/references", json!([])),
    ] {
        let client = session.clone();
        let method_owned = method.to_owned();
        let query = tokio::spawn(async move {
            match method_owned.as_str() {
                "textDocument/documentSymbol" => client
                    .document_symbols(URI, &ctx(), Duration::from_secs(1))
                    .await
                    .map(|_| ()),
                "textDocument/foldingRange" => client
                    .folding_ranges(URI, &ctx(), Duration::from_secs(1))
                    .await
                    .map(|_| ()),
                "textDocument/waitForDiagnostics" => {
                    client
                        .wait_for_diagnostics(URI, 2, &ctx(), Duration::from_secs(1))
                        .await
                }
                _ => client
                    .references(
                        URI,
                        Position {
                            line: 0,
                            character: 0,
                        },
                        &ctx(),
                        Duration::from_secs(1),
                    )
                    .await
                    .map(|_| ()),
            }
        });
        let request = peer.request(method).await;
        peer.send(Response::new_ok(request.id, answer)).await;
        query.await.unwrap().unwrap();
    }
    // Supported and unsupported peer requests use library response models.
    for (method, params, code) in [
        (
            "window/workDoneProgress/create",
            json!({"token":"probe"}),
            None,
        ),
        ("window/workDoneProgress/create", json!({}), Some(-32602)),
        (
            "workspace/configuration",
            json!({"items":[{"section":"Lean.editor"},{"scopeUri":URI}]}),
            None,
        ),
        ("workspace/workspaceFolders", Value::Null, None),
        (
            "client/registerCapability",
            json!({"registrations":[]}),
            Some(-32601),
        ),
        ("lean/unsupported", json!({}), Some(-32601)),
    ] {
        peer.send(lsp_server::Request::new(
            "server-id".to_owned().into(),
            method.into(),
            params,
        ))
        .await;
        let Message::Response(response) = peer.next().await else {
            panic!("response expected")
        };
        assert_eq!(
            response.response_result.as_ref().err().map(|e| e.code),
            code
        );
        if method == "workspace/configuration" {
            assert_eq!(response.response_result.unwrap(), json!([null, null]));
        } else if method == "workspace/workspaceFolders" {
            assert_eq!(response.response_result.unwrap()[0]["uri"], "file:///tmp");
        }
    }
    let client = session.clone();
    let shutdown = tokio::spawn(async move { client.cooperative_shutdown(&ctx()).await });
    let request = peer.request("shutdown").await;
    peer.send(Response::new_ok(request.id, Value::Null)).await;
    peer.notify("exit").await;
    shutdown.await.unwrap().unwrap();
    session.shutdown().await;
    peer.finish().await;
}

#[tokio::test]
async fn out_of_order_replies_cancellation_and_late_results_keep_their_ids() {
    let (session, mut peer, _) = pair();
    let one = session.clone();
    let first = tokio::spawn(async move {
        one.goal(
            URI,
            Position {
                line: 1,
                character: 0,
            },
            &ctx(),
            Duration::from_secs(1),
        )
        .await
    });
    let first_req = peer.request("$/lean/plainGoal").await;
    let two = session.clone();
    let second = tokio::spawn(async move {
        two.goal(
            URI,
            Position {
                line: 2,
                character: 0,
            },
            &ctx(),
            Duration::from_secs(1),
        )
        .await
    });
    let second_req = peer.request("$/lean/plainGoal").await;
    peer.send(Response::new_ok(second_req.id, json!({"goals":["second"]})))
        .await;
    peer.send(Response::new_ok(first_req.id, json!({"goals":["first"]})))
        .await;
    assert!(matches!(first.await.unwrap().unwrap(),GoalResult::Goals(g) if g.goals==["first"]));
    assert!(matches!(second.await.unwrap().unwrap(),GoalResult::Goals(g) if g.goals==["second"]));
    for cause in ["drop", "cancel", "timeout"] {
        let token = CancellationToken::new();
        let context = RequestContext::new(
            Some(if cause == "timeout" { 20 } else { 2000 }),
            token.clone(),
        )
        .unwrap();
        let client = session.clone();
        let query = tokio::spawn(async move {
            client
                .goal(
                    URI,
                    Position {
                        line: 0,
                        character: 0,
                    },
                    &context,
                    Duration::from_secs(1),
                )
                .await
        });
        let request = peer.request("$/lean/plainGoal").await;
        match cause {
            "drop" => query.abort(),
            "cancel" => token.cancel(),
            _ => {}
        }
        let cancel = peer.notify("$/cancelRequest").await;
        assert_eq!(
            cancel.params["id"],
            serde_json::to_value(&request.id).unwrap()
        );
        let result = query.await;
        match cause {
            "drop" => assert!(result.unwrap_err().is_cancelled()),
            "cancel" => assert!(matches!(result.unwrap(), Err(Error::Cancelled))),
            _ => assert!(matches!(result.unwrap(), Err(Error::Timeout))),
        }
        assert!(!session.io.has_pending());
        peer.send(Response::new_ok(request.id, Value::Null)).await;
    }
    let client = session.clone();
    let next = tokio::spawn(async move {
        client
            .goal(
                URI,
                Position {
                    line: 0,
                    character: 0,
                },
                &ctx(),
                Duration::from_secs(1),
            )
            .await
    });
    let request = peer.request("$/lean/plainGoal").await;
    peer.send(Response {
        id: request.id,
        response_result: Err(ResponseError {
            code: -32801,
            message: "modified".into(),
            data: Some(json!({"version":4})),
        }),
    })
    .await;
    assert!(
        matches!(next.await.unwrap(),Err(Error::Lsp {code:-32801,data:Some(v),..}) if v==json!({"version":4}))
    );
    session.shutdown().await;
    peer.finish().await;
}

#[tokio::test]
async fn malformed_input_and_eof_fail_pending_requests_and_shutdown_joins_idle_io() {
    for bytes in [
        b"Content-Length: 5\r\n\r\nnotjs".as_slice(),
        b"bad header\r\n\r\n".as_slice(),
        b"Content-Length: 10\r\n\r\n{".as_slice(),
        b"".as_slice(),
    ] {
        let (session, mut peer, _) = pair();
        let client = session.clone();
        let query = tokio::spawn(async move {
            client
                .goal(
                    URI,
                    Position {
                        line: 0,
                        character: 0,
                    },
                    &ctx(),
                    Duration::from_secs(1),
                )
                .await
        });
        peer.request("$/lean/plainGoal").await;
        peer.writer.write_all(bytes).await.unwrap();
        peer.writer.shutdown().await.unwrap();
        assert!(matches!(query.await.unwrap(), Err(Error::ServerDown(_))));
        assert!(session.is_dead());
        tokio::time::timeout(Duration::from_secs(1), session.shutdown())
            .await
            .unwrap();
        peer.finish().await;
    }
    let (session, mut peer, _) = pair();
    tokio::time::timeout(Duration::from_secs(1), session.shutdown())
        .await
        .unwrap();
    peer.finish().await;
}

#[tokio::test]
async fn writer_failure_is_session_failure_even_with_an_open_reader() {
    struct Broken;
    impl tokio::io::AsyncWrite for Broken {
        fn poll_write(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
            _: &[u8],
        ) -> std::task::Poll<std::io::Result<usize>> {
            std::task::Poll::Ready(Err(std::io::ErrorKind::BrokenPipe.into()))
        }
        fn poll_flush(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
        fn poll_shutdown(
            self: std::pin::Pin<&mut Self>,
            _: &mut std::task::Context<'_>,
        ) -> std::task::Poll<std::io::Result<()>> {
            std::task::Poll::Ready(Ok(()))
        }
    }
    let (reader, _peer) = tokio::io::duplex(1024);
    let session = LeanSession::spawn(reader, Broken, Arc::new(ElaborationStore::new()));
    let result = tokio::time::timeout(
        Duration::from_secs(1),
        session.goal(
            URI,
            Position {
                line: 0,
                character: 0,
            },
            &ctx(),
            Duration::from_secs(300),
        ),
    )
    .await
    .unwrap();
    assert!(matches!(result, Err(Error::ServerDown(_))));
    assert!(session.is_dead());
    tokio::time::timeout(Duration::from_secs(1), session.shutdown())
        .await
        .unwrap();
}
