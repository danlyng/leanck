use markdown_reflow::{EdgeWidths, Options as ReflowOptions, ReflowWidth};

use crate::Context;
use crate::prose::DocCommentKind;
pub use crate::spelling::SpellingDirection;
use crate::spelling::correct_spelling;
use crate::{CommentKind, NormalizeError};
use leanck_config::{DelimiterStyle, HeadingCase, NormalizeRules, NormalizeWidth};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub frontmatter: crate::frontmatter::Options,
    pub attributes: bool,
    /// Opt-in: convert `-- ===` banner comments into `/-! ## Heading -/`.
    pub headings: bool,
    pub comments: bool,
    pub comment_reflow: bool,
    /// Opt-in: turn plain URLs inside comments into markdown links.
    pub linkify_urls: bool,
    pub spelling: Option<SpellingDirection>,
    pub capitalize_after_colon: bool,
    pub collapse_blank_lines: bool,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Stats {
    pub frontmatter_changes: usize,
    pub attributes_moved: usize,
    pub headings_converted: usize,
    pub comment_blocks_changed: usize,
    pub blank_lines_collapsed: usize,
}

impl Stats {
    /// Accumulate another file's stats into this one.
    pub fn add(&mut self, other: &Stats) {
        self.frontmatter_changes += other.frontmatter_changes;
        self.attributes_moved += other.attributes_moved;
        self.headings_converted += other.headings_converted;
        self.comment_blocks_changed += other.comment_blocks_changed;
        self.blank_lines_collapsed += other.blank_lines_collapsed;
    }

    pub fn summary_parts(&self) -> Vec<String> {
        let mut parts = Vec::new();
        if self.frontmatter_changes > 0 {
            parts.push(format!("frontmatter={}", self.frontmatter_changes));
        }
        if self.attributes_moved > 0 {
            parts.push(format!("attributes={}", self.attributes_moved));
        }
        if self.headings_converted > 0 {
            parts.push(format!("headings={}", self.headings_converted));
        }
        if self.comment_blocks_changed > 0 {
            parts.push(format!("comments={}", self.comment_blocks_changed));
        }
        if self.blank_lines_collapsed > 0 {
            parts.push(format!("blank_lines={}", self.blank_lines_collapsed));
        }
        parts
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NormalizeResult {
    pub output: String,
    pub stats: Stats,
    pub warnings: Vec<leanck_config::Warning>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            frontmatter: crate::frontmatter::Options::default(),
            attributes: true,
            headings: false,
            comments: true,
            comment_reflow: true,
            linkify_urls: false,
            spelling: Some(SpellingDirection::BritishToAmerican),
            capitalize_after_colon: false,
            collapse_blank_lines: true,
        }
    }
}

impl Options {
    pub(crate) fn configuration(
        self,
        width: NormalizeWidth,
        rules: &NormalizeRules,
    ) -> leanck_config::Normalize {
        leanck_config::Normalize {
            width,
            rules: rules.clone(),
            frontmatter: self.frontmatter.enabled,
            header: self.frontmatter.header,
            sort_imports: self.frontmatter.sort_imports,
            attributes: self.attributes,
            headings: self.headings,
            comments: self.comments,
            comment_reflow: self.comment_reflow,
            linkify_urls: self.linkify_urls,
            spelling: match self.spelling {
                Some(SpellingDirection::BritishToAmerican) => leanck_config::Spelling::American,
                Some(SpellingDirection::AmericanToBritish) => leanck_config::Spelling::British,
                None => leanck_config::Spelling::Off,
            },
            capitalize_after_colon: self.capitalize_after_colon,
            collapse_blank_lines: self.collapse_blank_lines,
            ..Default::default()
        }
    }
}

const SUFFIX: &str = "-/";

/// Overhead of the opening delimiter followed by a space: `/-- ` or `/-! `.
const OPENING_OVERHEAD: usize = 4;

/// Reflow all Lean block comments (`/-- ... -/` and `/-! ... -/`) in `source`
/// to fit within `target_width` characters per line.
///
/// - The **first** `/-!` block (module docstring) always gets its delimiters
///   on their own lines.
/// - Every other block prefers delimiters on the same line as the content
///   they delimit.  An orphan closing `-/` is never left alone — the last
///   word is kicked to the delimiter's line when necessary.  Markdown table
///   rows are the exception: a delimiter never shares a line with a table
///   row, since that would break the column alignment.
/// - Unmatched delimiters cause an error.
#[cfg(test)]
pub fn normalize(
    source: &str,
    target_width: usize,
    options: Options,
) -> Result<String, NormalizeError> {
    normalize_with_stats(source, target_width.into(), options, 2026).map(|result| result.output)
}

pub fn normalize_with_stats(
    source: &str,
    target_width: NormalizeWidth,
    options: Options,
    _copyright_year: i32,
) -> Result<NormalizeResult, NormalizeError> {
    crate::normalize_with_context(
        source,
        target_width,
        options,
        &NormalizeRules::default(),
        &Context::default(),
    )
}

/// `guard_msgs_after` tells whether a `#guard_msgs` command follows `source`
/// after only whitespace and line comments, such as directives that end it.
pub(crate) fn normalize_configured(
    source: &str,
    target_width: NormalizeWidth,
    options: Options,
    rules: &NormalizeRules,
    context: &Context<'_>,
    comment_layout: bool,
    guard_msgs_after: bool,
) -> Result<NormalizeResult, NormalizeError> {
    let mut stats = Stats::default();
    let mut warnings = Vec::new();
    if let Some(warning) =
        crate::frontmatter::header_warning(source, options.frontmatter, &rules.frontmatter, context)
    {
        warnings.push(warning);
    }
    // Capture attribute diagnostics against the original segment's line numbers,
    // before frontmatter insertion or reordering can shift their locations.
    let attribute_normalized = if options.attributes {
        let (output, moved, attribute_warnings) =
            crate::attributes::configured(source, &rules.attributes, target_width);
        stats.attributes_moved = moved;
        warnings.extend(attribute_warnings);
        output
    } else {
        source.to_owned()
    };
    // Collapsing to zero blank lines merges import groups, so sort them as one
    // group now rather than again on the next run.
    let merged_groups;
    let frontmatter_rules = if options.collapse_blank_lines
        && rules.whitespace.max_blank_lines == 0
        && rules.imports.preserve_groups
    {
        merged_groups = NormalizeRules {
            imports: leanck_config::ImportRules {
                preserve_groups: false,
                ..rules.imports.clone()
            },
            ..rules.clone()
        };
        &merged_groups
    } else {
        rules
    };
    // Convert banners before frontmatter, so that frontmatter sees the
    // comments the next run will see.
    let heading_normalized = if options.headings {
        let (heading_normalized, converted) =
            convert_heading_banners_configured(&attribute_normalized, &rules.headings);
        stats.headings_converted = converted;
        heading_normalized
    } else {
        attribute_normalized
    };
    let normalized = crate::frontmatter::configured(
        &heading_normalized,
        options.frontmatter,
        frontmatter_rules,
        context,
    );
    if normalized != heading_normalized {
        stats.frontmatter_changes = 1;
    }
    let mut lines: Vec<String> = normalized.lines().map(|s| s.to_string()).collect();

    // Collapse blank lines first: comment rewriting never creates collapsible
    // runs, and a comment it leaves alone must not change afterwards.
    if options.collapse_blank_lines {
        stats.blank_lines_collapsed = collapse_blank_lines_configured(
            &mut lines,
            usize::from(rules.whitespace.max_blank_lines),
            guard_msgs_after,
        );
    }

    if options.comments
        && (comment_layout
            || options.headings
            || options.spelling.is_some()
            || options.comment_reflow
            || options.capitalize_after_colon)
    {
        let collapsed = lines.join("\n");
        // Report an unmatched delimiter at its line before collapsing.
        let blocks =
            find_blocks_with_context(&collapsed, context.seen_module_doc, guard_msgs_after)
                .map_err(|error| {
                    find_blocks_with_context(&normalized, context.seen_module_doc, guard_msgs_after)
                        .err()
                        .unwrap_or(error)
                })?;
        let (rewritten, changed) = rewrite_blocks(lines, &blocks, &mut |lines, block| {
            reflow_block(
                lines,
                block,
                target_width,
                options,
                rules,
                context,
                comment_layout,
            )
        })?;
        lines = rewritten;
        stats.comment_blocks_changed = changed;
    }

    let mut out = lines.join("\n");
    if !options.collapse_blank_lines {
        // lines()/join() otherwise loses one terminal blank line even when the
        // whitespace rule is disabled, including in partially reenabled regions.
        let tail = &source[source.trim_end_matches(['\r', '\n']).len()..];
        let newlines = tail.bytes().filter(|b| *b == b'\n').count();
        out.truncate(out.trim_end_matches('\n').len());
        out.extend(std::iter::repeat_n('\n', newlines));
    } else if source.ends_with('\n') {
        // Restore the final line ending that lines()/join() drops, so a
        // trailing blank line before a directive boundary is kept.
        out.push('\n');
    }
    Ok(NormalizeResult {
        output: out,
        stats,
        warnings,
    })
}

