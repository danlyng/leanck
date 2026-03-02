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
            let indent = leading_whitespace(lines[i]);
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

pub(crate) fn configured(
    source: &str,
    rules: &leanck_config::AttributeRules,
    width: usize,
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
            .is_some_and(|s| s.trim_start().starts_with("@["))
        {
            out.push(lines[i].to_owned());
            i += 1;
            continue;
        }
        let start = i;
        let indent = leading_whitespace(lines[i]);
        let mut attrs = Vec::new();
        let mut conflicting = false;
        let mut declaration = None;
        loop {
            let mut rest = lines[i].trim_start();
            let mut masked = code[i].trim_start();
            while masked.starts_with("@[") {
                let mut depth = 0;
                let mut end = None;
                for (j, c) in masked.char_indices().skip(1) {
                    if c == '[' {
                        depth += 1;
                    } else if c == ']' {
                        depth -= 1;
                        if depth == 0 {
                            end = Some(j + 1);
                            break;
                        }
                    }
                }
                let Some(end) = end else {
                    break;
                };
                let group = &rest[..end];
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
                rest = rest[end..].trim_start();
                masked = masked[end..].trim_start();
            }
            if starts_declaration(masked) {
                declaration = Some(rest.to_owned());
                i += 1;
                break;
            }
            if masked.trim().is_empty() && !rest.trim().is_empty() {
                i += 1;
                break;
            }
            if !masked.is_empty() || i + 1 >= lines.len() {
                i += 1;
                break;
            }
            i += 1;
            if !code[i].trim_start().starts_with("@[") {
                if starts_declaration(code[i]) {
                    declaration = Some(lines[i].trim_start().to_owned());
                    i += 1;
                }
                break;
            }
        }
        let Some(declaration) = declaration else {
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
        let long = rules.above_when_long && candidate.chars().count() > width;
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
    (out.join("\n"), changed, warnings)
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
