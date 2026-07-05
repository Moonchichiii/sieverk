use std::env;
use std::fs;
use std::process::ExitCode;

use codepage_437::{FromCp437, CP437_CONTROL};

mod metadata;
mod tokenizer;

/// SIE files declare `#FORMAT PC8`, which means IBM codepage 437 — a DOS-era
/// encoding. Reading them as UTF-8 turns å/ä/ö into mojibake, so decoding
/// is step zero, before any parsing.
///
/// (Real-world caveat: some exporters lie and emit ISO-8859-1 or UTF-8 anyway.
/// A robust reader eventually sniffs before assuming. Later problem.)
fn decode_sie_bytes(bytes: Vec<u8>) -> String {
    String::from_cp437(bytes, &CP437_CONTROL)
}

fn main() -> ExitCode {
    let Some(path) = env::args().nth(1) else {
        eprintln!("usage: sieverk <file.se>");
        return ExitCode::FAILURE;
    };

    let bytes = match fs::read(&path) {
        Ok(b) => b,
        Err(e) => {
            eprintln!("could not read {path}: {e}");
            return ExitCode::FAILURE;
        }
    };

    let text = decode_sie_bytes(bytes);
    let meta = metadata::parse_metadata(&text);

    fn show(v: &Option<String>) -> &str {
        v.as_deref().unwrap_or("—")
    }

    println!("sieverk — SIE inspection (early build)");
    println!("File:        {path}");
    println!(
        "SIE type:    {}",
        meta.sie_type.map_or("—".to_string(), |t| t.to_string())
    );
    println!("Company:     {}", show(&meta.company_name));
    println!("Org number:  {}", show(&meta.org_number));
    for fy in &meta.fiscal_years {
        println!(
            "Fiscal year: {} – {} (index {})",
            fy.start, fy.end, fy.index
        );
    }
    println!("Currency:    {}", show(&meta.currency));

    let tag_lines = text
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .count();
    println!("Tag lines:   {tag_lines}");

    if meta.warnings.is_empty() {
        println!("Warnings:    none");
    } else {
        for w in &meta.warnings {
            println!("Warning:     {w}");
        }
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::decode_sie_bytes;
    use crate::metadata;

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
