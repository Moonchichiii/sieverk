//! Metadata parser — step 2 of the SIE parser.
//!
//! Walks the tokenized lines and collects file-level header data: company,
//! org number, SIE type, fiscal years, currency, and friends. Anything it
//! does not recognize is skipped silently — unknown-tag reporting belongs
//! to the validator once the full tag set is handled. Malformed metadata
//! never panics; it becomes a warning and the field stays empty.

use crate::tokenizer::{tokenize, Token};

/// File-level header data from a SIE file. Every field is optional —
/// real-world files are missing things all the time, and reporting that
/// honestly beats crashing over it.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Metadata {
    pub sie_type: Option<u8>,
    pub company_name: Option<String>,
    pub org_number: Option<String>,
    pub fiscal_years: Vec<FiscalYear>,
    pub format: Option<String>,
    pub currency: Option<String>,
    pub program: Option<String>,
    pub generated_at: Option<String>,
    pub warnings: Vec<String>,
}

#[derive(Debug, PartialEq, Eq)]
pub struct FiscalYear {
    /// 0 = current year, -1 = previous, and so on.
    pub index: i32,
    /// Raw YYYYMMDD as written in the file. Proper date types arrive
    /// together with voucher parsing.
    pub start: String,
    pub end: String,
}

/// Extract header metadata from decoded SIE text.
pub fn parse_metadata(text: &str) -> Metadata {
    let mut meta = Metadata::default();

    for line in text.lines() {
        let tokens = tokenize(line);
        let mut words = tokens.iter().filter_map(|t| match t {
            Token::Word(w) => Some(w.as_str()),
            _ => None,
        });
        let Some(tag) = words.next() else { continue };

        match tag {
            "#FNAMN" => meta.company_name = words.next().map(str::to_string),
            "#ORGNR" => meta.org_number = words.next().map(str::to_string),
            "#FORMAT" => meta.format = words.next().map(str::to_string),
            "#VALUTA" => meta.currency = words.next().map(str::to_string),
            "#PROGRAM" => meta.program = words.next().map(str::to_string),
            "#GEN" => meta.generated_at = words.next().map(str::to_string),
            "#SIETYP" => match words.next().map(str::parse::<u8>) {
                Some(Ok(t)) => meta.sie_type = Some(t),
                Some(Err(_)) => meta
                    .warnings
                    .push("#SIETYP has a non-numeric value".to_string()),
                None => meta
                    .warnings
                    .push("#SIETYP is missing its value".to_string()),
            },
            "#RAR" => {
                let index = words.next().and_then(|w| w.parse::<i32>().ok());
                let start = words.next().map(str::to_string);
                let end = words.next().map(str::to_string);
                match (index, start, end) {
                    (Some(index), Some(start), Some(end)) => {
                        meta.fiscal_years.push(FiscalYear { index, start, end });
                    }
                    _ => meta.warnings.push(format!("#RAR row is malformed: {line}")),
                }
            }
            _ => {} // not metadata — someone else's job
        }
    }

    meta
}

#[cfg(test)]
mod tests {
    use super::*;

    const HEADER: &str = "\
#FLAGGA 0
#FORMAT PC8
#SIETYP 4
#FNAMN \"Demo Skogsbruk AB\"
#ORGNR 999999-9999
#RAR 0 20260101 20261231
#RAR -1 20250101 20251231
#VALUTA SEK
";

    #[test]
    fn parses_company_and_org_number() {
        let m = parse_metadata(HEADER);
        assert_eq!(m.company_name.as_deref(), Some("Demo Skogsbruk AB"));
        assert_eq!(m.org_number.as_deref(), Some("999999-9999"));
    }

    #[test]
    fn parses_sie_type_as_number() {
        assert_eq!(parse_metadata(HEADER).sie_type, Some(4));
    }

    #[test]
    fn parses_fiscal_years_in_file_order() {
        let m = parse_metadata(HEADER);
        assert_eq!(m.fiscal_years.len(), 2);
        assert_eq!(m.fiscal_years[0].index, 0);
        assert_eq!(m.fiscal_years[0].start, "20260101");
        assert_eq!(m.fiscal_years[0].end, "20261231");
        assert_eq!(m.fiscal_years[1].index, -1);
    }

    #[test]
    fn empty_input_yields_empty_metadata_without_panic() {
        let m = parse_metadata("");
        assert_eq!(m.sie_type, None);
        assert!(m.company_name.is_none());
        assert!(m.fiscal_years.is_empty());
        assert!(m.warnings.is_empty());
    }

    #[test]
    fn non_numeric_sietyp_becomes_warning_not_panic() {
        let m = parse_metadata("#SIETYP fyra");
        assert_eq!(m.sie_type, None);
        assert_eq!(m.warnings.len(), 1);
    }

    #[test]
    fn malformed_rar_becomes_warning_not_panic() {
        let m = parse_metadata("#RAR 0 20260101");
        assert!(m.fiscal_years.is_empty());
        assert_eq!(m.warnings.len(), 1);
    }

    #[test]
    fn voucher_lines_do_not_confuse_metadata() {
        let text = "#VER A 1 20260312 \"Diesel\"\n{\n#TRANS 1930 {} -1250.00\n}\n#FNAMN \"Efter verifikat AB\"";
        let m = parse_metadata(text);
        assert_eq!(m.company_name.as_deref(), Some("Efter verifikat AB"));
        assert!(m.warnings.is_empty());
    }
}
