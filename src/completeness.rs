//! SIE-COMPLETE-01 — fail-closed completeness gate for one export.
//!
//! Every `Receipt` and `IncomeEntry` in the snapshot is accounted for exactly
//! once: either as the exported `#VER` with its writer-assigned number, or as
//! an explicit exclusion with its reason (`engine::Exclusion`) and finding
//! codes. Nothing is decided here: the report re-derives the engine's
//! decisions in memory from the same workspace and masterdata, refuses any
//! divergence from the persisted, verified run, and refuses any case that
//! would be neither a voucher nor an explicit exclusion. No account, rule or
//! amount is chosen, changed or repaired.
//!
//! The manifest (`sieverk-completeness/1`) carries technical keys, codes,
//! statuses and öre amounts only — never a name, an identifier, a vendor, a
//! description or any other free text from the snapshot.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::Serialize;

use crate::engine::{
    assess_cases, decide_cases, project_cases, CaseSource, DecisionStatus, EngineError, Exclusion,
    FindingCode, SourceFacts,
};
use crate::money::Ore;
use crate::ruleset::Masterdata;
use crate::sie_writer::SieExport;
use crate::snapshot::Snapshot;
use crate::workspace::{Workspace, WorkspaceError, WORKSPACE_SCHEMA_DECISIONS};

/// Manifest format identifier.
pub const MANIFEST_FORMAT: &str = "sieverk-completeness/1";

/// SIE-COMPLETE-01 / SIE-OUT-01: how much of the snapshot the SIE file holds.
/// `Full` means every snapshot row is an exported voucher — nothing more: it
/// is not a statement that the bookkeeping is correct, approved or complete
/// against the source database. Only `Full` may ever be offered as a
/// complete submission; `Partial` and `Empty` never.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coverage {
    /// The snapshot has no receipts and no income entries.
    Empty,
    /// At least one row is an explicit exclusion.
    Partial,
    /// Every row is an exported voucher.
    Full,
}

impl Coverage {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Empty => "empty",
            Self::Partial => "partial",
            Self::Full => "full",
        }
    }
}

/// How one source row ends up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaseOutcome {
    /// Exported as `#VER "" "<ver_no>"` in the SIE file.
    Voucher { ver_no: usize },
    /// Not in the SIE file, for exactly this reason.
    Excluded { reason: Exclusion },
}

/// One snapshot row and its outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaseAccount {
    pub case_seq: i32,
    pub source: CaseSource,
    pub source_key: String,
    pub status: DecisionStatus,
    /// Gross amount: a receipt's total, an income entry's amount incl. VAT.
    pub amount: Ore,
    /// Every finding code of the case (Blocking and Warning), in rank order.
    pub finding_codes: Vec<FindingCode>,
    pub outcome: CaseOutcome,
}

/// Counts and gross amounts for one source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SourceTotals {
    pub cases: usize,
    pub vouchers: usize,
    pub excluded: usize,
    pub amount: Ore,
    pub exported_amount: Ore,
    pub excluded_amount: Ore,
}

/// The full accounting of one export.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompletenessReport {
    pub snapshot_sha256: String,
    pub decision_sha256: String,
    pub engine_version: String,
    pub chart_id: String,
    pub ruleset_version: String,
    pub ruleset_status: String,
    pub si_sha256: String,
    pub si_bytes: usize,
    pub cases: Vec<CaseAccount>,
    pub receipts: SourceTotals,
    pub income: SourceTotals,
}

impl CompletenessReport {
    pub fn vouchers(&self) -> usize {
        self.receipts.vouchers + self.income.vouchers
    }

    pub fn excluded(&self) -> usize {
        self.receipts.excluded + self.income.excluded
    }

    /// Cases that are neither a voucher nor an explicit exclusion. A report
    /// only exists when this is zero; it is exposed as evidence.
    pub fn silent(&self) -> usize {
        self.cases.len() - self.vouchers() - self.excluded()
    }

