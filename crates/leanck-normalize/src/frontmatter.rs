//! Configurable frontmatter ordering and history-derived missing attribution.
//! Existing header text is retained unless invalid_existing explicitly selects replacement.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Options {
    pub enabled: bool,
    pub header: bool,
    pub sort_imports: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            enabled: true,
            header: true,
            sort_imports: true,
        }
    }
}

/// Normalize the frontmatter of `source`. Files that show no sign of being a
/// Lean module (no `module`, `import`, or `@[expose] … section` line) are
/// returned unchanged, so comment snippets and partial input are never touched.
#[allow(dead_code)]
pub fn normalize(source: &str, year: i32) -> String {
    normalize_with_options(source, year, Options::default())
}

pub fn normalize_with_options(source: &str, _year: i32, options: Options) -> String {
    if !is_module_file(source) && leading_header(source).is_none() {
        return source.to_owned();
    }
    configured(
        source,
        options,
        &leanck_config::NormalizeRules::default(),
        &crate::Context::default(),
    )
    .trim_end_matches('\n')
    .to_owned()
}

// ---------------------------------------------------------------------------
// Line classification
// ---------------------------------------------------------------------------

/// Whether a line begins with Lean 4.28's `[public] [meta] import` command.
fn is_import(line: &str) -> bool {
    let mut words = line.split_whitespace().peekable();
    words.next_if_eq(&"public");
    words.next_if_eq(&"meta");
    words.next() == Some("import")
}

/// A line holding exactly one `[public] [meta] import [all] Name` command.
struct Import<'a> {
    public: bool,
    meta: bool,
    words: Vec<&'a str>,
}

fn parse_import(line: &str) -> Option<Import<'_>> {
    let words: Vec<_> = line.split_whitespace().collect();
    let mut rest = words.as_slice();
    let mut keyword = |word| {
        let found = rest.first() == Some(&word);
        if found {
            rest = &rest[1..];
        }
        found
    };
    let public = keyword("public");
    let meta = keyword("meta");
    if !keyword("import") {
        return None;
    }
    keyword("all");
    matches!(rest, [name] if !is_import_keyword(name)).then_some(Import {
        public,
        meta,
        words,
    })
}

fn is_import_keyword(word: &str) -> bool {
    matches!(word, "import" | "public" | "meta" | "all")
}

/// The code seen by frontmatter: strings and comments are masked, but `/--`
/// openers stay visible because a declaration docstring begins a command and
/// so ends the module header. Module docstrings stay transparent, so one
/// misplaced before the imports can still be reordered after them.
fn code_view(source: &str) -> String {
    let mut view = crate::syntax::code_mask(source).into_bytes();
    for (kind, range) in crate::syntax::spans(source) {
        if kind == crate::syntax::Span::BlockComment && source[range.start..].starts_with("/--") {
            view[range.start..range.start + 3].copy_from_slice(b"/--");
        }
    }
    String::from_utf8(view).expect("restoring an ASCII opener keeps the view UTF-8")
}

/// Number of leading lines in Lean's module header: optional `module` and
/// `prelude`, then the imports. Lean accepts imports nowhere else.
fn header_lines(view: &str) -> usize {
    let mut words = view
        .lines()
        .enumerate()
        .flat_map(|(n, line)| line.split_whitespace().map(move |word| (n, word)))
        .peekable();
    let mut end = 0;
    for keyword in ["module", "prelude"] {
        if let Some((n, _)) = words.next_if(|(_, word)| *word == keyword) {
            end = n + 1;
        }
    }
    loop {
        let mut command = words.clone();
        command.next_if(|(_, word)| *word == "public");
        command.next_if(|(_, word)| *word == "meta");
        if command.next_if(|(_, word)| *word == "import").is_none() {
            return end;
        }
        command.next_if(|(_, word)| *word == "all");
        match command.next() {
            Some((n, name)) if !is_import_keyword(name) => {
                end = n + 1;
                words = command;
            }
            _ => return end,
        }
    }
}

/// Matches `@[expose] public section` and `@[expose] public noncomputable
/// section` (the `noncomputable` keyword is carried along verbatim).
fn is_section(line: &str) -> bool {
    let t = line.trim();
    t.starts_with("@[expose]") && t.contains("public") && t.ends_with("section")
}

