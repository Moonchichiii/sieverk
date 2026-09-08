//! DuckDB engine workspace (SV-02D, docs/SV-02D-forspec.md rev 3.1).
//!
//! A real, file-backed DuckDB database derived from one immutable snapshot.
//! Django/PostgreSQL stays the source of truth; this file is reproducible
//! from the snapshot and is never synced back. Money is `BIGINT` öre
//! (`Ore(i64)`), dates are `DATE`, flags are `BOOLEAN` — never VARCHAR.
//!
//! Slices so far: the container contract (errors, `create`/`open` state
//! rules, schema v1), the snapshot digest (exactly Django's
//! canonicalisation, hashed by DuckDB's own `sha256`) and ingestion (one
//! transaction, one Appender per table, explicit flush, rollback on any
//! error) and readback (typed rows, contract-ordered queries use explicit
//! ORDER BY, nothing read from snapshot JSON after ingest). SV-03 Slice 2 adds
//! schema v2: `engine_run_meta`, `accounting_cases`, `findings` and
//! `decision_lines` (zero rows in Slice 2), persisted in one transaction with a
//! canonical `decision_sha256`, typed readback and fail-closed verification.

use std::collections::HashSet;
use std::fmt;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use duckdb::{params, Connection, Transaction};
use serde_json::Value;

use crate::money::Ore;
use crate::snapshot::{parse_snapshot, AuditEvent, Snapshot};

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
    /// The workspace already holds a different snapshot — never overwritten.
    SnapshotMismatch { existing: String, incoming: String },
    /// The typed snapshot parser (unchanged `snapshot.rs`) refused the input.
    Snapshot(String),
    /// A date field is not `YYYY-MM-DD`; DATE columns never get a VARCHAR fallback.
    InvalidDate { field: String, value: String },
    /// Anything DuckDB itself refused.
    Duckdb(String),
    /// An SV-03 read on a workspace that holds no engine run.
    NotRun,
    /// A second engine run on a workspace that already holds one — the
    /// workspace is one immutable evidence run; never replaced.
    AlreadyRun { decision_sha256: String },
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
            Self::SnapshotMismatch { existing, incoming } => write!(
                f,
                "workspace belongs to another snapshot ({existing}); refusing {incoming}"
            ),
            Self::Snapshot(e) => write!(f, "snapshot rejected: {e}"),
            Self::InvalidDate { field, value } => {
                write!(f, "{field}: {value:?} is not a YYYY-MM-DD date")
            }
            Self::Duckdb(e) => write!(f, "duckdb: {e}"),
            Self::NotRun => write!(f, "workspace holds no engine run"),
            Self::AlreadyRun { decision_sha256 } => write!(
                f,
                "workspace already holds an engine run ({decision_sha256}); refusing a second run"
            ),
        }
    }
}

impl std::error::Error for WorkspaceError {}

impl From<duckdb::Error> for WorkspaceError {
    fn from(e: duckdb::Error) -> Self {
        Self::Duckdb(e.to_string())
    }
}

/// Result of `Workspace::ingest`. The same snapshot twice is a no-op, not an
/// error; a different snapshot is `WorkspaceError::SnapshotMismatch`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IngestOutcome {
    Ingested {
        snapshot_sha256: String,
        properties: usize,
        receipts: usize,
        income_entries: usize,
        audit_items: usize,
    },
    AlreadyIngested {
        snapshot_sha256: String,
    },
}

