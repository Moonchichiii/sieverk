use std::env;
use std::fs;
use std::process::ExitCode;

use codepage_437::{FromCp437, CP437_CONTROL};

mod accounts;
mod metadata;
mod money;
mod tokenizer;
mod validator;
mod vouchers;

/// SIE files declare `#FORMAT PC8`, which means IBM codepage 437 — a DOS-era
/// encoding. Reading them as UTF-8 turns å/ä/ö into mojibake, so decoding
/// is step zero, before any parsing.
///
/// (Real-world caveat: some exporters lie and emit ISO-8859-1 or UTF-8 anyway.
/// A robust reader eventually sniffs before assuming. Later problem.)
fn decode_sie_bytes(bytes: Vec<u8>) -> String {
    String::from_cp437(bytes, &CP437_CONTROL)
}

struct Parsed {
    meta: metadata::Metadata,
    acc: accounts::AccountData,
    vou: vouchers::VoucherData,
    report: validator::Report,
    tag_lines: usize,
}

fn load_and_parse(path: &str) -> Result<Parsed, String> {
    let bytes = fs::read(path).map_err(|e| format!("could not read {path}: {e}"))?;
    let text = decode_sie_bytes(bytes);
    let meta = metadata::parse_metadata(&text);
    let acc = accounts::parse_accounts(&text);
    let vou = vouchers::parse_vouchers(&text);
    let report = validator::validate(&meta, &acc, &vou);
    let tag_lines = text
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .count();
    Ok(Parsed {
        meta,
        acc,
        vou,
        report,
        tag_lines,
    })
}

