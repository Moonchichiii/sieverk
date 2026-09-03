# masterdata/ — provenance and rights (SV-02)

Tracked here:

| File | What | Source |
|---|---|---|
| `taxonomy-1.0.json` | SkogsKvitto's taxonomy contract export (SK-TX01, `scripts/export_taxonomy.py`) — 51 categories with exactly the seven contract fields, income types and payment methods from the model enums | SkogsKvitto `8b73fe1`+, taxonomy 1.0, sha256 `98c8384e6fdb3d9e0571c4aa7bcbc24d8ebd9eecc1e86be343a49aab6a2d2c11` |
| `vibeke-k1-lista-2026-08-11.json` | Vibeke's "Förslag på enkel kontoplan för skogsägare som bokför enligt K1" — **45 accounts**, counted from her PDF; the `must_include` minimum of the profile (`must_include_origin=VIBEKE_K1_LIST`) and the name source for numbers the K1 table lacks | her own list, provided 2026-08-11 |

**Not tracked — `masterdata/real/` (gitignored, never staged):**

```
masterdata/real/
  Mastermatris_v1.2.xlsx        the real workbook (schema v1.2) — Vibeke fills it
  bas-2018-k1.xlsx              BAS 2018 för K1 (fullständig) — authoritative K1 table, bas.se
  bas-2026-crosscheck.xlsx      BAS 2026 — cross-check of general accounts only, never K1 authority
  generated/                    real artefacts (chart-lantbruk-k1-2026.1.json, …)
  build-report.txt              untracked build evidence (timestamp, git SHA, environment)
```

Rights status: **not confirmed by BAS.** bas.se offers the charts as free downloads;
whether a BAS-derived profile artefact may be redistributed from this public repository is
an open question to BAS (`info@bas.se`) — the answer is archived here when it arrives. Until
then no BAS-derived data is committed: the public contract fixture under
`fixtures/masterdata/synthetic/` uses synthetic names and Vibeke's own K1 list (2026-08-11,
her document). Nothing in this repository claims redistribution is legally cleared.

The paid "BAS-kontoplanen 2026 i JSON-format" (4 000 SEK ex VAT) is not bought until BAS has
answered whether it is intended for K1/förenklat årsbokslut or whether BAS 2018 för K1 still
applies.

Build the real workbook and artefacts locally (never in CI). Both BAS files are **required**
(M1): the K1 table is the authority, BAS 2026 is cross-check only; where they disagree for a
profile account the build refuses until a human writes a `source_decision` — nothing is merged
automatically. The reader expects a sheet with the columns `Konto` and `Kontonamn`; if the
downloaded files use other headings the build fails readably (do not rename accounts by hand).

```
uv run --with openpyxl tools/mastermatris_gen.py init-workbook --profile lantbruk-k1 \
    --taxonomy masterdata/taxonomy-1.0.json \
    --k1-list masterdata/vibeke-k1-lista-2026-08-11.json \
    --k1-table masterdata/real/bas-2018-k1.xlsx \
    --crosscheck masterdata/real/bas-2026-crosscheck.xlsx \
    --workbook masterdata/real/Mastermatris_v1.2.xlsx        # prefilled Konton; rows needing a decision are flagged in `kommentar`
uv run --with openpyxl tools/mastermatris_gen.py --root masterdata/real \
    --workbook masterdata/real/Mastermatris_v1.2.xlsx \
    --taxonomy masterdata/taxonomy-1.0.json \
    --k1-list masterdata/vibeke-k1-lista-2026-08-11.json \
    --k1-table masterdata/real/bas-2018-k1.xlsx \
    --crosscheck masterdata/real/bas-2026-crosscheck.xlsx \
    --out masterdata/real/generated --draft
cargo run -- inspect-masterdata --root masterdata/real/generated
git status --short            # must show nothing under masterdata/real/
```
