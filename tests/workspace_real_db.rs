//! workspace_real_db (SV-02D §4) — every test here opens a REAL DuckDB file
//! on disk. No mocks, no in-memory-only proof. Slice 1 covers the container
//! contract: create/open states, schema v1, typed columns, corruption.

use std::fs;
use std::path::PathBuf;

use chrono::NaiveDate;
use duckdb::Connection;
use serde_json::Value;
use sieverk::engine::{
    assess_cases, decide_cases, project_cases, run_engine, CaseSource, SourceFacts,
};
use sieverk::money::Ore;
use sieverk::ruleset::load_masterdata;
use sieverk::snapshot::parse_snapshot;
use sieverk::workspace::{
    canonical_decision_text, canonical_snapshot_bytes, EngineState, IngestOutcome, PersistedCase,
    PersistedFinding, PersistedLine, RunProvenance, Workspace, WorkspaceError,
    ENGINE_RUN_META_COLUMNS, SCHEMA_V1_TABLES, SCHEMA_V2_TABLES, WORKSPACE_SCHEMA,
    WORKSPACE_SCHEMA_DECISIONS, WORKSPACE_SCHEMA_ENGINE,
};

/// A unique path under the OS temp dir; removed before use so each test
/// starts from "no file", and removed again on drop.
struct TempDb(PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join("sieverk-workspace-tests");
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{}-{name}.duckdb", std::process::id()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("duckdb.wal"));
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(self.0.with_extension("duckdb.wal"));
    }
}

#[test]
fn create_writes_a_real_file_with_one_meta_row_and_no_snapshot() {
    let db = TempDb::new("create");
    let ws = Workspace::create(db.path()).expect("create");
    assert!(db.path().is_file(), "a real file must exist on disk");
    assert_eq!(ws.schema_version().expect("schema"), WORKSPACE_SCHEMA);
    assert_eq!(
        ws.created_by_version().expect("version"),
        env!("CARGO_PKG_VERSION")
    );
    assert_eq!(ws.snapshot_count().expect("count"), 0);
    assert!(matches!(
        ws.snapshot_sha256(),
        Err(WorkspaceError::NotIngested)
    ));
    ws.close().expect("close");
    assert!(fs::metadata(db.path()).expect("metadata").len() > 0);
}

#[test]
fn create_refuses_an_existing_path_and_leaves_it_untouched() {
    let db = TempDb::new("refuse");
    Workspace::create(db.path())
        .expect("create")
        .close()
        .expect("close");
    let before = fs::read(db.path()).expect("read");
    let err = Workspace::create(db.path()).expect_err("second create must fail");
    assert!(matches!(err, WorkspaceError::AlreadyExists(_)), "{err}");
    assert!(err.to_string().contains("already exists"));
    assert_eq!(
        fs::read(db.path()).expect("read"),
        before,
        "file bytes must be untouched"
    );
}

#[test]
fn close_then_reopen_from_disk_preserves_the_container() {
    let db = TempDb::new("reopen");
    Workspace::create(db.path())
        .expect("create")
        .close()
        .expect("close");
    let ws = Workspace::open(db.path()).expect("reopen");
    assert_eq!(ws.schema_version().expect("schema"), WORKSPACE_SCHEMA);
    assert_eq!(ws.snapshot_count().expect("count"), 0);
    let mut expected: Vec<String> = SCHEMA_V1_TABLES.iter().map(|s| s.to_string()).collect();
    expected.sort();
    assert_eq!(ws.table_names().expect("tables"), expected);
    assert_eq!(ws.row_count("workspace_meta").expect("count"), 1);
    for table in SCHEMA_V1_TABLES.iter().filter(|t| **t != "workspace_meta") {
        assert_eq!(ws.row_count(table).expect("count"), 0, "{table}");
    }
}

#[test]
fn open_on_missing_path_is_not_found_never_a_new_file() {
    let db = TempDb::new("missing");
    let err = Workspace::open(db.path()).expect_err("must fail");
    assert!(matches!(err, WorkspaceError::NotFound(_)), "{err}");
    assert!(!db.path().exists(), "open must never create a file");
}

#[test]
fn open_on_a_non_database_file_is_an_error_not_a_panic() {
    let db = TempDb::new("garbage");
    fs::write(db.path(), b"this is not a duckdb database\n").expect("write");
    let err = Workspace::open(db.path()).expect_err("must fail");
    assert!(!err.to_string().is_empty());
}

#[test]
fn schema_mismatch_is_reported_with_both_versions() {
    let db = TempDb::new("mismatch");
    Workspace::create(db.path())
        .expect("create")
        .close()
        .expect("close");
    {
        let raw = Connection::open(db.path()).expect("raw open");
        raw.execute_batch("UPDATE workspace_meta SET workspace_schema = 99")
            .expect("update");
        if let Err((_, e)) = raw.close() {
            panic!("raw close: {e}");
        }
    }
    let err = Workspace::open(db.path()).expect_err("must fail");
    assert!(
        matches!(
            err,
            WorkspaceError::SchemaMismatch {
                found: 99,
                expected: WORKSPACE_SCHEMA_DECISIONS
            }
        ),
        "{err}"
    );
    assert!(err.to_string().contains("99"));
}

#[test]
fn duplicated_meta_row_or_two_snapshots_is_corrupt() {
    let db = TempDb::new("corrupt");
    Workspace::create(db.path())
        .expect("create")
        .close()
        .expect("close");
    {
        let raw = Connection::open(db.path()).expect("raw open");
        raw.execute_batch("INSERT INTO workspace_meta VALUES (1, 'dup');")
            .expect("insert");
        if let Err((_, e)) = raw.close() {
            panic!("raw close: {e}");
        }
    }
    let err = Workspace::open(db.path()).expect_err("two meta rows must fail");
    assert!(matches!(err, WorkspaceError::Corrupt(_)), "{err}");

    let db2 = TempDb::new("two-snapshots");
    Workspace::create(db2.path())
        .expect("create")
        .close()
        .expect("close");
    {
        let raw = Connection::open(db2.path()).expect("raw open");
        raw.execute_batch(
            "INSERT INTO snapshot_meta VALUES ('a', '1.1', 1, 2026, 't', false, 2027, 'skogskvitto', 'dev');
             INSERT INTO snapshot_meta VALUES ('b', '1.1', 1, 2026, 't', false, 2027, 'skogskvitto', 'dev');",
        )
        .expect("insert");
        if let Err((_, e)) = raw.close() {
            panic!("raw close: {e}");
        }
    }
    let err = Workspace::open(db2.path()).expect_err(">1 snapshot rows must fail");
    assert!(matches!(err, WorkspaceError::Corrupt(_)), "{err}");
    assert!(err.to_string().contains("snapshot_meta"));
}

#[test]
fn ensure_schema_is_idempotent_but_create_is_not() {
    let db = TempDb::new("idempotent");
    let ws = Workspace::create(db.path()).expect("create");
    ws.ensure_schema().expect("second ddl run");
    ws.ensure_schema().expect("third ddl run");
    assert_eq!(ws.schema_version().expect("schema"), WORKSPACE_SCHEMA);
    assert_eq!(
        ws.table_names().expect("tables").len(),
        SCHEMA_V1_TABLES.len()
    );
    ws.close().expect("close");
    assert!(Workspace::create(db.path()).is_err());
}

#[test]
fn money_is_bigint_dates_are_date_flags_are_boolean_never_varchar() {
    let db = TempDb::new("types");
    let ws = Workspace::create(db.path()).expect("create");
    let ty = |t: &str, c: &str| ws.column_type(t, c).expect("column type");
    for col in ["total_ore", "vat_ore", "rounding_ore", "net_ore"] {
        assert_eq!(ty("receipts", col), "BIGINT", "{col}");
    }
    for col in ["ex_vat_ore", "vat_ore", "inc_vat_ore"] {
        assert_eq!(ty("income_entries", col), "BIGINT", "{col}");
    }
    // Four DATE columns in schema v1.
    assert_eq!(ty("receipts", "date"), "DATE");
    assert_eq!(ty("income_entries", "date"), "DATE");
    assert_eq!(ty("income_entries", "payment_date"), "DATE");
    assert_eq!(ty("audit_chain", "received_date"), "DATE");
    for col in [
        "requires_business_share",
        "investment_risk",
        "vat_check",
        "sensitive",
        "has_image",
    ] {
        assert_eq!(ty("receipts", col), "BOOLEAN", "{col}");
    }
    assert_eq!(ty("snapshot_meta", "all_properties_locked"), "BOOLEAN");
    assert_eq!(ty("properties", "is_default"), "BOOLEAN");
    assert_eq!(ty("properties", "property_id"), "BIGINT");
    assert_eq!(ty("workspace_meta", "workspace_schema"), "INTEGER");
    // No SV-03 tables in schema v1.
    let tables = ws.table_names().expect("tables");
    for later in [
        "engine_run_meta",
        "accounting_cases",
        "decision_lines",
        "findings",
    ] {
        assert!(
            !tables.iter().any(|t| t == later),
            "{later} belongs to SV-03"
        );
    }
}

#[test]
fn row_count_rejects_tables_outside_the_schema() {
    let db = TempDb::new("rowcount");
    let ws = Workspace::create(db.path()).expect("create");
    assert!(matches!(
        ws.row_count("sqlite_master"),
        Err(WorkspaceError::Corrupt(_))
    ));
    assert_eq!(ws.row_count("receipts").expect("count"), 0);
}

#[test]
fn two_workspaces_from_nothing_have_identical_container_state() {
    let a = TempDb::new("det-a");
    let b = TempDb::new("det-b");
    let wa = Workspace::create(a.path()).expect("create a");
    let wb = Workspace::create(b.path()).expect("create b");
    assert_eq!(wa.table_names().expect("a"), wb.table_names().expect("b"));
    assert_eq!(
        wa.schema_version().expect("a"),
        wb.schema_version().expect("b")
    );
    assert_eq!(
        wa.created_by_version().expect("a"),
        wb.created_by_version().expect("b")
    );
    // Logical row-equivalence is the contract — the two files' bytes need not match.
}

// ---------------------------------------------------------------------------
// Slice 2 — digest: exactly Django's snapshot_sha256(), hashed by DuckDB
// ---------------------------------------------------------------------------

fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("fixtures/snapshots")
            .join(name),
    )
    .expect("fixture exists")
}

fn django_digests() -> Vec<(String, String)> {
    let golden: Value =
        serde_json::from_slice(&fixture("django-digests.json")).expect("golden json");
    let mut pairs: Vec<(String, String)> = golden["digests"]
        .as_object()
        .expect("digests object")
        .iter()
        .map(|(k, v)| (k.clone(), v.as_str().expect("hex").to_string()))
        .collect();
    pairs.sort();
    pairs
}

