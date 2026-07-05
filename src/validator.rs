//! Validator — step 5 of the SIE parser. The judge.
//!
//! The parsers are deliberately tolerant: they collect what the file
//! says and never lose data. This module is where opinions live. It
//! takes the parsed structures and produces findings with severities:
//! errors make a file Invalid, warnings make it Valid-with-warnings.
//!
//! Rules in this first round:
//!   * VOUCHER_NOT_BALANCED (error)   — TRANS rows must sum to exactly 0.
//!   * DUPLICATE_VOUCHER_NUMBER (warn) — series+number must be unique.
//!   * VOUCHER_DATE_OUTSIDE_YEAR (warn) — date outside #RAR 0.
//!   * VOUCHER_DATE_MALFORMED (warn)  — date is not eight digits.
//!   * TRANS_ACCOUNT_NOT_DECLARED (warn) — no #KONTO row for the account.
//!     Skipped entirely when the file declares no accounts at all —
//!     voucher-only 4i-style files are legitimate and would otherwise
//!     drown in noise.

use std::collections::HashSet;
use std::fmt;

use crate::accounts::AccountData;
use crate::metadata::Metadata;
use crate::money::Ore;
use crate::vouchers::VoucherData;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Warning,
    Error,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Code {
    VoucherNotBalanced,
    DuplicateVoucherNumber,
    VoucherDateOutsideYear,
    VoucherDateMalformed,
    TransAccountNotDeclared,
}

impl Code {
    pub fn as_str(self) -> &'static str {
        match self {
            Code::VoucherNotBalanced => "VOUCHER_NOT_BALANCED",
            Code::DuplicateVoucherNumber => "DUPLICATE_VOUCHER_NUMBER",
            Code::VoucherDateOutsideYear => "VOUCHER_DATE_OUTSIDE_YEAR",
            Code::VoucherDateMalformed => "VOUCHER_DATE_MALFORMED",
            Code::TransAccountNotDeclared => "TRANS_ACCOUNT_NOT_DECLARED",
        }
    }
}

#[derive(Debug)]
pub struct Finding {
    pub severity: Severity,
    pub code: Code,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Valid,
    ValidWithWarnings,
    Invalid,
}

impl fmt::Display for Status {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Status::Valid => "Valid",
            Status::ValidWithWarnings => "Valid with warnings",
            Status::Invalid => "Invalid",
        })
    }
}

#[derive(Debug, Default)]
pub struct Report {
    pub findings: Vec<Finding>,
}

impl Report {
    pub fn error_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Error)
            .count()
    }

    pub fn warning_count(&self) -> usize {
        self.findings
            .iter()
            .filter(|f| f.severity == Severity::Warning)
            .count()
    }

    pub fn status(&self) -> Status {
        if self.error_count() > 0 {
            Status::Invalid
        } else if self.warning_count() > 0 {
            Status::ValidWithWarnings
        } else {
            Status::Valid
        }
    }

    fn push(&mut self, severity: Severity, code: Code, message: String) {
        self.findings.push(Finding {
            severity,
            code,
            message,
        });
    }
}

