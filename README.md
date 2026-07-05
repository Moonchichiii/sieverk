# sieverk

Sågverket tar timmer. Sieverk tar SIE-filer.

A Rust engine for Swedish SIE accounting files — parser, validator, and
(eventually) the analytics/reconciliation sidecar behind SkogsKvitto's
Bokföringskontroll. Standalone by design: it consumes snapshots and SIE
files, emits reports, and never touches a production database.

## Status: day zero

Done: CP437 decoding, byte-authentic synthetic fixtures, CI, one passing
test suite. Yours: tokenizer → metadata → accounts → vouchers → zero-sum
validator. The map is `docs/SIE-NOTES.md`; the Django boundary is
`docs/snapshot-contract.md` (v1.1 FINAL).

## Run

    cargo test                                # green from minute one
    cargo run -- fixtures/minimal_valid.se    # CP437 decoding proof

## Week-one finish line

`inspect-sie` on the valid fixture prints type/company/year/counts and
`Status: Valid`; the unbalanced fixture prints `VOUCHER_NOT_BALANCED`
with a 100.00 difference. No DuckDB, no SolidJS, no Django until after
the SkogsKvitto launch.

## House rules

Real SIE files never enter git (`.gitignore` blocks `*.se` globally;
only `fixtures/` is whitelisted — put local test files in `local-data/`).
Fixtures are binary in `.gitattributes` so Windows checkouts can't mangle
their CP437/CRLF bytes. Money is never `f64`. Parsers never panic on bad
input. Commit only on green.