#[test]
fn duckdb_sha256_matches_the_standard_test_vector() {
    let db = TempDb::new("sha-vector");
    let ws = Workspace::create(db.path()).expect("create");
    assert_eq!(
        ws.sha256_hex(b"abc").expect("hash"),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(
        ws.sha256_hex(b"").expect("hash"),
        "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
    );
}

#[test]
fn snapshot_digest_equals_django_for_every_fixture() {
    let db = TempDb::new("digest-golden");
    let ws = Workspace::create(db.path()).expect("create");
    let pairs = django_digests();
    assert!(
        pairs.len() >= 7,
        "golden must cover the fixtures, got {}",
        pairs.len()
    );
    for (name, django) in &pairs {
        let rust = ws.snapshot_digest(&fixture(name)).expect(name);
        assert_eq!(
            &rust, django,
            "{name}: Rust/DuckDB digest must equal Django's"
        );
    }
}

#[test]
fn testgarden_fixture_is_real_schema_1_1_and_parses_typed_separately() {
    let raw = fixture("testgarden-2026-1.1.json");
    let snap = parse_snapshot(&raw)
        .expect("Testgården snapshot parses with the unchanged snapshot module");
    assert_eq!(snap.schema_version, "1.1");
    assert_eq!(snap.properties.len(), 2);
    assert_eq!(snap.receipts.len(), 21);
    assert_eq!(snap.income_entries.len(), 6);
    assert_eq!(snap.audit_chain.len(), 0);
    assert!(!snap.lock.all_properties_locked);
    let db = TempDb::new("digest-testgarden");
    let ws = Workspace::create(db.path()).expect("create");
    let expected = django_digests()
        .into_iter()
        .find(|(n, _)| n == "testgarden-2026-1.1.json")
        .map(|(_, d)| d)
        .expect("golden has testgarden");
    assert_eq!(ws.snapshot_digest(&raw).expect("digest"), expected);
}

#[test]
fn digest_ignores_top_level_generated_at_but_not_content() {
    let db = TempDb::new("digest-invariance");
    let ws = Workspace::create(db.path()).expect("create");
    let raw = fixture("minimal-1.1.json");
    let base = ws.snapshot_digest(&raw).expect("digest");

    let mut v: Value = serde_json::from_slice(&raw).expect("json");
    v["generated_at"] = Value::from("2030-01-01T00:00:00+01:00");
    let regenerated = serde_json::to_vec_pretty(&v).expect("json");
    assert_eq!(
        ws.snapshot_digest(&regenerated).expect("digest"),
        base,
        "generated_at is excluded"
    );
    // Re-serialising with different whitespace/key order must not matter either.
    let compact = serde_json::to_vec(&v).expect("json");
    assert_eq!(ws.snapshot_digest(&compact).expect("digest"), base);

    let mut changed: Value = serde_json::from_slice(&raw).expect("json");
    changed["receipts"][0]["vat_amount"] = Value::from("249.00");
    assert_ne!(
        ws.snapshot_digest(&serde_json::to_vec(&changed).expect("json"))
            .expect("digest"),
        base
    );
}

#[test]
fn canonical_bytes_are_compact_sorted_and_utf8() {
    let raw = r#"{"generated_at":"x","zeta":[1,{"b":"åäö Skogsvård","a":null}],"alpha":"q\"uote"}"#
        .as_bytes();
    let canonical = canonical_snapshot_bytes(raw).expect("canonical");
    assert_eq!(
        String::from_utf8(canonical).expect("utf8"),
        r#"{"alpha":"q\"uote","zeta":[1,{"a":null,"b":"åäö Skogsvård"}]}"#
    );
    let err = canonical_snapshot_bytes(b"[1,2]").expect_err("array is not a snapshot");
    assert!(matches!(err, WorkspaceError::Corrupt(_)), "{err}");
    let err = canonical_snapshot_bytes(b"{ not json").expect_err("garbage");
    assert!(matches!(err, WorkspaceError::Corrupt(_)), "{err}");
}

// ---------------------------------------------------------------------------
// Slice 3 — ingest: one transaction, Appender per table, rollback on any error
// ---------------------------------------------------------------------------

const TESTGARDEN: &str = "testgarden-2026-1.1.json";
const SNAPSHOT_TABLES: [&str; 7] = [
    "snapshot_meta",
    "entity_context",
    "entity_operations",
    "properties",
    "receipts",
    "income_entries",
    "audit_chain",
];

fn counts(ws: &Workspace) -> Vec<(String, i64)> {
    SNAPSHOT_TABLES
        .iter()
        .map(|t| ((*t).to_string(), ws.row_count(t).expect("count")))
        .collect()
}

fn ingest_testgarden(db: &TempDb) -> Workspace {
    let mut ws = Workspace::create(db.path()).expect("create");
    let outcome = ws.ingest(&fixture(TESTGARDEN)).expect("ingest Testgården");
    assert!(
        matches!(
            outcome,
            IngestOutcome::Ingested {
                receipts: 21,
                income_entries: 6,
                properties: 2,
                audit_items: 0,
                ..
            }
        ),
        "{outcome:?}"
    );
    ws
}

fn one_i64(ws: &Workspace, sql: &str) -> i64 {
    ws.connection().query_row(sql, [], |r| r.get(0)).expect(sql)
}

fn one_string(ws: &Workspace, sql: &str) -> String {
    ws.connection().query_row(sql, [], |r| r.get(0)).expect(sql)
}

fn strings(ws: &Workspace, sql: &str) -> Vec<String> {
    let mut stmt = ws.connection().prepare(sql).expect(sql);
    let rows = stmt.query_map([], |r| r.get::<_, String>(0)).expect(sql);
    rows.map(|r| r.expect("row")).collect()
}

#[test]
fn a_testgarden_ingest_survives_close_and_reopen_with_exact_counts() {
    let db = TempDb::new("ingest-testgarden");
    let ws = ingest_testgarden(&db);
    let digest = ws.snapshot_sha256().expect("digest");
    ws.close().expect("close");

    let ws = Workspace::open(db.path()).expect("reopen");
    assert_eq!(ws.snapshot_sha256().expect("digest"), digest);
    assert_eq!(
        counts(&ws),
        vec![
            ("snapshot_meta".to_string(), 1),
            ("entity_context".to_string(), 1),
            ("entity_operations".to_string(), 4),
            ("properties".to_string(), 2),
            ("receipts".to_string(), 21),
            ("income_entries".to_string(), 6),
            ("audit_chain".to_string(), 0),
        ]
    );
    assert_eq!(ws.row_count("workspace_meta").expect("meta"), 1);
    // The digest stored is Django's (fixtures/snapshots/django-digests.json).
    let expected = django_digests()
        .into_iter()
        .find(|(n, _)| n == TESTGARDEN)
        .map(|(_, d)| d)
        .expect("golden");
    assert_eq!(digest, expected);
}

#[test]
fn b_testgarden_exact_values_types_and_order() {
    let db = TempDb::new("ingest-values");
    let ws = ingest_testgarden(&db);
    ws.close().expect("close");
    let ws = Workspace::open(db.path()).expect("reopen");

    // Större inköp — reservdel till skördare: 60 000,00 / 12 000,00 kr in öre.
    assert_eq!(
        one_i64(
            &ws,
            "SELECT total_ore FROM receipts WHERE category = 'Större inköp'"
        ),
        6_000_000
    );
    assert_eq!(
        one_i64(
            &ws,
            "SELECT vat_ore FROM receipts WHERE category = 'Större inköp'"
        ),
        1_200_000
    );
    // Both rounding signs, in öre.
    let rounding = strings(
        &ws,
        "SELECT CAST(rounding_ore AS VARCHAR) FROM receipts WHERE rounding_ore <> 0 ORDER BY rounding_ore",
    );
    assert_eq!(rounding, vec!["-20", "30"]);
    // Real DATE column, real date value, round-tripped through DuckDB.
    assert_eq!(
        one_string(&ws, "SELECT typeof(date) FROM receipts LIMIT 1"),
        "DATE"
    );
    assert_eq!(
        one_string(
            &ws,
            "SELECT CAST(date AS VARCHAR) FROM receipts WHERE vendor = 'Mackens Bensin AB'"
        ),
        "2026-08-20"
    );
    assert_eq!(
        one_i64(
            &ws,
            "SELECT count(*) FROM income_entries WHERE payment_date IS NULL"
        ),
        1,
        "efterlikvid has no payment_date"
    );
    // Order: selector_position preserves the snapshot's vector order exactly.
    let snap = parse_snapshot(&fixture(TESTGARDEN)).expect("typed parse");
    let expected_keys: Vec<String> = snap
        .receipts
        .iter()
        .map(|r| r.source_key.clone().expect("1.1 has source keys"))
        .collect();
    assert_eq!(
        strings(
            &ws,
            "SELECT source_key FROM receipts ORDER BY selector_position"
        ),
        expected_keys
    );
    let expected_income: Vec<String> = snap
        .income_entries
        .iter()
        .map(|e| e.source_key.clone().expect("1.1 has source keys"))
        .collect();
    assert_eq!(
        strings(
            &ws,
            "SELECT source_key FROM income_entries ORDER BY snapshot_position"
        ),
        expected_income
    );
    // Ordinals per property: Testgården 1..19, Norrskogen 1..2 — never renumbered.
    assert_eq!(
        one_i64(
            &ws,
            "SELECT CAST(max(ordinal_number) AS BIGINT) FROM receipts"
        ),
        19,
        "no global 1..21 sequence"
    );
    assert_eq!(
        one_i64(
            &ws,
            "SELECT count(DISTINCT property_id) FROM receipts WHERE ordinal_number = 1"
        ),
        2
    );
}

#[test]
fn c_downstream_inputs_are_all_in_the_db_no_json_sidechannel() {
    let db = TempDb::new("ingest-downstream");
    let raw = fixture(TESTGARDEN);
    let snap = parse_snapshot(&raw).expect("typed parse");
    ingest_testgarden(&db).close().expect("close");
    let ws = Workspace::open(db.path()).expect("reopen");

    let profile = snap
        .entity
        .accounting_profile
        .as_ref()
        .expect("1.1 profile");

    struct EntityContextRow {
        display_name: String,
        org_number: Option<String>,
        county: Option<String>,
        taxonomy_version: Option<String>,
        vat_registered: String,
        bookkeeping_method: String,
        default_payment_method: String,
        sie_series: String,
    }

    let row = ws
        .connection()
        .query_row(
            "SELECT display_name, org_number, county, taxonomy_version, vat_registered, \
             bookkeeping_method, default_payment_method, sie_series FROM entity_context",
            [],
            |r| {
                Ok(EntityContextRow {
                    display_name: r.get(0)?,
                    org_number: r.get(1)?,
                    county: r.get(2)?,
                    taxonomy_version: r.get(3)?,
                    vat_registered: r.get(4)?,
                    bookkeeping_method: r.get(5)?,
                    default_payment_method: r.get(6)?,
                    sie_series: r.get(7)?,
                })
            },
        )
        .expect("entity_context");

    assert_eq!(row.display_name, snap.entity.display_name);
    assert_eq!(row.org_number, snap.entity.org_number);
    assert_eq!(row.county, snap.entity.county);
    assert_eq!(row.taxonomy_version, snap.entity.taxonomy_version);
    assert_eq!(
        (
            row.vat_registered,
            row.bookkeeping_method,
            row.default_payment_method,
            row.sie_series,
        ),
        (
            profile.vat_registered.clone(),
            profile.bookkeeping_method.clone(),
            profile.default_payment_method.clone(),
            profile.sie_series.clone(),
        )
    );

    let mut ops = strings(
        &ws,
        "SELECT operation FROM entity_operations ORDER BY operation",
    );
    let mut expected_ops = snap.entity.operation.clone();
    ops.sort();
    expected_ops.sort();
    assert_eq!(ops, expected_ops);
    assert_eq!(
        one_i64(
            &ws,
            "SELECT CAST(declaration_year AS BIGINT) FROM snapshot_meta"
        ),
        i64::from(snap.lock.declaration_year)
    );
    assert_eq!(
        one_string(
            &ws,
            "SELECT CAST(all_properties_locked AS VARCHAR) FROM snapshot_meta"
        ),
        "false"
    );
    // category_context of the first receipt, field for field.
    let first = &snap.receipts[0];
    let ctx = first.category_context.as_ref().expect("1.1 has context");
    let flags: (bool, bool, bool, bool) = ws
        .connection()
        .query_row(
            "SELECT requires_business_share, investment_risk, vat_check, sensitive \
             FROM receipts WHERE selector_position = 0",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
        )
        .expect("flags");
    assert_eq!(
        flags,
        (
            ctx.requires_business_share,
            ctx.investment_risk,
            ctx.vat_check,
            ctx.sensitive
        )
    );
    // Properties carry the tax-year status the lock derives from.
    assert_eq!(
        strings(
            &ws,
            "SELECT slug || ':' || tax_year_status FROM properties ORDER BY property_id"
        ),
        snap.properties
            .iter()
            .map(|p| format!("{}:{}", p.slug, p.tax_year.status))
            .collect::<Vec<_>>()
    );
}

#[test]
fn d_schema_1_0_keeps_missing_context_and_profile_as_null_never_false() {
    let db = TempDb::new("ingest-1-0");
    let mut ws = Workspace::create(db.path()).expect("create");
    let outcome = ws.ingest(&fixture("minimal-1.0.json")).expect("ingest 1.0");
    assert!(matches!(outcome, IngestOutcome::Ingested { .. }));
    let total = one_i64(&ws, "SELECT count(*) FROM receipts");
    assert!(total > 0);
    for col in [
        "requires_business_share",
        "investment_risk",
        "vat_check",
        "sensitive",
    ] {
        assert_eq!(
            one_i64(
                &ws,
                &format!("SELECT count(*) FROM receipts WHERE {col} IS NULL")
            ),
            total,
            "{col} must be NULL for schema 1.0, never false"
        );
    }
    for col in [
        "vat_registered",
        "bookkeeping_method",
        "default_payment_method",
        "sie_series",
    ] {
        assert_eq!(
            one_i64(
                &ws,
                &format!("SELECT count(*) FROM entity_context WHERE {col} IS NULL")
            ),
            1,
            "{col} must be NULL when the 1.0 snapshot has no accounting_profile"
        );
    }
}

#[test]
fn e_same_snapshot_twice_is_a_noop() {
    let db = TempDb::new("ingest-noop");
    let mut ws = ingest_testgarden(&db);
    let before = counts(&ws);
    let digest = ws.snapshot_sha256().expect("digest");
    let again = ws.ingest(&fixture(TESTGARDEN)).expect("second ingest");
    assert_eq!(
        again,
        IngestOutcome::AlreadyIngested {
            snapshot_sha256: digest
        }
    );
    assert_eq!(counts(&ws), before);
}

#[test]
fn f_another_snapshot_is_refused_and_existing_rows_untouched() {
    let db = TempDb::new("ingest-mismatch");
    let mut ws = ingest_testgarden(&db);
    let before = counts(&ws);
    let digest = ws.snapshot_sha256().expect("digest");
    let keys_before = strings(
        &ws,
        "SELECT source_key FROM receipts ORDER BY selector_position",
    );
    let err = ws
        .ingest(&fixture("minimal-1.1.json"))
        .expect_err("must refuse");
    assert!(
        matches!(err, WorkspaceError::SnapshotMismatch { .. }),
        "{err}"
    );
    assert!(err.to_string().contains("another snapshot"));
    assert_eq!(counts(&ws), before);
    assert_eq!(ws.snapshot_sha256().expect("digest"), digest);
    assert_eq!(
        strings(
            &ws,
            "SELECT source_key FROM receipts ORDER BY selector_position"
        ),
        keys_before
    );
}

#[test]
fn g_real_rollback_on_appender_constraint_error_leaves_zero_snapshot_rows() {
    let db = TempDb::new("ingest-rollback");
    // Same snapshot, but two receipts share a source_key: the typed parser
    // permits it (no uniqueness rule there), the UNIQUE column does not.
    let mut v: Value = serde_json::from_slice(&fixture(TESTGARDEN)).expect("json");
    let dup = v["receipts"][0]["source_key"].clone();
    v["receipts"][1]["source_key"] = dup;
    let mutated = serde_json::to_vec(&v).expect("json");
    parse_snapshot(&mutated).expect("typed parse still accepts the duplicate source_key");

    let mut ws = Workspace::create(db.path()).expect("create");
    let err = ws
        .ingest(&mutated)
        .expect_err("UNIQUE(source_key) must fail at append or flush");
    assert!(matches!(err, WorkspaceError::Duckdb(_)), "{err}");
    ws.close().expect("close");

    let ws = Workspace::open(db.path()).expect("reopen after rollback");
    assert_eq!(ws.row_count("workspace_meta").expect("meta"), 1);
    for table in SNAPSHOT_TABLES {
        assert_eq!(
            ws.row_count(table).expect("count"),
            0,
            "{table} must be empty after rollback"
        );
    }
    assert!(matches!(
        ws.snapshot_sha256(),
        Err(WorkspaceError::NotIngested)
    ));
}

#[test]
fn h_invalid_date_is_refused_before_any_row_is_written() {
    let db = TempDb::new("ingest-bad-date");
    let mut v: Value = serde_json::from_slice(&fixture(TESTGARDEN)).expect("json");
    v["receipts"][3]["date"] = Value::from("2026-13-45");
    let mutated = serde_json::to_vec(&v).expect("json");
    parse_snapshot(&mutated).expect("typed parser keeps date as text");

    let mut ws = Workspace::create(db.path()).expect("create");
    let err = ws.ingest(&mutated).expect_err("must refuse");
    assert!(
        matches!(&err, WorkspaceError::InvalidDate { field, value } if field == "receipts[3].date" && value == "2026-13-45"),
        "{err}"
    );
    for table in SNAPSHOT_TABLES {
        assert_eq!(ws.row_count(table).expect("count"), 0, "{table}");
    }
    // A valid ingest afterwards still works — nothing was left half-written.
    assert!(matches!(
        ws.ingest(&fixture(TESTGARDEN)),
        Ok(IngestOutcome::Ingested { .. })
    ));
}

// ---------------------------------------------------------------------------
// Slice 4 — readback: typed rows, explicit ORDER BY, no JSON after ingest
// ---------------------------------------------------------------------------

fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("fixture date")
}