fn is_namespace(line: &str) -> bool {
    line.trim().starts_with("namespace ")
}

pub(crate) fn is_module_file(source: &str) -> bool {
    crate::syntax::code_mask(source).lines().any(|l| {
        let t = l.trim();
        t == "module" || is_import(l) || is_section(l)
    })
}

// ---------------------------------------------------------------------------
// 1. Copyright header
// ---------------------------------------------------------------------------

pub(crate) fn leading_header(source: &str) -> Option<std::ops::Range<usize>> {
    let start = source.len() - source.trim_start_matches('\u{feff}').trim_start().len();
    let rest = &source[start..];
    if !rest.starts_with("/-") || rest.starts_with("/--") || rest.starts_with("/-!") {
        return None;
    }
    let mut depth = 1;
    let bytes = rest.as_bytes();
    let mut i = 2;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/-") {
            depth += 1;
            i += 2;
        } else if bytes[i..].starts_with(b"-/") {
            depth -= 1;
            i += 2;
            if depth == 0 {
                return Some(start..start + i);
            }
        } else {
            i += 1;
        }
    }
    None
}

pub(crate) fn configured(
    source: &str,
    options: Options,
    rules: &leanck_config::NormalizeRules,
    context: &crate::Context<'_>,
) -> String {
    if !options.enabled
        || (!is_module_file(source)
            && leading_header(source).is_none()
            && context.history.is_none()
            && context.module_name.is_none())
    {
        return source.to_owned();
    }
    let mut out = source.to_owned();
    if let Some(range) = leading_header(&out) {
        if options.header {
            let replacement = (rules.frontmatter.invalid_existing
                == leanck_config::InvalidExistingHeader::Replace
                && !matches_header_template(&out[range.clone()], &rules.frontmatter))
            .then(|| generated_header(&rules.frontmatter, context))
            .flatten();
            if let Some(header) = replacement {
                out.replace_range(range, &header);
            } else if rules.frontmatter.normalize_existing {
                let normalized = format!(
                    "/-\n{}\n-/",
                    delimited_body(&out[range.start + 2..range.end - 2])
                );
                out.replace_range(range, &normalized);
            }
        }
    } else if options.header
        && let Some(header) = generated_header(&rules.frontmatter, context)
    {
        out = format!("{header}\n{}", out.trim_start_matches(['\r', '\n']));
    }
    let mut lines: Vec<_> = out.lines().map(str::to_owned).collect();
    if options.sort_imports {
        let view = code_view(&out);
        sort_imports_configured(&mut lines, &view, &rules.imports);
    }
    let mut out = reorder(&lines.join("\n"), &rules.frontmatter, context.module_name);
    // Keep the source's trailing line endings, which lines()/join() and
    // reordering drop, so a blank line before a directive survives.
    let tail = &source[source.trim_end_matches(['\r', '\n']).len()..];
    out.truncate(out.trim_end_matches(['\r', '\n']).len());
    out.extend(std::iter::repeat_n('\n', tail.matches('\n').count()));
    out
}

