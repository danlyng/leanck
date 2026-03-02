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
