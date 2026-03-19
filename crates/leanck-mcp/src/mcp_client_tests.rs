use super::fake_daemon::{Scratch, request, respond};
use super::*;
use leanck_core::{api::Response, workspace::WorkspaceRoot};
use rmcp::model::ErrorCode;

#[test]
fn daemon_categories_map_deliberately_to_mcp_errors() {
    for (kind, wire_name, invalid_params) in [
        (ErrorKind::BadRequest, "bad_request", true),
        (ErrorKind::NotFound, "not_found", true),
        (ErrorKind::Timeout, "timeout", false),
        (ErrorKind::Cancelled, "cancelled", false),
        (ErrorKind::ServerDown, "server_down", false),
        (ErrorKind::ServerUnhealthy, "server_unhealthy", false),
        (ErrorKind::Overloaded, "overloaded", false),
        (ErrorKind::Internal, "internal", false),
    ] {
        let error = to_mcp_error(ClientError::Daemon {
            kind,
            message: "detail".into(),
        });
        assert_eq!(
            error.code,
            if invalid_params {
                ErrorCode::INVALID_PARAMS
            } else {
                ErrorCode::INTERNAL_ERROR
            }
        );
        assert_eq!(error.data, Some(serde_json::json!({"kind":wire_name})));
    }
    let error = to_mcp_error(ClientError::Transport {
        phase: TransportPhase::Read,
        source: std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "lost response"),
    });
    assert_eq!(
        error.data,
        Some(serde_json::json!({"phase":"read","delivery_uncertain":true}))
    );
    let error = to_mcp_error(ClientError::Protocol(
        leanck_core::error::ProtocolError::MismatchedId {
            expected: "sent".into(),
            actual: "other".into(),
        },
    ));
    assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
    assert_eq!(
        error.data,
        Some(serde_json::json!({"kind":"protocol_error","delivery_uncertain":true}))
    );
    let error = to_mcp_error(ClientError::InstanceLimit {
        running: 8,
        limit: 8,
        busy: true,
    });
    assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
    assert!(error.message.contains("leanck daemons stop --idle"));
    assert_eq!(
        error.data,
        Some(serde_json::json!({"kind":"instance_limit","running":8,"limit":8}))
    );
    let error = to_mcp_error(ClientError::DaemonExited {
        code: 1,
        log: "/cache/leanck/leanck-abc.log".into(),
    });
    assert_eq!(error.code, ErrorCode::INTERNAL_ERROR);
    assert_eq!(
        error.data,
        Some(
            serde_json::json!({"kind":"daemon_exited","code":1,"log":"/cache/leanck/leanck-abc.log"})
        )
    );
}

#[tokio::test]
async fn switching_workspace_during_diagnostics_keeps_original_source_excerpt() {
    let first = Scratch::new();
    let second = Scratch::new();
    std::fs::write(first.0.join("Basic.lean"), "original source\n").unwrap();
    std::fs::write(second.0.join("Basic.lean"), "switched source\n").unwrap();
    std::fs::write(second.0.join("lean-toolchain"), "").unwrap();
    let listener = first.listener();
    let server = LeanckServer::new(&Config::defaults(
        WorkspaceRoot::from_path(&first.0).unwrap(),
    ));
    let in_flight = server.clone();
    let query = tokio::spawn(async move {
        let input =
            serde_json::from_value(serde_json::json!({"file":"Basic.lean","timeout_ms":37}))
                .unwrap();
        in_flight
            .diagnostics(Parameters(input), CancellationToken::new())
            .await
            .unwrap()
    });
    let (mut stream, request) = request(&listener).await;
    assert_eq!(request.timeout_ms, Some(37));
    assert!(request.params.get("timeout_ms").is_none());
    server
        .set_workspace(Parameters(SetWorkspaceInput {
            path: second.0.to_string_lossy().into_owned(),
        }))
        .await
        .unwrap();
    respond(
        &mut stream,
        Response::ok(
            request.id,
            serde_json::json!({
                "uri":"file:///original/Basic.lean", "range":null, "symbol":null, "version":1,
                "total":1, "source":"original source\n", "notes":[], "diagnostics":[{
                    "severity":1, "message":"contract error",
                    "range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}
                }]
            }),
        ),
    )
    .await;
    let content = query.await.unwrap();
    let text = &content.as_text().unwrap().text;
    assert!(text.contains("original source"), "{text}");
    assert!(!text.contains("switched source"), "{text}");
    assert_eq!(server.workspace.capture().workspace(), second.0);
}

