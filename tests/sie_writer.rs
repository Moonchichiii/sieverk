//! sie_writer (SV-04 Slice 1 + SK-SIE-BRIDGE-04) — real Testgården snapshot
//! 1.2, real DuckDB workspace, real synthetic masterdata, real SV-03 engine
//! run, real `VerifiedExportContext`, real SIE 4I bytes. No mocks.
//! The golden is the vNext canonical file (CASE B, SK-SIE-BRIDGE-04
//! Addendum 4; hash locked from the real writer bytes): the historical
//! 441-byte Slice-1 file (3fc89f8b…0660) plus `#ORGNR` and `#RAR 0` in the
//! Capego RUN-02 order, and every `#VER` with a blank series and a
//! writer-assigned number `1..N` in emission order. Two earlier vectors stay
//! evidence and are proven as exact reductions of the golden: the pre-D3
//! BRIDGE-04 file (487 bytes, b977601f…dd0b, blank series and number) and the
//! historical Slice-1 file.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use sieverk::engine::run_engine;
use sieverk::export_context::VerifiedExportContext;
use sieverk::money::Ore;
use sieverk::ruleset::{load_masterdata, Masterdata};
use sieverk::sie_writer::{write_sie4i, ExportMetadata, ProgramId, SieExport, SieWriteError};
use sieverk::validator::{validate, Code};
use sieverk::workspace::{Workspace, WorkspaceError, WORKSPACE_SCHEMA_DECISIONS};
use sieverk::{accounts, decode_sie_bytes, metadata, vouchers};

const TESTGARDEN: &str = "fixtures/snapshots/testgarden-2026-1.2.json";
const SYNTHETIC_ROOT: &str = "fixtures/masterdata/synthetic/generated";
const HISTORICAL_SHA256: &str = "3fc89f8b504f883d0a4444e11e5389c25b99f201b0bfdaf88a65c74a23e20660";
const HISTORICAL_BYTES: usize = 441;
const VNEXT_SHA256: &str = "3c225ff8332d2942d6b9115c19ab8fec0b47de53f0a0ef8fad404b63259bdba0";
const VNEXT_BYTES: usize = 489;
/// BRIDGE-04 pre-D3 compatibility vector (blank series and number): evidence.
const PRE_D3_SHA256: &str = "b977601f45b5a5495dead24849393f19626fa17d0d9c6b7f50d64d33e418dd0b";
const PRE_D3_BYTES: usize = 487;
const VER_NUM_1: &str = "#VER \"\" \"1\" ";
const VER_NUM_2: &str = "#VER \"\" \"2\" ";
const VER_BLANK: &str = "#VER \"\" \"\" ";
const ORGNR_LINE: &str = "#ORGNR 900101-9802\r\n";
const RAR_LINE: &str = "#RAR 0 20260101 20261231\r\n";

/// The locked reference text (CRLF, CP437) — decoded form for text comparison.
/// The historical Slice-1 file (evidence; never regenerated).
const HISTORICAL_TEXT: &str = "#FLAGGA 0\r\n\
#PROGRAM \"SIEverk\" \"0.4.0\"\r\n\
#FORMAT PC8\r\n\
#GEN 20260910\r\n\
#SIETYP 4\r\n\
#FNAMN \"Testgården\"\r\n\
#KONTO 1930 \"Bank\"\r\n\
#KONTO 2640 \"Ingående moms\"\r\n\
#KONTO 4470 \"Omkostnader skogen\"\r\n\
#KONTO 5360 \"Drivmedel och oljor\"\r\n\
#VER \"\" \"\" 20260820 \"Bränsle\"\r\n\
{\r\n\
#TRANS 5360 {} 5000.00\r\n\
#TRANS 2640 {} 1250.00\r\n\
#TRANS 1930 {} -6250.00\r\n\
}\r\n\
#VER \"\" \"\" 20260602 \"Skogsvård\"\r\n\
{\r\n\
#TRANS 4470 {} 25000.00\r\n\
#TRANS 2640 {} 6250.00\r\n\
#TRANS 1930 {} -31250.00\r\n\
}\r\n";

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

struct TempDb(PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join("sieverk-sie-writer-tests");
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{}-{name}.duckdb", std::process::id()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("duckdb.wal"));
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(self.0.with_extension("duckdb.wal"));
    }
}

