//! Accounting engine, Slice 1 (SV-03): case projection + structural assessment.
//!
//! Locked by `SV-03-prespec-report-rev2_1.md` + `SV-03-slice1-final-design-lock.md`.
//! Two public functions and nothing else:
//!
//! * `project_cases(&Workspace)` — typed DuckDB readback → one `EngineCase` per
//!   receipt (in `selector_position` order) then per income entry (in
//!   `snapshot_position` order). Source facts are copied, never normalised;
//!   canonical source keys are derived for schema-1.0 rows and verified for
//!   1.1 rows; property references and the copied money invariants are checked.
//! * `assess_cases(&[EngineCase], &EntityContext, &Masterdata)` — structural
//!   lookups against already-loaded masterdata only: default rule case,
//!   receipt counter row, legacy triggers, missing evidence, proposal guard,
//!   status ceiling. Zero accounting lines. No VAT findings (E1 is not
//!   IMPLEMENTABLE). No E1–E6. No persistence, no CLI, no SIE.
//!
//! Money stays `Ore(i64)`; every check uses checked i64 arithmetic. Nothing
//! here ever reads the snapshot JSON again.

use std::collections::HashSet;
use std::fmt;

use chrono::NaiveDate;

use crate::money::Ore;
use crate::ruleset::{AccountingCase as RuleCase, CounterRule, Masterdata};
use crate::workspace::{EntityContext, Workspace, WorkspaceError};

// ---------------------------------------------------------------------------
// Domain types (locked shape)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaseSource {
    Receipt,
    Income,
}

impl CaseSource {
    fn key_prefix(self) -> &'static str {
        match self {
            Self::Receipt => "receipt",
            Self::Income => "income",
        }
    }
}

/// The four category-context flags, copied exactly from the receipt row.
/// Schema-1.0 receipts and every income row carry `None` — never `false`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CaseContext {
    pub requires_business_share: Option<bool>,
    pub investment_risk: Option<bool>,
    pub vat_check: Option<bool>,
    pub sensitive: Option<bool>,
}

/// Source-faithful facts. A receipt and an income entry have different
/// shapes and keep them; nothing is mapped across sources.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceFacts {
    Receipt {
        total: Ore,
        vat: Ore,
        rounding: Ore,
        net: Ore,
        payment_method: Option<String>,
        entry_type: String,
        area: String,
        has_image: bool,
    },
    Income {
        ex_vat: Ore,
        vat: Ore,
        inc_vat: Ore,
        payment_date: Option<NaiveDate>,
        income_type: String,
        document_count: i32,
    },
}

/// One workspace row, in contract order. (`ruleset::AccountingCase` is the
/// masterdata *rule* case; this is the per-row instance.)
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineCase {
    pub case_seq: i32,
    pub source: CaseSource,
    /// Canonical `receipt:<id>` / `income:<id>`.
    pub source_key: String,
    pub row_id: i64,
    pub property_id: i64,
    /// Receipts only; copied, never renumbered, never a sort key.
    pub ordinal_number: Option<i32>,
    /// Row date, untransformed (E5 input; unused in Slice 1).
    pub date: NaiveDate,
    /// receipt.category / Some(income_type).
    pub subject: Option<String>,
    pub context: CaseContext,
    pub facts: SourceFacts,
}

/// The locked lifecycle. Confidence order: Automatic > Conditional > Manual.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecisionStatus {
    Automatic,
    Conditional,
    Manual,
}

impl DecisionStatus {
    fn confidence(self) -> u8 {
        match self {
            Self::Automatic => 2,
            Self::Conditional => 1,
            Self::Manual => 0,
        }
    }

    /// The less confident of two statuses. Confidence is never raised.
    pub fn least(self, other: Self) -> Self {
        if other.confidence() < self.confidence() {
            other
        } else {
            self
        }
    }

    fn from_automation(text: &str) -> Option<Self> {
        match text {
            "Automatic" => Some(Self::Automatic),
            "Conditional" => Some(Self::Conditional),
            "Manual" => Some(Self::Manual),
            _ => None,
        }
    }
}

/// Exact structural reference to the selected counter-account row — never a
/// composite string. Preserved even when the row has no account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CounterRuleRef {
    pub source: CaseSource,
    pub key: String,
    pub bookkeeping_method: Option<String>,
}

/// The seven-code contract. Declaration order is the fixed sort rank.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum FindingCode {
    UnresolvedVat,
    UnresolvedCounterAccount,
    UnmappedCategory,
    LegacyIncomeReceipt,
    UnspecifiedTimberSale,
    VatMismatch,
    MissingEvidence,
}

