//! SV-04 Slice 1 — SIE 4I writer (serialization only).
//!
//! Turns an already-verified schema-3 engine run into SIE 4I bytes: one
//! `#VER` per line-bearing non-Manual decision, one `#TRANS` per persisted
//! line, `#KONTO` for exactly the accounts those lines use, names from the
//! chart whose identity the persisted run provenance names. Nothing is
//! decided here: no account is chosen or changed, no VAT is inferred, no
//! balance is repaired, no date is reinterpreted, no Manual case is promoted,
//! and the snapshot JSON is never read again.
//!
//! Locked format (SIE 4B, SV-04 Slice 1 lock 2026-09-10): `#FLAGGA 0` first;
//! `#PROGRAM "<name>" "<version>"`; `#FORMAT PC8`; `#GEN <YYYYMMDD>` from an
//! explicit `generated_on` (never a clock); `#SIETYP 4`; `#FNAMN "<explicit
//! company name>"`; optional `#ORGNR`; no `#RAR`, no `#KPTYP`, no `#KSUMMA`;
//! `#VER "" "" <date> "<subject>"` (importer-assigned series/number); amounts
//! as öre with exactly two decimals, debit positive and credit negative;
//! every text field quoted, `"` escaped as `\"`, control characters and
//! literal backslashes refused; strict CP437 (never lossy); CRLF everywhere.

use std::collections::BTreeSet;
use std::fmt;

use chrono::NaiveDate;
use codepage_437::{FromCp437, ToCp437, CP437_CONTROL};

use crate::ruleset::Masterdata;
use crate::workspace::{
    PersistedCase, PersistedLine, RunMeta, Workspace, WorkspaceError, WORKSPACE_SCHEMA_DECISIONS,
};

/// `#PROGRAM` identity — explicit writer input, never read from the runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProgramId {
    pub name: String,
    pub version: String,
}

/// Serialization-only metadata. Nothing here carries accounting state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExportMetadata {
    /// `#FNAMN` — explicit export identity (Slice 1: `"Testgården"`). The
    /// workspace's `display_name` is deliberately not used as a legal name.
    pub company_name: String,
    /// `#ORGNR` — emitted only when `Some`; must already be `NNNNNN-NNNN`.
    pub org_number: Option<String>,
    /// `#GEN` — the file generation date, supplied by the caller.
    pub generated_on: NaiveDate,
    pub program: ProgramId,
}

/// The produced file plus evidence about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SieExport {
    pub bytes: Vec<u8>,
    pub vouchers: usize,
    pub accounts: usize,
    /// SHA-256 of `bytes`, computed by DuckDB — evidence only, not in the file.
    pub sha256: String,
}

#[derive(Debug)]
pub enum SieWriteError {
    SchemaNotExportable(i32),
    NoEngineRun,
    RunNotVerified(WorkspaceError),
    CorruptEvidence(String),
    ChartProvenanceMismatch {
        field: &'static str,
        persisted: String,
        loaded: String,
    },
    AccountNotInChart(String),
    InvalidAccountData(String),
    VoucherNotBalanced {
        case_seq: i32,
        sum: i64,
    },
    MoneyOverflow {
        case_seq: i32,
    },
    ForbiddenControlChar {
        field: String,
        index: usize,
    },
    ForbiddenBackslash {
        field: String,
    },
    UnencodableText {
        field: String,
        ch: char,
    },
    InvalidExportMetadata(String),
    IneligibleDecision {
        case_seq: i32,
        reason: String,
    },
    Io(String),
}

