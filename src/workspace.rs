//! DuckDB engine workspace (SV-02D, docs/SV-02D-forspec.md rev 3.1).
//!
//! A real, file-backed DuckDB database derived from one immutable snapshot.
//! Django/PostgreSQL stays the source of truth; this file is reproducible
//! from the snapshot and is never synced back. Money is `BIGINT` öre
//! (`Ore(i64)`), dates are `DATE`, flags are `BOOLEAN` — never VARCHAR.
//!
//! Slices so far: the container contract (errors, `create`/`open` state
//! rules, schema v1) and the snapshot digest (exactly Django's
//! canonicalisation, hashed by DuckDB's own `sha256`). Ingestion and
//! readback follow. SV-03's tables (`engine_run_meta`, accounting cases,
//! decision lines, findings) are not created here.

use std::fmt;
use std::path::{Path, PathBuf};

use duckdb::{params, Connection};
use serde_json::Value;

/// Schema version stored in the file (`workspace_meta.workspace_schema`).
/// SV-03 bumps this to 2 when it adds its tables.
pub const WORKSPACE_SCHEMA: i32 = 1;

#[derive(Debug)]
pub enum WorkspaceError {
    /// `create()` on a path that already exists — an old evidence database
    /// is never silently reused or overwritten.
    AlreadyExists(PathBuf),
    /// `open()` on a path that does not exist.
    NotFound(PathBuf),
    /// The file's `workspace_schema` is not the one this binary supports.
    SchemaMismatch { found: i32, expected: i32 },
    /// The file violates the container contract (missing/duplicated
    /// `workspace_meta`, more than one `snapshot_meta` row, …).
    Corrupt(String),
    /// A read that needs an ingested snapshot was attempted on an empty workspace.
    NotIngested,
    /// Anything DuckDB itself refused.
    Duckdb(String),
}

impl fmt::Display for WorkspaceError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::AlreadyExists(p) => write!(
                f,
                "workspace {} already exists — refusing to reuse or overwrite it",
                p.display()
            ),
            Self::NotFound(p) => write!(f, "workspace {} does not exist", p.display()),
            Self::SchemaMismatch { found, expected } => write!(
                f,
                "workspace schema {found} is not supported (this build expects {expected})"
            ),
            Self::Corrupt(why) => write!(f, "workspace is corrupt: {why}"),
            Self::NotIngested => write!(f, "workspace has no ingested snapshot"),
            Self::Duckdb(e) => write!(f, "duckdb: {e}"),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<duckdb::Error> for WorkspaceError {
    fn from(e: duckdb::Error) -> Self {
        Self::Duckdb(e.to_string())
    }
}

