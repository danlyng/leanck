use leanck_core::api::*;
use serde::{Serialize, de::DeserializeOwned};
use serde_json::{Value, json};

fn fixture(name: &str) -> Value {
    serde_json::from_str::<Value>(include_str!("../../../test-support/contracts/v4.json")).unwrap()
        [name]
        .clone()
}

fn round_trip<T: DeserializeOwned + Serialize>(value: Value) {
    let decoded: T = serde_json::from_value(value.clone()).unwrap();
    assert_eq!(serde_json::to_value(decoded).unwrap(), value);
}

#[test]
fn current_protocol_result_and_envelope_fixtures_round_trip() {
    let request = fixture("request");
    round_trip::<Request>(request.clone());
    let request: Request = serde_json::from_value(request).unwrap();
    let Command::Diagnostics(params) = request.command().unwrap() else {
        panic!("expected diagnostics request");
    };
    round_trip::<DiagnosticsParams>(request.params);
    assert_eq!(params.severity, SeverityFilter::All);
    assert_eq!(fixture("health")["protocol_version"], PROTOCOL_VERSION);
    for name in ["success", "null_success", "error"] {
        round_trip::<Response>(fixture(name));
    }
    round_trip::<DiagnosticsResult>(fixture("clean_diagnostics"));
    round_trip::<GoalResult>(fixture("goal_null"));
    round_trip::<GoalResult>(fixture("goal_empty"));
    round_trip::<SorryResult>(fixture("sorry_file"));
    round_trip::<SorryResult>(fixture("sorry_directory"));
    round_trip::<ListResult>(fixture("list"));
    round_trip::<ReferencesResult>(fixture("references"));
    round_trip::<RestartFileResult>(fixture("restart"));
    round_trip::<HealthResult>(fixture("health"));
}

#[test]
fn explicit_null_result_is_distinct_from_absent_result() {
    let null: Response = serde_json::from_value(fixture("null_success")).unwrap();
    assert_eq!(null.into_result("contract").unwrap().unwrap(), Value::Null);
    for invalid in [
        json!({"id":"x","ok":true}),
        json!({"id":"x","ok":false}),
        json!({"id":"x","ok":false,"error":null}),
        json!({"id":"x","ok":false,"result":null,"error":{"kind":"internal","message":"both"}}),
        json!({"id":"x","ok":true,"result":null,"error":{"kind":"internal","message":"both"}}),
    ] {
        assert!(serde_json::from_value::<Response>(invalid).is_err());
    }
    for invalid in fixture("malformed_responses").as_array().unwrap() {
        assert!(serde_json::from_value::<Response>(invalid.clone()).is_err());
    }
}

#[test]
fn operations_bind_their_parameters_in_both_directions() {
    let params: DiagnosticsParams = serde_json::from_value(
        json!({"file":"Basic.lean","severity":{"mode":"all"},"limit":1,"offset":1}),
    )
    .unwrap();
    assert_eq!((params.limit, params.offset), (Some(1), Some(1)));
    assert_eq!(
        serde_json::to_value(DiagnosticsParams {
            file: "Basic.lean".into(),
            ..Default::default()
        })
        .unwrap(),
        json!({"file": "Basic.lean", "severity": {"mode":"only","severities":["error"]}})
    );
    for command in [
        Command::Health,
        Command::Status,
        Command::Shutdown(ShutdownParams::default()),
        Command::Shutdown(ShutdownParams { if_idle: true }),
        Command::Diagnostics(DiagnosticsParams {
            file: "Basic.lean".into(),
            ..Default::default()
        }),
        Command::Goal(GoalParams {
            file: "Basic.lean".into(),
            line: 1,
            character: 2,
        }),
        Command::Sorry(SorryParams {
            path: ".".into(),
            ..Default::default()
        }),
        Command::List(ListParams {
            file: "Basic.lean".into(),
            kind: None,
        }),
        Command::References(ReferencesParams {
            file: "Basic.lean".into(),
            ident: "hello_world".into(),
            namespace: None,
        }),
        Command::RestartFile(RestartFileParams {
            file: "Basic.lean".into(),
        }),
        Command::Cancel(CancelParams {
            target_id: "target".into(),
        }),
    ] {
        let expected = command.operation();
        let request = command.into_request("request".into(), Some(37)).unwrap();
        let decoded = request.command().unwrap();
        assert_eq!(decoded.operation(), expected);
        assert_eq!(
            serde_json::to_value(decoded.into_request("request".into(), Some(37)).unwrap())
                .unwrap(),
            serde_json::to_value(request).unwrap()
        );
    }
    for (op, params) in [
        ("goal", json!({"file":"Basic.lean"})),
        ("diagnostics", json!({"path":"Basic.lean"})),
        ("sorry", json!({"file":"Basic.lean"})),
        ("references", json!({"file":"Basic.lean"})),
        ("unsupported", json!({})),
    ] {
        assert!(
            Request {
                id: "x".into(),
                op: op.into(),
                timeout_ms: None,
                params
            }
            .command()
            .is_err()
        );
    }
}

