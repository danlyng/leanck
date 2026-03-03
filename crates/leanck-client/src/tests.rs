use super::*;
use leanck_core::{
    api::{Response, op},
    error::{ErrorKind, WireError},
    wire,
};
use serde_json::json;
use std::time::Duration;

#[path = "../../../test-support/fake_daemon.rs"]
mod fake_daemon;
pub(crate) use fake_daemon::Scratch;
use fake_daemon::{request, respond};

impl Scratch {
    pub(crate) fn client(&self) -> Client {
        Client::from_path(&self.0).unwrap()
    }
}

#[tokio::test]
async fn existing_probe_never_starts_or_prepares_an_absent_daemon() {
    let scratch = Scratch::new();
    let client = scratch.client();
    assert!(
        client
            .probe_existing(RequestOptions::default())
            .await
            .unwrap()
            .is_none()
    );
    let runtime = RuntimePaths::for_workspace(&scratch.0);
    for artifact in [
        runtime.socket(),
        runtime.pid(),
        runtime.log(),
        runtime.lock(),
    ] {
        assert!(!artifact.exists());
    }
    let listener = scratch.listener();
    let daemon = tokio::spawn(async move {
        let (mut stream, request) = request(&listener).await;
        assert_eq!(request.op, op::HEALTH);
        respond(&mut stream, Response::ok(request.id, fixture("health"))).await;
    });
    assert_eq!(
        client
            .probe_existing(RequestOptions::default())
            .await
            .unwrap(),
        Some(serde_json::from_value(fixture("health")).unwrap())
    );
    daemon.await.unwrap();
}

#[tokio::test]
async fn request_options_are_independent_for_concurrent_callers() {
    let scratch = Scratch::new();
    let listener = scratch.listener();
    let daemon = tokio::spawn(async move {
        for _ in 0..2 {
            let (mut stream, request) = request(&listener).await;
            let result = match request.op.as_str() {
                op::DIAGNOSTICS => {
                    assert_eq!(request.timeout_ms, Some(42));
                    assert_eq!(
                        request.params,
                        json!({"file":"Basic.lean","severity":{"mode":"only","severities":["error"]}})
                    );
                    fixture("clean_diagnostics")
                }
                op::GOAL => {
                    assert_eq!(request.timeout_ms, None);
                    assert_eq!(
                        request.params,
                        json!({"file":"Basic.lean","line":1,"character":2})
                    );
                    fixture("goal_null")
                }
                other => panic!("unexpected operation {other}"),
            };
            respond(&mut stream, Response::ok(request.id, result)).await;
        }
    });
    let client = scratch.client();
    let diagnostic_params = DiagnosticsParams {
        file: "Basic.lean".into(),
        ..Default::default()
    };
    let goal_params = GoalParams {
        file: "Basic.lean".into(),
        line: 1,
        character: 2,
    };
    let (first, second) = tokio::join!(
        client.diagnostics(
            &diagnostic_params,
            RequestOptions {
                timeout_ms: Some(42),
                ..Default::default()
            }
        ),
        client.goal(&goal_params, RequestOptions::default()),
    );
    assert_eq!(
        serde_json::to_value(first.unwrap()).unwrap(),
        fixture("clean_diagnostics")
    );
    assert_eq!(second.unwrap(), GoalResult::NoGoal);
    daemon.await.unwrap();
}

