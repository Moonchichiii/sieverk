//! sie_writer (SV-04 Slice 1) — real Testgården workspace file, real
//! synthetic masterdata, real SV-03 engine run, real SIE 4I bytes. No mocks.
//! The golden expectation is the locked 441-byte reference
//! (sha256 3fc89f8b…0660) generated under the Slice-1 lock decisions.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use sieverk::engine::run_engine;
use sieverk::money::Ore;
use sieverk::ruleset::{load_masterdata, Masterdata};
use sieverk::sie_writer::{write_sie4i, ExportMetadata, ProgramId, SieExport, SieWriteError};
use sieverk::validator::{validate, Code, Severity};
use sieverk::workspace::{Workspace, WorkspaceError, WORKSPACE_SCHEMA_DECISIONS};
use sieverk::{accounts, decode_sie_bytes, metadata, vouchers};

const TESTGARDEN: &str = "fixtures/snapshots/testgarden-2026-1.1.json";
const SYNTHETIC_ROOT: &str = "fixtures/masterdata/synthetic/generated";
const REFERENCE_SHA256: &str = "3fc89f8b504f883d0a4444e11e5389c25b99f201b0bfdaf88a65c74a23e20660";
const REFERENCE_BYTES: usize = 441;

/// The locked reference text (CRLF, CP437) — decoded form for text comparison.
const REFERENCE_TEXT: &str = "#FLAGGA 0\r\n\
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

/// The canonical path: snapshot → create → ingest → run_engine (schema 3).
fn run_workspace(db: &TempDb, md: &Masterdata) -> Workspace {
    let raw = fs::read(repo(TESTGARDEN)).expect("fixture");
    let mut ws = Workspace::create(db.path()).expect("create");
    ws.ingest(&raw).expect("ingest");
    run_engine(&mut ws, md).expect("run_engine");
    ws
}

fn meta() -> ExportMetadata {
    ExportMetadata {
        company_name: "Testgården".to_string(),
        org_number: None,
        generated_on: NaiveDate::from_ymd_opt(2026, 9, 10).expect("date"),
        program: ProgramId {
            name: "SIEverk".to_string(),
            version: "0.4.0".to_string(),
        },
    }
}

fn export(ws: &Workspace, md: &Masterdata) -> SieExport {
    write_sie4i(ws, md, &meta()).expect("export")
}

fn sql(ws: &Workspace, statement: &str) {
    ws.connection().execute_batch(statement).expect(statement);
}

// W1 — exact golden bytes, CRLF everywhere, evidence counts
#[test]
fn w1_testgarden_export_matches_the_locked_reference_exactly() {
    let db = TempDb::new("w1");
    let md = masterdata();
    let ws = run_workspace(&db, &md);
    let out = export(&ws, &md);
    assert_eq!(out.bytes.len(), REFERENCE_BYTES);
    assert_eq!(out.sha256, REFERENCE_SHA256);
    assert_eq!(out.vouchers, 2);
    assert_eq!(out.accounts, 4);
    assert_eq!(decode_sie_bytes(out.bytes.clone()), REFERENCE_TEXT);
    // CRLF on every line including the last; no bare LF anywhere.
    assert!(out.bytes.ends_with(b"\r\n"));
    let mut i = 0;
    while i < out.bytes.len() {
        if out.bytes[i] == b'\n' {
            assert!(i > 0 && out.bytes[i - 1] == b'\r', "bare LF at byte {i}");
        }
        i += 1;
    }
    assert!(out.bytes.starts_with(b"#FLAGGA 0\r\n"));
    // Locked omissions and presences.
    for absent in [
        "#RAR",
        "#KPTYP",
        "#KSUMMA",
        "#ORGNR",
        "receipt:",
        "+",
        "999999-0006",
    ] {
        assert!(!REFERENCE_TEXT.contains(absent), "{absent} must not appear");
    }
    // Swedish characters as CP437 bytes: å=0x86 ä=0x84 ö=0x94 (in "Testgården", "Ingående", "Bränsle", "Skogsvård").
    let text_pos = REFERENCE_TEXT.find("Testg").expect("fnamn");
    let byte_pos = out
        .bytes
        .windows(5)
        .position(|w| w == b"Testg")
        .expect("fnamn bytes");
    assert_eq!(out.bytes[byte_pos + 5], 0x86, "å");
    assert!(REFERENCE_TEXT[text_pos + 5..].starts_with("år"));
    assert!(out.bytes.contains(&0x84), "ä present");
    assert!(!out.bytes.contains(&b'?'));
}