#[test]
fn readback_equals_the_typed_snapshot_field_for_field_in_contract_order() {
    let db = TempDb::new("readback-testgarden");
    let raw = fixture(TESTGARDEN);
    let snap = parse_snapshot(&raw).expect("typed parse");
    ingest_testgarden(&db).close().expect("close");
    let ws = Workspace::open(db.path()).expect("reopen");

    let meta = ws.read_meta().expect("meta");
    assert_eq!(
        meta.snapshot_sha256,
        ws.snapshot_digest(&raw).expect("expected digest")
    );
    assert_eq!(meta.schema_version, snap.schema_version);
    assert_eq!(meta.owner_id, snap.entity.owner_id);
    assert_eq!(meta.income_year, i32::from(snap.income_year));
    assert_eq!(meta.generated_at, snap.generated_at);
    assert_eq!(meta.all_properties_locked, snap.lock.all_properties_locked);
    assert_eq!(meta.declaration_year, i32::from(snap.lock.declaration_year));
    assert_eq!(
        (meta.source_app.as_str(), meta.source_environment.as_str()),
        (snap.source.app.as_str(), snap.source.environment.as_str())
    );

    let entity = ws.read_entity().expect("entity");
    let profile = snap
        .entity
        .accounting_profile
        .as_ref()
        .expect("1.1 profile");
    assert_eq!(entity.owner_id, snap.entity.owner_id);
    assert_eq!(entity.display_name, snap.entity.display_name);
    assert_eq!(entity.org_number, snap.entity.org_number);
    assert_eq!(entity.county, snap.entity.county);
    assert_eq!(entity.taxonomy_version, snap.entity.taxonomy_version);
    assert_eq!(
        entity.vat_registered.as_deref(),
        Some(profile.vat_registered.as_str())
    );
    assert_eq!(
        entity.bookkeeping_method.as_deref(),
        Some(profile.bookkeeping_method.as_str())
    );
    assert_eq!(
        entity.default_payment_method.as_deref(),
        Some(profile.default_payment_method.as_str())
    );
    assert_eq!(
        entity.sie_series.as_deref(),
        Some(profile.sie_series.as_str())
    );
    let mut ops = snap.entity.operation.clone();
    ops.sort();
    assert_eq!(entity.operations, ops);

    let properties = ws.read_properties().expect("properties");
    let mut expected_props: Vec<_> = snap.properties.iter().collect();
    expected_props.sort_by_key(|p| p.id);
    assert_eq!(properties.len(), expected_props.len());
    for (row, p) in properties.iter().zip(expected_props) {
        assert_eq!(
            (
                row.property_id,
                row.name.as_str(),
                row.slug.as_str(),
                row.is_default
            ),
            (p.id, p.name.as_str(), p.slug.as_str(), p.is_default)
        );
        assert_eq!(
            (row.tax_year_id, row.tax_year_status.as_str()),
            (p.tax_year.id, p.tax_year.status.as_str())
        );
        assert_eq!(row.locked_at, p.tax_year.locked_at);
    }

    let receipts = ws.read_receipts().expect("receipts");
    assert_eq!(receipts.len(), snap.receipts.len());
    for (i, (row, r)) in receipts.iter().zip(&snap.receipts).enumerate() {
        assert_eq!(
            row.selector_position, i as i32,
            "vector order is the contract order"
        );
        assert_eq!(row.id, r.id);
        assert_eq!(row.source_key, r.source_key);
        assert_eq!(row.property_id, r.property_id);
        assert_eq!(row.ordinal_number, r.ordinal_number.map(|o| o as i32));
        assert_eq!(row.vendor, r.vendor);
        assert_eq!(row.date, date(&r.date));
        assert_eq!(row.category, r.category);
        assert_eq!(
            (row.entry_type.as_str(), row.area.as_str()),
            (r.entry_type.as_str(), r.area.as_str())
        );
        let ctx = r.category_context.as_ref().expect("1.1 context");
        assert_eq!(
            (
                row.requires_business_share,
                row.investment_risk,
                row.vat_check,
                row.sensitive
            ),
            (
                Some(ctx.requires_business_share),
                Some(ctx.investment_risk),
                Some(ctx.vat_check),
                Some(ctx.sensitive)
            )
        );
        assert_eq!(
            (row.total, row.vat, row.rounding, row.net),
            (
                r.total_amount,
                r.vat_amount,
                r.rounding_amount,
                r.net_amount
            )
        );
        assert_eq!(row.payment_method, r.payment_method);
        assert_eq!(row.note, r.note);
        assert_eq!(row.has_image, r.has_image);
        assert_eq!(row.confirmed_at, r.confirmed_at);
    }
    let big = receipts
        .iter()
        .find(|r| r.category.as_deref() == Some("Större inköp"))
        .expect("row");
    assert_eq!((big.total, big.vat), (Ore(6_000_000), Ore(1_200_000)));

    let incomes = ws.read_income_entries().expect("incomes");
    assert_eq!(incomes.len(), snap.income_entries.len());
    for (i, (row, e)) in incomes.iter().zip(&snap.income_entries).enumerate() {
        assert_eq!(row.snapshot_position, i as i32);
        assert_eq!((row.id, row.property_id), (e.id, e.property_id));
        assert_eq!(row.source_key, e.source_key);
        assert_eq!(row.income_type, e.income_type);
        assert_eq!(row.date, date(&e.date));
        assert_eq!(row.buyer_name, e.buyer_name);
        assert_eq!(row.description, e.description);
        assert_eq!(
            (row.ex_vat, row.vat, row.inc_vat),
            (e.amount_ex_vat, e.vat_amount, e.amount_inc_vat)
        );
        assert_eq!(row.invoice_number, e.invoice_number);
        assert_eq!(row.payment_date, e.payment_date.as_deref().map(date));
        assert_eq!(row.document_count, e.document_count as i32);
    }
    assert!(ws.read_audit_chain().expect("audit").is_empty());
}

