//! Nonfatal, deterministic configuration diagnostics; no filesystem or I/O access.
use crate::{HeadingRules, Normalize, Settings, Spelling};
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Warning {
    pub code: &'static str,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub line: Option<usize>,
}
impl Warning {
    pub fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            line: None,
        }
    }
    pub fn at_line(mut self, line: usize) -> Self {
        self.line = Some(line);
        self
    }
}

impl Normalize {
    pub fn warnings(&self) -> Vec<Warning> {
        let mut warnings = Vec::new();
        let front = &self.rules.frontmatter;
        let cap = self.rules.whitespace.max_blank_lines;
        if self.frontmatter && self.collapse_blank_lines && cap < front.blank_lines {
            warnings.push(Warning::new("frontmatter_blank_line_cap", format!(
                "normalize.rules.whitespace.max_blank_lines={cap} overrides normalize.rules.frontmatter.blank_lines={}; increase the global cap or set normalize.collapse_blank_lines=false to retain the requested spacing", front.blank_lines)));
        }
        let headings = &self.rules.headings;
        if !self.headings && *headings != HeadingRules::default() {
            warnings.push(Warning::new("inactive_heading_rules",
                "custom normalize.rules.headings settings are inactive because normalize.headings=false; set normalize.headings=true to apply them"));
        }
        if self.frontmatter
            && self.header
            && !front.license_text.is_empty()
            && !front.header_template.contains("{license}")
        {
            warnings.push(Warning::new("unused_license_text",
                "normalize.rules.frontmatter.license_text is set but header_template has no {license} placeholder; newly generated headers will omit this configured license text"));
        }
        if self.headings
            && headings.level != HeadingRules::default().level
            && !headings.template.contains("{hashes}")
        {
            warnings.push(Warning::new("unused_heading_level", format!(
                "normalize.rules.headings.level={} is ignored because headings.template has no {{hashes}} placeholder", headings.level)));
        }
        let spelling = &self.rules.spelling;
        let custom = !spelling.files.is_empty() || !spelling.replacements.is_empty();
        if self.spelling == Spelling::Off && custom {
            warnings.push(Warning::new("inactive_custom_dictionaries",
                "custom spelling dictionaries are supplied but normalize.spelling=\"off\", so their substitutions are disabled; select \"american\" or \"british\" and set normalize.rules.spelling.use_builtin=false to use only custom dictionaries"));
        } else if self.comments
            && self.spelling != Spelling::Off
            && !spelling.use_builtin
            && !custom
        {
            warnings.push(Warning::new("empty_spelling_dictionary",
                "spelling is enabled but normalize.rules.spelling.use_builtin=false and no custom dictionaries or replacements are supplied; spelling substitutions will have no effect"));
        }
        warnings
    }
}
impl Settings {
    pub fn warnings(&self) -> Vec<Warning> {
        let mut warnings = self.normalize.warnings();
        warnings.extend(self.client.timeout_warnings(self.daemon.request_timeout_ms));
        warnings
    }
}
impl crate::Client {
    /// Compare socket I/O against the effective request budget, including request overrides.
    pub fn timeout_warnings(&self, default_operation_ms: u64) -> Vec<Warning> {
        let operation = self.timeout_ms.unwrap_or(default_operation_ms);
        match self.transport_timeout_ms {
            Some(transport) if transport < operation => vec![Warning::new(
                "short_transport_timeout",
                format!(
                    "client.transport_timeout_ms={transport} is shorter than the effective operation timeout of {operation} ms; socket I/O may time out before the operation deadline"
                ),
            )],
            _ => Vec::new(),
        }
    }
}
