use std::env;
use std::fs;
use std::process::ExitCode;

use codepage_437::{FromCp437, CP437_CONTROL};

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

    println!("--- first lines, decoded from CP437 ---");
    for line in text.lines().take(12) {
        println!("{line}");
    }

    let tag_lines = text
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .count();

    println!("---");
    println!("{tag_lines} tag lines found.");
    println!();
    println!("Your move from here (see SIE-NOTES.md):");
    println!("  1. Tokenize a line into (tag, fields) — mind quoted strings.");
    println!("  2. Parse #FNAMN, #ORGNR, #SIETYP, #RAR into a Metadata struct.");
    println!("  3. Parse #KONTO rows into accounts.");
    println!("  4. Parse #VER {{ #TRANS ... }} blocks into vouchers.");
    println!("  5. Validate: every voucher's TRANS amounts must sum to zero.");

    // The moment your tokenizer goes green, this line comes alive:
    if let Some(first) = text.lines().find(|l| l.trim_start().starts_with('#')) {
        println!();
        println!("tokenized first line: {:?}", tokenizer::tokenize(first));
    }

    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::decode_sie_bytes;

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