impl fmt::Display for SieWriteError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::SchemaNotExportable(schema) => write!(
                f,
                "workspace schema {schema} holds no accounting decisions; nothing to export"
            ),
            Self::NoEngineRun => write!(f, "workspace holds no engine run; nothing to export"),
            Self::RunNotVerified(e) => write!(f, "engine run did not verify: {e}"),
            Self::CorruptEvidence(m) => write!(f, "persisted evidence is inconsistent: {m}"),
            Self::ChartProvenanceMismatch {
                field,
                persisted,
                loaded,
            } => write!(
                f,
                "masterdata does not match the run's provenance: {field} persisted {persisted:?}, loaded {loaded:?}"
            ),
            Self::AccountNotInChart(n) => write!(f, "account {n} is not in the run's chart"),
            Self::InvalidAccountData(n) => write!(f, "account {n} has no usable name"),
            Self::VoucherNotBalanced { case_seq, sum } => {
                write!(f, "case {case_seq}: voucher does not balance (signed sum {sum} öre)")
            }
            Self::MoneyOverflow { case_seq } => {
                write!(f, "case {case_seq}: amount arithmetic overflows i64")
            }
            Self::ForbiddenControlChar { field, index } => {
                write!(f, "{field}: control character at index {index}")
            }
            Self::ForbiddenBackslash { field } => write!(f, "{field}: literal backslash is not representable"),
            Self::UnencodableText { field, ch } => {
                write!(f, "{field}: {ch:?} cannot be encoded as CP437")
            }
            Self::InvalidExportMetadata(m) => write!(f, "invalid export metadata: {m}"),
            Self::IneligibleDecision { case_seq, reason } => {
                write!(f, "case {case_seq}: {reason}")
            }
            Self::Io(m) => write!(f, "io: {m}"),
        }
    }
}

impl std::error::Error for SieWriteError {}

impl From<WorkspaceError> for SieWriteError {
    fn from(e: WorkspaceError) -> Self {
        Self::RunNotVerified(e)
    }
}

// ---------------------------------------------------------------------------
// Text rules
// ---------------------------------------------------------------------------

/// Validate a text field against the locked rules and return it quoted with
/// `"` escaped as `\"`. Control characters and literal backslashes are refused.
fn quoted(field: &str, text: &str) -> Result<String, SieWriteError> {
    for (index, ch) in text.char_indices() {
        // The locked rule: ASCII 0–31 and 127 are refused. (C1 controls are
        // not representable in CP437 and fail at encoding instead.)
        if (ch as u32) < 0x20 || ch as u32 == 0x7F {
            return Err(SieWriteError::ForbiddenControlChar {
                field: field.to_string(),
                index,
            });
        }
        if ch == '\\' {
            return Err(SieWriteError::ForbiddenBackslash {
                field: field.to_string(),
            });
        }
    }
    let mut out = String::with_capacity(text.len() + 2);
    out.push('"');
    for ch in text.chars() {
        if ch == '"' {
            out.push('\\');
        }
        out.push(ch);
    }
    out.push('"');
    Ok(out)
}

fn encode_line(field: &str, line: &str) -> Result<Vec<u8>, SieWriteError> {
    let bytes = match line.to_cp437(&CP437_CONTROL) {
        Ok(bytes) => bytes.into_owned(),
        Err(e) => {
            let ch = line
                .chars()
                .nth(e.representable_up_to)
                .unwrap_or('\u{FFFD}');

            return Err(SieWriteError::UnencodableText {
                field: field.to_string(),
                ch,
            });
        }
    };

    // `codepage-437` accepts some Unicode aliases for canonical CP437
    // code points. Slice 1 is fail-closed: encoded bytes must decode back
    // to exactly the original text, otherwise the conversion is lossy.
    let decoded = String::from_cp437(bytes.clone(), &CP437_CONTROL);

    if decoded != line {
        let ch = line
            .chars()
            .zip(decoded.chars())
            .find_map(|(original, roundtrip)| (original != roundtrip).then_some(original))
            .or_else(|| line.chars().nth(decoded.chars().count()))
            .unwrap_or('\u{FFFD}');

        return Err(SieWriteError::UnencodableText {
            field: field.to_string(),
            ch,
        });
    }

    Ok(bytes)
}

/// Öre → SIE amount text: optional leading minus, integer kronor, `.`, exactly
/// two öre digits. Never a plus sign, never a float.
fn sie_amount(signed_ore: i64) -> String {
    let abs = signed_ore.unsigned_abs();
    let sign = if signed_ore < 0 { "-" } else { "" };
    format!("{sign}{}.{:02}", abs / 100, abs % 100)
}

fn sie_date(d: NaiveDate) -> String {
    d.format("%Y%m%d").to_string()
}

