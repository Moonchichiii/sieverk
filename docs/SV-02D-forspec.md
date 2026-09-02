# SV-02D — förspec, rev 3.1 FINAL ARCHITECTURE LOCK (PRESPEC, ingen kod; dependency-lock avsiktligt kvar): DuckDB REAL GATE i SIEverk

*2026-09-02. Bas: SV-02 CLOSED (masterdata i repot). Syfte: ett riktigt, filbaserat DuckDB-
arbetsutrymme mellan snapshot och motor — inte en abstraktion, inte en mock, inte "vi testade
serialiseringen". Source of truth förblir Django/PostgreSQL → immutabel snapshot; DuckDB är
härledd engine workspace och kan alltid återskapas från snapshoten. Rev 3 (kirurgisk):
`workspace_meta` tillagd, engine-metadata flyttad till SV-03:s `engine_run_meta`, öppningstillstånd
definierade, exakt digest-kontrakt, Appender-flush-kontrakt + negativt test, "motorn kör från
raderna" flyttat till SV-03/LIVE-E2E, pk-formulering rättad. Rev 3.1: workspacet bevarar ALLA
nedströms beslutsinputs (entity-/accountingkontext, category_context) — SV-03 läser aldrig
snapshot-JSON igen; SHA256 via DuckDB:s `sha256(BLOB)`, ingen egen implementation, ingen sha2;
DATE-implementation väljs explicit före dependency-lock (A: `chrono`); Appender-negativtestet
tillåter fel vid append eller flush; `create` vägrar befintlig fil.*

## 1. DEPENDENCY DECISION (låses vid PRESPEC LOCK, efter fresh crate-läsning)

```
crate            duckdb            (crates.io, uppmätt 2026-09-02: max_version 1.10505.0,
                                    publicerad 2026-07-22, rust_version 1.85.1;
                                    features bl.a. bundled, bundled-cmake, json, parquet, appender-arrow)
version          PINNED EXAKT vid lock — tas från crates.io samma dag, inte från denna text
                                   (rev 2: 1.10505.0 är verifierad som aktuell publicerad version, fortfarande RAPPORTERAD, INTE PINNAD)
link strategy    bundled (rek.)    reproducerbart på Windows + Ubuntu; kostnad: C++-bygge av DuckDB
                                   (10–30 min första gången, MSVC/cc), permanent CI-tid
                                   alternativ: system-lib — snabbare men miljöberoende; avråds för evidensen
Windows          proven locally    Mats: cargo build/test-output
Ubuntu CI        proven on GitHub  cache: ~/.cargo/registry + target/ nyckelat på Cargo.lock + rust-toolchain
MSRV             rust-toolchain.toml stable ≥ crate.rust_version — bekräftas vid lock
money SQL type   BIGINT (öre)      money Rust type Ore(i64)
DOUBLE/FLOAT     FORBIDDEN för pengar     DECIMAL-roundtrip FORBIDDEN     f64 i Rust FORBIDDEN
features         bundled + DATE-beslut (rev 3.1, låses vid dependency-lock):
                   A (rek.)  duckdb = { version = "=<PINNED>", features = ["bundled", "chrono"] }
                             chrono = "=<PINNED KOMPATIBEL VERSION>"   ← BÅDA raderna: duckdb:s chrono-feature
                             aktiverar bara cratens optional chrono-integration; skriver SIEverk själv
                             `use chrono::NaiveDate` måste chrono deklareras som SIEverks egen direkta dependency
                             "YYYY-MM-DD" → chrono::NaiveDate → DuckDB DATE
                   B         bundled only + duckdb::types::Value::Date32(i32) (dagar sedan Unix epoch) + bevisad explicit
                             ISO→Date32-hjälpare med kanttester (skottår, 1970-gräns, ogiltigt datum); ingen chrono-crate
                   VARCHAR-datum är FÖRBJUDET som fallback; valet bevisas lokalt av Mats innan lock
                   ingen json/parquet/arrow/polars förrän de förtjänat sin plats
sha256           DuckDB:s inbyggda sha256(BLOB) — ingen egen SHA256, ingen sha2-crate (se §3)
```
Detta bryter medvetet SIEverks "ett beroende"-regel; beslutet dokumenteras i `docs/SIE-NOTES.md`
och `Cargo.toml`-kommentaren. `cargo-audit` i CI måste vara grön med den nya kedjan.