#[tokio::test]
async fn switching_workspace_during_health_keeps_original_socket_display() {
    let first = Scratch::new();
    let second = Scratch::new();
    let listener = first.listener();
    let mut config = Config::defaults(WorkspaceRoot::from_path(&first.0).unwrap());
    config.warnings.push(leanck_config::Warning::new(
        "ignored_project_machine_wide_section",
        "first workspace warning",
    ));
    let server = LeanckServer::new(&config);
    let in_flight = server.clone();
    let query = tokio::spawn(async move {
        in_flight
            .health(
                Parameters(HealthzInput { json: Some(true) }),
                CancellationToken::new(),
            )
            .await
            .unwrap()
    });
    let (mut stream, request) = request(&listener).await;
    server
        .workspace
        .set(WorkspaceRoot::from_path(&second.0).unwrap());
    respond(
        &mut stream,
        Response::ok(
            request.id,
            serde_json::json!({"ok":true,"workspace":first.0,"protocol_version":leanck_core::api::PROTOCOL_VERSION,"server":{"pid":123,"running":true},"idle_secs":0,"lifecycle":{"state":"ready","active_requests":0}}),
        ),
    )
    .await;
    let content = query.await.unwrap();
    let value: serde_json::Value = serde_json::from_str(&content.as_text().unwrap().text).unwrap();
    assert_eq!(
        value["lsp"]["socket_path"],
        leanck_core::paths::socket_path(&first.0)
            .to_string_lossy()
            .as_ref()
    );
    assert_eq!(
        value["lsp"]["workspace"],
        first.0.to_string_lossy().as_ref()
    );
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["issues"],
        serde_json::json!([{"level":"warning", "message":"first workspace warning"}])
    );
    assert!(server.workspace.capture_health().1.is_empty());
}

#[test]
fn frontend_options_stay_outside_shared_request_params() {
    let input: DiagnosticsInput = serde_json::from_value(serde_json::json!({"file":"Basic.lean","severity":{"mode":"only","severities":["warning","information"]},"offset":4294967295u32,"limit":1,"verbose":true,"timeout_ms":37})).unwrap();
    assert_eq!(
        serde_json::to_value(DiagnosticsParams::from(&input)).unwrap(),
        serde_json::json!({"file":"Basic.lean","severity":{"mode":"only","severities":["warning","information"]},"offset":4294967295u64,"limit":1})
    );
    let input: GoalInput = serde_json::from_value(
        serde_json::json!({"file":"Basic.lean","line":1,"character":2,"timeout_ms":37}),
    )
    .unwrap();
    assert_eq!(
        serde_json::to_value(GoalParams::from(&input)).unwrap(),
        serde_json::json!({"file":"Basic.lean","line":1,"character":2})
    );
    let input: SorryInput =
        serde_json::from_value(serde_json::json!({"path":"Basic.lean","timeout_ms":37})).unwrap();
    assert_eq!(
        serde_json::to_value(SorryParams::from(&input)).unwrap(),
        serde_json::json!({"path":"Basic.lean"})
    );
    let input: ListInput =
        serde_json::from_value(serde_json::json!({"file":"Basic.lean","timeout_ms":37})).unwrap();
    assert_eq!(
        serde_json::to_value(ListParams::from(&input)).unwrap(),
        serde_json::json!({"file":"Basic.lean","kind":[]})
    );
    let input: RestartFileInput =
        serde_json::from_value(serde_json::json!({"file":"Basic.lean","timeout_ms":37})).unwrap();
    assert_eq!(
        serde_json::to_value(RestartFileParams::from(&input)).unwrap(),
        serde_json::json!({"file":"Basic.lean"})
    );
}

