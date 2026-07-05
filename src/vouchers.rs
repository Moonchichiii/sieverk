//! Voucher parser — step 4 of the SIE parser.
//!
//! Parses `#VER` headers and their `{ ... }` transaction blocks into
//! vouchers with `#TRANS` rows. This is a state machine over *lines*,
//! the same way the tokenizer is a state machine over *characters*.
//!
//! Tolerance rules, same philosophy as the rest of the engine: malformed
//! rows become warnings and are skipped, structural surprises (orphan
//! braces, unterminated blocks, `#TRANS` outside a block) become warnings
//! — and already-collected rows are never thrown away. `#RTRANS`/`#BTRANS`
//! correction rows are silently ignored for now; they get their own
//! treatment when the validator learns about corrections.

use crate::money::Ore;
use crate::tokenizer::{tokenize, Token};

#[derive(Debug, Default)]
pub struct VoucherData {
    pub vouchers: Vec<Voucher>,
    pub warnings: Vec<String>,
}

#[derive(Debug, PartialEq)]
pub struct Voucher {
    pub series: String,
    pub number: String,
    /// Raw YYYYMMDD as written in the file — lexicographic comparison
    /// works for range checks, so a date type earns its place only once
    /// date arithmetic does.
    pub date: String,
    pub text: Option<String>,
    pub rows: Vec<Trans>,
}

#[derive(Debug, PartialEq)]
pub struct Trans {
    pub account: String,
    pub amount: Ore,
    /// Raw words from the `{ ... }` object list (dimension/object pairs).
    /// Kept flat until dimensions get first-class treatment.
    pub objects: Vec<String>,
}

/// Extract vouchers from decoded SIE text.
pub fn parse_vouchers(text: &str) -> VoucherData {
    let mut data = VoucherData::default();
    let mut current: Option<Voucher> = None;
    let mut in_block = false;

    for line in text.lines() {
        let tokens = tokenize(line);
        let Some(first) = tokens.first() else {
            continue;
        };

        match first {
            Token::Word(w) if w == "#VER" => {
                if let Some(prev) = current.take() {
                    data.warnings.push(format!(
                        "#VER {}-{} was never closed before the next #VER",
                        prev.series, prev.number
                    ));
                    data.vouchers.push(prev);
                    in_block = false;
                }
                let words: Vec<&str> = word_slice(&tokens[1..]);
                match (words.first(), words.get(1), words.get(2)) {
                    (Some(&series), Some(&number), Some(&date)) => {
                        current = Some(Voucher {
                            series: series.to_string(),
                            number: number.to_string(),
                            date: date.to_string(),
                            text: words.get(3).map(|t| (*t).to_string()),
                            rows: Vec::new(),
                        });
                    }
                    _ => data
                        .warnings
                        .push(format!("#VER header is malformed: {line}")),
                }
            }
            Token::BeginList if tokens.len() == 1 => {
                if current.is_some() && !in_block {
                    in_block = true;
                } else {
                    data.warnings
                        .push("unexpected '{' with no #VER header before it".to_string());
                }
            }
            Token::EndList if tokens.len() == 1 => {
                if in_block {
                    in_block = false;
                    if let Some(v) = current.take() {
                        data.vouchers.push(v);
                    }
                } else {
                    data.warnings
                        .push("unexpected '}' with no open voucher block".to_string());
                }
            }
            Token::Word(w) if w == "#TRANS" => {
                let Some(voucher) = current.as_mut().filter(|_| in_block) else {
                    data.warnings
                        .push(format!("#TRANS outside a voucher block: {line}"));
                    continue;
                };
                match parse_trans(&tokens) {
                    Ok(trans) => voucher.rows.push(trans),
                    Err(msg) => data.warnings.push(format!("{msg}: {line}")),
                }
            }
            _ => {} // metadata, accounts, unknown tags — not ours
        }
    }

    if let Some(v) = current.take() {
        data.warnings.push(format!(
            "#VER {}-{} block never closes before end of file",
            v.series, v.number
        ));
        data.vouchers.push(v);
    }

    data
}

fn word_slice(tokens: &[Token]) -> Vec<&str> {
    tokens
        .iter()
        .filter_map(|t| match t {
            Token::Word(w) => Some(w.as_str()),
            _ => None,
        })
        .collect()
}