fn masterdata() -> Masterdata {
    load_masterdata(&repo(SYNTHETIC_ROOT)).expect("synthetic masterdata")
}

/// The vNext canonical text: the historical file with `#ORGNR` before `#FNAMN`,
/// `#RAR 0` after it (DDR-ORDER, Capego RUN-02) and every voucher in series
/// `A` with a blank number (CASE A).
fn vnext_text() -> String {
    HISTORICAL_TEXT
        .replacen("#FNAMN", &format!("{ORGNR_LINE}#FNAMN"), 1)
        .replacen("#KONTO 1930", &format!("{RAR_LINE}#KONTO 1930"), 1)
        .replacen(VER_BLANK, VER_NUM_1, 1)
        .replacen(VER_BLANK, VER_NUM_2, 1)
}

/// Replace exactly one occurrence of `from` with `to` in raw bytes.
fn replace_once(bytes: &mut Vec<u8>, from: &[u8], to: &[u8]) {
    let at = bytes
        .windows(from.len())
        .position(|w| w == from)
        .expect("needle present");
    bytes.splice(at..at + from.len(), to.iter().copied());
}

fn snapshot_json() -> serde_json::Value {
    serde_json::from_slice(&fs::read(repo(TESTGARDEN)).expect("fixture")).expect("json")
}

fn snapshot_with(f: impl FnOnce(&mut serde_json::Value)) -> Vec<u8> {
    let mut v = snapshot_json();
    f(&mut v);
    serde_json::to_vec(&v).expect("serialise")
}

/// The canonical path: snapshot -> create -> ingest_snapshot -> run_engine -> bind.
fn run_raw(db: &TempDb, raw: &[u8], md: &Masterdata) -> (Workspace, VerifiedExportContext) {
    let mut ws = Workspace::create(db.path()).expect("create");
    let ingested = ws.ingest_snapshot(raw).expect("ingest_snapshot");
    run_engine(&mut ws, md).expect("run_engine");
    let ctx = VerifiedExportContext::bind(&ingested, &ws).expect("bind");
    (ws, ctx)
}

fn run_workspace(db: &TempDb, md: &Masterdata) -> (Workspace, VerifiedExportContext) {
    run_raw(db, &fs::read(repo(TESTGARDEN)).expect("fixture"), md)
}

fn meta() -> ExportMetadata {
    ExportMetadata {
        generated_on: NaiveDate::from_ymd_opt(2026, 9, 10).expect("date"),
        program: ProgramId {
            name: "SIEverk".to_string(),
            version: "0.4.0".to_string(),
        },
    }
}

fn export(ws: &Workspace, md: &Masterdata, ctx: &VerifiedExportContext) -> SieExport {
    write_sie4i(ws, md, ctx, &meta()).expect("export")
}

fn sql(ws: &Workspace, statement: &str) {
    ws.connection().execute_batch(statement).expect(statement);
}

// W1 — exact golden bytes, CRLF everywhere, evidence counts
#[test]
fn w1_testgarden_export_matches_the_vnext_canonical_exactly() {
    let db = TempDb::new("w1");
    let md = masterdata();
    let (ws, ctx) = run_workspace(&db, &md);
    let out = export(&ws, &md, &ctx);
    let text = decode_sie_bytes(out.bytes.clone());
    // If these fail: STOP and diff — never adjust content to match the hash.
    assert_eq!(
        text,
        vnext_text(),
        "decoded text differs from the vNext canonical"
    );
    assert_eq!(out.bytes.len(), VNEXT_BYTES);
    assert_eq!(out.sha256, VNEXT_SHA256);
    assert_eq!(out.vouchers, 2);
    assert_eq!(out.accounts, 4);
    // CRLF on every line including the last; no bare LF anywhere.
    assert!(out.bytes.ends_with(b"\r\n"));
    for (i, b) in out.bytes.iter().enumerate() {
        if *b == b'\n' {
            assert!(i > 0 && out.bytes[i - 1] == b'\r', "bare LF at byte {i}");
        }
    }
    // DDR-ORDER (Capego RUN-02), line by line.
    let lines: Vec<&str> = text.split("\r\n").collect();
    assert_eq!(
        lines[..9].to_vec(),
        vec![
            "#FLAGGA 0",
            "#PROGRAM \"SIEverk\" \"0.4.0\"",
            "#FORMAT PC8",
            "#GEN 20260910",
            "#SIETYP 4",
            "#ORGNR 900101-9802",
            "#FNAMN \"Testgården\"",
            "#RAR 0 20260101 20261231",
            "#KONTO 1930 \"Bank\"",
        ]
    );
    for absent in ["#KPTYP", "#KSUMMA", "#IB", "#UB", "#RES", "receipt:", "+"] {
        assert!(!text.contains(absent), "{absent} must not appear");
    }
    // Swedish characters as CP437 bytes: å=0x86 ä=0x84.
    let byte_pos = out
        .bytes
        .windows(5)
        .position(|w| w == b"Testg")
        .expect("fnamn bytes");
    assert_eq!(out.bytes[byte_pos + 5], 0x86, "å");
    assert!(out.bytes.contains(&0x84), "ä present");
    assert!(!out.bytes.contains(&b'?'));
}