#[cfg(test)]
pub fn reflow_comments(
    source: &str,
    target_width: usize,
    spelling: SpellingDirection,
) -> Result<String, NormalizeError> {
    let options = Options {
        spelling: Some(spelling),
        ..Options::default()
    };
    normalize(source, target_width, options)
}

/// Collapse runs of consecutive blank lines to at most `max` blank lines,
/// anywhere in the file (inside comments or not). A line counts as blank when
/// it is empty or holds only spaces and tabs; outside doc comments the kept
/// line is emptied of its whitespace. Inside doc comments one blank line
/// always remains, since it separates Markdown blocks, and the code, math,
/// and HTML that Lean reads there are left alone.
fn collapse_blank_lines_configured(
    lines: &mut Vec<String>,
    max: usize,
    guard_msgs_after: bool,
) -> usize {
    let before = lines.len();
    let mut blanks = 0;
    let mut fence: Option<(char, usize)> = None;
    let source = lines.join("\n");
    let mut literal_lines = crate::syntax::literal_lines(&source);
    let mut doc_lines = std::collections::HashSet::new();
    if let Ok(blocks) = find_blocks_with_context(&source, false, guard_msgs_after) {
        for block in blocks {
            let range = block.start_line..=block.end_line;
            let protected = if block.verbatim {
                None
            } else {
                crate::prose::protected_lines(&lean_text(&extract_raw_content(lines, &block)))
            };
            match protected {
                Some(protected) => {
                    literal_lines.extend(protected.into_iter().map(|line| block.start_line + line));
                    doc_lines.extend(range);
                }
                None => literal_lines.extend(range),
            }
        }
    }
    let mut line_number = 0;
    let mut math = false;
    lines.retain_mut(|line| {
        let number = line_number;
        line_number += 1;
        if literal_lines.contains(&number) {
            blanks = 0;
            return true;
        }
        let in_doc = doc_lines.contains(&number);
        // Outside doc comments, whose Markdown is parsed above, protect code
        // fences and display math by their delimiter lines.
        let t = line.trim_start();
        if !in_doc && t.trim() == "$$" {
            math = !math;
            blanks = 0;
            return true;
        }
        if !in_doc && math {
            blanks = 0;
            return true;
        }
        let mark = t.chars().next().filter(|c| matches!(c, '`' | '~'));
        let count = mark.map_or(0, |c| t.chars().take_while(|x| *x == c).count());
        if !in_doc && count >= 3 {
            if let Some((c, n)) = fence {
                if Some(c) == mark && count >= n {
                    fence = None;
                }
            } else {
                fence = Some((mark.unwrap(), count));
            }
            blanks = 0;
            return true;
        }
        if !in_doc && fence.is_some() {
            return true;
        }
        if !is_blank(line) {
            blanks = 0;
            return true;
        }
        blanks += 1;
        if in_doc {
            blanks <= max.max(1)
        } else if blanks <= max {
            line.clear();
            true
        } else {
            false
        }
    });
    before - lines.len()
}

/// Capitalize the first letter of a word that follows a colon and whitespace
/// (e.g. `foo: bar` → `foo: Bar`). The word must begin with an ASCII letter;
/// a colon followed by anything else (a backtick, digit, punctuation) is left
/// alone. Text inside inline-code / fenced-code spans (delimited by backticks)
/// is never touched, so `` this: `should stay` `` is preserved verbatim.
fn capitalize_after_colon(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut in_code = false;
    let mut i = 0;

    while i < chars.len() {
        let c = chars[i];
        if c == '`' {
            in_code = !in_code;
            out.push(c);
            i += 1;
            continue;
        }
        if c == ':' && !in_code {
            out.push(c);
            i += 1;
            // Consume the run of whitespace after the colon.
            let ws_start = i;
            while i < chars.len() && chars[i].is_whitespace() {
                out.push(chars[i]);
                i += 1;
            }
            // Capitalize only when at least one space separated a word. A
            // single letter is usually a variable, as in `f: a → b`.
            if i > ws_start
                && i < chars.len()
                && chars[i].is_ascii_alphabetic()
                && chars.get(i + 1).is_some_and(|c| c.is_alphabetic())
            {
                out.push(chars[i].to_ascii_uppercase());
                i += 1;
            }
            continue;
        }
        out.push(c);
        i += 1;
    }

    out
}

// ---------------------------------------------------------------------------
// Heading banners (opt-in)
// ---------------------------------------------------------------------------

/// Convert `--` banner comments into Mathlib-style section headings:
///
/// ```text
/// -- ===========================================================================
/// -- The Beer-Quiche signaling game
/// -- ===========================================================================
/// ```
///
/// becomes
///
/// ```text
/// /-! ## The Beer-Quiche Signaling Game -/
/// ```
///
/// A banner is a rule line (`--` followed by a run of `=` or `-`), one or more
/// `--` text lines, and a closing rule line. The title is title-cased, with
/// small words (articles, short prepositions) and backtick code spans left
/// alone. Banners inside block comments are not touched.
fn convert_heading_banners_configured(
    source: &str,
    rules: &leanck_config::HeadingRules,
) -> (String, usize) {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut converted = 0usize;
    let mut cursor = LineCursor::new(source);
    let actual_comments: std::collections::HashSet<_> = crate::syntax::line_comments(source)
        .into_iter()
        .map(|r| cursor.position(r.start).0)
        .collect();
    let mut depth = 0usize;
    let mut i = 0;

    while i < lines.len() {
        if actual_comments.contains(&i)
            && let Some((title, end)) = parse_banner(&lines, i)
            && (i..=end).all(|line| actual_comments.contains(&line))
        {
            let indent = leading_whitespace(lines[i]);
            let title = rules.replacements.get(&title).cloned().unwrap_or(title);
            let title = match rules.case {
                HeadingCase::Off => title,
                HeadingCase::Title => title_case_with_words(&title, &rules.small_words),
                HeadingCase::Sentence => capitalize_segments_first(&title),
            };
            out.push(format!(
                "{indent}{}",
                rules
                    .template
                    .replace("{hashes}", &"#".repeat(usize::from(rules.level)))
                    .replace("{title}", &title)
            ));
            converted += 1;
            i = end + 1;
            continue;
        }
        depth = update_block_comment_depth(lines[i], depth);
        out.push(lines[i].to_string());
        i += 1;
    }

    let mut joined = out.join("\n");
    if source.ends_with('\n') {
        joined.push('\n');
    }
    (joined, converted)
}

/// Track nesting depth of `/- ... -/` block comments across a line. A `--`
/// outside any block comment starts a line comment, which makes the rest of
/// the line inert.
fn update_block_comment_depth(line: &str, mut depth: usize) -> usize {
    let bytes = line.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/-") {
            depth += 1;
            i += 2;
        } else if bytes[i..].starts_with(b"-/") {
            depth = depth.saturating_sub(1);
            i += 2;
        } else if depth == 0 && bytes[i..].starts_with(b"--") {
            break;
        } else {
            i += 1;
        }
    }
    depth
}

/// A rule line: `--` followed (after optional whitespace) by at least four of
/// the same character, all `=` or all `-`, and nothing else.
fn is_banner_rule(line: &str) -> bool {
    let Some(rest) = line.trim().strip_prefix("--") else {
        return false;
    };
    let rest = rest.trim();
    rest.chars().count() >= 4 && (rest.chars().all(|c| c == '=') || rest.chars().all(|c| c == '-'))
}

/// The title text of a `--` line inside a banner, with any markdown heading
/// markers stripped. Returns `None` for rule lines, blank comments, and lines
/// that are not `--` comments.
fn banner_text(line: &str) -> Option<String> {
    if is_banner_rule(line) {
        return None;
    }
    let rest = line.trim().strip_prefix("--")?;
    let text = rest.trim().trim_start_matches('#').trim_start();
    if text.is_empty() {
        return None;
    }
    Some(text.to_string())
}

/// Parse a banner starting at `start`. Returns the title-cased heading text
/// and the index of the closing rule line.
fn parse_banner(lines: &[&str], start: usize) -> Option<(String, usize)> {
    if !is_banner_rule(lines[start]) {
        return None;
    }
    let mut texts: Vec<String> = Vec::new();
    let mut i = start + 1;
    while i < lines.len()
        && let Some(text) = banner_text(lines[i])
    {
        texts.push(text);
        i += 1;
    }
    if texts.is_empty() || i >= lines.len() || !is_banner_rule(lines[i]) {
        return None;
    }
    let title = texts.join(" ");
    // A title containing comment delimiters would corrupt the generated block.
    if title.contains("/-") || title.contains(SUFFIX) {
        return None;
    }
    Some((title, i))
}

/// Whether a title word is a lone letter, such as the variable in `for x`.
fn is_single_letter(word: &str) -> bool {
    let mut letters = word.chars().filter(|c| c.is_alphanumeric());
    letters.next().is_some_and(char::is_alphabetic) && letters.next().is_none()
}