// ---------------------------------------------------------------------------
// Readback rows — what SV-03/SV-04 consume. Read from DuckDB only; the
// snapshot JSON is never opened again after ingest.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMeta {
    pub snapshot_sha256: String,
    pub schema_version: String,
    pub owner_id: i64,
    pub income_year: i32,
    pub generated_at: String,
    pub all_properties_locked: bool,
    pub declaration_year: i32,
    pub source_app: String,
    pub source_environment: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntityContext {
    pub owner_id: i64,
    pub display_name: String,
    pub org_number: Option<String>,
    pub county: Option<String>,
    pub taxonomy_version: Option<String>,
    pub vat_registered: Option<String>,
    pub bookkeeping_method: Option<String>,
    pub default_payment_method: Option<String>,
    pub sie_series: Option<String>,
    /// `entity.operation` items, sorted (the set has no contractual order).
    pub operations: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PropertyRow {
    pub property_id: i64,
    pub name: String,
    pub slug: String,
    pub is_default: bool,
    pub tax_year_id: i64,
    pub tax_year_status: String,
    pub locked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReceiptRow {
    pub id: i64,
    pub source_key: Option<String>,
    pub property_id: i64,
    pub ordinal_number: Option<i32>,
    pub vendor: Option<String>,
    pub date: NaiveDate,
    pub category: Option<String>,
    pub entry_type: String,
    pub area: String,
    pub requires_business_share: Option<bool>,
    pub investment_risk: Option<bool>,
    pub vat_check: Option<bool>,
    pub sensitive: Option<bool>,
    pub total: Ore,
    pub vat: Ore,
    pub rounding: Ore,
    pub net: Ore,
    pub payment_method: Option<String>,
    pub note: Option<String>,
    pub has_image: bool,
    pub confirmed_at: Option<String>,
    pub selector_position: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomeRow {
    pub id: i64,
    pub source_key: Option<String>,
    pub property_id: i64,
    pub income_type: String,
    pub date: NaiveDate,
    pub buyer_name: Option<String>,
    pub description: String,
    pub ex_vat: Ore,
    pub vat: Ore,
    pub inc_vat: Ore,
    pub invoice_number: Option<String>,
    pub payment_date: Option<NaiveDate>,
    pub document_count: i32,
    pub snapshot_position: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AuditRow {
    pub seq: i32,
    pub kind: String,
    pub property_id: i64,
    pub event_type: Option<String>,
    pub occurred_at: Option<String>,
    pub document_type: Option<String>,
    pub received_date: Option<NaiveDate>,
    pub checksum_sha256: Option<String>,
    pub storage_backend: Option<String>,
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

/// Schema version of a workspace that holds a persisted engine run (SV-03).
pub const WORKSPACE_SCHEMA_ENGINE: i32 = 2;

/// Schema v2 — the four SV-03 tables (SV-03 Slice 2 design rev 2 + final
/// addendum). Money is BIGINT öre, dates DATE, flags BOOLEAN. Source facts are
/// duplicated from the v1 rows deliberately and verified against them on every
/// read (`verify_run`).
pub const SCHEMA_V2_DDL: &str = "
CREATE TABLE IF NOT EXISTS engine_run_meta (
    run_seq            INTEGER NOT NULL PRIMARY KEY CHECK (run_seq = 0),
    snapshot_sha256    TEXT    NOT NULL,
    engine_version     TEXT    NOT NULL,
    chart_id           TEXT    NOT NULL,
    chart_version      TEXT    NOT NULL,
    ruleset_version    TEXT    NOT NULL,
    ruleset_status     TEXT    NOT NULL CHECK (ruleset_status IN ('draft','approved')),
    taxonomy_version   TEXT    NOT NULL,
    workbook_sha256    TEXT    NOT NULL,
    generator_version  TEXT    NOT NULL,
    decision_sha256    TEXT    NOT NULL CHECK (length(decision_sha256) = 64)
);
CREATE TABLE IF NOT EXISTS accounting_cases (
    case_seq                   INTEGER NOT NULL PRIMARY KEY,
    source                     TEXT    NOT NULL CHECK (source IN ('receipt','income')),
    source_key                 TEXT    NOT NULL UNIQUE,
    row_id                     BIGINT  NOT NULL,
    property_id                BIGINT  NOT NULL,
    ordinal_number             INTEGER,
    date                       DATE    NOT NULL,
    subject                    TEXT,
    requires_business_share    BOOLEAN,
    investment_risk            BOOLEAN,
    vat_check                  BOOLEAN,
    sensitive                  BOOLEAN,
    total_ore                  BIGINT,
    receipt_vat_ore            BIGINT,
    rounding_ore               BIGINT,
    net_ore                    BIGINT,
    payment_method             TEXT,
    entry_type                 TEXT,
    area                       TEXT,
    has_image                  BOOLEAN,
    ex_vat_ore                 BIGINT,
    income_vat_ore             BIGINT,
    inc_vat_ore                BIGINT,
    payment_date               DATE,
    income_type                TEXT,
    document_count             INTEGER,
    rule_case_id               TEXT,
    vat_rule_id                TEXT,
    counter_source             TEXT    CHECK (counter_source IS NULL OR counter_source IN ('receipt','income')),
    counter_key                TEXT,
    counter_bookkeeping_method TEXT,
    status                     TEXT    NOT NULL CHECK (status IN ('Automatic','Conditional','Manual')),
    CHECK (
        (source = 'receipt'
            AND total_ore IS NOT NULL AND receipt_vat_ore IS NOT NULL AND rounding_ore IS NOT NULL AND net_ore IS NOT NULL
            AND entry_type IS NOT NULL AND area IS NOT NULL AND has_image IS NOT NULL
            AND ex_vat_ore IS NULL AND income_vat_ore IS NULL AND inc_vat_ore IS NULL AND payment_date IS NULL
            AND income_type IS NULL AND document_count IS NULL)
        OR
        (source = 'income'
            AND ex_vat_ore IS NOT NULL AND income_vat_ore IS NOT NULL AND inc_vat_ore IS NOT NULL
            AND income_type IS NOT NULL AND document_count IS NOT NULL
            AND subject IS NOT NULL AND subject = income_type
            AND ordinal_number IS NULL
            AND requires_business_share IS NULL AND investment_risk IS NULL AND vat_check IS NULL AND sensitive IS NULL
            AND total_ore IS NULL AND receipt_vat_ore IS NULL AND rounding_ore IS NULL AND net_ore IS NULL
            AND payment_method IS NULL AND entry_type IS NULL AND area IS NULL AND has_image IS NULL)
    ),
    CHECK (
        (counter_source IS NULL AND counter_key IS NULL AND counter_bookkeeping_method IS NULL)
        OR
        (counter_source IS NOT NULL AND counter_key IS NOT NULL AND counter_source = source)
    )
);
CREATE TABLE IF NOT EXISTS findings (
    case_seq    INTEGER NOT NULL,
    finding_no  INTEGER NOT NULL CHECK (finding_no >= 0),
    code        TEXT    NOT NULL CHECK (code IN ('UNRESOLVED_VAT','UNRESOLVED_COUNTER_ACCOUNT','UNMAPPED_CATEGORY',
                                                'LEGACY_INCOME_RECEIPT','UNSPECIFIED_TIMBER_SALE','VAT_MISMATCH','MISSING_EVIDENCE')),
    severity    TEXT    NOT NULL CHECK (severity IN ('info','warning','blocking')),
    message     TEXT    NOT NULL,
    question    TEXT,
    PRIMARY KEY (case_seq, finding_no)
);
CREATE TABLE IF NOT EXISTS decision_lines (
    case_seq    INTEGER NOT NULL,
    line_no     INTEGER NOT NULL CHECK (line_no >= 0),
    account     TEXT    NOT NULL,
    role        TEXT    NOT NULL CHECK (role IN ('expense','income','input_vat','output_vat','counter','rounding')),
    debit_ore   BIGINT  NOT NULL CHECK (debit_ore >= 0),
    credit_ore  BIGINT  NOT NULL CHECK (credit_ore >= 0),
    CHECK ((debit_ore > 0) <> (credit_ore > 0)),
    PRIMARY KEY (case_seq, line_no)
);
";

/// The four SV-03 tables, in creation order.
pub const SCHEMA_V2_TABLES: [&str; 4] = [
    "engine_run_meta",
    "accounting_cases",
    "findings",
    "decision_lines",
];

/// The `engine_run_meta` columns in DDL order (also the `run` digest record order).
pub const ENGINE_RUN_META_COLUMNS: [&str; 11] = [
    "run_seq",
    "snapshot_sha256",
    "engine_version",
    "chart_id",
    "chart_version",
    "ruleset_version",
    "ruleset_status",
    "taxonomy_version",
    "workbook_sha256",
    "generator_version",
    "decision_sha256",
];

/// The seven-code finding contract in fixed rank order. Slice 2 may persist
/// only the five structural codes; the two VAT codes exist for later slices.
pub const FINDING_CODE_RANK: [&str; 7] = [
    "UNRESOLVED_VAT",
    "UNRESOLVED_COUNTER_ACCOUNT",
    "UNMAPPED_CATEGORY",
    "LEGACY_INCOME_RECEIPT",
    "UNSPECIFIED_TIMBER_SALE",
    "VAT_MISMATCH",
    "MISSING_EVIDENCE",
];

/// Slice-2 `(code, severity)` contract — the only pairs a Slice-2 run may hold.
pub const SLICE2_FINDING_SEVERITY: [(&str, &str); 5] = [
    ("UNRESOLVED_COUNTER_ACCOUNT", "blocking"),
    ("UNMAPPED_CATEGORY", "blocking"),
    ("LEGACY_INCOME_RECEIPT", "blocking"),
    ("UNSPECIFIED_TIMBER_SALE", "blocking"),
    ("MISSING_EVIDENCE", "warning"),
];

/// The locked lifecycle as persisted text.
pub const STATUSES: [&str; 3] = ["Automatic", "Conditional", "Manual"];

/// Whether a workspace holds a persisted engine run.
#[derive(Debug, Clone, PartialEq, Eq)]
// Run(RunMeta) is design-locked; boxing it would change the public API.
#[allow(clippy::large_enum_variant)]
pub enum EngineState {
    NotRun,
    Run(RunMeta),
}

/// Content identity of a run — what `engine_run_meta` stores besides the
/// digest. No clock, no path, no counts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunProvenance {
    pub snapshot_sha256: String,
    pub engine_version: String,
    pub chart_id: String,
    pub chart_version: String,
    pub ruleset_version: String,
    pub ruleset_status: String,
    pub taxonomy_version: String,
    pub workbook_sha256: String,
    pub generator_version: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunMeta {
    pub provenance: RunProvenance,
    pub decision_sha256: String,
}

/// One `accounting_cases` row, fields in DDL column order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedCase {
    pub case_seq: i32,
    pub source: String,
    pub source_key: String,
    pub row_id: i64,
    pub property_id: i64,
    pub ordinal_number: Option<i32>,
    pub date: NaiveDate,
    pub subject: Option<String>,
    pub requires_business_share: Option<bool>,
    pub investment_risk: Option<bool>,
    pub vat_check: Option<bool>,
    pub sensitive: Option<bool>,
    pub total: Option<Ore>,
    pub receipt_vat: Option<Ore>,
    pub rounding: Option<Ore>,
    pub net: Option<Ore>,
    pub payment_method: Option<String>,
    pub entry_type: Option<String>,
    pub area: Option<String>,
    pub has_image: Option<bool>,
    pub ex_vat: Option<Ore>,
    pub income_vat: Option<Ore>,
    pub inc_vat: Option<Ore>,
    pub payment_date: Option<NaiveDate>,
    pub income_type: Option<String>,
    pub document_count: Option<i32>,
    pub rule_case_id: Option<String>,
    pub vat_rule_id: Option<String>,
    pub counter_source: Option<String>,
    pub counter_key: Option<String>,
    pub counter_bookkeeping_method: Option<String>,
    pub status: String,
}

/// One `findings` row, fields in DDL column order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedFinding {
    pub case_seq: i32,
    pub finding_no: i32,
    pub code: String,
    pub severity: String,
    pub message: String,
    pub question: Option<String>,
}

/// One `decision_lines` row — readback shape only; Slice 2 never writes one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersistedLine {
    pub case_seq: i32,
    pub line_no: i32,
    pub account: String,
    pub role: String,
    pub debit: Ore,
    pub credit: Ore,
}

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
        if found != WORKSPACE_SCHEMA && found != WORKSPACE_SCHEMA_ENGINE {
            return Err(WorkspaceError::SchemaMismatch {
                found,
                expected: WORKSPACE_SCHEMA_ENGINE,
            });
        }
        let snapshots = self.snapshot_count()?;
        if snapshots > 1 {
            return Err(WorkspaceError::Corrupt(format!(
                "snapshot_meta must have 0 or 1 rows, found {snapshots}"
            )));
        }
        if found == WORKSPACE_SCHEMA_ENGINE {
            self.verify_engine_container(snapshots)?;
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
        sha256_via(&self.conn, bytes)
    }

    /// The snapshot's content digest, contract-identical to Django's
    /// `snapshot_sha256()`: canonical bytes (see `canonical_snapshot_bytes`)
    /// hashed by DuckDB. Proven against Django's own digests in
    /// fixtures/snapshots/django-digests.json.
    pub fn snapshot_digest(&self, raw_json: &[u8]) -> Result<String, WorkspaceError> {
        let canonical = canonical_snapshot_bytes(raw_json)?;
        self.sha256_hex(&canonical)
    }

    /// Ingest one snapshot into an empty workspace.
    ///
    /// Digest first (DuckDB `sha256`, Django's canonicalisation), then the
    /// state rule: 0 rows ⇒ ingest; same digest ⇒ `AlreadyIngested`;
    /// another digest ⇒ `SnapshotMismatch` — existing rows are never touched.
    /// Typed parse goes through the unchanged `snapshot::parse_snapshot`; every
    /// DATE is parsed before the transaction opens so an invalid date can
    /// never leave partial rows. Then exactly one transaction: one Appender
    /// per table, explicit `flush()`, commit only after every appender
    /// succeeded; any append or flush error rolls everything back.
    pub fn ingest(&mut self, raw_json: &[u8]) -> Result<IngestOutcome, WorkspaceError> {
        // Schema gate before anything else (Slice 2 §5.6): schema 1 ingests,
        // schema 2 is an evidence container (digest comparison only, never a
        // write), anything else is unsupported and refused before parse,
        // Appender or transaction.
        match self.schema_version()? {
            WORKSPACE_SCHEMA => {}
            WORKSPACE_SCHEMA_ENGINE => {
                return match self.snapshot_count()? {
                    0 => Err(WorkspaceError::Corrupt(
                        "schema 2 workspace without a snapshot cannot be ingested into".to_string(),
                    )),
                    1 => {
                        let digest = self.snapshot_digest(raw_json)?;
                        let existing = self.snapshot_sha256()?;
                        if existing == digest {
                            Ok(IngestOutcome::AlreadyIngested {
                                snapshot_sha256: digest,
                            })
                        } else {
                            Err(WorkspaceError::SnapshotMismatch {
                                existing,
                                incoming: digest,
                            })
                        }
                    }
                    n => Err(WorkspaceError::Corrupt(format!(
                        "snapshot_meta must have 0 or 1 rows, found {n}"
                    ))),
                };
            }
            found => {
                return Err(WorkspaceError::SchemaMismatch {
                    found,
                    expected: WORKSPACE_SCHEMA_ENGINE,
                })
            }
        }
        let digest = self.snapshot_digest(raw_json)?;
        match self.snapshot_count()? {
            0 => {}
            1 => {
                let existing = self.snapshot_sha256()?;
                if existing == digest {
                    return Ok(IngestOutcome::AlreadyIngested {
                        snapshot_sha256: digest,
                    });
                }
                return Err(WorkspaceError::SnapshotMismatch {
                    existing,
                    incoming: digest,
                });
            }
            n => {
                return Err(WorkspaceError::Corrupt(format!(
                    "snapshot_meta must have 0 or 1 rows, found {n}"
                )))
            }
        }
        let snapshot =
            parse_snapshot(raw_json).map_err(|e| WorkspaceError::Snapshot(e.to_string()))?;
        let dates = ParsedDates::parse_all(&snapshot)?;

        let tx = self.conn.transaction()?;
        let counts = match append_snapshot(&tx, &digest, &snapshot, &dates) {
            Ok(counts) => counts,
            Err(e) => {
                // Explicit for readers; dropping the transaction would roll
                // back too, but the contract says so out loud.
                let _ = tx.rollback();
                return Err(e);
            }
        };
        tx.commit()?;
        Ok(IngestOutcome::Ingested {
            snapshot_sha256: digest,
            properties: counts.0,
            receipts: counts.1,
            income_entries: counts.2,
            audit_items: counts.3,
        })
    }

    /// Every read below requires exactly one ingested snapshot.
    fn require_snapshot(&self) -> Result<(), WorkspaceError> {
        match self.snapshot_count()? {
            0 => Err(WorkspaceError::NotIngested),
            1 => Ok(()),
            n => Err(WorkspaceError::Corrupt(format!(
                "snapshot_meta must have 0 or 1 rows, found {n}"
            ))),
        }
    }

    pub fn read_meta(&self) -> Result<WorkspaceMeta, WorkspaceError> {
        self.require_snapshot()?;
        Ok(self.conn.query_row(
            "SELECT snapshot_sha256, schema_version, owner_id, income_year, generated_at, \
             all_properties_locked, declaration_year, source_app, source_environment \
             FROM snapshot_meta",
            [],
            |r| {
                Ok(WorkspaceMeta {
                    snapshot_sha256: r.get(0)?,
                    schema_version: r.get(1)?,
                    owner_id: r.get(2)?,
                    income_year: r.get(3)?,
                    generated_at: r.get(4)?,
                    all_properties_locked: r.get(5)?,
                    declaration_year: r.get(6)?,
                    source_app: r.get(7)?,
                    source_environment: r.get(8)?,
                })
            },
        )?)
    }

    pub fn read_entity(&self) -> Result<EntityContext, WorkspaceError> {
        self.require_snapshot()?;
        let mut entity = self.conn.query_row(
            "SELECT owner_id, display_name, org_number, county, taxonomy_version, vat_registered, \
             bookkeeping_method, default_payment_method, sie_series FROM entity_context",
            [],
            |r| {
                Ok(EntityContext {
                    owner_id: r.get(0)?,
                    display_name: r.get(1)?,
                    org_number: r.get(2)?,
                    county: r.get(3)?,
                    taxonomy_version: r.get(4)?,
                    vat_registered: r.get(5)?,
                    bookkeeping_method: r.get(6)?,
                    default_payment_method: r.get(7)?,
                    sie_series: r.get(8)?,
                    operations: Vec::new(),
                })
            },
        )?;
        let mut stmt = self
            .conn
            .prepare("SELECT operation FROM entity_operations ORDER BY operation")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        for op in rows {
            entity.operations.push(op?);
        }
        Ok(entity)
    }

    pub fn read_properties(&self) -> Result<Vec<PropertyRow>, WorkspaceError> {
        self.require_snapshot()?;
        let mut stmt = self.conn.prepare(
            "SELECT property_id, name, slug, is_default, tax_year_id, tax_year_status, locked_at \
             FROM properties ORDER BY property_id",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PropertyRow {
                property_id: r.get(0)?,
                name: r.get(1)?,
                slug: r.get(2)?,
                is_default: r.get(3)?,
                tax_year_id: r.get(4)?,
                tax_year_status: r.get(5)?,
                locked_at: r.get(6)?,
            })
        })?;
        collect(rows)
    }

    /// Receipts in the contract order (`selector_position` = the snapshot's
    /// `receipts_for_year` order). Never re-sorted by date or ordinal (R12).
    pub fn read_receipts(&self) -> Result<Vec<ReceiptRow>, WorkspaceError> {
        self.require_snapshot()?;
        let mut stmt = self.conn.prepare(
            "SELECT id, source_key, property_id, ordinal_number, vendor, date, category, entry_type, \
             area, requires_business_share, investment_risk, vat_check, sensitive, total_ore, vat_ore, \
             rounding_ore, net_ore, payment_method, note, has_image, confirmed_at, selector_position \
             FROM receipts ORDER BY selector_position",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ReceiptRow {
                id: r.get(0)?,
                source_key: r.get(1)?,
                property_id: r.get(2)?,
                ordinal_number: r.get(3)?,
                vendor: r.get(4)?,
                date: r.get(5)?,
                category: r.get(6)?,
                entry_type: r.get(7)?,
                area: r.get(8)?,
                requires_business_share: r.get(9)?,
                investment_risk: r.get(10)?,
                vat_check: r.get(11)?,
                sensitive: r.get(12)?,
                total: Ore(r.get(13)?),
                vat: Ore(r.get(14)?),
                rounding: Ore(r.get(15)?),
                net: Ore(r.get(16)?),
                payment_method: r.get(17)?,
                note: r.get(18)?,
                has_image: r.get(19)?,
                confirmed_at: r.get(20)?,
                selector_position: r.get(21)?,
            })
        })?;
        collect(rows)
    }

    /// Income entries in snapshot order (`snapshot_position` = (date, pk)).
    pub fn read_income_entries(&self) -> Result<Vec<IncomeRow>, WorkspaceError> {
        self.require_snapshot()?;
        let mut stmt = self.conn.prepare(
            "SELECT id, source_key, property_id, income_type, date, buyer_name, description, \
             ex_vat_ore, vat_ore, inc_vat_ore, invoice_number, payment_date, document_count, \
             snapshot_position FROM income_entries ORDER BY snapshot_position",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(IncomeRow {
                id: r.get(0)?,
                source_key: r.get(1)?,
                property_id: r.get(2)?,
                income_type: r.get(3)?,
                date: r.get(4)?,
                buyer_name: r.get(5)?,
                description: r.get(6)?,
                ex_vat: Ore(r.get(7)?),
                vat: Ore(r.get(8)?),
                inc_vat: Ore(r.get(9)?),
                invoice_number: r.get(10)?,
                payment_date: r.get(11)?,
                document_count: r.get(12)?,
                snapshot_position: r.get(13)?,
            })
        })?;
        collect(rows)
    }

    pub fn read_audit_chain(&self) -> Result<Vec<AuditRow>, WorkspaceError> {
        self.require_snapshot()?;
        let mut stmt = self.conn.prepare(
            "SELECT seq, kind, property_id, event_type, occurred_at, document_type, received_date, \
             checksum_sha256, storage_backend FROM audit_chain ORDER BY seq",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(AuditRow {
                seq: r.get(0)?,
                kind: r.get(1)?,
                property_id: r.get(2)?,
                event_type: r.get(3)?,
                occurred_at: r.get(4)?,
                document_type: r.get(5)?,
                received_date: r.get(6)?,
                checksum_sha256: r.get(7)?,
                storage_backend: r.get(8)?,
            })
        })?;
        collect(rows)
    }

    /// Deterministic fingerprint of the workspace's logical content — built
    /// from the readback rows in contract order and hashed by DuckDB's
    /// `sha256`, so two workspaces ingested from the same snapshot compare
    /// equal even though their file bytes need not. Never touches the file's
    /// physical layout.
    pub fn logical_fingerprint(&self) -> Result<String, WorkspaceError> {
        let mut text = String::new();
        let meta = self.read_meta()?;
        text.push_str(&format!("meta|{meta:?}\n"));
        text.push_str(&format!("entity|{:?}\n", self.read_entity()?));
        for p in self.read_properties()? {
            text.push_str(&format!("property|{p:?}\n"));
        }
        for r in self.read_receipts()? {
            text.push_str(&format!("receipt|{r:?}\n"));
        }
        for e in self.read_income_entries()? {
            text.push_str(&format!("income|{e:?}\n"));
        }
        for a in self.read_audit_chain()? {
            text.push_str(&format!("audit|{a:?}\n"));
        }
        self.sha256_hex(text.as_bytes())
    }

    // -----------------------------------------------------------------------
    // SV-03 Slice 2 — engine run persistence, readback, verification
    // -----------------------------------------------------------------------

    fn run_row_count(&self) -> Result<i64, WorkspaceError> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM engine_run_meta", [], |r| r.get(0))?)
    }

    /// Schema-2 container checks (Slice 2 §E.1): the four tables exist,
    /// `engine_run_meta` has 0 or 1 rows, and a run row requires exactly one
    /// snapshot whose digest it names. Deeper checks live in `verify_run`.
    fn verify_engine_container(&self, snapshots: i64) -> Result<(), WorkspaceError> {
        let tables = self.table_names()?;
        for t in SCHEMA_V2_TABLES {
            if !tables.iter().any(|n| n == t) {
                return Err(WorkspaceError::Corrupt(format!(
                    "schema 2 workspace lacks table {t}"
                )));
            }
        }
        match self.run_row_count()? {
            0 => Ok(()),
            1 => {
                if snapshots != 1 {
                    return Err(WorkspaceError::Corrupt(
                        "engine run present but snapshot_meta has no row".to_string(),
                    ));
                }
                // The singleton key is part of the locked run identity: the
                // digest's `run` record starts with `i:0`, so a stored row
                // with any other run_seq is malformed evidence.
                let run_seq: i32 =
                    self.conn
                        .query_row("SELECT run_seq FROM engine_run_meta", [], |r| r.get(0))?;
                if run_seq != 0 {
                    return Err(WorkspaceError::Corrupt(format!(
                        "engine_run_meta.run_seq must be 0, found {run_seq}"
                    )));
                }
                let run_sha: String = self.conn.query_row(
                    "SELECT snapshot_sha256 FROM engine_run_meta",
                    [],
                    |r| r.get(0),
                )?;
                let snap_sha = self.snapshot_sha256()?;
                if run_sha != snap_sha {
                    return Err(WorkspaceError::Corrupt(format!(
                        "engine run names snapshot {run_sha} but the workspace holds {snap_sha}"
                    )));
                }
                Ok(())
            }
            n => Err(WorkspaceError::Corrupt(format!(
                "engine_run_meta must have 0 or 1 rows, found {n} (more than one run row)"
            ))),
        }
    }

    /// Whether this workspace holds a persisted engine run.
    pub fn engine_state(&self) -> Result<EngineState, WorkspaceError> {
        if self.schema_version()? != WORKSPACE_SCHEMA_ENGINE {
            return Ok(EngineState::NotRun);
        }
        match self.run_row_count()? {
            0 => Ok(EngineState::NotRun),
            1 => Ok(EngineState::Run(self.read_run_meta()?)),
            n => Err(WorkspaceError::Corrupt(format!(
                "engine_run_meta must have 0 or 1 rows, found {n}"
            ))),
        }
    }

    fn require_run(&self) -> Result<(), WorkspaceError> {
        if self.schema_version()? != WORKSPACE_SCHEMA_ENGINE {
            return Err(WorkspaceError::NotRun);
        }
        match self.run_row_count()? {
            0 => Err(WorkspaceError::NotRun),
            1 => Ok(()),
            n => Err(WorkspaceError::Corrupt(format!(
                "engine_run_meta must have 0 or 1 rows, found {n}"
            ))),
        }
    }

    pub fn read_run_meta(&self) -> Result<RunMeta, WorkspaceError> {
        self.require_run()?;
        Ok(self.conn.query_row(
            "SELECT snapshot_sha256, engine_version, chart_id, chart_version, ruleset_version, \
             ruleset_status, taxonomy_version, workbook_sha256, generator_version, decision_sha256 \
             FROM engine_run_meta",
            [],
            |r| {
                Ok(RunMeta {
                    provenance: RunProvenance {
                        snapshot_sha256: r.get(0)?,
                        engine_version: r.get(1)?,
                        chart_id: r.get(2)?,
                        chart_version: r.get(3)?,
                        ruleset_version: r.get(4)?,
                        ruleset_status: r.get(5)?,
                        taxonomy_version: r.get(6)?,
                        workbook_sha256: r.get(7)?,
                        generator_version: r.get(8)?,
                    },
                    decision_sha256: r.get(9)?,
                })
            },
        )?)
    }

    /// Persisted cases in contract order (`case_seq`).
    pub fn read_cases(&self) -> Result<Vec<PersistedCase>, WorkspaceError> {
        self.require_run()?;
        let mut stmt = self.conn.prepare(
            "SELECT case_seq, source, source_key, row_id, property_id, ordinal_number, date, subject, \
             requires_business_share, investment_risk, vat_check, sensitive, \
             total_ore, receipt_vat_ore, rounding_ore, net_ore, payment_method, entry_type, area, has_image, \
             ex_vat_ore, income_vat_ore, inc_vat_ore, payment_date, income_type, document_count, \
             rule_case_id, vat_rule_id, counter_source, counter_key, counter_bookkeeping_method, status \
             FROM accounting_cases ORDER BY case_seq",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PersistedCase {
                case_seq: r.get(0)?,
                source: r.get(1)?,
                source_key: r.get(2)?,
                row_id: r.get(3)?,
                property_id: r.get(4)?,
                ordinal_number: r.get(5)?,
                date: r.get(6)?,
                subject: r.get(7)?,
                requires_business_share: r.get(8)?,
                investment_risk: r.get(9)?,
                vat_check: r.get(10)?,
                sensitive: r.get(11)?,
                total: r.get::<_, Option<i64>>(12)?.map(Ore),
                receipt_vat: r.get::<_, Option<i64>>(13)?.map(Ore),
                rounding: r.get::<_, Option<i64>>(14)?.map(Ore),
                net: r.get::<_, Option<i64>>(15)?.map(Ore),
                payment_method: r.get(16)?,
                entry_type: r.get(17)?,
                area: r.get(18)?,
                has_image: r.get(19)?,
                ex_vat: r.get::<_, Option<i64>>(20)?.map(Ore),
                income_vat: r.get::<_, Option<i64>>(21)?.map(Ore),
                inc_vat: r.get::<_, Option<i64>>(22)?.map(Ore),
                payment_date: r.get(23)?,
                income_type: r.get(24)?,
                document_count: r.get(25)?,
                rule_case_id: r.get(26)?,
                vat_rule_id: r.get(27)?,
                counter_source: r.get(28)?,
                counter_key: r.get(29)?,
                counter_bookkeeping_method: r.get(30)?,
                status: r.get(31)?,
            })
        })?;
        collect(rows)
    }

    /// Persisted findings in contract order (`case_seq`, `finding_no`).
    pub fn read_findings(&self) -> Result<Vec<PersistedFinding>, WorkspaceError> {
        self.require_run()?;
        let mut stmt = self.conn.prepare(
            "SELECT case_seq, finding_no, code, severity, message, question \
             FROM findings ORDER BY case_seq, finding_no",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PersistedFinding {
                case_seq: r.get(0)?,
                finding_no: r.get(1)?,
                code: r.get(2)?,
                severity: r.get(3)?,
                message: r.get(4)?,
                question: r.get(5)?,
            })
        })?;
        collect(rows)
    }

    /// Persisted decision lines in contract order (`case_seq`, `line_no`).
    /// Empty in every Slice-2 run.
    pub fn read_decision_lines(&self) -> Result<Vec<PersistedLine>, WorkspaceError> {
        self.require_run()?;
        let mut stmt = self.conn.prepare(
            "SELECT case_seq, line_no, account, role, debit_ore, credit_ore \
             FROM decision_lines ORDER BY case_seq, line_no",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(PersistedLine {
                case_seq: r.get(0)?,
                line_no: r.get(1)?,
                account: r.get(2)?,
                role: r.get(3)?,
                debit: Ore(r.get(4)?),
                credit: Ore(r.get(5)?),
            })
        })?;
        collect(rows)
    }

    /// The canonical decision digest (Slice 2 §F): a pure function of the
    /// provenance, cases, findings and lines in contract order, hashed by
    /// DuckDB's `sha256`. Excludes only `decision_sha256` itself.
    pub fn decision_digest(
        &self,
        provenance: &RunProvenance,
        cases: &[PersistedCase],
        findings: &[PersistedFinding],
        lines: &[PersistedLine],
    ) -> Result<String, WorkspaceError> {
        self.sha256_hex(&canonical_decision_text(provenance, cases, findings, lines))
    }

    /// Persist one engine run. Safe to call directly: every locked
    /// precondition is enforced here, before any mutation, on the input as
    /// given (nothing is sorted or normalised). Then exactly one transaction:
    /// transactional DDL → Appender(accounting_cases) → Appender(findings) →
    /// digest → INSERT engine_run_meta → workspace_schema = 2 → COMMIT; any
    /// error rolls back to the exact schema-1 state.
    pub fn persist_run(
        &mut self,
        provenance: &RunProvenance,
        cases: &[PersistedCase],
        findings: &[PersistedFinding],
    ) -> Result<RunMeta, WorkspaceError> {
        // 1–3. State: snapshot, schema, no stray engine tables.
        let snapshots = self.snapshot_count()?;
        let schema = self.schema_version()?;
        if schema == WORKSPACE_SCHEMA && snapshots == 0 {
            return Err(WorkspaceError::NotIngested);
        }
        if snapshots != 1 {
            return Err(WorkspaceError::Corrupt(format!(
                "snapshot_meta must have exactly 1 row for a run, found {snapshots}"
            )));
        }
        if schema == WORKSPACE_SCHEMA_ENGINE {
            return match self.run_row_count()? {
                1 => Err(WorkspaceError::AlreadyRun {
                    decision_sha256: self.read_run_meta()?.decision_sha256,
                }),
                0 => Err(WorkspaceError::Corrupt(
                    "schema 2 workspace without a run row cannot receive a run".to_string(),
                )),
                n => Err(WorkspaceError::Corrupt(format!(
                    "engine_run_meta must have 0 or 1 rows, found {n}"
                ))),
            };
        }
        if schema != WORKSPACE_SCHEMA {
            return Err(WorkspaceError::SchemaMismatch {
                found: schema,
                expected: WORKSPACE_SCHEMA_ENGINE,
            });
        }
        let tables = self.table_names()?;
        if let Some(stray) = SCHEMA_V2_TABLES
            .iter()
            .find(|t| tables.iter().any(|n| n == *t))
        {
            return Err(WorkspaceError::Corrupt(format!(
                "schema 1 workspace already contains engine table {stray}"
            )));
        }
        // 4. Provenance names this snapshot.
        let snapshot_sha = self.snapshot_sha256()?;
        if provenance.snapshot_sha256 != snapshot_sha {
            return Err(WorkspaceError::Corrupt(format!(
                "provenance snapshot {} is not this workspace's snapshot {snapshot_sha}",
                provenance.snapshot_sha256
            )));
        }
        // 5–8. Structural + source-mirror checks on the input as given.
        let receipts = self.read_receipts()?;
        let incomes = self.read_income_entries()?;
        check_run_rows(provenance, cases, findings, &[], &receipts, &incomes)?;

        // The transaction.
        let tx = self.conn.transaction()?;
        let result = write_run(&tx, provenance, cases, findings);
        match result {
            Ok(decision_sha256) => {
                tx.commit()?;
                Ok(RunMeta {
                    provenance: provenance.clone(),
                    decision_sha256,
                })
            }
            Err(e) => {
                let _ = tx.rollback();
                Err(e)
            }
        }
    }

    /// Full integrity check of a persisted run (Slice 2 §G): container
    /// state, source mirror against the v1 rows, structural finding/status
    /// contract, zero lines, and the recomputed digest. Nothing is healed.
    pub fn verify_run(&self) -> Result<RunMeta, WorkspaceError> {
        self.require_run()?;
        let snapshots = self.snapshot_count()?;
        self.verify_engine_container(snapshots)?;
        let meta = self.read_run_meta()?;
        if meta.provenance.ruleset_status != "draft" && meta.provenance.ruleset_status != "approved"
        {
            return Err(WorkspaceError::Corrupt(format!(
                "stored ruleset_status {:?} is not draft or approved",
                meta.provenance.ruleset_status
            )));
        }
        let receipts = self.read_receipts()?;
        let incomes = self.read_income_entries()?;
        let cases = self.read_cases()?;
        let findings = self.read_findings()?;
        let lines = self.read_decision_lines()?;
        check_run_rows(
            &meta.provenance,
            &cases,
            &findings,
            &lines,
            &receipts,
            &incomes,
        )?;
        if meta.decision_sha256.len() != 64
            || !meta
                .decision_sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(WorkspaceError::Corrupt(
                "stored decision_sha256 is not 64 lowercase hex characters".to_string(),
            ));
        }
        let recomputed = self.decision_digest(&meta.provenance, &cases, &findings, &lines)?;
        if recomputed != meta.decision_sha256 {
            return Err(WorkspaceError::Corrupt(format!(
                "decision digest mismatch: stored {} recomputed {recomputed}",
                meta.decision_sha256
            )));
        }
        Ok(meta)
    }

    /// Close explicitly so a close failure is an error, not a silent drop.
    pub fn close(self) -> Result<(), WorkspaceError> {
        self.conn
            .close()
            .map_err(|(_, e)| WorkspaceError::Duckdb(e.to_string()))
    }
}

