/// Normalize Lean declaration attributes.
///
/// Currently this only moves a standalone `@[simp]` line onto the immediately
/// following declaration line, e.g.
///
/// ```text
/// @[simp]
/// lemma foo : True := trivial
/// ```
///
/// becomes
///
/// ```text
/// @[simp] lemma foo : True := trivial
/// ```
#[allow(dead_code)]
pub fn normalize(source: &str) -> String {
    normalize_with_count(source).0
}

pub fn normalize_with_count(source: &str) -> (String, usize) {
    let mut out = Vec::new();
    let lines: Vec<&str> = source.lines().collect();
    let mut moved = 0;
    let mut i = 0;

    while i < lines.len() {
        if is_standalone_simp(lines[i]) && i + 1 < lines.len() && starts_declaration(lines[i + 1]) {
            let indent = leading_whitespace(lines[i + 1]);
            let next = lines[i + 1].trim_start();
            out.push(format!("{indent}@[simp] {next}"));
            moved += 1;
            i += 2;
        } else {
            out.push(lines[i].to_string());
            i += 1;
        }
    }

    (out.join("\n"), moved)
}

fn is_standalone_simp(line: &str) -> bool {
    line.trim() == "@[simp]"
}

fn starts_declaration(line: &str) -> bool {
    let mut rest = line.trim_start();
    if rest.is_empty() || rest.starts_with("--") || rest.starts_with("/-") || rest.starts_with("@[")
    {
        return false;
    }

    loop {
        let Some((token, after)) = split_first_token(rest) else {
            return false;
        };

        if is_declaration_keyword(token) {
            return true;
        }

        if is_declaration_modifier(token) {
            rest = after.trim_start();
            continue;
        }

        return false;
    }
}

fn split_first_token(s: &str) -> Option<(&str, &str)> {
    let trimmed = s.trim_start();
    if trimmed.is_empty() {
        return None;
    }
    let end = trimmed
        .char_indices()
        .find(|(_, c)| c.is_whitespace())
        .map_or(trimmed.len(), |(i, _)| i);
    Some((&trimmed[..end], &trimmed[end..]))
}

fn is_declaration_keyword(token: &str) -> bool {
    matches!(
        token,
        "abbrev"
            | "axiom"
            | "class"
            | "def"
            | "example"
            | "inductive"
            | "instance"
            | "lemma"
            | "opaque"
            | "structure"
            | "theorem"
    )
}

fn is_declaration_modifier(token: &str) -> bool {
    matches!(
        token,
        "private" | "protected" | "public" | "noncomputable" | "unsafe" | "partial"
    )
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

/// True when a line starts with an attribute group in code rather than inside a
/// comment or string, with no comment before it as in `/- c -/ @[simp]`.
fn is_attribute_line(line: &str, masked: &str) -> bool {
    let masked = masked.trim_start();
    masked.starts_with("@[") && line.trim_start().len() == masked.len()
}

/// True when an attribute group carries a comment, as in `@[to_dual /-- doc -/]`.
fn contains_comment(group: &str) -> bool {
    let bytes = group.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/-") || bytes[i..].starts_with(b"--") {
            return true;
        }
        i = crate::syntax::literal_end(group, i).unwrap_or(i + 1);
    }
    false
}