// W1b — the golden reduces exactly to both earlier vectors (unchanged
// accounting content; this transformation does not authorise blank numbers)
#[test]
fn w1b_vnext_reduces_to_the_pre_d3_and_historical_files() {
    let db = TempDb::new("w1b");
    let md = masterdata();
    let (ws, ctx) = run_workspace(&db, &md);
    let out = export(&ws, &md, &ctx);
    let text = decode_sie_bytes(out.bytes.clone());
    assert_eq!(text.matches(ORGNR_LINE).count(), 1);
    assert_eq!(text.matches(RAR_LINE).count(), 1);
    assert_eq!(text.matches(VER_NUM_1).count(), 1);
    assert_eq!(text.matches(VER_NUM_2).count(), 1);
    assert_eq!(text.matches(VER_BLANK).count(), 0);
    // 1. Numbers 1 and 2 -> blank gives exactly the pre-D3 BRIDGE-04 vector.
    let mut bytes = out.bytes.clone();
    for numbered in [VER_NUM_1, VER_NUM_2] {
        replace_once(&mut bytes, numbered.as_bytes(), VER_BLANK.as_bytes());
    }
    assert_eq!(bytes.len(), PRE_D3_BYTES);
    assert_eq!(ws.sha256_hex(&bytes).expect("sha"), PRE_D3_SHA256);
    // 2. Then without #ORGNR and #RAR 0 it is exactly the historical file.
    for line in [ORGNR_LINE, RAR_LINE] {
        replace_once(&mut bytes, line.as_bytes(), b"");
    }
    assert_eq!(bytes.len(), HISTORICAL_BYTES);
    assert_eq!(ws.sha256_hex(&bytes).expect("sha"), HISTORICAL_SHA256);
    assert_eq!(decode_sie_bytes(bytes), HISTORICAL_TEXT);
}

// W2 — deterministic across independent workspaces
#[test]
fn w2_export_is_deterministic() {
    let a = TempDb::new("w2a");
    let b = TempDb::new("w2b");
    let md = masterdata();
    let (wa, ca) = run_workspace(&a, &md);
    let (wb, cb) = run_workspace(&b, &md);
    let ea = export(&wa, &md, &ca);
    let eb = export(&wb, &md, &cb);
    assert_eq!(ea, eb);
    assert_eq!(
        export(&wa, &md, &ca),
        ea,
        "repeated export of the same workspace"
    );
}