/// Schema v1 — exactly the snapshot's content, typed. Column order and
/// nullability follow docs/SV-02D-forspec.md §2: nullable where schema 1.0
/// snapshots lack the value (never a guessed default), NOT NULL otherwise.
pub const SCHEMA_V1_DDL: &str = "
CREATE TABLE IF NOT EXISTS workspace_meta (
    workspace_schema    INTEGER NOT NULL,
    created_by_version  TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS snapshot_meta (
    snapshot_sha256        TEXT    PRIMARY KEY,
    schema_version         TEXT    NOT NULL,
    owner_id               BIGINT  NOT NULL,
    income_year            INTEGER NOT NULL,
    generated_at           TEXT    NOT NULL,
    all_properties_locked  BOOLEAN NOT NULL,
    declaration_year       INTEGER NOT NULL,
    source_app             TEXT    NOT NULL,
    source_environment     TEXT    NOT NULL
);
CREATE TABLE IF NOT EXISTS entity_context (
    owner_id                BIGINT PRIMARY KEY,
    display_name            TEXT   NOT NULL,
    org_number              TEXT,
    county                  TEXT,
    taxonomy_version        TEXT,
    vat_registered          TEXT,
    bookkeeping_method      TEXT,
    default_payment_method  TEXT,
    sie_series              TEXT
);
CREATE TABLE IF NOT EXISTS entity_operations (
    operation  TEXT PRIMARY KEY
);
CREATE TABLE IF NOT EXISTS properties (
    property_id      BIGINT  PRIMARY KEY,
    name             TEXT    NOT NULL,
    slug             TEXT    NOT NULL,
    is_default       BOOLEAN NOT NULL,
    tax_year_id      BIGINT  NOT NULL,
    tax_year_status  TEXT    NOT NULL,
    locked_at        TEXT
);
CREATE TABLE IF NOT EXISTS receipts (
    id                       BIGINT  PRIMARY KEY,
    source_key               TEXT    UNIQUE,
    property_id              BIGINT  NOT NULL,
    ordinal_number           INTEGER,
    vendor                   TEXT,
    date                     DATE    NOT NULL,
    category                 TEXT,
    entry_type               TEXT    NOT NULL,
    area                     TEXT    NOT NULL,
    requires_business_share  BOOLEAN,
    investment_risk          BOOLEAN,
    vat_check                BOOLEAN,
    sensitive                BOOLEAN,
    total_ore                BIGINT  NOT NULL,
    vat_ore                  BIGINT  NOT NULL,
    rounding_ore             BIGINT  NOT NULL,
    net_ore                  BIGINT  NOT NULL,
    payment_method           TEXT,
    note                     TEXT,
    has_image                BOOLEAN NOT NULL,
    confirmed_at             TEXT,
    selector_position        INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS income_entries (
    id                 BIGINT  PRIMARY KEY,
    source_key         TEXT    UNIQUE,
    property_id        BIGINT  NOT NULL,
    income_type        TEXT    NOT NULL,
    date               DATE    NOT NULL,
    buyer_name         TEXT,
    description        TEXT    NOT NULL,
    ex_vat_ore         BIGINT  NOT NULL,
    vat_ore            BIGINT  NOT NULL,
    inc_vat_ore        BIGINT  NOT NULL,
    invoice_number     TEXT,
    payment_date       DATE,
    document_count     INTEGER NOT NULL,
    snapshot_position  INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS audit_chain (
    seq              INTEGER PRIMARY KEY,
    kind             TEXT    NOT NULL,
    property_id      BIGINT  NOT NULL,
    event_type       TEXT,
    occurred_at      TEXT,
    document_type    TEXT,
    received_date    DATE,
    checksum_sha256  TEXT,
    storage_backend  TEXT
);
";

/// Every table schema v1 owns, in creation order.
pub const SCHEMA_V1_TABLES: [&str; 8] = [
    "workspace_meta",
    "snapshot_meta",
    "entity_context",
    "entity_operations",
    "properties",
    "receipts",
    "income_entries",
    "audit_chain",
];

/// A file-backed workspace. Holds the single connection; dropping it
/// closes the file (use `close()` to surface a close error explicitly).
pub struct Workspace {
    conn: Connection,
    path: PathBuf,
}

impl fmt::Debug for Workspace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Workspace")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl Workspace {
    /// Create a new workspace file with schema v1 and one `workspace_meta`
    /// row. Refuses an existing path (`AlreadyExists`): an old evidence
    /// database must never be reused or overwritten by accident.
    pub fn create(path: &Path) -> Result<Self, WorkspaceError> {
        if path.exists() {
            return Err(WorkspaceError::AlreadyExists(path.to_path_buf()));
        }
        let conn = Connection::open(path)?;
        let ws = Self {
            conn,
            path: path.to_path_buf(),
        };
        ws.ensure_schema()?;
        ws.conn.execute(
            "INSERT INTO workspace_meta (workspace_schema, created_by_version) VALUES (?, ?)",
            params![WORKSPACE_SCHEMA, env!("CARGO_PKG_VERSION")],
        )?;
        Ok(ws)
    }

    /// Open an existing workspace and verify the container contract:
    /// `workspace_meta` has exactly one row with a supported schema, and
    /// `snapshot_meta` has 0 or 1 rows (an empty workspace, or the result of
    /// a rolled-back ingest, is a valid state — more than one row is not).
    pub fn open(path: &Path) -> Result<Self, WorkspaceError> {
        if !path.exists() {
            return Err(WorkspaceError::NotFound(path.to_path_buf()));
        }
        let conn = Connection::open(path)?;
        let ws = Self {
            conn,
            path: path.to_path_buf(),
        };
        ws.verify_container()?;
        Ok(ws)
    }

    /// Idempotent DDL: safe to call on a file that already has the schema.
    pub fn ensure_schema(&self) -> Result<(), WorkspaceError> {
        self.conn.execute_batch(SCHEMA_V1_DDL)?;
        Ok(())
    }

    fn verify_container(&self) -> Result<(), WorkspaceError> {
        let meta_rows: i64 = self
            .conn
            .query_row("SELECT count(*) FROM workspace_meta", [], |r| r.get(0))
            .map_err(|e| WorkspaceError::Corrupt(format!("workspace_meta is unreadable ({e})")))?;
        if meta_rows != 1 {
            return Err(WorkspaceError::Corrupt(format!(
                "workspace_meta must have exactly one row, found {meta_rows}"
            )));
        }
        let found: i32 =
            self.conn
                .query_row("SELECT workspace_schema FROM workspace_meta", [], |r| {
                    r.get(0)
                })?;
        if found != WORKSPACE_SCHEMA {
            return Err(WorkspaceError::SchemaMismatch {
                found,
                expected: WORKSPACE_SCHEMA,
            });
        }
        let snapshots = self.snapshot_count()?;
        if snapshots > 1 {
            return Err(WorkspaceError::Corrupt(format!(
                "snapshot_meta must have 0 or 1 rows, found {snapshots}"
            )));
        }
        Ok(())
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Borrow the connection for read queries (readback, inspection).
    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    /// The schema version recorded in the file.
    pub fn schema_version(&self) -> Result<i32, WorkspaceError> {
        Ok(self
            .conn
            .query_row("SELECT workspace_schema FROM workspace_meta", [], |r| {
                r.get(0)
            })?)
    }

    /// The sieverk version that created the file.
    pub fn created_by_version(&self) -> Result<String, WorkspaceError> {
        Ok(self
            .conn
            .query_row("SELECT created_by_version FROM workspace_meta", [], |r| {
                r.get(0)
            })?)
    }

    /// Number of ingested snapshots — 0 or 1 in a valid workspace.
    pub fn snapshot_count(&self) -> Result<i64, WorkspaceError> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM snapshot_meta", [], |r| r.get(0))?)
    }

    /// The ingested snapshot's digest. Reads that need a snapshot require
    /// exactly one row: none is `NotIngested`, more than one is `Corrupt`.
    pub fn snapshot_sha256(&self) -> Result<String, WorkspaceError> {
        match self.snapshot_count()? {
            0 => Err(WorkspaceError::NotIngested),
            1 => Ok(self
                .conn
                .query_row("SELECT snapshot_sha256 FROM snapshot_meta", [], |r| {
                    r.get(0)
                })?),
            n => Err(WorkspaceError::Corrupt(format!(
                "snapshot_meta must have 0 or 1 rows, found {n}"
            ))),
        }
    }

    /// Every table in the file, sorted — for inspection and contract tests.
    pub fn table_names(&self) -> Result<Vec<String>, WorkspaceError> {
        let mut stmt = self.conn.prepare(
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = 'main' ORDER BY table_name",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut names = Vec::new();
        for name in rows {
            names.push(name?);
        }
        Ok(names)
    }

    /// The declared DuckDB type of one column — proof that money is BIGINT,
    /// dates are DATE and flags are BOOLEAN, never VARCHAR.
    pub fn column_type(&self, table: &str, column: &str) -> Result<String, WorkspaceError> {
        Ok(self.conn.query_row(
            "SELECT data_type FROM information_schema.columns \
             WHERE table_schema = 'main' AND table_name = ? AND column_name = ?",
            params![table, column],
            |r| r.get(0),
        )?)
    }

    /// Row count of one schema table (readback support).
    pub fn row_count(&self, table: &str) -> Result<i64, WorkspaceError> {
        if !SCHEMA_V1_TABLES.contains(&table) {
            return Err(WorkspaceError::Corrupt(format!(
                "{table} is not a schema v1 table"
            )));
        }
        Ok(self
            .conn
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |r| r.get(0))?)
    }

    /// SHA-256 of arbitrary bytes, computed by DuckDB itself
    /// (`SELECT sha256(?::BLOB)`) — no hand-rolled hash, no extra crate.
    pub fn sha256_hex(&self, bytes: &[u8]) -> Result<String, WorkspaceError> {
        Ok(self
            .conn
            .query_row("SELECT sha256(?::BLOB)", params![bytes.to_vec()], |r| {
                r.get(0)
            })?)
    }

    /// The snapshot's content digest, contract-identical to Django's
    /// `snapshot_sha256()`: canonical bytes (see `canonical_snapshot_bytes`)
    /// hashed by DuckDB. Proven against Django's own digests in
    /// fixtures/snapshots/django-digests.json.
    pub fn snapshot_digest(&self, raw_json: &[u8]) -> Result<String, WorkspaceError> {
        let canonical = canonical_snapshot_bytes(raw_json)?;
        self.sha256_hex(&canonical)
    }

    /// Close explicitly so a close failure is an error, not a silent drop.
    pub fn close(self) -> Result<(), WorkspaceError> {
        self.conn
            .close()
            .map_err(|(_, e)| WorkspaceError::Duckdb(e.to_string()))
    }
}

/// Canonical snapshot bytes — exactly what Django hashes:
/// top-level `generated_at` removed, object keys sorted, compact separators
/// (`,` and `:`), UTF-8 without ASCII escaping. Key order is enforced here
/// by sorting (UTF-8 byte order == code point order == Python `sort_keys`),
/// so it does not depend on serde_json's map implementation. Scalars and
/// strings are written by serde_json, whose escaping matches Python's
/// `ensure_ascii=False` output for the snapshot's content (`"`, `\\`, control
/// characters as `\uXXXX` or the short escapes). The Django golden test is
/// the proof; any divergence there is a STOP, not a fallback.
pub fn canonical_snapshot_bytes(raw_json: &[u8]) -> Result<Vec<u8>, WorkspaceError> {
    let mut value: Value = serde_json::from_slice(raw_json)
        .map_err(|e| WorkspaceError::Corrupt(format!("snapshot is not valid JSON: {e}")))?;
    let top = value
        .as_object_mut()
        .ok_or_else(|| WorkspaceError::Corrupt("snapshot must be a JSON object".to_string()))?;
    top.remove("generated_at");
    let mut out = Vec::with_capacity(raw_json.len());
    write_canonical(&value, &mut out)?;
    Ok(out)
}

fn write_canonical(value: &Value, out: &mut Vec<u8>) -> Result<(), WorkspaceError> {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push(b'{');
            for (i, key) in keys.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                out.extend(serde_json::to_vec(key).map_err(json_err)?);
                out.push(b':');
                write_canonical(&map[key.as_str()], out)?;
            }
            out.push(b'}');
        }
        Value::Array(items) => {
            out.push(b'[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(b',');
                }
                write_canonical(item, out)?;
            }
            out.push(b']');
        }
        scalar => out.extend(serde_json::to_vec(scalar).map_err(json_err)?),
    }
    Ok(())
}

fn json_err(e: serde_json::Error) -> WorkspaceError {
    WorkspaceError::Corrupt(format!("cannot serialise canonical JSON: {e}"))
}