## 2. SCHEMA (`src/workspace.rs`, DDL i Rust som konstanter; versionen **lagras** i filen)

```
workspace_meta     (workspace_schema INTEGER NOT NULL, created_by_version TEXT NOT NULL)   -- exakt en rad, skrivs av create()
snapshot_meta      (snapshot_sha256 TEXT PK, schema_version TEXT, owner_id BIGINT, income_year INTEGER,
                    generated_at TEXT, all_properties_locked BOOLEAN, declaration_year INTEGER,
                    source_app TEXT, source_environment TEXT)                              -- 0 eller 1 rad; endast snapshotens egna fält
entity_context     (owner_id BIGINT PK, display_name TEXT, org_number TEXT NULL, county TEXT NULL,
                    taxonomy_version TEXT NULL, vat_registered TEXT, bookkeeping_method TEXT,
                    default_payment_method TEXT, sie_series TEXT)                          -- 0/1 rad, följer snapshot_meta; SV-03:s och
                                                                                           -- SV-04:s identitets-/profilinput, ingen JSON-sidokanal
entity_operations  (operation TEXT PK)                                                    -- verksamhetsgrupper ur entity.operation
-- engine_run_meta (chart_id, chart_version, ruleset_version, ruleset_status, run_at, …)   -- INTRODUCERAS OCH FYLLS I SV-03,
--                                                                                          -- inte i SV-02D: ingest(&Snapshot) har inte dessa
--                                                                                          -- värden och får inte hitta på dem
properties         (property_id BIGINT PK, name TEXT, slug TEXT, is_default BOOLEAN,
                    tax_year_id BIGINT, tax_year_status TEXT, locked_at TEXT)
receipts           (id BIGINT PK, source_key TEXT UNIQUE, property_id BIGINT, ordinal_number INTEGER NULL,
                    vendor TEXT NULL, date DATE, category TEXT NULL, entry_type TEXT, area TEXT,
                    requires_business_share BOOLEAN NULL, investment_risk BOOLEAN NULL,
                    vat_check BOOLEAN NULL, sensitive BOOLEAN NULL,                        -- category_context; NULL när kontext saknas
                                                                                           -- (schema 1.0 / okänd kategori) — aldrig gissat false
                    total_ore BIGINT, vat_ore BIGINT, rounding_ore BIGINT, net_ore BIGINT,
                    payment_method TEXT, note TEXT NULL, has_image BOOLEAN, confirmed_at TEXT NULL,
                    selector_position INTEGER)      -- R12: positionen ur receipts_for_year, aldrig omräknad
income_entries     (id BIGINT PK, source_key TEXT UNIQUE, property_id BIGINT, income_type TEXT, date DATE,
                    buyer_name TEXT NULL, description TEXT, ex_vat_ore BIGINT, vat_ore BIGINT, inc_vat_ore BIGINT,
                    invoice_number TEXT NULL, payment_date DATE NULL, document_count INTEGER,
                    snapshot_position INTEGER)      -- (date, pk)-ordningen ur snapshoten
audit_chain        (seq INTEGER PK, kind TEXT, property_id BIGINT, event_type TEXT NULL, occurred_at TEXT NULL,
                    document_type TEXT NULL, received_date DATE NULL, checksum_sha256 TEXT NULL, storage_backend TEXT NULL)
-- accounting_cases / decision_lines / findings: SV-03:s tabeller — skapas av SV-03:s schema-steg (workspace_schema=2),
--                                                inte tomma i SV-02D
```
Ingen `ingested_at`: det enda icke-deterministiska fältet är borttaget så att två workspaces från
samma snapshot är logiskt radekvivalenta rakt av.
**Hård regel (rev 3.1):** efter lyckad ingest ska SV-03:s samtliga beslutsinputs (belopp, datum,
kategori + `category_context`, betalsätt, `vat_registered`, `bookkeeping_method`,
`default_payment_method`, `sie_series`, taxonomy_version, verksamhetsgrupper, låsstatus) och SV-04:s
identitetsinput (`display_name`, `org_number`, `sie_series`, `declaration_year`) kunna läsas ur
**DuckDB + masterdata utan att snapshot-JSON öppnas igen**. Det är vad "DuckDB på riktigt" betyder;
ett test i §4 bevisar det fält för fält mot snapshoten.
Datum lagras som DATE (aldrig VARCHAR), belopp som BIGINT öre, booleans som BOOLEAN — DuckDB:s
egen guidance mot "allt VARCHAR". Ingen tabell speglar en Django-modell rakt av; det är
snapshotens fält, inget mer.