#[test]
fn readback_is_identical_before_and_after_reopen_and_across_workspaces() {
    let a = TempDb::new("readback-a");
    let b = TempDb::new("readback-b");
    let ws_a = ingest_testgarden(&a);
    let before = (
        ws_a.read_receipts().expect("r"),
        ws_a.read_income_entries().expect("i"),
        ws_a.read_properties().expect("p"),
        ws_a.read_entity().expect("e"),
        ws_a.read_meta().expect("m"),
    );
    let fp_a = ws_a.logical_fingerprint().expect("fp");
    ws_a.close().expect("close");
    let ws_a = Workspace::open(a.path()).expect("reopen");
    let after = (
        ws_a.read_receipts().expect("r"),
        ws_a.read_income_entries().expect("i"),
        ws_a.read_properties().expect("p"),
        ws_a.read_entity().expect("e"),
        ws_a.read_meta().expect("m"),
    );
    assert_eq!(before, after);
    assert_eq!(ws_a.logical_fingerprint().expect("fp"), fp_a);

    // A second file from the same snapshot is logically equivalent — bytes may differ.
    let ws_b = ingest_testgarden(&b);
    assert_eq!(ws_b.logical_fingerprint().expect("fp"), fp_a);
    assert_eq!(ws_b.read_receipts().expect("r"), after.0);
    assert_eq!(fp_a.len(), 64);

    // Another snapshot ⇒ another fingerprint.
    let c = TempDb::new("readback-c");
    let mut ws_c = Workspace::create(c.path()).expect("create");
    ws_c.ingest(&fixture("minimal-1.1.json")).expect("ingest");
    assert_ne!(ws_c.logical_fingerprint().expect("fp"), fp_a);
}

#[test]
fn empty_workspace_refuses_every_read_with_not_ingested() {
    let db = TempDb::new("readback-empty");
    let ws = Workspace::create(db.path()).expect("create");
    assert!(matches!(ws.read_meta(), Err(WorkspaceError::NotIngested)));
    assert!(matches!(ws.read_entity(), Err(WorkspaceError::NotIngested)));
    assert!(matches!(
        ws.read_properties(),
        Err(WorkspaceError::NotIngested)
    ));
    assert!(matches!(
        ws.read_receipts(),
        Err(WorkspaceError::NotIngested)
    ));
    assert!(matches!(
        ws.read_income_entries(),
        Err(WorkspaceError::NotIngested)
    ));
    assert!(matches!(
        ws.read_audit_chain(),
        Err(WorkspaceError::NotIngested)
    ));
    assert!(matches!(
        ws.logical_fingerprint(),
        Err(WorkspaceError::NotIngested)
    ));
}

#[test]
fn schema_1_0_readback_keeps_missing_context_and_profile_as_none() {
    let db = TempDb::new("readback-1-0");
    let mut ws = Workspace::create(db.path()).expect("create");
    ws.ingest(&fixture("minimal-1.0.json")).expect("ingest");
    let entity = ws.read_entity().expect("entity");
    assert_eq!(
        (
            entity.vat_registered,
            entity.bookkeeping_method,
            entity.default_payment_method,
            entity.sie_series
        ),
        (None, None, None, None)
    );
    for r in ws.read_receipts().expect("receipts") {
        assert_eq!(
            (
                r.requires_business_share,
                r.investment_risk,
                r.vat_check,
                r.sensitive
            ),
            (None, None, None, None)
        );
    }
}

#[test]
fn audit_chain_readback_keeps_document_nulls_and_event_fields() {
    let db = TempDb::new("readback-audit");
    let mut ws = Workspace::create(db.path()).expect("create");
    ws.ingest(&fixture("document-nulls-1.1.json"))
        .expect("ingest");
    let chain = ws.read_audit_chain().expect("audit");
    assert_eq!(
        chain.iter().map(|a| a.seq).collect::<Vec<_>>(),
        vec![0, 1, 2]
    );
    assert_eq!(chain[0].kind, "event");
    assert_eq!(
        chain[0].event_type.as_deref(),
        Some("submitted_to_accountant")
    );
    assert_eq!(
        chain[0].occurred_at.as_deref(),
        Some("2027-01-15T09:00:00+01:00")
    );
    assert_eq!(
        (
            chain[0].document_type.as_deref(),
            chain[0].received_date,
            chain[0].storage_backend.as_deref()
        ),
        (None, None, None)
    );
    assert_eq!(chain[1].kind, "document");
    assert_eq!(chain[1].received_date, Some(date("2027-02-20")));
    assert_eq!(
        chain[1].checksum_sha256.as_deref(),
        Some("0000000000000000000000000000000000000000000000000000000000000000")
    );
    assert_eq!(chain[1].storage_backend.as_deref(), Some("b2"));
    assert_eq!(
        chain[2].document_type.as_deref(),
        Some("supporting_document")
    );
    assert_eq!(
        (chain[2].received_date, chain[2].checksum_sha256.as_deref()),
        (None, None)
    );
    assert_eq!(chain[2].storage_backend.as_deref(), Some("cloudinary"));
    assert!(chain.iter().all(|a| a.property_id == 7));
}

// ---------------------------------------------------------------------------
// SV-03 Slice 2 — schema v2, persist_run, readback, digest, verification
// ---------------------------------------------------------------------------

const SYNTHETIC_ROOT: &str = "fixtures/masterdata/synthetic/generated";

fn masterdata() -> sieverk::ruleset::Masterdata {
    load_masterdata(&std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(SYNTHETIC_ROOT))
        .expect("synthetic masterdata")
}

fn sql(ws: &Workspace, statement: &str) {
    ws.connection().execute_batch(statement).expect(statement);
}

fn count(ws: &Workspace, table: &str) -> i64 {
    ws.connection()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))
        .expect(table)
}

fn v1_tables() -> Vec<String> {
    let mut v: Vec<String> = SCHEMA_V1_TABLES.iter().map(|s| s.to_string()).collect();
    v.sort();
    v
}

/// Ingest Testgården and run the engine; returns the open workspace + evidence.
fn run_testgarden(db: &TempDb) -> (Workspace, sieverk::engine::RunEvidence) {
    let mut ws = ingest_testgarden(db);
    let ev = run_engine(&mut ws, &masterdata()).expect("run_engine");
    (ws, ev)
}

/// Snapshot of everything schema-v1 that Slice 2 must leave untouched.
fn v1_state(
    ws: &Workspace,
) -> (
    String,
    Vec<(String, i64)>,
    Vec<sieverk::workspace::ReceiptRow>,
) {
    (
        ws.logical_fingerprint().expect("fp"),
        counts(ws),
        ws.read_receipts().expect("receipts"),
    )
}

// S1 — v1 → v2 preserves every v1 row and the v1 fingerprint
#[test]
fn s1_run_preserves_schema_v1_rows_and_fingerprint() {
    let db = TempDb::new("s1");
    let mut ws = ingest_testgarden(&db);
    let before = v1_state(&ws);
    let meta_before = ws.read_meta().expect("meta");
    let entity_before = ws.read_entity().expect("entity");
    assert_eq!(ws.schema_version().expect("schema"), WORKSPACE_SCHEMA);
    let ev = run_engine(&mut ws, &masterdata()).expect("run");
    assert_eq!(
        ws.schema_version().expect("schema"),
        WORKSPACE_SCHEMA_DECISIONS
    );
    assert_eq!(v1_state(&ws), before);
    assert_eq!(ws.read_meta().expect("meta"), meta_before);
    assert_eq!(ws.read_entity().expect("entity"), entity_before);
    // Slice 3: Bränsle + Skogsvård carry three lines each; everything else zero.
    assert_eq!((ev.cases, ev.lines), (27, 6));
    let mut expected: Vec<String> = v1_tables();
    expected.extend(SCHEMA_V2_TABLES.iter().map(|s| s.to_string()));
    expected.sort();
    assert_eq!(ws.table_names().expect("tables"), expected);
}

// S2 — persist + close + reopen + readback + verify
#[test]
fn s2_persisted_run_survives_reopen_and_verifies() {
    let db = TempDb::new("s2");
    let (ws, ev) = run_testgarden(&db);
    ws.close().expect("close");
    let ws = Workspace::open(db.path()).expect("reopen schema 2");
    assert_eq!(
        ws.engine_state().expect("state"),
        EngineState::Run(ev.run.clone())
    );
    assert_eq!(ws.read_run_meta().expect("meta"), ev.run);
    let cases = ws.read_cases().expect("cases");
    assert_eq!(cases.len(), 27);
    for (i, c) in cases.iter().enumerate() {
        assert_eq!(c.case_seq, i as i32);
    }
    let findings = ws.read_findings().expect("findings");
    assert!(findings
        .windows(2)
        .all(|w| (w[0].case_seq, w[0].finding_no) < (w[1].case_seq, w[1].finding_no)));
    assert_eq!(ws.read_decision_lines().expect("lines").len(), 6);
    assert_eq!(ws.verify_run().expect("verify"), ev.run);
}

// S3 — a schema-2 (Slice-2) evidence file stays zero-line forever: any line ⇒ Corrupt
#[test]
fn s3_schema2_evidence_rejects_any_decision_line() {
    let db = TempDb::new("s3");
    let (ws, ev) = run_testgarden(&db);
    // Reconstruct a Slice-2 evidence file from this run: schema 2, zero lines,
    // digest recomputed over the zero-line content (what the Slice-2 build wrote).
    sql(&ws, "DELETE FROM decision_lines");
    let cases = ws.read_cases().expect("cases");
    let findings = ws.read_findings().expect("findings");
    let d2 = ws
        .decision_digest(&ev.run.provenance, &cases, &findings, &[])
        .expect("digest");
    sql(
        &ws,
        &format!("UPDATE engine_run_meta SET decision_sha256 = '{d2}'"),
    );
    sql(&ws, "UPDATE workspace_meta SET workspace_schema = 2");
    ws.close().expect("close");
    let ws = Workspace::open(db.path()).expect("schema-2 file opens");
    assert_eq!(
        ws.schema_version().expect("schema"),
        WORKSPACE_SCHEMA_ENGINE
    );
    assert!(ws.read_decision_lines().expect("lines").is_empty());
    assert_eq!(count(&ws, "decision_lines"), 0);
    ws.verify_run()
        .expect("zero-line schema-2 evidence verifies");
    sql(
        &ws,
        "INSERT INTO decision_lines VALUES (0, 0, '5360', 'expense', 1000, 0)",
    );
    let e = ws
        .verify_run()
        .expect_err("line must be refused in a schema-2 run");
    assert!(
        matches!(&e, WorkspaceError::Corrupt(m) if m.contains("decision_lines")),
        "{e}"
    );
    // ingest() on schema 2 stays digest-only.
    let mut ws = ws;
    assert!(matches!(
        ws.ingest(&fixture(TESTGARDEN)),
        Ok(IngestOutcome::AlreadyIngested { .. })
    ));
}

// S4 — determinism across two independent workspaces; row mutation ⇒ digest mismatch
#[test]
fn s4_two_workspaces_same_rows_and_digest_and_mutation_breaks_digest() {
    let a = TempDb::new("s4a");
    let b = TempDb::new("s4b");
    let (wa, ea) = run_testgarden(&a);
    let (wb, eb) = run_testgarden(&b);
    assert_eq!(ea, eb);
    assert_eq!(wa.read_cases().expect("a"), wb.read_cases().expect("b"));
    assert_eq!(
        wa.read_findings().expect("a"),
        wb.read_findings().expect("b")
    );
    assert_eq!(ea.run.decision_sha256.len(), 64);
    sql(&wb, "UPDATE findings SET message = message || ' (edited)' WHERE case_seq = 0 AND finding_no = 0");
    let e = wb.verify_run().expect_err("edited row");
    assert!(
        matches!(&e, WorkspaceError::Corrupt(m) if m.contains("digest")),
        "{e}"
    );
}

