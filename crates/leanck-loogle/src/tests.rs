use crate::*;

#[test]
fn query_matches_structured_args() {
    let input = LoogleQuery {
        constant: vec!["Real.sin".to_string()],
        substr: vec!["differ".to_string()],
        subexpr: vec!["_ * (_ ^ _)".to_string()],
        conclusion: vec![
            "tsum _ = _".to_string(),
            "⊢ a = b".to_string(),
            "|- c = d".to_string(),
        ],
    };

    assert_eq!(
        input.build().as_deref(),
        Some("Real.sin, \"differ\", _ * (_ ^ _), |- tsum _ = _, ⊢ a = b, |- c = d")
    );
}

#[test]
fn format_suppresses_summary_when_showing_all_nonempty_results() {
    let result = LoogleResult {
        count: 1,
        hits: vec![LoogleHit {
            name: "Real.sin".to_string(),
            type_sig: "(x : ℝ) : ℝ".to_string(),
            module: "Mathlib.Trig".to_string(),
        }],
        suggestions: vec![],
        error: None,
        note: None,
    };

    assert_eq!(
        format_results(&result),
        "  Real.sin  (Mathlib.Trig)\n    (x : ℝ) : ℝ"
    );
}

#[test]
fn format_matches_human_output() {
    let result = LoogleResult {
        count: 2,
        hits: vec![LoogleHit {
            name: "Real.sin".to_string(),
            type_sig: "(x : ℝ) : ℝ".to_string(),
            module: "Mathlib.Trig".to_string(),
        }],
        suggestions: vec!["try: Real.cos".to_string()],
        error: None,
        note: None,
    };

    assert_eq!(
        format_results(&result),
        "  2 results, showing 1\n  Real.sin  (Mathlib.Trig)\n    (x : ℝ) : ℝ\n\n  Suggestions: try: Real.cos"
    );
}

#[test]
fn empty_multi_term_result_gets_note() {
    let mut result = LoogleResult {
        count: 0,
        hits: vec![],
        suggestions: vec![],
        error: None,
        note: None,
    };

    result.add_empty_multi_term_note(true);

    assert_eq!(result.note.as_deref(), Some(MULTI_TERM_EMPTY_NOTE));
    assert_eq!(
        format_results(&result),
        format!("  0 results, showing 0\n  Note: {MULTI_TERM_EMPTY_NOTE}")
    );
}

#[test]
fn build_returns_none_when_all_terms_blank() {
    let q = LoogleQuery {
        constant: vec!["   ".to_string()],
        substr: vec![],
        subexpr: vec![],
        conclusion: vec!["".to_string()],
    };
    assert!(q.build().is_none());
    assert_eq!(q.term_count(), 0);
    assert!(!q.has_multiple_terms());

    let q = LoogleQuery {
        constant: vec!["Real.sin".to_string(), "  ".to_string()],
        substr: vec!["differ".to_string()],
        subexpr: vec![],
        conclusion: vec![],
    };
    assert_eq!(q.term_count(), 2);
    assert!(q.has_multiple_terms());
}

use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpListener;

async fn read_request(reader: &mut BufReader<tokio::net::tcp::OwnedReadHalf>) -> Vec<String> {
    let mut headers = Vec::new();
    loop {
        let mut line = String::new();
        assert!(reader.read_line(&mut line).await.unwrap() > 0);
        if line == "\r\n" {
            return headers;
        }
        headers.push(line.trim_end().to_string());
    }
}

async fn service(
    status: &'static str,
    body: String,
    extra_headers: String,
) -> (String, tokio::task::JoinHandle<Vec<String>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/json", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let headers = read_request(&mut BufReader::new(read)).await;
        let response = format!(
            "HTTP/1.1 {status}\r\nContent-Length: {}\r\nContent-Type: application/json\r\nConnection: close\r\n{extra_headers}\r\n{body}",
            body.len()
        );
        write.write_all(response.as_bytes()).await.unwrap();
        write.shutdown().await.unwrap();
        headers
    });
    (url, server)
}

fn query() -> LoogleQuery {
    LoogleQuery {
        constant: vec!["Nat.add".into()],
        substr: vec!["add".into()],
        ..Default::default()
    }
}