    /// Fail-closed coverage: `Full` only when nothing is excluded or silent.
    pub fn coverage(&self) -> Coverage {
        if self.cases.is_empty() {
            Coverage::Empty
        } else if self.excluded() == 0 && self.silent() == 0 {
            Coverage::Full
        } else {
            Coverage::Partial
        }
    }

    /// Exclusion counts in the fixed `Exclusion::ALL` order (zeros included).
    pub fn excluded_by_reason(&self) -> Vec<(Exclusion, usize)> {
        Exclusion::ALL
            .iter()
            .map(|r| {
                let n = self
                    .cases
                    .iter()
                    .filter(|c| c.outcome == CaseOutcome::Excluded { reason: *r })
                    .count();
                (*r, n)
            })
            .collect()
    }

    /// Deterministic manifest bytes (UTF-8 JSON, LF, trailing newline).
    pub fn manifest_json(&self) -> Result<Vec<u8>, CompletenessError> {
        let manifest = Manifest {
            format: MANIFEST_FORMAT,
            snapshot_sha256: &self.snapshot_sha256,
            decision_sha256: &self.decision_sha256,
            engine_version: &self.engine_version,
            chart_id: &self.chart_id,
            ruleset_version: &self.ruleset_version,
            ruleset_status: &self.ruleset_status,
            si_sha256: &self.si_sha256,
            si_bytes: self.si_bytes,
            summary: ManifestSummary {
                coverage: self.coverage().as_str(),
                cases: self.cases.len(),
                vouchers: self.vouchers(),
                excluded: self.excluded(),
                silent: self.silent(),
                receipts: ManifestTotals::from(&self.receipts),
                income_entries: ManifestTotals::from(&self.income),
                excluded_by_reason: self
                    .excluded_by_reason()
                    .into_iter()
                    .map(|(r, n)| (r.as_str(), n))
                    .collect(),
            },
            cases: self
                .cases
                .iter()
                .map(|c| {
                    let (outcome, ver_no, reason) = match c.outcome {
                        CaseOutcome::Voucher { ver_no } => ("voucher", Some(ver_no), None),
                        CaseOutcome::Excluded { reason } => {
                            ("excluded", None, Some(reason.as_str()))
                        }
                    };
                    ManifestCase {
                        case_seq: c.case_seq,
                        source: c.source.key_prefix(),
                        source_key: &c.source_key,
                        status: c.status.as_str(),
                        amount_ore: c.amount.0,
                        outcome,
                        ver_no,
                        reason,
                        finding_codes: c.finding_codes.iter().map(|f| f.as_str()).collect(),
                    }
                })
                .collect(),
        };
        let mut bytes = serde_json::to_vec_pretty(&manifest)
            .map_err(|e| CompletenessError::Manifest(e.to_string()))?;
        bytes.push(b'\n');
        Ok(bytes)
    }
}

#[derive(Serialize)]
struct Manifest<'a> {
    format: &'static str,
    snapshot_sha256: &'a str,
    decision_sha256: &'a str,
    engine_version: &'a str,
    chart_id: &'a str,
    ruleset_version: &'a str,
    ruleset_status: &'a str,
    si_sha256: &'a str,
    si_bytes: usize,
    summary: ManifestSummary,
    cases: Vec<ManifestCase<'a>>,
}

#[derive(Serialize)]
struct ManifestSummary {
    coverage: &'static str,
    cases: usize,
    vouchers: usize,
    excluded: usize,
    silent: usize,
    receipts: ManifestTotals,
    income_entries: ManifestTotals,
    excluded_by_reason: BTreeMap<&'static str, usize>,
}

#[derive(Serialize)]
struct ManifestTotals {
    cases: usize,
    vouchers: usize,
    excluded: usize,
    amount_ore: i64,
    exported_amount_ore: i64,
    excluded_amount_ore: i64,
}

impl From<&SourceTotals> for ManifestTotals {
    fn from(t: &SourceTotals) -> Self {
        Self {
            cases: t.cases,
            vouchers: t.vouchers,
            excluded: t.excluded,
            amount_ore: t.amount.0,
            exported_amount_ore: t.exported_amount.0,
            excluded_amount_ore: t.excluded_amount.0,
        }
    }
}