#[test]
fn sorry_file_alias_is_rejected() {
    let error =
        serde_json::from_value::<SorryInput>(serde_json::json!({"file":"Basic.lean"})).unwrap_err();
    assert!(error.to_string().contains("missing field `path`"));
}

#[test]
fn list_kinds_are_typed_and_schema_is_closed() {
    use leanck_core::api::KindSelector;
    let input: ListInput = serde_json::from_value(serde_json::json!({
        "file":"Basic.lean", "kind":["theorem","enum_member","type_parameter"], "timeout_ms":37
    }))
    .unwrap();
    assert_eq!(
        ListParams::from(&input).kind.unwrap(),
        [
            KindSelector::Theorem,
            KindSelector::EnumMember,
            KindSelector::TypeParameter
        ]
    );
    for typo in ["enumMember", "typeParameter", "thorem", "THEOREM"] {
        assert!(
            serde_json::from_value::<ListInput>(
                serde_json::json!({"file":"Basic.lean","kind":[typo]})
            )
            .is_err()
        );
    }
    let schema = serde_json::to_value(schemars::schema_for!(ListInput)).unwrap();
    let values = schema["$defs"]["KindSelector"]["enum"].as_array().unwrap();
    assert_eq!(values.len(), KindSelector::ALL.len());
    for kind in KindSelector::ALL {
        assert!(values.contains(&serde_json::json!(kind.as_str())));
    }
}

#[tokio::test]
async fn malformed_health_is_reported_as_unhealthy() {
    let workspace = Scratch::new();
    let listener = workspace.listener();
    let server = LeanckServer::new(&Config::defaults(
        WorkspaceRoot::from_path(&workspace.0).unwrap(),
    ));
    let daemon = tokio::spawn(async move {
        let (mut stream, request) = request(&listener).await;
        respond(&mut stream, Response::ok(request.id, serde_json::json!({}))).await;
    });
    let content = server
        .health(
            Parameters(HealthzInput { json: Some(true) }),
            CancellationToken::new(),
        )
        .await
        .unwrap();
    let report: HealthReport = serde_json::from_str(&content.as_text().unwrap().text).unwrap();
    assert!(!report.ok);
    assert_eq!(report.issues.len(), 1);
    assert!(report.issues[0].message.contains("invalid health result"));
    let instances = report
        .instances
        .expect("health attaches the instance summary");
    assert_eq!(
        (instances.limit, instances.on_limit),
        (8, leanck_config::OnLimit::EvictIdle)
    );
    daemon.await.unwrap();
}

#[tokio::test]
async fn mcp_cancellation_reaches_the_shared_client() {
    let scratch = Scratch::new();
    let listener = scratch.listener();
    let server = LeanckServer::new(&Config::defaults(
        WorkspaceRoot::from_path(&scratch.0).unwrap(),
    ));
    let token = CancellationToken::new();
    let request_token = token.clone();
    let query = tokio::spawn(async move {
        server
            .list(
                Parameters(
                    serde_json::from_value(serde_json::json!({"file":"Basic.lean"})).unwrap(),
                ),
                request_token,
            )
            .await
    });
    let (_stream, original) = request(&listener).await;
    token.cancel();
    let (mut stream, cancel) =
        tokio::time::timeout(std::time::Duration::from_secs(2), request(&listener))
            .await
            .unwrap();
    assert_eq!(cancel.op, "cancel");
    assert_eq!(cancel.params["target_id"], original.id);
    respond(
        &mut stream,
        Response::ok(cancel.id, serde_json::json!({"cancelled":original.id})),
    )
    .await;
    let error = query.await.unwrap().unwrap_err();
    assert_eq!(
        error.data,
        Some(serde_json::json!({"kind":"cancelled", "delivery_uncertain":true}))
    );
}

