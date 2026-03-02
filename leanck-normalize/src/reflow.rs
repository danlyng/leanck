use markdown_reflow::reflow;

pub use crate::spelling::SpellingDirection;
use crate::spelling::correct_spelling;

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
            capitalize_after_colon: true,
            collapse_blank_lines: true,
        }
    }
}

const PREFIX: &str = "/--";
const PREFIX_DOC: &str = "/-!";
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
pub fn normalize(source: &str, target_width: usize, options: Options) -> Result<String, String> {
    normalize_with_stats(source, target_width, options).map(|result| result.output)
}

pub fn normalize_with_stats(
    source: &str,
    target_width: usize,
    options: Options,
) -> Result<NormalizeResult, String> {
    let mut stats = Stats::default();

    let mut normalized = crate::frontmatter::normalize_with_options(
        source,
        crate::frontmatter::current_year(),
        options.frontmatter,
    );
    if normalized != source {
        stats.frontmatter_changes = 1;
    }

    if options.attributes {
        let (attribute_normalized, moved) = crate::attributes::normalize_with_count(&normalized);
        normalized = attribute_normalized;
        stats.attributes_moved = moved;
    }

    if options.headings {
        let (heading_normalized, converted) = convert_heading_banners(&normalized);
        normalized = heading_normalized;
        stats.headings_converted = converted;
    }
    let mut lines: Vec<String> = normalized.lines().map(|s| s.to_string()).collect();

    if options.comments {
        let blocks = find_blocks(&normalized)?;
        // Process from bottom to top so line indices above don't shift.
        for block in blocks.iter().rev() {
            if reflow_block(&mut lines, block, target_width, options)? {
                stats.comment_blocks_changed += 1;
            }
        }
    }

    if options.collapse_blank_lines {
        stats.blank_lines_collapsed = collapse_blank_lines(&mut lines);
    }

    let mut out = lines.join("\n");
    if source.ends_with('\n') && !out.ends_with('\n') {
        out.push('\n');
    }
    Ok(NormalizeResult { output: out, stats })
}

#[allow(dead_code)]
pub fn reflow_comments(
    source: &str,
    target_width: usize,
    spelling: SpellingDirection,
) -> Result<String, String> {
    let options = Options {
        spelling: Some(spelling),
        ..Options::default()
    };
    normalize(source, target_width, options)
}

