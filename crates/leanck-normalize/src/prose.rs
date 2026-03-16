//! Source-position based Markdown text transformations; code/math/URLs stay exact.
use markdown::mdast::Node;

/// Parse Markdown, treating a parser panic like a parse failure.
fn parse(text: &str, constructs: markdown::Constructs) -> Option<Node> {
    let options = markdown::ParseOptions {
        constructs,
        ..Default::default()
    };
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        markdown::to_mdast(text, &options)
    }))
    .ok()?
    .ok()
}

fn math_constructs() -> markdown::Constructs {
    markdown::Constructs {
        math_text: true,
        math_flow: true,
        ..Default::default()
    }
}

/// Apply `f` to every text node. `None` means the text could not be parsed.
pub(crate) fn transform(text: &str, mut f: impl FnMut(&str) -> String) -> Option<String> {
    transform_with_offsets(text, |part, _| f(part))
}
pub(crate) fn transform_with_offsets(
    text: &str,
    mut f: impl FnMut(&str, usize) -> String,
) -> Option<String> {
    let root = parse(
        text,
        markdown::Constructs {
            gfm_autolink_literal: true,
            ..math_constructs()
        },
    )?;
    fn ranges(node: &Node, text: &str, out: &mut Vec<std::ops::Range<usize>>) {
        if let Node::Link(link) = node
            && link.children.len() == 1
            && matches!(&link.children[0],Node::Text(t) if t.value == link.url || link.url == format!("mailto:{}",t.value))
        {
            return;
        }
        if let Node::Text(t) = node {
            if let Some(p) = &t.position
                && text.is_char_boundary(p.start.offset)
                && text.is_char_boundary(p.end.offset)
            {
                out.push(p.start.offset..p.end.offset);
            }
        } else if let Some(children) = node.children() {
            for child in children {
                ranges(child, text, out);
            }
        }
    }
    let mut spans = Vec::new();
    ranges(&root, text, &mut spans);
    let mut out = text.to_owned();
    for range in spans.into_iter().rev() {
        let replacement = f(&text[range.clone()], range.start);
        out.replace_range(range, &replacement);
    }
    Some(out)
}

/// Turn bare URLs into Markdown links in place, leaving every other byte. A
/// URL stays bare where brackets would change how its text or its neighbors
/// parse. `None` means the text could not be parsed.
pub(crate) fn linkify(text: &str) -> Option<String> {
    let root = parse(
        text,
        markdown::Constructs {
            gfm_autolink_literal: true,
            ..math_constructs()
        },
    )?;
    let mut links = Vec::new();
    let mut stack = vec![&root];
    while let Some(node) = stack.pop() {
        if let Node::Link(link) = node {
            // Explicit links and `<...>` autolinks start with their delimiter;
            // the parser may nest a literal URL inside them, so stop here.
            if let Some(p) = node.position()
                && !text[p.start.offset..].starts_with(['<', '['])
            {
                let raw = &text[p.start.offset..p.end.offset];
                let before = text[..p.start.offset].chars().next_back();
                let after = text[p.end.offset..].chars().next();
                let flanking = |c: Option<char>| c.is_some_and(|c| matches!(c, '*' | '_' | '~'));
                if !before.is_some_and(|c| matches!(c, '!' | '\\' | ']'))
                    && !after.is_some_and(|c| matches!(c, '(' | '['))
                    && !flanking(before)
                    && !flanking(after)
                    && survives_as_link(raw, &link.url)
                {
                    links.push((p.start.offset..p.end.offset, link.url.clone()));
                }
            }
        } else if let Some(children) = node.children() {
            stack.extend(children);
        }
    }
    links.sort_by_key(|(range, _)| range.start);
    let mut out = text.to_owned();
    for (range, url) in links.into_iter().rev() {
        let label = &text[range.clone()];
        out.replace_range(range, &format!("[{label}]({url})"));
    }
    Some(out)
}

/// Whether `[label](url)` parses as one link to `url` whose text is `label`.
fn survives_as_link(label: &str, url: &str) -> bool {
    let Some(root) = parse(&format!("[{label}]({url})"), math_constructs()) else {
        return false;
    };
    let Some([Node::Paragraph(paragraph)]) = root.children().map(Vec::as_slice) else {
        return false;
    };
    matches!(
        paragraph.children.as_slice(),
        [Node::Link(link)] if link.url == url
            && matches!(link.children.as_slice(), [Node::Text(text)] if text.value == label)
    )
}

/// Placeholder digits come from the Private Use Area, which Markdown treats as
/// plain word characters. One digit per column keeps the measured width exact.
const DIGIT_BASE: u32 = 0xE000;
const DIGITS: u32 = 0x1900;

fn is_digit(c: char) -> bool {
    (DIGIT_BASE..DIGIT_BASE + DIGITS).contains(&u32::from(c))
}

