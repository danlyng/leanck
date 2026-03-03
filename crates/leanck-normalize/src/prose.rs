//! Source-position based Markdown text transformations; code/math/URLs stay exact.
use markdown::mdast::Node;

pub(crate) fn transform(text: &str, mut f: impl FnMut(&str) -> String) -> String {
    transform_with_offsets(text, |part, _| f(part))
}
pub(crate) fn transform_with_offsets(
    text: &str,
    mut f: impl FnMut(&str, usize) -> String,
) -> String {
    let Ok(root) = markdown::to_mdast(
        text,
        &markdown::ParseOptions {
            constructs: markdown::Constructs {
                math_text: true,
                math_flow: true,
                gfm_autolink_literal: true,
                ..markdown::Constructs::default()
            },
            ..Default::default()
        },
    ) else {
        return text.to_owned();
    };
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
    out
}

/// Render math as temporary atomic code spans/blocks so the reflow dependency
/// cannot wrap or rewrite it. Restore the original math bytes afterward.
pub(crate) fn reflow_protected(
    text: &str,
    options: markdown_reflow::Options,
) -> Result<String, String> {
    let root = markdown::to_mdast(
        text,
        &markdown::ParseOptions {
            constructs: markdown::Constructs {
                math_text: true,
                math_flow: true,
                ..Default::default()
            },
            ..Default::default()
        },
    )
    .map_err(|e| e.to_string())?;
    // In disabled-width mode placeholders do not influence wrapping. Protect
    // explicit links from being autolinked again inside their own labels.
    let protect_links = options.linkify_urls
        && [
            options.widths.first,
            options.widths.middle,
            options.widths.last,
            options.widths.single,
        ]
        .contains(&markdown_reflow::ReflowWidth::Disabled);
    let mut link_prefix = "leanck-protected-link-".to_owned();
    while text.contains(&link_prefix) {
        link_prefix.push('x');
    }
    fn collect(
        node: &Node,
        text: &str,
        out: &mut Vec<(std::ops::Range<usize>, String, String)>,
        protect_links: bool,
        link_prefix: &str,
    ) {
        if protect_links && matches!(node, Node::Link(_) | Node::LinkReference(_)) {
            if let Some(p) = node.position() {
                let raw = &text[p.start.offset..p.end.offset];
                let wrapped = format!("`{link_prefix}{}`", out.len());
                out.push((p.start.offset..p.end.offset, wrapped, raw.into()));
            }
        } else if matches!(node, Node::InlineMath(_) | Node::Math(_)) {
            if let Some(p) = node.position() {
                let raw = &text[p.start.offset..p.end.offset];
                let wrapped = if matches!(node, Node::Math(_)) {
                    format!("```leanck-math\n{raw}\n```")
                } else {
                    let n = raw.chars().filter(|c| *c == '`').count() + 1;
                    let fence = "`".repeat(n);
                    format!("{fence}{raw}{fence}")
                };
                out.push((p.start.offset..p.end.offset, wrapped, raw.into()));
            }
        } else if let Some(children) = node.children() {
            for child in children {
                collect(child, text, out, protect_links, link_prefix);
            }
        }
    }
    let mut replacements = Vec::new();
    collect(&root, text, &mut replacements, protect_links, &link_prefix);
    let mut protected = text.to_owned();
    for (range, wrapped, _) in replacements.iter().rev() {
        protected.replace_range(range.clone(), wrapped);
    }
    let mut rendered = markdown_reflow::reflow(&protected, options)?;
    for (_, wrapped, original) in replacements {
        rendered = rendered.replace(&wrapped, &original);
    }
    Ok(rendered)
}

pub(crate) fn heading_titles(text: &str, mut f: impl FnMut(&str) -> String) -> String {
    let Ok(root) = markdown::to_mdast(
        text,
        &markdown::ParseOptions {
            constructs: markdown::Constructs {
                math_text: true,
                math_flow: true,
                ..Default::default()
            },
            ..Default::default()
        },
    ) else {
        return text.to_owned();
    };
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
    out
}

pub(crate) fn protected_lines(text: &str) -> std::collections::HashSet<usize> {
    let mut out = std::collections::HashSet::new();
    let Ok(root) = markdown::to_mdast(
        text,
        &markdown::ParseOptions {
            constructs: markdown::Constructs {
                math_text: true,
                math_flow: true,
                ..Default::default()
            },
            ..Default::default()
        },
    ) else {
        return out;
    };
    fn collect(node: &Node, out: &mut std::collections::HashSet<usize>) {
        if matches!(
            node,
            Node::Code(_) | Node::Math(_) | Node::Html(_) | Node::InlineCode(_)
        ) {
            if let Some(p) = node.position() {
                out.extend(p.start.line - 1..p.end.line);
            }
        } else if let Some(children) = node.children() {
            for child in children {
                collect(child, out);
            }
        }
    }
    collect(&root, &mut out);
    out
}