/// Run every validation rule and return the verdict material.
pub fn validate(meta: &Metadata, acc: &AccountData, vou: &VoucherData) -> Report {
    let mut report = Report::default();

    let declared: HashSet<&str> = acc.accounts.iter().map(|a| a.number.as_str()).collect();
    let current_year = meta.fiscal_years.iter().find(|f| f.index == 0);
    let mut seen: HashSet<(&str, &str)> = HashSet::new();

    for v in &vou.vouchers {
        let label = format!("{}-{}", v.series, v.number);

        let sum: Ore = v.rows.iter().map(|r| r.amount).sum();
        if sum != Ore::ZERO {
            report.push(
                Severity::Error,
                Code::VoucherNotBalanced,
                format!("{label} sums to {sum}"),
            );
        }

        if !seen.insert((v.series.as_str(), v.number.as_str())) {
            report.push(
                Severity::Warning,
                Code::DuplicateVoucherNumber,
                format!("{label} appears more than once"),
            );
        }

        if v.date.len() == 8 && v.date.bytes().all(|b| b.is_ascii_digit()) {
            if let Some(fy) = current_year {
                if v.date < fy.start || v.date > fy.end {
                    report.push(
                        Severity::Warning,
                        Code::VoucherDateOutsideYear,
                        format!(
                            "{label} is dated {} outside fiscal year {} – {}",
                            v.date, fy.start, fy.end
                        ),
                    );
                }
            }
        } else {
            report.push(
                Severity::Warning,
                Code::VoucherDateMalformed,
                format!("{label} has a malformed date '{}'", v.date),
            );
        }

        if !declared.is_empty() {
            for row in &v.rows {
                if !declared.contains(row.account.as_str()) {
                    report.push(
                        Severity::Warning,
                        Code::TransAccountNotDeclared,
                        format!(
                            "{label} uses account {} which has no #KONTO row",
                            row.account
                        ),
                    );
                }
            }
        }
    }

    report
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{accounts, metadata, vouchers};

    fn run(text: &str) -> Report {
        validate(
            &metadata::parse_metadata(text),
            &accounts::parse_accounts(text),
            &vouchers::parse_vouchers(text),
        )
    }

    const BALANCED: &str = "\
#RAR 0 20260101 20261231
#KONTO 1930 \"Bank\"
#KONTO 5611 \"Drivmedel\"
#VER A 1 20260312 \"Diesel\"
{
#TRANS 1930 {} -1000.00
#TRANS 5611 {} 1000.00
}
";

    #[test]
    fn balanced_voucher_yields_no_findings() {
        let r = run(BALANCED);
        assert!(r.findings.is_empty());
        assert_eq!(r.status(), Status::Valid);
    }

    #[test]
    fn unbalanced_voucher_is_an_error_with_exact_difference() {
        let text = BALANCED.replace("-1000.00", "-1100.00");
        let r = run(&text);
        assert_eq!(r.error_count(), 1);
        assert_eq!(r.findings[0].code, Code::VoucherNotBalanced);
        assert_eq!(r.findings[0].message, "A-1 sums to -100.00");
        assert_eq!(r.status(), Status::Invalid);
    }

    #[test]
    fn date_outside_fiscal_year_warns() {
        let text = BALANCED.replace("20260312", "20250312");
        let r = run(&text);
        assert_eq!(r.warning_count(), 1);
        assert_eq!(r.findings[0].code, Code::VoucherDateOutsideYear);
        assert_eq!(r.status(), Status::ValidWithWarnings);
    }

    #[test]
    fn malformed_date_warns_instead_of_crashing() {
        let text = BALANCED.replace("20260312", "2026031");
        let r = run(&text);
        assert_eq!(r.findings[0].code, Code::VoucherDateMalformed);
        assert_eq!(r.status(), Status::ValidWithWarnings);
    }

    #[test]
    fn undeclared_account_warns() {
        let text = BALANCED.replace("#KONTO 5611 \"Drivmedel\"\n", "");
        let r = run(&text);
        assert_eq!(r.warning_count(), 1);
        assert_eq!(r.findings[0].code, Code::TransAccountNotDeclared);
    }

    #[test]
    fn voucher_only_files_skip_the_declaration_check() {
        // 4i-style: no #KONTO rows at all — silence, not a warning storm.
        let text = "#RAR 0 20260101 20261231\n#VER A 1 20260312\n{\n#TRANS 1930 {} -1.00\n#TRANS 5611 {} 1.00\n}\n";
        let r = run(text);
        assert!(r.findings.is_empty());
    }

    #[test]
    fn duplicate_voucher_number_warns() {
        let text = format!(
            "{BALANCED}#VER A 1 20260401\n{{\n#TRANS 1930 {{}} -1.00\n#TRANS 5611 {{}} 1.00\n}}\n"
        );
        let r = run(&text);
        assert_eq!(r.warning_count(), 1);
        assert_eq!(r.findings[0].code, Code::DuplicateVoucherNumber);
    }

    #[test]
    fn errors_and_warnings_combine_into_invalid() {
        let text = BALANCED
            .replace("-1000.00", "-1100.00")
            .replace("20260312", "20250312");
        let r = run(&text);
        assert_eq!(r.error_count(), 1);
        assert_eq!(r.warning_count(), 1);
        assert_eq!(r.status(), Status::Invalid);
    }
}