/// Encode `index` in exactly `len` digits.
fn digits(mut index: usize, len: usize) -> Option<String> {
    let mut out = vec!['\0'; len];
    for slot in out.iter_mut().rev() {
        let digit = u32::try_from(index % DIGITS as usize).ok()?;
        *slot = char::from_u32(DIGIT_BASE + digit)?;
        index /= DIGITS as usize;
    }
    (index == 0).then(|| out.into_iter().collect())
}

/// Math hidden from the reflow dependency, which does not parse math.
enum Math {
    /// Inline math, replaced by its fences around an opaque run of digits of
    /// the same width, so wrapping and emphasis flanking see the same text.
    Inline { fence: String, original: String },
    /// Display math, replaced by a fenced code block that keeps its lines.
    Display { open: String, close: String },
}

/// Render math as temporary atomic placeholders so the reflow dependency
/// cannot wrap or rewrite it, then restore the original math bytes.
///
/// Returns `Ok(None)` when the math cannot be protected and restored exactly,
/// including when the Markdown parser panics; callers then keep the comment.
pub(crate) fn reflow_protected(
    text: &str,
    options: markdown_reflow::Options,
) -> Result<Option<String>, String> {
    let Some(root) = parse(text, math_constructs()) else {
        return Ok(None);
    };
    let mut marker = "leanck-math-".to_owned();
    while text.contains(&marker) {
        marker.insert(0, 'x');
    }
    fn collect<'a>(node: &'a Node, out: &mut Vec<&'a Node>) {
        if matches!(node, Node::InlineMath(_) | Node::Math(_)) {
            out.push(node);
        } else if let Some(children) = node.children() {
            for child in children {
                collect(child, out);
            }
        }
    }
    let mut nodes = Vec::new();
    collect(&root, &mut nodes);
    if !nodes.is_empty() && text.chars().any(is_digit) {
        return Ok(None);
    }
    let mut maths = Vec::new();
    let mut protected = String::with_capacity(text.len());
    let mut copied = 0;
    for node in nodes {
        let Some(position) = node.position() else {
            return Ok(None);
        };
        let range = position.start.offset..position.end.offset;
        let raw = &text[range.clone()];
        let index = maths.len();
        let (math, placeholder) = match node {
            Node::InlineMath(math) => {
                let fence: String = raw.chars().take_while(|c| *c == '$').collect();
                let original = if raw.contains('\n') {
                    // Joined lines must not carry container prefixes along.
                    // Like code spans, line endings in math are spaces, and
                    // TeX ignores the indentation around them.
                    let inner = &raw[fence.len()..raw.len() - fence.len()];
                    let pad = if inner.len() > 2
                        && inner.starts_with(' ')
                        && inner.ends_with(' ')
                        && !inner.trim().is_empty()
                    {
                        " "
                    } else {
                        ""
                    };
                    let lines: Vec<&str> = math.value.split('\n').collect();
                    let value = lines
                        .iter()
                        .enumerate()
                        .map(|(i, line)| {
                            let line = if i > 0 { line.trim_start() } else { line };
                            if i + 1 < lines.len() {
                                line.trim_end()
                            } else {
                                line
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    format!("{fence}{pad}{value}{pad}{fence}")
                } else {
                    raw.to_owned()
                };
                let width = original.chars().count();
                let Some(body) = width
                    .checked_sub(2 * fence.len())
                    .filter(|len| *len > 0)
                    .and_then(|len| digits(index, len))
                else {
                    return Ok(None);
                };
                let placeholder = format!("{fence}{body}{fence}");
                (Math::Inline { fence, original }, placeholder)
            }
            Node::Math(_) => {
                let raw_lines: Vec<&str> = raw.split('\n').collect();
                let first = raw_lines[0];
                let last = raw_lines[raw_lines.len() - 1].trim_end();
                let opening = first.trim_start();
                let open_run = opening.chars().take_while(|c| *c == '$').count();
                let close_run = last.len() - last.trim_end_matches('$').len();
                let head = &last[..last.len() - close_run];
                // An unclosed block runs to the end of its container; leave it.
                if raw_lines.len() < 2
                    || close_run < open_run
                    || !head.chars().all(|c| matches!(c, ' ' | '\t' | '>'))
                {
                    return Ok(None);
                }
                let longest = raw.split(|c| c != '~').map(str::len).max().unwrap_or(0);
                let fence = "~".repeat(longest.max(2) + 1);
                let indent = &first[..first.len() - opening.len()];
                let mut placeholder = format!("{indent}{fence}{marker}{index}\n");
                for line in &raw_lines[1..raw_lines.len() - 1] {
                    placeholder.push_str(line);
                    placeholder.push('\n');
                }
                placeholder.push_str(head);
                placeholder.push_str(&fence);
                let math = Math::Display {
                    open: opening.trim_end().to_owned(),
                    close: last[head.len()..].to_owned(),
                };
                (math, placeholder)
            }
            _ => unreachable!("only math nodes are collected"),
        };
        protected.push_str(&text[copied..range.start]);
        protected.push_str(&placeholder);
        copied = range.end;
        maths.push(math);
    }
    protected.push_str(&text[copied..]);
    let rendered = std::panic::catch_unwind(|| markdown_reflow::reflow(&protected, options));
    let Ok(rendered) = rendered else {
        return Ok(None);
    };
    let rendered = rendered?;
    if maths.is_empty() {
        return Ok(Some(rendered));
    }
    Ok(restore_display(&rendered, &maths, &marker).and_then(|r| restore_inline(&r, &maths)))
}

/// Replace each display-math code block's fences with the original `$$` lines.
fn restore_display(rendered: &str, maths: &[Math], marker: &str) -> Option<String> {
    let mut lines: Vec<String> = rendered.split('\n').map(str::to_owned).collect();
    let mut found = vec![false; maths.len()];
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i].trim_end();
        let Some(at) = line.find(marker) else {
            i += 1;
            continue;
        };
        let index: usize = line[at + marker.len()..].parse().ok()?;
        let Some(Math::Display { open, close }) = maths.get(index) else {
            return None;
        };
        let head = &line[..at];
        let fence_char = head.chars().next_back()?;
        let fence_len = head.len() - head.trim_end_matches(fence_char).len();
        if found[index] || !matches!(fence_char, '`' | '~') || fence_len < 3 {
            return None;
        }
        found[index] = true;
        lines[i] = format!("{}{open}", &head[..head.len() - fence_len]);
        let end = (i + 1..lines.len()).find(|&j| {
            let t = lines[j].trim_end();
            let body = t.trim_start_matches([' ', '>']);
            body.len() >= fence_len && body.chars().all(|c| c == fence_char)
        })?;
        let t = lines[end].trim_end();
        let prefix = &t[..t.len() - t.trim_start_matches([' ', '>']).len()];
        lines[end] = format!("{prefix}{close}");
        i = end + 1;
    }
    let expected = maths
        .iter()
        .zip(&found)
        .all(|(math, found)| matches!(math, Math::Inline { .. }) || *found);
    expected.then(|| lines.join("\n"))
}