#[test]
fn diagnostics_severity_uses_the_shared_typed_filter() {
    use serde_json::json;
    let omitted: DiagnosticsInput = serde_json::from_value(json!({"file":"x"})).unwrap();
    assert_eq!(
        DiagnosticsParams::from(&omitted).severity,
        SeverityFilter::default()
    );
    for severity in [
        json!({"mode":"all"}),
        json!({"mode":"only","severities":[]}),
    ] {
        let input: DiagnosticsInput =
            serde_json::from_value(json!({"file":"x","severity":severity})).unwrap();
        assert_eq!(
            serde_json::to_value(DiagnosticsParams::from(&input)).unwrap()["severity"],
            severity
        );
    }
    for severity in [
        json!(["all"]),
        json!("all"),
        json!(null),
        json!({"mode":"only","severities":["info"]}),
    ] {
        assert!(
            serde_json::from_value::<DiagnosticsInput>(json!({"file":"x","severity":severity}))
                .is_err()
        );
    }
}

#[test]
fn diagnostics_schema_exposes_named_severities_and_filter_modes() {
    use serde_json::json;
    let schema = serde_json::to_value(schemars::schema_for!(DiagnosticsInput)).unwrap();
    assert_eq!(
        schema["$defs"]["DiagnosticSeverity"]["enum"],
        json!(["error", "warning", "information", "hint"])
    );
    let filter = &schema["$defs"]["SeverityFilter"]["oneOf"];
    assert_eq!(filter[0]["properties"]["mode"]["const"], "all");
    assert_eq!(filter[1]["properties"]["mode"]["const"], "only");
    assert_eq!(filter[1]["required"], json!(["mode", "severities"]));
    assert_eq!(schema["properties"]["severity"]["default"], json!(null));
}

#[tokio::test]
async fn workspace_switch_captures_settings_and_rejects_invalid_configuration_atomically() {
    let first = Scratch::new();
    let second = Scratch::new();
    std::fs::write(second.0.join("lean-toolchain"), "").unwrap();
    std::fs::write(
        second.0.join("leanck.toml"),
        "[normalize]\nwidth=83\n[loogle]\nendpoints=[]\n[client]\ntimeout_ms=170\n",
    )
    .unwrap();
    let mut config = Config::defaults(WorkspaceRoot::from_path(&first.0).unwrap());
    config.args.no_config = false;
    let server = LeanckServer::new(&config);
    let before = server.workspace.capture();
    let switch = || {
        Parameters(SetWorkspaceInput {
            path: second.0.to_string_lossy().into_owned(),
        })
    };
    server.set_workspace(switch()).await.unwrap();
    let after = server.workspace.capture();
    assert_eq!(
        after.settings().normalize.width,
        leanck_config::NormalizeWidth::Finite(83)
    );
    assert_eq!(after.settings().client.timeout_ms, Some(170));
    assert!(after.settings().loogle.endpoints.is_empty());
    assert_eq!(
        before.settings().normalize.width,
        leanck_config::NormalizeWidth::Finite(100)
    );
    assert_eq!(before.workspace(), first.0.canonicalize().unwrap());
    std::fs::write(second.0.join("leanck.toml"), "[normalize]\nwidht=3").unwrap();
    assert!(server.set_workspace(switch()).await.is_err());
    assert_eq!(
        server.workspace.capture().settings().normalize.width,
        leanck_config::NormalizeWidth::Finite(83)
    );
}

#[test]
fn omitted_and_explicit_false_build_policy_are_distinct() {
    let omitted: BuildInput = serde_json::from_value(serde_json::json!({})).unwrap();
    let explicit: BuildInput = serde_json::from_value(
        serde_json::json!({"strict":false,"warnings":false,"json":false,"targets":[]}),
    )
    .unwrap();
    assert!(omitted.strict.is_none() && omitted.targets.is_none());
    assert_eq!(explicit.strict, Some(false));
    assert_eq!(explicit.warnings, Some(false));
    assert_eq!(explicit.json, Some(false));
    assert_eq!(explicit.targets, Some(vec![]));
}

#[tokio::test]
async fn configuration_includes_nonfatal_warnings_without_starting_a_daemon() {
    let scratch = Scratch::new();
    let mut config = Config::defaults(WorkspaceRoot::from_path(&scratch.0).unwrap());
    config.settings.normalize.rules.headings.level = 3;
    config.settings.client.transport_timeout_ms = Some(100);
    let server = LeanckServer::new(&config);
    let content = server.configuration().await.unwrap();
    let value: serde_json::Value = serde_json::from_str(&content.as_text().unwrap().text).unwrap();
    let warnings = value["warnings"].as_array().unwrap();
    assert!(
        warnings
            .iter()
            .any(|w| w["code"] == "inactive_heading_rules")
    );
    assert!(
        warnings
            .iter()
            .any(|w| w["code"] == "short_transport_timeout")
    );
    assert!(!server.workspace.capture().socket().exists());
}