/// Collapse runs of consecutive blank lines into a single blank line, anywhere
/// in the file (inside comments or not). A line counts as blank when it is
/// empty or whitespace-only; the kept line is emptied of its whitespace.
fn collapse_blank_lines(lines: &mut Vec<String>) -> usize {
    let before = lines.len();
    let mut prev_blank = false;
    lines.retain_mut(|line| {
        let blank = line.trim().is_empty();
        let keep = !(blank && prev_blank);
        prev_blank = blank;
        if keep && blank {
            line.clear();
        }
        keep
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
            // Capitalize only when at least one space separated a letter token.
            if i > ws_start && i < chars.len() && chars[i].is_ascii_alphabetic() {
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
fn convert_heading_banners(source: &str) -> (String, usize) {
    let lines: Vec<&str> = source.lines().collect();
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut converted = 0usize;
    let mut depth = 0usize;
    let mut i = 0;

    while i < lines.len() {
        if depth == 0
            && let Some((title, end)) = parse_banner(&lines, i)
        {
            let indent = leading_whitespace(lines[i]);
            out.push(format!("{indent}{PREFIX_DOC} ## {title} {SUFFIX}"));
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
    Some((title_case(&title), i))
}

/// Small words left uncapitalized in the middle of a title.
const SMALL_WORDS: &[&str] = &[
    "a", "an", "and", "as", "at", "but", "by", "for", "from", "in", "into", "nor", "of", "off",
    "on", "onto", "or", "over", "per", "so", "the", "to", "up", "via", "vs", "with", "yet",
];

/// Title-case `text`: capitalize the first letter of each word (and of each
/// hyphen-separated segment), except small words in the middle of the title.
/// Words touching backtick code spans are never altered, and existing
/// capitalization is preserved (only lowercase first letters are raised).
fn title_case(text: &str) -> String {
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
        let small = SMALL_WORDS.contains(&word.to_ascii_lowercase().as_str());
        if small && i != 0 && i != last {
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
    /// Whether this is the first `/-!` block in the file (module docstring).
    module_doc: bool,
    /// Byte offset of the start delimiter on `start_line`.
    start_delim_col: usize,
    /// Byte offset of the end delimiter on `end_line`.
    end_delim_col: usize,
}

/// Scan `source` for Lean block comments.  Returns an error if any delimiter
/// is unmatched.
fn find_blocks(source: &str) -> Result<Vec<Block>, String> {
    let lines: Vec<&str> = source.lines().collect();
    let mut blocks = Vec::new();
    let mut seen_doc_comment = false;

    // (start_line, start_after_byte, is_doc, start_delim_col)
    let mut open: Option<(usize, usize, bool, usize)> = None;

    for (li, line) in lines.iter().enumerate() {
        let bytes = line.as_bytes();
        let mut col = 0usize;
        while col < line.len() {
            if let Some((sl, sa, is_doc, sdc)) = open {
                if bytes[col..].starts_with(SUFFIX.as_bytes()) {
                    // A `## …` block is a section heading, never the module
                    // docstring — it must not claim the module-doc slot.
                    let module_doc = is_doc
                        && !seen_doc_comment
                        && !is_section_heading_block(&lines, sl, sa, li, col);
                    if is_doc {
                        seen_doc_comment = true;
                    }
                    blocks.push(Block {
                        start_line: sl,
                        start_after: sa,
                        end_line: li,
                        end_before: col,
                        is_doc,
                        module_doc,
                        start_delim_col: sdc,
                        end_delim_col: col,
                    });
                    open = None;
                    col += SUFFIX.len();
                } else {
                    col += 1;
                }
            } else {
                if bytes[col..].starts_with(PREFIX_DOC.as_bytes()) {
                    open = Some((li, col + PREFIX_DOC.len(), true, col));
                    col += PREFIX_DOC.len();
                } else if bytes[col..].starts_with(PREFIX.as_bytes()) {
                    open = Some((li, col + PREFIX.len(), false, col));
                    col += PREFIX.len();
                } else {
                    col += 1;
                }
            }
        }
    }

    if let Some((sl, _, is_doc, _)) = open {
        let tag = if is_doc { "/-!" } else { "/--" };
        return Err(format!(
            "unmatched opening delimiter '{tag}' at line {} (no closing '-/')",
            sl + 1
        ));
    }

    Ok(blocks)
}

/// Whether a block's content begins with a markdown `##` heading. Such blocks
/// are section headings, not module docstrings.
fn is_section_heading_block(
    lines: &[&str],
    start_line: usize,
    start_after: usize,
    end_line: usize,
    end_before: usize,
) -> bool {
    for (offset, line) in lines[start_line..=end_line].iter().enumerate() {
        let li = start_line + offset;
        let start = if li == start_line { start_after } else { 0 };
        let end = if li == end_line { end_before } else { line.len() };
        let t = line[start..end].trim();
        if !t.is_empty() {
            return t.starts_with("##");
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Per-block reflow
// ---------------------------------------------------------------------------

fn reflow_block(
    lines: &mut Vec<String>,
    block: &Block,
    target_width: usize,
    options: Options,
) -> Result<bool, String> {
    let before = lines[block.start_line..=block.end_line].join("\n");
    let prefix = if block.is_doc { "/-!" } else { "/--" };

    let original_content = extract_content(lines, block);
    let mut content = original_content.clone();
    if let Some(spelling) = options.spelling {
        content = correct_spelling(&content, spelling);
    }
    if options.capitalize_after_colon {
        content = capitalize_after_colon(&content);
    }
    if !options.comment_reflow && content == original_content {
        return Ok(false);
    }
    if content.trim().is_empty() {
        return Ok(false);
    }

    let indent = leading_whitespace(&lines[block.start_line]).to_string();
    let indent_w = char_width(&indent);

    if block.module_doc {
        // Module docstring ── delimiters always on their own lines.
        let reflowed;
        let content_ref = if options.comment_reflow {
            let effective_w = target_width.saturating_sub(indent_w).max(1);
            reflowed = reflow::reflow_with_options(
                &content,
                Some(effective_w),
                reflow::Options {
                    linkify_urls: options.linkify_urls,
                },
            )?;
            reflowed.trim_end()
        } else {
            content.trim_end()
        };
        let lines_ref: Vec<&str> = content_ref.lines().collect();
        expand_module_doc(lines, block, &lines_ref, prefix, &indent);
    } else {
        // Normal comment ── prefer delimiters on same line as content.
        let reflowed;
        let content_ref = if options.comment_reflow {
            let before = &lines[block.start_line][..block.start_delim_col];
            let after_start = block.end_delim_col + SUFFIX.len();
            let after = &lines[block.end_line][after_start..];
            let continuation_w = if before.trim().is_empty() {
                indent_w
            } else {
                char_width(before)
            };
            let opening_w = continuation_w + OPENING_OVERHEAD;
            let closing_w = continuation_w + 1 + char_width(SUFFIX) + char_width(after);
            let both_w = opening_w + 1 + char_width(SUFFIX) + char_width(after);

            // Reserve space for delimiters only on the lines where they appear:
            // `/-- ` on the first line, ` -/` on the last line, and both when
            // the content remains on a single line.
            let first_w = target_width.saturating_sub(opening_w).max(1);
            let middle_w = target_width.saturating_sub(continuation_w).max(1);
            let final_w = target_width.saturating_sub(closing_w).max(1);
            let single_w = target_width.saturating_sub(both_w).max(1);
            reflowed = reflow::reflow_with_edge_widths(
                &content,
                Some(first_w),
                Some(middle_w),
                Some(final_w),
                Some(single_w),
                reflow::Options {
                    linkify_urls: options.linkify_urls,
                },
            )?;
            reflowed.trim_end()
        } else {
            content.trim_end()
        };
        let lines_ref: Vec<&str> = content_ref.lines().collect();

        // If the original was single-line and the content is still a single
        // line, try the true single-line format.
        if block.start_line == block.end_line
            && lines_ref.len() == 1
            && try_single_line(lines, block, lines_ref[0], prefix, target_width)
        {
            return Ok(block_changed(lines, block.start_line, &before));
        }

        expand_compact(lines, block, &lines_ref, prefix, &indent, target_width);
    }

    Ok(block_changed(lines, block.start_line, &before))
}

fn block_changed(lines: &[String], start: usize, before: &str) -> bool {
    let end = (start..lines.len())
        .find(|&i| lines[i].contains(SUFFIX))
        .unwrap_or(start);
    lines[start..=end].join("\n") != before
}

// ---------------------------------------------------------------------------
// Content extraction
// ---------------------------------------------------------------------------

fn extract_content(lines: &[String], block: &Block) -> String {
    let mut parts: Vec<&str> = Vec::new();

    for li in block.start_line..=block.end_line {
        let line = &lines[li];
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

    let joined = parts.join("\n");
    strip_common_indent(&joined)
}

fn strip_common_indent(s: &str) -> String {
    let lines: Vec<&str> = s.lines().collect();
    let min_indent = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .map(|l| l.len() - l.trim_start().len())
        .min()
        .unwrap_or(0);

    if min_indent == 0 {
        return s.to_string();
    }

    let mut out = String::with_capacity(s.len());
    for (i, line) in lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.len() >= min_indent {
            out.push_str(&line[min_indent..]);
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Single-line attempt (returns false if it doesn't fit)
// ---------------------------------------------------------------------------

fn try_single_line(
    lines: &mut Vec<String>,
    block: &Block,
    content: &str,
    prefix: &str,
    target_width: usize,
) -> bool {
    let before: String = lines[block.start_line][..block.start_delim_col].to_string();
    let after_start = block.end_delim_col + SUFFIX.len();
    let after: String = lines[block.start_line][after_start..].to_string();

    let candidate = format!("{before}{prefix} {content} {SUFFIX}{after}");

    if char_width(&candidate) <= target_width {
        lines[block.start_line] = candidate;
        true
    } else {
        false
    }
}

// ---------------------------------------------------------------------------
// Module-docstring expansion (delimiters on their own lines)
// ---------------------------------------------------------------------------

fn expand_module_doc(
    lines: &mut Vec<String>,
    block: &Block,
    content_lines: &[&str],
    prefix: &str,
    indent: &str,
) {
    let before: String = lines[block.start_line][..block.start_delim_col].to_string();
    let after_start = block.end_delim_col + SUFFIX.len();
    let after: String = lines[block.end_line][after_start..].to_string();

    let base_indent = if before.trim().is_empty() {
        indent.to_string()
    } else {
        " ".repeat(char_width(&before))
    };

    let mut out: Vec<String> = Vec::new();
    out.push(format!("{before}{prefix}"));

    for cl in content_lines {
        if cl.trim().is_empty() {
            out.push(base_indent.clone());
        } else {
            out.push(format!("{base_indent}{cl}"));
        }
    }

    if after.is_empty() {
        out.push(format!("{base_indent}{SUFFIX}"));
    } else {
        out.push(format!("{base_indent}{SUFFIX}{after}"));
    }

    splice_vec(lines, block.start_line, block.end_line, &out);
}

// ---------------------------------------------------------------------------
// Compact expansion (delimiters share lines with content; no orphan -/)
// ---------------------------------------------------------------------------

fn expand_compact(
    lines: &mut Vec<String>,
    block: &Block,
    content_lines: &[&str],
    prefix: &str,
    indent: &str,
    target_width: usize,
) {
    let before: String = lines[block.start_line][..block.start_delim_col].to_string();
    let after_start = block.end_delim_col + SUFFIX.len();
    let after: String = lines[block.end_line][after_start..].to_string();

    // Indentation for continuation lines.
    let base_indent = if before.trim().is_empty() {
        indent.to_string()
    } else {
        " ".repeat(char_width(&before))
    };

    let mut out: Vec<String> = Vec::new();

    if content_lines.len() == 1 {
        // Only one content line ─ put both delimiters on it.
        let c = content_lines[0];
        out.push(format!("{before}{prefix} {c} {SUFFIX}{after}"));
    } else {
        let first = content_lines[0];
        let last = content_lines[content_lines.len() - 1];

        // --- opening delimiter ----------------------------------------------
        // A table row never shares a line with the delimiter: prepending
        // `/-- ` would shift the row and break the column alignment.
        let first_with_prefix = format!("{before}{prefix} {first}");
        if !is_table_row(first) && char_width(&first_with_prefix) <= target_width {
            out.push(first_with_prefix);
        } else {
            // Doesn't fit — opening delimiter on its own line.
            out.push(format!("{before}{prefix}"));
            // The first content line becomes a regular continuation line.
            out.push(format!("{base_indent}{first}"));
        }

        // --- middle lines (if any) ------------------------------------------
        for i in 1..content_lines.len() - 1 {
            let line = content_lines[i];
            if line.trim().is_empty() {
                out.push(base_indent.clone());
            } else {
                out.push(format!("{base_indent}{line}"));
            }
        }

        // --- closing delimiter (attached to the last content line, except
        // after a table row, where the orphan rule yields to the table) ------
        if is_table_row(last) {
            out.push(format!("{base_indent}{last}"));
            out.push(format!("{base_indent}{SUFFIX}{after}"));
        } else if after.is_empty() {
            out.push(format!("{base_indent}{last} {SUFFIX}"));
        } else {
            out.push(format!("{base_indent}{last} {SUFFIX}{after}"));
        }
    }

    splice_vec(lines, block.start_line, block.end_line, &out);
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// A markdown table row: the trimmed line starts and ends with a pipe.
fn is_table_row(line: &str) -> bool {
    let t = line.trim();
    t.len() >= 2 && t.starts_with('|') && t.ends_with('|')
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

fn splice_vec(vec: &mut Vec<String>, start: usize, end: usize, replacement: &[String]) {
    vec.splice(start..=end, replacement.iter().cloned());
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
        assert_eq!(out, "/-! ## The Beer-Quiche Signaling Game -/\ndef x := 1\n");
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
    fn capitalize_after_colon_in_reflowed_comment() {
        let out = reflow_comments(
            "/-- This is: not okay -/\n",
            100,
            SpellingDirection::BritishToAmerican,
        )
        .unwrap();
        assert_eq!(out, "/-- This is: Not okay -/\n");
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
            "/-- The expectation of the Gamma distribution with shape `shape` and rate `rate` is\n`shape / rate`. -/"
        );
    }
}
