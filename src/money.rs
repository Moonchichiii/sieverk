//! Money — exact, dependency-free, panic-free.
//!
//! SIE amounts have öre resolution (at most two decimals), so the honest
//! representation is an integer count of öre. `f64` is banned in this
//! repo: floats cannot promise that 0.10 + 0.20 == 0.30, and accounting
//! demands exactly that promise.

use std::fmt;
use std::iter::Sum;
use std::ops::Add;

/// An exact amount in öre. `Ore(125_000_00)` is 125 000,00 kr.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord)]
pub struct Ore(pub i64);

impl Ore {
    pub const ZERO: Ore = Ore(0);

    /// Parse a SIE amount: optional sign, digits, optionally a decimal
    /// point followed by one or two digits. `"1000"`, `"-1250.00"` and
    /// `"12.5"` are valid; three decimals, stray characters, or overflow
    /// yield `None` — never a panic.
    pub fn parse(s: &str) -> Option<Ore> {
        let s = s.trim();
        let (negative, rest) = match s.strip_prefix('-') {
            Some(rest) => (true, rest),
            None => (false, s.strip_prefix('+').unwrap_or(s)),
        };

        let (int_part, dec_part) = match rest.split_once('.') {
            Some((i, d)) => (i, d),
            None => (rest, ""),
        };

        if int_part.is_empty() || !int_part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        if dec_part.len() > 2 || !dec_part.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }

        let kronor: i64 = int_part.parse().ok()?;
        let ore: i64 = match dec_part.len() {
            0 => 0,
            1 => dec_part.parse::<i64>().ok()? * 10,
            _ => dec_part.parse().ok()?,
        };

        let total = kronor.checked_mul(100)?.checked_add(ore)?;
        Some(Ore(if negative { -total } else { total }))
    }
}

impl fmt::Display for Ore {
    /// Renders exactly like a SIE amount: `-1250.00`, `0.05`, `125000.00`.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let sign = if self.0 < 0 { "-" } else { "" };
        let abs = self.0.unsigned_abs();
        write!(f, "{sign}{}.{:02}", abs / 100, abs % 100)
    }
}

impl Add for Ore {
    type Output = Ore;
    fn add(self, rhs: Ore) -> Ore {
        // Saturating on purpose: a sum that hits i64 limits is already a
        // corrupt file, and the validator will flag the imbalance — but
        // this engine never panics on malformed input.
        Ore(self.0.saturating_add(rhs.0))
    }
}

impl Sum for Ore {
    fn sum<I: Iterator<Item = Ore>>(iter: I) -> Ore {
        iter.fold(Ore::ZERO, Add::add)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_integer_kronor() {
        assert_eq!(Ore::parse("1000"), Some(Ore(100_000)));
    }

    #[test]
    fn parses_two_decimals_exactly() {
        assert_eq!(Ore::parse("125000.00"), Some(Ore(12_500_000)));
        assert_eq!(Ore::parse("0.05"), Some(Ore(5)));
    }

    #[test]
    fn one_decimal_means_tens_of_ore() {
        assert_eq!(Ore::parse("12.5"), Some(Ore(1250)));
    }

    #[test]
    fn parses_negative_amounts() {
        assert_eq!(Ore::parse("-1250.00"), Some(Ore(-125_000)));
    }

    #[test]
    fn rejects_garbage_and_three_decimals() {
        assert_eq!(Ore::parse("abc"), None);
        assert_eq!(Ore::parse("1.234"), None);
        assert_eq!(Ore::parse("1,50"), None); // SIE uses '.', never ','
        assert_eq!(Ore::parse(""), None);
        assert_eq!(Ore::parse("."), None);
    }

    #[test]
    fn displays_back_in_sie_shape() {
        assert_eq!(Ore(12_500_000).to_string(), "125000.00");
        assert_eq!(Ore(-125_000).to_string(), "-1250.00");
        assert_eq!(Ore(5).to_string(), "0.05");
        assert_eq!(Ore(-5).to_string(), "-0.05");
    }

    #[test]
    fn sums_exactly_to_zero_like_a_balanced_voucher() {
        let rows = [Ore(-125_000), Ore(100_000), Ore(25_000)];
        assert_eq!(rows.into_iter().sum::<Ore>(), Ore::ZERO);
    }
}