fn capitalize_segments_first(text: &str) -> String {
    // A leading `a` is more likely the article than a variable.
    if text
        .split_whitespace()
        .next()
        .is_some_and(|word| is_single_letter(word) && word != "a")
    {
        return text.to_owned();
    }
    let mut chars = text.chars();
    match chars.next() {
        Some(c) => c.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

/// Title-case each text part, leaving small words lowercase in the middle.
fn title_case_with_words(text: &str, small_words: &[String]) -> String {
    let titled = crate::prose::transform_with_offsets(text, |part, offset| {
        let leading = &part[..part.len() - part.trim_start().len()];
        let trailing = &part[part.trim_end().len()..];
        if part.trim().is_empty() {
            return part.to_owned();
        }
        format!(
            "{leading}{}{trailing}",
            title_case_plain(
                part.trim(),
                small_words,
                text[..offset].trim().is_empty(),
                text[offset + part.len()..].trim().is_empty()
            )
        )
    });
    titled.unwrap_or_else(|| text.to_owned())
}
fn title_case_plain(text: &str, small_words: &[String], at_start: bool, at_end: bool) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    let last = words.len().saturating_sub(1);
    let mut in_code = false;
    let mut out: Vec<String> = Vec::with_capacity(words.len());

    for (i, word) in words.iter().enumerate() {
        let was_in_code = in_code;
        if word.matches('`').count() % 2 == 1 {
            in_code = !in_code;
        }
        if was_in_code || word.contains('`') {
            out.push((*word).to_string());
            continue;
        }
        let small = small_words.iter().any(|s| s.eq_ignore_ascii_case(word));
        if (small && (i != 0 || !at_start) && (i != last || !at_end))
            || (!small && is_single_letter(word))
        {
            out.push((*word).to_string());
        } else {
            out.push(capitalize_segments(word));
        }
    }

    out.join(" ")
}

/// Capitalize the first letter of each hyphen-separated segment of `word`,
/// e.g. `beer-quiche` → `Beer-Quiche`.
fn capitalize_segments(word: &str) -> String {
    word.split('-')
        .map(|segment| {
            let mut chars = segment.chars();
            match chars.next() {
                Some(c) if c.is_ascii_lowercase() => {
                    let mut s = String::with_capacity(segment.len());
                    s.push(c.to_ascii_uppercase());
                    s.push_str(chars.as_str());
                    s
                }
                _ => segment.to_string(),
            }
        })
        .collect::<Vec<_>>()
        .join("-")
}

// ---------------------------------------------------------------------------
// Block detection
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct Block {
    /// 0-based line index where the opening delimiter appears.
    start_line: usize,
    /// Byte offset *after* the opening delimiter on `start_line`.
    start_after: usize,
    /// 0-based line index where the closing `-/` appears.
    end_line: usize,
    /// Byte offset where `-/` *starts* on `end_line`.
    end_before: usize,
    /// Whether the opening delimiter is `/-!` (doc comment).
    is_doc: bool,
    /// Whether this is the first `/-!` block in the file.
    first_doc: bool,
    /// Whether this is the module docstring: the first `/-!` block, unless
    /// it is a section heading.
    module_doc: bool,
    /// Whether this `/-!` block contains exactly one Markdown heading.
    heading_only: bool,
    /// Byte offset of the start delimiter on `start_line`.
    start_delim_col: usize,
    /// Byte offset of the end delimiter on `end_line`.
    end_delim_col: usize,
    /// Whether the text is exact output for `#guard_msgs`, which compares it
    /// verbatim apart from whitespace at line breaks, so no rule may touch it.
    verbatim: bool,
}

impl Block {
    /// The same block in a slice of lines starting at `first_line`.
    fn shifted(&self, first_line: usize) -> Self {
        Self {
            start_line: self.start_line - first_line,
            end_line: self.end_line - first_line,
            ..self.clone()
        }
    }

    /// This block after its lines were replaced by `lines`, which must keep
    /// the opening delimiter's position and the text after the closing one.
    fn rewritten(&self, lines: &[String], after: &str) -> Self {
        let end_line = lines.len() - 1;
        let end_delim_col = lines[end_line].len() - after.len() - SUFFIX.len();
        let mut block = Self {
            start_line: 0,
            end_line,
            end_before: end_delim_col,
            end_delim_col,
            ..self.clone()
        };
        if block.is_doc {
            let kind = doc_comment_kind(&extract_raw_content(lines, &block));
            block.heading_only = kind == DocCommentKind::Heading;
            block.module_doc = block.first_doc && kind == DocCommentKind::Documentation;
        }
        block
    }
}

/// Converts increasing byte offsets to (line, byte column) in linear total time.
struct LineCursor<'a> {
    source: &'a str,
    offset: usize,
    line: usize,
    line_start: usize,
}

impl<'a> LineCursor<'a> {
    fn new(source: &'a str) -> Self {
        Self {
            source,
            offset: 0,
            line: 0,
            line_start: 0,
        }
    }

    /// The position of `offset`, which must not precede the previous offset.
    fn position(&mut self, offset: usize) -> (usize, usize) {
        for (i, byte) in self.source.as_bytes()[self.offset..offset]
            .iter()
            .enumerate()
        {
            if *byte == b'\n' {
                self.line += 1;
                self.line_start = self.offset + i + 1;
            }
        }
        self.offset = offset;
        (self.line, offset - self.line_start)
    }
}

/// Whether only whitespace and line comments separate `offset` from a
/// `#guard_msgs` command, which `guard_msgs_after` tells for the end of
/// `source`.
pub(crate) fn precedes_guard_msgs(source: &str, mut offset: usize, guard_msgs_after: bool) -> bool {
    let bytes = source.as_bytes();
    loop {
        while bytes.get(offset).is_some_and(u8::is_ascii_whitespace) {
            offset += 1;
        }
        if offset >= bytes.len() {
            return guard_msgs_after;
        }
        if !bytes[offset..].starts_with(b"--") {
            return bytes[offset..].starts_with(b"#guard_msgs");
        }
        while bytes.get(offset).is_some_and(|b| *b != b'\n') {
            offset += 1;
        }
    }
}

/// Scan `source` for Lean block comments.  Returns an error if any delimiter
/// is unmatched. `guard_msgs_after` tells whether `#guard_msgs` follows the
/// end of `source`; see `precedes_guard_msgs`.
fn find_blocks_with_context(
    source: &str,
    mut seen: bool,
    guard_msgs_after: bool,
) -> Result<Vec<Block>, NormalizeError> {
    let bytes = source.as_bytes();
    let mut blocks = Vec::new();
    let mut i = 0;
    let mut cursor = LineCursor::new(source);
    while i < bytes.len() {
        if bytes[i..].starts_with(b"--") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if let Some(end) = crate::syntax::literal_end(source, i) {
            i = end;
            continue;
        }
        if !bytes[i..].starts_with(b"/-") {
            i += 1;
            continue;
        }
        let start = i;
        let doc = bytes[i..].starts_with(b"/-!");
        let recognized = doc || bytes[i..].starts_with(b"/--");
        // `block_end` scans for the end only after a third character, as Lean
        // does, so `/--/` does not close itself.
        let Some(end) = crate::syntax::block_end(source, start) else {
            if recognized {
                return Err(NormalizeError::UnmatchedOpeningDelimiter {
                    kind: if doc {
                        CommentKind::Doc
                    } else {
                        CommentKind::Block
                    },
                    line: cursor.position(start).0 + 1,
                });
            }
            break;
        };
        i = end - 2;
        if recognized {
            let (sl, sc) = cursor.position(start);
            let (el, ec) = cursor.position(i);
            let first_doc = doc && !seen;
            let kind = if doc {
                doc_comment_kind(&source[start + 3..i])
            } else {
                DocCommentKind::Documentation
            };
            let module_doc = first_doc && kind == DocCommentKind::Documentation;
            if doc {
                seen = true;
            }
            blocks.push(Block {
                start_line: sl,
                start_after: sc + 3,
                end_line: el,
                end_before: ec,
                is_doc: doc,
                first_doc,
                module_doc,
                heading_only: doc && kind == DocCommentKind::Heading,
                start_delim_col: sc,
                end_delim_col: ec,
                verbatim: !doc && precedes_guard_msgs(source, end, guard_msgs_after),
            });
        }
        i += 2;
    }
    Ok(blocks)
}

/// Classify a raw `/-!` body using the indentation Lean removes. Frontmatter
/// ordering and comment layout share this decision, including after a rewrite.
pub(crate) fn doc_comment_kind(body: &str) -> DocCommentKind {
    crate::prose::doc_comment_kind(&lean_text(body))
}

// ---------------------------------------------------------------------------
// Per-block reflow
// ---------------------------------------------------------------------------

/// A rewrite of one block's lines; `None` keeps them.
type Rewrite<'a> = dyn FnMut(&[String], &Block) -> Result<Option<Vec<String>>, NormalizeError> + 'a;