// W2 — deterministic across independent workspaces
#[test]
fn w2_export_is_deterministic() {
    let a = TempDb::new("w2a");
    let b = TempDb::new("w2b");
    let md = masterdata();
    let wa = run_workspace(&a, &md);
    let wb = run_workspace(&b, &md);
    let ea = export(&wa, &md);
    let eb = export(&wb, &md);
    assert_eq!(ea, eb);
    assert_eq!(
        export(&wa, &md),
        ea,
        "repeated export of the same workspace"
    );
}

// W3 — parser round-trip and validator verdict
#[test]
fn w3_roundtrip_through_existing_parser_and_validator() {
    let db = TempDb::new("w3");
    let md = masterdata();
    let ws = run_workspace(&db, &md);
    let out = export(&ws, &md);
    let text = decode_sie_bytes(out.bytes.clone());
    let m = metadata::parse_metadata(&text);
    assert_eq!(m.sie_type, Some(4));
    assert_eq!(m.company_name.as_deref(), Some("Testgården"));
    assert_eq!(m.org_number, None);
    assert_eq!(m.format.as_deref(), Some("PC8"));
    assert!(m.program.as_deref().is_some_and(|p| p.contains("SIEverk")));
    assert_eq!(m.generated_at.as_deref(), Some("20260910"));
    assert!(m.fiscal_years.is_empty(), "no #RAR");
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
        ("", "", "20260820")
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
        ("", "", "20260602")
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
    // Validator: zero errors; exactly one warning — the existing validator keys
    // duplicates on (series, number) and two importer-assigned ("", "") vouchers
    // therefore trigger DUPLICATE_VOUCHER_NUMBER. Locked as an internal
    // validator expectation (U-P2-1); never silenced by inventing numbers.
    let report = validate(&m, &a, &v);
    assert_eq!(report.error_count(), 0, "{:?}", report.findings);
    assert_eq!(report.warning_count(), 1, "{:?}", report.findings);
    let w = report
        .findings
        .iter()
        .find(|f| f.severity == Severity::Warning)
        .expect("warning");
    assert_eq!(w.code, Code::DuplicateVoucherNumber);
}

// W4 — Manual and zero-line decisions are never vouchers; only used accounts declared
#[test]
fn w4_only_line_bearing_non_manual_decisions_and_used_accounts() {
    let db = TempDb::new("w4");
    let md = masterdata();
    let ws = run_workspace(&db, &md);
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
    let out = export(&ws, &md);
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

// W5 — schema 1 and schema 2 cannot export; corrupt schema 3 cannot export
#[test]
fn w5_schema_gates_and_verify_run_are_authoritative() {
    let md = masterdata();
    // schema 1
    let db1 = TempDb::new("w5-1");
    let raw = fs::read(repo(TESTGARDEN)).expect("fixture");
    let mut ws1 = Workspace::create(db1.path()).expect("create");
    ws1.ingest(&raw).expect("ingest");
    assert!(matches!(
        write_sie4i(&ws1, &md, &meta()),
        Err(SieWriteError::NoEngineRun)
    ));
    // schema 2 (reconstructed Slice-2 evidence file)
    let db2 = TempDb::new("w5-2");
    let ws2 = run_workspace(&db2, &md);
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
        write_sie4i(&ws2, &md, &meta()),
        Err(SieWriteError::SchemaNotExportable(2))
    ));
    // corrupt schema 3: a mutated persisted amount fails verify_run
    let db3 = TempDb::new("w5-3");
    let ws3 = run_workspace(&db3, &md);
    sql(
        &ws3,
        "UPDATE decision_lines SET debit_ore = debit_ore + 1 WHERE case_seq = 1 AND line_no = 0",
    );
    let e = write_sie4i(&ws3, &md, &meta()).expect_err("corrupt");
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
    let ws = run_workspace(&db, &md);
    let mut wrong = masterdata();
    wrong.chart.version = "2027.1".to_string();
    let e = write_sie4i(&ws, &wrong, &meta()).expect_err("provenance");
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
    let e = write_sie4i(&ws, &wrong, &meta()).expect_err("status");
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
    let e = write_sie4i(&ws, &missing, &meta()).expect_err("missing account");
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
    let e = write_sie4i(&ws, &unnamed, &meta()).expect_err("empty name");
    assert!(
        matches!(&e, SieWriteError::InvalidAccountData(n) if n == "1930"),
        "{e}"
    );
}

