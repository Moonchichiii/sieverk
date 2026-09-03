//! workspace_real_db (SV-02D §4) — every test here opens a REAL DuckDB file
//! on disk. No mocks, no in-memory-only proof. Slice 1 covers the container
//! contract: create/open states, schema v1, typed columns, corruption.

use std::fs;
use std::path::PathBuf;

use duckdb::Connection;
use serde_json::Value;
use sieverk::snapshot::parse_snapshot;
use sieverk::workspace::{
    canonical_snapshot_bytes, IngestOutcome, Workspace, WorkspaceError, SCHEMA_V1_TABLES,
    WORKSPACE_SCHEMA,
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
                expected: WORKSPACE_SCHEMA
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