#[derive(Serialize)]
struct ManifestCase<'a> {
    case_seq: i32,
    source: &'static str,
    source_key: &'a str,
    status: &'static str,
    amount_ore: i64,
    outcome: &'static str,
    ver_no: Option<usize>,
    reason: Option<&'static str>,
    finding_codes: Vec<&'static str>,
}

#[derive(Debug)]
pub enum CompletenessError {
    Workspace(WorkspaceError),
    Engine(EngineError),
    /// Only a schema-3 decision run can be accounted for.
    SchemaNotDecisions(i32),
    /// The snapshot is not the one the verified run was built from.
    SnapshotMismatch,
    /// The snapshot itself lists one source row twice.
    DuplicateSourceRow {
        source_key: String,
    },
    /// A snapshot row has no persisted case.
    MissingCase {
        source_key: String,
    },
    /// A persisted case has no snapshot row.
    UnexpectedCase {
        source_key: String,
    },
    /// One source row is persisted more than once.
    DuplicateCase {
        source_key: String,
    },
    /// The in-memory re-derivation differs from the persisted run.
    RunMismatch {
        case_seq: i32,
        field: Field,
    },
    /// Case or source counts differ between snapshot, re-derivation and run.
    CountMismatch,
    /// Per-source totals do not split exactly into exported + excluded.
    TotalsDoNotReconcile,
    /// A case is neither a voucher nor an explicit exclusion.
    SilentExclusion {
        case_seq: i32,
    },
    /// The SIE file's vouchers are not exactly the line-bearing cases.
    VoucherMismatch {
        detail: String,
    },
    MoneyOverflow,
    Manifest(String),
}

/// Which part of a case differs from the persisted run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    CaseSeq,
    SourceKey,
    Status,
    Lines,
    Findings,
}

impl Field {
    fn as_str(self) -> &'static str {
        match self {
            Self::CaseSeq => "case_seq",
            Self::SourceKey => "source_key",
            Self::Status => "status",
            Self::Lines => "lines",
            Self::Findings => "findings",
        }
    }
}

impl fmt::Display for CompletenessError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Workspace(e) => write!(f, "workspace: {e}"),
            Self::Engine(e) => write!(f, "engine: {e}"),
            Self::SchemaNotDecisions(schema) => write!(
                f,
                "completeness: workspace schema {schema} holds no accounting decisions"
            ),
            Self::SnapshotMismatch => write!(
                f,
                "completeness: the snapshot is not the one the verified run was built from"
            ),
            Self::DuplicateSourceRow { source_key } => {
                write!(
                    f,
                    "completeness: snapshot lists {source_key} more than once"
                )
            }
            Self::MissingCase { source_key } => {
                write!(f, "completeness: {source_key} has no accounting case")
            }
            Self::UnexpectedCase { source_key } => write!(
                f,
                "completeness: case {source_key} is not a row of the snapshot"
            ),
            Self::DuplicateCase { source_key } => {
                write!(
                    f,
                    "completeness: {source_key} is accounted for more than once"
                )
            }
            Self::RunMismatch { case_seq, field } => write!(
                f,
                "completeness: case {case_seq}: {} differs from the persisted run",
                field.as_str()
            ),
            Self::CountMismatch => write!(
                f,
                "completeness: case counts differ between snapshot and persisted run"
            ),
            Self::TotalsDoNotReconcile => write!(
                f,
                "completeness: totals do not split into exported and excluded"
            ),
            Self::SilentExclusion { case_seq } => write!(
                f,
                "completeness: case {case_seq} is neither a voucher nor an explicit exclusion"
            ),
            Self::VoucherMismatch { detail } => write!(f, "completeness: {detail}"),
            Self::MoneyOverflow => write!(f, "completeness: amount sum overflows i64"),
            Self::Manifest(m) => write!(f, "completeness: manifest: {m}"),
        }
    }
}

impl std::error::Error for CompletenessError {}