/// Apply attribute placement policies to declaration attribute blocks.
///
/// Blocks whose attribute lines contain a comment are left unchanged. The
/// comment pass runs later and may reshape such a comment, for example collapsing
/// a multiline doc comment inside `@[to_dual /-- ... -/]` onto one line, so
/// placement must not depend on comment layout or a second run would move what
/// the first left in place.
///
/// The declaration line keeps its own indentation; attributes join it or sit
/// above it at that column, so `  @[simp]\ntheorem` becomes `@[simp] theorem`.
pub(crate) fn configured(
    source: &str,
    rules: &leanck_config::AttributeRules,
    width: leanck_config::NormalizeWidth,
) -> (String, usize, Vec<leanck_config::Warning>) {
    use leanck_config::AttributePlacement as P;
    let lines: Vec<_> = source.lines().collect();
    let mask = crate::syntax::code_mask(source);
    let code: Vec<_> = mask.lines().collect();
    let mut out = Vec::new();
    let mut i = 0;
    let mut changed = 0;
    let mut warnings = Vec::new();
    while i < lines.len() {
        if !code
            .get(i)
            .is_some_and(|masked| is_attribute_line(lines[i], masked))
        {
            out.push(lines[i].to_owned());
            i += 1;
            continue;
        }
        let start = i;
        let mut attrs = Vec::new();
        let mut conflicting = false;
        let mut commented = false;
        let mut declaration = None;
        loop {
            let (line, masked) = (lines[i], code[i]);
            // The source and its mask share byte offsets, so one position walks
            // both; the mask hides brackets inside strings and comments.
            let mut pos = line.len() - line.trim_start().len();
            while masked[pos..].starts_with("@[") {
                let mut depth = 0;
                let mut end = None;
                for (j, c) in masked[pos..].char_indices().skip(1) {
                    if c == '[' {
                        depth += 1;
                    } else if c == ']' {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(pos + j + 1);
                            break;
                        }
                    }
                }
                let Some(end) = end else {
                    break;
                };
                let group = &line[pos..end];
                commented |= contains_comment(group);
                let mut positions = group[2..group.len() - 1]
                    .split(',')
                    .filter_map(|s| s.split_whitespace().next())
                    .map(|name| {
                        rules
                            .overrides
                            .get(name)
                            .copied()
                            .unwrap_or(rules.placement)
                    });
                let placement = positions.next().unwrap_or(P::Off);
                let placement = if positions.all(|p| p == placement) {
                    placement
                } else {
                    conflicting = true;
                    P::Off
                };
                attrs.push((group.to_owned(), placement));
                // Masked whitespace that is not source whitespace is a comment.
                let gap = masked[end..].len() - masked[end..].trim_start().len();
                commented |= !line[end..end + gap].trim().is_empty();
                pos = end + gap;
            }
            let rest = &masked[pos..];
            if starts_declaration(rest) {
                declaration = Some((leading_whitespace(line), &line[pos..]));
                i += 1;
                break;
            }
            if !rest.is_empty() || i + 1 >= lines.len() {
                i += 1;
                break;
            }
            i += 1;
            if !is_attribute_line(lines[i], code[i]) {
                // A comment before the declaration keyword is an intervening comment.
                if starts_declaration(code[i])
                    && lines[i].trim_start().len() == code[i].trim_start().len()
                {
                    declaration = Some((leading_whitespace(lines[i]), lines[i].trim_start()));
                    i += 1;
                }
                break;
            }
        }
        let Some((indent, declaration)) = declaration.filter(|_| !commented) else {
            out.extend(lines[start..i].iter().map(|s| s.to_string()));
            continue;
        };
        let mut seen_same = false;
        let mut safe = true;
        for (_, p) in &attrs {
            match p {
                P::Off => safe = false,
                P::SameLine => seen_same = true,
                P::Above if seen_same => {
                    safe = false;
                    conflicting = true;
                }
                _ => {}
            }
        }
        if !safe {
            if conflicting {
                warnings.push(leanck_config::Warning::new("conflicting_attribute_placement", format!(
                    "conflicting placement policies for {} cannot be applied without splitting groups or changing attribute order; this declaration's attributes are left unchanged", attrs.iter().map(|(text, _)| text.as_str()).collect::<Vec<_>>().join(" "))).at_line(start + 1));
            }
            out.extend(lines[start..i].iter().map(|s| s.to_string()));
            continue;
        }
        let candidate = format!(
            "{indent}{} {declaration}",
            attrs
                .iter()
                .filter(|(_, p)| *p == P::SameLine)
                .map(|(s, _)| s.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        );
        // Comment reflow lays out a doc comment on the declaration line to the
        // width itself, moving the code after it to its last line, so only the
        // text before it counts; otherwise the next run could decide
        // differently once the comment is wrapped. Other block comments keep
        // their layout and count like code.
        let doc_comment = crate::syntax::spans(declaration)
            .into_iter()
            .find(|(kind, range)| {
                *kind == crate::syntax::Span::BlockComment
                    && (declaration[range.start..].starts_with("/--")
                        || declaration[range.start..].starts_with("/-!"))
            })
            .map(|(_, range)| range.start);
        let measured = doc_comment.map_or(candidate.as_str(), |k| {
            candidate[..candidate.len() - declaration.len() + k].trim_end()
        });
        let long = rules.above_when_long && !width.fits(measured.chars().count());
        let mut rendered = Vec::new();
        for (attr, p) in &attrs {
            if *p == P::Above || long {
                rendered.push(format!("{indent}{attr}"));
            }
        }
        if long || !attrs.iter().any(|(_, p)| *p == P::SameLine) {
            rendered.push(format!("{indent}{declaration}"));
        } else {
            rendered.push(candidate);
        }
        if rendered.join("\n") != lines[start..i].join("\n") {
            changed += attrs.len();
        }
        out.extend(rendered);
    }
    let mut out = out.join("\n");
    // lines()/join() drop the final line ending, which would also drop a
    // trailing blank line, such as one before a directive.
    if source.ends_with('\n') {
        out.push('\n');
    }
    (out, changed, warnings)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn moves_simp_onto_lemma_line() {
        let src = "@[simp]\nlemma my_simp_lemma : true := trivial\n";
        assert_eq!(
            normalize(src),
            "@[simp] lemma my_simp_lemma : true := trivial"
        );
    }

    #[test]
    fn preserves_indentation() {
        let src = "  @[simp]\n  lemma foo : True := trivial\n";
        assert_eq!(normalize(src), "  @[simp] lemma foo : True := trivial");
    }

    #[test]
    fn uses_declaration_indentation() {
        let src = "  @[simp]\nlemma foo : True := trivial\n";
        assert_eq!(normalize(src), "@[simp] lemma foo : True := trivial");
    }

    #[test]
    fn handles_declaration_modifiers() {
        let src = "@[simp]\nprivate noncomputable def foo := 1\n";
        assert_eq!(normalize(src), "@[simp] private noncomputable def foo := 1");
    }

    #[test]
    fn leaves_non_declarations_unchanged() {
        let src = "@[simp]\n-- lemma foo\n";
        assert_eq!(normalize(src), "@[simp]\n-- lemma foo");
    }

    #[test]
    fn does_not_cross_other_attributes() {
        let src = "@[simp]\n@[foo]\nlemma bar : True := trivial\n";
        assert_eq!(
            normalize(src),
            "@[simp]\n@[foo]\nlemma bar : True := trivial"
        );
    }
}
