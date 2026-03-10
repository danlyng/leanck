//! Typed normalization policies shared with the pure normalization engine.
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RuleError(pub String);
impl std::fmt::Display for RuleError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}
impl std::error::Error for RuleError {}
macro_rules! ensure {($condition:expr,$($message:tt)*)=>{if !$condition {return Err(RuleError(format!($($message)*)));}}}

use std::{collections::BTreeMap, path::PathBuf};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NormalizeRules {
    pub frontmatter: FrontmatterRules,
    pub headings: HeadingRules,
    pub spelling: DictionaryRules,
    pub attributes: AttributeRules,
    pub imports: ImportRules,
    pub comments: CommentRules,
    pub whitespace: WhitespaceRules,
    pub directives: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct FrontmatterRules {
    /// Preferred order for included frontmatter blocks. Omitted blocks remain barriers.
    /// Missing Lean commands are never synthesized; an empty list preserves all positions.
    pub order: Vec<FrontmatterPart>,
    pub blank_lines: u8,
    pub header_template: String,
    pub license_text: String,
    /// Format retained leading headers; replacement is controlled by invalid_existing.
    pub normalize_existing: bool,
    pub invalid_existing: InvalidExistingHeader,
    pub history_timeout_ms: u64,
    pub follow_renames: bool,
    /// Exact author names, emails, or `Name <email>` signatures to omit from new headers.
    /// Matching is ASCII case-insensitive, before and after Git mailmap canonicalization.
    pub ignore_authors: Vec<String>,
    /// Exact, case-sensitive mailmapped Git names mapped to header display strings.
    /// Applied once, after author exclusions, to generated/replaced headers only.
    pub author_map: BTreeMap<String, String>,
    /// Optional module docstring template, used only when the docstring is missing.
    pub module_doc_template: Option<String>,
}
impl Default for FrontmatterRules {
    fn default() -> Self {
        Self {
            order: vec![
                FrontmatterPart::Header,
                FrontmatterPart::Module,
                FrontmatterPart::Imports,
                FrontmatterPart::Docstring,
                FrontmatterPart::Section,
                FrontmatterPart::Namespace,
            ],
            blank_lines: 1,
            header_template: "/-\n{copyright}\n{license}\nAuthors: {authors}\n-/".into(),
            license_text: String::new(),
            normalize_existing: true,
            invalid_existing: InvalidExistingHeader::Warn,
            history_timeout_ms: 5000,
            follow_renames: true,
            ignore_authors: [
                "noreply@anthropic.com",
                "209825114+claude[bot]@users.noreply.github.com",
                "215619710+anthropic-claude[bot]@users.noreply.github.com",
                "208546643+claude-code-action[bot]@users.noreply.github.com",
                "noreply@openai.com",
                "codex@openai.com",
                "215057067+openai-codex[bot]@users.noreply.github.com",
                "199175422+chatgpt-codex-connector[bot]@users.noreply.github.com",
                "cursoragent@cursor.com",
                "206951365+cursor[bot]@users.noreply.github.com",
                "198982749+Copilot@users.noreply.github.com",
                "167198135+copilot[bot]@users.noreply.github.com",
                "aider@aider.chat",
                "noreply@aider.chat",
                "176961590+gemini-code-assist[bot]@users.noreply.github.com",
                "158243242+devin-ai-integration[bot]@users.noreply.github.com",
            ]
            .into_iter()
            .map(str::to_owned)
            .collect(),
            author_map: BTreeMap::new(),
            module_doc_template: None,
        }
    }
}
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InvalidExistingHeader {
    Replace,
    #[default]
    Warn,
    Ignore,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FrontmatterPart {
    Header,
    Module,
    Imports,
    Docstring,
    Section,
    Namespace,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HeadingRules {
    pub level: u8,
    pub case: HeadingCase,
    pub small_words: Vec<String>,
    pub replacements: BTreeMap<String, String>,
    pub template: String,
}
impl Default for HeadingRules {
    fn default() -> Self {
        Self { level: 2, case: HeadingCase::Title, small_words: "a an and as at but by for from in into nor of off on onto or over per so the to up via vs with yet".split_whitespace().map(str::to_owned).collect(), replacements: BTreeMap::new(), template: "/-! {hashes} {title} -/".into() }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeadingCase {
    Off,
    Title,
    Sentence,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DictionaryRules {
    pub use_builtin: bool,
    /// Ordered JSON word -> replacement maps. Later files and inline replacements win.
    pub files: Vec<PathBuf>,
    pub replacements: BTreeMap<String, String>,
    pub protected_words: Vec<String>,
    pub case_sensitive: bool,
    pub inflections: bool,
}
impl Default for DictionaryRules {
    fn default() -> Self {
        Self {
            use_builtin: true,
            files: vec![],
            replacements: BTreeMap::new(),
            protected_words: vec![],
            case_sensitive: false,
            inflections: true,
        }
    }
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AttributeRules {
    pub placement: AttributePlacement,
    /// Per-name overrides, e.g. simp="same_line", ext="above".
    pub overrides: BTreeMap<String, AttributePlacement>,
    pub above_when_long: bool,
}
impl Default for AttributeRules {
    fn default() -> Self {
        Self {
            placement: AttributePlacement::SameLine,
            overrides: BTreeMap::new(),
            above_when_long: false,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AttributePlacement {
    SameLine,
    Above,
    Off,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ImportRules {
    pub order: ImportOrder,
    pub public_first: bool,
    pub preserve_groups: bool,
    pub deduplicate: bool,
}
impl Default for ImportRules {
    fn default() -> Self {
        Self {
            order: ImportOrder::Alphabetical,
            public_first: true,
            preserve_groups: true,
            deduplicate: true,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ImportOrder {
    Off,
    Alphabetical,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct CommentRules {
    pub module: DelimiterStyle,
    pub declaration: DelimiterStyle,
    pub section: DelimiterStyle,
}
impl Default for CommentRules {
    fn default() -> Self {
        Self {
            module: DelimiterStyle::Separate,
            declaration: DelimiterStyle::Compact,
            section: DelimiterStyle::Compact,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DelimiterStyle {
    Compact,
    Separate,
    Off,
}
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct WhitespaceRules {
    pub max_blank_lines: u8,
    pub line_endings: LineEndings,
    pub final_newline: FinalNewline,
    /// Remove the leading UTF-8 byte-order mark; never touch marks inside source text.
    pub remove_bom: bool,
}
impl Default for WhitespaceRules {
    fn default() -> Self {
        Self {
            max_blank_lines: 1,
            line_endings: LineEndings::Lf,
            final_newline: FinalNewline::Ensure,
            remove_bom: true,
        }
    }
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LineEndings {
    Off,
    Lf,
    Crlf,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FinalNewline {
    Off,
    Ensure,
    Remove,
}

impl NormalizeRules {
    pub fn validate(&self) -> Result<(), RuleError> {
        ensure!(
            (1..=6).contains(&self.headings.level),
            "normalize.rules.headings.level must be between 1 and 6"
        );
        ensure!(
            self.frontmatter.blank_lines < u8::MAX && self.whitespace.max_blank_lines < u8::MAX,
            "normalization blank-line counts must be less than {}",
            u8::MAX
        );
        ensure!(
            (1..=60_000).contains(&self.frontmatter.history_timeout_ms),
            "history_timeout_ms must be between 1 and 60000"
        );
        ensure!(
            self.frontmatter
                .order
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                == self.frontmatter.order.len(),
            "frontmatter.order must not repeat components"
        );
        // Check relative safety constraints only when both components are included.
        // Omitted components cannot be crossed by the reordering engine.
        let pos = |part| self.frontmatter.order.iter().position(|p| *p == part);
        let precedes = |left, right| match (pos(left), pos(right)) {
            (Some(l), Some(r)) => l < r,
            _ => true,
        };
        ensure!(
            pos(FrontmatterPart::Header).is_none_or(|p| p == 0)
                && precedes(FrontmatterPart::Module, FrontmatterPart::Imports)
                && precedes(FrontmatterPart::Module, FrontmatterPart::Section)
                && precedes(FrontmatterPart::Module, FrontmatterPart::Namespace)
                && precedes(FrontmatterPart::Imports, FrontmatterPart::Section)
                && precedes(FrontmatterPart::Imports, FrontmatterPart::Namespace),
            "frontmatter.order must keep included headers first and module/imports before scopes"
        );
        ensure!(
            self.frontmatter
                .ignore_authors
                .iter()
                .all(|identity| !identity.trim().is_empty()
                    && !identity.chars().any(char::is_control)),
            "frontmatter.ignore_authors entries must be nonempty single-line identities without control characters"
        );
        ensure!(
            self.frontmatter.author_map.iter().all(|(name, display)| {
                [name, display]
                    .iter()
                    .all(|text| !text.trim().is_empty() && !text.chars().any(char::is_control))
                    && !display.contains("/-")
                    && !display.contains("-/")
            }),
            "frontmatter.author_map keys and values must be nonempty single-line strings without control characters; values must not contain Lean comment delimiters"
        );
        validate_template(
            &self.frontmatter.header_template,
            &[
                "copyright",
                "authors",
                "years",
                "license",
                "first_year",
                "last_year",
            ],
            "header_template",
        )?;
        ensure!(
            self.frontmatter
                .header_template
                .trim_start()
                .starts_with("/-")
                && self.frontmatter.header_template.trim_end().ends_with("-/")
                && !self
                    .frontmatter
                    .header_template
                    .trim_start()
                    .starts_with("/--")
                && !self
                    .frontmatter
                    .header_template
                    .trim_start()
                    .starts_with("/-!"),
            "header_template must be a Lean block comment"
        );
        validate_template(
            &self.headings.template,
            &["title", "hashes"],
            "headings.template",
        )?;
        ensure!(
            self.headings.template.starts_with("/-!")
                && self.headings.template.ends_with("-/")
                && self.headings.template.contains("{title}"),
            "headings.template must be a /-! ... -/ comment containing {{title}}"
        );
        if let Some(template) = &self.frontmatter.module_doc_template {
            validate_template(template, &["module"], "module_doc_template")?;
            ensure!(
                template.starts_with("/-!") && template.ends_with("-/"),
                "module_doc_template must be a /-! ... -/ comment"
            );
        }
        for template in [&self.frontmatter.header_template, &self.headings.template]
            .into_iter()
            .chain(self.frontmatter.module_doc_template.iter())
        {
            let inner = template
                .trim()
                .strip_prefix("/-")
                .unwrap()
                .strip_suffix("-/")
                .unwrap();
            ensure!(
                !inner.contains("-/") && !inner.contains("/-"),
                "comment templates must contain one outer comment only"
            );
        }
        ensure!(
            !self.frontmatter.license_text.contains("-/")
                && !self.frontmatter.license_text.contains("/-"),
            "license_text must not contain Lean comment delimiters"
        );
        for (from, to) in &self.headings.replacements {
            validate_word_pair(from, to)?;
        }
        let mut keys = std::collections::BTreeSet::new();
        for key in self.spelling.replacements.keys() {
            ensure!(
                keys.insert(if self.spelling.case_sensitive {
                    key.clone()
                } else {
                    key.to_lowercase()
                }),
                "ambiguous dictionary replacement key {key:?}"
            );
        }
        for (from, to) in &self.spelling.replacements {
            validate_word_pair(from, to)?;
        }
        for path in &self.spelling.files {
            ensure!(
                !path.as_os_str().is_empty(),
                "dictionary paths must not be empty"
            );
        }
        for word in &self.spelling.protected_words {
            ensure!(
                !word.is_empty(),
                "protected_words entries must not be empty"
            );
        }
        Ok(())
    }
}
fn validate_template(template: &str, allowed: &[&str], name: &str) -> Result<(), RuleError> {
    let mut rest = template;
    while let Some((_, tail)) = rest.split_once('{') {
        let (key, after) = tail
            .split_once('}')
            .ok_or_else(|| RuleError(format!("unclosed placeholder in {name}")))?;
        ensure!(
            allowed.contains(&key),
            "unknown placeholder {{{key}}} in {name}"
        );
        rest = after;
    }
    Ok(())
}
pub fn validate_word_pair(from: &str, to: &str) -> Result<(), RuleError> {
    ensure!(
        !from.is_empty()
            && !to.is_empty()
            && !from.contains(['\n', '\r'])
            && !to.contains(['\n', '\r'])
            && !to.contains("-/")
            && !to.contains("/-"),
        "dictionary entries require nonempty single-line words/replacements without comment delimiters"
    );
    Ok(())
}

impl Default for NormalizeRules {
    fn default() -> Self {
        Self {
            frontmatter: Default::default(),
            headings: Default::default(),
            spelling: Default::default(),
            attributes: Default::default(),
            imports: Default::default(),
            comments: Default::default(),
            whitespace: Default::default(),
            directives: true,
        }
    }
}