impl FindingCode {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::UnresolvedVat => "UNRESOLVED_VAT",
            Self::UnresolvedCounterAccount => "UNRESOLVED_COUNTER_ACCOUNT",
            Self::UnmappedCategory => "UNMAPPED_CATEGORY",
            Self::LegacyIncomeReceipt => "LEGACY_INCOME_RECEIPT",
            Self::UnspecifiedTimberSale => "UNSPECIFIED_TIMBER_SALE",
            Self::VatMismatch => "VAT_MISMATCH",
            Self::MissingEvidence => "MISSING_EVIDENCE",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Info,
    Warning,
    Blocking,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// Dense 0..n-1, assigned after sorting by `FindingCode` rank.
    pub finding_no: i32,
    pub code: FindingCode,
    pub severity: Severity,
    /// Deterministic text; no timestamps; no personal data beyond source_key.
    pub message: String,
    pub question: Option<String>,
}

/// Slice 1 output: structural assessment only. No lines exist in this slice.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseAssessment {
    pub case_seq: i32,
    /// Matched default rule case. May be `Some` together with
    /// `UNMAPPED_CATEGORY` (a default case without an account).
    pub rule_case_id: Option<String>,
    /// Preserved id only — no VAT finding is derived in Slice 1.
    pub vat_rule_id: Option<String>,
    /// Receipts: the single matching counter row; income: `None` in Slice 1.
    pub counter_rule: Option<CounterRuleRef>,
    pub status_ceiling: DecisionStatus,
    pub findings: Vec<Finding>,
}

#[derive(Debug)]
pub enum EngineError {
    Workspace(WorkspaceError),
    InvalidSourceKey {
        case_seq: i32,
        found: String,
        expected: String,
    },
    MissingProperty {
        case_seq: i32,
        property_id: i64,
    },
    MoneyInvariant {
        case_seq: i32,
        detail: String,
    },
    AmbiguousCounterRule {
        case_seq: i32,
        key: String,
        candidates: usize,
    },
    ProposalGuard {
        case_seq: i32,
        account: String,
        reason: String,
    },
    /// Masterdata carries a value the loader should already have refused
    /// (e.g. an unknown automation) — fail closed, never guess.
    Masterdata {
        case_seq: i32,
        detail: String,
    },
    /// The workspace's taxonomy version (schema 1.1) differs from the
    /// ruleset's — assessing against the wrong taxonomy is refused.
    TaxonomyMismatch {
        entity_version: String,
        ruleset_version: String,
    },
}

impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace(e) => write!(f, "workspace: {e}"),
            Self::InvalidSourceKey {
                case_seq,
                found,
                expected,
            } => write!(
                f,
                "case {case_seq}: source_key {found:?} is not the canonical {expected:?}"
            ),
            Self::MissingProperty {
                case_seq,
                property_id,
            } => write!(f, "case {case_seq}: property {property_id} is not in the workspace"),
            Self::MoneyInvariant { case_seq, detail } => {
                write!(f, "case {case_seq}: money invariant failed: {detail}")
            }
            Self::AmbiguousCounterRule {
                case_seq,
                key,
                candidates,
            } => write!(
                f,
                "case {case_seq}: {candidates} counter rules match payment_method {key:?}; refusing to choose"
            ),
            Self::ProposalGuard {
                case_seq,
                account,
                reason,
            } => write!(f, "case {case_seq}: account {account} may not be proposed: {reason}"),
            Self::Masterdata { case_seq, detail } => {
                write!(f, "case {case_seq}: masterdata: {detail}")
            }
            Self::TaxonomyMismatch {
                entity_version,
                ruleset_version,
            } => write!(
                f,
                "workspace taxonomy {entity_version} does not match ruleset taxonomy {ruleset_version}"
            ),
        }
    }
}

impl std::error::Error for EngineError {}

impl From<WorkspaceError> for EngineError {
    fn from(e: WorkspaceError) -> Self {
        Self::Workspace(e)
    }
}

// ---------------------------------------------------------------------------
// project_cases
// ---------------------------------------------------------------------------

fn canonical_key(source: CaseSource, row_id: i64) -> String {
    format!("{}:{row_id}", source.key_prefix())
}

