//! Accounts & balances parser — step 3 of the SIE parser.
//!
//! Collects `#KONTO` account definitions and the `#IB`/`#UB`/`#RES`
//! balance rows (opening, closing, result). Balances are stored as rows
//! exactly as the file states them — per year index (0 = current,
//! -1 = previous) — aggregation is a later concern.
//!
//! Money is `Ore` (exact i64 öre), never `f64`. Accounting amounts must
//! survive round-trips exactly; floats cannot promise that. Malformed
//! rows never panic — they become warnings and are skipped.

use crate::money::Ore;
use crate::tokenizer::{tokenize, Token};

#[derive(Debug, Default)]
pub struct AccountData {
    pub accounts: Vec<Account>,
    pub balances: Vec<Balance>,
    pub warnings: Vec<String>,
}

/// One `#KONTO` row. The number stays a raw string on purpose: real-world
/// files contain oddities, and tolerating them here while letting the
/// validator judge them later is the division of labour in this engine.
#[derive(Debug, PartialEq, Eq)]
pub struct Account {
    pub number: String,
    pub name: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BalanceKind {
    /// `#IB` — opening balance (balance accounts).
    Opening,
    /// `#UB` — closing balance (balance accounts).
    Closing,
    /// `#RES` — year result (result accounts).
    Result,
}

impl BalanceKind {
    fn tag(self) -> &'static str {
        match self {
            BalanceKind::Opening => "#IB",
            BalanceKind::Closing => "#UB",
            BalanceKind::Result => "#RES",
        }
    }
}

#[derive(Debug, PartialEq)]
pub struct Balance {
    pub kind: BalanceKind,
    /// 0 = current fiscal year, -1 = previous, matching `#RAR` indices.
    pub year_index: i32,
    pub account: String,
    pub amount: Ore,
}

/// Extract accounts and balance rows from decoded SIE text.
pub fn parse_accounts(text: &str) -> AccountData {
    let mut data = AccountData::default();

    for line in text.lines() {
        let tokens = tokenize(line);
        let words: Vec<&str> = tokens
            .iter()
            .filter_map(|t| match t {
                Token::Word(w) => Some(w.as_str()),
                _ => None,
            })
            .collect();
        let Some(&tag) = words.first() else { continue };

        match tag {
            "#KONTO" => match words.get(1) {
                Some(&number) => data.accounts.push(Account {
                    number: number.to_string(),
                    name: words.get(2).map(|s| (*s).to_string()),
                }),
                None => data
                    .warnings
                    .push(format!("#KONTO row is missing its account number: {line}")),
            },
            "#IB" => push_balance(BalanceKind::Opening, &words, line, &mut data),
            "#UB" => push_balance(BalanceKind::Closing, &words, line, &mut data),
            "#RES" => push_balance(BalanceKind::Result, &words, line, &mut data),
            _ => {} // not ours — vouchers and metadata have their own modules
        }
    }

    data
}

fn push_balance(kind: BalanceKind, words: &[&str], line: &str, data: &mut AccountData) {
    let year_index = words.get(1).and_then(|w| w.parse::<i32>().ok());
    let account = words.get(2).map(|s| (*s).to_string());
    let amount = words.get(3).and_then(|w| Ore::parse(w));

    match (year_index, account, amount) {
        (Some(year_index), Some(account), Some(amount)) => data.balances.push(Balance {
            kind,
            year_index,
            account,
            amount,
        }),
        _ => data
            .warnings
            .push(format!("{} row is malformed: {line}", kind.tag())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ore(s: &str) -> Ore {
        Ore::parse(s).expect("test literal should be a valid amount")
    }

    #[test]
    fn parses_account_number_and_name() {
        let d = parse_accounts("#KONTO 1930 \"Företagskonto\"");
        assert_eq!(d.accounts.len(), 1);
        assert_eq!(d.accounts[0].number, "1930");
        assert_eq!(d.accounts[0].name.as_deref(), Some("Företagskonto"));
    }

    #[test]
    fn preserves_file_order_of_accounts() {
        let d = parse_accounts("#KONTO 5611 \"Drivmedel\"\n#KONTO 1930 \"Företagskonto\"");
        assert_eq!(d.accounts[0].number, "5611");
        assert_eq!(d.accounts[1].number, "1930");
    }

    #[test]
    fn parses_opening_and_closing_balances_exactly() {
        let d = parse_accounts("#IB 0 1930 125000.00\n#UB 0 1930 180000.00");
        assert_eq!(d.balances.len(), 2);
        assert_eq!(d.balances[0].kind, BalanceKind::Opening);
        assert_eq!(d.balances[0].amount, ore("125000.00"));
        assert_eq!(d.balances[1].kind, BalanceKind::Closing);
        assert_eq!(d.balances[1].amount, ore("180000.00"));
    }

    #[test]
    fn parses_result_rows_with_negative_amounts() {
        let d = parse_accounts("#RES 0 3011 -45000.00");
        assert_eq!(d.balances[0].kind, BalanceKind::Result);
        assert_eq!(d.balances[0].year_index, 0);
        assert_eq!(d.balances[0].account, "3011");
        assert_eq!(d.balances[0].amount, ore("-45000.00"));
    }

    #[test]
    fn malformed_amount_becomes_warning_not_panic() {
        let d = parse_accounts("#IB 0 1930 abc");
        assert!(d.balances.is_empty());
        assert_eq!(d.warnings.len(), 1);
    }

    #[test]
    fn nameless_account_is_tolerated_here() {
        // The parser tolerates it; flagging it is the validator's job.
        let d = parse_accounts("#KONTO 9999");
        assert_eq!(d.accounts[0].number, "9999");
        assert_eq!(d.accounts[0].name, None);
        assert!(d.warnings.is_empty());
    }
}