#[test]
fn endpoint_order_deduplication_and_role_are_explicit() {
    let defaults = Client::default();
    assert_eq!(
        defaults
            .endpoints()
            .iter()
            .map(|e| e.source)
            .collect::<Vec<_>>(),
        [EndpointSource::Local, EndpointSource::Main]
    );
    assert_eq!(
        defaults
            .endpoints()
            .iter()
            .map(|e| e.url.clone())
            .collect::<Vec<_>>(),
        leanck_config::Loogle::default().endpoints
    );
    let client = Client::with_endpoints(vec![
        Endpoint::new(" https://custom.example/json ", EndpointSource::Configured),
        Endpoint::new("https://custom.example/json", EndpointSource::Main),
        Endpoint::new(" ", EndpointSource::Configured),
    ]);
    assert_eq!(client.endpoints().len(), 1);
    assert_eq!(client.endpoints()[0].url, "https://custom.example/json");
    assert_eq!(client.endpoints()[0].source, EndpointSource::Configured);
}

#[tokio::test]
async fn empty_query_is_rejected_before_network_and_empty_endpoints_are_errors() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::with_endpoints(vec![Endpoint::new(
        format!("http://{}/json", listener.local_addr().unwrap()),
        EndpointSource::Override,
    )]);
    assert!(matches!(
        client
            .search(&LoogleQuery::default(), SearchOptions::default())
            .await,
        Err(SearchError::EmptyQuery)
    ));
    assert!(
        tokio::time::timeout(Duration::from_millis(20), listener.accept())
            .await
            .is_err()
    );
    assert!(matches!(
        Client::with_endpoints(vec![])
            .search(&query(), SearchOptions::default())
            .await,
        Err(SearchError::NoEndpoints)
    ));
}

