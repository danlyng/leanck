use crate::MULTI_TERM_EMPTY_NOTE;
use serde::{Deserialize, Serialize};

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
pub(crate) struct LoogleApiResponse {
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
    pub(crate) fn into_result(self, limit: usize) -> LoogleResult {
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