fn verify_key(
    case_seq: i32,
    source: CaseSource,
    row_id: i64,
    stored: Option<&str>,
) -> Result<String, EngineError> {
    let expected = canonical_key(source, row_id);
    match stored {
        None => Ok(expected),
        Some(found) if found == expected => Ok(expected),
        Some(found) => Err(EngineError::InvalidSourceKey {
            case_seq,
            found: found.to_string(),
            expected,
        }),
    }
}

fn checked_sum(case_seq: i32, parts: &[i64], label: &str) -> Result<i64, EngineError> {
    parts.iter().try_fold(0i64, |acc, p| {
        acc.checked_add(*p)
            .ok_or_else(|| EngineError::MoneyInvariant {
                case_seq,
                detail: format!("{label}: i64 overflow"),
            })
    })
}

/// Typed DuckDB readback → `EngineCase`s in contract order. Reads nothing but
/// the workspace; the snapshot JSON is never opened.
pub fn project_cases(workspace: &Workspace) -> Result<Vec<EngineCase>, EngineError> {
    workspace.read_meta()?; // requires exactly one ingested snapshot
    let properties: HashSet<i64> = workspace
        .read_properties()?
        .into_iter()
        .map(|p| p.property_id)
        .collect();
    let receipts = workspace.read_receipts()?;
    let incomes = workspace.read_income_entries()?;

    let mut cases = Vec::with_capacity(receipts.len() + incomes.len());
    let mut case_seq: i32 = 0;

    for r in receipts {
        let source_key = verify_key(case_seq, CaseSource::Receipt, r.id, r.source_key.as_deref())?;
        if !properties.contains(&r.property_id) {
            return Err(EngineError::MissingProperty {
                case_seq,
                property_id: r.property_id,
            });
        }
        let sum = checked_sum(
            case_seq,
            &[r.net.0, r.vat.0, r.rounding.0],
            "net + vat + rounding",
        )?;
        if sum != r.total.0 {
            return Err(EngineError::MoneyInvariant {
                case_seq,
                detail: format!(
                    "net {} + vat {} + rounding {} != total {} (öre)",
                    r.net.0, r.vat.0, r.rounding.0, r.total.0
                ),
            });
        }
        cases.push(EngineCase {
            case_seq,
            source: CaseSource::Receipt,
            source_key,
            row_id: r.id,
            property_id: r.property_id,
            ordinal_number: r.ordinal_number,
            date: r.date,
            subject: r.category.clone(),
            context: CaseContext {
                requires_business_share: r.requires_business_share,
                investment_risk: r.investment_risk,
                vat_check: r.vat_check,
                sensitive: r.sensitive,
            },
            facts: SourceFacts::Receipt {
                total: r.total,
                vat: r.vat,
                rounding: r.rounding,
                net: r.net,
                payment_method: r.payment_method.clone(),
                entry_type: r.entry_type.clone(),
                area: r.area.clone(),
                has_image: r.has_image,
            },
        });
        case_seq += 1;
    }

    for e in incomes {
        let source_key = verify_key(case_seq, CaseSource::Income, e.id, e.source_key.as_deref())?;
        if !properties.contains(&e.property_id) {
            return Err(EngineError::MissingProperty {
                case_seq,
                property_id: e.property_id,
            });
        }
        let sum = checked_sum(case_seq, &[e.ex_vat.0, e.vat.0], "ex_vat + vat")?;
        if sum != e.inc_vat.0 {
            return Err(EngineError::MoneyInvariant {
                case_seq,
                detail: format!(
                    "ex_vat {} + vat {} != inc_vat {} (öre)",
                    e.ex_vat.0, e.vat.0, e.inc_vat.0
                ),
            });
        }
        cases.push(EngineCase {
            case_seq,
            source: CaseSource::Income,
            source_key,
            row_id: e.id,
            property_id: e.property_id,
            ordinal_number: None,
            date: e.date,
            subject: Some(e.income_type.clone()),
            context: CaseContext::default(),
            facts: SourceFacts::Income {
                ex_vat: e.ex_vat,
                vat: e.vat,
                inc_vat: e.inc_vat,
                payment_date: e.payment_date,
                income_type: e.income_type.clone(),
                document_count: e.document_count,
            },
        });
        case_seq += 1;
    }

    Ok(cases)
}

// ---------------------------------------------------------------------------
// assess_cases
// ---------------------------------------------------------------------------

fn automation(case_seq: i32, text: &str) -> Result<DecisionStatus, EngineError> {
    DecisionStatus::from_automation(text).ok_or_else(|| EngineError::Masterdata {
        case_seq,
        detail: format!("unknown automation {text:?}"),
    })
}