#[test]
fn shutdown_parameters_default_to_unconditional_and_stay_compatible() {
    let request = |params: Value| Request {
        id: "x".into(),
        op: "shutdown".into(),
        timeout_ms: None,
        params,
    };
    // Old clients send `{}` (or nothing); both mean an unconditional shutdown.
    for params in [json!({}), Value::Null] {
        let Command::Shutdown(parsed) = request(params).command().unwrap() else {
            panic!("expected shutdown");
        };
        assert_eq!(parsed, ShutdownParams::default());
    }
    let Command::Shutdown(parsed) = request(json!({"if_idle": true})).command().unwrap() else {
        panic!("expected shutdown");
    };
    assert!(parsed.if_idle);
    assert!(
        request(json!({"if_idle": true, "force": true}))
            .command()
            .is_err()
    );
    // The default still serializes as `{}`, so old daemons see no change.
    let unconditional = Command::Shutdown(ShutdownParams::default())
        .into_request("x".into(), None)
        .unwrap();
    assert_eq!(unconditional.params, json!({}));
    let conditional = Command::Shutdown(ShutdownParams { if_idle: true })
        .into_request("x".into(), None)
        .unwrap();
    assert_eq!(conditional.params, json!({"if_idle": true}));
}

#[test]
fn diagnostics_preserve_enrichment_numeric_severity_and_extension_nulls() {
    let diagnostic = json!({
        "range":{"start":{"line":1,"character":2},"end":{"line":1,"character":3}},
        "severity":2, "code":"hasSorry", "message":"declaration uses sorry", "tags":[1],
        "relatedInformation":[{"location":{"uri":"file:///other.lean","range":{"start":{"line":0,"character":0},"end":{"line":0,"character":1}}},"message":"related"}],
        "data":{"lean_extension":true}, "goals":["n = m"]
    });
    round_trip::<Diagnostic>(diagnostic.clone());
    let mut diagnostic = diagnostic;
    diagnostic["severity"] = json!(17);
    diagnostic["code"] = Value::Null;
    diagnostic["data"] = Value::Null;
    diagnostic["relatedInformation"] = Value::Null;
    round_trip::<Diagnostic>(diagnostic);
    round_trip::<GoalResult>(json!({"goals":["⊢ True"],"lean_extension":{"value":1}}));
}

#[test]
fn required_fields_cannot_masquerade_as_empty_results() {
    for name in [
        "uri",
        "range",
        "symbol",
        "version",
        "total",
        "diagnostics",
        "source",
        "notes",
    ] {
        let mut value = fixture("clean_diagnostics");
        value.as_object_mut().unwrap().remove(name);
        assert!(
            serde_json::from_value::<DiagnosticsResult>(value).is_err(),
            "missing {name}"
        );
    }
    assert!(serde_json::from_value::<GoalResult>(json!({})).is_err());
    assert!(
        serde_json::from_value::<SorryResult>(
            json!({"uri":"file:///x","sorries":[],"total":0,"files":[]})
        )
        .is_err()
    );
    let mut health = fixture("health");
    health["server"].as_object_mut().unwrap().remove("pid");
    assert!(serde_json::from_value::<HealthResult>(health).is_err());
    let mut health = fixture("health");
    health["server"]["pid"] = Value::Null;
    round_trip::<HealthResult>(health);
}

#[test]
fn pinned_source_and_lifecycle_reject_missing_or_null_fields() {
    for null in [false, true] {
        let mut diagnostics = fixture("clean_diagnostics");
        let mut health = fixture("health");
        if null {
            diagnostics["source"] = Value::Null;
            health["lifecycle"] = Value::Null;
        } else {
            diagnostics.as_object_mut().unwrap().remove("source");
            health.as_object_mut().unwrap().remove("lifecycle");
        }
        assert!(serde_json::from_value::<DiagnosticsResult>(diagnostics).is_err());
        assert!(serde_json::from_value::<HealthResult>(health).is_err());
    }
}

#[test]
fn sorry_params_only_serialize_the_scan_path() {
    round_trip::<SorryParams>(json!({"path":"."}));
    let params = SorryParams { path: ".".into() };
    assert_eq!(serde_json::to_value(params).unwrap(), json!({"path":"."}));
}

#[test]
fn pagination_and_typed_severity_round_trip() {
    let params = DiagnosticsParams {
        file: "x".into(),
        limit: Some(u64::MAX),
        offset: Some(u64::MAX),
        ..Default::default()
    };
    round_trip::<DiagnosticsParams>(serde_json::to_value(params).unwrap());
    let omitted: DiagnosticsParams = serde_json::from_value(json!({"file":"x"})).unwrap();
    assert_eq!(omitted.severity, SeverityFilter::default());
    for severity in [
        json!({"mode":"all"}),
        json!({"mode":"only","severities":[]}),
        json!({"mode":"only","severities":["error","warning","information","hint"]}),
        json!({"mode":"only","severities":["warning","warning"]}),
    ] {
        round_trip::<SeverityFilter>(severity.clone());
        round_trip::<DiagnosticsParams>(json!({"file":"x","severity":severity}));
    }
}

#[test]
fn invalid_severity_names_and_shapes_are_rejected() {
    for severity in [
        json!(null),
        json!("all"),
        json!(["error"]),
        json!(["all"]),
        json!({}),
        json!({"mode":"bogus"}),
        json!({"mode":"only"}),
        json!({"mode":"only","severities":"error"}),
        json!({"mode":"only","severities":["info"]}),
        json!({"mode":"only","severities":["warn"]}),
        json!({"mode":"only","severities":["all"]}),
        json!({"mode":"only","severities":[1]}),
        json!({"mode":"only","severities":["Error"]}),
        json!({"mode":"all","severities":["error"]}),
    ] {
        assert!(
            serde_json::from_value::<DiagnosticsParams>(json!({"file":"x","severity":severity}))
                .is_err(),
            "accepted invalid filter: {severity}"
        );
    }
}
