# SIEverk – BAS/K1 annual source and masterdata candidate workflow

**Scope:** SIEverk only. The production SkogsKvitto SIE export remains **OFF** until independently approved real masterdata, its deployment and the remaining export gates pass.

## Ownership and hierarchy

1. `masterdata/taxonomy-1.0.json` – SkogsKvitto's existing taxonomy contract (tracked).
2. `masterdata/vibeke-k1-lista-2026-08-11.json` – Vibeke's existing 45-account minimum, unchanged (tracked).
3. `masterdata/real/incoming-2026-11-14/` – unmodified private copies of the downloaded BAS 2018 K1 `.xls` and BAS 2026 `.xlsx` (Git-ignored).
4. `masterdata/real/releases/YYYY.N-candidate-YYYYMMDD/` – **new and never overwritten** per-year candidates, including normalized references, a comparison and a draft Mastermatris (Git-ignored).
5. `tools/mastermatris_gen.py` – existing authoritative generator and validation logic; **not modified by this change**.

The original K1 chart is authoritative for supported K1 accounts; BAS 2026 is a **cross-check**, not a replacement. Vibeke-specific accounts missing from either BAS version are not deleted, remapped, or approved automatically.

## Candidate command

Run the provided `RUN-CANDIDATE.ps1` from Windows PowerShell 7 **after** installing the three tracked files with `INSTALL-LOCAL.ps1`.

Pinned source SHA-256 (from independent, user-operated copy checks):

- `Kontoplan_K1_2018.xls`: `93F49B31CF6FB04F812D7FBF2B1E7CF22CFA15956D2B68434AB7668B52CC5DF1`
- `BAS_kontoplan_2026_v2.xlsx`: `A86B39937FAB280D4E5DB895C04C2AF6E145695863D4845FF14EEA5D0302328A`

The import preflights source hashes, the original K1 title, BAS 2026 sheet/title, number formats, counts (151 K1; 1282 distinct annual numbers), duplicate/conflicting names, Vibeke list (exactly 45) and Git-ignore status before creating anything. It creates a new `releases/2026.1-candidate-20261114/` and fails if that target already exists.

### Candidate outputs

- `normalized/bas-2018-k1.xlsx` – full unambiguous K1 reference: `Konto`, `Kontonamn`.
- `normalized/bas-2026-crosscheck.xlsx` – full **unambiguous** annual reference in the same two-column format, not a replacement for the BAS source. Ambiguous account 2087 is omitted **only from this derived working reference**; both original rows remain intact in the source and fully recorded with their locations in the manifest and review document.
- `vibeke-comparison.csv` – **all 45** Vibeke accounts, provenance and explicit human review state.
- `Mastermatris_v1.2.xlsx` – fresh **draft** initialized by the existing SIEverk generator with the existing 45-account Vibeke minimum and ruleset core accounts. Draft status is not approval.
- `manifest.json` – source and generated SHA-256, counts, K2 marks, conflicts, missing-account lists and `approved_for_production: false`.
- `review.md` – human-readable findings and decisions still needed.

**Known 2026 source conflict:** BAS 2026 account 2087 has two distinct names at source rows 241 and 242. No name is chosen. The candidate is `DRAFT_REVIEW_REQUIRED`; a human must resolve the documented source conflict before any release decision.

**Known 2018 K1 vs Vibeke gap:** 25 of Vibeke's 45 accounts do not occur in BAS 2018 K1. This is expected by the existing architecture: `source_id=VIBEKE_K1_LIST`, not an account deletion. BAS 2026 has 11 of these 45 absent, with the 2026 version serving as reference only.

## Annual 2027 update

Run the same tool with a **new** `--release-id`, `--annual-year`, cross-check source and expected file SHA. Always keep the earlier release and source bytes. Review `manifest.json` and `vibeke-comparison.csv`, perform human source decisions, then update a **new** Mastermatris copy (never the previous approved version). The private candidate stays outside Git; only source code, synthetic regression tests and generic process documentation are committed.

**No bulk approval:** Running `init-workbook` does not create a valid production `generated/` ruleset. Accounting case rules, VAT/SRU mappings, `accounting_reviewer` review and approved examples are required, followed by the existing generator `--approved` and Rust `inspect-masterdata` using a separate, approved generated root.

## Operational restrictions

- No source overwrite, silent merge, account renumbering or name inference.
- No `git reset --hard`, force delete, remote push or pull request.
- No GitHub Actions, Fly, Neon, Backblaze or customer data operation during candidate creation.
- Existing SIEverk Rust and `mastermatris_gen.py` remain unchanged.
- The old, mistakenly created `masterdata/incoming-bas-2026-11-14/` directory is left untouched. It contains no candidate output from this tool.
- If a preflight fails, there is no output candidate. If a failure occurs **after** the new directory is created, leave it for inspection; do not automatically delete or retry into the same path.
