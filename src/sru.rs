//! SRU masterdata (SV-02 M4): load and validate the field-code table. This
//! module knows nothing about the SIE file format — `#SRU`/`#TAXAR` mechanics
//! belong to SRU-01 or a later writer drop. A row is data only when its 2026
//! validity was verified; unverified rows are carried but flagged, never
//! silently trusted.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::chart::{is_account_number, ChartProfile, Header, MasterdataError, Review};

pub const SRU_SCHEMA: &str = "sieverk-sru/1";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SruRow {
    pub account: String,
    pub sru: String,
    pub form: String,
    #[serde(default)]
    pub valid_from: Option<u32>,
    #[serde(default)]
    pub valid_to: Option<u32>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub source_version: Option<String>,
    #[serde(default)]
    pub verified_at: Option<String>,
    #[serde(default)]
    pub verified: bool,
    pub review: Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SruTable {
    #[serde(rename = "_header")]
    pub header: Header,
    pub schema: String,
    pub rows: Vec<SruRow>,
}

pub fn parse_sru(bytes: &[u8]) -> Result<SruTable, MasterdataError> {
    let table: SruTable = serde_json::from_slice(bytes)
        .map_err(|e| MasterdataError::new("", format!("invalid SRU JSON: {e}")))?;
    if table.schema != SRU_SCHEMA {
        return Err(MasterdataError::new(
            "schema",
            format!("expected {SRU_SCHEMA}, got {}", table.schema),
        ));
    }
    for (i, row) in table.rows.iter().enumerate() {
        let path = format!("rows[{i}]/{}", row.account);
        if !is_account_number(&row.account) {
            return Err(MasterdataError::new(&path, "account must be four digits"));
        }
        if !is_account_number(&row.sru) {
            return Err(MasterdataError::new(&path, "sru code must be four digits"));
        }
        if let (Some(from), Some(to)) = (row.valid_from, row.valid_to) {
            if to < from {
                return Err(MasterdataError::new(&path, "valid_to before valid_from"));
            }
        }
    }
    Ok(table)
}

/// V6 against the chart: every row points at a chart account and no two rows
/// for the same account overlap in validity.
pub fn validate_sru(table: &SruTable, chart: &ChartProfile) -> Result<(), MasterdataError> {
    let mut spans: BTreeMap<&str, Vec<(u32, u32)>> = BTreeMap::new();
    for (i, row) in table.rows.iter().enumerate() {
        let path = format!("rows[{i}]/{}", row.account);
        if chart.account(&row.account).is_none() {
            return Err(MasterdataError::new(&path, "account is not in the chart"));
        }
        spans.entry(row.account.as_str()).or_default().push((
            row.valid_from.unwrap_or(0),
            row.valid_to.unwrap_or(u32::MAX),
        ));
    }
    for (account, list) in spans.iter_mut() {
        list.sort_unstable();
        for pair in list.windows(2) {
            if pair[1].0 <= pair[0].1 {
                return Err(MasterdataError::new(
                    format!("rows/{account}"),
                    "overlapping validity for the same account",
                ));
            }
        }
    }
    Ok(())
}

impl SruTable {
    /// Rows usable as data: verified for their year. Unverified rows are
    /// carried for review but never returned here.
    pub fn verified_for(&self, account: &str, year: u32) -> Option<&SruRow> {
        self.rows.iter().find(|r| {
            r.account == account
                && r.verified
                && r.valid_from.is_none_or(|f| f <= year)
                && r.valid_to.is_none_or(|t| year <= t)
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table(rows: &str) -> String {
        format!(
            r#"{{"_header":{{"workbook":"w.xlsx","workbook_sha256":"0",
              "taxonomy_version":"1.0","generator_version":"mastermatris-gen/1","review_status":"draft"}},
              "schema":"sieverk-sru/1","rows":[{rows}]}}"#
        )
    }

    fn row(account: &str, from: &str, to: &str, verified: bool) -> String {
        format!(
            r#"{{"account":"{account}","sru":"7200","form":"NE","valid_from":{from},"valid_to":{to},
               "verified":{verified},"review":{{"status":"Utkast"}}}}"#
        )
    }

    #[test]
    fn unverified_rows_are_never_data() {
        let t =
            parse_sru(table(&row("3420", "2026", "null", false)).as_bytes()).expect("test value");
        assert!(t.verified_for("3420", 2026).is_none());
        let t =
            parse_sru(table(&row("3420", "2026", "null", true)).as_bytes()).expect("test value");
        assert!(t.verified_for("3420", 2026).is_some());
        assert!(t.verified_for("3420", 2025).is_none());
    }

    #[test]
    fn bad_code_is_rejected_without_panic() {
        let json =
            table(&row("3420", "2026", "null", true)).replace("\"sru\":\"7200\"", "\"sru\":\"72\"");
        assert!(parse_sru(json.as_bytes()).is_err());
        assert!(parse_sru(b"nope").is_err());
    }
}