/// Replace each block's lines with `rewrite`'s result in linear total time.
/// Blocks that share a line are rewritten together, from the first to the
/// last, so a later block sees the final indentation of its opening line.
fn rewrite_blocks(
    mut lines: Vec<String>,
    blocks: &[Block],
    rewrite: &mut Rewrite<'_>,
) -> Result<(Vec<String>, usize), NormalizeError> {
    let mut out = Vec::with_capacity(lines.len());
    let mut changed = 0;
    let mut copied = 0;
    let mut next = 0;
    while next < blocks.len() {
        let group = next;
        let first_line = blocks[next].start_line;
        let mut last_line = blocks[next].end_line;
        next += 1;
        while next < blocks.len() && blocks[next].start_line <= last_line {
            last_line = last_line.max(blocks[next].end_line);
            next += 1;
        }
        out.extend(lines[copied..first_line].iter_mut().map(std::mem::take));
        let mut local: Vec<String> = lines[first_line..=last_line]
            .iter_mut()
            .map(std::mem::take)
            .collect();
        let mut group: Vec<Block> = blocks[group..next]
            .iter()
            .map(|block| block.shifted(first_line))
            .collect();
        if group.len() == 1 {
            changed += rewrite_group(&mut local, &mut group, rewrite)?;
        } else {
            // A block's last line also depends on the text after it, which
            // the next block's rewrite changes, so repeat until neither moves.
            let original = local.clone();
            let mut first_round = None;
            let mut settled = false;
            for _ in 0..SETTLE_LIMIT {
                let round = rewrite_group(&mut local, &mut group, rewrite)?;
                first_round.get_or_insert(round);
                if round == 0 {
                    settled = true;
                    break;
                }
            }
            if settled {
                changed += first_round.unwrap_or(0);
            } else {
                local = original;
            }
        }
        out.extend(local);
        copied = last_line + 1;
    }
    out.extend(lines.drain(copied..));
    Ok((out, changed))
}

/// Rewrite blocks that share lines once, from the first to the last, and
/// update their positions. Each later block opens on the previous block's
/// closing line, whose text after the delimiter a rewrite keeps verbatim.
fn rewrite_group(
    local: &mut Vec<String>,
    blocks: &mut [Block],
    rewrite: &mut Rewrite<'_>,
) -> Result<usize, NormalizeError> {
    let mut changed = 0;
    let mut added_lines = 0;
    let mut added_columns = 0;
    for block in blocks {
        block.start_line = block.start_line.saturating_add_signed(added_lines);
        block.end_line = block.end_line.saturating_add_signed(added_lines);
        block.start_delim_col = block.start_delim_col.saturating_add_signed(added_columns);
        block.start_after = block.start_after.saturating_add_signed(added_columns);
        if block.start_line == block.end_line {
            block.end_delim_col = block.end_delim_col.saturating_add_signed(added_columns);
            block.end_before = block.end_before.saturating_add_signed(added_columns);
        }
        let current = &local[block.start_line..=block.end_line];
        let (current_len, closing_len) = (current.len(), current[current.len() - 1].len());
        added_columns = 0;
        let Some((replacement, settled)) =
            settle(current, &block.shifted(block.start_line), rewrite)?
        else {
            continue;
        };
        added_lines += replacement.len() as isize - current_len as isize;
        added_columns = replacement[replacement.len() - 1].len() as isize - closing_len as isize;
        let start_line = block.start_line;
        *block = Block {
            start_line,
            end_line: start_line + settled.end_line,
            ..settled
        };
        local.splice(start_line..start_line + current_len, replacement);
        changed += 1;
    }
    Ok(changed)
}

/// The most rewrites of one block before giving up on reaching a fixed point.
const SETTLE_LIMIT: usize = 4;

/// Rewrite a block's lines until rewriting them again changes nothing, so a
/// second run leaves the result alone, and return them with the block's new
/// position. The block stays unchanged when its rewrite panics, does not
/// settle, or would change how Lean delimits it.
fn settle(
    original: &[String],
    block: &Block,
    rewrite: &mut Rewrite<'_>,
) -> Result<Option<(Vec<String>, Block)>, NormalizeError> {
    let after = original[original.len() - 1][block.end_delim_col + SUFFIX.len()..].to_owned();
    let mut current = original.to_vec();
    let mut current_block = block.clone();
    for round in 0..SETTLE_LIMIT {
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            rewrite(&current, &current_block)
        }));
        let next = match result {
            Ok(Ok(next)) => next,
            // Only the original text may report an error; a later round
            // failing means the first result is not stable.
            Ok(Err(error)) if round == 0 => return Err(error),
            Ok(Err(_)) | Err(_) => return Ok(None),
        };
        let Some(next) = next.filter(|next| *next != current) else {
            return Ok((current != original).then_some((current, current_block)));
        };
        if !keeps_delimiters(&current, &next, &current_block) {
            return Ok(None);
        }
        current_block = current_block.rewritten(&next, &after);
        current = next;
    }
    Ok(None)
}

/// Whether `replacement` keeps the block's surrounding text, its delimiters,
/// and the sequence of nested delimiters Lean finds in its body.
fn keeps_delimiters(current: &[String], replacement: &[String], block: &Block) -> bool {
    let original = current.join("\n");
    let rewritten = replacement.join("\n");
    let opening = &current[0][..block.start_after];
    let after = &current[current.len() - 1][block.end_delim_col + SUFFIX.len()..];
    let original_body = &original[block.start_after..original.len() - after.len() - SUFFIX.len()];
    let Some(body) = rewritten
        .strip_prefix(opening)
        .and_then(|rest| rest.strip_suffix(after))
        .and_then(|rest| rest.strip_suffix(SUFFIX))
    else {
        return false;
    };
    // Lean's scan continues through the closing delimiter, so a body ending
    // in `/` would turn it into an opening one.
    delimiter_tokens(&format!("{body}{SUFFIX}"))
        == delimiter_tokens(&format!("{original_body}{SUFFIX}"))
}

/// The `/-` (true) and `-/` (false) delimiters in `text`, scanned as Lean does.
fn delimiter_tokens(text: &str) -> Vec<bool> {
    let bytes = text.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/-") {
            tokens.push(true);
            i += 2;
        } else if bytes[i..].starts_with(b"-/") {
            tokens.push(false);
            i += 2;
        } else {
            i += 1;
        }
    }
    tokens
}

