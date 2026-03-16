//! Small lexical scanner used to keep formatting out of Lean strings and comments.
//! It follows Lean's tokenizer (`Lean.Parser.Basic` and `Lean.Parser.StrInterpolation`)
//! only as far as needed to find comments, literals, and escaped identifiers.
use std::ops::Range;

/// A source span inside which Lean does not recognize comment delimiters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Span {
    LineComment,
    BlockComment,
    /// A string, raw string, interpolated string, or character literal.
    Literal,
    /// A `«…»` identifier escape, which is code but may contain comment delimiters.
    EscapedIdent,
}

/// Every comment, literal, and escaped identifier in source order.
pub(crate) fn spans(source: &str) -> Vec<(Span, Range<usize>)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < source.len() {
        if let Some((kind, end)) = span_at(source, i) {
            out.push((kind, i..end));
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// Replace strings and comments with spaces, preserving byte offsets and newlines.
pub(crate) fn code_mask(source: &str) -> String {
    let mut out = source.as_bytes().to_vec();
    for (kind, range) in spans(source) {
        if kind == Span::EscapedIdent {
            continue;
        }
        for c in &mut out[range] {
            if *c != b'\n' && *c != b'\r' {
                *c = b' ';
            }
        }
    }
    String::from_utf8(out).expect("masking replaces whole UTF-8 sequences with spaces")
}

/// Offsets of actual Lean line comments, excluding strings and nested block comments.
pub(crate) fn line_comments(source: &str) -> Vec<Range<usize>> {
    spans(source)
        .into_iter()
        .filter(|(kind, _)| *kind == Span::LineComment)
        .map(|(_, range)| range)
        .collect()
}

/// End of a block comment, including nested blocks. For historical comment
/// snippets containing a literal unmatched `/-`, retain the first closing delimiter.
pub(crate) fn block_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    // Lean consumes the character after `/-` before looking for delimiters, so
    // `/--/` and `/-/` open comments rather than closing them.
    let mut i = start + 2 + source.get(start + 2..)?.chars().next()?.len_utf8();
    let mut depth = 1;
    let mut first = None;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"/-") {
            depth += 1;
            i += 2;
        } else if bytes[i..].starts_with(b"-/") {
            first.get_or_insert(i + 2);
            depth -= 1;
            i += 2;
            if depth == 0 {
                return Some(i);
            }
        } else {
            i += 1;
        }
    }
    first
}

/// End of a literal or escaped identifier starting here: an ordinary, raw, or
/// interpolated string, a character literal, or a `«…»` identifier escape.
/// Comment delimiters inside these spans are not comments.
pub(crate) fn literal_end(source: &str, start: usize) -> Option<usize> {
    token_end(source, start).map(|(_, end)| end)
}

pub(crate) fn literal_lines(source: &str) -> std::collections::HashSet<usize> {
    let b = source.as_bytes();
    let mut out = std::collections::HashSet::new();
    let mut line = 0;
    let mut last = 0;
    for (kind, range) in spans(source) {
        line += b[last..range.start].iter().filter(|c| **c == b'\n').count();
        let n = b[range.clone()].iter().filter(|c| **c == b'\n').count();
        if matches!(kind, Span::Literal | Span::EscapedIdent) {
            out.extend(line..=line + n);
        }
        line += n;
        last = range.end;
    }
    out
}

fn span_at(source: &str, i: usize) -> Option<(Span, usize)> {
    let b = source.as_bytes();
    if b[i..].starts_with(b"--") {
        let end = b[i..]
            .iter()
            .position(|c| *c == b'\n')
            .map_or(b.len(), |n| i + n);
        Some((Span::LineComment, end))
    } else if b[i..].starts_with(b"/-") {
        Some((Span::BlockComment, block_end(source, i).unwrap_or(b.len())))
    } else {
        token_end(source, i)
    }
}

fn token_end(source: &str, start: usize) -> Option<(Span, usize)> {
    if !source.is_char_boundary(start) {
        return None;
    }
    let b = source.as_bytes();
    let rest = &source[start..];
    let after_ident = || source[..start].chars().next_back().is_some_and(is_id_rest);
    if rest.starts_with('«') {
        return rest
            .find('»')
            .map(|end| (Span::EscapedIdent, start + end + '»'.len_utf8()));
    }
    if rest.starts_with('r') && !after_ident() {
        let hashes = rest[1..].bytes().take_while(|c| *c == b'#').count();
        if rest.as_bytes().get(1 + hashes) == Some(&b'"') {
            let body = start + 2 + hashes;
            let suffix = format!("\"{}", "#".repeat(hashes));
            let end = source[body..]
                .find(&suffix)
                .map_or(b.len(), |end| body + end + suffix.len());
            return Some((Span::Literal, end));
        }
    }
    if rest.starts_with('"') {
        return Some((Span::Literal, string_end(source, start)));
    }
    if rest.starts_with('\'') && !after_ident() {
        return char_end(source, start).map(|end| (Span::Literal, end));
    }
    None
}

