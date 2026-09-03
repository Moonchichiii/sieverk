//! Chart profile masterdata (SV-02, docs/SV-02-forspec.md + docs/mastermatris-v1.2-amendment.md).
//!
//! `LANTBRUK_K1` is complete for the supported profile, never full BAS. Every
//! account carries who owns its number/name (`source_id`) separately from why it
//! is in the profile (`must_include_origin`). Loading re-runs the structural
//! invariants (V4 + provenance) because a generated file can be hand-edited:
//! malformed input is a readable `MasterdataError`, never a panic.

use std::collections::{BTreeMap, HashSet};
use std::fmt;

use serde::Deserialize;

/// Any masterdata load/validation failure. `path` points at the offending
/// element (`accounts[12]`, `cases/bransle.default`), empty for whole-file errors.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MasterdataError {
    pub path: String,
    pub message: String,
}

impl MasterdataError {
    pub fn new(path: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for MasterdataError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for MasterdataError {}

pub const CHART_SCHEMA: &str = "sieverk-chart/1";
pub const REVIEW_DRAFT: &str = "draft";
pub const REVIEW_APPROVED: &str = "approved";
pub const STATUS_APPROVED: &str = "Godkänd";
pub const ROLE_REVIEWER: &str = "accounting_reviewer";

/// Shared review columns (v1.1 §2): who approved a row, in which role.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Review {
    pub status: String,
    #[serde(default)]
    pub reviewed_by: Option<String>,
    #[serde(default)]
    pub reviewed_role: Option<String>,
    #[serde(default)]
    pub reviewed_at: Option<String>,
}

impl Review {
    /// V9: `Godkänd` needs a named reviewer in the `accounting_reviewer` role.
    /// Enforced at load time only for `approved` masterdata (v1.2 §C: a
    /// warning in draft) — see `ruleset::validate_masterdata`.
    pub fn approval_error(&self, path: &str) -> Option<MasterdataError> {
        if self.status != STATUS_APPROVED {
            return None;
        }
        match (&self.reviewed_by, self.reviewed_role.as_deref()) {
            (Some(by), Some(ROLE_REVIEWER)) if !by.is_empty() => None,
            (None, _) | (Some(_), _) => Some(MasterdataError::new(
                path,
                "Godkänd utan granskad_av i rollen accounting_reviewer",
            )),
        }
    }
}

/// Header every generated artefact carries. Deliberately without any
/// wall-clock field (V14): two builds of the same workbook are byte-identical.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Header {
    pub workbook: String,
    pub workbook_sha256: String,
    pub taxonomy_version: String,
    pub generator_version: String,
    pub review_status: String,
    #[serde(default)]
    pub profile_scope: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Source {
    pub name: String,
    #[serde(default)]
    pub publisher: String,
    #[serde(default)]
    pub reference: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub retrieved: Option<String>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub rights: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountRole {
    pub active: bool,
    pub user_selectable: bool,
    pub engine_proposable: bool,
    pub return_sie_allowed: bool,
    pub closing_account: bool,
    #[serde(default)]
    pub business_groups: Vec<String>,
}

/// An explicit human decision where the K1 table and the 2026 cross-check
/// disagree (M1). All three parts are required: a decision without a person
/// or a date is not a decision.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct SourceDecision {
    pub reason: String,
    pub decided_by: String,
    pub decided_at: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Account {
    pub number: String,
    pub name: String,
    pub kind: String,
    pub source_id: String,
    /// Mandatory per v1.2 §B — a missing key fails deserialisation, an empty
    /// value fails validation.
    pub source_version: String,
    pub must_include: bool,
    #[serde(default)]
    pub must_include_origin: Option<String>,
    #[serde(default)]
    pub source_decision: Option<SourceDecision>,
    pub roles: AccountRole,
    pub review: Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChartProfile {
    #[serde(rename = "_header")]
    pub header: Header,
    pub schema: String,
    pub chart_id: String,
    pub version: String,
    pub framework: String,
    pub entity_type: String,
    #[serde(default)]
    pub profile_scope: String,
    pub review_status: String,
    pub sources: BTreeMap<String, Source>,
    pub accounts: Vec<Account>,
}

/// How a number seen in a return SIE relates to the profile (D13 v1.12):
/// outside-profile accounts are a warning, never a crash.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccountStatus {
    /// In the profile and allowed in return files without a warning.
    InProfile,
    /// In the profile but not `return_sie_allowed` (finding, not error).
    InProfileNotReturnAllowed,
    /// Well-formed BAS-shaped number outside the profile — consultant-introduced.
    ConsultantIntroduced,
    /// Not a four-digit account number at all.
    Malformed,
}

pub const KINDS: [&str; 5] = ["tillgång", "skuld", "eget_kapital", "intäkt", "kostnad"];
pub const ORIGINS: [&str; 3] = ["VIBEKE_K1_LIST", "RULESET", "ENGINE"];

pub fn is_account_number(text: &str) -> bool {
    text.len() == 4 && text.bytes().all(|b| b.is_ascii_digit())
}

/// The account class an account number's first digit implies; `None` where
/// BAS leaves the choice to the row (class 2 = skuld|eget_kapital, class 8 =
/// intäkt|kostnad).
fn implied_kind(number: &str) -> Option<&'static str> {
    match number.as_bytes()[0] {
        b'1' => Some("tillgång"),
        b'3' => Some("intäkt"),
        b'4'..=b'7' => Some("kostnad"),
        _ => None,
    }
}

pub fn parse_chart(bytes: &[u8]) -> Result<ChartProfile, MasterdataError> {
    let chart: ChartProfile = serde_json::from_slice(bytes)
        .map_err(|e| MasterdataError::new("", format!("invalid chart JSON: {e}")))?;
    validate_chart(&chart)?;
    Ok(chart)
}

pub fn validate_chart(chart: &ChartProfile) -> Result<(), MasterdataError> {
    if chart.schema != CHART_SCHEMA {
        return Err(MasterdataError::new(
            "schema",
            format!("expected {CHART_SCHEMA}, got {}", chart.schema),
        ));
    }
    if chart.review_status != REVIEW_DRAFT && chart.review_status != REVIEW_APPROVED {
        return Err(MasterdataError::new(
            "review_status",
            format!("must be draft or approved, got {:?}", chart.review_status),
        ));
    }
    if chart.header.review_status != chart.review_status {
        return Err(MasterdataError::new(
            "_header.review_status",
            "does not match review_status",
        ));
    }
    if chart.accounts.is_empty() {
        return Err(MasterdataError::new("accounts", "chart has no accounts"));
    }
    let mut seen = HashSet::new();
    for (i, a) in chart.accounts.iter().enumerate() {
        let path = format!("accounts[{i}]/{}", a.number);
        // V4 — four digits, unique, class agrees with the first digit.
        if !is_account_number(&a.number) {
            return Err(MasterdataError::new(
                &path,
                "account number must be exactly four digits",
            ));
        }
        if !seen.insert(a.number.clone()) {
            return Err(MasterdataError::new(&path, "duplicate account number"));
        }
        if !KINDS.contains(&a.kind.as_str()) {
            return Err(MasterdataError::new(
                &path,
                format!("unknown kind {:?}", a.kind),
            ));
        }
        match (implied_kind(&a.number), a.number.as_bytes()[0]) {
            (Some(k), _) if k != a.kind => {
                return Err(MasterdataError::new(
                    &path,
                    format!("kind {:?} contradicts first digit (expected {k})", a.kind),
                ))
            }
            (None, b'2') if a.kind != "skuld" && a.kind != "eget_kapital" => {
                return Err(MasterdataError::new(
                    &path,
                    "class 2 must be skuld or eget_kapital",
                ))
            }
            (None, b'8') if a.kind != "intäkt" && a.kind != "kostnad" => {
                return Err(MasterdataError::new(
                    &path,
                    "class 8 must be intäkt or kostnad",
                ))
            }
            _ => {}
        }
        if a.name.trim().is_empty() {
            return Err(MasterdataError::new(&path, "name is empty"));
        }
        // Provenance (v1.2 §B) — every account is owned by a declared source.
        if !chart.sources.contains_key(&a.source_id) {
            return Err(MasterdataError::new(
                &path,
                format!("source_id {:?} is not declared in sources", a.source_id),
            ));
        }
        if a.source_version.trim().is_empty() {
            return Err(MasterdataError::new(
                &path,
                "source_version is mandatory (v1.2 §B)",
            ));
        }
        if let Some(d) = &a.source_decision {
            if d.reason.trim().is_empty()
                || d.decided_by.trim().is_empty()
                || d.decided_at.trim().is_empty()
            {
                return Err(MasterdataError::new(
                    &path,
                    "source_decision needs reason, decided_by and decided_at",
                ));
            }
        }
        if a.must_include {
            match a.must_include_origin.as_deref() {
                Some(o) if ORIGINS.contains(&o) => {}
                _ => {
                    return Err(MasterdataError::new(
                        &path,
                        "must_include requires must_include_origin ∈ VIBEKE_K1_LIST|RULESET|ENGINE",
                    ))
                }
            }
        }
    }
    Ok(())
}

impl ChartProfile {
    pub fn account(&self, number: &str) -> Option<&Account> {
        self.accounts.iter().find(|a| a.number == number)
    }