// W3 — parser round-trip and validator verdict
#[test]
fn w3_roundtrip_through_existing_parser_and_validator() {
    let db = TempDb::new("w3");
    let md = masterdata();
    let (ws, ctx) = run_workspace(&db, &md);
    let out = export(&ws, &md, &ctx);
    let text = decode_sie_bytes(out.bytes.clone());
    let m = metadata::parse_metadata(&text);
    assert_eq!(m.sie_type, Some(4));
    assert_eq!(m.company_name.as_deref(), Some("Testgården"));
    assert_eq!(m.org_number.as_deref(), Some("900101-9802"));
    assert_eq!(m.format.as_deref(), Some("PC8"));
    assert!(m.program.as_deref().is_some_and(|p| p.contains("SIEverk")));
    assert_eq!(m.generated_at.as_deref(), Some("20260910"));
    assert_eq!(m.fiscal_years.len(), 1, "exactly #RAR 0");
    assert_eq!(
        (
            m.fiscal_years[0].index,
            m.fiscal_years[0].start.as_str(),
            m.fiscal_years[0].end.as_str()
        ),
        (0, "20260101", "20261231")
    );
    let a = accounts::parse_accounts(&text);
    assert_eq!(
        a.accounts
            .iter()
            .map(|x| (x.number.as_str(), x.name.as_deref()))
            .collect::<Vec<_>>(),
        vec![
            ("1930", Some("Bank")),
            ("2640", Some("Ingående moms")),
            ("4470", Some("Omkostnader skogen")),
            ("5360", Some("Drivmedel och oljor")),
        ]
    );
    assert!(a.balances.is_empty(), "no #IB/#UB/#RES in a 4I");
    let v = vouchers::parse_vouchers(&text);
    assert_eq!(v.vouchers.len(), 2);
    let first = &v.vouchers[0];
    assert_eq!(
        (
            first.series.as_str(),
            first.number.as_str(),
            first.date.as_str()
        ),
        ("", "1", "20260820")
    );
    assert_eq!(first.text.as_deref(), Some("Bränsle"));
    assert_eq!(
        first
            .rows
            .iter()
            .map(|r| (r.account.as_str(), r.amount))
            .collect::<Vec<_>>(),
        vec![
            ("5360", Ore(500_000)),
            ("2640", Ore(125_000)),
            ("1930", Ore(-625_000))
        ]
    );
    let second = &v.vouchers[1];
    assert_eq!(
        (
            second.series.as_str(),
            second.number.as_str(),
            second.date.as_str()
        ),
        ("", "2", "20260602")
    );
    assert_eq!(second.text.as_deref(), Some("Skogsvård"));
    assert_eq!(
        second
            .rows
            .iter()
            .map(|r| (r.account.as_str(), r.amount))
            .collect::<Vec<_>>(),
        vec![
            ("4470", Ore(2_500_000)),
            ("2640", Ore(625_000)),
            ("1930", Ore(-3_125_000))
        ]
    );
    for vo in &v.vouchers {
        let sum: Ore = vo.rows.iter().map(|r| r.amount).sum();
        assert_eq!(sum, Ore(0));
    }
    // Validator: zero errors and zero warnings. The validator keys duplicates
    // on (series, number); CASE B numbers ("", "1") and ("", "2") are unique,
    // so DUPLICATE_VOUCHER_NUMBER (expected only for blank numbers) is gone.
    let report = validate(&m, &a, &v);
    assert_eq!(report.error_count(), 0, "{:?}", report.findings);
    assert_eq!(report.warning_count(), 0, "{:?}", report.findings);
    assert!(!report
        .findings
        .iter()
        .any(|f| f.code == Code::DuplicateVoucherNumber));
}

// W4 — Manual and zero-line decisions are never vouchers; only used accounts declared
#[test]
fn w4_only_line_bearing_non_manual_decisions_and_used_accounts() {
    let db = TempDb::new("w4");
    let md = masterdata();
    let (ws, ctx) = run_workspace(&db, &md);
    let cases = ws.read_cases().expect("cases");
    let lines = ws.read_decision_lines().expect("lines");
    assert_eq!(cases.len(), 27);
    let line_bearing: Vec<i32> = cases
        .iter()
        .filter(|c| c.status != "Manual" && lines.iter().any(|l| l.case_seq == c.case_seq))
        .map(|c| c.case_seq)
        .collect();
    assert_eq!(
        line_bearing.len(),
        2,
        "exactly the two proven decisions carry lines"
    );
    assert!(cases
        .iter()
        .filter(|c| c.status == "Manual")
        .all(|c| !lines.iter().any(|l| l.case_seq == c.case_seq)));
    let out = export(&ws, &md, &ctx);
    let text = decode_sie_bytes(out.bytes);
    assert_eq!(text.matches("#VER ").count(), 2);
    assert_eq!(text.matches("#KONTO ").count(), 4);
    // Accounts used by no exported line are absent even though the chart declares them.
    assert!(md.chart.account("2013").is_some());
    assert!(!text.contains("#KONTO 2013"));
    // Grusgropen (rounding / supplier credit) and every Manual case are absent.
    assert!(!text.contains("Grus och material"));
    assert!(!text.contains("Annat"));
}

