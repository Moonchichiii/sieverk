//! workspace_real_db (SV-02D §4) — every test here opens a REAL DuckDB file
//! on disk. No mocks, no in-memory-only proof. Slice 1 covers the container
//! contract: create/open states, schema v1, typed columns, corruption.

use std::fs;
use std::path::PathBuf;

use duckdb::Connection;
use sieverk::workspace::{Workspace, WorkspaceError, SCHEMA_V1_TABLES, WORKSPACE_SCHEMA};

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
