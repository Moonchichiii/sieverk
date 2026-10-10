//! SK-SIE-BRIDGE-03 — snapshot schema 1.2, the ingest envelope and
//! `VerifiedExportContext`. Real Django-generated fixture (byte-exact BRIDGE-02
//! evidence), real DuckDB workspace, real synthetic masterdata, real engine,
//! real binary for the CLI checks. No mocks.
//!
//! Test identities: `900101-9802` is the SIE wire form of Skatteverket's
//! official test personnummer 189001019802; `200161-2395` of the test
//! samordningsnummer 202001612395; `999999-0006` is a synthetic repository
//! organisation-number constant (not a Skatteverket number).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Instant;

use chrono::NaiveDate;
use serde_json::Value;
use sieverk::engine::run_engine;
use sieverk::export_context::{ExportContextError, VerifiedExportContext};
use sieverk::ruleset::{load_masterdata, Masterdata};
use sieverk::sie_writer::{write_sie4i, ExportMetadata, ProgramId};
use sieverk::snapshot::{parse_snapshot, EntityType, IdentifierKind};
use sieverk::workspace::{IngestOutcome, Workspace};

const V12: &str = "fixtures/snapshots/testgarden-2026-1.2.json";
const V11: &str = "fixtures/snapshots/testgarden-2026-1.1.json";
const SYNTHETIC_ROOT: &str = "fixtures/masterdata/synthetic/generated";
const V12_FILE_SHA256: &str = "03a6385f97dcc899eef7798f27037adba97ba7b3125b8cd7b9bc086a79bf1158";
const V12_DIGEST: &str = "a857edd5381bfe9204dad4192875923a13ee7ec9a4aadaf1fc53f2a3135be88f";
const HISTORICAL_SI_SHA256: &str =
    "3fc89f8b504f883d0a4444e11e5389c25b99f201b0bfdaf88a65c74a23e20660";
const WIRE_PERSON: &str = "900101-9802";
const WIRE_DIGITS: &str = "9001019802";
/// Deliberately unique and distinct from `entity.display_name`, so a hit in
/// DuckDB can only come from the accounting legal name.
const LEGAL_NAME_TOKEN: &str = "LegalNameToken-ÅÄÖ-4711";

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn read(path: &str) -> Vec<u8> {
    fs::read(repo(path)).expect("fixture")
}

