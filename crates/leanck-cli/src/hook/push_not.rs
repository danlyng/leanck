//! Textual migration of `push_neg`, including occurrences in strings and comments.

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Match the Python hook's token boundaries and flat config atoms without regex.
/// Nested parentheses are deliberately left after the new `Not` head term.
pub(super) fn rewrite(source: &str) -> Option<(String, usize)> {
    const TOKEN: &str = "push_neg";
    let mut output = String::new();
    let mut copied = 0;
    let mut search = 0;
    let mut count = 0;
    while let Some(offset) = source[search..].find(TOKEN) {
        let start = search + offset;
        let end = start + TOKEN.len();
        search = end;
        if source[..start].chars().next_back().is_some_and(is_word)
            || source[end..]
                .chars()
                .next()
                .is_some_and(|c| is_word(c) || c == '.')
        {
            continue;
        }

        let mut config_end = end;
        loop {
            let atom = config_end
                + source[config_end..]
                    .bytes()
                    .take_while(|c| matches!(c, b' ' | b'\t'))
                    .count();
            let next_end = match source.as_bytes().get(atom) {
                Some(b'+' | b'-') => {
                    let word_len: usize = source[atom + 1..]
                        .chars()
                        .take_while(|c| is_word(*c))
                        .map(char::len_utf8)
                        .sum();
                    (word_len > 0).then_some(atom + 1 + word_len)
                }
                Some(b'(') => source[atom + 1..]
                    .find(['(', ')'])
                    .filter(|offset| source.as_bytes()[atom + 1 + offset] == b')')
                    .map(|offset| atom + offset + 2),
                _ => None,
            };
            let Some(next_end) = next_end else {
                break;
            };
            config_end = next_end;
        }

        output.push_str(&source[copied..start]);
        output.push_str("push");
        output.push_str(&source[end..config_end]);
        output.push_str(" Not");
        copied = config_end;
        search = config_end;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    output.push_str(&source[copied..]);
    Some((output, count))
}

#[cfg(test)]
mod tests {
    use super::rewrite;

    #[test]
    fn rewrites_tokens_and_preserves_config_order() {
        for (source, expected) in [
            ("push_neg", "push Not"),
            ("push_neg at h ⊢", "push Not at h ⊢"),
            ("push_neg +distrib", "push +distrib Not"),
            ("push_neg +distrib at h", "push +distrib Not at h"),
            ("push_neg (config := foo)", "push (config := foo) Not"),
            ("push_neg (disch := (simp))", "push Not (disch := (simp))"),
            ("push_neg'", "push Not'"),
            (
                "push_neg\t-distrib +foo (config := x)",
                "push\t-distrib +foo (config := x) Not",
            ),
            ("push_neg +é_2", "push +é_2 Not"),
            ("push_neg (push_neg)", "push (push_neg) Not"),
            ("-- push_neg\n\"push_neg\"", "-- push Not\n\"push Not\""),
        ] {
            let count = if source.starts_with("--") { 2 } else { 1 };
            assert_eq!(
                rewrite(source),
                Some((expected.to_owned(), count)),
                "{source}"
            );
        }
    }

    #[test]
    fn leaves_option_names_and_word_characters_unchanged() {
        for source in [
            "set_option push_neg.use_distrib true",
            "my_push_neg",
            "push_negation",
            "épush_neg",
            "push_negé",
            "push_neg_",
            "simp",
        ] {
            assert_eq!(rewrite(source), None, "{source}");
        }
    }

    #[test]
    fn counts_multiple_occurrences() {
        assert_eq!(
            rewrite("push_neg at h\npush_neg +distrib\npush_neg"),
            Some(("push Not at h\npush +distrib Not\npush Not".to_owned(), 3))
        );
    }
}