/// The replacement for a block's lines, or `None` to keep them unchanged.
fn reflow_block(
    lines: &[String],
    block: &Block,
    target_width: NormalizeWidth,
    options: Options,
    rules: &NormalizeRules,
    context: &Context<'_>,
    comment_layout: bool,
) -> Result<Option<Vec<String>>, NormalizeError> {
    if block.verbatim {
        return Ok(None);
    }
    let prefix = if block.is_doc { "/-!" } else { "/--" };
    let style = if !comment_layout {
        DelimiterStyle::Off
    } else if block.heading_only {
        rules.comments.heading.unwrap_or(rules.comments.section)
    } else if block.module_doc {
        rules.comments.module
    } else if block.is_doc {
        rules.comments.section
    } else {
        rules.comments.declaration
    };

    // With layout and wrapping disabled, apply prose edits directly to the raw
    // comment body so indentation, blank lines, and delimiter spacing stay exact.
    let prose_only = style == DelimiterStyle::Off && !options.comment_reflow;
    let original_content = if prose_only {
        extract_raw_content(lines, block)
    } else if !options.comment_reflow {
        // Delimiter-only lines and their padding are not Markdown content. The
        // reflow dependency normally removes these; layout alone must do so too.
        trim_delimiter_padding(&extract_content(lines, block))
    } else {
        extract_content(lines, block)
    };
    // The rules below read the body without the space after the delimiter.
    // Leave the rare comments where that space changes Lean's Markdown, such
    // as list nesting under a first-line item.
    if !prose_only
        && !crate::prose::same_structure(
            &lean_text(&extract_raw_content(lines, block)),
            &original_content,
        )
    {
        return Ok(None);
    }
    let mut content = original_content.clone();
    if options.headings {
        let titled = crate::prose::heading_titles(&content, |title| {
            let title = rules
                .headings
                .replacements
                .get(title)
                .map(String::as_str)
                .unwrap_or(title);
            match rules.headings.case {
                HeadingCase::Off => title.to_owned(),
                HeadingCase::Title => title_case_with_words(title, &rules.headings.small_words),
                HeadingCase::Sentence => crate::prose::transform(title, capitalize_segments_first)
                    .unwrap_or_else(|| title.to_owned()),
            }
        });
        let Some(titled) = titled else {
            return Ok(None);
        };
        content = titled;
    }
    let edited = crate::prose::transform(&content, |text| {
        let mut text = text.to_owned();
        if let Some(spelling) = options.spelling {
            text = if let Some(dictionary) = context.dictionary {
                dictionary.correct(&text)
            } else {
                correct_spelling(&text, spelling)
            };
        }
        if options.capitalize_after_colon {
            text = capitalize_after_colon(&text);
        }
        text
    });
    let Some(edited) = edited else {
        return Ok(None);
    };
    content = edited.clone();
    if !options.comment_reflow && options.linkify_urls {
        // Without reflow, linkification only adds link syntax around URLs.
        let Some(linked) = crate::prose::linkify(&content) else {
            return Ok(None);
        };
        content = linked;
    }
    if prose_only {
        if content == original_content {
            return Ok(None);
        }
        let opening = &lines[block.start_line][..block.start_after];
        let closing = &lines[block.end_line][block.end_before..];
        let replacement = format!("{opening}{content}{closing}");
        return Ok(Some(replacement.split('\n').map(str::to_owned).collect()));
    }
    if content.split('\n').all(is_blank) {
        return Ok(None);
    }

    // Continuation lines use the indentation of the line holding the opening
    // delimiter, even after code such as `@[to_dual /--`, which is exactly what
    // Lean's dedent removes again.
    let indent = &lines[block.start_line][..indentation(&lines[block.start_line])];
    let indent_w = char_width(indent);
    let before = &lines[block.start_line][..block.start_delim_col];
    let after = &lines[block.end_line][block.end_delim_col + SUFFIX.len()..];
    let layout_width = if options.comment_reflow {
        target_width
    } else {
        NormalizeWidth::Unlimited
    };
    let separate = style == DelimiterStyle::Separate
        || (style == DelimiterStyle::Off
            && is_blank(&lines[block.start_line][block.start_after..])
            && is_blank(&lines[block.end_line][..block.end_before]));
    let budget = |used: usize| match target_width {
        NormalizeWidth::Finite(width) => ReflowWidth::Finite(width.saturating_sub(used).max(1)),
        NormalizeWidth::Unlimited => ReflowWidth::Infinite,
    };
    let widths = if separate {
        // Delimiters on their own lines leave every content line the same width.
        let width = budget(indent_w);
        EdgeWidths {
            first: width,
            middle: width,
            last: width,
            single: width,
        }
    } else {
        // Reserve space for delimiters only on the lines where they appear:
        // `/-- ` on the first line, ` -/` on the last line, and both when
        // the content remains on a single line.
        let opening_w = char_width(before) + OPENING_OVERHEAD;
        let closing_w = 1 + char_width(SUFFIX) + char_width(after);
        EdgeWidths {
            first: budget(opening_w),
            middle: budget(indent_w),
            last: budget(indent_w + closing_w),
            single: budget(opening_w + closing_w),
        }
    };
    let reflowed;
    let content_ref = if options.comment_reflow {
        let result = crate::prose::reflow_protected(
            &content,
            ReflowOptions {
                widths,
                linkify_urls: options.linkify_urls,
                frontmatter: false,
            },
        )
        .map_err(NormalizeError::Reflow)?;
        let Some(result) = result else {
            return Ok(None);
        };
        reflowed = result;
        reflowed.trim_end()
    } else {
        content.trim_end()
    };
    let content_lines: Vec<&str> = content_ref.lines().collect();
    if content_lines.iter().all(|line| is_blank(line)) {
        return Ok(None);
    }

    let replacement = if separate {
        expand_separate(before, after, &content_lines, prefix, indent)
    } else if style == DelimiterStyle::Off {
        expand_preserving(lines, block, &content_lines, prefix, indent)
    } else if let Some(line) = (block.start_line == block.end_line && content_lines.len() == 1)
        .then(|| single_line(before, after, content_lines[0], prefix, layout_width))
        .flatten()
    {
        // The original was single-line and the content still is: keep the
        // true single-line format when it fits.
        vec![line]
    } else {
        expand_compact(before, after, &content_lines, prefix, indent, layout_width)
    };
    // Layout and wrapping must not change the Markdown structure Lean reads.
    let joined = replacement.join("\n");
    let body = joined.get(block.start_after..joined.len() - after.len() - SUFFIX.len());
    if !body.is_some_and(|body| crate::prose::same_structure(&edited, &lean_text(body))) {
        return Ok(None);
    }
    Ok(Some(replacement))
}

// ---------------------------------------------------------------------------
// Content extraction
// ---------------------------------------------------------------------------

/// The comment body as Lean reads it: `String.removeLeadingSpaces` removes the
/// smallest indentation of the later non-blank lines from every line.
fn lean_text(raw: &str) -> String {
    dedent(raw, 0)
}

/// The comment body as Lean reads it, without the space separating the first
/// line from the delimiter, which is layout rather than content.
fn extract_content(lines: &[String], block: &Block) -> String {
    dedent(&extract_raw_content(lines, block), 1)
}

/// Lean's dedent, removing at least `first_min` columns from the first line.
fn dedent(raw: &str, first_min: usize) -> String {
    let parts: Vec<&str> = raw.split('\n').collect();
    let dedent = parts[1..]
        .iter()
        .filter(|line| line.contains(|c| !matches!(c, ' ' | '\t')))
        .map(|line| indentation(line))
        .min()
        .unwrap_or(usize::MAX);
    // Only ASCII spaces and tabs count, so these byte offsets are boundaries.
    fn strip(line: &str, n: usize) -> &str {
        &line[indentation(line).min(n)..]
    }
    let mut out = vec![strip(parts[0], dedent.max(first_min))];
    out.extend(parts[1..].iter().map(|line| strip(line, dedent)));
    out.join("\n")
}

fn trim_delimiter_padding(content: &str) -> String {
    let mut lines: Vec<_> = content.lines().collect();
    let first = lines.iter().position(|line| !is_blank(line));
    let last = lines.iter().rposition(|line| !is_blank(line));
    let (Some(first), Some(last)) = (first, last) else {
        return String::new();
    };
    lines[last] = lines[last].trim_end_matches([' ', '\t']);
    lines[first..=last].join("\n")
}

fn extract_raw_content(lines: &[String], block: &Block) -> String {
    let mut parts: Vec<&str> = Vec::new();

    for (offset, line) in lines[block.start_line..=block.end_line].iter().enumerate() {
        let li = block.start_line + offset;
        let start = if li == block.start_line {
            block.start_after
        } else {
            0
        };
        let end = if li == block.end_line {
            block.end_before
        } else {
            line.len()
        };
        parts.push(&line[start..end]);
    }

    parts.join("\n")
}

// ---------------------------------------------------------------------------
// Layouts
// ---------------------------------------------------------------------------

/// Both delimiters and the content on one line, when it fits.
fn single_line(
    before: &str,
    after: &str,
    content: &str,
    prefix: &str,
    target_width: NormalizeWidth,
) -> Option<String> {
    let candidate = format!("{before}{prefix} {content} {SUFFIX}{after}");
    target_width
        .fits(char_width(&candidate))
        .then_some(candidate)
}

/// Delimiters on their own lines, as for module docstrings.
fn expand_separate(
    before: &str,
    after: &str,
    content_lines: &[&str],
    prefix: &str,
    indent: &str,
) -> Vec<String> {
    let mut out = vec![format!("{before}{prefix}")];
    out.extend(content_lines.iter().map(|line| indented(indent, line)));
    out.push(format!("{indent}{SUFFIX}{after}"));
    out
}

/// Delimiters share lines with content; no orphan `-/` unless the last line
/// must keep its own line.
fn expand_compact(
    before: &str,
    after: &str,
    content_lines: &[&str],
    prefix: &str,
    indent: &str,
    target_width: NormalizeWidth,
) -> Vec<String> {
    if let [content] = content_lines {
        // Only one content line ─ put both delimiters on it.
        return vec![format!("{before}{prefix} {content} {SUFFIX}{after}")];
    }
    let first = content_lines[0];
    let last = content_lines[content_lines.len() - 1];
    let mut out = Vec::with_capacity(content_lines.len() + 2);

    // --- opening delimiter --------------------------------------------------
    let first_with_prefix = format!("{before}{prefix} {first}");
    if shares_opening_line(content_lines) && target_width.fits(char_width(&first_with_prefix)) {
        out.push(first_with_prefix);
    } else {
        // Doesn't fit or would change the content — opening delimiter on its
        // own line, and the first content line becomes a continuation line.
        out.push(format!("{before}{prefix}"));
        out.push(indented(indent, first));
    }

    // --- middle lines (if any) ----------------------------------------------
    for line in &content_lines[1..content_lines.len() - 1] {
        out.push(indented(indent, line));
    }

    // --- closing delimiter --------------------------------------------------
    if shares_closing_line(last) {
        out.push(format!("{indent}{last} {SUFFIX}{after}"));
    } else {
        out.push(indented(indent, last));
        out.push(format!("{indent}{SUFFIX}{after}"));
    }
    out
}