// W4b — numbering consumes ordinals only for exported vouchers: a Manual case
// before the first export and a zero-line Conditional case between two exports
// get no number; no gaps, no duplicates, case_seq emission order preserved.
#[test]
fn w4b_numbering_skips_manual_and_zero_line_cases() {
    let md = masterdata();
    let db = TempDb::new("w4b");
    // Move receipt 18 ("Grus och material", a zero-line Conditional decision)
    // to directly after receipt 1 (Bränsle). selector_position is the snapshot
    // order, so it becomes case_seq 2 — between the two exported vouchers.
    let raw = snapshot_with(|v| {
        let receipts = v["receipts"].as_array_mut().expect("receipts");
        let from = receipts
            .iter()
            .position(|r| r["id"] == 18)
            .expect("receipt 18");
        let grus = receipts.remove(from);
        let after = receipts
            .iter()
            .position(|r| r["id"] == 1)
            .expect("receipt 1");
        receipts.insert(after + 1, grus);
    });
    let (ws, ctx) = run_raw(&db, &raw, &md);
    let cases = ws.read_cases().expect("cases");
    let lines = ws.read_decision_lines().expect("lines");
    let line_count = |seq: i32| lines.iter().filter(|l| l.case_seq == seq).count();
    let shape: Vec<(i32, &str, &str, usize)> = cases
        .iter()
        .take(7)
        .map(|c| {
            (
                c.case_seq,
                c.subject.as_deref().unwrap_or(""),
                c.status.as_str(),
                line_count(c.case_seq),
            )
        })
        .collect();
    assert_eq!(
        shape[0].2, "Manual",
        "a Manual case precedes the first export"
    );
    assert_eq!(shape[0].3, 0);
    assert_eq!(shape[1], (1, "Bränsle", "Conditional", 3));
    assert_eq!(shape[2], (2, "Grus och material", "Conditional", 0));
    assert_eq!(shape[6], (6, "Skogsvård", "Conditional", 3));
    for s in &shape[3..6] {
        assert_eq!((s.2, s.3), ("Manual", 0), "{s:?}");
    }
    let out = export(&ws, &md, &ctx);
    let text = decode_sie_bytes(out.bytes.clone());
    let v = vouchers::parse_vouchers(&text);
    assert_eq!(
        v.vouchers
            .iter()
            .map(|x| (x.series.as_str(), x.number.as_str(), x.date.as_str()))
            .collect::<Vec<_>>(),
        vec![("", "1", "20260820"), ("", "2", "20260602")]
    );
    assert!(!text.contains("Grus och material"));
    // Same exported content, same numbers, same bytes as the canonical file.
    assert_eq!(text, vnext_text());
    assert_eq!(out.sha256, VNEXT_SHA256);
}

// W5 — schema 1 and schema 2 cannot export; corrupt schema 3 cannot export
#[test]
fn w5_schema_gates_and_verify_run_are_authoritative() {
    let md = masterdata();
    // A valid context from another, complete run: the schema gate comes first.
    let dbc = TempDb::new("w5-ctx");
    let (_wc, ctx) = run_workspace(&dbc, &md);
    // schema 1
    let db1 = TempDb::new("w5-1");
    let raw = fs::read(repo(TESTGARDEN)).expect("fixture");
    let mut ws1 = Workspace::create(db1.path()).expect("create");
    ws1.ingest(&raw).expect("ingest");
    assert!(matches!(
        write_sie4i(&ws1, &md, &ctx, &meta()),
        Err(SieWriteError::NoEngineRun)
    ));
    // schema 2 (reconstructed Slice-2 evidence file)
    let db2 = TempDb::new("w5-2");
    let (ws2, ctx2) = run_workspace(&db2, &md);
    sql(&ws2, "DELETE FROM decision_lines");
    let cases = ws2.read_cases().expect("cases");
    let findings = ws2.read_findings().expect("findings");
    let run = ws2.read_run_meta().expect("run");
    let d2 = ws2
        .decision_digest(&run.provenance, &cases, &findings, &[])
        .expect("digest");
    sql(
        &ws2,
        &format!("UPDATE engine_run_meta SET decision_sha256 = '{d2}'"),
    );
    sql(&ws2, "UPDATE workspace_meta SET workspace_schema = 2");
    ws2.close().expect("close");
    let ws2 = Workspace::open(db2.path()).expect("schema 2 opens");
    assert!(matches!(
        write_sie4i(&ws2, &md, &ctx2, &meta()),
        Err(SieWriteError::SchemaNotExportable(2))
    ));
    // corrupt schema 3: a mutated persisted amount fails verify_run
    let db3 = TempDb::new("w5-3");
    let (ws3, ctx3) = run_workspace(&db3, &md);
    sql(
        &ws3,
        "UPDATE decision_lines SET debit_ore = debit_ore + 1 WHERE case_seq = (SELECT min(case_seq) FROM decision_lines) AND line_no = 0",
    );
    let e = write_sie4i(&ws3, &md, &ctx3, &meta()).expect_err("corrupt");
    assert!(
        matches!(e, SieWriteError::RunNotVerified(WorkspaceError::Corrupt(_))),
        "{e}"
    );
    assert_eq!(
        ws3.schema_version().expect("schema"),
        WORKSPACE_SCHEMA_DECISIONS
    );
}

