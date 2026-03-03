use crate::{DEFAULT_LIMIT, LoogleQuery, LoogleResult, models::LoogleApiResponse};
use serde::Serialize;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);

/// Identifies a configured fallback slot; it makes no claim about indexed libraries.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EndpointSource {
    Override,
    Local,
    SelfHosted,
    Main,
    Configured,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Endpoint {
    pub url: String,
    pub source: EndpointSource,
}
impl Endpoint {
    pub fn new(url: impl Into<String>, source: EndpointSource) -> Self {
        Self {
            url: url.into(),
            source,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct EndpointProvenance {
    pub source: EndpointSource,
    pub requested_url: String,
    /// Final endpoint URL after redirects, without request query parameters.
    pub url: String,
}

#[derive(Debug, Clone, Copy)]
pub struct SearchOptions {
    pub limit: usize,
}
impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            limit: DEFAULT_LIMIT,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct SearchOutcome {
    #[serde(flatten)]
    pub result: LoogleResult,
    pub provenance: EndpointProvenance,
}

#[derive(Debug, thiserror::Error)]
pub enum EndpointError {
    #[error("request failed: {0}")]
    Request(#[source] reqwest::Error),
    #[error("response read failed: {0}")]
    Read(#[source] reqwest::Error),
    #[error("HTTP {status}: {body}")]
    Http { status: u16, body: String },
    #[error("invalid JSON: {0}")]
    Json(#[source] serde_json::Error),
}

#[derive(Debug)]
pub struct EndpointFailure {
    pub endpoint: Endpoint,
    pub error: EndpointError,
}
#[derive(Debug)]
pub struct EndpointFailures(pub Vec<EndpointFailure>);
impl std::fmt::Display for EndpointFailures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for (index, failure) in self.0.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            write!(f, "{}: {}", failure.endpoint.url, failure.error)?;
        }
        Ok(())
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    #[error("no Loogle query provided. Use constant, substr, subexpr, or conclusion.")]
    EmptyQuery,
    #[error("no Loogle endpoints configured")]
    NoEndpoints,
    #[error("loogle request failed for all endpoints: {0}")]
    AllEndpointsFailed(EndpointFailures),
}

/// Clones reuse the same reqwest connection pool throughout an MCP session.
#[derive(Debug, Clone)]
pub struct Client {
    http: reqwest::Client,
    endpoints: Vec<Endpoint>,
    timeout: Duration,
}
impl Default for Client {
    fn default() -> Self {
        Self::from_config(&leanck_config::Loogle::default())
    }
}
impl Client {
    pub fn with_endpoints(endpoints: Vec<Endpoint>) -> Self {
        Self::with_http(reqwest::Client::new(), endpoints)
    }
    pub fn with_http(http: reqwest::Client, endpoints: Vec<Endpoint>) -> Self {
        let mut unique = Vec::new();
        for endpoint in endpoints {
            push_endpoint(&mut unique, endpoint);
        }
        Self {
            http,
            endpoints: unique,
            timeout: REQUEST_TIMEOUT,
        }
    }
    pub fn from_config(config: &leanck_config::Loogle) -> Self {
        Self::with_endpoints(Vec::new()).configured(config)
    }
    /// Reconfigure while retaining the HTTP connection pool.
    pub fn configured(&self, config: &leanck_config::Loogle) -> Self {
        let defaults = default_endpoints();
        let endpoints = config
            .endpoints
            .iter()
            .map(|url| {
                let source = defaults
                    .iter()
                    .find(|e| &e.url == url)
                    .map(|e| e.source)
                    .unwrap_or(EndpointSource::Configured);
                Endpoint::new(url.clone(), source)
            })
            .collect();
        let mut client = Self::with_http(self.http.clone(), endpoints);
        client.timeout = Duration::from_millis(config.timeout_ms);
        client
    }
    pub fn endpoints(&self) -> &[Endpoint] {
        &self.endpoints
    }

    pub async fn search(
        &self,
        input: &LoogleQuery,
        options: SearchOptions,
    ) -> Result<SearchOutcome, SearchError> {
        let query = input.build().ok_or(SearchError::EmptyQuery)?;
        if self.endpoints.is_empty() {
            return Err(SearchError::NoEndpoints);
        }
        let mut failures = Vec::new();
        for endpoint in &self.endpoints {
            match self.fetch(endpoint, &query, options.limit).await {
                Ok((mut result, url)) => {
                    result.add_empty_multi_term_note(input.has_multiple_terms());
                    return Ok(SearchOutcome {
                        result,
                        provenance: EndpointProvenance {
                            source: endpoint.source,
                            requested_url: endpoint.url.clone(),
                            url,
                        },
                    });
                }
                Err(error) => failures.push(EndpointFailure {
                    endpoint: endpoint.clone(),
                    error,
                }),
            }
        }
        Err(SearchError::AllEndpointsFailed(EndpointFailures(failures)))
    }

    async fn fetch(
        &self,
        endpoint: &Endpoint,
        query: &str,
        limit: usize,
    ) -> Result<(LoogleResult, String), EndpointError> {
        let response = self
            .http
            .get(&endpoint.url)
            .query(&[("q", query)])
            .header("Accept", "application/json")
            .header(
                "User-Agent",
                concat!("leanck-loogle/", env!("CARGO_PKG_VERSION")),
            )
            .timeout(self.timeout)
            .send()
            .await
            .map_err(EndpointError::Request)?;
        let mut url = response.url().clone();
        url.set_query(None);
        url.set_fragment(None);
        let url = url.to_string();
        let status = response.status();
        let body = response.text().await.map_err(EndpointError::Read)?;
        if !status.is_success() {
            return Err(EndpointError::Http {
                status: status.as_u16(),
                body: truncate_for_error(&body),
            });
        }
        let data: LoogleApiResponse = serde_json::from_str(&body).map_err(EndpointError::Json)?;
        Ok((data.into_result(limit), url))
    }
}

pub(crate) fn default_endpoints() -> Vec<Endpoint> {
    let mut endpoints = Vec::new();
    for (url, source) in [
        ("http://127.0.0.1:8118/loogle/json", EndpointSource::Local),
        ("https://loogle.lean-lang.org/json", EndpointSource::Main),
    ] {
        push_endpoint(&mut endpoints, Endpoint::new(url, source));
    }
    endpoints
}

fn push_endpoint(endpoints: &mut Vec<Endpoint>, mut endpoint: Endpoint) {
    endpoint.url = endpoint.url.trim().to_owned();
    if !endpoint.url.is_empty()
        && !endpoints
            .iter()
            .any(|existing| existing.url == endpoint.url)
    {
        endpoints.push(endpoint);
    }
}

pub(crate) fn truncate_for_error(text: &str) -> String {
    let trimmed = text.trim();
    let mut truncated = trimmed.chars().take(200).collect::<String>();
    if trimmed.chars().count() > 200 {
        truncated.push_str("...");
    }
    truncated
}