/// Replace each inline placeholder, which must appear exactly once and intact.
fn restore_inline(rendered: &str, maths: &[Math]) -> Option<String> {
    let mut out = String::with_capacity(rendered.len());
    let mut found = vec![false; maths.len()];
    let mut copied = 0;
    let mut chars = rendered.char_indices().peekable();
    while let Some((start, c)) = chars.next() {
        if !is_digit(c) {
            continue;
        }
        let mut index = (u32::from(c) - DIGIT_BASE) as usize;
        let mut len = 1;
        let mut end = start + c.len_utf8();
        while let Some(&(at, d)) = chars.peek()
            && is_digit(d)
        {
            index = index
                .checked_mul(DIGITS as usize)?
                .checked_add((u32::from(d) - DIGIT_BASE) as usize)?;
            len += 1;
            end = at + d.len_utf8();
            chars.next();
        }
        let Some(Math::Inline { fence, original }) = maths.get(index) else {
            return None;
        };
        let width = original.chars().count();
        if found[index]
            || width != len + 2 * fence.len()
            || start < copied + fence.len()
            || !rendered[..start].ends_with(fence.as_str())
            || !rendered[end..].starts_with(fence.as_str())
        {
            return None;
        }
        found[index] = true;
        out.push_str(&rendered[copied..start - fence.len()]);
        out.push_str(original);
        copied = end + fence.len();
    }
    out.push_str(&rendered[copied..]);
    let expected = maths
        .iter()
        .zip(&found)
        .all(|(math, found)| matches!(math, Math::Display { .. }) || *found);
    expected.then_some(out)
}

/// Whether two Markdown texts have the same structure, apart from whitespace in
/// text and the spelling of equivalent syntax. A parse failure counts as different.
pub(crate) fn same_structure(a: &str, b: &str) -> bool {
    a == b || structure(a).is_some_and(|sa| structure(b).is_some_and(|sb| sa == sb))
}