    pub fn is_draft(&self) -> bool {
        self.review_status == REVIEW_DRAFT
    }

    /// Return-SIE tolerance: never an error for an unknown but well-formed
    /// number — the consultant may introduce accounts the profile lacks.
    pub fn classify(&self, number: &str) -> AccountStatus {
        if !is_account_number(number) {
            return AccountStatus::Malformed;
        }
        match self.account(number) {
            Some(a) if a.roles.return_sie_allowed => AccountStatus::InProfile,
            Some(_) => AccountStatus::InProfileNotReturnAllowed,
            None => AccountStatus::ConsultantIntroduced,
        }
    }

    /// Accounts the engine may propose for a case (active + engine_proposable).
    pub fn proposable(&self, number: &str) -> bool {
        self.account(number)
            .is_some_and(|a| a.roles.active && a.roles.engine_proposable)
    }

    pub fn must_include_numbers(&self) -> Vec<&str> {
        self.accounts
            .iter()
            .filter(|a| a.must_include)
            .map(|a| a.number.as_str())
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn minimal(accounts: &str) -> String {
        format!(
            r#"{{"_header":{{"workbook":"w.xlsx","workbook_sha256":"0",
              "taxonomy_version":"1.0","generator_version":"mastermatris-gen/1","review_status":"draft"}},
              "schema":"sieverk-chart/1","chart_id":"T","version":"2026.1","framework":"BAS",
              "entity_type":"enskild_firma","review_status":"draft",
              "sources":{{"SYNTHETIC":{{"name":"s"}}}},"accounts":[{accounts}]}}"#
        )
    }

    fn acc(number: &str, kind: &str) -> String {
        format!(
            r#"{{"number":"{number}","name":"n","kind":"{kind}","source_id":"SYNTHETIC","source_version":"1","must_include":false,
               "roles":{{"active":true,"user_selectable":true,"engine_proposable":true,"return_sie_allowed":true,
               "closing_account":false}},"review":{{"status":"Utkast"}}}}"#
        )
    }

    #[test]
    fn valid_minimal_chart_loads() {
        let chart = parse_chart(minimal(&acc("1930", "tillgång")).as_bytes()).expect("test value");
        assert_eq!(chart.accounts.len(), 1);
        assert_eq!(chart.classify("1930"), AccountStatus::InProfile);
        assert_eq!(chart.classify("4470"), AccountStatus::ConsultantIntroduced);
        assert_eq!(chart.classify("abc"), AccountStatus::Malformed);
    }

    #[test]
    fn v4_duplicate_number_is_a_readable_error() {
        let json = minimal(&format!(
            "{},{}",
            acc("1930", "tillgång"),
            acc("1930", "tillgång")
        ));
        let err = parse_chart(json.as_bytes()).expect_err("must fail");
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[test]
    fn v4_kind_must_match_first_digit() {
        let err = parse_chart(minimal(&acc("1930", "kostnad")).as_bytes()).expect_err("must fail");
        assert!(err.to_string().contains("contradicts"), "{err}");
    }

    #[test]
    fn v4_number_must_be_four_digits() {
        let err = parse_chart(minimal(&acc("193", "tillgång")).as_bytes()).expect_err("must fail");
        assert!(err.to_string().contains("four digits"), "{err}");
    }

    #[test]
    fn unknown_source_is_rejected() {
        let json = minimal(&acc("1930", "tillgång"))
            .replace("\"source_id\":\"SYNTHETIC\"", "\"source_id\":\"NOPE\"");
        let err = parse_chart(json.as_bytes()).expect_err("must fail");
        assert!(err.to_string().contains("source_id"), "{err}");
    }

    #[test]
    fn source_version_is_mandatory_and_source_decision_is_structured() {
        let missing = minimal(&acc("1930", "tillgång")).replace("\"source_version\":\"1\",", "");
        assert!(
            parse_chart(missing.as_bytes()).is_err(),
            "missing source_version must fail"
        );
        let empty = minimal(&acc("1930", "tillgång"))
            .replace("\"source_version\":\"1\"", "\"source_version\":\" \"");
        let e = parse_chart(empty.as_bytes())
            .expect_err("must fail")
            .to_string();
        assert!(e.contains("source_version"), "{e}");
        let ok = minimal(&acc("1930", "tillgång")).replace(
            "\"must_include\":false,",
            "\"must_include\":false,\"source_decision\":{\"reason\":\"r\",\"decided_by\":\"Mats\",\"decided_at\":\"2026-09-03\"},",
        );
        let chart = parse_chart(ok.as_bytes()).expect("structured decision loads");
        assert_eq!(
            chart.accounts[0]
                .source_decision
                .as_ref()
                .expect("test value")
                .decided_by,
            "Mats"
        );
        let string_form = minimal(&acc("1930", "tillgång")).replace(
            "\"must_include\":false,",
            "\"must_include\":false,\"source_decision\":\"r | Mats | 2026-09-03\",",
        );
        assert!(
            parse_chart(string_form.as_bytes()).is_err(),
            "a bare string is not a decision"
        );
        let blank_part = ok.replace("\"decided_by\":\"Mats\"", "\"decided_by\":\"\"");
        assert!(parse_chart(blank_part.as_bytes()).is_err());
    }

    #[test]
    fn garbage_json_never_panics() {
        assert!(parse_chart(b"{not json").is_err());
        assert!(parse_chart(b"[]").is_err());
    }
}