#[tokio::test]
async fn reconnect_after_proven_connection_failure_uses_captured_workspace() {
    let first_workspace = Scratch::new();
    let other_workspace = Scratch::new();
    let client = first_workspace.client();
    let runtime = RuntimePaths::for_workspace(client.workspace());
    let bytes = client
        .request_bytes(
            Command::RestartFile(RestartFileParams {
                file: "Basic.lean".into(),
            }),
            RequestOptions::default(),
        )
        .unwrap();
    let failure = transport::call(&runtime.socket(), &bytes).await;
    assert!(failure.as_ref().unwrap_err().is_pre_dispatch());
    // Rebind after the actual failed connection, as an idle-reaped daemon would.
    let listener = first_workspace.listener();
    let other_listener = other_workspace.listener();
    let daemon = tokio::spawn(async move {
        let (mut stream, request) = request(&listener).await;
        assert_eq!(request.op, op::RESTART_FILE);
        respond(
            &mut stream,
            Response::ok(request.id, json!({"workspace":"first"})),
        )
        .await;
    });
    let response = tokio::time::timeout(
        Duration::from_secs(2),
        client.finish_call(
            failure,
            runtime,
            &bytes,
            &RequestOptions::default(),
            Duration::from_secs(301),
            true,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(response["workspace"], "first");
    assert!(
        tokio::time::timeout(Duration::from_millis(20), other_listener.accept())
            .await
            .is_err()
    );
    daemon.await.unwrap();
}

#[tokio::test]
async fn dropped_and_malformed_responses_are_not_replayed() {
    for operation in [op::RESTART_FILE, op::DIAGNOSTICS] {
        for malformed in [false, true] {
            let scratch = Scratch::new();
            let listener = scratch.listener();
            let daemon = tokio::spawn(async move {
                let (mut stream, request) = request(&listener).await;
                assert_eq!(request.op, operation);
                if malformed {
                    wire::write_frame(&mut wire::writer(&mut stream), b"not json")
                        .await
                        .unwrap();
                }
                drop(stream);
                listener // Keep accepting possible replays until the assertion below.
            });
            let error = tokio::time::timeout(
                Duration::from_secs(2),
                scratch.client().call(
                    if operation == op::RESTART_FILE {
                        Command::RestartFile(RestartFileParams {
                            file: "Basic.lean".into(),
                        })
                    } else {
                        Command::Diagnostics(DiagnosticsParams {
                            file: "Basic.lean".into(),
                            ..Default::default()
                        })
                    },
                    RequestOptions::default(),
                    true,
                ),
            )
            .await
            .unwrap()
            .unwrap_err();
            if malformed {
                assert!(matches!(
                    error,
                    ClientError::Protocol(ProtocolError::Envelope(_))
                ));
            } else {
                assert!(matches!(
                    error,
                    ClientError::Transport {
                        phase: TransportPhase::Read,
                        ..
                    }
                ));
            }
            let listener = daemon.await.unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(20), listener.accept())
                    .await
                    .is_err()
            );
        }
    }
}

#[tokio::test]
async fn partial_write_failure_is_uncertain_and_not_replayed() {
    use tokio::io::AsyncReadExt;
    let scratch = Scratch::new();
    let listener = scratch.listener();
    let daemon = tokio::spawn(async move {
        let (mut handshake, _) = listener.accept().await.unwrap();
        let frame = wire::read_frame(&mut wire::reader(&mut handshake))
            .await
            .unwrap()
            .unwrap();
        let probe: Request = serde_json::from_slice(&frame).unwrap();
        respond(&mut handshake, Response::ok(probe.id, fixture("health"))).await;
        drop(handshake);
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut length = [0; 4];
        stream.read_exact(&mut length).await.unwrap();
        drop(stream);
        listener
    });
    let error = tokio::time::timeout(
        Duration::from_secs(2),
        scratch.client().restart_file(
            &RestartFileParams {
                file: "x".repeat(4 * 1024 * 1024),
            },
            RequestOptions::default(),
        ),
    )
    .await
    .unwrap()
    .unwrap_err();
    assert!(matches!(
        error,
        ClientError::Transport {
            phase: TransportPhase::Write,
            ..
        }
    ));
    assert!(!error.is_pre_dispatch());
    let listener = daemon.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn daemon_errors_keep_wire_categories_without_retry() {
    let scratch = Scratch::new();
    let listener = scratch.listener();
    let daemon = tokio::spawn(async move {
        let (mut stream, request) = request(&listener).await;
        respond(
            &mut stream,
            Response::err(
                request.id,
                WireError {
                    kind: ErrorKind::NotFound,
                    message: "missing".into(),
                },
            ),
        )
        .await;
        listener
    });
    let error = scratch
        .client()
        .list(
            &ListParams {
                file: "missing.lean".into(),
                kind: None,
            },
            RequestOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(
        matches!(error, ClientError::Daemon { kind: ErrorKind::NotFound, ref message } if message == "missing")
    );
    let listener = daemon.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
}

fn fixture(name: &str) -> Value {
    serde_json::from_str::<Value>(include_str!("../../../test-support/contracts/v4.json")).unwrap()
        [name]
        .clone()
}

#[tokio::test]
async fn invalid_envelopes_ids_and_results_are_protocol_errors_without_replay() {
    for (reply, invalid_result) in [
        (json!({"ok":true}), false),
        (
            json!({"ok":true,"result":{},"error":{"kind":"internal","message":"both"}}),
            false,
        ),
        (json!({"ok":false}), false),
        (
            json!({"ok":false,"result":null,"error":{"kind":"internal","message":"both"}}),
            false,
        ),
        (json!({"ok":true,"result":{},"id":"another-request"}), false),
        (
            json!({"ok":false,"error":{"kind":"internal","message":"wrong id"},"id":"another-request"}),
            false,
        ),
        (json!({"ok":true,"result":{}}), true),
        (
            json!({"ok":true,"result":{"uri":"file:///x","symbols":"not an array"}}),
            true,
        ),
    ] {
        let scratch = Scratch::new();
        let listener = scratch.listener();
        let daemon = tokio::spawn(async move {
            let (mut stream, request) = request(&listener).await;
            let mut reply = reply;
            if reply.get("id").is_none() {
                reply["id"] = json!(request.id);
            }
            wire::write_frame(
                &mut wire::writer(&mut stream),
                &serde_json::to_vec(&reply).unwrap(),
            )
            .await
            .unwrap();
            listener
        });
        let error = scratch
            .client()
            .list(
                &ListParams {
                    file: "Basic.lean".into(),
                    kind: None,
                },
                RequestOptions::default(),
            )
            .await
            .unwrap_err();
        assert!(matches!(error, ClientError::Protocol(_)), "{error:?}");
        assert_eq!(
            matches!(
                error,
                ClientError::Protocol(ProtocolError::InvalidResult { .. })
            ),
            invalid_result
        );
        let listener = daemon.await.unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), listener.accept())
                .await
                .is_err()
        );
    }
}