// S5 — REAL rollback (H5 erratum): valid rows, provenance.ruleset_status = "bogus"
#[test]
fn s5_rollback_via_engine_run_meta_check_leaves_exact_schema_v1_state() {
    // Source of valid persisted rows: a completed run in another file.
    let donor = TempDb::new("s5-donor");
    let (dws, dev) = run_testgarden(&donor);
    let cases = dws.read_cases().expect("cases");
    let findings = dws.read_findings().expect("findings");
    let lines = dws.read_decision_lines().expect("lines");
    assert_eq!(lines.len(), 6, "the donor run carries lines");

    let db = TempDb::new("s5");
    let mut ws = ingest_testgarden(&db);
    let before = v1_state(&ws);
    let mut bogus = dev.run.provenance.clone();
    bogus.ruleset_status = "bogus".to_string();
    let e = ws
        .persist_run(&bogus, &cases, &findings, &lines)
        .expect_err("CHECK on engine_run_meta must fail");
    assert!(matches!(e, WorkspaceError::Duckdb(_)), "{e}");
    assert_eq!(ws.schema_version().expect("schema"), WORKSPACE_SCHEMA);
    assert_eq!(
        ws.table_names().expect("tables"),
        v1_tables(),
        "no v2 table survives the rollback"
    );
    assert_eq!(v1_state(&ws), before);
    assert!(matches!(ws.engine_state(), Ok(EngineState::NotRun)));
    ws.close().expect("close");
    let mut ws = Workspace::open(db.path()).expect("reopen as schema 1");
    assert_eq!(ws.schema_version().expect("schema"), WORKSPACE_SCHEMA);
    // The same workspace can still perform a valid run afterwards.
    let ev = run_engine(&mut ws, &masterdata()).expect("valid run after rollback");
    assert_eq!(ev, dev);
}

// S6 — AlreadyRun: run_engine and direct persist_run
#[test]
fn s6_second_run_is_already_run_before_mutation() {
    let db = TempDb::new("s6");
    let (mut ws, ev) = run_testgarden(&db);
    let rows_before = (ws.read_cases().expect("c"), ws.read_findings().expect("f"));
    let e = run_engine(&mut ws, &masterdata()).expect_err("rerun");
    assert!(
        matches!(&e, sieverk::engine::EngineError::Workspace(WorkspaceError::AlreadyRun { decision_sha256 }) if *decision_sha256 == ev.run.decision_sha256),
        "{e}"
    );
    let cases = rows_before.0.clone();
    let findings = rows_before.1.clone();
    let e = ws
        .persist_run(&ev.run.provenance, &cases, &findings, &[])
        .expect_err("direct rerun");
    assert!(matches!(e, WorkspaceError::AlreadyRun { .. }), "{e}");
    assert_eq!(
        (ws.read_cases().expect("c"), ws.read_findings().expect("f")),
        rows_before
    );
    assert_eq!(ws.verify_run().expect("still valid"), ev.run);
}

// S7 — schema 2 with zero runs: NotRun for reads, Corrupt for runs; schema 1 reads NotRun; ingest guard
#[test]
fn s7_schema2_without_run_is_notrun_for_reads_corrupt_for_runs_and_ingest_refuses() {
    let db = TempDb::new("s7");
    let (ws, ev) = run_testgarden(&db);
    sql(&ws, "DELETE FROM engine_run_meta");
    ws.close().expect("close");
    let mut ws = Workspace::open(db.path()).expect("recovery state opens");
    assert_eq!(ws.engine_state().expect("state"), EngineState::NotRun);
    assert!(matches!(ws.read_run_meta(), Err(WorkspaceError::NotRun)));
    assert!(matches!(ws.read_cases(), Err(WorkspaceError::NotRun)));
    assert!(matches!(ws.read_findings(), Err(WorkspaceError::NotRun)));
    assert!(matches!(
        ws.read_decision_lines(),
        Err(WorkspaceError::NotRun)
    ));
    assert!(matches!(ws.verify_run(), Err(WorkspaceError::NotRun)));
    let e = run_engine(&mut ws, &masterdata()).expect_err("run on recovery container");
    assert!(
        matches!(
            e,
            sieverk::engine::EngineError::Workspace(WorkspaceError::Corrupt(_))
        ),
        "{e}"
    );
    let cases = Vec::<PersistedCase>::new();
    let e = ws
        .persist_run(&ev.run.provenance, &cases, &[], &[])
        .expect_err("persist on recovery container");
    assert!(matches!(e, WorkspaceError::Corrupt(_)), "{e}");
    // ingest guard on schema 2 with one snapshot: digest comparison only, no write.
    let fp = ws.logical_fingerprint().expect("fp");
    let same = ws.ingest(&fixture(TESTGARDEN)).expect("same digest");
    assert!(matches!(same, IngestOutcome::AlreadyIngested { .. }));
    let e = ws
        .ingest(&fixture("minimal-1.1.json"))
        .expect_err("other snapshot");
    assert!(matches!(e, WorkspaceError::SnapshotMismatch { .. }), "{e}");
    assert_eq!(ws.logical_fingerprint().expect("fp"), fp);
    // schema 1 reads are NotRun.
    let db1 = TempDb::new("s7-v1");
    let ws1 = ingest_testgarden(&db1);
    assert!(matches!(ws1.read_cases(), Err(WorkspaceError::NotRun)));
    assert_eq!(ws1.engine_state().expect("state"), EngineState::NotRun);
}

// S7b — schema 2 + zero run + zero snapshot: ingest refuses without mutation (addendum §3)
#[test]
fn s7b_schema2_zero_run_zero_snapshot_ingest_refuses_without_mutation() {
    let db = TempDb::new("s7b");
    let (ws, _) = run_testgarden(&db);
    sql(&ws, "DELETE FROM engine_run_meta; DELETE FROM accounting_cases; DELETE FROM findings; \
              DELETE FROM snapshot_meta; DELETE FROM entity_context; DELETE FROM entity_operations; \
              DELETE FROM properties; DELETE FROM receipts; DELETE FROM income_entries; DELETE FROM audit_chain");
    ws.close().expect("close");
    let mut ws = Workspace::open(db.path()).expect("empty run-schema container opens");
    assert_eq!(
        ws.schema_version().expect("schema"),
        WORKSPACE_SCHEMA_DECISIONS
    );
    assert_eq!(ws.snapshot_count().expect("count"), 0);
    let e = ws.ingest(&fixture(TESTGARDEN)).expect_err("must refuse");
    assert!(matches!(e, WorkspaceError::Corrupt(_)), "{e}");
    assert_eq!(ws.snapshot_count().expect("count"), 0);
    for table in SCHEMA_V1_TABLES.iter().filter(|t| **t != "workspace_meta") {
        assert_eq!(ws.row_count(table).expect("count"), 0, "{table}");
    }
    assert_eq!(
        ws.schema_version().expect("schema"),
        WORKSPACE_SCHEMA_DECISIONS
    );
}

// S8 — >1 run rows (malformed table) and run row without snapshot ⇒ Corrupt on open
#[test]
fn s8_malformed_run_table_and_missing_snapshot_are_corrupt_on_open() {
    let db = TempDb::new("s8");
    let (ws, ev) = run_testgarden(&db);
    let p = &ev.run.provenance;
    let d = &ev.run.decision_sha256;
    sql(&ws, "DROP TABLE engine_run_meta");
    sql(&ws, "CREATE TABLE engine_run_meta (run_seq INTEGER, snapshot_sha256 TEXT, engine_version TEXT, chart_id TEXT, \
              chart_version TEXT, ruleset_version TEXT, ruleset_status TEXT, taxonomy_version TEXT, workbook_sha256 TEXT, \
              generator_version TEXT, decision_sha256 TEXT)");
    for seq in [0, 1] {
        sql(&ws, &format!(
            "INSERT INTO engine_run_meta VALUES ({seq}, '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{d}')",
            p.snapshot_sha256, p.engine_version, p.chart_id, p.chart_version, p.ruleset_version,
            p.ruleset_status, p.taxonomy_version, p.workbook_sha256, p.generator_version
        ));
    }
    ws.close().expect("close");
    let e = Workspace::open(db.path()).expect_err("two run rows");
    assert!(
        matches!(&e, WorkspaceError::Corrupt(m) if m.contains("more than one run row")),
        "{e}"
    );

    let db2 = TempDb::new("s8-nosnap");
    let (ws2, _) = run_testgarden(&db2);
    sql(&ws2, "DELETE FROM snapshot_meta");
    ws2.close().expect("close");
    let e = Workspace::open(db2.path()).expect_err("run without snapshot");
    assert!(
        matches!(&e, WorkspaceError::Corrupt(m) if m.contains("snapshot_meta")),
        "{e}"
    );
}

// S8b — one malformed run row with run_seq = 1 ⇒ Corrupt on open (blocker 1)
#[test]
fn s8b_single_run_row_with_wrong_run_seq_is_corrupt_on_open() {
    let db = TempDb::new("s8b");
    let (ws, ev) = run_testgarden(&db);
    let p = &ev.run.provenance;
    let d = &ev.run.decision_sha256;
    sql(&ws, "DROP TABLE engine_run_meta");
    sql(&ws, "CREATE TABLE engine_run_meta (run_seq INTEGER, snapshot_sha256 TEXT, engine_version TEXT, chart_id TEXT, \
              chart_version TEXT, ruleset_version TEXT, ruleset_status TEXT, taxonomy_version TEXT, workbook_sha256 TEXT, \
              generator_version TEXT, decision_sha256 TEXT)");
    sql(&ws, &format!(
        "INSERT INTO engine_run_meta VALUES (1, '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{}', '{d}')",
        p.snapshot_sha256, p.engine_version, p.chart_id, p.chart_version, p.ruleset_version,
        p.ruleset_status, p.taxonomy_version, p.workbook_sha256, p.generator_version
    ));
    ws.close().expect("close");
    let e = Workspace::open(db.path()).expect_err("run_seq 1 must be refused");
    assert!(
        matches!(&e, WorkspaceError::Corrupt(m) if m.contains("run_seq")),
        "{e}"
    );
}

// S9b — ingest() on an already-open workspace with an unsupported schema (blocker 2)
#[test]
fn s9b_ingest_refuses_unsupported_schema_before_any_mutation() {
    let db = TempDb::new("s9b");
    let mut ws = Workspace::create(db.path()).expect("create");
    sql(&ws, "UPDATE workspace_meta SET workspace_schema = 4");
    let before = counts(&ws);
    assert_eq!(ws.snapshot_count().expect("count"), 0);
    let e = ws
        .ingest(&fixture(TESTGARDEN))
        .expect_err("unsupported schema");
    assert!(
        matches!(
            e,
            WorkspaceError::SchemaMismatch {
                found: 4,
                expected: WORKSPACE_SCHEMA_DECISIONS
            }
        ),
        "{e}"
    );
    assert_eq!(ws.snapshot_count().expect("count"), 0);
    assert_eq!(counts(&ws), before);
    assert_eq!(ws.table_names().expect("tables"), v1_tables());
}

// S7c — run_engine on both schema-2 recovery forms ⇒ Corrupt (blocker 3)
#[test]
fn s7c_run_engine_on_schema2_recovery_states_is_corrupt() {
    // zero run + one snapshot
    let db = TempDb::new("s7c-one");
    let (ws, _) = run_testgarden(&db);
    sql(&ws, "DELETE FROM engine_run_meta");
    ws.close().expect("close");
    let mut ws = Workspace::open(db.path()).expect("recovery opens");
    assert_eq!(ws.engine_state().expect("state"), EngineState::NotRun);
    let e = run_engine(&mut ws, &masterdata()).expect_err("must not run");
    assert!(
        matches!(
            e,
            sieverk::engine::EngineError::Workspace(WorkspaceError::Corrupt(_))
        ),
        "{e}"
    );
    assert_eq!(ws.engine_state().expect("state"), EngineState::NotRun);
    assert!(matches!(ws.read_cases(), Err(WorkspaceError::NotRun)));
    // zero run + zero snapshot
    let db2 = TempDb::new("s7c-zero");
    let (ws2, _) = run_testgarden(&db2);
    sql(&ws2, "DELETE FROM engine_run_meta; DELETE FROM accounting_cases; DELETE FROM findings; \
               DELETE FROM snapshot_meta; DELETE FROM entity_context; DELETE FROM entity_operations; \
               DELETE FROM properties; DELETE FROM receipts; DELETE FROM income_entries; DELETE FROM audit_chain");
    ws2.close().expect("close");
    let mut ws2 = Workspace::open(db2.path()).expect("empty recovery opens");
    assert_eq!(ws2.engine_state().expect("state"), EngineState::NotRun);
    let e = run_engine(&mut ws2, &masterdata()).expect_err("must not run");
    assert!(
        matches!(
            e,
            sieverk::engine::EngineError::Workspace(WorkspaceError::Corrupt(_))
        ),
        "{e}"
    );
    assert_eq!(ws2.snapshot_count().expect("count"), 0);
    assert!(matches!(ws2.read_cases(), Err(WorkspaceError::NotRun)));
}