// W6 — chart provenance and account data are proven, never guessed
#[test]
fn w6_provenance_and_account_data_fail_closed() {
    let db = TempDb::new("w6");
    let md = masterdata();
    let (ws, ctx) = run_workspace(&db, &md);
    let mut wrong = masterdata();
    wrong.chart.version = "2027.1".to_string();
    let e = write_sie4i(&ws, &wrong, &ctx, &meta()).expect_err("provenance");
    assert!(
        matches!(
            &e,
            SieWriteError::ChartProvenanceMismatch {
                field: "chart_version",
                ..
            }
        ),
        "{e}"
    );
    let mut wrong = masterdata();
    wrong.ruleset.review_status = "approved".to_string();
    let e = write_sie4i(&ws, &wrong, &ctx, &meta()).expect_err("status");
    assert!(
        matches!(
            &e,
            SieWriteError::ChartProvenanceMismatch {
                field: "ruleset_status",
                ..
            }
        ),
        "{e}"
    );
    let mut missing = masterdata();
    missing.chart.accounts.retain(|a| a.number != "2640");
    let e = write_sie4i(&ws, &missing, &ctx, &meta()).expect_err("missing account");
    assert!(
        matches!(&e, SieWriteError::AccountNotInChart(n) if n == "2640"),
        "{e}"
    );
    let mut unnamed = masterdata();
    unnamed
        .chart
        .accounts
        .iter_mut()
        .find(|a| a.number == "1930")
        .expect("acc")
        .name = "  ".to_string();
    let e = write_sie4i(&ws, &unnamed, &ctx, &meta()).expect_err("empty name");
    assert!(
        matches!(&e, SieWriteError::InvalidAccountData(n) if n == "1930"),
        "{e}"
    );
}

// W7 — text rules on #FNAMN (from legal_name): strict CP437, escaping,
// backslash; control characters are already refused by the snapshot parser.
fn export_with_legal_name(name: &str, tag: &str) -> Result<SieExport, SieWriteError> {
    let md = masterdata();
    let db = TempDb::new(tag);
    let raw = snapshot_with(|v| v["entity"]["accounting_identity"]["legal_name"] = name.into());
    let (ws, ctx) = run_raw(&db, &raw, &md);
    write_sie4i(&ws, &md, &ctx, &meta())
}

#[test]
fn w7_text_rules_and_strict_cp437() {
    let e = export_with_legal_name("Testgården €", "w7-euro").expect_err("euro");
    assert!(
        matches!(&e, SieWriteError::UnencodableText { ch: '€', .. }),
        "{e}"
    );
    let e = export_with_legal_name("Testgården 😀", "w7-emoji").expect_err("emoji");
    assert!(
        matches!(&e, SieWriteError::UnencodableText { ch: '😀', .. }),
        "{e}"
    );
    let e = export_with_legal_name("C:\\SIE", "w7-bs").expect_err("backslash");
    assert!(matches!(e, SieWriteError::ForbiddenBackslash { .. }), "{e}");
    // Control characters never reach the writer: the 1.2 parser refuses them.
    let ctl = TempDb::new("w7-ctl");
    let mut ws = Workspace::create(ctl.path()).expect("create");
    let raw =
        snapshot_with(|v| v["entity"]["accounting_identity"]["legal_name"] = "Test\tgården".into());
    assert!(ws.ingest_snapshot(&raw).is_err());
    // Quote escaping and the full Swedish CP437 set survive the round trip.
    let ok = export_with_legal_name("Gården \"Åke\" ÅÄÖ åäö", "w7-ok").expect("quoted");
    let text = decode_sie_bytes(ok.bytes.clone());
    assert!(
        text.contains("#FNAMN \"Gården \\\"Åke\\\" ÅÄÖ åäö\"\r\n"),
        "{text}"
    );
    for (ch, byte) in [
        ('Å', 0x8Fu8),
        ('Ä', 0x8E),
        ('Ö', 0x99),
        ('å', 0x86),
        ('ä', 0x84),
        ('ö', 0x94),
    ] {
        assert!(ok.bytes.contains(&byte), "{ch} → {byte:#04x}");
    }
    assert!(!ok.bytes.contains(&b'?'));
    let m = metadata::parse_metadata(&text);
    assert_eq!(m.company_name.as_deref(), Some("Gården \"Åke\" ÅÄÖ åäö"));
    // program identity is explicit input
    let md = masterdata();
    let db = TempDb::new("w7-prog");
    let (ws, ctx) = run_workspace(&db, &md);
    let prog = ExportMetadata {
        program: ProgramId {
            name: "X".into(),
            version: " ".into(),
        },
        ..meta()
    };
    assert!(matches!(
        write_sie4i(&ws, &md, &ctx, &prog),
        Err(SieWriteError::InvalidExportMetadata(_))
    ));
}