/// The proposal guard (mastermatris v1.1 §2.4, M3): applied only to an
/// account that masterdata would let the engine propose (its automation is
/// Automatic or Conditional). Manual references are never proposals.
fn proposal_guard(
    case_seq: i32,
    account: &str,
    status: DecisionStatus,
    entity: &EntityContext,
    masterdata: &Masterdata,
) -> Result<(), EngineError> {
    if status == DecisionStatus::Manual {
        return Ok(());
    }
    let reject = |reason: &str| EngineError::ProposalGuard {
        case_seq,
        account: account.to_string(),
        reason: reason.to_string(),
    };
    let acc = masterdata
        .chart
        .account(account)
        .ok_or_else(|| reject("not in the chart"))?;
    if !acc.roles.active {
        return Err(reject("inactive"));
    }
    if !acc.roles.engine_proposable {
        return Err(reject("not engine_proposable"));
    }
    let groups_ok = acc.roles.business_groups.is_empty()
        || acc
            .roles
            .business_groups
            .iter()
            .any(|g| entity.operations.iter().any(|o| o == g));
    if !groups_ok {
        return Err(reject("business_groups do not intersect entity.operations"));
    }
    Ok(())
}

fn finding(
    code: FindingCode,
    severity: Severity,
    message: String,
    question: Option<String>,
) -> Finding {
    Finding {
        finding_no: -1, // assigned after sorting
        code,
        severity,
        message,
        question,
    }
}

fn default_rule_case<'a>(case: &EngineCase, masterdata: &'a Masterdata) -> Option<&'a RuleCase> {
    match (case.source, case.subject.as_deref()) {
        (CaseSource::Receipt, Some(category)) => masterdata.default_case_for_category(category),
        (CaseSource::Income, Some(income_type)) => {
            masterdata.default_case_for_income_type(income_type)
        }
        (_, None) => None,
    }
}

/// Outcome of the counter lookup: the single matching row, or the finding
/// that explains why none is usable.
type CounterSelection<'a> = Result<&'a CounterRule, Finding>;

/// Locked receipt counter lookup. Returns the selected row (if exactly one),
/// or a finding when there is no usable row; more than one candidate is a
/// structural error — no precedence is invented.
fn select_counter_row<'a>(
    case: &EngineCase,
    payment_method: Option<&str>,
    entity: &EntityContext,
    masterdata: &'a Masterdata,
) -> Result<CounterSelection<'a>, EngineError> {
    let Some(key) = payment_method else {
        return Ok(Err(finding(
            FindingCode::UnresolvedCounterAccount,
            Severity::Blocking,
            "receipt has no payment_method".to_string(),
            None,
        )));
    };
    let candidates: Vec<&CounterRule> = masterdata
        .counter
        .rows
        .iter()
        .filter(|r| r.source == "receipt" && r.key == key)
        .filter(
            |r| match (&r.bookkeeping_method, &entity.bookkeeping_method) {
                (None, _) => true,
                (Some(rule_method), Some(entity_method)) => rule_method == entity_method,
                (Some(_), None) => false,
            },
        )
        .collect();
    match candidates.len() {
        0 => Ok(Err(finding(
            FindingCode::UnresolvedCounterAccount,
            Severity::Blocking,
            format!("no counter rule for payment_method {key:?}"),
            None,
        ))),
        1 => Ok(Ok(candidates[0])),
        n => Err(EngineError::AmbiguousCounterRule {
            case_seq: case.case_seq,
            key: key.to_string(),
            candidates: n,
        }),
    }
}

