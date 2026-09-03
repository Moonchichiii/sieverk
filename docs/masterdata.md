# Masterdata in SIEverk (SV-02) — how the pieces fit

Contract: `docs/SV-02-forspec.md` rev 3.1, `docs/mastermatris-v1.1-schema.md` +
`docs/mastermatris-v1.2-amendment.md`. This page is the operating manual, not a new contract.

## Two roots, no fallback

```
TRACKED PUBLIC                                   REAL LOCAL (gitignored)
fixtures/masterdata/synthetic/                   masterdata/real/
  Mastermatris_v1.2-synthetic.xlsx                 Mastermatris_v1.2.xlsx
  generated/  chart-synthetic-k1-2026.1.json       bas-2018-k1.xlsx
              sru-2026.json                        bas-2026-crosscheck.xlsx
              vat-rules-2026.1.json                generated/  chart-lantbruk-k1-2026.1.json …
              counter-accounts-2026.1.json         build-report.txt
              ruleset-2026.1.json
              examples-2026.1.json
  invalid/    25 broken roots, one per load-time rule (tools/tests/make_invalid_fixtures.py)
masterdata/taxonomy-1.0.json               (SkogsKvitto export, tracked)
masterdata/vibeke-k1-lista-2026-08-11.json (Vibeke's 45 accounts — must_include minimum, tracked)
```

`tools/mastermatris_gen.py` and `sieverk inspect-masterdata` take the root explicitly. Public
CI regenerates and tests the synthetic root only; `cargo test --all-targets` never needs
`masterdata/real/**`. Real masterdata is verified locally (see `masterdata/README.md`) and used
by LIVE-E2E.

## Draft vs approved (v1.2 §C)

`--draft`: V1–V4, V6–V8, V10–V12, V14 are hard; V5/V9/V13 are warnings; every `Automatic`
input that is not approved (status `Godkänd`, `accounting_reviewer`, ≥1 approved example) is
downgraded **in the generator** to `Conditional` with `villkor = "Ej accounting_reviewer-godkänd
för automatisk kontering."` and `kontrollfråga = "Regeln är inte konsultgranskad."` when blank.
Artefacts carry `review_status: draft`; the engine may only produce preliminary output from them.
Rust never downgrades: a draft ruleset that contains `Automatic` refuses to load.

`--approved`: V1–V14 hard; `Automatic` only for reviewed cases with approved examples.

## What the generator emits

Six JSON files, sorted keys, sorted rows, `_header` without any wall-clock field (V14):
`{generated, workbook, workbook_sha256, workbook_schema, taxonomy_version, generator_version,
review_status, profile_scope}`. The ruleset carries the taxonomy universe it was validated
against (`taxonomy.categories/income_types/payment_methods`), so Rust re-runs V1/V2 from the
root alone. The real build timestamp, git SHA and Python version go to the untracked
`build-report.txt`.

## Rust surface

- `chart.rs` — `ChartProfile`, `Account{source_id, source_version, must_include,
  must_include_origin, source_decision, roles}`, load-time V4 + provenance, `classify()`
  (InProfile / InProfileNotReturnAllowed / ConsultantIntroduced / Malformed — a return-SIE
  account outside the profile is a warning class, never an error).
- `sru.rs` — `SruTable` load + validation only (V6); `verified_for(account, year)` returns
  only rows whose validity was verified. No `#SRU`/`#TAXAR` mechanics here.
- `ruleset.rs` — `VatRules`, `CounterRules`, `AccountingRuleset`, `load_masterdata(root)` with
  the cross-file invariants (V1–V4, V6–V8, V10 shape, draft/approved, chart reference).
- CLI: `sieverk inspect-masterdata --root <generated-dir>` (exit 0 valid, 1 invalid).

## Real LANTBRUK_K1 build path (M1)

`init-workbook --profile lantbruk-k1 --k1-list … --k1-table bas-2018-k1.xlsx --crosscheck
bas-2026-crosscheck.xlsx` creates `Mastermatris_v1.2.xlsx` with the generated sheets **and a
prefilled `Konton`**: Vibeke's 45 accounts (name from the K1 table when the number exists there,
`source_id=BAS_2018_K1`; otherwise her name as a free account, `source_id=VIBEKE_K1_LIST`) plus the
needs-driven ruleset core (1910, 2440, 3740; `must_include_origin=RULESET`). Rows where the K1
table and the 2026 cross-check disagree are flagged `KRÄVER source_decision` in `kommentar`.
The build (`--k1-table`/`--crosscheck` mandatory for `profile=lantbruk_k1`) then enforces M1:
every `BAS_2018_K1` account must exist in the K1 table with the same name, a K1/2026 difference
for a profile account needs an explicit `source_decision`, a number present in the K1 table may
not be labelled `VIBEKE_K1_LIST`, and missing source files are a hard failure — the cross-check
is never skipped silently. The real files stay under gitignored `masterdata/real/`; the
capability is proven in `tools/tests/test_generator.py` on synthetic stand-ins of the same shape.

## Workbook creation (synthetic)

`init-workbook --profile synthetic --taxonomy … --workbook <new.xlsx>` creates the generated,
locked sheets (`Taxonomi`, `Värdelistor`) and headers only. The synthetic fixture is built by
`tools/tests/make_synthetic_workbook.py`; the xlsx container is not byte-reproducible (zip
timestamps), so rebuilding it changes `workbook_sha256` in the generated headers — commit both.