fn parse_trans(tokens: &[Token]) -> Result<Trans, String> {
    // tokens[0] is #TRANS, guaranteed by the caller.
    let account = match tokens.get(1) {
        Some(Token::Word(a)) => a.clone(),
        _ => return Err("#TRANS is missing its account number".to_string()),
    };

    let mut i = 2;
    let mut objects = Vec::new();
    if let Some(Token::BeginList) = tokens.get(i) {
        i += 1;
        loop {
            match tokens.get(i) {
                Some(Token::Word(w)) => {
                    objects.push(w.clone());
                    i += 1;
                }
                Some(Token::EndList) => {
                    i += 1;
                    break;
                }
                Some(Token::BeginList) => {
                    return Err("nested '{' inside a #TRANS object list".to_string());
                }
                None => return Err("#TRANS object list never closes".to_string()),
            }
        }
    }

    let amount = match tokens.get(i) {
        Some(Token::Word(w)) => {
            Ore::parse(w).ok_or_else(|| format!("#TRANS amount '{w}' is not a valid amount"))?
        }
        _ => return Err("#TRANS is missing its amount".to_string()),
    };

    Ok(Trans {
        account,
        amount,
        objects,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ore(s: &str) -> Ore {
        Ore::parse(s).expect("test literal should be a valid amount")
    }

    const ONE_VOUCHER: &str = "\
#VER A 1 20260312 \"Diesel skogsmaskin\"
{
#TRANS 1930 {} -1250.00
#TRANS 5611 {} 1000.00
#TRANS 2641 {} 250.00
}
";

    #[test]
    fn parses_voucher_header() {
        let d = parse_vouchers(ONE_VOUCHER);
        assert_eq!(d.vouchers.len(), 1);
        let v = &d.vouchers[0];
        assert_eq!(v.series, "A");
        assert_eq!(v.number, "1");
        assert_eq!(v.date, "20260312");
        assert_eq!(v.text.as_deref(), Some("Diesel skogsmaskin"));
    }

    #[test]
    fn parses_trans_rows_with_exact_amounts() {
        let v = &parse_vouchers(ONE_VOUCHER).vouchers[0];
        assert_eq!(v.rows.len(), 3);
        assert_eq!(v.rows[0].account, "1930");
        assert_eq!(v.rows[0].amount, ore("-1250.00"));
        assert_eq!(v.rows[1].amount, ore("1000.00"));
        assert_eq!(v.rows[2].amount, ore("250.00"));
    }

    #[test]
    fn captures_object_list_contents() {
        let d =
            parse_vouchers("#VER A 1 20260101\n{\n#TRANS 5611 {1 \"Norra skiftet\"} 1000.00\n}\n");
        let row = &d.vouchers[0].rows[0];
        assert_eq!(
            row.objects,
            vec!["1".to_string(), "Norra skiftet".to_string()]
        );
        assert_eq!(row.amount, ore("1000.00"));
    }

    #[test]
    fn empty_object_list_yields_no_objects() {
        let d = parse_vouchers(ONE_VOUCHER);
        assert!(d.vouchers[0].rows[0].objects.is_empty());
    }

    #[test]
    fn voucher_without_text_is_tolerated() {
        let d = parse_vouchers("#VER A 2 20260418\n{\n#TRANS 1930 {} 100.00\n}\n");
        assert_eq!(d.vouchers[0].text, None);
        assert!(d.warnings.is_empty());
    }

    #[test]
    fn multiple_vouchers_arrive_in_file_order() {
        let text = format!(
            "{ONE_VOUCHER}#VER A 2 20260418 \"Nummer två\"\n{{\n#TRANS 1930 {{}} 100.00\n}}\n"
        );
        let d = parse_vouchers(&text);
        assert_eq!(d.vouchers.len(), 2);
        assert_eq!(d.vouchers[1].number, "2");
        assert!(d.warnings.is_empty());
    }

    #[test]
    fn trans_outside_block_becomes_warning() {
        let d = parse_vouchers("#TRANS 1930 {} 100.00");
        assert!(d.vouchers.is_empty());
        assert_eq!(d.warnings.len(), 1);
    }

    #[test]
    fn unterminated_block_keeps_rows_and_warns() {
        let d = parse_vouchers("#VER A 1 20260312 \"Halvfärdig\"\n{\n#TRANS 1930 {} -100.00\n");
        assert_eq!(d.vouchers.len(), 1);
        assert_eq!(d.vouchers[0].rows.len(), 1);
        assert_eq!(d.warnings.len(), 1);
    }

    #[test]
    fn malformed_trans_amount_becomes_warning_row_skipped() {
        let d = parse_vouchers(
            "#VER A 1 20260312\n{\n#TRANS 1930 {} tusen\n#TRANS 5611 {} 100.00\n}\n",
        );
        assert_eq!(d.vouchers[0].rows.len(), 1);
        assert_eq!(d.vouchers[0].rows[0].account, "5611");
        assert_eq!(d.warnings.len(), 1);
    }
}
