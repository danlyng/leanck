use crate::{Dictionary, NormalizeError, NormalizeResult, Options, Stats, history::FileHistory};
use leanck_config::{FinalNewline, LineEndings, NormalizeRules};
use std::collections::BTreeSet;

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Rule {
    Frontmatter,
    Headings,
    Attributes,
    Comments,
    Spelling,
    CommentReflow,
    CapitalizeAfterColon,
    Whitespace,
}

impl Rule {
    const ALL: [Self; 8] = [
        Self::Frontmatter,
        Self::Headings,
        Self::Attributes,
        Self::Comments,
        Self::Spelling,
        Self::CommentReflow,
        Self::CapitalizeAfterColon,
        Self::Whitespace,
    ];
    // The comments master switch also controls its prose transformations.
    // Expand it into individual rules so a later named `on` can restore one child.
    const COMMENT_RULES: [Self; 4] = [
        Self::Comments,
        Self::Spelling,
        Self::CommentReflow,
        Self::CapitalizeAfterColon,
    ];

    fn parse(name: &str) -> Result<Self, NormalizeError> {
        match name {
            "frontmatter" => Ok(Self::Frontmatter),
            "headings" => Ok(Self::Headings),
            "attributes" => Ok(Self::Attributes),
            "comments" => Ok(Self::Comments),
            "spelling" => Ok(Self::Spelling),
            "comment_reflow" => Ok(Self::CommentReflow),
            "capitalize_after_colon" => Ok(Self::CapitalizeAfterColon),
            "whitespace" => Ok(Self::Whitespace),
            _ => Err(NormalizeError::InvalidConfiguration(format!(
                "unknown directive rule {name:?}"
            ))),
        }
    }
}

#[derive(Default, Clone, Copy)]
pub struct Context<'a> {
    pub history: Option<&'a FileHistory>,
    pub dictionary: Option<&'a Dictionary>,
    pub module_name: Option<&'a str>,
    pub seen_module_doc: bool,
}