#[tokio::test]
async fn fallback_handles_http_and_json_failures_limits_and_provenance() {
    let (failed, first) = service(
        "503 Service Unavailable",
        "unavailable".into(),
        String::new(),
    )
    .await;
    let (malformed, second) = service("200 OK", "not JSON".into(), String::new()).await;
    let (working, third) = service("200 OK", r#"{"count":5,"hits":[{"name":"one","type":"Nat","module":"Local"},{"name":"two","type":"Nat"},{"name":"three","type":"Nat"}],"suggestions":["try another query"]}"#.into(), String::new()).await;
    let client = Client::with_endpoints(vec![
        Endpoint::new(&failed, EndpointSource::Override),
        Endpoint::new(&malformed, EndpointSource::Local),
        Endpoint::new(&working, EndpointSource::SelfHosted),
    ]);
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        client.search(&query(), SearchOptions { limit: 2 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(outcome.result.count, 5);
    assert_eq!(outcome.result.hits.len(), 2);
    assert_eq!(outcome.provenance.requested_url, working);
    assert_eq!(outcome.provenance.url, working);
    assert_eq!(outcome.provenance.source, EndpointSource::SelfHosted);
    assert!(outcome.result.note.is_none());
    let json = serde_json::to_value(&outcome).unwrap();
    assert_eq!(json["count"], 5);
    assert_eq!(json["provenance"]["source"], "self_hosted");
    assert!(format_human(&outcome).ends_with(&format!("  Endpoint: {working}")));
    let requests = [
        first.await.unwrap(),
        second.await.unwrap(),
        third.await.unwrap(),
    ];
    for headers in requests {
        let target = headers[0].split_whitespace().nth(1).unwrap();
        let url = reqwest::Url::parse("http://fake.example")
            .unwrap()
            .join(target)
            .unwrap();
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "q").unwrap().1,
            "Nat.add, \"add\""
        );
        assert!(
            headers
                .iter()
                .any(|header| header.eq_ignore_ascii_case("accept: application/json"))
        );
    }
}

#[tokio::test]
async fn all_failures_preserve_structured_endpoint_errors() {
    let (failed, first) = service("502 Bad Gateway", "x".repeat(300), String::new()).await;
    let (malformed, second) = service("200 OK", "bad JSON".into(), String::new()).await;
    let client = Client::with_endpoints(vec![
        Endpoint::new("invalid URL", EndpointSource::Override),
        Endpoint::new(&failed, EndpointSource::Local),
        Endpoint::new(&malformed, EndpointSource::Main),
    ]);
    let error = client
        .search(&query(), SearchOptions::default())
        .await
        .unwrap_err();
    let text = error.to_string();
    let SearchError::AllEndpointsFailed(failures) = error else {
        panic!("expected aggregate failure");
    };
    assert_eq!(failures.0.len(), 3);
    assert!(matches!(failures.0[0].error, EndpointError::Request(_)));
    assert!(
        matches!(failures.0[1].error, EndpointError::Http { status: 502, ref body } if body.len() == 203)
    );
    assert!(matches!(failures.0[2].error, EndpointError::Json(_)));
    assert!(text.contains("invalid JSON"));
    assert!(text.contains("HTTP 502"));
    first.await.unwrap();
    second.await.unwrap();
}

#[tokio::test]
async fn cloned_clients_reuse_connection_and_apply_multi_term_policy() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/json", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        // Both searches must use this single accepted keep-alive connection.
        let (stream, _) = listener.accept().await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut reader = BufReader::new(read);
        for _ in 0..2 {
            read_request(&mut reader).await;
            let body = r#"{"count":0,"hits":[]}"#;
            write.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: application/json\r\n\r\n{body}", body.len()).as_bytes()).await.unwrap();
        }
    });
    let client = Client::with_endpoints(vec![Endpoint::new(url, EndpointSource::Local)]);
    let first = tokio::time::timeout(
        Duration::from_secs(2),
        client.search(&query(), SearchOptions::default()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(first.result.note.as_deref(), Some(MULTI_TERM_EMPTY_NOTE));
    let single = LoogleQuery {
        constant: vec!["Nat.add".into()],
        ..Default::default()
    };
    let second = tokio::time::timeout(
        Duration::from_secs(2),
        client.clone().search(&single, SearchOptions { limit: 0 }),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(second.result.note.is_none());
    server.await.unwrap();
}

#[tokio::test]
async fn query_errors_do_not_trigger_fallback_or_empty_notes() {
    let (url, server) = service(
        "200 OK",
        r#"{"count":0,"hits":[],"error":"unknown constant"}"#.into(),
        String::new(),
    )
    .await;
    let unused = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let client = Client::with_endpoints(vec![
        Endpoint::new(url, EndpointSource::Override),
        Endpoint::new(
            format!("http://{}/json", unused.local_addr().unwrap()),
            EndpointSource::Local,
        ),
    ]);
    let outcome = client
        .search(&query(), SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(outcome.result.error.as_deref(), Some("unknown constant"));
    assert!(outcome.result.note.is_none());
    assert!(
        tokio::time::timeout(Duration::from_millis(20), unused.accept())
            .await
            .is_err()
    );
    server.await.unwrap();
}

#[tokio::test]
async fn redirects_report_the_final_endpoint() {
    let (destination, destination_server) =
        service("200 OK", r#"{"count":0,"hits":[]}"#.into(), String::new()).await;
    let (redirect, redirect_server) = service(
        "302 Found",
        String::new(),
        format!("Location: {destination}\r\n"),
    )
    .await;
    let client = Client::with_endpoints(vec![Endpoint::new(&redirect, EndpointSource::Override)]);
    let outcome = client
        .search(&query(), SearchOptions::default())
        .await
        .unwrap();
    assert_eq!(outcome.provenance.requested_url, redirect);
    assert_eq!(outcome.provenance.url, destination);
    assert_eq!(outcome.provenance.source, EndpointSource::Override);
    redirect_server.await.unwrap();
    destination_server.await.unwrap();
}

#[tokio::test]
async fn configured_timeout_falls_back_only_to_configured_endpoints() {
    let silent = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let silent_url = format!("http://{}/json", silent.local_addr().unwrap());
    let stalled = tokio::spawn(async move {
        let (_stream, _) = silent.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let (working, server) =
        service("200 OK", r#"{"count":0,"hits":[]}"#.into(), String::new()).await;
    let config = leanck_config::Loogle {
        endpoints: vec![silent_url, working.clone()],
        timeout_ms: 50,
        limit: 2,
    };
    let client = Client::from_config(&config);
    assert_eq!(client.endpoints().len(), 2);
    let outcome = tokio::time::timeout(
        Duration::from_secs(2),
        client.search(
            &query(),
            SearchOptions {
                limit: config.limit,
            },
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(outcome.provenance.url, working);
    assert_eq!(outcome.provenance.source, EndpointSource::Configured);
    server.await.unwrap();
    stalled.abort();
}