// W7 — text rules: control chars, backslash, quote escaping, CP437 strictness
#[test]
fn w7_text_rules_and_strict_cp437() {
    let db = TempDb::new("w7");
    let md = masterdata();
    let ws = run_workspace(&db, &md);
    let with = |name: &str| ExportMetadata {
        company_name: name.to_string(),
        ..meta()
    };
    let e = write_sie4i(&ws, &md, &with("Test\tgården")).expect_err("tab");
    assert!(
        matches!(e, SieWriteError::ForbiddenControlChar { .. }),
        "{e}"
    );
    let e = write_sie4i(&ws, &md, &with("Test\u{7f}gården")).expect_err("del");
    assert!(
        matches!(e, SieWriteError::ForbiddenControlChar { .. }),
        "{e}"
    );
    let e = write_sie4i(&ws, &md, &with("C:\\SIE")).expect_err("backslash");
    assert!(matches!(e, SieWriteError::ForbiddenBackslash { .. }), "{e}");
    let e = write_sie4i(&ws, &md, &with("Testgården €")).expect_err("euro");
    assert!(
        matches!(&e, SieWriteError::UnencodableText { ch: '€', .. }),
        "{e}"
    );
    let e = write_sie4i(&ws, &md, &with("Testgården 😀")).expect_err("emoji");
    assert!(
        matches!(&e, SieWriteError::UnencodableText { ch: '😀', .. }),
        "{e}"
    );
    let e = write_sie4i(&ws, &md, &with("   ")).expect_err("empty");
    assert!(matches!(e, SieWriteError::InvalidExportMetadata(_)), "{e}");
    // Quote escaping and the full Swedish CP437 set survive the round trip.
    let ok = write_sie4i(&ws, &md, &with("Gården \"Åke\" ÅÄÖ åäö")).expect("quoted");
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
    // org number: shape enforced, emitted verbatim when supplied
    let bad = ExportMetadata {
        org_number: Some("9999990006".to_string()),
        ..meta()
    };
    assert!(matches!(
        write_sie4i(&ws, &md, &bad),
        Err(SieWriteError::InvalidExportMetadata(_))
    ));
    let with_org = ExportMetadata {
        org_number: Some("999999-0006".to_string()),
        ..meta()
    };
    let out = write_sie4i(&ws, &md, &with_org).expect("org");
    let text = decode_sie_bytes(out.bytes);
    assert!(
        text.contains("#FNAMN \"Testgården\"\r\n#ORGNR 999999-0006\r\n#KONTO 1930"),
        "{text}"
    );
    assert_ne!(
        out.sha256, REFERENCE_SHA256,
        "the canonical file has no #ORGNR"
    );
    // program identity is explicit input
    let prog = ExportMetadata {
        program: ProgramId {
            name: "X".into(),
            version: " ".into(),
        },
        ..meta()
    };
    assert!(matches!(
        write_sie4i(&ws, &md, &prog),
        Err(SieWriteError::InvalidExportMetadata(_))
    ));
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
    let mut ws = Workspace::create(db.path()).expect("create");
    ws.ingest(&raw).expect("ingest");
    run_engine(&mut ws, &md).expect("run");
    fs::remove_file(&copy).expect("delete the only JSON");
    let out = export(&ws, &md);
    let path = db.path().with_extension("SI");
    fs::write(&path, &out.bytes).expect("write");
    let back = fs::read(&path).expect("read back");
    assert_eq!(back, out.bytes);
    assert_eq!(ws.sha256_hex(&back).expect("sha"), REFERENCE_SHA256);
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
        "sie_series",
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