/// Keep the original choice of delimiter placement.
fn expand_preserving(
    lines: &[String],
    block: &Block,
    content: &[&str],
    prefix: &str,
    indent: &str,
) -> Vec<String> {
    let before = &lines[block.start_line][..block.start_delim_col];
    let after = &lines[block.end_line][block.end_delim_col + SUFFIX.len()..];
    let mut content = content.to_vec();
    while content.first().is_some_and(|s| is_blank(s)) {
        content.remove(0);
    }
    while content.last().is_some_and(|s| is_blank(s)) {
        content.pop();
    }
    let open_separate =
        is_blank(&lines[block.start_line][block.start_after..]) || !shares_opening_line(&content);
    let close_separate = is_blank(&lines[block.end_line][..block.end_before])
        || content
            .last()
            .is_some_and(|last| !shares_closing_line(last));
    let mut out = Vec::new();
    if open_separate {
        out.push(format!("{before}{prefix}"));
    }
    for (i, line) in content.iter().enumerate() {
        if i == 0 && !open_separate {
            out.push(format!("{before}{prefix} {line}"));
        } else {
            out.push(indented(indent, line));
        }
    }
    if close_separate {
        out.push(format!("{indent}{SUFFIX}{after}"));
    } else if let Some(last) = out.last_mut() {
        last.push_str(&format!(" {SUFFIX}{after}"));
    }
    out
}

/// Whether the first content line can follow the opening delimiter without
/// changing what Lean reads. Lean removes the smallest indentation of the
/// later non-blank lines from every line, so one of them must be unindented.
/// The first line gains the separating space, which moves a list item's
/// content column, so the item's indented lines would no longer belong to it.
/// Fences (Verso requires their columns) and table rows keep their own lines.
fn shares_opening_line(content: &[&str]) -> bool {
    let Some((first, rest)) = content.split_first() else {
        return false;
    };
    let indents: Vec<usize> = rest
        .iter()
        .filter(|line| !is_blank(line))
        .map(|line| indentation(line))
        .collect();
    !first.starts_with([' ', '\t'])
        && !is_table_row(first)
        && !is_fence(first)
        && indents.iter().min().is_none_or(|min| *min == 0)
        && !(starts_list_item(first) && indents.iter().any(|indent| *indent > 0))
}

/// Whether the closing delimiter can follow the last content line.
fn shares_closing_line(last: &str) -> bool {
    !is_table_row(last) && !is_fence(last)
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A markdown table row: the trimmed line starts and ends with a pipe.
fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 2 && t.starts_with('|') && t.ends_with('|')
}

/// A code or math fence line.
fn is_fence(line: &str) -> bool {
    let t = line.trim_start();
    t.starts_with("```") || t.starts_with("~~~") || t.starts_with("$$")
}

/// A line starting with a bullet or ordered list marker.
fn starts_list_item(line: &str) -> bool {
    let t = line.trim_start();
    let rest = if let Some(rest) = t.strip_prefix(['*', '-', '+']) {
        rest
    } else {
        let digits = t.len() - t.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        match t[digits..].strip_prefix(['.', ')']) {
            Some(rest) if (1..=9).contains(&digits) => rest,
            _ => return false,
        }
    };
    rest.is_empty() || rest.starts_with([' ', '\t'])
}

/// A line of only ASCII spaces and tabs (and a stray carriage return), the
/// whitespace Lean and Markdown agree on; other Unicode spaces are content.
fn is_blank(line: &str) -> bool {
    line.bytes().all(|b| matches!(b, b' ' | b'\t' | b'\r'))
}

/// The number of leading ASCII spaces and tabs, which Lean counts alike.
fn indentation(line: &str) -> usize {
    line.len() - line.trim_start_matches([' ', '\t']).len()
}

fn indented(indent: &str, line: &str) -> String {
    if is_blank(line) {
        String::new()
    } else {
        format!("{indent}{line}")
    }
}

fn leading_whitespace(s: &str) -> &str {
    let end = s
        .char_indices()
        .take_while(|(_, c)| c.is_whitespace())
        .last()
        .map(|(i, c)| i + c.len_utf8())
        .unwrap_or(0);
    &s[..end]
}