## 3. INGESTION- OCH QUERY-KONTRAKT

- **Tillstånd.** `Workspace::create(path)` **vägrar en befintlig sökväg** (`AlreadyExists`) — den
  får aldrig återanvända eller skriva över en gammal evidensdatabas; intern `ensure_schema()` får
  vara idempotent men `create()` är det inte. Den skapar filen, schemat och `workspace_meta` (1 rad)
  — ett tomt workspace är ett giltigt tillstånd. `Workspace::open(path)` verifierar `workspace_meta`
  (exakt 1 rad, `workspace_schema` == stödd version, annars läsbart fel `SchemaMismatch`) och
  accepterar **0 eller 1** `snapshot_meta`-rad; >1 ⇒ `Corrupt`. Operationer som kräver ingesterad
  snapshot (readback, `inspect-workspace`, senare SV-03) kräver exakt 1 rad och ger annars explicit
  `NotIngested`. Så kan både ett tomt workspace och rollback-evidensen (0 rader) öppnas och
  inspekteras korrekt.
- **Digest = exakt Djangos kontrakt, ingen "ekvivalent".** Django: top-level `generated_at`
  exkluderas → `json.dumps(sort_keys=True, separators=(",", ":"), ensure_ascii=False)` → UTF-8 →
  SHA256; `accounting_snapshot` skriver digesten till stderr. SV-02D låser: `ingest(raw_json:
  &[u8])` i `workspace.rs` beräknar digesten från **råa snapshot-bytes** — `serde_json::Value`
  (objektnycklar i BTreeMap ⇒ samma ordning som Pythons `sort_keys` för dessa ASCII-nycklar) →
  ta bort top-level `generated_at` → `serde_json::to_vec` (kompakt, inga blanksteg, ingen
  ASCII-escapning av icke-ASCII, samma kontrollteckenescapning) → **SHA256 via DuckDB:s inbyggda
  `SELECT sha256(?::BLOB)`** (kanoniska bytes som BLOB-parameter, hex-digest tillbaka) — ingen egen
  SHA256-implementation, ingen `sha2`-crate; typad parse sker separat via oförändrad
  `snapshot::parse_snapshot` (NOT TO TOUCH). Kan detta inte passera lokalt ⇒ **STOPP**, inte
  fallback. **Golden-test:** Rust-digesten
  == Djangos digest för SK-05-goldens snapshot och för Testgårdens snapshot (stderr-värdet
  incheckat bredvid fixturen). Avvikelse ⇒ testfel, aldrig "nästan lika"; ingen andra
  kanonisering. Alternativet (digesten skickas in som explicit verifierat värde) används bara om
  golden-testet visar att bytesekvivalens inte kan garanteras — då är det ett STOPP-beslut, inte en
  tyst fallback.