fn collect<T, I: Iterator<Item = duckdb::Result<T>>>(rows: I) -> Result<Vec<T>, WorkspaceError> {
    let mut out = Vec::new();
    for row in rows {
        out.push(row?);
    }
    Ok(out)
}

/// Every DATE the snapshot carries, parsed up front so the transaction can
/// never fail half-way on a date. Field names address the offending value.
struct ParsedDates {
    receipts: Vec<NaiveDate>,
    incomes: Vec<(NaiveDate, Option<NaiveDate>)>,
    audit_received: Vec<Option<NaiveDate>>,
}

impl ParsedDates {
    fn parse_all(snapshot: &Snapshot) -> Result<Self, WorkspaceError> {
        let mut receipts = Vec::with_capacity(snapshot.receipts.len());
        for (i, r) in snapshot.receipts.iter().enumerate() {
            receipts.push(parse_date(&format!("receipts[{i}].date"), &r.date)?);
        }
        let mut incomes = Vec::with_capacity(snapshot.income_entries.len());
        for (i, e) in snapshot.income_entries.iter().enumerate() {
            let date = parse_date(&format!("income_entries[{i}].date"), &e.date)?;
            let paid = match &e.payment_date {
                Some(d) => Some(parse_date(&format!("income_entries[{i}].payment_date"), d)?),
                None => None,
            };
            incomes.push((date, paid));
        }
        let mut audit_received = Vec::with_capacity(snapshot.audit_chain.len());
        for (i, item) in snapshot.audit_chain.iter().enumerate() {
            audit_received.push(match item {
                AuditEvent::Document {
                    received_date: Some(d),
                    ..
                } => Some(parse_date(&format!("audit_chain[{i}].received_date"), d)?),
                _ => None,
            });
        }
        Ok(Self {
            receipts,
            incomes,
            audit_received,
        })
    }
}

