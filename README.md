# sieverk

Sågverket tar timmer. Sieverk tar SIE-filer.

A Rust engine for Swedish SIE accounting files — parser, validator, and
(eventually) the analytics/reconciliation sidecar behind SkogsKvitto's
Bokföringskontroll. Standalone by design: it consumes snapshots and SIE
files, emits reports, and never touches a production database.

## Status: FOUNDATION (SV-01) + MASTERDATA (SV-02) + DUCKDB WORKSPACE (SV-02D)

Library + thin CLI. The read chain runs: CP437 decode → tokenizer →
metadata → accounts → vouchers → validator. The Django boundary is read
too: `src/snapshot.rs` ingests snapshot JSON (schema 1.0 and 1.1) into
exact `Ore` amounts, or fails with the offending field path. 84 tests.
The boundary is `docs/snapshot-contract.md` (v1.3); the format map is
`docs/SIE-NOTES.md`.

SV-02 adds masterdata: `chart.rs`/`sru.rs`/`ruleset.rs` load the
generated profile chart, SRU table, VAT rules, counter accounts and accounting
cases from an explicit root and re-run the structural invariants;
`tools/mastermatris_gen.py` turns a Mastermatris v1.2 workbook into those
artefacts (draft/approved). See `docs/masterdata.md`.

SV-02D adds the engine workspace: `src/workspace.rs` turns one snapshot into
a file-backed DuckDB database. Django/PostgreSQL remains the source of truth;
the workspace is derived and reproducible from the snapshot, and nothing is
ever written back. Chain: snapshot JSON → canonical digest (Django's
canonicalisation, hashed by DuckDB's `sha256`) → typed ingestion in one
transaction → DuckDB tables with money as `BIGINT` öre, dates as `DATE`,
flags as `BOOLEAN` → typed readback from DuckDB only. No accounting
decisions, no accounting engine and no SIE output from the workspace yet — those are
the next gated drops (SV-03, SV-04) in SkogsKvitto's `docs/SIE-plan-2026-09.md`.

## Run

    cargo test --all-targets
    cargo run -- inspect-sie fixtures/minimal_valid.se                  # Status: Valid
    cargo run -- validate-sie fixtures/invalid_unbalanced.se            # the verdict
    cargo run -- inspect-snapshot fixtures/snapshots/minimal-1.1.json   # exit 0
    cargo run -- inspect-snapshot fixtures/snapshots/invalid-net.json   # field path, exit 1
    cargo run -- inspect-masterdata --root fixtures/masterdata/synthetic/generated   # SV-02, exit 0
    cargo run -- inspect-masterdata --root fixtures/masterdata/synthetic/invalid/v4-duplicate-account   # exit 1
    cargo run -- ingest --snapshot fixtures/snapshots/testgarden-2026-1.1.json --workspace temp/testgarden.duckdb   # SV-02D
    cargo run -- inspect-workspace --workspace temp/testgarden.duckdb        # counts + digest + fingerprint, from DuckDB only

## Next

In plan order only: accounting engine (SV-03) reading this workspace plus
masterdata, SIE 4I writer with round-trip through this parser (SV-04),
LIVE-E2E-01, return-SIE reconciliation. Each behind its own gate.

## House rules

Real SIE files never enter git (`.gitignore` blocks `*.se` globally;
only `fixtures/` is whitelisted — put local test files in `local-data/`).
Fixtures are binary in `.gitattributes` so Windows checkouts can't mangle
their CP437/CRLF bytes; JSON snapshot fixtures are text with LF. Money is
never `f64`, and at the snapshot boundary it is a string with exactly two
decimals. Parsers never panic on bad input. Commit only on green.