fn generated_header(
    rules: &leanck_config::FrontmatterRules,
    context: &crate::Context<'_>,
) -> Option<String> {
    let history = context
        .history?
        .excluding(&rules.ignore_authors)
        .map_names(&rules.author_map);
    let years: Vec<_> = history
        .contributors
        .iter()
        .flat_map(|c| c.years.iter().copied())
        .collect();
    let replacements = [
        ("copyright", history.copyright()),
        ("authors", history.authors()),
        ("years", history.years()),
        ("license", rules.license_text.clone()),
        ("first_year", years.iter().min()?.to_string()),
        ("last_year", years.iter().max()?.to_string()),
    ];
    let header = render_template(&rules.header_template, &replacements);
    Some(
        header
            .lines()
            .filter(|line| !line.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

/// Only missing headers and explicitly replaceable mismatches need attribution.
#[cfg(feature = "command")]
pub(crate) fn needs_history(
    source: &str,
    options: Options,
    rules: &leanck_config::FrontmatterRules,
) -> bool {
    options.enabled
        && options.header
        && leading_header(source).is_none_or(|range| {
            rules.invalid_existing == leanck_config::InvalidExistingHeader::Replace
                && !matches_header_template(&source[range], rules)
        })
}

pub(crate) fn header_warning(
    source: &str,
    options: Options,
    rules: &leanck_config::FrontmatterRules,
    context: &crate::Context<'_>,
) -> Option<leanck_config::Warning> {
    use leanck_config::InvalidExistingHeader as Policy;
    if !options.enabled || !options.header || rules.invalid_existing == Policy::Ignore {
        return None;
    }
    let range = leading_header(source)?;
    if matches_header_template(&source[range.clone()], rules) {
        return None;
    }
    let warning = match rules.invalid_existing {
        Policy::Warn => leanck_config::Warning::new(
            "header_template_mismatch",
            "existing header does not match normalize.rules.frontmatter.header_template (including configured license_text); existing attribution and license text are retained",
        ),
        Policy::Replace if generated_header(rules, context).is_none() => {
            leanck_config::Warning::new(
                "header_replacement_unavailable",
                "cannot replace existing header without usable author history; existing header retained",
            )
        }
        Policy::Replace => return None,
        Policy::Ignore => unreachable!(),
    };
    Some(
        warning.at_line(
            source[..range.start]
                .bytes()
                .filter(|b| *b == b'\n')
                .count()
                + 1,
        ),
    )
}

/// Check shape and fixed text, never attribution against today's Git history.
/// Whitespace that header normalization can adjust is insignificant here.
fn matches_header_template(header: &str, rules: &leanck_config::FrontmatterRules) -> bool {
    let template = comment_body(&rules.header_template);
    let template = template
        .lines()
        .filter(|line| !(line.trim() == "{license}" && rules.license_text.is_empty()))
        .collect::<Vec<_>>()
        .join("\n");
    let mut pattern = String::new();
    let mut rest = template.as_str();
    while let Some((literal, tail)) = rest.split_once('{') {
        append_literal_pattern(&mut pattern, literal);
        let (key, after) = tail.split_once('}').expect("validated header template");
        match key {
            "copyright" => pattern
                .push_str(r"Copyright\s+\(c\)\s+[0-9]{1,4}(?:[–-][0-9]{1,4})?[ \t]+[^\r\n]+\."),
            "authors" => pattern.push_str(r"[^\r\n]+"),
            "years" => pattern.push_str(r"[0-9]{1,4}(?:[–-][0-9]{1,4})?"),
            "first_year" | "last_year" => pattern.push_str(r"[0-9]{1,4}"),
            "license" => append_literal_pattern(&mut pattern, &rules.license_text),
            _ => unreachable!("validated header placeholder"),
        }
        rest = after;
    }
    append_literal_pattern(&mut pattern, rest);
    // Empty inline placeholders can leave adjacent or edge whitespace. Ignore
    // it just as comment_body ignores whitespace at the edges of actual text.
    while pattern.contains(r"\s+\s+") {
        pattern = pattern.replace(r"\s+\s+", r"\s+");
    }
    let pattern = format!(
        r"\A{}\z",
        pattern.trim_start_matches(r"\s+").trim_end_matches(r"\s+")
    );
    let body = comment_body(header);
    regex::Regex::new(&pattern).is_ok_and(|regex| regex.is_match(&body))
}

/// Trim only the whitespace next to the delimiters of a header body: blank
/// lines after `/-`, spaces before text on the opening line, and whitespace
/// before `-/`. Indentation inside the comment is retained.
fn delimited_body(body: &str) -> &str {
    let mut body = body.trim_end();
    if body
        .lines()
        .next()
        .is_none_or(|first| !first.trim().is_empty())
    {
        return body.trim_start();
    }
    while let Some((first, rest)) = body.split_once('\n')
        && first.trim().is_empty()
    {
        body = rest;
    }
    body
}

fn comment_body(comment: &str) -> String {
    let comment = comment.trim();
    comment[2..comment.len() - 2]
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

fn append_literal_pattern(pattern: &mut String, literal: &str) {
    let mut whitespace = false;
    for ch in literal.chars() {
        if ch.is_whitespace() {
            if !whitespace {
                pattern.push_str(r"\s+");
            }
        } else {
            pattern.push_str(&regex::escape(&ch.to_string()));
        }
        whitespace = ch.is_whitespace();
    }
}

/// Sort and deduplicate runs of import lines in Lean's module header. A line
/// joins a run only when it is one complete import command with no comment, so
/// the code view and the raw text agree on every line that moves.
fn sort_imports_configured(
    lines: &mut Vec<String>,
    view: &str,
    rules: &leanck_config::ImportRules,
) {
    use leanck_config::ImportOrder;
    if rules.order == ImportOrder::Off && !rules.deduplicate {
        return;
    }
    let code: Vec<_> = view.lines().collect();
    let header = header_lines(view).min(lines.len());
    let imports: Vec<_> = (0..header)
        .map(|i| {
            if code.get(i) != Some(&lines[i].as_str()) {
                return None;
            }
            let import = parse_import(&lines[i])?;
            Some((import.public, import.meta, import.words.join(" ")))
        })
        .collect();
    let mut out = Vec::with_capacity(lines.len());
    let mut i = 0;
    while i < header {
        if imports[i].is_none() {
            out.push(std::mem::take(&mut lines[i]));
            i += 1;
            continue;
        }
        let start = i;
        i += 1;
        while i < header
            && (imports[i].is_some() || (!rules.preserve_groups && lines[i].trim().is_empty()))
        {
            i += 1;
        }
        // Blank lines after the run's last import stay outside it.
        while imports[i - 1].is_none() {
            i -= 1;
        }
        let mut run: Vec<_> = (start..i)
            .filter_map(|j| {
                let (public, meta, words) = imports[j].clone()?;
                Some((public, meta, words, std::mem::take(&mut lines[j])))
            })
            .collect();
        if rules.order == ImportOrder::Alphabetical {
            run.sort_by_cached_key(|(public, meta, _, line)| {
                (rules.public_first && !public, !meta, line.trim().to_owned())
            });
        }
        if rules.deduplicate {
            let mut seen = std::collections::HashSet::new();
            run.retain(|(_, _, words, _)| seen.insert(words.clone()));
        }
        out.extend(run.into_iter().map(|(_, _, _, line)| line));
    }
    out.extend(lines.drain(header..));
    *lines = out;
}

/// Only the recognized initial prefix is reordered; body declarations are a barrier.
fn reorder(
    source: &str,
    rules: &leanck_config::FrontmatterRules,
    module_name: Option<&str>,
) -> String {
    use leanck_config::FrontmatterPart as P;
    let lines: Vec<_> = source.lines().collect();
    let line_starts: Vec<_> = std::iter::once(0)
        .chain(source.match_indices('\n').map(|(n, _)| n + 1))
        .collect();
    let view = code_view(source);
    let code: Vec<_> = view.lines().collect();
    let header = header_lines(&view);
    let mut blocks: Vec<(P, String)> = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        if lines[i].trim().is_empty() {
            i += 1;
            continue;
        }
        let t = lines[i].trim_start();
        let start = i;
        let part = if t.starts_with("/-") && !t.starts_with("/--") {
            // Only a comment that opens the file is a header. A later plain
            // comment is unrecognized content, so hoisting it cannot turn it
            // into a header on the next run.
            let open = line_starts[i] + lines[i].len() - t.len();
            let Some(end) = crate::syntax::block_end(source, open) else {
                break;
            };
            // A section heading, such as a converted banner, belongs to the
            // body rather than being the module docstring. Comment reflow
            // makes the same distinction.
            let part = if t.starts_with("/-!") {
                if crate::reflow::is_section_heading_text(&source[open + 3..end - 2]) {
                    break;
                }
                P::Docstring
            } else if blocks.is_empty() {
                P::Header
            } else {
                break;
            };
            let last = i + source[open..end].matches('\n').count();
            if !lines[last][end - line_starts[last]..].trim().is_empty() {
                break;
            }
            i = last + 1;
            part
        } else if i < header && code.get(i).is_some_and(|c| c.trim() == "module") {
            i += 1;
            P::Module
        } else if i < header && is_import(code.get(i).copied().unwrap_or("")) {
            i += 1;
            while i < header
                && (is_import(code.get(i).copied().unwrap_or("")) || lines[i].trim().is_empty())
            {
                i += 1;
            }
            while i > start && lines[i - 1].trim().is_empty() {
                i -= 1;
            }
            P::Imports
        } else if is_section(code.get(i).copied().unwrap_or("")) {
            i += 1;
            P::Section
        } else if is_namespace(code.get(i).copied().unwrap_or("")) {
            i += 1;
            P::Namespace
        } else {
            break;
        };
        if blocks.iter().any(|(p, _)| *p == part) {
            i = start;
            break;
        }
        blocks.push((part, lines[start..i].join("\n")));
    }
    if !blocks.iter().any(|(part, _)| *part == P::Docstring)
        && let (Some(template), Some(module)) = (&rules.module_doc_template, module_name)
    {
        blocks.push((P::Docstring, template.replace("{module}", module)));
    }
    if blocks.is_empty() {
        return source.to_owned();
    }
    // Hard scope constraints and omitted-block barriers take precedence over
    // the preferred order. Pick the next eligible preferred block each time.
    let mut edges = vec![
        (P::Module, P::Imports),
        (P::Module, P::Section),
        (P::Module, P::Namespace),
        (P::Imports, P::Section),
        (P::Imports, P::Namespace),
    ];
    for (part, _) in &blocks {
        if *part != P::Header {
            edges.push((P::Header, *part));
        }
    }
    // An omitted component keeps its position relative to every other block.
    // Included blocks can therefore move only within the runs between barriers.
    for (anchor, (part, _)) in blocks.iter().enumerate() {
        if !rules.order.contains(part) {
            for (before, _) in &blocks[..anchor] {
                edges.push((*before, *part));
            }
            for (after, _) in &blocks[anchor + 1..] {
                edges.push((*part, *after));
            }
        }
    }
    blocks.sort_by_key(|(p, _)| {
        rules
            .order
            .iter()
            .position(|k| k == p)
            .unwrap_or(usize::MAX)
    });
    let mut ordered = Vec::new();
    while !blocks.is_empty() {
        let Some(next) = blocks.iter().position(|(candidate, _)| {
            !edges.iter().any(|(parent, child)| {
                child == candidate && blocks.iter().any(|(p, _)| p == parent)
            })
        }) else {
            return source.to_owned();
        };
        ordered.push(blocks.remove(next));
    }
    let blocks = ordered;
    let gap = "\n".repeat((rules.blank_lines + 1) as usize); // Safe because u8::MAX << usize::MAX
    let mut out = blocks
        .into_iter()
        .map(|(_, s)| s)
        .collect::<Vec<_>>()
        .join(&gap);
    if i < lines.len() {
        out.push_str(&gap);
        out.push_str(&lines[i..].join("\n"));
    }
    out
}

// ---------------------------------------------------------------------------
// 2. Import sorting
// ---------------------------------------------------------------------------

fn render_template(template: &str, values: &[(&str, String)]) -> String {
    let mut out = String::new();
    let mut rest = template;
    while let Some((before, tail)) = rest.split_once('{') {
        out.push_str(before);
        let Some((key, after)) = tail.split_once('}') else {
            out.push('{');
            out.push_str(tail);
            return out;
        };
        if let Some((_, value)) = values.iter().find(|(name, _)| *name == key) {
            out.push_str(value);
        } else {
            out.push('{');
            out.push_str(key);
            out.push('}');
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const YEAR: i32 = 2026;

    #[test]
    fn snippets_without_module_markers_are_untouched() {
        let src = "/-- a doc comment -/\ndef x := 1\n";
        assert_eq!(normalize(src, YEAR), src);
    }

    #[test]
    fn missing_header_is_not_invented_without_history() {
        assert_eq!(
            normalize("module\n\nimport Foo\n", YEAR),
            "module\n\nimport Foo"
        );
    }

    #[test]
    fn existing_header_is_left_untouched() {
        let src = "/-\nCopyright (c) 1999 Someone Else.\n-/\nmodule\n\nimport Foo\n";
        let out = normalize(src, YEAR);
        assert!(out.starts_with("/-\nCopyright (c) 1999 Someone Else.\n-/"));
    }

    #[test]
    fn imports_are_alphabetized_with_public_first() {
        let src = "/-\nx\n-/\nmodule\n\nimport Zebra\npublic import Apple\nimport Mango\npublic import Cat\n";
        let out = normalize(src, YEAR);
        let imports: Vec<&str> = out.lines().filter(|l| is_import(l)).collect();
        assert_eq!(
            imports,
            vec![
                "public import Apple",
                "public import Cat",
                "import Mango",
                "import Zebra",
            ]
        );
    }

    #[test]
    fn single_import_is_left_as_is() {
        let src = "/-\nx\n-/\nmodule\n\nimport OnlyOne\n";
        let out = normalize(src, YEAR);
        assert!(out.contains("import OnlyOne"));
    }

    #[test]
    fn section_is_moved_after_docstring() {
        // A docstring that is only a title is a section heading, so these
        // module docstrings have a body.
        let src = "/-\nx\n-/\nmodule\n\nimport Foo\n\n@[expose] public section\n\n/-!\n# Title\n\nBody.\n-/\n";
        let out = normalize(src, YEAR);
        let section = out
            .lines()
            .position(|l| l.trim() == "@[expose] public section")
            .unwrap();
        let title = out.lines().position(|l| l.contains("# Title")).unwrap();
        // section now follows the docstring body.
        assert!(section > title);
    }

    #[test]
    fn noncomputable_section_is_carried_along() {
        let src = "/-\nx\n-/\nmodule\n\n@[expose] public noncomputable section\n\n/-!\n# Title\n\nBody.\n-/\n";
        let out = normalize(src, YEAR);
        let lines: Vec<&str> = out.lines().collect();
        let section = lines
            .iter()
            .position(|l| l.trim() == "@[expose] public noncomputable section")
            .unwrap();
        let title = lines.iter().position(|l| l.contains("# Title")).unwrap();
        assert!(section > title);
    }

    #[test]
    fn section_already_after_docstring_stays() {
        let src = "/-\nx\n-/\nmodule\n\n/-!\n# Title\n\nBody.\n-/\n\n@[expose] public section\n";
        let out = normalize(src, YEAR);
        // Idempotent: running again yields the same text.
        assert_eq!(normalize(&out, YEAR), out);
    }

    #[test]
    fn namespace_between_docstring_and_section_moves_after_section() {
        let src = "/-\nx\n-/\nmodule\n\n/-!\n# Title\n\nBody.\n-/\n\nnamespace Foo\n\n@[expose] public section\n";
        let out = normalize(src, YEAR);
        let lines: Vec<&str> = out.lines().collect();
        let title = lines.iter().position(|l| l.contains("# Title")).unwrap();
        let section = lines.iter().position(|l| is_section(l)).unwrap();
        let ns = lines.iter().position(|l| is_namespace(l)).unwrap();
        assert!(title < section, "section after docstring");
        assert!(section < ns, "namespace after section");
    }

    #[test]
    fn namespace_moves_when_section_precedes_docstring() {
        // section before docstring, namespace already after: section is moved
        // after the docstring, and the namespace ends up after the section.
        let src = "/-\nx\n-/\nmodule\n\n@[expose] public section\n\n/-!\n# Title\n\nBody.\n-/\n\nnamespace Foo\n";
        let out = normalize(src, YEAR);
        let lines: Vec<&str> = out.lines().collect();
        let title = lines.iter().position(|l| l.contains("# Title")).unwrap();
        let section = lines.iter().position(|l| is_section(l)).unwrap();
        let ns = lines.iter().position(|l| is_namespace(l)).unwrap();
        assert!(title < section && section < ns);
        // and stable under re-running.
        assert_eq!(normalize(&out, YEAR), out);
    }

    #[test]
    fn inverse_case_docstring_last_is_fully_reordered() {
        // Original order: section, namespace, docstring, body. Must become
        // docstring, section, namespace, body.
        let src = "/-\nx\n-/\nmodule\n\n@[expose] public section\n\nnamespace Foo\n\n/-!\n# Title\n\nBody.\n-/\n\ndef foo := 1\n";
        let out = normalize(src, YEAR);
        let lines: Vec<&str> = out.lines().collect();
        let title = lines.iter().position(|l| l.contains("# Title")).unwrap();
        let section = lines.iter().position(|l| is_section(l)).unwrap();
        let ns = lines.iter().position(|l| is_namespace(l)).unwrap();
        let body = lines.iter().position(|l| l.contains("def foo")).unwrap();
        assert!(
            title < section && section < ns && ns < body,
            "expected docstring < section < namespace < body, got {title} {section} {ns} {body}"
        );
        // Idempotent.
        assert_eq!(normalize(&out, YEAR), out);
    }
}