- `ingest`: **en transaktion**; alla tabeller eller ingen. Batchad insert via `Appender` per tabell
  (Transaction derefar till Connection, så Appender kan användas inom transaktionen); ingen
  rad-för-rad-SQL. 21 kvitton motiverar ingen bulkpipeline-abstraktion — Appender är
  standard-API:et, inte en ny arkitektur.
- **Appender-/transaktionskontrakt:** varje Appender: `append_rows…` → **`flush()?`** → drop.
  duckdb-rs dokumenterar att constraint-fel kan ytas antingen under append (intern flush) eller vid
  explicit `flush()`, och att fel från implicit flush i `Drop` går förlorade — därför: första felet
  (append **eller** flush) avbryter vägen; är alla appends Ok är explicit `flush()` obligatorisk;
  **alla** resultat kontrolleras innan `commit()`; minsta fel ⇒ `rollback()` och läsbart fel. Ingen
  commit efter något fel.
- Re-ingest av samma `snapshot_sha256` ⇒ no-op (`AlreadyIngested`); annan sha i ett workspace
  med 1 rad ⇒ fel "workspace tillhör annan snapshot" — aldrig tyst överskrivning.
- Läsning: alla frågor som bär kontraktsordning har **explicit `ORDER BY`** (`selector_position`
  för kvitton, `snapshot_position` för inkomster, `seq` för audit). `preserve_insertion_order`
  lämnas på default; ordningen får aldrig bero på den.
- Ingen egen trådpool ovanpå DuckDB; `threads` default.

## 4. TESTKONTRAKT — riktiga databastester (≈ 15, mäts)

Det särskilda testet (låst ordagrant från styrningen): skapa `temp/accounting-test.duckdb` → ingestera
**Testgårdens riktiga snapshot** (dumpad från SK-05B-kunden via `accounting_snapshot`, incheckad som
`fixtures/snapshots/testgarden-2026-1.1.json` med sha256) → stäng anslutningen → öppna filen igen →
läs tillbaka → exakta radantal (2 properties, 21 receipts, 6 income_entries, 0 audit) → exakta belopp
i öre (t.ex. skördare-reservdelen 6 000 000 öre total / 1 200 000 moms; öresutjämning +30 och −20
öre) → ordering/source keys (`receipt:<pk>` i `selector_position`-ordning == snapshotens ordning) →
**(Raden "motorn kör från dessa rader" tillhör SV-03/LIVE-E2E:s kontrakt, inte SV-02D CLOSE:
SV-02D bevisar snapshot → riktig DuckDB → close → reopen → typad/query-readback → exakta
antal/belopp/ordning/source keys. SV-03 lägger sedan till samma riktiga workspace → AccountingCase →
AccountingDecision; LIVE-E2E lägger ihop kedjan.)**

Övriga: `create` skriver riktig fil (storlek > 0, header läsbar) och `workspace_meta` har 1 rad ·
`open` på tomt workspace ⇒ Ok, readback ⇒ `NotIngested` · `create` på befintlig sökväg ⇒
`AlreadyExists`, filen orörd · schema-DDL idempotent · **negativt Appender-test (REAL DB):** BEGIN tx
→ append rader med ett constraint-fel (dubblett-`source_key`) → felet får ytas under append/intern
flush **eller** vid explicit `flush()` — testet kräver: fel observerat, ingen commit sker, rollback,
close, reopen, 0 rader i alla snapshot-tabeller, `workspace_meta` intakt (inte att felet kommer på
ett visst anrop) · **inga sidokanaler:** efter ingest reproduceras varje SV-03-/SV-04-input fält för
fält ur DuckDB (`entity_context`, `entity_operations`, `category_context`-kolumnerna, låsstatus) och
jämförs mot snapshoten; en snapshot 1.0 utan `category_context` ger NULL, aldrig false ·
digest-golden via `sha256(BLOB)` mot Djangos stderr-värde · re-ingest samma sha = no-op · annan sha = fel ·
`open` på fil med annan `workspace_schema` = `SchemaMismatch` · två `snapshot_meta`-rader
(injicerade via rå SQL i testet) ⇒ `Corrupt` · digest-golden mot Djangos stderr-värde för båda
fixturerna · BIGINT round-trip på max-öre-värden
(`i64::MAX`-nära) · DATE round-trip · NULL-hantering (`ordinal_number` legacy NULL sorterar sist
via `selector_position`, inte via SQL-NULL-regler) · `ORDER BY`-frågan ger samma ordning som
`receipts_for_year` för SK-05-goldens sex kvitton · workspace kan raderas och återskapas från
samma snapshot **logiskt radekvivalent / samma deterministiska fingeravtryck** (DuckDB-filens fysiska
bytes behöver inte vara identiska) · ingen f64 i något `*_ore`-fält (typkontroll i Rust +
`typeof()`-assert i SQL).
**Identitet:** workspacet bevarar snapshotens `id`/`source_key` (`receipt:<pk>`, `income:<pk>`)
exakt; samma exakta snapshot ⇒ samma logiska radinnehåll och ordning; en nyprovisionerad
Testgården kan ha andra databas-pk/source_keys — SIEverk **omnumrerar dem aldrig**.

