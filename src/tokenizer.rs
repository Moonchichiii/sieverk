//! Line tokenizer — step 1 of the SIE parser.
//!
//! Turns one raw line into tokens. This is the only place in the whole
//! engine that deals with quotes, escapes and braces; everything after
//! this works on clean tokens.
//!
//! YOUR TASK: implement `tokenize`. The tests at the bottom are the
//! complete specification — make them green without changing them.
//!
//! Hints, in the order you will need them:
//!   * `line.split_whitespace()` is the trap — it shreds quoted strings.
//!     You need a character loop: `line.chars()`, probably `.peekable()`.
//!   * You cannot index a Rust string (`line[0]` will not compile) —
//!     strings are UTF-8, you walk them with iterators. This is normal;
//!     embrace `chars()`.
//!   * Build each token in a `String` buffer with `buf.push(c)`, and take
//!     it out with `std::mem::take(&mut buf)` when the token ends.
//!   * Inside quotes: `\"` becomes a literal quote, `\\` a literal
//!     backslash. Everything else (including spaces) is kept as-is.
//!   * A small state machine beats a clever one-liner. An
//!     `enum State { Between, InWord, InQuotes }` is a fine shape.

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
    todo!("tokenize {line:?} — the tests below are the spec")
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
}