fn is_org_number_shape(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 11
        && b[..6].iter().all(u8::is_ascii_digit)
        && b[6] == b'-'
        && b[7..].iter().all(u8::is_ascii_digit)
}

// ---------------------------------------------------------------------------
// Provenance and eligibility
// ---------------------------------------------------------------------------

fn check_provenance(run: &RunMeta, masterdata: &Masterdata) -> Result<(), SieWriteError> {
    let p = &run.provenance;
    let pairs: [(&'static str, &str, &str); 7] = [
        ("chart_id", &p.chart_id, &masterdata.chart.chart_id),
        ("chart_version", &p.chart_version, &masterdata.chart.version),
        (
            "workbook_sha256",
            &p.workbook_sha256,
            &masterdata.chart.header.workbook_sha256,
        ),
        (
            "generator_version",
            &p.generator_version,
            &masterdata.chart.header.generator_version,
        ),
        (
            "ruleset_version",
            &p.ruleset_version,
            &masterdata.ruleset.ruleset_version,
        ),
        (
            "ruleset_status",
            &p.ruleset_status,
            &masterdata.ruleset.review_status,
        ),
        (
            "taxonomy_version",
            &p.taxonomy_version,
            &masterdata.ruleset.taxonomy_version,
        ),
    ];
    for (field, persisted, loaded) in pairs {
        if persisted != loaded {
            return Err(SieWriteError::ChartProvenanceMismatch {
                field,
                persisted: persisted.to_string(),
                loaded: loaded.to_string(),
            });
        }
    }
    Ok(())
}

/// One exportable voucher: the persisted case and its lines in `line_no` order.
struct Voucher<'a> {
    case: &'a PersistedCase,
    lines: Vec<&'a PersistedLine>,
}

