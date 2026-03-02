use std::env;
use std::time::Duration;

use serde::{Deserialize, Serialize};

pub const DEFAULT_LIMIT: usize = 10;
pub const MULTI_TERM_EMPTY_NOTE: &str = "Loogle joins multiple terms with `and` semantics. Try separate searches if you want broader coverage.";

const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
const LOCAL_URL: &str = "http://127.0.0.1:8118/json";
const SELF_HOSTED_URL: &str = "https://lean4.party/loogle/json";
const MAIN_URL: &str = "https://loogle.lean-lang.org/json";

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct LoogleQuery {
    pub constant: Vec<String>,
    pub substr: Vec<String>,
    pub subexpr: Vec<String>,
    pub conclusion: Vec<String>,
}

impl LoogleQuery {
    pub fn build(&self) -> Option<String> {
        let mut parts = Vec::new();
        for name in &self.constant {
            let name = name.trim();
            if !name.is_empty() {
                parts.push(name.to_string());
            }
        }
        for substr in &self.substr {
            let substr = substr.trim();
            if !substr.is_empty() {
                parts.push(format!("\"{substr}\""));
            }
        }
        for subexpr in &self.subexpr {
            let subexpr = subexpr.trim();
            if !subexpr.is_empty() {
                parts.push(subexpr.to_string());
            }
        }
        for conclusion in &self.conclusion {
            let conclusion = conclusion.trim();
            if conclusion.is_empty() {
                continue;
            }
            if conclusion.starts_with("|-") || conclusion.starts_with('⊢') {
                parts.push(conclusion.to_string());
            } else {
                parts.push(format!("|- {conclusion}"));
            }
        }
        (!parts.is_empty()).then(|| parts.join(", "))
    }

    pub fn term_count(&self) -> usize {
        self.constant
            .iter()
            .chain(&self.substr)
            .chain(&self.subexpr)
            .chain(&self.conclusion)
            .filter(|term| !term.trim().is_empty())
            .count()
    }