/// End of the string whose opening quote is at `quote`. Interpolated strings
/// may contain nested terms, including strings and comments, between braces.
fn string_end(source: &str, quote: usize) -> usize {
    let b = source.as_bytes();
    let interpolated = is_interpolated(source, quote);
    let mut i = quote + 1;
    let mut depth = 0usize;
    while i < b.len() {
        if depth == 0 {
            match b[i] {
                b'"' => return i + 1,
                b'\\' => i += 2,
                b'{' if interpolated => {
                    depth = 1;
                    i += 1;
                }
                _ => i += 1,
            }
        } else if let Some((_, end)) = span_at(source, i) {
            i = end;
        } else {
            match b[i] {
                b'{' => depth += 1,
                b'}' => depth -= 1,
                _ => {}
            }
            i += 1;
        }
    }
    b.len()
}

/// A character literal per Lean's `charLitFnAux`, or `None` when the quote
/// starts another token such as `''`.
fn char_end(source: &str, quote: usize) -> Option<usize> {
    let mut chars = source[quote + 1..].char_indices();
    let (_, c) = chars.next()?;
    if c == '\'' {
        return None;
    }
    if c == '\\' {
        let (_, escape) = chars.next()?;
        let digits = match escape {
            '\\' | '"' | '\'' | 'r' | 'n' | 't' => 0,
            'x' => 2,
            'u' => 4,
            _ => return None,
        };
        for _ in 0..digits {
            chars.next().filter(|(_, d)| d.is_ascii_hexdigit())?;
        }
    }
    let (offset, close) = chars.next()?;
    (close == '\'').then_some(quote + 1 + offset + 1)
}

/// Lean parses `interpolatedStr(term)` only after particular syntax, so this
/// checks the preceding tokens against the core interpolating forms.
fn is_interpolated(source: &str, quote: usize) -> bool {
    const PREFIXES: [&str; 10] = [
        "s!",
        "m!",
        "f!",
        "println!",
        "v!",
        "dbg_trace",
        "throwError",
        "reportIssue!",
        "reportDbgIssue!",
        "reportEMatchIssue!",
    ];
    let before = source[..quote].trim_end();
    if let Some(open) = before
        .strip_suffix(']')
        .and_then(|inner| matching_open(inner, b'[', b']'))
    {
        let head = &before[..open];
        if matches!(trailing_ident(head), "trace" | "Macro.trace" | "trace_goal") {
            return true;
        }
    }
    if PREFIXES.contains(&trailing_ident(before)) {
        return true;
    }
    // `throwNamedError name "…"` and its relatives name the error first.
    let named = strip_ident(before);
    if named.is_some_and(|head| {
        matches!(
            trailing_ident(head),
            "throwNamedError" | "logNamedError" | "logNamedWarning"
        )
    }) {
        return true;
    }
    // `throwErrorAt ref "…"` and `throwNamedErrorAt ref name "…"`, where
    // `ref` is one maximal-precedence term such as `stx`, `(f x)`, or
    // `stx[0]`.
    strip_max_term(before).is_some_and(|head| trailing_ident(head) == "throwErrorAt")
        || named.and_then(strip_max_term).is_some_and(|head| {
            matches!(
                trailing_ident(head),
                "throwNamedErrorAt" | "logNamedErrorAt" | "logNamedWarningAt"
            )
        })
}