// S9 — schema 3 / 0 ⇒ SchemaMismatch { found, expected: 2 }
#[test]
fn s9_unsupported_schema_versions_are_mismatch() {
    for (name, v) in [("s9-4", 4), ("s9-0", 0)] {
        let db = TempDb::new(name);
        Workspace::create(db.path())
            .expect("create")
            .close()
            .expect("close");
        {
            let raw = Connection::open(db.path()).expect("raw");
            raw.execute_batch(&format!("UPDATE workspace_meta SET workspace_schema = {v}"))
                .expect("update");
            if let Err((_, e)) = raw.close() {
                panic!("raw close: {e}");
            }
        }
        let e = Workspace::open(db.path()).expect_err("unsupported");
        assert!(
            matches!(e, WorkspaceError::SchemaMismatch { found, expected: WORKSPACE_SCHEMA_DECISIONS } if found == v),
            "{e}"
        );
    }
}

// S10 — provenance: exact values, exact 11 columns, no path/time/count; mutation ⇒ Corrupt
#[test]
fn s10_provenance_is_content_identity_only_and_bound_by_the_digest() {
    let db = TempDb::new("s10");
    let (ws, ev) = run_testgarden(&db);
    let md = masterdata();
    let p = &ev.run.provenance;
    assert_eq!(p.snapshot_sha256, ws.snapshot_sha256().expect("sha"));
    assert_eq!(p.engine_version, env!("CARGO_PKG_VERSION"));
    assert_eq!(
        (p.chart_id.as_str(), p.chart_version.as_str()),
        (md.chart.chart_id.as_str(), md.chart.version.as_str())
    );
    assert_eq!(
        (p.ruleset_version.as_str(), p.ruleset_status.as_str()),
        (
            md.ruleset.ruleset_version.as_str(),
            md.ruleset.review_status.as_str()
        )
    );
    assert_eq!(p.taxonomy_version, md.ruleset.taxonomy_version);
    assert_eq!(p.workbook_sha256, md.chart.header.workbook_sha256);
    assert_eq!(p.generator_version, md.chart.header.generator_version);
    let columns = strings(
        &ws,
        "SELECT column_name FROM information_schema.columns WHERE table_schema = 'main' AND table_name = 'engine_run_meta' ORDER BY ordinal_position",
    );
    assert_eq!(
        columns,
        ENGINE_RUN_META_COLUMNS
            .iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
    );
    for forbidden in [
        "run_at",
        "masterdata_root",
        "path",
        "case_count",
        "finding_count",
    ] {
        assert!(!columns.iter().any(|c| c == forbidden), "{forbidden}");
    }
    sql(&ws, "UPDATE engine_run_meta SET ruleset_version = '9.9'");
    let e = ws.verify_run().expect_err("provenance mutation");
    assert!(
        matches!(&e, WorkspaceError::Corrupt(m) if m.contains("digest")),
        "{e}"
    );
}

// S11 — Testgården counts and structural references persisted
#[test]
fn s11_testgarden_persisted_counts_references_and_types() {
    let db = TempDb::new("s11");
    let (ws, _) = run_testgarden(&db);
    let cases = ws.read_cases().expect("cases");
    let findings = ws.read_findings().expect("findings");
    assert_eq!(cases.iter().filter(|c| c.source == "receipt").count(), 21);
    assert_eq!(cases.iter().filter(|c| c.source == "income").count(), 6);
    let unmapped = |seq: i32| {
        findings
            .iter()
            .any(|f| f.case_seq == seq && f.code == "UNMAPPED_CATEGORY")
    };
    assert_eq!(
        cases
            .iter()
            .filter(|c| c.source == "receipt" && unmapped(c.case_seq))
            .count(),
        17
    );
    assert_eq!(
        cases
            .iter()
            .filter(|c| c.source == "income" && unmapped(c.case_seq))
            .count(),
        3
    );
    let annat = cases
        .iter()
        .find(|c| c.subject.as_deref() == Some("Annat / osäkert"))
        .expect("row");
    assert_eq!(annat.rule_case_id.as_deref(), Some("annat_osakert.default"));
    assert!(unmapped(annat.case_seq));
    assert_eq!(annat.status, "Manual");
    let unknown = cases
        .iter()
        .find(|c| c.payment_method.as_deref() == Some("unknown"))
        .expect("row");
    assert_eq!(
        (
            unknown.counter_source.as_deref(),
            unknown.counter_key.as_deref(),
            unknown.counter_bookkeeping_method.as_deref()
        ),
        (Some("receipt"), Some("unknown"), None)
    );
    assert_eq!(unknown.status, "Manual");
    assert!(cases
        .iter()
        .filter(|c| c.source == "income")
        .all(|c| c.counter_source.is_none()
            && c.counter_key.is_none()
            && c.counter_bookkeeping_method.is_none()));
    assert!(findings
        .iter()
        .all(|f| f.code != "UNRESOLVED_VAT" && f.code != "VAT_MISMATCH"));
    assert!(cases
        .iter()
        .filter(|c| c.source == "receipt")
        .all(|c| c.has_image == Some(false)));
    assert!(cases
        .iter()
        .filter(|c| c.source == "income")
        .all(|c| c.document_count == Some(0)));
    assert!(cases
        .iter()
        .filter(|c| c.source == "income")
        .all(|c| c.requires_business_share.is_none() && c.subject == c.income_type));
    assert!(cases.iter().filter(|c| c.source == "receipt").all(|c| c
        .requires_business_share
        .is_some()
        && c.entry_type.as_deref() == Some("expense")
        && c.area.is_some()));
    assert_eq!(count(&ws, "decision_lines"), 6);
    assert_eq!(
        one_string(&ws, "SELECT typeof(date) FROM accounting_cases LIMIT 1"),
        "DATE"
    );
    assert_eq!(
        one_string(
            &ws,
            "SELECT typeof(payment_date) FROM accounting_cases WHERE source = 'income' LIMIT 1"
        ),
        "DATE"
    );
    assert_eq!(
        one_string(
            &ws,
            "SELECT typeof(total_ore) FROM accounting_cases WHERE source = 'receipt' LIMIT 1"
        ),
        "BIGINT"
    );
    assert_eq!(
        one_string(
            &ws,
            "SELECT typeof(has_image) FROM accounting_cases WHERE source = 'receipt' LIMIT 1"
        ),
        "BOOLEAN"
    );
    assert_eq!(one_i64(&ws, "SELECT count(*) FROM findings WHERE code = 'MISSING_EVIDENCE' AND severity = 'warning'"), 27);
}

// S12 — schema 1.0: derived canonical keys persisted and verified
#[test]
fn s12_schema_1_0_run_persists_derived_keys_and_verifies() {
    let db = TempDb::new("s12");
    let mut ws = Workspace::create(db.path()).expect("create");
    ws.ingest(&fixture("minimal-1.0.json")).expect("ingest 1.0");
    let ev = run_engine(&mut ws, &masterdata()).expect("run");
    let cases = ws.read_cases().expect("cases");
    assert_eq!(cases.len(), ev.cases);
    for c in &cases {
        assert_eq!(c.source_key, format!("{}:{}", c.source, c.row_id));
        assert!(
            c.requires_business_share.is_none()
                && c.investment_risk.is_none()
                && c.vat_check.is_none()
                && c.sensitive.is_none()
        );
    }
    assert!(cases
        .iter()
        .filter(|c| c.source == "receipt")
        .all(|c| c.payment_method.is_none() && c.counter_source.is_none()));
    assert_eq!(ws.verify_run().expect("verify"), ev.run);
}

// S13 — lossless round-trip via public API only
#[test]
fn s13_public_api_round_trip_is_lossless() {
    let db = TempDb::new("s13");
    let mut ws = ingest_testgarden(&db);
    let md = masterdata();
    let entity = ws.read_entity().expect("entity");
    let engine_cases = project_cases(&ws).expect("project");
    let assessments = assess_cases(&engine_cases, &entity, &md).expect("assess");
    let decisions = decide_cases(&engine_cases, &assessments, &entity, &md).expect("decide");
    run_engine(&mut ws, &md).expect("run");
    let persisted = ws.read_cases().expect("cases");
    let findings = ws.read_findings().expect("findings");
    let plines = ws.read_decision_lines().expect("lines");
    assert_eq!(persisted.len(), engine_cases.len());
    for ((ec, a), pc) in engine_cases.iter().zip(&decisions).zip(&persisted) {
        assert_eq!(pc.case_seq, ec.case_seq);
        assert_eq!(
            pc.source,
            match ec.source {
                CaseSource::Receipt => "receipt",
                CaseSource::Income => "income",
            }
        );
        assert_eq!(
            (
                pc.source_key.as_str(),
                pc.row_id,
                pc.property_id,
                pc.ordinal_number,
                pc.date
            ),
            (
                ec.source_key.as_str(),
                ec.row_id,
                ec.property_id,
                ec.ordinal_number,
                ec.date
            )
        );
        assert_eq!(pc.subject, ec.subject);
        assert_eq!(
            (
                pc.requires_business_share,
                pc.investment_risk,
                pc.vat_check,
                pc.sensitive
            ),
            (
                ec.context.requires_business_share,
                ec.context.investment_risk,
                ec.context.vat_check,
                ec.context.sensitive
            )
        );
        match &ec.facts {
            SourceFacts::Receipt {
                total,
                vat,
                rounding,
                net,
                payment_method,
                entry_type,
                area,
                has_image,
            } => {
                assert_eq!(
                    (pc.total, pc.receipt_vat, pc.rounding, pc.net),
                    (Some(*total), Some(*vat), Some(*rounding), Some(*net))
                );
                assert_eq!(
                    (
                        &pc.payment_method,
                        pc.entry_type.as_deref(),
                        pc.area.as_deref(),
                        pc.has_image
                    ),
                    (
                        payment_method,
                        Some(entry_type.as_str()),
                        Some(area.as_str()),
                        Some(*has_image)
                    )
                );
                assert!(
                    pc.ex_vat.is_none()
                        && pc.income_type.is_none()
                        && pc.document_count.is_none()
                        && pc.payment_date.is_none()
                );
            }
            SourceFacts::Income {
                ex_vat,
                vat,
                inc_vat,
                payment_date,
                income_type,
                document_count,
            } => {
                assert_eq!(
                    (pc.ex_vat, pc.income_vat, pc.inc_vat, pc.payment_date),
                    (Some(*ex_vat), Some(*vat), Some(*inc_vat), *payment_date)
                );
                assert_eq!(
                    (pc.income_type.as_deref(), pc.document_count),
                    (Some(income_type.as_str()), Some(*document_count))
                );
                assert!(pc.total.is_none() && pc.entry_type.is_none() && pc.has_image.is_none());
            }
        }
        assert_eq!(
            (&pc.rule_case_id, &pc.vat_rule_id),
            (&a.rule_case_id, &a.vat_rule_id)
        );
        match &a.counter_rule {
            Some(r) => {
                assert_eq!(
                    pc.counter_source.as_deref(),
                    Some(match r.source {
                        CaseSource::Receipt => "receipt",
                        CaseSource::Income => "income",
                    })
                );
                assert_eq!(pc.counter_key.as_deref(), Some(r.key.as_str()));
                assert_eq!(pc.counter_bookkeeping_method, r.bookkeeping_method);
            }
            None => assert!(
                pc.counter_source.is_none()
                    && pc.counter_key.is_none()
                    && pc.counter_bookkeeping_method.is_none()
            ),
        }
        assert_eq!(pc.status, format!("{:?}", a.status));
        let mine_lines: Vec<&PersistedLine> = plines
            .iter()
            .filter(|l| l.case_seq == ec.case_seq)
            .collect();
        assert_eq!(mine_lines.len(), a.lines.len());
        for (pl, l) in mine_lines.iter().zip(&a.lines) {
            assert_eq!(
                (pl.line_no, pl.account.as_str(), pl.role.as_str()),
                (l.line_no, l.account.as_str(), l.role.as_str())
            );
            assert_eq!((pl.debit, pl.credit), (l.debit, l.credit));
        }
        let mine: Vec<&PersistedFinding> = findings
            .iter()
            .filter(|f| f.case_seq == ec.case_seq)
            .collect();
        assert_eq!(mine.len(), a.findings.len());
        for (pf, f) in mine.iter().zip(&a.findings) {
            assert_eq!(
                (pf.finding_no, pf.code.as_str()),
                (f.finding_no, f.code.as_str())
            );
            assert_eq!(pf.severity, format!("{:?}", f.severity).to_lowercase());
            assert_eq!((&pf.message, &pf.question), (&f.message, &f.question));
        }
    }
}