// W10 — a context bound to another workspace is refused
#[test]
fn w10_context_from_another_workspace_is_refused() {
    let md = masterdata();
    let a = TempDb::new("w10-a");
    let b = TempDb::new("w10-b");
    let (_wa, ctx_a) = run_workspace(&a, &md);
    let other =
        snapshot_with(|v| v["entity"]["accounting_identity"]["legal_name"] = "Annan".into());
    let (wb, _ctx_b) = run_raw(&b, &other, &md);
    let e = write_sie4i(&wb, &md, &ctx_a, &meta()).expect_err("A context, B workspace");
    assert!(matches!(e, SieWriteError::ContextNotBound), "{e}");
}

// W11 — export support is sole traders only (K1 scope)
#[test]
fn w11_unsupported_entity_type_is_refused() {
    let md = masterdata();
    let db = TempDb::new("w11");
    let raw = snapshot_with(|v| {
        let id = &mut v["entity"]["accounting_identity"];
        id["entity_type"] = "limited_company".into();
        id["identifier_kind"] = "organisation_number".into();
        id["business_identifier"] = "999999-0006".into();
    });
    let (ws, ctx) = run_raw(&db, &raw, &md);
    let e = write_sie4i(&ws, &md, &ctx, &meta()).expect_err("AB");
    assert!(
        matches!(e, SieWriteError::UnsupportedEntityType("limited_company")),
        "{e}"
    );
    assert!(!e.to_string().contains("999999-0006"));
}

// W12 — a voucher outside the fiscal period is refused, never repaired
#[test]
fn w12_voucher_outside_fiscal_period_is_refused() {
    let md = masterdata();
    let db = TempDb::new("w12");
    // Shortened first year 2026-07-01..2026-12-31: Skogsvård (2026-06-02) is outside.
    let raw = snapshot_with(|v| {
        v["fiscal_period"] = serde_json::json!({"start": "2026-07-01", "end": "2026-12-31"})
    });
    let (ws, ctx) = run_raw(&db, &raw, &md);
    let e = write_sie4i(&ws, &md, &ctx, &meta()).expect_err("outside period");
    assert!(
        matches!(e, SieWriteError::VoucherOutsideFiscalPeriod { .. }),
        "{e}"
    );
}

// W8 — disk write/read preserves bytes; no snapshot JSON is reopened
#[test]
fn w8_disk_roundtrip_and_no_json_reopen() {
    let db = TempDb::new("w8");
    let md = masterdata();
    let copy = std::env::temp_dir().join(format!(
        "sieverk-sie-writer-{}-snapshot.json",
        std::process::id()
    ));
    fs::copy(repo(TESTGARDEN), &copy).expect("copy");
    let raw = fs::read(&copy).expect("read");
    let (ws, ctx) = run_raw(&db, &raw, &md);
    fs::remove_file(&copy).expect("delete the only JSON");
    let out = export(&ws, &md, &ctx);
    let path = db.path().with_extension("SI");
    fs::write(&path, &out.bytes).expect("write");
    let back = fs::read(&path).expect("read back");
    assert_eq!(back, out.bytes);
    assert_eq!(ws.sha256_hex(&back).expect("sha"), VNEXT_SHA256);
    let _ = fs::remove_file(&path);
}