#[tokio::test]
async fn ignored_project_machine_wide_settings_appear_in_diagnostics_and_refresh_with_workspace() {
    let scratch = Scratch::new();
    std::fs::write(scratch.0.join("lean-toolchain"), "").unwrap();
    let path = scratch.0.join("leanck.toml");
    std::fs::write(
        &path,
        "[normalize]\nwidth=83\n[instances]\nlimit='ignored'\n[updates]\ncheck='ignored'\nunknown=true\n",
    )
    .unwrap();
    let config = Config::from_args(
        crate::config::Args {
            workspace: Some(scratch.0.clone()),
        },
        leanck_config::ConfigArgs {
            overrides: vec!["updates.check=false".into()],
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        config.settings.normalize.width,
        leanck_config::NormalizeWidth::Finite(83)
    );
    assert_eq!(config.warnings.len(), 2);
    let server = LeanckServer::new(&config);
    for json in [true, false] {
        let content = server
            .doctor(Parameters(HealthzInput { json: Some(json) }))
            .await
            .unwrap();
        let text = &content.as_text().unwrap().text;
        if json {
            let report: serde_json::Value = serde_json::from_str(text).unwrap();
            assert_eq!(report["ok"], true);
            assert_eq!(report["issues"].as_array().unwrap().len(), 2);
            assert!(
                report["issues"][1]["message"]
                    .as_str()
                    .unwrap()
                    .contains("[updates]")
            );
        } else {
            assert!(text.contains("warning: [instances]"), "{text}");
            assert!(text.contains("warning: [updates]"), "{text}");
        }
    }
    let listener = scratch.listener();
    for json in [true, false] {
        let checking = server.clone();
        let query = tokio::spawn(async move {
            checking
                .health(
                    Parameters(HealthzInput { json: Some(json) }),
                    CancellationToken::new(),
                )
                .await
                .unwrap()
        });
        let (mut stream, request) = request(&listener).await;
        respond(
            &mut stream,
            Response::ok(
                request.id,
                serde_json::json!({
                    "ok": true, "workspace": scratch.0,
                    "protocol_version": leanck_core::api::PROTOCOL_VERSION,
                    "server": {"pid":123,"running":true}, "idle_secs":0,
                    "lifecycle":{"state":"ready","active_requests":0}
                }),
            ),
        )
        .await;
        let content = query.await.unwrap();
        let text = &content.as_text().unwrap().text;
        if json {
            let report: HealthReport = serde_json::from_str(text).unwrap();
            assert!(report.ok);
            assert_eq!(report.issues.len(), 2);
            assert_eq!(
                report.issues[0].level,
                leanck_client::render::HealthIssueLevel::Warning
            );
            assert!(
                report.issues[0]
                    .message
                    .contains(&path.display().to_string())
            );
        } else {
            assert!(text.starts_with("warning: [instances]"), "{text}");
            assert!(text.contains("warning: [updates]"), "{text}");
        }
    }
    // Invalid replacement configs must preserve the previous warning/settings snapshot.
    let switch = || {
        Parameters(SetWorkspaceInput {
            path: scratch.0.to_string_lossy().into_owned(),
        })
    };
    std::fs::write(&path, "[normalize]\nwidht=83\n").unwrap();
    assert!(server.set_workspace(switch()).await.is_err());
    assert_eq!(server.workspace.capture_health().1.len(), 2);
    std::fs::write(&path, "[normalize]\nwidth=84\n").unwrap();
    server.set_workspace(switch()).await.unwrap();
    let (client, warnings) = server.workspace.capture_health();
    assert!(warnings.is_empty());
    assert_eq!(
        client.settings().normalize.width,
        leanck_config::NormalizeWidth::Finite(84)
    );
}