struct TempDb(PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join("sieverk-snapshot-v1-2-tests");
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

/// The 1.2 fixture as JSON, edited by `f`, re-serialised.
fn v12_with(f: impl FnOnce(&mut Value)) -> Vec<u8> {
    let mut value: Value = serde_json::from_slice(&read(V12)).expect("json");
    f(&mut value);
    serde_json::to_vec(&value).expect("serialise")
}

fn identity_mut(v: &mut Value) -> &mut Value {
    &mut v["entity"]["accounting_identity"]
}

/// create -> ingest_snapshot -> run_engine.
fn engine_run(
    db: &TempDb,
    raw: &[u8],
    md: &Masterdata,
) -> (Workspace, sieverk::workspace::IngestedSnapshot) {
    let mut ws = Workspace::create(db.path()).expect("create");
    let ingested = ws.ingest_snapshot(raw).expect("ingest_snapshot");
    run_engine(&mut ws, md).expect("run_engine");
    (ws, ingested)
}

fn parse_error(raw: &[u8]) -> String {
    parse_snapshot(raw)
        .expect_err("must be refused")
        .to_string()
}

// ---------------------------------------------------------------------------
// Fixture and digest
// ---------------------------------------------------------------------------

#[test]
fn fixture_is_the_byte_exact_django_evidence_and_digest_matches() {
    let raw = read(V12);
    let db = TempDb::new("digest");
    let ws = Workspace::create(db.path()).expect("create");
    assert_eq!(ws.sha256_hex(&raw).expect("sha"), V12_FILE_SHA256);
    assert_eq!(ws.snapshot_digest(&raw).expect("digest"), V12_DIGEST);
    let golden: Value =
        serde_json::from_slice(&read("fixtures/snapshots/django-digests.json")).expect("json");
    assert_eq!(golden["digests"]["testgarden-2026-1.2.json"], V12_DIGEST);
}

#[test]
fn real_django_1_2_parses_typed() {
    let snap = parse_snapshot(&read(V12)).expect("1.2 parses");
    assert_eq!(snap.schema_version, "1.2");
    assert_eq!(snap.properties.len(), 2);
    assert_eq!(snap.receipts.len(), 21);
    assert_eq!(snap.income_entries.len(), 6);
    assert_eq!(snap.entity.org_number, None);
    let identity = snap.entity.accounting_identity.as_ref().expect("identity");
    assert_eq!(identity.entity_type, EntityType::SoleTrader);
    assert_eq!(identity.identifier_kind, IdentifierKind::PersonalNumber);
    assert_eq!(identity.legal_name, "Testgården");
    assert_eq!(identity.business_identifier, WIRE_PERSON);
    let period = snap.fiscal_period.expect("period");
    assert_eq!(
        period.start,
        NaiveDate::from_ymd_opt(2026, 1, 1).expect("date")
    );
    assert_eq!(
        period.end,
        NaiveDate::from_ymd_opt(2026, 12, 31).expect("date")
    );
}

#[test]
fn schemas_1_0_and_1_1_still_parse_without_identity_or_period() {
    for name in [
        "minimal-1.0.json",
        "minimal-1.1.json",
        "testgarden-2026-1.1.json",
    ] {
        let snap = parse_snapshot(&read(&format!("fixtures/snapshots/{name}"))).expect(name);
        assert!(snap.entity.accounting_identity.is_none(), "{name}");
        assert!(snap.fiscal_period.is_none(), "{name}");
    }
}

#[test]
fn coordination_and_organisation_identities_parse() {
    let coord = v12_with(|v| {
        identity_mut(v)["identifier_kind"] = "coordination_number".into();
        identity_mut(v)["business_identifier"] = "200161-2395".into();
    });
    let snap = parse_snapshot(&coord).expect("coordination");
    assert_eq!(
        snap.entity.accounting_identity.expect("id").identifier_kind,
        IdentifierKind::CoordinationNumber
    );
    for entity_type in [
        "limited_company",
        "trading_partnership",
        "limited_partnership",
        "other_legal_person",
    ] {
        let org = v12_with(|v| {
            identity_mut(v)["entity_type"] = entity_type.into();
            identity_mut(v)["identifier_kind"] = "organisation_number".into();
            identity_mut(v)["business_identifier"] = "999999-0006".into();
        });
        let snap = parse_snapshot(&org).expect(entity_type);
        assert_eq!(
            snap.entity.accounting_identity.expect("id").identifier_kind,
            IdentifierKind::OrganisationNumber
        );
    }
}

#[test]
fn null_identity_and_null_period_are_structurally_valid() {
    let raw = v12_with(|v| {
        v["entity"]["accounting_identity"] = Value::Null;
        v["fiscal_period"] = Value::Null;
    });
    let snap = parse_snapshot(&raw).expect("valid snapshot");
    assert!(snap.entity.accounting_identity.is_none() && snap.fiscal_period.is_none());
}

// ---------------------------------------------------------------------------
// L-KEYS: version-specific key presence
// ---------------------------------------------------------------------------

#[test]
fn schema_1_2_refuses_org_number_even_as_null() {
    for value in [Value::Null, Value::from("999999-0006")] {
        let raw = v12_with(|v| v["entity"]["org_number"] = value.clone());
        assert!(parse_error(&raw).starts_with("entity.org_number:"));
    }
}

#[test]
fn schema_1_2_requires_both_keys() {
    let no_identity = v12_with(|v| {
        v["entity"]
            .as_object_mut()
            .expect("obj")
            .remove("accounting_identity");
    });
    assert!(parse_error(&no_identity).starts_with("entity.accounting_identity:"));
    let no_period = v12_with(|v| {
        v.as_object_mut().expect("obj").remove("fiscal_period");
    });
    assert!(parse_error(&no_period).starts_with("fiscal_period:"));
}

#[test]
fn schema_1_1_refuses_the_new_keys() {
    let mut v11: Value = serde_json::from_slice(&read(V11)).expect("json");
    v11["entity"]["accounting_identity"] = Value::Null;
    assert!(parse_error(&serde_json::to_vec(&v11).expect("ser"))
        .starts_with("entity.accounting_identity:"));
    let mut v11: Value = serde_json::from_slice(&read(V11)).expect("json");
    v11["fiscal_period"] = Value::Null;
    assert!(parse_error(&serde_json::to_vec(&v11).expect("ser")).starts_with("fiscal_period:"));
}

// ---------------------------------------------------------------------------
// Identity validation (L-WIRE-VALID) — every error text is value-free
// ---------------------------------------------------------------------------

fn identity_error(f: impl FnOnce(&mut Value)) -> String {
    let raw = v12_with(|v| f(identity_mut(v)));
    let message = parse_error(&raw);
    for token in [WIRE_PERSON, WIRE_DIGITS, "Testgården"] {
        assert!(
            !message.contains(token),
            "error text leaks {token}: {message}"
        );
    }
    message
}

#[test]
fn unknown_types_kinds_and_wrong_pairs_are_refused() {
    assert!(
        identity_error(|i| i["entity_type"] = "sole_proprietor".into())
            .contains("unknown entity type")
    );
    assert!(
        identity_error(|i| i["identifier_kind"] = "tax_number".into())
            .contains("unknown identifier kind")
    );
    assert!(
        identity_error(|i| i["identifier_kind"] = "organisation_number".into())
            .contains("does not match")
    );
    assert!(identity_error(|i| {
        i["entity_type"] = "limited_company".into();
    })
    .contains("does not match"));
    assert!(identity_error(|i| i["entity_type"] = 7.into()).contains("must be a string"));
}

#[test]
fn malformed_identifiers_are_refused() {
    for bad in [
        "9001019802",            // no hyphen
        "900101-98021",          // too long
        "19900101-9802",         // 12 digits
        " 900101-9802",          // leading space (no trimming)
        "900101-9802 ",          // trailing space
        "900101 9802",           // space instead of hyphen
        "٩٠٠١٠١-٩٨٠٢",           // Arabic-Indic digits
        "９００１０１-９８０２", // fullwidth digits
        "900101-9803",           // Luhn
        "901301-9802",           // month 13 (personal)
        "900161-9802",           // day 61: a coordination day under personal_number
    ] {
        let message = identity_error(|i| i["business_identifier"] = bad.into());
        assert!(message.contains("business_identifier"), "{bad}: {message}");
    }
    // coordination_number with an ordinary day
    let message = identity_error(|i| {
        i["identifier_kind"] = "coordination_number".into();
    });
    assert!(message.contains("business_identifier"));
    // organisation number whose third digit is < 2 (personnummer-shaped)
    let message = identity_error(|i| {
        i["entity_type"] = "limited_company".into();
        i["identifier_kind"] = "organisation_number".into();
    });
    assert!(message.contains("business_identifier"));
}

#[test]
fn legal_name_rules() {
    assert!(identity_error(|i| i["legal_name"] = "".into()).contains("must not be empty"));
    assert!(identity_error(|i| i["legal_name"] = "   ".into()).contains("must not be empty"));
    assert!(
        identity_error(|i| i["legal_name"] = "Test\u{7}gården".into())
            .contains("control characters")
    );
    // Unicode outside CP437 is kept as sent (R2-F1); the writer decides later.
    let raw = v12_with(|v| identity_mut(v)["legal_name"] = "Łukasz € Skog".into());
    let snap = parse_snapshot(&raw).expect("unicode name");
    assert_eq!(
        snap.entity.accounting_identity.expect("id").legal_name,
        "Łukasz € Skog"
    );
}

#[test]
fn identifier_is_kept_exactly_as_sent() {
    let snap = parse_snapshot(&read(V12)).expect("parse");
    assert_eq!(
        snap.entity
            .accounting_identity
            .expect("id")
            .business_identifier,
        WIRE_PERSON
    );
}

// ---------------------------------------------------------------------------
// Fiscal period
// ---------------------------------------------------------------------------

fn period_result(start: &str, end: &str) -> Result<(), String> {
    let raw = v12_with(|v| v["fiscal_period"] = serde_json::json!({"start": start, "end": end}));
    parse_snapshot(&raw).map(|_| ()).map_err(|e| e.to_string())
}

#[test]
fn eighteen_calendar_month_vectors_match_django() {
    assert!(period_result("2025-07-01", "2026-12-31").is_ok());
    assert!(period_result("2025-07-01", "2027-01-01").is_err());
    assert!(period_result("2026-01-01", "2026-12-31").is_ok());
    assert!(period_result("2025-08-31", "2027-02-27").is_ok());
    assert!(period_result("2025-08-31", "2027-02-28").is_err());
}

#[test]
fn period_shape_and_order() {
    assert!(period_result("2026-12-31", "2026-01-01")
        .expect_err("order")
        .contains("start is after end"));
    for bad in [
        "2026-1-01",
        "20260101",
        "2026-02-30",
        "2026-01-01T00:00",
        "２０２６-01-01",
    ] {
        assert!(period_result(bad, "2026-12-31").is_err(), "{bad}");
    }
}

#[test]
fn period_is_never_derived_from_income_year() {
    // A shortened first year is carried as sent.
    let raw = v12_with(|v| {
        v["fiscal_period"] = serde_json::json!({"start": "2026-03-15", "end": "2026-12-31"})
    });
    let period = parse_snapshot(&raw)
        .expect("parse")
        .fiscal_period
        .expect("period");
    assert_eq!(
        period.start,
        NaiveDate::from_ymd_opt(2026, 3, 15).expect("date")
    );
}

// ---------------------------------------------------------------------------
// Ingest, engine, writer regression
// ---------------------------------------------------------------------------

#[test]
fn v1_2_ingests_with_null_legacy_org_number() {
    let db = TempDb::new("ingest");
    let mut ws = Workspace::create(db.path()).expect("create");
    let ingested = ws.ingest_snapshot(&read(V12)).expect("ingest");
    match ingested.outcome() {
        IngestOutcome::Ingested {
            snapshot_sha256,
            properties,
            receipts,
            income_entries,
            ..
        } => {
            assert_eq!(
                (
                    snapshot_sha256.as_str(),
                    *properties,
                    *receipts,
                    *income_entries
                ),
                (V12_DIGEST, 2, 21, 6)
            );
        }
        other => panic!("unexpected outcome {other:?}"),
    }
    assert_eq!(ingested.snapshot_sha256(), V12_DIGEST);
    assert_eq!(ws.snapshot_sha256().expect("digest"), V12_DIGEST);
    assert_eq!(ws.read_entity().expect("entity").org_number, None);
}

#[test]
fn ingest_and_ingest_snapshot_agree() {
    let a = TempDb::new("agree-a");
    let b = TempDb::new("agree-b");
    let mut wa = Workspace::create(a.path()).expect("create");
    let mut wb = Workspace::create(b.path()).expect("create");
    let outcome = wa.ingest(&read(V12)).expect("ingest");
    let ingested = wb.ingest_snapshot(&read(V12)).expect("ingest_snapshot");
    assert_eq!(&outcome, ingested.outcome());
    assert_eq!(
        wa.logical_fingerprint().expect("fp"),
        wb.logical_fingerprint().expect("fp")
    );
    // The already-ingested branch still returns a typed snapshot.
    let again = wb.ingest_snapshot(&read(V12)).expect("again");
    assert!(matches!(
        again.outcome(),
        IngestOutcome::AlreadyIngested { .. }
    ));
    assert_eq!(again.snapshot(), ingested.snapshot());
}

type CaseShape = (
    String,
    Option<String>,
    String,
    Vec<(String, String, i64, i64)>,
);

fn case_shapes(ws: &Workspace) -> BTreeMap<(String, String), Vec<CaseShape>> {
    let lines = ws.read_decision_lines().expect("lines");
    let mut out: BTreeMap<(String, String), Vec<CaseShape>> = BTreeMap::new();
    for case in ws.read_cases().expect("cases") {
        let case_lines = lines
            .iter()
            .filter(|l| l.case_seq == case.case_seq)
            .map(|l| (l.account.clone(), l.role.clone(), l.debit.0, l.credit.0))
            .collect();
        out.entry((case.source.clone(), case.date.to_string()))
            .or_default()
            .push((
                case.source.clone(),
                case.subject.clone(),
                case.status.clone(),
                case_lines,
            ));
    }
    for v in out.values_mut() {
        v.sort();
    }
    out
}

#[test]
fn engine_semantics_are_unchanged_between_1_1_and_1_2() {
    let md = masterdata();
    let a = TempDb::new("sem-11");
    let b = TempDb::new("sem-12");
    let mut w11 = Workspace::create(a.path()).expect("create");
    w11.ingest(&read(V11)).expect("ingest 1.1");
    run_engine(&mut w11, &md).expect("engine 1.1");
    let (w12, _) = engine_run(&b, &read(V12), &md);
    assert_eq!(case_shapes(&w11), case_shapes(&w12));
    assert_eq!(
        w12.read_run_meta().expect("run").provenance.snapshot_sha256,
        V12_DIGEST
    );
}

#[test]
fn vnext_minus_identity_lines_is_the_historical_441_byte_file() {
    // SK-SIE-BRIDGE-04: the writer now takes identity only from the context.
    // Removing exactly #ORGNR and #RAR 0 from the vNext file must give the
    // historical Slice-1 bytes — the accounting content is unchanged.
    let md = masterdata();
    let db = TempDb::new("writer-regression");
    let (ws, ingested) = engine_run(&db, &read(V12), &md);
    let ctx = VerifiedExportContext::bind(&ingested, &ws).expect("bind");
    let meta = ExportMetadata {
        generated_on: NaiveDate::from_ymd_opt(2026, 9, 10).expect("date"),
        program: ProgramId {
            name: "SIEverk".to_string(),
            version: "0.4.0".to_string(),
        },
    };
    let out = write_sie4i(&ws, &md, &ctx, &meta).expect("export");
    assert_eq!(out.bytes.len(), 487);
    let mut bytes = out.bytes.clone();
    for line in ["#ORGNR 900101-9802\r\n", "#RAR 0 20260101 20261231\r\n"] {
        let needle = line.as_bytes();
        let at = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("line present once");
        bytes.drain(at..at + needle.len());
    }
    assert_eq!(bytes.len(), 441);
    assert_eq!(ws.sha256_hex(&bytes).expect("sha"), HISTORICAL_SI_SHA256);
}

// ---------------------------------------------------------------------------
// VerifiedExportContext
// ---------------------------------------------------------------------------

#[test]
fn bind_succeeds_on_complete_1_2_and_reads_no_file() {
    let md = masterdata();
    let db = TempDb::new("bind-ok");
    let copy = std::env::temp_dir().join(format!("sieverk-v12-{}.json", std::process::id()));
    fs::copy(repo(V12), &copy).expect("copy");
    let raw = fs::read(&copy).expect("read once");
    let started = Instant::now();
    let (ws, ingested) = engine_run(&db, &raw, &md);
    let ingest_and_engine = started.elapsed();
    fs::remove_file(&copy).expect("the only snapshot file is gone before bind");
    let started = Instant::now();
    let ctx = VerifiedExportContext::bind(&ingested, &ws).expect("bind");
    eprintln!(
        "informational wall time (no threshold): ingest_snapshot+engine {ingest_and_engine:?}, bind {:?}",
        started.elapsed()
    );
    assert_eq!(ctx.legal_name(), "Testgården");
    assert_eq!(ctx.entity_type(), EntityType::SoleTrader);
    assert_eq!(ctx.identifier_kind(), IdentifierKind::PersonalNumber);
    assert_eq!(ctx.business_identifier(), WIRE_PERSON);
    assert_eq!(
        ctx.fiscal_period().start,
        NaiveDate::from_ymd_opt(2026, 1, 1).expect("d")
    );
    assert_eq!(
        ctx.fiscal_period().end,
        NaiveDate::from_ymd_opt(2026, 12, 31).expect("d")
    );
    assert_eq!(ctx.snapshot_sha256(), V12_DIGEST);
}

#[test]
fn bind_before_the_engine_run_is_refused() {
    let db = TempDb::new("bind-norun");
    let mut ws = Workspace::create(db.path()).expect("create");
    let ingested = ws.ingest_snapshot(&read(V12)).expect("ingest");
    let e = VerifiedExportContext::bind(&ingested, &ws).expect_err("no run");
    assert!(matches!(e, ExportContextError::RunNotVerified(_)), "{e:?}");
}

#[test]
fn bind_refuses_1_1_and_null_identity_or_period() {
    let md = masterdata();
    let db = TempDb::new("bind-11");
    let (ws, ingested) = engine_run(&db, &read(V11), &md);
    assert!(matches!(
        VerifiedExportContext::bind(&ingested, &ws),
        Err(ExportContextError::IdentityMissing)
    ));

    let db = TempDb::new("bind-null-id");
    let raw = v12_with(|v| v["entity"]["accounting_identity"] = Value::Null);
    let (ws, ingested) = engine_run(&db, &raw, &md);
    assert!(matches!(
        VerifiedExportContext::bind(&ingested, &ws),
        Err(ExportContextError::IdentityMissing)
    ));

    let db = TempDb::new("bind-null-period");
    let raw = v12_with(|v| v["fiscal_period"] = Value::Null);
    let (ws, ingested) = engine_run(&db, &raw, &md);
    assert!(matches!(
        VerifiedExportContext::bind(&ingested, &ws),
        Err(ExportContextError::FiscalPeriodMissing)
    ));
}

#[test]
fn envelope_from_another_snapshot_is_refused() {
    let md = masterdata();
    let a = TempDb::new("mismatch-a");
    let b = TempDb::new("mismatch-b");
    let (_, ingested_a) = engine_run(&a, &read(V12), &md);
    let other = v12_with(|v| identity_mut(v)["legal_name"] = "Annan".into());
    let (ws_b, _) = engine_run(&b, &other, &md);
    let e = VerifiedExportContext::bind(&ingested_a, &ws_b).expect_err("A envelope, B workspace");
    assert!(
        matches!(
            e,
            ExportContextError::ProvenanceMismatch { point: "workspace" }
        ),
        "{e:?}"
    );
}

#[test]
fn corrupt_run_is_refused() {
    let md = masterdata();
    let db = TempDb::new("corrupt");
    let (ws, ingested) = engine_run(&db, &read(V12), &md);
    ws.connection()
        .execute_batch("UPDATE decision_lines SET debit_ore = debit_ore + 1 WHERE case_seq = (SELECT min(case_seq) FROM decision_lines) AND line_no = 0")
        .expect("mutate");
    let e = VerifiedExportContext::bind(&ingested, &ws).expect_err("corrupt");
    assert!(matches!(e, ExportContextError::RunNotVerified(_)), "{e:?}");
}

#[test]
fn run_naming_another_snapshot_is_caught_by_the_authoritative_verifier_first() {
    // Observed first-failure behaviour (BRIDGE-03 review note): verify_run()
    // already requires engine_run_meta to name the workspace snapshot, so a
    // mutated run digest fails at step 1, before bind's own step 3.
    let md = masterdata();
    let db = TempDb::new("run-digest");
    let (ws, ingested) = engine_run(&db, &read(V12), &md);
    ws.connection()
        .execute_batch("UPDATE engine_run_meta SET snapshot_sha256 = repeat('0', 64)")
        .expect("mutate");
    let e = VerifiedExportContext::bind(&ingested, &ws).expect_err("mutated run");
    assert!(matches!(e, ExportContextError::RunNotVerified(_)), "{e:?}");
}

// ---------------------------------------------------------------------------
// PII: nothing sensitive persisted, printed or echoed
// ---------------------------------------------------------------------------

fn tokens() -> [&'static str; 3] {
    [WIRE_PERSON, WIRE_DIGITS, LEGAL_NAME_TOKEN]
}

fn token_snapshot() -> Vec<u8> {
    v12_with(|v| identity_mut(v)["legal_name"] = LEGAL_NAME_TOKEN.into())
}

#[test]
fn identity_and_legal_name_are_absent_from_every_duckdb_table() {
    let md = masterdata();
    let db = TempDb::new("pii-scan");
    let raw = token_snapshot();
    let parsed = parse_snapshot(&raw).expect("parse");
    assert_ne!(
        parsed.entity.display_name, LEGAL_NAME_TOKEN,
        "the token must differ from display_name"
    );
    let (ws, ingested) = engine_run(&db, &raw, &md);
    let ctx = VerifiedExportContext::bind(&ingested, &ws).expect("bind");
    assert_eq!(ctx.legal_name(), LEGAL_NAME_TOKEN);

    {
        let conn = ws.connection();
        let mut stmt = conn
            .prepare("SELECT table_name, column_name FROM information_schema.columns WHERE table_schema = 'main'")
            .expect("columns");
        let columns: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
            .expect("query")
            .map(|c| c.expect("row"))
            .collect();
        assert!(
            columns.len() > 20,
            "scan must cover the real schema, saw {}",
            columns.len()
        );
        for (table, column) in &columns {
            for token in tokens() {
                let hits: i64 = conn
                    .query_row(
                        &format!("SELECT count(*) FROM \"{table}\" WHERE CAST(\"{column}\" AS VARCHAR) LIKE ?"),
                        [format!("%{token}%")],
                        |r| r.get(0),
                    )
                    .expect("scan");
                assert_eq!(hits, 0, "{token} found in {table}.{column}");
            }
        }
    }
    drop(ws);
    // Supplementary: the closed file and WAL bytes (compression could hide a
    // value, so the table scan above is the authoritative check).
    for path in [
        db.path().to_path_buf(),
        db.path().with_extension("duckdb.wal"),
    ] {
        if let Ok(bytes) = fs::read(&path) {
            for token in tokens() {
                assert!(
                    !bytes.windows(token.len()).any(|w| w == token.as_bytes()),
                    "{token} in {}",
                    path.display()
                );
            }
        }
    }
}

#[test]
fn debug_and_errors_never_print_identity() {
    let md = masterdata();
    let db = TempDb::new("pii-debug");
    let raw = token_snapshot();
    let (ws, ingested) = engine_run(&db, &raw, &md);
    let ctx = VerifiedExportContext::bind(&ingested, &ws).expect("bind");
    let snap = parse_snapshot(&raw).expect("parse");
    let texts = [
        format!("{ctx:?}"),
        format!("{ingested:?}"),
        format!("{snap:?}"),
        format!("{:?}", snap.entity.accounting_identity),
        ws.logical_fingerprint().expect("fp"),
        ExportContextError::IdentityMissing.to_string(),
        ExportContextError::ProvenanceMismatch { point: "workspace" }.to_string(),
    ];
    for text in &texts {
        for token in tokens() {
            assert!(!text.contains(token), "{token} leaked: {text}");
        }
    }
    assert!(format!("{ctx:?}").contains("[REDACTED]"));
}

fn run_cli(args: &[&str]) -> (String, String) {
    let out = Command::new(env!("CARGO_BIN_EXE_sieverk"))
        .args(args)
        .output()
        .expect("binary runs");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
    )
}

#[test]
fn inspect_commands_print_no_identity() {
    let snapshot_path =
        std::env::temp_dir().join(format!("sieverk-v12-token-{}.json", std::process::id()));
    fs::write(&snapshot_path, token_snapshot()).expect("write");
    let (out, err) = run_cli(&["inspect-snapshot", snapshot_path.to_str().expect("utf8")]);
    assert!(out.contains("Schema:      1.2"), "{out}{err}");
    let db = TempDb::new("pii-cli");
    let mut ws = Workspace::create(db.path()).expect("create");
    ws.ingest_snapshot(&token_snapshot()).expect("ingest");
    drop(ws);
    let (wout, werr) = run_cli(&[
        "inspect-workspace",
        "--workspace",
        db.path().to_str().expect("utf8"),
    ]);
    for text in [&out, &err, &wout, &werr] {
        for token in tokens() {
            assert!(!text.contains(token), "{token} printed: {text}");
        }
    }
    let _ = fs::remove_file(&snapshot_path);
}
