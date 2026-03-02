use serde::{Deserialize, Serialize};

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
