# sieverk

Sågverket tar timmer. Sieverk tar SIE-filer.

A Rust engine for Swedish SIE accounting files — parser, validator, and
(eventually) the analytics/reconciliation sidecar behind SkogsKvitto's
Bokföringskontroll. Standalone by design: it consumes snapshots and SIE
files, emits reports, and never touches a production database.

## Status: FOUNDATION (SV-01)

Library + thin CLI. The read chain runs: CP437 decode → tokenizer →
metadata → accounts → vouchers → validator. The Django boundary is read
too: `src/snapshot.rs` ingests snapshot JSON (schema 1.0 and 1.1) into
exact `Ore` amounts, or fails with the offending field path. 80 tests.
The boundary is `docs/snapshot-contract.md` (v1.3); the format map is
`docs/SIE-NOTES.md`.

No chart of accounts, VAT rules, accounting engine or SIE writer yet —
those are gated drops in SkogsKvitto's `docs/SIE-plan-2026-09.md`.

## Run

    cargo test --all-targets
    cargo run -- inspect-sie fixtures/minimal_valid.se                  # Status: Valid
    cargo run -- validate-sie fixtures/invalid_unbalanced.se            # the verdict
    cargo run -- inspect-snapshot fixtures/snapshots/minimal-1.1.json   # exit 0
    cargo run -- inspect-snapshot fixtures/snapshots/invalid-net.json   # field path, exit 1

## Next

In plan order only: masterdata (chart/SRU/ruleset as generated data),
accounting engine, SIE 4I writer with round-trip through this parser,
return-SIE reconciliation. Each behind its own gate.

## House rules

Real SIE files never enter git (`.gitignore` blocks `*.se` globally;
only `fixtures/` is whitelisted — put local test files in `local-data/`).
Fixtures are binary in `.gitattributes` so Windows checkouts can't mangle
their CP437/CRLF bytes; JSON snapshot fixtures are text with LF. Money is
never `f64`, and at the snapshot boundary it is a string with exactly two
decimals. Parsers never panic on bad input. Commit only on green.
