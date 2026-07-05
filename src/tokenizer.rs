//! Line tokenizer — step 1 of the SIE parser.
//!
//! Turns one raw line into tokens. This is the only place in the whole
//! engine that deals with quotes, escapes and braces; everything after
//! this works on clean tokens.
//!
//! Implementation: a character-by-character state machine with three
//! states (Between / InWord / InQuotes). `split_whitespace()` cannot be
//! used here — it would shred quoted strings like "Diesel skogsmaskin".
//! Inside quotes, `\"` and `\\` are unescaped. An unterminated quote does
//! not panic: the rest of the line becomes the token content, because
//! parsers in this engine never panic on malformed input.

/// One token from a SIE line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    /// A bare word (`#KONTO`, `1930`, `-1250.00`) or the contents of a
    /// quoted string (quotes stripped, escapes resolved).
    Word(String),
    /// `{` — start of an object list (inside #TRANS) or of a #VER block.
    BeginList,
    /// `}` — end of same.
    EndList,
}

/// Tokenize one line of SIE text.
///
/// Whitespace (spaces/tabs) separates tokens. Quoted strings are single
/// tokens with the quotes removed and `\"` / `\\` unescaped. `{` and `}`
/// are always their own tokens, even glued to other text (`{}` is two
/// tokens). An empty or whitespace-only line yields an empty Vec.
pub fn tokenize(line: &str) -> Vec<Token> {
    enum State {
        Between,
        InWord,
        InQuotes,
    }

    let mut state = State::Between;
    let mut buf = String::new();
    let mut out = Vec::new();
    let mut chars = line.chars().peekable();

    while let Some(c) = chars.next() {
        match state {
            State::Between => match c {
                c if c.is_whitespace() => {}
                '{' => out.push(Token::BeginList),
                '}' => out.push(Token::EndList),
                '"' => state = State::InQuotes,
                _ => {
                    buf.push(c);
                    state = State::InWord;
                }
            },
            State::InWord => match c {
                c if c.is_whitespace() => {
                    out.push(Token::Word(std::mem::take(&mut buf)));
                    state = State::Between;
                }
                '{' => {
                    out.push(Token::Word(std::mem::take(&mut buf)));
                    out.push(Token::BeginList);
                    state = State::Between;
                }
                '}' => {
                    out.push(Token::Word(std::mem::take(&mut buf)));
                    out.push(Token::EndList);
                    state = State::Between;
                }
                _ => buf.push(c),
            },
            State::InQuotes => match c {
                '\\' => match chars.peek() {
                    Some(&next) if next == '"' || next == '\\' => {
                        buf.push(next);
                        chars.next();
                    }
                    _ => buf.push('\\'),
                },
                '"' => {
                    out.push(Token::Word(std::mem::take(&mut buf)));
                    state = State::Between;
                }
                _ => buf.push(c),
            },
        }
    }

    // End of line: flush whatever is in flight. An unterminated quote does
    // not panic — the rest of the line becomes the token content.
    match state {
        State::InWord | State::InQuotes => out.push(Token::Word(buf)),
        State::Between => {}
    }

    out
}

#[cfg(test)]
mod tests {
    use super::{tokenize, Token};

    fn w(s: &str) -> Token {
        Token::Word(s.to_string())
    }

    #[test]
    fn simple_tag_and_field() {
        assert_eq!(tokenize("#FLAGGA 0"), vec![w("#FLAGGA"), w("0")]);
    }

    #[test]
    fn quoted_string_is_one_token_without_quotes() {
        assert_eq!(
            tokenize("#KONTO 1930 \"Företagskonto\""),
            vec![w("#KONTO"), w("1930"), w("Företagskonto")]
        );
    }

    #[test]
    fn quoted_string_keeps_inner_spaces() {
        assert_eq!(
            tokenize("#VER A 1 20260312 \"Diesel skogsmaskin\""),
            vec![
                w("#VER"),
                w("A"),
                w("1"),
                w("20260312"),
                w("Diesel skogsmaskin")
            ]
        );
    }

    #[test]
    fn empty_object_list_is_two_tokens() {
        assert_eq!(
            tokenize("#TRANS 1930 {} -1250.00"),
            vec![
                w("#TRANS"),
                w("1930"),
                Token::BeginList,
                Token::EndList,
                w("-1250.00")
            ]
        );
    }

    #[test]
    fn object_list_with_content() {
        assert_eq!(
            tokenize("#TRANS 5611 {1 \"Norra skiftet\"} 1000.00"),
            vec![
                w("#TRANS"),
                w("5611"),
                Token::BeginList,
                w("1"),
                w("Norra skiftet"),
                Token::EndList,
                w("1000.00")
            ]
        );
    }

    #[test]
    fn lone_brace_lines_delimit_ver_blocks() {
        assert_eq!(tokenize("{"), vec![Token::BeginList]);
        assert_eq!(tokenize("}"), vec![Token::EndList]);
    }

    #[test]
    fn escaped_quote_inside_string() {
        assert_eq!(
            tokenize(r#"#FNAMN "Firma \"Skogen\" AB""#),
            vec![w("#FNAMN"), w(r#"Firma "Skogen" AB"#)]
        );
    }

    #[test]
    fn tabs_and_multiple_spaces_separate() {
        assert_eq!(
            tokenize("#KONTO\t1930   \"Kassa\""),
            vec![w("#KONTO"), w("1930"), w("Kassa")]
        );
    }

    #[test]
    fn blank_and_whitespace_lines_yield_nothing() {
        assert_eq!(tokenize(""), Vec::<Token>::new());
        assert_eq!(tokenize("   \t "), Vec::<Token>::new());
    }

    #[test]
    fn unterminated_quote_does_not_panic() {
        // Malformed input from a buggy exporter: a quote that never closes.
        // House rule — parsers never panic. The rest of the line becomes
        // the token content.
        assert_eq!(
            tokenize("#FNAMN \"Aldrig stängd"),
            vec![w("#FNAMN"), w("Aldrig stängd")]
        );
    }
}