// S14 — canonical digest vector: exact literal + DuckDB hash + sensitivity
#[test]
fn s14_canonical_decision_text_matches_the_locked_vector() {
    let db = TempDb::new("s14");
    let ws = Workspace::create(db.path()).expect("create");
    let prov = RunProvenance {
        snapshot_sha256: "snap-sha".to_string(),
        engine_version: "9.9.9".to_string(),
        chart_id: "CHART".to_string(),
        chart_version: "2026.1".to_string(),
        ruleset_version: "2026.1".to_string(),
        ruleset_status: "draft".to_string(),
        taxonomy_version: "1.0".to_string(),
        workbook_sha256: "wb-sha".to_string(),
        generator_version: "gen/1".to_string(),
    };
    let case = PersistedCase {
        case_seq: 0,
        source: "receipt".to_string(),
        source_key: "receipt:7".to_string(),
        row_id: 7,
        property_id: 1,
        ordinal_number: Some(3),
        date: date("2026-08-20"),
        subject: Some("Grus \"och\" \\ material\nrad".to_string()),
        requires_business_share: None,
        investment_risk: Some(true),
        vat_check: None,
        sensitive: Some(false),
        total: Some(Ore(125_000)),
        receipt_vat: Some(Ore(25_000)),
        rounding: Some(Ore(-20)),
        net: Some(Ore(100_020)),
        payment_method: Some("unknown".to_string()),
        entry_type: Some("expense".to_string()),
        area: Some("ovrigt".to_string()),
        has_image: Some(false),
        ex_vat: None,
        income_vat: None,
        inc_vat: None,
        payment_date: None,
        income_type: None,
        document_count: None,
        rule_case_id: Some("grus_och_material.default".to_string()),
        vat_rule_id: Some("ing25".to_string()),
        counter_source: Some("receipt".to_string()),
        counter_key: Some("unknown".to_string()),
        counter_bookkeeping_method: None,
        status: "Manual".to_string(),
    };
    let finding = PersistedFinding {
        case_seq: 0,
        finding_no: 0,
        code: "UNRESOLVED_COUNTER_ACCOUNT".to_string(),
        severity: "blocking".to_string(),
        message: "åäö \"q\" \\ end".to_string(),
        question: None,
    };
    let text = canonical_decision_text(
        &prov,
        std::slice::from_ref(&case),
        std::slice::from_ref(&finding),
        &[],
    );
    let expected: &str = "sieverk-decision/1\nrun\ti:0\ts:\"snap-sha\"\ts:\"9.9.9\"\ts:\"CHART\"\ts:\"2026.1\"\ts:\"2026.1\"\ts:\"draft\"\ts:\"1.0\"\ts:\"wb-sha\"\ts:\"gen/1\"\ncase\ti:0\ts:\"receipt\"\ts:\"receipt:7\"\ti:7\ti:1\ti:3\td:2026-08-20\ts:\"Grus \\\"och\\\" \\\\ material\\nrad\"\t~\tb:1\t~\tb:0\ti:125000\ti:25000\ti:-20\ti:100020\ts:\"unknown\"\ts:\"expense\"\ts:\"ovrigt\"\tb:0\t~\t~\t~\t~\t~\t~\ts:\"grus_och_material.default\"\ts:\"ing25\"\ts:\"receipt\"\ts:\"unknown\"\t~\ts:\"Manual\"\nfinding\ti:0\ti:0\ts:\"UNRESOLVED_COUNTER_ACCOUNT\"\ts:\"blocking\"\ts:\"åäö \\\"q\\\" \\\\ end\"\t~\nend\ti:1\ti:1\ti:0\n";
    assert_eq!(String::from_utf8(text.clone()).expect("utf8"), expected);
    let digest = ws
        .decision_digest(
            &prov,
            std::slice::from_ref(&case),
            std::slice::from_ref(&finding),
            &[],
        )
        .expect("digest");
    assert_eq!(
        digest,
        "36dd4b0277f06ed642dbdc8b220a304afbe2bb9cec67ce3cfea7c89f051888fc"
    );
    assert_eq!(digest, ws.sha256_hex(&text).expect("hash"));
    let mut other_prov = prov.clone();
    other_prov.ruleset_version = "2026.2".to_string();
    assert_ne!(
        ws.decision_digest(
            &other_prov,
            std::slice::from_ref(&case),
            std::slice::from_ref(&finding),
            &[]
        )
        .expect("d"),
        digest
    );
    let mut other_case = case.clone();
    other_case.net = Some(Ore(100_021));
    assert_ne!(
        ws.decision_digest(
            &prov,
            std::slice::from_ref(&other_case),
            std::slice::from_ref(&finding),
            &[]
        )
        .expect("d"),
        digest
    );
    let line = PersistedLine {
        case_seq: 0,
        line_no: 0,
        account: "5360".to_string(),
        role: "expense".to_string(),
        debit: Ore(1),
        credit: Ore(0),
    };
    assert_ne!(
        ws.decision_digest(
            &prov,
            std::slice::from_ref(&case),
            std::slice::from_ref(&finding),
            std::slice::from_ref(&line)
        )
        .expect("d"),
        digest
    );
}

// S15 — source-mirror mutations after a run are refused before the digest step
#[test]
fn s15_source_mirror_mutations_are_corrupt_before_digest() {
    for (name, statement) in [
        ("s15-date", "UPDATE accounting_cases SET date = DATE '2000-01-01' WHERE case_seq = 0"),
        ("s15-net", "UPDATE accounting_cases SET net_ore = net_ore + 1 WHERE case_seq = 0"),
        ("s15-entry", "UPDATE accounting_cases SET entry_type = 'income' WHERE case_seq = 0"),
        ("s15-income", "UPDATE accounting_cases SET income_type = 'grot', subject = 'grot' WHERE case_seq = 21"),
    ] {
        let db = TempDb::new(name);
        let (ws, _) = run_testgarden(&db);
        sql(&ws, statement);
        let e = ws.verify_run().expect_err(statement);
        assert!(matches!(&e, WorkspaceError::Corrupt(m) if m.contains("mirror")), "{name}: {e}");
    }
    // The same guard runs on persist_run input: a hand-altered case is refused before any mutation.
    let donor = TempDb::new("s15-donor");
    let (dws, dev) = run_testgarden(&donor);
    let mut cases = dws.read_cases().expect("cases");
    let findings = dws.read_findings().expect("findings");
    cases[0].net = cases[0].net.map(|o| Ore(o.0 + 1));
    let db = TempDb::new("s15-preflight");
    let mut ws = ingest_testgarden(&db);
    let e = ws
        .persist_run(&dev.run.provenance, &cases, &findings, &[])
        .expect_err("altered net");
    assert!(
        matches!(&e, WorkspaceError::Corrupt(m) if m.contains("mirror")),
        "{e}"
    );
    assert_eq!(ws.schema_version().expect("schema"), WORKSPACE_SCHEMA);
    assert_eq!(ws.table_names().expect("tables"), v1_tables());
}

// S16 — persist_run input guards (addendum §2 + Rev-2 §E.2) all refuse before mutation
#[test]
fn s16_persist_run_preflight_refuses_every_structural_violation() {
    let donor = TempDb::new("s16-donor");
    let (dws, dev) = run_testgarden(&donor);
    let good_cases = dws.read_cases().expect("cases");
    let good_findings = dws.read_findings().expect("findings");
    let prov = dev.run.provenance.clone();
    // A blocking finding exists on case 0 (Testgården receipt 0 is unmapped) — locate one warning-only case (Bränsle).
    let bransle = good_cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row") as i32;
    let unknown = good_cases
        .iter()
        .position(|c| c.payment_method.as_deref() == Some("unknown"))
        .expect("row") as i32;

    type Mutation =
        Box<dyn Fn(&mut Vec<PersistedCase>, &mut Vec<PersistedFinding>, &mut RunProvenance)>;
    let good_lines = dws.read_decision_lines().expect("lines");
    fn m(
        f: impl Fn(&mut Vec<PersistedCase>, &mut Vec<PersistedFinding>, &mut RunProvenance) + 'static,
    ) -> Mutation {
        Box::new(f)
    }
    let scenarios: Vec<(&str, Mutation)> = vec![
        ("non-dense case_seq", m(|c, _, _| c[5].case_seq = 99)),
        ("findings out of order", m(|_, f, _| f.swap(0, 1))),
        ("orphan finding", m(|_, f, _| f[0].case_seq = 999)),
        (
            "Automatic under draft",
            m(move |c, _, _| c[bransle as usize].status = "Automatic".to_string()),
        ),
        (
            "wrong provenance snapshot",
            m(|_, _, p| p.snapshot_sha256 = "0".repeat(64)),
        ),
        (
            "income counter reference",
            m(|c, _, _| {
                let i = c.iter().position(|x| x.source == "income").expect("income");
                c[i].counter_source = Some("income".to_string());
                c[i].counter_key = Some("betald".to_string());
            }),
        ),
        (
            "VAT finding on a non-Manual case",
            m(move |_, f, _| {
                f.retain(|x| x.case_seq != bransle);
                f.push(PersistedFinding {
                    case_seq: bransle,
                    finding_no: 0,
                    code: "UNRESOLVED_VAT".to_string(),
                    severity: "blocking".to_string(),
                    message: "x".to_string(),
                    question: None,
                });
                f.sort_by_key(|x| (x.case_seq, x.finding_no));
            }),
        ),
        (
            "wrong severity",
            m(|_, f, _| {
                let i = f
                    .iter()
                    .position(|x| x.code == "MISSING_EVIDENCE")
                    .expect("me");
                f[i].severity = "blocking".to_string();
            }),
        ),
        (
            "rank order violated",
            m(move |_, f, _| {
                // unknown receipt: [COUNTER, UNMAPPED, MISSING] → swap the first two codes keeping numbering dense
                let i = f
                    .iter()
                    .position(|x| x.case_seq == unknown && x.finding_no == 0)
                    .expect("f0");
                let code0 = f[i].code.clone();
                let sev0 = f[i].severity.clone();
                f[i].code = f[i + 1].code.clone();
                f[i].severity = f[i + 1].severity.clone();
                f[i + 1].code = code0;
                f[i + 1].severity = sev0;
            }),
        ),
        (
            "duplicate finding code",
            m(move |_, f, _| {
                let i = f
                    .iter()
                    .position(|x| x.case_seq == unknown && x.finding_no == 1)
                    .expect("f1");
                f[i].code = "UNRESOLVED_COUNTER_ACCOUNT".to_string();
                f[i].severity = "blocking".to_string();
            }),
        ),
        (
            "Blocking finding with non-Manual status",
            m(move |c, _, _| c[unknown as usize].status = "Conditional".to_string()),
        ),
        (
            "vat_rule_id without rule_case_id",
            m(move |c, _, _| c[bransle as usize].rule_case_id = None),
        ),
        (
            "counter key != payment_method",
            m(move |c, _, _| c[bransle as usize].counter_key = Some("private".to_string())),
        ),
        (
            "partial counter reference",
            m(move |c, _, _| c[bransle as usize].counter_key = None),
        ),
        (
            "counter without payment_method",
            m(move |c, _, _| c[bransle as usize].payment_method = None),
        ),
        (
            "subject != income_type",
            m(|c, _, _| {
                let i = c.iter().position(|x| x.source == "income").expect("income");
                c[i].subject = Some("other".to_string());
            }),
        ),
    ];
    for (name, mutate) in scenarios {
        let db = TempDb::new(&format!("s16-{}", name.replace(' ', "-")));
        let mut ws = ingest_testgarden(&db);
        let fp = ws.logical_fingerprint().expect("fp");
        let (mut c, mut f, mut p) = (good_cases.clone(), good_findings.clone(), prov.clone());
        mutate(&mut c, &mut f, &mut p);
        let e = ws.persist_run(&p, &c, &f, &good_lines).expect_err(name);
        assert!(matches!(e, WorkspaceError::Corrupt(_)), "{name}: {e}");
        assert_eq!(
            ws.schema_version().expect("schema"),
            WORKSPACE_SCHEMA,
            "{name}"
        );
        assert_eq!(ws.table_names().expect("tables"), v1_tables(), "{name}");
        assert_eq!(ws.logical_fingerprint().expect("fp"), fp, "{name}");
    }
}