#[tokio::test]
async fn null_goals_are_success_but_missing_goals_are_protocol_errors() {
    for (value, expected) in [
        (Value::Null, Some(GoalResult::NoGoal)),
        (
            json!({"goals":[]}),
            Some(GoalResult::Goals(Goals {
                goals: vec![],
                extensions: Default::default(),
            })),
        ),
        (json!({}), None),
    ] {
        let scratch = Scratch::new();
        let listener = scratch.listener();
        let daemon = tokio::spawn(async move {
            let (mut stream, request) = request(&listener).await;
            respond(&mut stream, Response::ok(request.id, value)).await;
        });
        let result = scratch
            .client()
            .goal(
                &GoalParams {
                    file: "Basic.lean".into(),
                    line: 0,
                    character: 0,
                },
                RequestOptions::default(),
            )
            .await;
        match expected {
            Some(expected) => assert_eq!(result.unwrap(), expected),
            None => assert!(matches!(
                result,
                Err(ClientError::Protocol(ProtocolError::InvalidResult {
                    operation: Operation::Goal,
                    ..
                }))
            )),
        }
        daemon.await.unwrap();
    }
}

#[tokio::test]
async fn incompatible_protocol_never_dispatches_or_shuts_down_the_peer() {
    assert_incompatible_protocol_is_not_dispatched(999).await;
}

#[tokio::test]
async fn previous_protocol_three_is_rejected_before_dispatch() {
    assert_incompatible_protocol_is_not_dispatched(3).await;
}