// W9 — source scans: no floats, no clock, no lossy encoding, no account-controlled logic
#[test]
fn w9_writer_source_invariants() {
    let full = fs::read_to_string(repo("src/sie_writer.rs")).expect("sie_writer.rs");
    // Scan code, not comments: the module documentation may name what the
    // writer deliberately does NOT use.
    let src: String = full
        .lines()
        .filter(|l| !l.trim_start().starts_with("//"))
        .collect::<Vec<_>>()
        .join("\n");
    for forbidden in [
        "f32",
        "f64",
        "as f",
        "now()",
        "SystemTime",
        "Local::",
        "Utc::",
        "to_cp437_lossy",
        "display_name",
        "company_name",
        "org_number",
        "income_year",
        "sie_series",
        "read_entity",
        "unwrap()",
    ] {
        assert!(
            !src.contains(forbidden),
            "{forbidden} must not appear in the writer"
        );
    }
    for account in [
        "\"1930\"", "\"2640\"", "\"3740\"", "\"5360\"", "\"4470\"", "\"2440\"",
    ] {
        assert!(
            !src.contains(account),
            "{account} must not control writer behaviour"
        );
    }
    for accounting in [
        "vat_rule",
        "rates",
        "deductibility",
        "rounding",
        "payment_method",
    ] {
        assert!(
            !src.contains(accounting),
            "{accounting} is accounting logic, not serialization"
        );
    }
    assert!(src.contains("to_cp437(&CP437_CONTROL)"), "strict encoder");
    assert!(
        src.contains("checked_neg()") && src.contains("checked_add"),
        "checked money arithmetic"
    );
}

// W13 — the stored voucher-series setting is never read: any value, or none,
// gives byte-identical CASE B output (Addendum 4 §4)
fn export_with_series(series: &str, tag: &str) -> Result<SieExport, SieWriteError> {
    let md = masterdata();
    let db = TempDb::new(tag);
    let raw = snapshot_with(|v| v["entity"]["accounting_profile"]["sie_series"] = series.into());
    let (ws, ctx) = run_raw(&db, &raw, &md);
    write_sie4i(&ws, &md, &ctx, &meta())
}

#[test]
fn w13_voucher_series_setting_does_not_affect_the_export() {
    for (series, tag) in [
        ("A", "w13-a"),
        ("SK", "w13-sk"),
        ("B", "w13-b"),
        ("a", "w13-lower"),
        (" A", "w13-space"),
        ("", "w13-empty"),
    ] {
        let out = export_with_series(series, tag).expect(series);
        assert_eq!(out.sha256, VNEXT_SHA256, "{series:?}");
        assert_eq!(decode_sie_bytes(out.bytes), vnext_text(), "{series:?}");
    }
    // A missing series (NULL in the verified workspace) changes nothing either;
    // entity_context is outside the decision digest, so verify_run still passes.
    let md = masterdata();
    let db = TempDb::new("w13-null");
    let (ws, ctx) = run_workspace(&db, &md);
    sql(&ws, "UPDATE entity_context SET sie_series = NULL");
    let out = write_sie4i(&ws, &md, &ctx, &meta()).expect("NULL series exports");
    assert_eq!(out.sha256, VNEXT_SHA256);
}

#[test]
fn w13b_blank_series_and_contiguous_numbers_on_every_voucher() {
    let db = TempDb::new("w13b");
    let md = masterdata();
    let (ws, ctx) = run_workspace(&db, &md);
    let out = export(&ws, &md, &ctx);
    let text = decode_sie_bytes(out.bytes.clone());
    let v = vouchers::parse_vouchers(&text);
    assert_eq!(v.vouchers.len(), 2);
    assert_eq!(v.vouchers.iter().map(|x| x.rows.len()).sum::<usize>(), 6);
    for (i, vo) in v.vouchers.iter().enumerate() {
        let expected = (i + 1).to_string();
        assert_eq!(
            (vo.series.as_str(), vo.number.as_str()),
            ("", expected.as_str())
        );
    }
    assert_eq!(text.matches("#VER ").count(), 2);
    assert_eq!(text.matches("#TRANS ").count(), 6);
    assert_eq!(text.matches(VER_BLANK).count(), 0);
    assert_eq!(out.sha256, VNEXT_SHA256);
}
