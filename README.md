# sieverk

Sågverket tar timmer. Sieverk tar SIE-filer.

A Rust engine for Swedish SIE accounting files — parser, validator, and
(eventually) the analytics/reconciliation sidecar behind SkogsKvitto's
Bokföringskontroll. Standalone by design: it consumes snapshots and SIE
files, emits reports, and never touches a production database.

## Status: week-one goal reached

The full chain runs: CP437 decode → tokenizer → metadata → accounts →
vouchers → validator. 54 tests. The Django boundary is
`docs/snapshot-contract.md` (v1.1 FINAL); the format map is
`docs/SIE-NOTES.md`.

## Run

    cargo test
    cargo run -- inspect-sie fixtures/minimal_valid.se       # Status: Valid
    cargo run -- validate-sie fixtures/invalid_unbalanced.se # the verdict

## Next

Nothing, on purpose, until after the SkogsKvitto launch (2026-08-01).
Then, per plan: snapshot ingestion, reconciliation, SIE-compatible export
drafts. No DuckDB, no SolidJS, no Django until the engine earns them.

## House rules

Real SIE files never enter git (`.gitignore` blocks `*.se` globally;
only `fixtures/` is whitelisted — put local test files in `local-data/`).
Fixtures are binary in `.gitattributes` so Windows checkouts can't mangle
their CP437/CRLF bytes. Money is never `f64`. Parsers never panic on bad
input. Commit only on green.