async fn assert_incompatible_protocol_is_not_dispatched(version: u64) {
    let scratch = Scratch::new();
    let listener = scratch.listener();
    let daemon = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let frame = wire::read_frame(&mut wire::reader(&mut stream))
            .await
            .unwrap()
            .unwrap();
        let req: Request = serde_json::from_slice(&frame).unwrap();
        assert_eq!(req.op, op::HEALTH);
        // Negotiation only depends on the protocol header, not a future health schema.
        respond(
            &mut stream,
            Response::ok(req.id, json!({"protocol_version":version})),
        )
        .await;
        listener
    });
    let error = scratch
        .client()
        .restart_file(
            &RestartFileParams {
                file: "Basic.lean".into(),
            },
            RequestOptions::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        ClientError::IncompatibleProtocol {
            expected: leanck_core::api::PROTOCOL_VERSION,
            actual
        } if actual == version
    ));
    let listener = daemon.await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(30), listener.accept())
            .await
            .is_err()
    );
    // Explicit shutdown remains available for deliberate recovery.
    let peer = tokio::spawn(async move {
        let (mut stream, req) = request(&listener).await;
        assert_eq!(req.op, op::SHUTDOWN);
        respond(
            &mut stream,
            Response::ok(req.id, json!({"shutting_down":true})),
        )
        .await;
    });
    scratch
        .client()
        .shutdown_existing(RequestOptions::default())
        .await
        .unwrap();
    peer.await.unwrap();
}

#[tokio::test]
async fn cancellation_timeout_and_dropped_call_send_the_original_request_id() {
    for cause in ["token", "timeout", "drop"] {
        let scratch = Scratch::new();
        let listener = scratch.listener();
        let token = CancellationToken::new();
        let client = scratch.client();
        let options = RequestOptions {
            cancellation: Some(token.clone()),
            transport_timeout_ms: (cause == "timeout").then_some(30),
            ..Default::default()
        };
        let query = tokio::spawn(async move {
            client
                .list(
                    &ListParams {
                        file: "Basic.lean".into(),
                        kind: None,
                    },
                    options,
                )
                .await
        });
        let (stream, original) = request(&listener).await;
        assert_eq!(original.op, op::LIST);
        if cause == "token" {
            token.cancel();
        }
        if cause == "drop" {
            query.abort();
        }
        let (mut control, cancelled) =
            tokio::time::timeout(Duration::from_secs(2), request(&listener))
                .await
                .unwrap();
        assert_eq!(cancelled.op, op::CANCEL);
        assert_eq!(cancelled.params["target_id"], original.id);
        respond(
            &mut control,
            Response::ok(cancelled.id, json!({"cancelled":original.id})),
        )
        .await;
        let result = query.await;
        match cause {
            "token" => assert!(matches!(
                result.unwrap(),
                Err(ClientError::Cancelled {
                    delivery_uncertain: true
                })
            )),
            "timeout" => assert!(matches!(
                result.unwrap(),
                Err(ClientError::TransportTimeout {
                    phase: TransportPhase::Read
                })
            )),
            "drop" => assert!(result.unwrap_err().is_cancelled()),
            _ => unreachable!(),
        }
        drop(stream);
    }
}

#[tokio::test(start_paused = true)]
async fn a_silent_existing_health_probe_is_bounded_and_does_not_spawn() {
    let scratch = Scratch::new();
    let listener = scratch.listener();
    let client = scratch.client();
    let probe = tokio::spawn(async move { client.probe_existing(RequestOptions::default()).await });
    let (_stream, req) = request(&listener).await;
    assert_eq!(req.op, op::HEALTH);
    assert!(matches!(
        probe.await.unwrap(),
        Err(ClientError::TransportTimeout {
            phase: TransportPhase::Read
        })
    ));
    assert!(!RuntimePaths::for_workspace(&scratch.0).pid().exists());
}

#[test]
fn timeout_warnings_follow_request_overrides_instead_of_only_config_defaults() {
    let scratch = Scratch::new();
    let mut settings = leanck_config::Settings::default();
    settings.client.transport_timeout_ms = Some(100);
    settings.client.timeout_ms = Some(200);
    let client = scratch.client().with_settings(settings);
    assert_eq!(client.request_warnings(&RequestOptions::default()).len(), 1);
    assert!(
        client
            .request_warnings(&RequestOptions {
                timeout_ms: Some(50),
                ..Default::default()
            })
            .is_empty()
    );
    assert!(
        client
            .request_warnings(&RequestOptions {
                transport_timeout_ms: Some(200),
                ..Default::default()
            })
            .is_empty()
    );
    let warnings = client.request_warnings(&RequestOptions {
        timeout_ms: Some(500),
        transport_timeout_ms: Some(300),
        ..Default::default()
    });
    assert_eq!(warnings.len(), 1);
    assert!(warnings[0].message.contains("500 ms"));
}
