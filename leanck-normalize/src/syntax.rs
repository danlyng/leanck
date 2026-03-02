//! Small lexical scanner used to keep formatting out of Lean strings and comments.
/// Replace strings and comments with spaces, preserving byte offsets and newlines.
pub(crate) fn code_mask(source: &str) -> String {
    let bytes = source.as_bytes();
    let mut out = bytes.to_vec();
    let mut i = 0;
    while i < bytes.len() {
        let start = i;
        if bytes[i..].starts_with(b"--") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
        } else if bytes[i..].starts_with(b"/-") {
            i = block_end(source, i).unwrap_or(bytes.len());
        } else if let Some(end) = literal_end(source, i) {
            i = end;
        } else {
            i += 1;
            continue;
        }
        for c in &mut out[start..i] {
            if *c != b'\n' && *c != b'\r' {
                *c = b' ';
            }
        }
    }
    String::from_utf8(out).unwrap()
}

/// Offsets of actual Lean line comments, excluding strings and nested block comments.
pub(crate) fn line_comments(source: &str) -> Vec<std::ops::Range<usize>> {
    let bytes = source.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i..].starts_with(b"--") {
            let start = i;
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            out.push(start..i);
        } else if bytes[i..].starts_with(b"/-") {
            i = block_end(source, i).unwrap_or(bytes.len());
        } else if let Some(end) = literal_end(source, i) {
            i = end;
        } else {
            i += 1;
        }
    }
    out
}

/// End of a block comment, including nested blocks. For historical comment
/// snippets containing a literal unmatched `/-`, retain the first closing delimiter.
pub(crate) fn block_end(source: &str, start: usize) -> Option<usize> {
    let bytes = source.as_bytes();
    let mut i = start + 2;
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

/// End of an ordinary/raw string or character literal, when one starts here.
pub(crate) fn literal_end(source: &str, start: usize) -> Option<usize> {
    let b = source.as_bytes();
    if b.get(start) == Some(&b'r') {
        let mut quote = start + 1;
        while b.get(quote) == Some(&b'#') {
            quote += 1;
        }
        if b.get(quote) == Some(&b'"') {
            let suffix = format!("\"{}", "#".repeat(quote - start - 1));
            return source[quote + 1..]
                .find(&suffix)
                .map(|end| quote + 1 + end + suffix.len())
                .or(Some(b.len()));
        }
    }
    if b.get(start) == Some(&b'"') {
        let mut i = start + 1;
        while i < b.len() {
            if b[i] == b'\\' {
                i = (i + 2).min(b.len());
            } else if b[i] == b'"' {
                return Some(i + 1);
            } else {
                i += 1;
            }
        }
        return Some(b.len());
    }
    if b.get(start) == Some(&b'\'') {
        let mut i = start + 1;
        if b.get(i) == Some(&b'\\') {
            i += 2;
        } else if let Some(c) = source.get(i..).and_then(|s| s.chars().next()) {
            i += c.len_utf8();
        }
        if b.get(i) == Some(&b'\'') {
            return Some(i + 1);
        }
    }
    None
}

pub(crate) fn literal_lines(source: &str) -> std::collections::HashSet<usize> {
    let b = source.as_bytes();
    let mut out = std::collections::HashSet::new();
    let mut i = 0;
    let mut line = 0;
    while i < b.len() {
        if let Some(end) = literal_end(source, i) {
            let n = b[i..end].iter().filter(|c| **c == b'\n').count();
            out.extend(line..=line + n);
            line += n;
            i = end;
        } else if b[i..].starts_with(b"/-") {
            let end = block_end(source, i).unwrap_or(b.len());
            line += b[i..end].iter().filter(|c| **c == b'\n').count();
            i = end;
        } else if b[i..].starts_with(b"--") {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else {
            if b[i] == b'\n' {
                line += 1;
            }
            i += 1;
        }
    }
    out
}