fn main() -> ExitCode {
    let mut args = env::args().skip(1);
    // Two subcommands do not justify a parser-generator dependency.
    // A bare path is treated as inspect-sie, which also keeps the CI
    // smoke step (`cargo run -- fixtures/...`) working unchanged.
    let (command, path) = match (args.next(), args.next()) {
        (Some(cmd), Some(path)) if cmd == "inspect-sie" || cmd == "validate-sie" => (cmd, path),
        (Some(path), None) => ("inspect-sie".to_string(), path),
        _ => {
            eprintln!("usage: sieverk <inspect-sie|validate-sie> <file.se>");
            return ExitCode::FAILURE;
        }
    };

    let parsed = match load_and_parse(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    match command.as_str() {
        "validate-sie" => run_validate(&path, &parsed),
        _ => run_inspect(&path, &parsed),
    }
}

fn run_inspect(path: &str, p: &Parsed) -> ExitCode {
    fn show(v: &Option<String>) -> &str {
        v.as_deref().unwrap_or("—")
    }

    println!("sieverk — SIE inspection");
    println!("File:        {path}");
    println!(
        "SIE type:    {}",
        p.meta.sie_type.map_or("—".to_string(), |t| t.to_string())
    );
    println!("Company:     {}", show(&p.meta.company_name));
    println!("Org number:  {}", show(&p.meta.org_number));
    for fy in &p.meta.fiscal_years {
        println!(
            "Fiscal year: {} – {} (index {})",
            fy.start, fy.end, fy.index
        );
    }
    println!("Currency:    {}", show(&p.meta.currency));
    println!("Accounts:    {}", p.acc.accounts.len());
    println!("Balances:    {} rows", p.acc.balances.len());
    println!("Vouchers:    {}", p.vou.vouchers.len());
    println!(
        "Ledger rows: {}",
        p.vou.vouchers.iter().map(|v| v.rows.len()).sum::<usize>()
    );
    println!("Tag lines:   {}", p.tag_lines);

    let parser_warnings = p.meta.warnings.len() + p.acc.warnings.len() + p.vou.warnings.len();
    println!("Parser notes: {parser_warnings}");
    println!(
        "Findings:    {} errors, {} warnings",
        p.report.error_count(),
        p.report.warning_count()
    );
    println!("Status:      {}", p.report.status());

    ExitCode::SUCCESS
}

fn run_validate(path: &str, p: &Parsed) -> ExitCode {
    println!("sieverk — SIE validation");
    println!("File:        {path}");

    for f in &p.report.findings {
        let sev = match f.severity {
            validator::Severity::Error => "ERROR  ",
            validator::Severity::Warning => "WARNING",
        };
        println!("{sev} {}: {}", f.code.as_str(), f.message);
    }
    for w in p
        .meta
        .warnings
        .iter()
        .chain(p.acc.warnings.iter())
        .chain(p.vou.warnings.iter())
    {
        println!("PARSER  {w}");
    }

    println!(
        "Errors:      {}  Warnings: {}",
        p.report.error_count(),
        p.report.warning_count()
    );
    println!("Status:      {}", p.report.status());

    if p.report.error_count() > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

#[cfg(test)]
mod tests {
    use super::decode_sie_bytes;
    use crate::{accounts, metadata, validator, vouchers};

    /// The week-one finish line, part one: the valid fixture comes out
    /// of the full pipeline with a clean verdict.
    #[test]
    fn fixture_valid_passes_validation() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let text = decode_sie_bytes(bytes);
        let report = validator::validate(
            &metadata::parse_metadata(&text),
            &accounts::parse_accounts(&text),
            &vouchers::parse_vouchers(&text),
        );
        assert!(report.findings.is_empty());
        assert_eq!(report.status(), validator::Status::Valid);
    }

    /// The week-one finish line, part two: the broken fixture finally
    /// hears its verdict — with the exact difference, in öre.
    #[test]
    fn fixture_unbalanced_fails_with_exact_difference() {
        let bytes = std::fs::read("fixtures/invalid_unbalanced.se")
            .expect("fixture file should exist — run from the crate root");
        let text = decode_sie_bytes(bytes);
        let report = validator::validate(
            &metadata::parse_metadata(&text),
            &accounts::parse_accounts(&text),
            &vouchers::parse_vouchers(&text),
        );
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.findings[0].code, validator::Code::VoucherNotBalanced);
        assert_eq!(report.findings[0].message, "A-1 sums to -100.00");
        assert_eq!(report.status(), validator::Status::Invalid);
    }

    /// Full pipeline on the real fixture: bytes → CP437 decode →
    /// tokenizer → accounts. Locks the fixture's account inventory and
    /// one exact Decimal amount, å intact and all.
    #[test]
    fn fixture_accounts_survive_full_pipeline() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let acc = accounts::parse_accounts(&decode_sie_bytes(bytes));

        assert_eq!(acc.accounts.len(), 7);
        assert_eq!(acc.balances.len(), 8);
        assert!(acc.warnings.is_empty());

        let skogsvard = acc
            .accounts
            .iter()
            .find(|a| a.number == "6390")
            .expect("account 6390 should exist in the fixture");
        assert_eq!(
            skogsvard.name.as_deref(),
            Some("Skogsvård och övriga kostnader")
        );

        let opening_1930 = acc
            .balances
            .iter()
            .find(|b| b.kind == accounts::BalanceKind::Opening && b.account == "1930")
            .expect("opening balance for 1930 should exist");
        let expected =
            crate::money::Ore::parse("125000.00").expect("test literal should be a valid amount");
        assert_eq!(opening_1930.amount, expected);
    }

    /// Full pipeline on the real fixture: bytes → CP437 decode →
    /// tokenizer → vouchers. Locks both vouchers, all six rows, and one
    /// exact amount.
    #[test]
    fn fixture_vouchers_survive_full_pipeline() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let vou = vouchers::parse_vouchers(&decode_sie_bytes(bytes));

        assert_eq!(vou.vouchers.len(), 2);
        assert!(vou.warnings.is_empty());

        let first = &vou.vouchers[0];
        assert_eq!(first.series, "A");
        assert_eq!(first.text.as_deref(), Some("Diesel skogsmaskin"));
        assert_eq!(first.rows.len(), 3);
        let expected =
            crate::money::Ore::parse("-1250.00").expect("test literal should be a valid amount");
        assert_eq!(first.rows[0].amount, expected);

        assert_eq!(vou.vouchers[1].rows.len(), 3);
    }

    /// Full pipeline on the real fixture: bytes → CP437 decode →
    /// tokenizer → metadata. If this is green, the whole chain holds.
    #[test]
    fn fixture_metadata_survives_full_pipeline() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let meta = metadata::parse_metadata(&decode_sie_bytes(bytes));

        assert_eq!(meta.sie_type, Some(4));
        assert_eq!(meta.company_name.as_deref(), Some("Demo Skogsbruk AB"));
        assert_eq!(meta.org_number.as_deref(), Some("999999-9999"));
        assert_eq!(meta.fiscal_years.len(), 2);
        assert_eq!(meta.currency.as_deref(), Some("SEK"));
        assert!(meta.warnings.is_empty());
    }

    /// Green from minute one: proves the CP437 round-trip works and the
    /// fixtures are wired up. If this fails, nothing else matters yet.
    #[test]
    fn fixture_survives_cp437_decoding() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let text = decode_sie_bytes(bytes);

        assert!(text.contains("Företagskonto"), "ö did not survive decoding");
        assert!(text.contains("Skogsvård"), "å did not survive decoding");
        assert!(text.contains("Intäkter"), "ä did not survive decoding");
        assert!(text.contains("#SIETYP 4"));
    }

    #[test]
    fn unbalanced_fixture_exists_for_validator_work() {
        let bytes = std::fs::read("fixtures/invalid_unbalanced.se")
            .expect("fixture file should exist — run from the crate root");
        let text = decode_sie_bytes(bytes);

        // -1000.00 + 900.00 = -100.00: your future validator must catch this.
        assert!(text.contains("Reparation traktor"));
    }
}