/// A flat signature of the Markdown structure with whitespace-collapsed text.
fn structure(text: &str) -> Option<Vec<String>> {
    let root = parse(
        text,
        markdown::Constructs {
            math_text: true,
            math_flow: true,
            ..markdown::Constructs::gfm()
        },
    )?;
    fn collapse(text: &str) -> String {
        text.split_whitespace().collect::<Vec<_>>().join(" ")
    }
    fn flush(out: &mut Vec<String>, text: &mut String) {
        let collapsed = collapse(text);
        if !collapsed.is_empty() {
            out.push(collapsed);
        }
        text.clear();
    }
    fn walk(node: &Node, out: &mut Vec<String>, text: &mut String, in_link: bool) {
        let tag = match node {
            Node::Text(t) => {
                text.push_str(&t.value);
                return;
            }
            Node::Root(_) => None,
            // The parser can nest a literal URL's link inside an explicit one.
            Node::Link(_) if in_link => None,
            Node::InlineCode(c) => Some(format!("code:{}", collapse(&c.value))),
            Node::InlineMath(m) => Some(format!("math:{}", collapse(&m.value))),
            Node::Code(c) => Some(format!("block:{:?}:{}", c.lang, c.value)),
            Node::Math(m) => Some(format!("display:{}", m.value)),
            Node::Html(h) => Some(format!("html:{}", collapse(&h.value))),
            Node::Heading(h) => Some(format!("h{}", h.depth)),
            Node::List(l) => Some(format!("list:{}:{:?}", l.ordered, l.start)),
            Node::ListItem(item) => Some(format!("item:{:?}", item.checked)),
            Node::Table(t) => Some(format!("table:{:?}", t.align)),
            Node::Link(l) => Some(format!("link:{}:{:?}", l.url, l.title)),
            Node::LinkReference(l) => Some(format!("linkref:{}", l.identifier)),
            Node::Image(i) => Some(format!("image:{}:{}", i.url, collapse(&i.alt))),
            Node::ImageReference(i) => Some(format!("imageref:{}", i.identifier)),
            Node::Definition(d) => Some(format!("def:{}:{}:{:?}", d.identifier, d.url, d.title)),
            Node::FootnoteDefinition(f) => Some(format!("footnote:{}", f.identifier)),
            Node::FootnoteReference(f) => Some(format!("note:{}", f.identifier)),
            other => Some(
                format!("{other:?}")
                    .chars()
                    .take_while(|c| c.is_alphanumeric())
                    .collect(),
            ),
        };
        if let Some(tag) = &tag {
            flush(out, text);
            out.push(tag.clone());
        }
        if let Some(children) = node.children() {
            for child in children {
                walk(child, out, text, in_link || matches!(node, Node::Link(_)));
            }
            if tag.is_some() {
                flush(out, text);
                out.push("end".to_owned());
            }
        }
    }
    let mut out = Vec::new();
    let mut text = String::new();
    walk(&root, &mut out, &mut text, false);
    flush(&mut out, &mut text);
    Some(out)
}

/// Apply `f` to each heading's content. `None` means the text could not be parsed.
pub(crate) fn heading_titles(text: &str, mut f: impl FnMut(&str) -> String) -> Option<String> {
    let root = parse(text, math_constructs())?;
    fn collect(node: &Node, out: &mut Vec<std::ops::Range<usize>>) {
        if let Node::Heading(heading) = node {
            if let (Some(first), Some(last)) = (
                heading.children.first().and_then(Node::position),
                heading.children.last().and_then(Node::position),
            ) {
                out.push(first.start.offset..last.end.offset);
            }
        } else if let Some(children) = node.children() {
            for c in children {
                collect(c, out);
            }
        }
    }
    let mut ranges = Vec::new();
    collect(&root, &mut ranges);
    let mut out = text.to_owned();
    for range in ranges.into_iter().rev() {
        out.replace_range(range.clone(), &f(&text[range]));
    }
    Some(out)
}

/// Zero-based lines of code, math, and HTML, whose blank lines are content.
/// Blank lines after a code or math block are included, since they belong to
/// it when it is unclosed. `None` means the text could not be parsed, so no
/// line is known to be prose.
pub(crate) fn protected_lines(text: &str) -> Option<std::collections::HashSet<usize>> {
    let root = parse(text, math_constructs())?;
    // Markdown also ends lines at a lone carriage return; count line feeds.
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let line_of = |offset: usize| line_starts.partition_point(|start| *start <= offset) - 1;
    let lines: Vec<&str> = text.split('\n').collect();
    let mut out = std::collections::HashSet::new();
    let mut stack = vec![&root];
    while let Some(node) = stack.pop() {
        if matches!(
            node,
            Node::Code(_) | Node::Math(_) | Node::Html(_) | Node::InlineCode(_)
        ) {
            if let Some(p) = node.position() {
                let mut end = line_of(p.end.offset);
                if matches!(node, Node::Code(_) | Node::Math(_)) {
                    while lines
                        .get(end + 1)
                        .is_some_and(|line| line.trim_matches([' ', '\t']).is_empty())
                    {
                        end += 1;
                    }
                }
                out.extend(line_of(p.start.offset)..=end);
            }
        } else if let Some(children) = node.children() {
            stack.extend(children);
        }
    }
    Some(out)
}