impl From<WorkspaceError> for CompletenessError {
    fn from(e: WorkspaceError) -> Self {
        Self::Workspace(e)
    }
}

impl From<EngineError> for CompletenessError {
    fn from(e: EngineError) -> Self {
        Self::Engine(e)
    }
}

fn add(a: Ore, b: Ore) -> Result<Ore, CompletenessError> {
    a.0.checked_add(b.0)
        .map(Ore)
        .ok_or(CompletenessError::MoneyOverflow)
}

/// Account for every snapshot row of one export, fail-closed.
///
/// `snapshot` and `snapshot_sha256` are the ingested snapshot of this
/// workspace; `export` is the writer's result for its verified run.
pub fn completeness_report(
    workspace: &Workspace,
    masterdata: &Masterdata,
    snapshot: &Snapshot,
    snapshot_sha256: &str,
    export: &SieExport,
) -> Result<CompletenessReport, CompletenessError> {
    // 1. Only a verified schema-3 run of exactly this snapshot.
    let schema = workspace.schema_version()?;
    if schema != WORKSPACE_SCHEMA_DECISIONS {
        return Err(CompletenessError::SchemaNotDecisions(schema));
    }
    let run = workspace.verify_run()?;
    if run.provenance.snapshot_sha256 != snapshot_sha256
        || workspace.snapshot_sha256()? != snapshot_sha256
    {
        return Err(CompletenessError::SnapshotMismatch);
    }

    // 2. The expected population: every snapshot row, exactly once.
    let mut expected: BTreeSet<String> = BTreeSet::new();
    let keys = snapshot
        .receipts
        .iter()
        .map(|r| format!("{}:{}", CaseSource::Receipt.key_prefix(), r.id))
        .chain(
            snapshot
                .income_entries
                .iter()
                .map(|i| format!("{}:{}", CaseSource::Income.key_prefix(), i.id)),
        );
    for key in keys {
        if !expected.insert(key.clone()) {
            return Err(CompletenessError::DuplicateSourceRow { source_key: key });
        }
    }

    // 3. The persisted population: one case per row, nothing extra.
    let persisted = workspace.read_cases()?;
    let findings = workspace.read_findings()?;
    let lines = workspace.read_decision_lines()?;
    let mut seen: BTreeSet<&str> = BTreeSet::new();
    for c in &persisted {
        if !seen.insert(c.source_key.as_str()) {
            return Err(CompletenessError::DuplicateCase {
                source_key: c.source_key.clone(),
            });
        }
        if !expected.contains(&c.source_key) {
            return Err(CompletenessError::UnexpectedCase {
                source_key: c.source_key.clone(),
            });
        }
    }
    if let Some(missing) = expected.iter().find(|k| !seen.contains(k.as_str())) {
        return Err(CompletenessError::MissingCase {
            source_key: missing.clone(),
        });
    }

    // 4. Re-derive the decisions in memory and require the persisted run.
    let entity = workspace.read_entity()?;
    let cases = project_cases(workspace)?;
    let assessments = assess_cases(&cases, &entity, masterdata)?;
    let decisions = decide_cases(&cases, &assessments, &entity, masterdata)?;
    if cases.len() != persisted.len() || decisions.len() != persisted.len() {
        return Err(CompletenessError::CountMismatch);
    }

    let mut out = Vec::with_capacity(cases.len());
    let mut voucher_seqs: Vec<i32> = Vec::new();
    let mut receipts = SourceTotals::default();
    let mut income = SourceTotals::default();
    for ((case, decision), p) in cases.iter().zip(&decisions).zip(&persisted) {
        let seq = case.case_seq;
        let mismatch = |field| CompletenessError::RunMismatch {
            case_seq: seq,
            field,
        };
        if decision.case_seq != seq || p.case_seq != seq {
            return Err(mismatch(Field::CaseSeq));
        }
        if p.source_key != case.source_key || p.source != case.source.key_prefix() {
            return Err(mismatch(Field::SourceKey));
        }
        if p.status != decision.status.as_str() {
            return Err(mismatch(Field::Status));
        }
        let mine: Vec<_> = lines.iter().filter(|l| l.case_seq == seq).collect();
        let same_lines = mine.len() == decision.lines.len()
            && mine.iter().zip(&decision.lines).all(|(l, d)| {
                l.line_no == d.line_no
                    && l.account == d.account
                    && l.role == d.role.as_str()
                    && l.debit == d.debit
                    && l.credit == d.credit
            });
        if !same_lines {
            return Err(mismatch(Field::Lines));
        }
        let theirs: Vec<_> = findings.iter().filter(|f| f.case_seq == seq).collect();
        let same_findings = theirs.len() == decision.findings.len()
            && theirs.iter().zip(&decision.findings).all(|(p, d)| {
                p.finding_no == d.finding_no
                    && p.code == d.code.as_str()
                    && p.severity == d.severity.as_str()
            });
        if !same_findings {
            return Err(mismatch(Field::Findings));
        }

        let outcome = match (decision.lines.is_empty(), decision.exclusion) {
            (false, None) if decision.status != DecisionStatus::Manual => {
                voucher_seqs.push(seq);
                CaseOutcome::Voucher {
                    ver_no: voucher_seqs.len(),
                }
            }
            (true, Some(reason)) if decision.status == DecisionStatus::Manual => {
                CaseOutcome::Excluded { reason }
            }
            _ => return Err(CompletenessError::SilentExclusion { case_seq: seq }),
        };
        let amount = match &case.facts {
            SourceFacts::Receipt { total, .. } => *total,
            SourceFacts::Income { inc_vat, .. } => *inc_vat,
        };
        let totals = match case.source {
            CaseSource::Receipt => &mut receipts,
            CaseSource::Income => &mut income,
        };
        totals.cases += 1;
        totals.amount = add(totals.amount, amount)?;
        match outcome {
            CaseOutcome::Voucher { .. } => {
                totals.vouchers += 1;
                totals.exported_amount = add(totals.exported_amount, amount)?;
            }
            CaseOutcome::Excluded { .. } => {
                totals.excluded += 1;
                totals.excluded_amount = add(totals.excluded_amount, amount)?;
            }
        }
        out.push(CaseAccount {
            case_seq: seq,
            source: case.source,
            source_key: case.source_key.clone(),
            status: decision.status,
            amount,
            finding_codes: decision.findings.iter().map(|f| f.code).collect(),
            outcome,
        });
    }

    // 5. The SIE file holds exactly the line-bearing cases, numbered 1..N.
    if export.voucher_case_seqs != voucher_seqs || export.vouchers != voucher_seqs.len() {
        return Err(CompletenessError::VoucherMismatch {
            detail: format!(
                "the SIE file has {} vouchers but {} cases carry lines",
                export.vouchers,
                voucher_seqs.len()
            ),
        });
    }
    if export.sha256 != workspace.sha256_hex(&export.bytes)? {
        return Err(CompletenessError::VoucherMismatch {
            detail: "the SIE digest does not match its bytes".to_string(),
        });
    }

    // 6. Totals reconcile: every row counted once, amounts split exactly.
    for t in [&receipts, &income] {
        if t.vouchers + t.excluded != t.cases
            || add(t.exported_amount, t.excluded_amount)? != t.amount
        {
            return Err(CompletenessError::TotalsDoNotReconcile);
        }
    }
    if receipts.cases != snapshot.receipts.len() || income.cases != snapshot.income_entries.len() {
        return Err(CompletenessError::CountMismatch);
    }

    Ok(CompletenessReport {
        snapshot_sha256: snapshot_sha256.to_string(),
        decision_sha256: run.decision_sha256,
        engine_version: run.provenance.engine_version,
        chart_id: run.provenance.chart_id,
        ruleset_version: run.provenance.ruleset_version,
        ruleset_status: run.provenance.ruleset_status,
        si_sha256: export.sha256.clone(),
        si_bytes: export.bytes.len(),
        cases: out,
        receipts,
        income,
    })
}