// S17 — post-run raw mutations of the structural contract are Corrupt before digest acceptance
#[test]
fn s17_post_run_structural_mutations_are_corrupt() {
    let scenarios = [
        ("s17-vat-code", "UPDATE findings SET code = 'UNRESOLVED_VAT' WHERE case_seq = 0 AND finding_no = 0", "digest"),
        ("s17-severity", "UPDATE findings SET severity = 'blocking' WHERE code = 'MISSING_EVIDENCE' AND case_seq = 1", "must have severity"),
        ("s17-dup", "UPDATE findings SET code = 'MISSING_EVIDENCE', severity = 'warning' WHERE case_seq = 0 AND finding_no = 0", "duplicate code"),
        ("s17-status", "UPDATE accounting_cases SET status = 'Conditional' WHERE case_seq = 0", "blocking finding but status"),
        ("s17-vatref", "UPDATE accounting_cases SET rule_case_id = NULL WHERE subject = 'Bränsle'", "vat_rule_id without rule_case_id"),
        ("s17-counterkey", "UPDATE accounting_cases SET counter_key = 'private' WHERE subject = 'Bränsle'", "counter_key"),
    ];
    for (name, statement, reason) in scenarios {
        let db = TempDb::new(name);
        let (ws, _) = run_testgarden(&db);
        sql(&ws, statement);
        let e = ws.verify_run().expect_err(statement);
        assert!(
            matches!(&e, WorkspaceError::Corrupt(m) if m.contains(reason)),
            "{name}: {e}"
        );
    }
}

// ---------------------------------------------------------------------------
// SV-03 Slice 3 — schema 3 line persistence and verification (FINAL lock §8.2)
// ---------------------------------------------------------------------------

// T1 — schema 1 → 3 with the Testgården lines; readback shape; reopen; ingest digest-only
#[test]
fn t1_schema3_run_persists_three_lines_per_eligible_receipt() {
    let db = TempDb::new("t1");
    let (ws, ev) = run_testgarden(&db);
    assert_eq!(
        ws.schema_version().expect("schema"),
        WORKSPACE_SCHEMA_DECISIONS
    );
    assert_eq!(ev.lines, 6);
    ws.close().expect("close");
    let mut ws = Workspace::open(db.path()).expect("reopen schema 3");
    let lines = ws.read_decision_lines().expect("lines");
    let cases = ws.read_cases().expect("cases");
    assert_eq!(lines.len(), 6);
    let bransle = cases
        .iter()
        .find(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row");
    let mine: Vec<_> = lines
        .iter()
        .filter(|l| l.case_seq == bransle.case_seq)
        .collect();
    assert_eq!(
        mine.iter()
            .map(|l| (
                l.line_no,
                l.account.as_str(),
                l.role.as_str(),
                l.debit.0,
                l.credit.0
            ))
            .collect::<Vec<_>>(),
        vec![
            (0, "5360", "expense", 500_000, 0),
            (1, "2640", "input_vat", 125_000, 0),
            (2, "1930", "counter", 0, 625_000)
        ]
    );
    assert_eq!(bransle.status, "Conditional");
    for c in &cases {
        let n = lines.iter().filter(|l| l.case_seq == c.case_seq).count();
        assert!(n == 0 || n == 3, "case {}: {n} lines", c.case_seq);
        if c.status == "Manual" {
            assert_eq!(n, 0);
        }
    }
    assert_eq!(
        one_string(&ws, "SELECT typeof(debit_ore) FROM decision_lines LIMIT 1"),
        "BIGINT"
    );
    assert_eq!(ws.verify_run().expect("verify"), ev.run);
    // ingest() on schema 3 is digest-only.
    let fp = ws.logical_fingerprint().expect("fp");
    assert!(matches!(
        ws.ingest(&fixture(TESTGARDEN)),
        Ok(IngestOutcome::AlreadyIngested { .. })
    ));
    assert!(matches!(
        ws.ingest(&fixture("minimal-1.1.json")),
        Err(WorkspaceError::SnapshotMismatch { .. })
    ));
    assert_eq!(ws.logical_fingerprint().expect("fp"), fp);
    assert_eq!(count(&ws, "decision_lines"), 6);
}

// T2 — every schema-3 line corruption is Corrupt (or refused by the DDL) after reopen
#[test]
fn t2_schema3_line_corruptions_are_corrupt() {
    // Case 1 = Bränsle (three lines); case 0 = Röjning (Manual, unmapped, zero lines).
    let scenarios = [
        ("t2-orphan", "INSERT INTO decision_lines VALUES (999, 0, '5360', 'expense', 1, 0)", "orphan"),
        ("t2-gap", "UPDATE decision_lines SET line_no = 7 WHERE case_seq = 1 AND line_no = 2", "dense"),
        ("t2-unbalanced", "UPDATE decision_lines SET debit_ore = debit_ore + 1 WHERE case_seq = 1 AND line_no = 0", "balance"),
        ("t2-manual", "INSERT INTO decision_lines VALUES (0, 0, '5360', 'expense', 1, 0)", "Manual case carries lines"),
        ("t2-shape", "UPDATE decision_lines SET account = '536' WHERE case_seq = 1 AND line_no = 0", "four digits"),
        ("t2-automatic-nolines", "UPDATE accounting_cases SET status = 'Automatic' WHERE case_seq = 0", "Automatic"),
    ];
    for (name, statement, reason) in scenarios {
        let db = TempDb::new(name);
        let (ws, _) = run_testgarden(&db);
        sql(&ws, statement);
        let e = ws.verify_run().expect_err(statement);
        assert!(
            matches!(&e, WorkspaceError::Corrupt(m) if m.contains(reason)),
            "{name}: {e}"
        );
    }
    // Neither side positive and an invalid role are refused by the DDL CHECKs themselves.
    let db = TempDb::new("t2-ddl");
    let (ws, _) = run_testgarden(&db);
    assert!(ws.connection().execute_batch("UPDATE decision_lines SET debit_ore = 0, credit_ore = 0 WHERE case_seq = 1 AND line_no = 0").is_err());
    assert!(
        ws.connection()
            .execute_batch(
                "UPDATE decision_lines SET credit_ore = 1 WHERE case_seq = 1 AND line_no = 0"
            )
            .is_err(),
        "both sides positive is refused by the DDL"
    );
    assert!(ws
        .connection()
        .execute_batch(
            "UPDATE decision_lines SET role = 'bogus' WHERE case_seq = 1 AND line_no = 0"
        )
        .is_err());
    assert!(ws
        .connection()
        .execute_batch(
            "UPDATE decision_lines SET debit_ore = -1 WHERE case_seq = 1 AND line_no = 0"
        )
        .is_err());
    ws.verify_run().expect("untouched run still verifies");
}

// T3 — persist_run refuses line-level violations before mutation (schema stays 1)
#[test]
fn t3_persist_run_line_preflight() {
    let donor = TempDb::new("t3-donor");
    let (dws, dev) = run_testgarden(&donor);
    let good_cases = dws.read_cases().expect("cases");
    let good_findings = dws.read_findings().expect("findings");
    let good_lines = dws.read_decision_lines().expect("lines");
    let bransle = good_cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row") as i32;
    type LineMutation = Box<dyn Fn(&mut Vec<PersistedCase>, &mut Vec<PersistedLine>)>;
    fn m(f: impl Fn(&mut Vec<PersistedCase>, &mut Vec<PersistedLine>) + 'static) -> LineMutation {
        Box::new(f)
    }
    let scenarios: Vec<(&str, LineMutation)> = vec![
        ("orphan line", m(|_, l| l[0].case_seq = 999)),
        ("line under Manual", m(|_, l| l[0].case_seq = 0)),
        ("unbalanced", m(|_, l| l[0].debit = Ore(l[0].debit.0 + 1))),
        ("both sides", m(|_, l| l[0].credit = Ore(1))),
        ("non-dense line_no", m(|_, l| l[2].line_no = 5)),
        (
            "bad account shape",
            m(|_, l| l[0].account = "53600".to_string()),
        ),
        ("unknown role", m(|_, l| l[0].role = "bogus".to_string())),
        (
            "Automatic without lines",
            m(move |c, _| c[0].status = "Automatic".to_string()),
        ),
        (
            "lines on a Manual status",
            m(move |c, _| c[bransle as usize].status = "Manual".to_string()),
        ),
    ];
    for (name, mutate) in scenarios {
        let db = TempDb::new(&format!("t3-{}", name.replace(' ', "-")));
        let mut ws = ingest_testgarden(&db);
        let fp = ws.logical_fingerprint().expect("fp");
        let (mut c, mut l) = (good_cases.clone(), good_lines.clone());
        mutate(&mut c, &mut l);
        let e = ws
            .persist_run(&dev.run.provenance, &c, &good_findings, &l)
            .expect_err(name);
        assert!(matches!(e, WorkspaceError::Corrupt(_)), "{name}: {e}");
        assert_eq!(
            ws.schema_version().expect("schema"),
            WORKSPACE_SCHEMA,
            "{name}"
        );
        assert_eq!(ws.table_names().expect("tables"), v1_tables(), "{name}");
        assert_eq!(ws.logical_fingerprint().expect("fp"), fp, "{name}");
    }
    // The unmodified rows persist and verify on a fresh workspace.
    let db = TempDb::new("t3-good");
    let mut ws = ingest_testgarden(&db);
    let run = ws
        .persist_run(
            &dev.run.provenance,
            &good_cases,
            &good_findings,
            &good_lines,
        )
        .expect("valid");
    assert_eq!(run, dev.run);
    assert_eq!(ws.verify_run().expect("verify"), dev.run);
}

// T4 — digest with line records is deterministic and line-sensitive
#[test]
fn t4_digest_covers_lines() {
    let a = TempDb::new("t4a");
    let b = TempDb::new("t4b");
    let (wa, ea) = run_testgarden(&a);
    let (wb, eb) = run_testgarden(&b);
    assert_eq!(ea.run.decision_sha256, eb.run.decision_sha256);
    assert_eq!(
        wa.read_decision_lines().expect("a"),
        wb.read_decision_lines().expect("b")
    );
    sql(
        &wb,
        "UPDATE decision_lines SET account = '5361' WHERE case_seq = 1 AND line_no = 0",
    );
    let e = wb.verify_run().expect_err("changed line");
    assert!(
        matches!(&e, WorkspaceError::Corrupt(m) if m.contains("digest")),
        "{e}"
    );
}