fn parse_date(field: &str, value: &str) -> Result<NaiveDate, WorkspaceError> {
    NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| WorkspaceError::InvalidDate {
        field: field.to_string(),
        value: value.to_string(),
    })
}

/// Append every table inside `tx`. Each Appender is flushed explicitly and
/// dropped before the next one; the first error (append or flush) returns
/// immediately so the caller rolls back. Returns
/// (properties, receipts, income_entries, audit_items) row counts.
fn append_snapshot(
    tx: &Transaction<'_>,
    digest: &str,
    s: &Snapshot,
    dates: &ParsedDates,
) -> Result<(usize, usize, usize, usize), WorkspaceError> {
    {
        let mut app = tx.appender("snapshot_meta")?;
        app.append_row(params![
            digest,
            s.schema_version.as_str(),
            s.entity.owner_id,
            i32::from(s.income_year),
            s.generated_at.as_str(),
            s.lock.all_properties_locked,
            i32::from(s.lock.declaration_year),
            s.source.app.as_str(),
            s.source.environment.as_str(),
        ])?;
        app.flush()?;
    }
    {
        let profile = s.entity.accounting_profile.as_ref();
        let mut app = tx.appender("entity_context")?;
        app.append_row(params![
            s.entity.owner_id,
            s.entity.display_name.as_str(),
            s.entity.org_number.as_deref(),
            s.entity.county.as_deref(),
            s.entity.taxonomy_version.as_deref(),
            profile.map(|p| p.vat_registered.as_str()),
            profile.map(|p| p.bookkeeping_method.as_str()),
            profile.map(|p| p.default_payment_method.as_str()),
            profile.map(|p| p.sie_series.as_str()),
        ])?;
        app.flush()?;
    }
    {
        let mut app = tx.appender("entity_operations")?;
        for op in &s.entity.operation {
            app.append_row(params![op.as_str()])?;
        }
        app.flush()?;
    }
    {
        let mut app = tx.appender("properties")?;
        for p in &s.properties {
            app.append_row(params![
                p.id,
                p.name.as_str(),
                p.slug.as_str(),
                p.is_default,
                p.tax_year.id,
                p.tax_year.status.as_str(),
                p.tax_year.locked_at.as_deref(),
            ])?;
        }
        app.flush()?;
    }
    {
        let mut app = tx.appender("receipts")?;
        for (i, r) in s.receipts.iter().enumerate() {
            let ctx = r.category_context.as_ref();
            app.append_row(params![
                r.id,
                r.source_key.as_deref(),
                r.property_id,
                r.ordinal_number.map(|o| o as i32),
                r.vendor.as_deref(),
                dates.receipts[i],
                r.category.as_deref(),
                r.entry_type.as_str(),
                r.area.as_str(),
                ctx.map(|c| c.requires_business_share),
                ctx.map(|c| c.investment_risk),
                ctx.map(|c| c.vat_check),
                ctx.map(|c| c.sensitive),
                r.total_amount.0,
                r.vat_amount.0,
                r.rounding_amount.0,
                r.net_amount.0,
                r.payment_method.as_deref(),
                r.note.as_deref(),
                r.has_image,
                r.confirmed_at.as_deref(),
                i as i32,
            ])?;
        }
        app.flush()?;
    }
    {
        let mut app = tx.appender("income_entries")?;
        for (i, e) in s.income_entries.iter().enumerate() {
            let (date, paid) = dates.incomes[i];
            app.append_row(params![
                e.id,
                e.source_key.as_deref(),
                e.property_id,
                e.income_type.as_str(),
                date,
                e.buyer_name.as_deref(),
                e.description.as_str(),
                e.amount_ex_vat.0,
                e.vat_amount.0,
                e.amount_inc_vat.0,
                e.invoice_number.as_deref(),
                paid,
                e.document_count as i32,
                i as i32,
            ])?;
        }
        app.flush()?;
    }
    {
        let mut app = tx.appender("audit_chain")?;
        for (i, item) in s.audit_chain.iter().enumerate() {
            match item {
                AuditEvent::Event {
                    event_type,
                    property_id,
                    occurred_at,
                    ..
                } => app.append_row(params![
                    i as i32,
                    "event",
                    *property_id,
                    event_type.as_str(),
                    occurred_at.as_str(),
                    Option::<&str>::None,
                    Option::<NaiveDate>::None,
                    Option::<&str>::None,
                    Option::<&str>::None,
                ])?,
                AuditEvent::Document {
                    document_type,
                    property_id,
                    checksum_sha256,
                    storage_backend,
                    ..
                } => app.append_row(params![
                    i as i32,
                    "document",
                    *property_id,
                    Option::<&str>::None,
                    Option::<&str>::None,
                    document_type.as_str(),
                    dates.audit_received[i],
                    checksum_sha256.as_deref(),
                    storage_backend.as_str(),
                ])?,
            }
        }
        app.flush()?;
    }
    Ok((
        s.properties.len(),
        s.receipts.len(),
        s.income_entries.len(),
        s.audit_chain.len(),
    ))
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

// ---------------------------------------------------------------------------
// SV-03 Slice 2 — free functions
// ---------------------------------------------------------------------------

fn sha256_via(conn: &Connection, bytes: &[u8]) -> Result<String, WorkspaceError> {
    Ok(
        conn.query_row("SELECT sha256(?::BLOB)", params![bytes.to_vec()], |r| {
            r.get(0)
        })?,
    )
}

/// The transaction body of `persist_run`. Returns the decision digest on
/// success; any `Err` makes the caller roll back.
fn write_run(
    tx: &Transaction<'_>,
    provenance: &RunProvenance,
    cases: &[PersistedCase],
    findings: &[PersistedFinding],
) -> Result<String, WorkspaceError> {
    tx.execute_batch(SCHEMA_V2_DDL)?;
    {
        let mut app = tx.appender("accounting_cases")?;
        for c in cases {
            app.append_row(params![
                c.case_seq,
                c.source.as_str(),
                c.source_key.as_str(),
                c.row_id,
                c.property_id,
                c.ordinal_number,
                c.date,
                c.subject.as_deref(),
                c.requires_business_share,
                c.investment_risk,
                c.vat_check,
                c.sensitive,
                c.total.map(|o| o.0),
                c.receipt_vat.map(|o| o.0),
                c.rounding.map(|o| o.0),
                c.net.map(|o| o.0),
                c.payment_method.as_deref(),
                c.entry_type.as_deref(),
                c.area.as_deref(),
                c.has_image,
                c.ex_vat.map(|o| o.0),
                c.income_vat.map(|o| o.0),
                c.inc_vat.map(|o| o.0),
                c.payment_date,
                c.income_type.as_deref(),
                c.document_count,
                c.rule_case_id.as_deref(),
                c.vat_rule_id.as_deref(),
                c.counter_source.as_deref(),
                c.counter_key.as_deref(),
                c.counter_bookkeeping_method.as_deref(),
                c.status.as_str(),
            ])?;
        }
        app.flush()?;
    }
    {
        let mut app = tx.appender("findings")?;
        for f in findings {
            app.append_row(params![
                f.case_seq,
                f.finding_no,
                f.code.as_str(),
                f.severity.as_str(),
                f.message.as_str(),
                f.question.as_deref(),
            ])?;
        }
        app.flush()?;
    }
    let decision_sha256 = sha256_via(
        tx,
        &canonical_decision_text(provenance, cases, findings, &[]),
    )?;
    tx.execute(
        "INSERT INTO engine_run_meta (run_seq, snapshot_sha256, engine_version, chart_id, chart_version, \
         ruleset_version, ruleset_status, taxonomy_version, workbook_sha256, generator_version, decision_sha256) \
         VALUES (0, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)",
        params![
            provenance.snapshot_sha256.as_str(),
            provenance.engine_version.as_str(),
            provenance.chart_id.as_str(),
            provenance.chart_version.as_str(),
            provenance.ruleset_version.as_str(),
            provenance.ruleset_status.as_str(),
            provenance.taxonomy_version.as_str(),
            provenance.workbook_sha256.as_str(),
            provenance.generator_version.as_str(),
            decision_sha256.as_str(),
        ],
    )?;
    tx.execute(
        "UPDATE workspace_meta SET workspace_schema = ?",
        params![WORKSPACE_SCHEMA_ENGINE],
    )?;
    Ok(decision_sha256)
}

// ---- canonical digest text (Slice 2 §F) --------------------------------------

fn enc_null(out: &mut String) {
    out.push('~');
}
fn enc_int(out: &mut String, v: i64) {
    out.push_str("i:");
    out.push_str(&v.to_string());
}
fn enc_opt_int(out: &mut String, v: Option<i64>) {
    match v {
        Some(v) => enc_int(out, v),
        None => enc_null(out),
    }
}
fn enc_bool(out: &mut String, v: bool) {
    out.push_str(if v { "b:1" } else { "b:0" });
}
fn enc_opt_bool(out: &mut String, v: Option<bool>) {
    match v {
        Some(v) => enc_bool(out, v),
        None => enc_null(out),
    }
}
fn enc_date(out: &mut String, d: NaiveDate) {
    out.push_str("d:");
    out.push_str(&d.format("%Y-%m-%d").to_string());
}
fn enc_opt_date(out: &mut String, d: Option<NaiveDate>) {
    match d {
        Some(d) => enc_date(out, d),
        None => enc_null(out),
    }
}
fn enc_str(out: &mut String, s: &str) {
    out.push_str("s:");
    // serde_json's string escaping: `"`, `\`, control characters; UTF-8 kept.
    // `to_string` of a `&str` cannot fail; should it ever, the canonical text
    // must not silently degrade, so the failure is made loud.
    match serde_json::to_string(s) {
        Ok(encoded) => out.push_str(&encoded),
        Err(e) => panic!("canonical string encoding failed: {e}"),
    }
}
fn enc_opt_str(out: &mut String, s: Option<&str>) {
    match s {
        Some(s) => enc_str(out, s),
        None => enc_null(out),
    }
}

/// The locked digest preimage: framing line, one `run` record (provenance in
/// DDL order, `run_seq` first, `decision_sha256` excluded), then `case`,
/// `finding` and `line` records in contract order with fields in DDL column
/// order, TAB before every field, LF after every record, and a final `end`
/// record with the three counts.
pub fn canonical_decision_text(
    provenance: &RunProvenance,
    cases: &[PersistedCase],
    findings: &[PersistedFinding],
    lines: &[PersistedLine],
) -> Vec<u8> {
    let mut out = String::from("sieverk-decision/1\n");
    out.push_str("run");
    out.push('\t');
    enc_int(&mut out, 0);
    for field in [
        &provenance.snapshot_sha256,
        &provenance.engine_version,
        &provenance.chart_id,
        &provenance.chart_version,
        &provenance.ruleset_version,
        &provenance.ruleset_status,
        &provenance.taxonomy_version,
        &provenance.workbook_sha256,
        &provenance.generator_version,
    ] {
        out.push('\t');
        enc_str(&mut out, field);
    }
    out.push('\n');
    for c in cases {
        out.push_str("case");
        out.push('\t');
        enc_int(&mut out, i64::from(c.case_seq));
        out.push('\t');
        enc_str(&mut out, &c.source);
        out.push('\t');
        enc_str(&mut out, &c.source_key);
        out.push('\t');
        enc_int(&mut out, c.row_id);
        out.push('\t');
        enc_int(&mut out, c.property_id);
        out.push('\t');
        enc_opt_int(&mut out, c.ordinal_number.map(i64::from));
        out.push('\t');
        enc_date(&mut out, c.date);
        out.push('\t');
        enc_opt_str(&mut out, c.subject.as_deref());
        for flag in [
            c.requires_business_share,
            c.investment_risk,
            c.vat_check,
            c.sensitive,
        ] {
            out.push('\t');
            enc_opt_bool(&mut out, flag);
        }
        for money in [c.total, c.receipt_vat, c.rounding, c.net] {
            out.push('\t');
            enc_opt_int(&mut out, money.map(|o| o.0));
        }
        out.push('\t');
        enc_opt_str(&mut out, c.payment_method.as_deref());
        out.push('\t');
        enc_opt_str(&mut out, c.entry_type.as_deref());
        out.push('\t');
        enc_opt_str(&mut out, c.area.as_deref());
        out.push('\t');
        enc_opt_bool(&mut out, c.has_image);
        for money in [c.ex_vat, c.income_vat, c.inc_vat] {
            out.push('\t');
            enc_opt_int(&mut out, money.map(|o| o.0));
        }
        out.push('\t');
        enc_opt_date(&mut out, c.payment_date);
        out.push('\t');
        enc_opt_str(&mut out, c.income_type.as_deref());
        out.push('\t');
        enc_opt_int(&mut out, c.document_count.map(i64::from));
        for s in [
            c.rule_case_id.as_deref(),
            c.vat_rule_id.as_deref(),
            c.counter_source.as_deref(),
            c.counter_key.as_deref(),
            c.counter_bookkeeping_method.as_deref(),
        ] {
            out.push('\t');
            enc_opt_str(&mut out, s);
        }
        out.push('\t');
        enc_str(&mut out, &c.status);
        out.push('\n');
    }
    for f in findings {
        out.push_str("finding");
        out.push('\t');
        enc_int(&mut out, i64::from(f.case_seq));
        out.push('\t');
        enc_int(&mut out, i64::from(f.finding_no));
        out.push('\t');
        enc_str(&mut out, &f.code);
        out.push('\t');
        enc_str(&mut out, &f.severity);
        out.push('\t');
        enc_str(&mut out, &f.message);
        out.push('\t');
        enc_opt_str(&mut out, f.question.as_deref());
        out.push('\n');
    }
    for l in lines {
        out.push_str("line");
        out.push('\t');
        enc_int(&mut out, i64::from(l.case_seq));
        out.push('\t');
        enc_int(&mut out, i64::from(l.line_no));
        out.push('\t');
        enc_str(&mut out, &l.account);
        out.push('\t');
        enc_str(&mut out, &l.role);
        out.push('\t');
        enc_int(&mut out, l.debit.0);
        out.push('\t');
        enc_int(&mut out, l.credit.0);
        out.push('\n');
    }
    out.push_str("end");
    for n in [cases.len(), findings.len(), lines.len()] {
        out.push('\t');
        enc_int(&mut out, n as i64);
    }
    out.push('\n');
    out.into_bytes()
}

// ---- structural + source-mirror validation (Slice 2 §E.2 / §G / addendum §2) --

fn corrupt(msg: String) -> WorkspaceError {
    WorkspaceError::Corrupt(msg)
}

fn rank_of(code: &str) -> Option<usize> {
    FINDING_CODE_RANK.iter().position(|c| *c == code)
}

/// Every structural rule a Slice-2 run must satisfy, applied to rows exactly
/// as given (input to `persist_run`, or readback in `verify_run`). Nothing
/// is sorted or repaired; the first violation is returned.
fn check_run_rows(
    provenance: &RunProvenance,
    cases: &[PersistedCase],
    findings: &[PersistedFinding],
    lines: &[PersistedLine],
    receipts: &[ReceiptRow],
    incomes: &[IncomeRow],
) -> Result<(), WorkspaceError> {
    // Zero lines in Slice 2.
    if !lines.is_empty() {
        return Err(corrupt(format!(
            "decision_lines must be empty in Slice 2, found {}",
            lines.len()
        )));
    }
    // Count and dense sequence.
    if cases.len() != receipts.len() + incomes.len() {
        return Err(corrupt(format!(
            "expected {} cases (receipts {} + incomes {}), found {}",
            receipts.len() + incomes.len(),
            receipts.len(),
            incomes.len(),
            cases.len()
        )));
    }
    let mut keys = HashSet::new();
    for (i, c) in cases.iter().enumerate() {
        if c.case_seq != i as i32 {
            return Err(corrupt(format!(
                "case_seq at position {i} is {} (must be dense from 0)",
                c.case_seq
            )));
        }
        if !keys.insert(c.source_key.as_str()) {
            return Err(corrupt(format!("duplicate source_key {}", c.source_key)));
        }
        if !STATUSES.contains(&c.status.as_str()) {
            return Err(corrupt(format!(
                "case {i}: status {:?} is not in the lifecycle",
                c.status
            )));
        }
        if provenance.ruleset_status == "draft" && c.status == "Automatic" {
            return Err(corrupt(format!(
                "case {i}: Automatic under draft provenance"
            )));
        }
        if c.vat_rule_id.is_some() && c.rule_case_id.is_none() {
            return Err(corrupt(format!(
                "case {i}: vat_rule_id without rule_case_id"
            )));
        }
        // Counter reference: none, or source+key with counter_source == source.
        match (&c.counter_source, &c.counter_key) {
            (None, None) => {
                if c.counter_bookkeeping_method.is_some() {
                    return Err(corrupt(format!("case {i}: partial counter reference")));
                }
            }
            (Some(src), Some(key)) => {
                if src != &c.source {
                    return Err(corrupt(format!(
                        "case {i}: counter_source {src} != source {}",
                        c.source
                    )));
                }
                if c.source == "income" {
                    return Err(corrupt(format!(
                        "case {i}: income counter reference is not allowed in Slice 2"
                    )));
                }
                match &c.payment_method {
                    Some(pm) if pm == key => {}
                    Some(pm) => {
                        return Err(corrupt(format!(
                            "case {i}: counter_key {key} != payment_method {pm}"
                        )))
                    }
                    None => {
                        return Err(corrupt(format!(
                            "case {i}: counter reference without payment_method"
                        )))
                    }
                }
            }
            _ => return Err(corrupt(format!("case {i}: partial counter reference"))),
        }
    }
    // Source mirror: receipts first in selector order, then incomes in snapshot order.
    for (i, r) in receipts.iter().enumerate() {
        let c = &cases[i];
        let expected_key = format!("receipt:{}", r.id);
        let mirror_ok = c.source == "receipt"
            && c.row_id == r.id
            && c.source_key == expected_key
            && c.property_id == r.property_id
            && c.ordinal_number == r.ordinal_number
            && c.date == r.date
            && c.subject == r.category
            && c.requires_business_share == r.requires_business_share
            && c.investment_risk == r.investment_risk
            && c.vat_check == r.vat_check
            && c.sensitive == r.sensitive
            && c.total == Some(r.total)
            && c.receipt_vat == Some(r.vat)
            && c.rounding == Some(r.rounding)
            && c.net == Some(r.net)
            && c.payment_method == r.payment_method
            && c.entry_type.as_deref() == Some(r.entry_type.as_str())
            && c.area.as_deref() == Some(r.area.as_str())
            && c.has_image == Some(r.has_image)
            && c.ex_vat.is_none()
            && c.income_vat.is_none()
            && c.inc_vat.is_none()
            && c.payment_date.is_none()
            && c.income_type.is_none()
            && c.document_count.is_none();
        if !mirror_ok {
            return Err(corrupt(format!(
                "case {i} does not mirror receipt {expected_key} (selector_position {})",
                r.selector_position
            )));
        }
    }
    for (j, e) in incomes.iter().enumerate() {
        let i = receipts.len() + j;
        let c = &cases[i];
        let expected_key = format!("income:{}", e.id);
        let mirror_ok = c.source == "income"
            && c.row_id == e.id
            && c.source_key == expected_key
            && c.property_id == e.property_id
            && c.ordinal_number.is_none()
            && c.date == e.date
            && c.subject.as_deref() == Some(e.income_type.as_str())
            && c.income_type.as_deref() == Some(e.income_type.as_str())
            && c.requires_business_share.is_none()
            && c.investment_risk.is_none()
            && c.vat_check.is_none()
            && c.sensitive.is_none()
            && c.ex_vat == Some(e.ex_vat)
            && c.income_vat == Some(e.vat)
            && c.inc_vat == Some(e.inc_vat)
            && c.payment_date == e.payment_date
            && c.document_count == Some(e.document_count)
            && c.total.is_none()
            && c.receipt_vat.is_none()
            && c.rounding.is_none()
            && c.net.is_none()
            && c.payment_method.is_none()
            && c.entry_type.is_none()
            && c.area.is_none()
            && c.has_image.is_none();
        if !mirror_ok {
            return Err(corrupt(format!(
                "case {i} does not mirror income {expected_key} (snapshot_position {})",
                e.snapshot_position
            )));
        }
    }
    // Findings: exact order, dense per case, no orphan, Slice-2 code set,
    // severity mapping, strict rank, no duplicate code, Blocking ⇒ Manual.
    let mut expected_no: i32 = 0;
    let mut prev_seq: Option<i32> = None;
    let mut prev_rank: Option<usize> = None;
    for (idx, f) in findings.iter().enumerate() {
        if f.case_seq < 0 || (f.case_seq as usize) >= cases.len() {
            return Err(corrupt(format!(
                "finding {idx}: orphan case_seq {}",
                f.case_seq
            )));
        }
        match prev_seq {
            Some(p) if f.case_seq < p => {
                return Err(corrupt(format!(
                    "finding {idx}: case_seq {} out of order",
                    f.case_seq
                )))
            }
            Some(p) if f.case_seq == p => {}
            _ => {
                expected_no = 0;
                prev_rank = None;
            }
        }
        if f.finding_no != expected_no {
            return Err(corrupt(format!(
                "finding {idx}: finding_no {} but expected {expected_no} (dense per case)",
                f.finding_no
            )));
        }
        let Some(rank) = rank_of(&f.code) else {
            return Err(corrupt(format!("finding {idx}: unknown code {:?}", f.code)));
        };
        let Some((_, severity)) = SLICE2_FINDING_SEVERITY.iter().find(|(c, _)| *c == f.code) else {
            return Err(corrupt(format!(
                "finding {idx}: code {} is not allowed in a Slice-2 run",
                f.code
            )));
        };
        if f.severity != *severity {
            return Err(corrupt(format!(
                "finding {idx}: code {} must have severity {severity}, found {}",
                f.code, f.severity
            )));
        }
        if let Some(p) = prev_rank {
            if rank == p {
                return Err(corrupt(format!(
                    "finding {idx}: duplicate code {} in one case",
                    f.code
                )));
            }
            if rank < p {
                return Err(corrupt(format!(
                    "finding {idx}: code {} out of fixed rank order",
                    f.code
                )));
            }
        }
        if f.severity == "blocking" && cases[f.case_seq as usize].status != "Manual" {
            return Err(corrupt(format!(
                "case {}: blocking finding but status {}",
                f.case_seq, cases[f.case_seq as usize].status
            )));
        }
        prev_seq = Some(f.case_seq);
        prev_rank = Some(rank);
        expected_no += 1;
    }
    Ok(())
}