/// The offset of the `open` byte that pairs with a `close` byte as the
/// closer just past the end of `text`.
fn matching_open(text: &str, open: u8, close: u8) -> Option<usize> {
    let mut depth = 1usize;
    for (i, c) in text.bytes().enumerate().rev().take(4096) {
        if c == close {
            depth += 1;
        } else if c == open {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// `text` without its trailing identifier and the whitespace before it, or
/// `None` when it does not end with one.
fn strip_ident(text: &str) -> Option<&str> {
    let ident = trailing_ident(text);
    (!ident.is_empty()).then(|| text[..text.len() - ident.len()].trim_end())
}

/// `text` without one trailing maximal-precedence term, such as `stx`,
/// `(f x)`, or `stx[0]`, and the whitespace before it.
fn strip_max_term(text: &str) -> Option<&str> {
    let mut head = text;
    while let Some(open) = head
        .strip_suffix(']')
        .and_then(|inner| matching_open(inner, b'[', b']'))
    {
        head = &head[..open];
    }
    match head
        .strip_suffix(')')
        .and_then(|inner| matching_open(inner, b'(', b')'))
    {
        Some(open) => Some(head[..open].trim_end()),
        None => strip_ident(head),
    }
}

/// The dotted identifier that ends `text`, or an empty string.
fn trailing_ident(text: &str) -> &str {
    let start = text
        .char_indices()
        .rev()
        .take_while(|(_, c)| is_id_rest(*c) || *c == '.')
        .last()
        .map_or(text.len(), |(i, _)| i);
    &text[start..]
}

/// Lean's `isIdRest` (ASCII alphanumerics, `_`, `'`, `!`, `?`, letter-like
/// symbols, and subscripts) plus the closing `»` of an identifier escape.
/// A quote after one of these continues an identifier.
fn is_id_rest(c: char) -> bool {
    let v = c as u32;
    c.is_ascii_alphanumeric()
        || matches!(c, '_' | '\'' | '!' | '?' | '»')
        || ((0x3b1..=0x3c9).contains(&v) && v != 0x3bb)
        || ((0x391..=0x3a9).contains(&v) && v != 0x3a0 && v != 0x3a3)
        || (0x3ca..=0x3fb).contains(&v)
        || (0x1f00..=0x1ffe).contains(&v)
        || (0x2100..=0x214f).contains(&v)
        || (0x1d49c..=0x1d59f).contains(&v)
        || ((0xc0..=0xff).contains(&v) && v != 0xd7 && v != 0xf7)
        || (0x100..=0x17f).contains(&v)
        || (0x2080..=0x209c).contains(&v)
        || (0x1d62..=0x1d6a).contains(&v)
        || v == 0x2c7c
}

#[cfg(test)]
mod tests {
    use super::*;

    fn masked(source: &str) -> String {
        code_mask(source)
    }

    #[test]
    fn interpolated_strings_contain_nested_strings_and_braces() {
        let source = "def x := s!\"{\"/--\"}\" /- c -/\n";
        assert_eq!(masked(source), format!("def x := s!{}\n", " ".repeat(17)));
        let source = "  m!\"{msg?.getD \"\"}\\n{ {a := 1}.a }\" -- c\nimport A\n";
        assert_eq!(
            masked(source),
            format!("  m!{}\nimport A\n", " ".repeat(37))
        );
        let source = "f!\"{'}'}\" ++ throwError \"{\"}\"}\" ++ throwErrorAt stx \"{\"}\"}\"";
        assert_eq!(
            masked(source),
            format!(
                "f!{}++ throwError{}++ throwErrorAt stx{}",
                " ".repeat(8),
                " ".repeat(9),
                " ".repeat(8)
            )
        );
        assert_eq!(
            masked("throwErrorAt stx[0] \"{\"}\"}\" x"),
            format!("throwErrorAt stx[0]{}x", " ".repeat(9))
        );
        assert_eq!(
            masked("trace[Meta.debug] \"{\"}\"}\" x"),
            format!("trace[Meta.debug]{}x", " ".repeat(9))
        );
    }

    #[test]
    fn plain_strings_keep_braces_literal() {
        assert_eq!(
            masked("logInfo \"{\" -- c"),
            format!("logInfo{}", " ".repeat(9))
        );
        assert_eq!(
            masked("panic! \"{\" -- c"),
            format!("panic!{}", " ".repeat(9))
        );
    }

    #[test]
    fn escaped_identifiers_hide_comment_delimiters_but_stay_code() {
        let source = "def «x/--y» := «a--b»\n";
        assert_eq!(masked(source), source);
        assert!(line_comments(source).is_empty());
        assert_eq!(literal_end(source, 4), Some(4 + "«x/--y»".len()));
    }

    #[test]
    fn primes_continue_identifiers_and_char_literals_follow_lean() {
        let source = "theorem a' (b' : B) : f '' s = c' -- c\n";
        assert_eq!(line_comments(source), vec![34..38]);
        assert_eq!(masked("x = '\"' -- c"), format!("x ={}", " ".repeat(9)));
        assert_eq!(
            masked("x = '\\u0041' + '\\x41'"),
            format!("x ={}+{}", " ".repeat(10), " ".repeat(7))
        );
        assert_eq!(literal_end("'ab'", 0), None);
    }

    #[test]
    fn raw_strings_need_a_token_start() {
        assert_eq!(masked("r#\"a\"-/\"# x"), "          x");
        assert_eq!(literal_end("bar\"x\"", 2), None);
    }

    #[test]
    fn the_character_after_a_comment_opener_is_skipped() {
        assert_eq!(block_end("/--/ x -/", 0), Some(9));
        assert_eq!(block_end("/-/ x -/ y", 0), Some(8));
        assert_eq!(block_end("/--/", 0), None);
        assert_eq!(block_end("/-", 0), None);
        assert_eq!(masked("/--/ a -/b"), "         b");
    }

    #[test]
    fn literal_lines_cover_multiline_interpolations() {
        let source = "x\ns!\"{\n\"a\"}\"\ny";
        assert_eq!(
            literal_lines(source),
            std::collections::HashSet::from([1, 2])
        );
    }
}