/// Pure normalization with explicitly supplied style, attribution, and dictionary.
pub fn normalize_with_context(
    source: &str,
    width: usize,
    options: Options,
    rules: &NormalizeRules,
    context: &Context<'_>,
) -> Result<NormalizeResult, NormalizeError> {
    let bom = source.starts_with('\u{feff}');
    let mut preserve_bom = !rules.whitespace.remove_bom;
    let source = source.strip_prefix('\u{feff}').unwrap_or(source);
    if width == 0 {
        return Err(NormalizeError::InvalidWidth);
    }
    rules
        .validate()
        .map_err(|e| NormalizeError::InvalidConfiguration(e.to_string()))?;
    let dictionary = if context.dictionary.is_none()
        && rules.spelling != leanck_config::DictionaryRules::default()
    {
        options
            .spelling
            .map(|d| crate::configured_map(d, &rules.spelling, &Default::default()))
            .transpose()
            .map_err(NormalizeError::InvalidConfiguration)?
    } else {
        None
    };
    let context = Context {
        dictionary: context.dictionary.or(dictionary.as_ref()),
        ..*context
    };
    let context = &context;
    let mut out = String::new();
    let mut stats = Stats::default();
    let mut warnings = options.configuration(width, rules).warnings();
    let mut cursor = 0;
    // Lines before `cursor`, counted once per segment to stay linear.
    let mut cursor_line = 0;
    let newlines = |text: &str| text.bytes().filter(|b| *b == b'\n').count();
    let mut disabled = BTreeSet::new();
    let directives = if rules.directives {
        crate::syntax::line_comments(source)
    } else {
        vec![]
    };
    let mut first = true;
    for range in directives {
        let Some(command) = source[range.clone()].strip_prefix("-- leanck:") else {
            continue;
        };
        // Only standalone comments are directives, even if inline text is invalid.
        let start = source[..range.start].rfind('\n').map_or(0, |i| i + 1);
        if !source[start..range.start].trim().is_empty() {
            continue;
        }
        let command = command.trim();
        let (verb, names) = command
            .split_once(char::is_whitespace)
            .unwrap_or((command, ""));
        if !matches!(verb, "off" | "on") {
            return Err(NormalizeError::InvalidConfiguration(format!(
                "unknown leanck directive {verb:?}"
            )));
        }
        let names: Vec<_> = if names.trim().is_empty() {
            vec!["all"]
        } else {
            names.split(',').map(str::trim).collect()
        };
        let line = cursor_line + newlines(&source[cursor..start]) + 1;
        let mut selected = BTreeSet::new();
        for name in names {
            if name == "all" {
                selected.extend(Rule::ALL);
            } else {
                let rule = Rule::parse(name)?;
                if verb == "on" {
                    if rule == Rule::Frontmatter {
                        warnings.push(leanck_config::Warning::new("frontmatter_directive_barrier",
                            "on frontmatter cannot resume frontmatter processing after a directive boundary; place directives after the initial frontmatter").at_line(line));
                    } else {
                        let enabled = match rule {
                            Rule::Headings => options.headings,
                            Rule::Attributes => options.attributes,
                            Rule::Comments => options.comments,
                            Rule::Spelling => options.comments && options.spelling.is_some(),
                            Rule::CommentReflow => options.comments && options.comment_reflow,
                            Rule::CapitalizeAfterColon => {
                                options.comments && options.capitalize_after_colon
                            }
                            Rule::Whitespace => {
                                options.collapse_blank_lines
                                    || rules.whitespace.line_endings != LineEndings::Off
                                    || rules.whitespace.final_newline != FinalNewline::Off
                                    || rules.whitespace.remove_bom
                            }
                            Rule::Frontmatter => unreachable!(),
                        };
                        if !enabled {
                            warnings.push(leanck_config::Warning::new("globally_disabled_directive", format!(
                                "on {name} cannot enable a globally disabled feature; enable its configuration setting or command option first")).at_line(line));
                        }
                    }
                }
                if rule == Rule::Comments {
                    selected.extend(Rule::COMMENT_RULES);
                } else {
                    selected.insert(rule);
                }
            }
        }
        append_segment(
            &source[cursor..start],
            width,
            options,
            rules,
            context,
            &disabled,
            first,
            &mut out,
            &mut stats,
            &mut warnings,
            cursor_line,
            crate::reflow::precedes_guard_msgs(source, start, false),
        )?;
        first = false;
        let end = range.end + usize::from(source.as_bytes().get(range.end) == Some(&b'\n'));
        out.push_str(&source[start..end]);
        cursor_line += newlines(&source[cursor..end]);
        cursor = end;
        if verb == "off" {
            disabled.extend(selected);
        } else {
            disabled.retain(|rule| !selected.contains(rule));
        }
        // A first-line directive also governs the leading BOM. Later directives
        // cannot retroactively change whether that initial character is protected.
        if start == 0 && disabled.contains(&Rule::Whitespace) {
            preserve_bom = true;
        }
    }
    append_segment(
        &source[cursor..],
        width,
        options,
        rules,
        context,
        &disabled,
        first,
        &mut out,
        &mut stats,
        &mut warnings,
        cursor_line,
        false,
    )?;
    if !disabled.contains(&Rule::Whitespace) {
        if options.collapse_blank_lines {
            // Blank lines at the end of the file separate nothing.
            let mut end = out.len();
            while out[..end].ends_with('\n') {
                end -= 1 + usize::from(out[..end - 1].ends_with('\r'));
            }
            if end < out.len() {
                let ending = if out[end..].starts_with("\r\n") { 2 } else { 1 };
                out.truncate(end + ending);
            }
        }
        let final_newline = match rules.whitespace.final_newline {
            FinalNewline::Off => source.ends_with('\n'),
            FinalNewline::Ensure => true,
            FinalNewline::Remove => false,
        };
        if final_newline {
            if !out.ends_with('\n') {
                // A lone carriage return before the new line ending would be
                // trailing whitespace on its line, which the next run removes.
                if out.ends_with('\r') {
                    out.pop();
                }
                // Unchanged line endings continue the style of the last line.
                let crlf = match out.rfind('\n') {
                    Some(end) => out[..end].ends_with('\r'),
                    None => source.contains("\r\n"),
                };
                if rules.whitespace.line_endings == LineEndings::Crlf
                    || (rules.whitespace.line_endings == LineEndings::Off && crlf)
                {
                    out.push_str("\r\n");
                } else {
                    out.push('\n');
                }
            }
        } else {
            out.truncate(out.trim_end_matches(['\r', '\n']).len());
        }
    }
    if bom && preserve_bom {
        out.insert(0, '\u{feff}');
    }
    warnings.sort_by_key(|warning| warning.line.unwrap_or(0));
    Ok(NormalizeResult {
        output: out,
        stats,
        warnings,
    })
}
#[allow(clippy::too_many_arguments)]
fn append_segment(
    source: &str,
    width: usize,
    mut options: Options,
    rules: &NormalizeRules,
    context: &Context<'_>,
    disabled: &BTreeSet<Rule>,
    first: bool,
    out: &mut String,
    stats: &mut Stats,
    warnings: &mut Vec<leanck_config::Warning>,
    line_offset: usize,
    guard_msgs_after: bool,
) -> Result<(), NormalizeError> {
    if source.is_empty() {
        return Ok(());
    }
    if disabled.len() == Rule::ALL.len() {
        out.push_str(source);
        return Ok(());
    }
    let mut context = *context;
    if !first {
        options.frontmatter.enabled = false;
        context.history = None;
        context.module_name = None;
        context.seen_module_doc = true;
    }
    for rule in disabled {
        match rule {
            Rule::Frontmatter => options.frontmatter.enabled = false,
            Rule::Headings => options.headings = false,
            Rule::Attributes => options.attributes = false,
            Rule::Comments => options.linkify_urls = false,
            Rule::Spelling => options.spelling = None,
            Rule::CommentReflow => options.comment_reflow = false,
            Rule::CapitalizeAfterColon => options.capitalize_after_colon = false,
            Rule::Whitespace => options.collapse_blank_lines = false,
        }
    }
    let comment_layout = !disabled.contains(&Rule::Comments);
    let result = crate::reflow::normalize_configured(
        source,
        width,
        options,
        rules,
        &context,
        comment_layout,
        guard_msgs_after,
    )?;
    let style = if disabled.contains(&Rule::Whitespace) {
        LineEndings::Off
    } else {
        rules.whitespace.line_endings
    };
    if style == LineEndings::Crlf || (style == LineEndings::Off && source.contains("\r\n")) {
        out.push_str(&result.output.replace('\n', "\r\n"));
    } else if style == LineEndings::Lf {
        // A carriage return left before a line feed would read as a CRLF
        // line ending in the next run, which removes it; remove it now.
        for line in result.output.split_inclusive('\n') {
            match line.strip_suffix('\n') {
                Some(line) => {
                    out.push_str(line.trim_end_matches('\r'));
                    out.push('\n');
                }
                None => out.push_str(line),
            }
        }
    } else {
        out.push_str(&result.output);
    }
    stats.add(&result.stats);
    warnings.extend(result.warnings.into_iter().map(|mut warning| {
        warning.line = warning.line.map(|line| line + line_offset);
        warning
    }));
    Ok(())
}