fn char_width(s: &str) -> usize {
    s.chars().count()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // Error cases
    // ------------------------------------------------------------------

    #[test]
    fn unmatched_start_errors() {
        assert!(
            reflow_comments(
                "/-- no end\ncode\n",
                100,
                SpellingDirection::BritishToAmerican
            )
            .is_err()
        );
    }

    #[test]
    fn normalizes_standalone_simp_attributes_by_default() {
        let out = normalize(
            "@[simp]\nlemma my_simp_lemma : true := trivial\n",
            100,
            Options::default(),
        )
        .unwrap();
        assert_eq!(out, "@[simp] lemma my_simp_lemma : true := trivial\n");
    }

    #[test]
    fn can_disable_attribute_normalization() {
        let options = Options {
            attributes: false,
            ..Options::default()
        };
        let src = "@[simp]\nlemma my_simp_lemma : true := trivial\n";
        let out = normalize(src, 100, options).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn collapses_consecutive_blank_lines() {
        let out = reflow_comments(
            "def a := 1\n\n\n\ndef b := 2\n",
            100,
            SpellingDirection::BritishToAmerican,
        )
        .unwrap();
        assert_eq!(out, "def a := 1\n\ndef b := 2\n");
    }

    #[test]
    fn collapses_whitespace_only_lines() {
        let out = reflow_comments(
            "def a := 1\n   \n\t\ndef b := 2\n",
            100,
            SpellingDirection::BritishToAmerican,
        )
        .unwrap();
        assert_eq!(out, "def a := 1\n\ndef b := 2\n");
    }

    #[test]
    fn collapses_blank_lines_inside_comments() {
        let out = reflow_comments(
            "/-- one\n\n\n\ntwo -/\n",
            100,
            SpellingDirection::BritishToAmerican,
        )
        .unwrap();
        assert_eq!(out, "/-- one\n\ntwo -/\n");
    }

    // ------------------------------------------------------------------
    // Heading banners (opt-in)
    // ------------------------------------------------------------------

    fn with_headings() -> Options {
        Options {
            headings: true,
            ..Options::default()
        }
    }

    #[test]
    fn heading_banner_converts_to_doc_heading() {
        let src = "\
-- ===========================================================================
-- The Beer-Quiche signaling game
-- ===========================================================================
def x := 1
";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(
            out,
            "/-! ## The Beer-Quiche Signaling Game -/\ndef x := 1\n"
        );
    }

    #[test]
    fn heading_banners_are_left_alone_by_default() {
        let src = "\
-- ====
-- A heading
-- ====
def x := 1
";
        let out = normalize(src, 100, Options::default()).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn dash_rule_banner_converts() {
        let src = "-- ---------------\n-- helper lemmas\n-- ---------------\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, "/-! ## Helper Lemmas -/\n");
    }

    #[test]
    fn banner_title_keeps_small_words_lowercase() {
        let src = "-- ====\n-- the expectation of the gamma distribution\n-- ====\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, "/-! ## The Expectation of the Gamma Distribution -/\n");
    }

    #[test]
    fn banner_title_skips_backtick_code() {
        let src = "-- ====\n-- lemmas about `foo bar` and baz\n-- ====\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, "/-! ## Lemmas About `foo bar` and Baz -/\n");
    }

    #[test]
    fn multi_line_banner_title_is_joined() {
        let src = "-- ====\n-- first part\n-- second part\n-- ====\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, "/-! ## First Part Second Part -/\n");
    }

    #[test]
    fn banner_preserves_indentation() {
        let src = "  -- ====\n  -- inner section\n  -- ====\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, "  /-! ## Inner Section -/\n");
    }

    #[test]
    fn banner_strips_existing_heading_markers() {
        let src = "-- ====\n-- ## already marked\n-- ====\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, "/-! ## Already Marked -/\n");
    }

    #[test]
    fn unclosed_banner_is_left_alone() {
        let src = "-- ====\n-- not a heading\ndef x := 1\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn rule_without_text_is_left_alone() {
        let src = "-- ====\n-- ====\ndef x := 1\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn banner_inside_block_comment_is_left_alone() {
        let src = "/-\n-- ====\n-- not converted\n-- ====\n-/\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn banner_title_with_delimiter_is_left_alone() {
        let src = "-- ====\n-- contains -/ delimiter\n-- ====\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn converted_heading_is_not_module_docstring() {
        // Even as the first /-! in the file, a converted heading stays on one
        // line; the next plain /-! is not promoted to module docstring either.
        let src = "-- ====\n-- a heading\n-- ====\n\n/-! prose doc -/\n";
        let out = normalize(src, 100, with_headings()).unwrap();
        assert_eq!(out, "/-! ## A Heading -/\n\n/-! prose doc -/\n");
    }

    #[test]
    fn converted_heading_is_idempotent() {
        let src = "-- ====\n-- the beer-quiche signaling game\n-- ====\n";
        let once = normalize(src, 100, with_headings()).unwrap();
        let twice = normalize(&once, 100, with_headings()).unwrap();
        assert_eq!(once, twice);
    }

    #[test]
    fn capitalize_after_colon_basic() {
        assert_eq!(
            capitalize_after_colon("This is: not okay"),
            "This is: Not okay"
        );
    }

    #[test]
    fn capitalize_after_colon_already_upper() {
        assert_eq!(
            capitalize_after_colon("This is: The correct style"),
            "This is: The correct style"
        );
    }

    #[test]
    fn capitalize_after_colon_skips_code() {
        // The token after the colon is a backtick, so nothing is capitalized,
        // and the code span itself is untouched.
        assert_eq!(
            capitalize_after_colon("But this: `should stay as is`"),
            "But this: `should stay as is`"
        );
        // A colon inside a code span is also left alone.
        assert_eq!(capitalize_after_colon("`key: value`"), "`key: value`");
    }

    #[test]
    fn capitalize_after_colon_requires_space_and_letter() {
        assert_eq!(capitalize_after_colon("ratio a:b"), "ratio a:b");
        assert_eq!(capitalize_after_colon("count: 3 items"), "count: 3 items");
        assert_eq!(
            capitalize_after_colon("over newline:\nword"),
            "over newline:\nWord"
        );
    }

    #[test]
    fn capitalize_after_colon_is_opt_in() {
        let source = "/-- This is: not okay -/\n";
        assert_eq!(normalize(source, 100, Options::default()).unwrap(), source);
        let options = Options {
            capitalize_after_colon: true,
            ..Default::default()
        };
        assert_eq!(
            normalize(source, 100, options).unwrap(),
            "/-- This is: Not okay -/\n"
        );
    }

    #[test]
    fn stray_closing_is_harmless() {
        let out =
            reflow_comments("code -/ more\n", 100, SpellingDirection::BritishToAmerican).unwrap();
        assert_eq!(out, "code -/ more\n");
    }

    // ------------------------------------------------------------------
    // Single-line comments
    // ------------------------------------------------------------------

    #[test]
    fn single_line_fits() {
        let out = reflow_comments(
            "  /-- short -/\n",
            100,
            SpellingDirection::BritishToAmerican,
        )
        .unwrap();
        assert_eq!(out, "  /-- short -/\n");
    }

    #[test]
    fn empty_comment_unchanged() {
        let out = reflow_comments("/---/\n", 100, SpellingDirection::BritishToAmerican).unwrap();
        assert_eq!(out, "/---/\n");
    }

    #[test]
    fn preserves_code_around_single_line() {
        let out = reflow_comments(
            "def foo := /-- comment -/ 42\n",
            100,
            SpellingDirection::BritishToAmerican,
        )
        .unwrap();
        assert_eq!(out, "def foo := /-- comment -/ 42\n");
    }

    #[test]
    fn multiple_single_line_comments() {
        let out = reflow_comments(
            "/-- first -/\n/-- second -/\n",
            100,
            SpellingDirection::BritishToAmerican,
        )
        .unwrap();
        assert_eq!(out, "/-- first -/\n/-- second -/\n");
    }

    // ------------------------------------------------------------------
    // Compact expansion (normal comments)
    // ------------------------------------------------------------------

    #[test]
    fn single_line_expands_compact() {
        let src = "/-- this is a very long comment that will not fit on a single line -/\n";
        let out = reflow_comments(src, 40, SpellingDirection::BritishToAmerican).unwrap();
        // Should be multi-line with delimiters on content lines.
        let lines: Vec<&str> = out.lines().collect();
        assert!(lines.len() >= 2, "expected multiple lines, got: '{out}'");
        // Opening delimiter shares first content line.
        assert!(
            lines[0].starts_with("/-- "),
            "first line should start with '/-- <content>': '{}'",
            lines[0]
        );
        // Closing delimiter is on the last content line — never orphaned.
        let last = lines.last().unwrap();
        assert!(
            last.ends_with(" -/"),
            "last line should end with ' -/', got: '{last}'"
        );
        // No line exceeds the target width.
        for line in &lines {
            assert!(
                line.chars().count() <= 40,
                "line too wide ({w}): '{line}'",
                w = line.chars().count()
            );
        }
    }

    #[test]
    fn multi_line_reflow_compact() {
        let src = "/--\nthis is a long paragraph that needs to be wrapped at a certain width\n-/\n";
        let out = reflow_comments(src, 30, SpellingDirection::BritishToAmerican).unwrap();
        for line in out.lines() {
            assert!(line.chars().count() <= 30, "line too wide: '{line}'");
        }
        // Closing delimiter should be on the last content line.
        let last = out.lines().last().unwrap();
        assert!(
            last.contains(" -/"),
            "expected ' -/' on last line: '{last}'"
        );
    }

    #[test]
    fn continuation_lines_use_full_width_without_opening_delimiter_overhead() {
        let src = "/-- aaaaaaaaa bbbbbbbbb ccccccccc ddddddddd -/\n";
        let out = reflow_comments(src, 20, SpellingDirection::BritishToAmerican).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "/-- aaaaaaaaa");
        assert_eq!(lines[1], "bbbbbbbbb ccccccccc");
        assert_eq!(lines[2], "ddddddddd -/");
    }

    #[test]
    fn closing_delimiter_overhead_is_reserved_on_last_line() {
        let src = "/-- aaaaaaaaa bbbbbbbbb ccccccccc -/\n";
        let out = reflow_comments(src, 20, SpellingDirection::BritishToAmerican).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "/-- aaaaaaaaa");
        assert_eq!(lines[1], "bbbbbbbbb");
        assert_eq!(lines[2], "ccccccccc -/");
    }

    #[test]
    fn single_line_reflow_reserves_both_delimiters() {
        let src = "/-- aaaaaaaaa bbbb -/\n";
        let out = reflow_comments(src, 20, SpellingDirection::BritishToAmerican).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "/-- aaaaaaaaa");
        assert_eq!(lines[1], "bbbb -/");
    }

    #[test]
    fn inner_delimiter_preserved() {
        let out = reflow_comments(
            "/-- outer /-- text -/\n",
            100,
            SpellingDirection::BritishToAmerican,
        )
        .unwrap();
        assert!(out.contains("/--"), "inner /-- should be preserved as text");
        assert!(out.contains("-/"));
    }

    #[test]
    fn expansion_preserves_code_before() {
        let src = "  def foo := /-- a very long comment that definitely needs to be expanded across multiple lines -/\n";
        let out = reflow_comments(src, 50, SpellingDirection::BritishToAmerican).unwrap();
        assert!(out.starts_with("  def foo := /--"));
        assert!(out.trim_end().ends_with("-/"));
    }

    // ------------------------------------------------------------------
    // Module docstring (first /-! — delimiters always on own lines)
    // ------------------------------------------------------------------

    #[test]
    fn module_doc_has_delimiters_on_own_lines() {
        let src = "/-!\nThis is the module docstring that should have its delimiters on separate lines\n-/\n";
        let out = reflow_comments(src, 40, SpellingDirection::BritishToAmerican).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "/-!");
        assert_eq!(lines.last().unwrap(), &"-/");
    }

    #[test]
    fn module_doc_single_line_expands_to_separate_delimiters() {
        let src = "/-! Module doc that is too long to fit on one line so it must expand -/\n";
        let out = reflow_comments(src, 50, SpellingDirection::BritishToAmerican).unwrap();
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines[0], "/-!");
        assert_eq!(lines.last().unwrap(), &"-/");
        // Content lines are between.
        assert!(lines.len() >= 3);
    }

    #[test]
    fn second_doc_comment_is_compact() {
        // The second /-! should use compact formatting.
        let src = "/-! module doc -/\n\n/-! This is another doc comment that is very long and should use compact formatting with delimiters on content lines -/\n";
        let out = reflow_comments(src, 50, SpellingDirection::BritishToAmerican).unwrap();
        // After the first /-! (module doc, delimiters on own lines), the
        // second /-! should be compact.
        let lines: Vec<&str> = out.lines().collect();
        // Find the second comment by looking for lines after the first -/
        let first_close = lines.iter().position(|l| *l == "-/").unwrap();
        let after_first = &lines[first_close + 1..];
        // The second comment's opening line should start with "/-! ".
        let second_open = after_first
            .iter()
            .find(|l| l.starts_with("/-! "))
            .expect("second /-! should use compact format with '/-! <content>'");
        assert!(second_open.len() > 4);
    }

    // ------------------------------------------------------------------
    // Doc comments (/-!) that are not module docs
    // ------------------------------------------------------------------

    #[test]
    fn doc_comment_single_line_fits() {
        // First /-! is always a module docstring — delimiters on own lines.
        let out = reflow_comments(
            "/-! doc comment -/\n",
            100,
            SpellingDirection::BritishToAmerican,
        )
        .unwrap();
        assert_eq!(out, "/-!\ndoc comment\n-/\n");
    }

    #[test]
    fn doc_comment_expands_compact() {
        // Not the first /-! in the file, so should be compact.
        let src = "/-! first -/\n/-! This is a long doc comment that needs expansion -/\n";
        let out = reflow_comments(src, 40, SpellingDirection::BritishToAmerican).unwrap();
        // The second comment should have compact formatting.
        let lines: Vec<&str> = out.lines().collect();
        let compact_open = lines
            .iter()
            .find(|l| l.starts_with("/-! ") && l.len() > 4)
            .expect("should have compact /-! line");
        assert!(compact_open.starts_with("/-! "));
    }

    // ------------------------------------------------------------------
    // Indentation handling
    // ------------------------------------------------------------------

    #[test]
    fn indented_block_comment() {
        let src = "    /--\n    this is indented and needs reflowing at a narrow width\n    -/\n";
        let out = reflow_comments(src, 30, SpellingDirection::BritishToAmerican).unwrap();
        for line in out.lines() {
            if !line.trim().is_empty() {
                assert!(
                    line.starts_with("    "),
                    "expected 4-space indent: '{line}'"
                );
            }
            assert!(line.chars().count() <= 30, "line too wide: '{line}'");
        }
    }

    #[test]
    fn no_orphan_closing_delimiter() {
        // At a width that forces wrapping, the -/ must never sit alone.
        let src = "/-- a b c d e f g h i j k l m n o p q r s t u v w x y z -/\n";
        let out = reflow_comments(src, 20, SpellingDirection::BritishToAmerican).unwrap();
        for line in out.lines() {
            if line.contains("-/") {
                // The -/ must share the line with content (not be the only thing).
                let trimmed = line.trim();
                assert!(trimmed.len() > 2, "orphan -/ detected: '{line}'");
                assert!(
                    trimmed.starts_with(|c: char| c.is_alphanumeric()) || trimmed.starts_with('/'),
                    "-/ line should have content before it: '{line}'"
                );
            }
        }
    }

    // ------------------------------------------------------------------
    // Regular /-- near module docstring
    // ------------------------------------------------------------------

    #[test]
    fn regular_comment_before_module_doc() {
        // A /-- before the first /-! should not inherit module-doc behavior.
        // The first /-! (module docstring) gets delimiters on own lines.
        let src = "/-- regular comment -/\n/-! module docstring -/\n";
        let out = reflow_comments(src, 100, SpellingDirection::BritishToAmerican).unwrap();
        assert_eq!(out, "/-- regular comment -/\n/-!\nmodule docstring\n-/\n");
    }

    #[test]
    fn bullet_list_preserves_continuation_indent() {
        // A bullet list inside a comment should keep its continuation indent.
        let src = "/-- * this is a long bullet point that wraps across multiple lines of text -/\n";
        let out = reflow_comments(src, 30, SpellingDirection::BritishToAmerican).unwrap();
        for line in out.lines() {
            assert!(line.chars().count() <= 30, "line too wide: '{line}'");
        }
        // Continuation lines should be indented (2 spaces for the bullet).
        let has_continuation = out
            .lines()
            .any(|l| l.starts_with("  ") && !l.starts_with("  /--"));
        assert!(
            has_continuation,
            "expected bullet continuation indent, got:\n{out}"
        );
    }

    #[test]
    fn bullet_list_in_module_doc_preserves_indent() {
        let src =
            "/-!\n* this is a long bullet point that wraps across multiple lines of text\n-/\n";
        let out = reflow_comments(src, 30, SpellingDirection::BritishToAmerican).unwrap();
        for line in out.lines() {
            assert!(line.chars().count() <= 30, "line too wide: '{line}'");
        }
        // Continuation lines should have the bullet indent preserved.
        let has_continuation = out
            .lines()
            .any(|l| l.starts_with("  ") && l != "/-!" && l != "-/");
        assert!(
            has_continuation,
            "expected bullet continuation indent, got:\n{out}"
        );
    }

    // ------------------------------------------------------------------
    // URL linkification (opt-in)
    // ------------------------------------------------------------------

    #[test]
    fn bare_urls_are_left_alone_by_default() {
        let src = "/-- See https://example.com/docs for details. -/\n";
        let out = normalize(src, 100, Options::default()).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn bare_urls_become_links_with_option() {
        let options = Options {
            linkify_urls: true,
            ..Options::default()
        };
        let src = "/-- See https://example.com/docs for details. -/\n";
        let out = normalize(src, 100, options).unwrap();
        assert_eq!(
            out,
            "/-- See [https://example.com/docs](https://example.com/docs) for details. -/\n"
        );
    }

    #[test]
    fn bare_urls_in_module_doc_become_links_with_option() {
        let options = Options {
            linkify_urls: true,
            ..Options::default()
        };
        let src = "/-!\nSee https://example.com for details.\n-/\n";
        let out = normalize(src, 100, options).unwrap();
        assert_eq!(
            out,
            "/-!\nSee [https://example.com](https://example.com) for details.\n-/\n"
        );
    }

    #[test]
    fn explicit_links_stay_links_by_default() {
        let src = "/-- See [the docs](https://example.com) for more. -/\n";
        let out = normalize(src, 100, Options::default()).unwrap();
        assert_eq!(out, src);
    }

    // ------------------------------------------------------------------
    // Markdown tables
    // ------------------------------------------------------------------

    #[test]
    fn padded_table_round_trips_unchanged() {
        let src = "\
/--
| node            | mover | Stop payoff |
| --------------- | ----- | ----------- |
| round 1 (root)  | 0     | `(1, 0)`    |
| round 4 (final) | 1     | `(0, 4)`    |
-/
";
        let out = normalize(src, 100, Options::default()).unwrap();
        assert_eq!(out, src);
    }

    #[test]
    fn delimiters_never_share_a_line_with_table_rows() {
        // Even when the rows would fit beside the delimiters, gluing them on
        // would shift the header row and break the column alignment.
        let src = "/-- | A | B |\n| --- | --- |\n| 1 | 2 | -/\n";
        let out = normalize(src, 100, Options::default()).unwrap();
        assert_eq!(
            out,
            "\
/--
| A   | B   |
| --- | --- |
| 1   | 2   |
-/
"
        );
    }

    #[test]
    fn prose_before_table_keeps_opening_delimiter_attached() {
        let src = "\
/-- The payoff matrix.

| A | B |
| --- | --- |
| 1 | 2 |
-/
";
        let out = normalize(src, 100, Options::default()).unwrap();
        assert_eq!(
            out,
            "\
/-- The payoff matrix.

| A   | B   |
| --- | --- |
| 1   | 2   |
-/
"
        );
    }

    #[test]
    fn code_at_end_reflows_correctly() {
        let src =
            "/-- The expectation of the Gamma distribution with shape `shape` and rate `rate` is
        `shape / rate`. -/";
        let out = reflow_comments(src, 100, SpellingDirection::AmericanToBritish).unwrap();
        assert_eq!(
            out,
            "/-- The expectation of the Gamma distribution with shape `shape` and rate `rate` is\n`shape / rate`. -/\n"
        );
    }

    fn rewrite_with(source: &str, mut rewrite: impl FnMut(&str) -> String) -> (Vec<String>, usize) {
        let lines: Vec<String> = source.lines().map(str::to_owned).collect();
        let blocks = find_blocks_with_context(source, false, false).unwrap();
        rewrite_blocks(lines, &blocks, &mut |lines, block| {
            let text = lines[block.start_line..=block.end_line].join("\n");
            Ok(Some(
                rewrite(&text).split('\n').map(str::to_owned).collect(),
            ))
        })
        .unwrap()
    }

    #[test]
    fn rewrites_that_change_comment_delimiters_are_discarded() {
        let source = "/-- a -/ def x := 1\n";
        // A body ending in `/` turns the closing delimiter into an opening
        // one; an extra `-/` closes the comment early; the trailing code and
        // the opening delimiter must stay.
        for replacement in [
            "/-- a /-/ def x := 1",
            "/-- a -/ b -/ def x := 1",
            "/-- a -/ def x := 2",
            "/- a -/ def x := 1",
        ] {
            let (lines, changed) = rewrite_with(source, |_| replacement.to_owned());
            assert_eq!(lines, ["/-- a -/ def x := 1"]);
            assert_eq!(changed, 0);
        }
        let (lines, changed) = rewrite_with(source, |_| "/-- b\nc -/ def x := 1".to_owned());
        assert_eq!(lines, ["/-- b", "c -/ def x := 1"]);
        assert_eq!(changed, 1);
    }

    #[test]
    fn rewrites_that_panic_or_never_settle_are_discarded() {
        let source = "/-- a -/\n/-- b -/\n/-- c -/\n";
        let (lines, changed) = rewrite_with(source, |text| {
            assert!(!text.contains('b'), "rewrite failed");
            text.replace('a', "A").replace('c', "C")
        });
        assert_eq!(lines, ["/-- A -/", "/-- b -/", "/-- C -/"]);
        assert_eq!(changed, 2);
        let source = "/-- a -/\n/-- b -/ /-- c -/\n";
        let (lines, changed) = rewrite_with(source, |text| {
            if text.contains('x') {
                text.replace('x', "y")
            } else {
                text.replace(['a', 'y'], "x")
            }
        });
        assert_eq!(lines, ["/-- a -/", "/-- b -/ /-- c -/"]);
        assert_eq!(changed, 0);
    }

    #[test]
    fn blocks_on_a_shared_line_see_each_other_s_final_layout() {
        // The second comment opens on the first one's closing line: it is
        // indented like that final line, and the first comment's last line
        // leaves room for the second comment's final opening.
        let source = "    /-- aaaa bbbb cccc dddd eeee -/ /-! ffff -/\n";
        assert_eq!(
            normalize(source, 24, Options::default()).unwrap(),
            "    /-- aaaa bbbb cccc\n    dddd eeee -/ /-!\n    ffff\n    -/\n"
        );
    }
}
