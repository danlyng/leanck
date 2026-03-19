//! Shared heuristic Lean lexer. Token positions use the LSP UTF-16 encoding.
use crate::api::{Position, Range};
#[derive(Debug, Clone)]
pub struct Token {
    pub text: String,
    pub range: Range,
}
fn ident(c: char) -> bool {
    c.is_alphanumeric()
        || matches!(c, '_' | '\'' | '.')
        || (!c.is_ascii() && !c.is_whitespace() && !matches!(c, '«' | '»' | '⦃' | '⦄' | '⟨' | '⟩'))
}
pub fn tokens(source: &str) -> Vec<Token> {
    let chars: Vec<char> = source.chars().collect();
    let mut i = 0;
    let mut pos = Position {
        line: 0,
        character: 0,
    };
    let mut out = Vec::new();
    fn advance(chars: &[char], i: &mut usize, p: &mut Position, n: usize) {
        for _ in 0..n {
            if chars[*i] == '\n' {
                p.line += 1;
                p.character = 0;
            } else {
                p.character += chars[*i].len_utf16() as u32;
            }
            *i += 1;
        }
    }
    while i < chars.len() {
        let pair = (chars[i], chars.get(i + 1).copied());
        if pair == ('-', Some('-')) {
            while i < chars.len() && chars[i] != '\n' {
                advance(&chars, &mut i, &mut pos, 1);
            }
            continue;
        }
        if pair == ('/', Some('-')) {
            let mut depth = 1;
            advance(&chars, &mut i, &mut pos, 2);
            while i < chars.len() && depth > 0 {
                match (chars[i], chars.get(i + 1).copied()) {
                    ('/', Some('-')) => {
                        depth += 1;
                        advance(&chars, &mut i, &mut pos, 2)
                    }
                    ('-', Some('/')) => {
                        depth -= 1;
                        advance(&chars, &mut i, &mut pos, 2)
                    }
                    _ => advance(&chars, &mut i, &mut pos, 1),
                }
            }
            continue;
        }
        if chars[i] == '"' {
            advance(&chars, &mut i, &mut pos, 1);
            while i < chars.len() {
                let c = chars[i];
                advance(&chars, &mut i, &mut pos, 1);
                if c == '\\' && i < chars.len() {
                    advance(&chars, &mut i, &mut pos, 1);
                } else if c == '"' {
                    break;
                }
            }
            continue;
        }
        if chars[i].is_whitespace() {
            advance(&chars, &mut i, &mut pos, 1);
            continue;
        }
        let start = pos;
        let begin = i;
        if chars[i] == '«' {
            advance(&chars, &mut i, &mut pos, 1);
            while i < chars.len() {
                let c = chars[i];
                advance(&chars, &mut i, &mut pos, 1);
                if c == '»' {
                    break;
                }
            }
        } else if ident(chars[i]) {
            while i < chars.len() && ident(chars[i]) {
                advance(&chars, &mut i, &mut pos, 1);
            }
        } else {
            advance(&chars, &mut i, &mut pos, 1);
        }
        out.push(Token {
            text: chars[begin..i].iter().collect(),
            range: Range { start, end: pos },
        });
    }
    out
}
pub fn imports(source: &str) -> Vec<String> {
    let tokens = tokens(source);
    let mut out = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        let line = tokens[i].range.start.line;
        let start = i;
        while i < tokens.len() && tokens[i].range.start.line == line {
            i += 1;
        }
        if tokens[start].text == "import" {
            out.extend(
                tokens[start + 1..i]
                    .iter()
                    .filter(|t| {
                        t.text != "all"
                            && t.text
                                .chars()
                                .next()
                                .is_some_and(|c| c.is_alphabetic() || c == '_')
                    })
                    .map(|t| t.text.clone()),
            );
        }
    }
    out
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn ignores_nested_comments_escaped_strings_and_quoted_identifiers() {
        let input = "/- sorry /- nested -/ -/ \"sorry \\\" -- /-\"\n-- sorry\n«sorry» sorry αsorry sorryα\n";
        assert_eq!(
            tokens(input).iter().filter(|t| t.text == "sorry").count(),
            1
        );
    }
    #[test]
    fn utf16_columns_count_non_bmp_characters() {
        let t = tokens("\"😀\" sorry");
        assert_eq!(t[0].range.start.character, 5);
        assert_eq!(t[0].range.end.character, 10);
    }
    #[test]
    fn imports_skip_comment_and_string_text() {
        assert_eq!(
            imports("/- import Bad -/\nimport all A.B C.D\ndef x := \"import Fake\""),
            ["A.B", "C.D"]
        );
    }
}