FORBIDDEN i testerna: mock-DuckDB, in-memory-only som enda bevis, trait-repository med
fake-implementation, `assert_called_once`-liknande attrapper.

## 5. ARTEFAKTER / EVIDENS

Mats/GitHub levererar: `cargo test`-output med testnamnen, filen `temp/accounting-test.duckdb`
sha256 + storlek, `SELECT count(*)` per tabell, `DESCRIBE`/`SHOW TABLES`-dump, ingestionens
`snapshot_sha256`, build-identitet (git SHA, `Cargo.lock`-sha256), Windows + Ubuntu.

## 6. FILES TO TOUCH / NOT TO TOUCH

TOUCH: `Cargo.toml` (`[package] version = "0.4.0"` = SIEverks egen version; vid DATE A **båda** raderna `duckdb = { version = "=<PINNED>", features = ["bundled", "chrono"] }` och `chrono = "=<PINNED KOMPATIBEL VERSION>"`; vid DATE B endast `duckdb = { version = "=<PINNED>", features = ["bundled"] }`; de två versionsnumren (paket vs crate) är olika saker; aldrig `sha2`; **inga versioner pinnas i denna docs-errata**), `Cargo.lock`, `src/workspace.rs` (ny),
`src/lib.rs`, `src/main.rs` (subkommando `ingest --snapshot <json> --workspace <file>` +
`inspect-workspace`), `tests/workspace_real_db.rs` (ny), `fixtures/snapshots/testgarden-2026-1.1.json`
(ny, från SK-05B-kunden), `.github/workflows/ci.yml` (cache + bygge), `.gitignore` (`temp/`, `*.duckdb`),
`docs/SIE-NOTES.md`, `README.md`.
NOT TO TOUCH: `money.rs`, `snapshot.rs`, `chart.rs`, `sru.rs`, `ruleset.rs`, parser-/validator-
moduler, `data/**`, `masterdata/**`, `fixtures/*.se`, `fixtures/snapshots/*` befintliga.

## 7. BEVISAR / BEVISAR INTE · GO/NO-GO

**Bevisar:** riktig DuckDB-fil på disk med lagrad schemaversion, transaktion med kontrollerade
Appender-flushar, batchad ingestion, SQL med explicit ordning, reopen, rollback, i64-öre utan float,
exakt Django-digest, bevarade snapshot-identiteter. **Bevisar inte:** något om konteringsregler (SV-03), SIE-bytes (SV-04), prestanda vid
tusentals kvitton (mäts när det finns), licens.
**SV-02D PRESPEC rev 3.1 = FINAL ARCHITECTURE LOCK · dependency-lock avsiktligt kvar · BUILD NO-GO
tills SV-02 CLOSED, crate-versionen pinnad och DATE-beslutet (A/B) bevisat lokalt · SV-03 NO-GO tills
SV-02D CLOSED.**