fn eligible_vouchers<'a>(
    cases: &'a [PersistedCase],
    lines: &'a [PersistedLine],
) -> Result<Vec<Voucher<'a>>, SieWriteError> {
    let mut out = Vec::new();
    for case in cases {
        let mine: Vec<&PersistedLine> = lines
            .iter()
            .filter(|l| l.case_seq == case.case_seq)
            .collect();
        let manual = case.status == "Manual";
        match (manual, mine.is_empty()) {
            (true, true) => {}
            (true, false) => {
                return Err(SieWriteError::IneligibleDecision {
                    case_seq: case.case_seq,
                    reason: "Manual decision carries lines".to_string(),
                })
            }
            (false, true) => {
                if case.status == "Automatic" {
                    return Err(SieWriteError::IneligibleDecision {
                        case_seq: case.case_seq,
                        reason: "Automatic decision without lines".to_string(),
                    });
                }
                // Conditional without lines: structurally assessed only — not a voucher.
            }
            (false, false) => out.push(Voucher { case, lines: mine }),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Writer
// ---------------------------------------------------------------------------

/// Serialize the verified schema-3 run of `workspace` as SIE 4I.
///
/// Order of authority: workspace schema must be 3 → `verify_run()` must pass
/// → persisted provenance must equal the loaded masterdata → cases and lines
/// are read back → eligible vouchers are built → every text/amount is
/// validated → bytes are produced. Any failure returns an error and no bytes.
pub fn write_sie4i(
    workspace: &Workspace,
    masterdata: &Masterdata,
    meta: &ExportMetadata,
) -> Result<SieExport, SieWriteError> {
    // 1. Schema gate.
    let schema = workspace.schema_version()?;
    if schema == 1 {
        return Err(SieWriteError::NoEngineRun);
    }
    if schema != WORKSPACE_SCHEMA_DECISIONS {
        return Err(SieWriteError::SchemaNotExportable(schema));
    }
    // 2. The authoritative verifier.
    let run = workspace.verify_run()?;
    // 3. Provenance before any account name is read.
    check_provenance(&run, masterdata)?;
    // 4. Export metadata.
    if meta.company_name.trim().is_empty() {
        return Err(SieWriteError::InvalidExportMetadata(
            "company_name is empty".to_string(),
        ));
    }
    if meta.program.name.trim().is_empty() || meta.program.version.trim().is_empty() {
        return Err(SieWriteError::InvalidExportMetadata(
            "program name and version are required".to_string(),
        ));
    }
    if let Some(org) = &meta.org_number {
        if !is_org_number_shape(org) {
            return Err(SieWriteError::InvalidExportMetadata(format!(
                "org_number {org:?} is not NNNNNN-NNNN"
            )));
        }
    }
    // 5. Readback and eligibility.
    let cases = workspace.read_cases()?;
    let lines = workspace.read_decision_lines()?;
    let vouchers = eligible_vouchers(&cases, &lines)?;

    // 6. Used accounts, ascending, with names proven from the exact chart.
    let used: BTreeSet<&str> = vouchers
        .iter()
        .flat_map(|v| v.lines.iter().map(|l| l.account.as_str()))
        .collect();
    let mut konto_lines: Vec<(String, String)> = Vec::with_capacity(used.len());
    for number in &used {
        let account = masterdata
            .chart
            .account(number)
            .ok_or_else(|| SieWriteError::AccountNotInChart((*number).to_string()))?;
        if account.name.trim().is_empty() {
            return Err(SieWriteError::InvalidAccountData((*number).to_string()));
        }
        konto_lines.push((
            (*number).to_string(),
            quoted(&format!("#KONTO {number}"), &account.name)?,
        ));
    }

    // 7. Build text lines (validated), then encode strictly.
    let mut text_lines: Vec<(String, String)> = Vec::new(); // (field label, line)
    text_lines.push(("#FLAGGA".into(), "#FLAGGA 0".into()));
    text_lines.push((
        "#PROGRAM".into(),
        format!(
            "#PROGRAM {} {}",
            quoted("#PROGRAM name", &meta.program.name)?,
            quoted("#PROGRAM version", &meta.program.version)?
        ),
    ));
    text_lines.push(("#FORMAT".into(), "#FORMAT PC8".into()));
    text_lines.push((
        "#GEN".into(),
        format!("#GEN {}", sie_date(meta.generated_on)),
    ));
    text_lines.push(("#SIETYP".into(), "#SIETYP 4".into()));
    text_lines.push((
        "#FNAMN".into(),
        format!("#FNAMN {}", quoted("#FNAMN", &meta.company_name)?),
    ));
    if let Some(org) = &meta.org_number {
        text_lines.push(("#ORGNR".into(), format!("#ORGNR {org}")));
    }
    for (number, name) in &konto_lines {
        text_lines.push((
            format!("#KONTO {number}"),
            format!("#KONTO {number} {name}"),
        ));
    }
    for v in &vouchers {
        let seq = v.case.case_seq;
        let mut sum: i64 = 0;
        let mut trans: Vec<String> = Vec::with_capacity(v.lines.len());
        for l in &v.lines {
            let signed = if l.debit.0 > 0 {
                l.debit.0
            } else {
                l.credit
                    .0
                    .checked_neg()
                    .ok_or(SieWriteError::MoneyOverflow { case_seq: seq })?
            };
            sum = sum
                .checked_add(signed)
                .ok_or(SieWriteError::MoneyOverflow { case_seq: seq })?;
            trans.push(format!("#TRANS {} {{}} {}", l.account, sie_amount(signed)));
        }
        if sum != 0 {
            return Err(SieWriteError::VoucherNotBalanced { case_seq: seq, sum });
        }
        let field = format!("#VER case {seq}");
        let mut ver = format!("#VER \"\" \"\" {}", sie_date(v.case.date));
        if let Some(subject) = &v.case.subject {
            ver.push(' ');
            ver.push_str(&quoted(&field, subject)?);
        }
        text_lines.push((field.clone(), ver));
        text_lines.push((field.clone(), "{".into()));
        for t in trans {
            text_lines.push((field.clone(), t));
        }
        text_lines.push((field, "}".into()));
    }

    let mut bytes = Vec::new();
    for (field, line) in &text_lines {
        bytes.extend_from_slice(&encode_line(field, line)?);
        bytes.extend_from_slice(b"\r\n");
    }
    let sha256 = workspace.sha256_hex(&bytes)?;
    Ok(SieExport {
        bytes,
        vouchers: vouchers.len(),
        accounts: konto_lines.len(),
        sha256,
    })
}