    pub fn has_multiple_terms(&self) -> bool {
        self.term_count() > 1
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct LoogleResult {
    pub count: usize,
    pub hits: Vec<LoogleHit>,
    pub suggestions: Vec<String>,
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct LoogleHit {
    pub name: String,
    pub type_sig: String,
    pub module: String,
}

#[derive(Debug, Deserialize)]
struct LoogleApiResponse {
    #[serde(default)]
    count: usize,
    #[serde(default)]
    hits: Vec<LoogleApiHit>,
    #[serde(default)]
    suggestions: Vec<String>,
    error: Option<String>,
}

#[derive(Debug, Deserialize)]
struct LoogleApiHit {
    #[serde(default)]
    name: String,
    #[serde(default, rename = "type")]
    type_sig: String,
    #[serde(default)]
    module: String,
}

impl LoogleApiResponse {
    fn into_result(self, limit: usize) -> LoogleResult {
        let mut hits: Vec<LoogleHit> = self
            .hits
            .into_iter()
            .map(|hit| LoogleHit {
                name: hit.name,
                type_sig: hit.type_sig,
                module: hit.module,
            })
            .collect();
        hits.truncate(limit);
        LoogleResult {
            count: self.count,
            hits,
            suggestions: self.suggestions,
            error: self.error,
            note: None,
        }
    }
}

impl LoogleResult {
    pub fn add_empty_multi_term_note(&mut self, has_multiple_terms: bool) {
        if has_multiple_terms
            && self.count == 0
            && self.hits.is_empty()
            && self.error.as_deref().is_none_or(str::is_empty)
            && self.note.is_none()
        {
            self.note = Some(MULTI_TERM_EMPTY_NOTE.to_string());
        }
    }
}

pub async fn search(query: &str, limit: usize) -> Result<LoogleResult, String> {
    let client = reqwest::Client::new();
    search_with_client(&client, query, limit).await
}

pub async fn search_with_client(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<LoogleResult, String> {
    Ok(search_with_client_endpoint(client, query, limit).await?.0)
}

/// Like [`search`], but also returns the endpoint URL that actually served the
/// request. Callers use this to tell whether the Econlib-indexing local
/// endpoint answered (see [`endpoint_searches_econlib`]).
pub async fn search_with_endpoint(
    query: &str,
    limit: usize,
) -> Result<(LoogleResult, String), String> {
    let client = reqwest::Client::new();
    search_with_client_endpoint(&client, query, limit).await
}

pub async fn search_with_client_endpoint(
    client: &reqwest::Client,
    query: &str,
    limit: usize,
) -> Result<(LoogleResult, String), String> {
    let mut errors = Vec::new();
    for url in loogle_urls() {
        match fetch_endpoint(client, &url, query, limit).await {
            Ok(result) => return Ok((result, url)),
            Err(err) => errors.push(format!("{url}: {err}")),
        }
    }
    Err(format!(
        "loogle request failed for all endpoints: {}",
        errors.join("; ")
    ))
}

/// Whether the given endpoint also indexes Econlib. Only the bundled local
/// endpoint does; the self-hosted server, the main Loogle server, and any
/// custom `LOOGLE_URL` are Mathlib-only.
pub fn endpoint_searches_econlib(url: &str) -> bool {
    let url = url.trim();
    url == LOCAL_URL
        || url.contains("localhost:8118")
        || url.contains("127.0.0.1:8118")
        || url.contains("lean4.party")
}

fn loogle_urls() -> Vec<String> {
    let mut urls = Vec::new();
    if let Ok(url) = env::var("LOOGLE_URL") {
        push_url(&mut urls, url);
    }
    push_url(&mut urls, LOCAL_URL);
    push_url(&mut urls, SELF_HOSTED_URL);
    push_url(&mut urls, MAIN_URL);
    urls
}

fn push_url(urls: &mut Vec<String>, url: impl AsRef<str>) {
    let url = url.as_ref().trim();
    if !url.is_empty() && !urls.iter().any(|existing| existing == url) {
        urls.push(url.to_string());
    }
}

async fn fetch_endpoint(
    client: &reqwest::Client,
    url: &str,
    query: &str,
    limit: usize,
) -> Result<LoogleResult, String> {
    let response = client
        .get(url)
        .query(&[("q", query)])
        .header("Accept", "application/json")
        .header("User-Agent", "leanck-loogle/0.1")
        .timeout(REQUEST_TIMEOUT)
        .send()
        .await
        .map_err(|e| format!("request failed: {e}"))?;
    let status = response.status();
    let body = response
        .text()
        .await
        .map_err(|e| format!("response read failed: {e}"))?;
    if !status.is_success() {
        return Err(format!("HTTP {status}: {}", truncate_for_error(&body)));
    }
    let data: LoogleApiResponse =
        serde_json::from_str(&body).map_err(|e| format!("invalid JSON: {e}"))?;
    Ok(data.into_result(limit))
}

fn truncate_for_error(text: &str) -> String {
    let trimmed = text.trim();
    let mut truncated = trimmed.chars().take(200).collect::<String>();
    if trimmed.chars().count() > 200 {
        truncated.push_str("...");
    }
    truncated
}

pub fn format_human(result: &LoogleResult) -> String {
    let mut lines = Vec::new();
    let shown = result.hits.len();
    if result.count != shown || result.count == 0 {
        lines.push(format!("  {} results, showing {}", result.count, shown));
    }
    for hit in &result.hits {
        let mod_str = if hit.module.is_empty() {
            String::new()
        } else {
            format!("  ({})", hit.module)
        };
        let name = if hit.name.is_empty() { "?" } else { &hit.name };
        lines.push(format!("  {name}{mod_str}"));
        lines.push(format!("    {}", hit.type_sig));
    }
    if !result.suggestions.is_empty() {
        lines.push(String::new());
        lines.push(format!("  Suggestions: {}", result.suggestions.join(", ")));
    }
    if let Some(error) = &result.error
        && !error.is_empty()
    {
        lines.push(format!("  Error: {error}"));
    }
    if let Some(note) = &result.note
        && !note.is_empty()
    {
        lines.push(format!("  Note: {note}"));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_matches_structured_args() {
        let input = LoogleQuery {
            constant: vec!["Real.sin".to_string()],
            substr: vec!["differ".to_string()],
            subexpr: vec!["_ * (_ ^ _)".to_string()],
            conclusion: vec!["tsum _ = _".to_string(), "⊢ a = b".to_string()],
        };

        assert_eq!(
            input.build().as_deref(),
            Some("Real.sin, \"differ\", _ * (_ ^ _), |- tsum _ = _, ⊢ a = b")
        );
    }

    #[test]
    fn query_counts_nonempty_terms() {
        let input = LoogleQuery {
            constant: vec!["Real.sin".to_string(), "  ".to_string()],
            substr: vec!["differ".to_string()],
            subexpr: vec![],
            conclusion: vec![],
        };

        assert_eq!(input.term_count(), 2);
        assert!(input.has_multiple_terms());
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
            format_human(&result),
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
            format_human(&result),
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
            format_human(&result),
            format!("  0 results, showing 0\n  Note: {MULTI_TERM_EMPTY_NOTE}")
        );
    }

    #[test]
    fn single_term_result_does_not_get_note() {
        let mut result = LoogleResult {
            count: 0,
            hits: vec![],
            suggestions: vec![],
            error: None,
            note: None,
        };

        result.add_empty_multi_term_note(false);

        assert!(result.note.is_none());
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
    }

    #[test]
    fn build_does_not_double_prefix_an_existing_turnstile() {
        let q = LoogleQuery {
            constant: vec![],
            substr: vec![],
            subexpr: vec![],
            conclusion: vec!["|- a = b".to_string(), "⊢ c = d".to_string()],
        };
        assert_eq!(q.build().as_deref(), Some("|- a = b, ⊢ c = d"));
    }

    #[test]
    fn truncate_for_error_caps_at_200_chars() {
        assert_eq!(truncate_for_error("  short  "), "short");
        let long = "x".repeat(250);
        let out = truncate_for_error(&long);
        assert_eq!(out.chars().count(), 203); // 200 + "..."
        assert!(out.ends_with("..."));
    }

    #[test]
    fn only_the_local_endpoint_searches_econlib() {
        assert!(endpoint_searches_econlib(LOCAL_URL));
        assert!(endpoint_searches_econlib("http://localhost:8118/json"));
        assert!(endpoint_searches_econlib("http://127.0.0.1:8118/json"));
        assert!(endpoint_searches_econlib(SELF_HOSTED_URL));
        assert!(!endpoint_searches_econlib(MAIN_URL));
        assert!(!endpoint_searches_econlib(
            "https://example.com/loogle/json"
        ));
    }

    #[test]
    fn push_url_trims_and_dedupes() {
        let mut urls = Vec::new();
        push_url(&mut urls, "  http://a  ");
        push_url(&mut urls, "http://a"); // duplicate after trim
        push_url(&mut urls, "   "); // empty
        push_url(&mut urls, "http://b");
        assert_eq!(urls, vec!["http://a".to_string(), "http://b".to_string()]);
    }

    #[test]
    fn into_result_truncates_hits_to_the_limit() {
        let resp = LoogleApiResponse {
            count: 5,
            hits: (0..5)
                .map(|i| LoogleApiHit {
                    name: format!("n{i}"),
                    type_sig: "T".to_string(),
                    module: "M".to_string(),
                })
                .collect(),
            suggestions: vec![],
            error: None,
        };
        let result = resp.into_result(2);
        assert_eq!(result.count, 5); // upstream total preserved
        assert_eq!(result.hits.len(), 2); // local view truncated
    }

    #[test]
    fn format_human_renders_empty_result() {
        let result = LoogleResult {
            count: 0,
            hits: vec![],
            suggestions: vec![],
            error: None,
            note: None,
        };
        assert_eq!(format_human(&result), "  0 results, showing 0");
    }

    #[test]
    fn note_is_skipped_when_an_error_or_hits_are_present() {
        let mut errored = LoogleResult {
            count: 0,
            hits: vec![],
            suggestions: vec![],
            error: Some("boom".to_string()),
            note: None,
        };
        errored.add_empty_multi_term_note(true);
        assert!(errored.note.is_none());

        let mut has_hits = LoogleResult {
            count: 1,
            hits: vec![LoogleHit {
                name: "x".to_string(),
                type_sig: "T".to_string(),
                module: String::new(),
            }],
            suggestions: vec![],
            error: None,
            note: None,
        };
        has_hits.add_empty_multi_term_note(true);
        assert!(has_hits.note.is_none());
    }
}