/// Structural assessment against loaded masterdata. Zero lines, no VAT
/// findings, no E1–E6.
pub fn assess_cases(
    cases: &[EngineCase],
    entity: &EntityContext,
    masterdata: &Masterdata,
) -> Result<Vec<CaseAssessment>, EngineError> {
    // Prerequisite (prespec §F.2): a schema-1.1 workspace names its taxonomy
    // version and it must be the one the ruleset was validated against.
    // Schema 1.0 carries None and is accepted.
    if let Some(entity_version) = &entity.taxonomy_version {
        if entity_version != &masterdata.ruleset.taxonomy_version {
            return Err(EngineError::TaxonomyMismatch {
                entity_version: entity_version.clone(),
                ruleset_version: masterdata.ruleset.taxonomy_version.clone(),
            });
        }
    }
    let draft = masterdata.is_draft();
    let mut out = Vec::with_capacity(cases.len());

    for case in cases {
        let seq = case.case_seq;
        let mut findings: Vec<Finding> = Vec::new();
        let mut ceiling = DecisionStatus::Automatic;

        // 1. Default rule case: usable ⇔ exists ∧ has an account.
        let rule = default_rule_case(case, masterdata);
        let rule_case_id = rule.map(|r| r.case_id.clone());
        let vat_rule_id = rule.and_then(|r| r.vat_rule.clone());
        match rule {
            Some(r) if r.account.is_some() => {
                let status = automation(seq, &r.automation)?;
                if let Some(account) = &r.account {
                    proposal_guard(seq, account, status, entity, masterdata)?;
                }
                ceiling = ceiling.least(status);
            }
            other => {
                ceiling = DecisionStatus::Manual;
                let message = match (case.subject.as_deref(), other) {
                    (None, _) => format!("{} has no subject", case.source.key_prefix()),
                    (Some(s), None) => format!(
                        "no default rule case for {} subject {s:?}",
                        case.source.key_prefix()
                    ),
                    (Some(s), Some(r)) => format!(
                        "default rule case {} for {} subject {s:?} has no account",
                        r.case_id,
                        case.source.key_prefix()
                    ),
                };
                findings.push(finding(
                    FindingCode::UnmappedCategory,
                    Severity::Blocking,
                    message,
                    other.and_then(|r| r.question.clone()),
                ));
            }
        }

        // 2. Receipt counter row (income keying is deferred beyond Slice 1).
        let mut counter_rule = None;
        if let SourceFacts::Receipt { payment_method, .. } = &case.facts {
            match select_counter_row(case, payment_method.as_deref(), entity, masterdata)? {
                Ok(row) => {
                    counter_rule = Some(CounterRuleRef {
                        source: CaseSource::Receipt,
                        key: row.key.clone(),
                        bookkeeping_method: row.bookkeeping_method.clone(),
                    });
                    let status = automation(seq, &row.automation)?;
                    match &row.account {
                        Some(account) => {
                            proposal_guard(seq, account, status, entity, masterdata)?;
                        }
                        None => findings.push(finding(
                            FindingCode::UnresolvedCounterAccount,
                            Severity::Blocking,
                            format!(
                                "counter rule for payment_method {:?} has no account",
                                row.key
                            ),
                            row.question.clone(),
                        )),
                    }
                    ceiling = ceiling.least(status);
                }
                Err(f) => findings.push(f),
            }
        }

        // 3. Legacy triggers.
        match &case.facts {
            SourceFacts::Receipt { entry_type, .. } if entry_type == "income" => {
                findings.push(finding(
                    FindingCode::LegacyIncomeReceipt,
                    Severity::Blocking,
                    "receipt row carries legacy entry_type income".to_string(),
                    None,
                ));
            }
            SourceFacts::Income { income_type, .. } if income_type == "timber_sale" => {
                findings.push(finding(
                    FindingCode::UnspecifiedTimberSale,
                    Severity::Blocking,
                    "income_type timber_sale is unspecified (legacy)".to_string(),
                    None,
                ));
            }
            _ => {}
        }

        // 4. Missing evidence — a warning, never a status or line effect by itself.
        match &case.facts {
            SourceFacts::Receipt {
                has_image: false, ..
            } => findings.push(finding(
                FindingCode::MissingEvidence,
                Severity::Warning,
                "receipt has no image".to_string(),
                None,
            )),
            SourceFacts::Income { document_count, .. } if *document_count == 0 => {
                findings.push(finding(
                    FindingCode::MissingEvidence,
                    Severity::Warning,
                    "income entry has no documents".to_string(),
                    None,
                ))
            }
            _ => {}
        }

        // 5. Deterministic order: sort by code rank, then dense finding_no.
        findings.sort_by_key(|f| f.code);
        for (i, f) in findings.iter_mut().enumerate() {
            f.finding_no = i as i32;
        }

        // 6. Ceiling: draft cap, then Blocking cap. Never raised.
        if draft {
            ceiling = ceiling.least(DecisionStatus::Conditional);
        }
        if findings.iter().any(|f| f.severity == Severity::Blocking) {
            ceiling = DecisionStatus::Manual;
        }

        out.push(CaseAssessment {
            case_seq: seq,
            rule_case_id,
            vat_rule_id,
            counter_rule,
            status_ceiling: ceiling,
            findings,
        });
    }

    Ok(out)
}
