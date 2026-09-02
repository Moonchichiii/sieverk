# SV-02 — dataaudit + förspec, rev 3.1 FINAL LOCK (PRESPEC, ingen kod): masterdata i SIEverk

*2026-09-02. Repo: sieverk. Bas: `main` = `c89194e5ea0e9f6f480f43128e79033506507d65` (SV-01b CLOSED),
version 0.2.0, `sieverk-20260902-1049.zip` sha256 `54fe329b…4fe91` — verifierad byte-identisk mot
SV-01b-källan (28 filer exkl. `target/`), 84 `#[test]`-funktioner räknade i källan (STATIC REVIEW:
ingen cargo-körning i sandboxen). Indata från SkogsKvitto: `main` =
`8b73fe1488073fc7507ab96886896843d7c3e793` (SK-05B + CodeQL/Mypy-follow-up = slutlig SK-05B CLOSED:
lokalt 1164 passed · 10 skipped · 0 failed, CI/Security/CodeQL gröna, SARIF 55 → 55, rent träd —
`17f37fa` var mellansteget, inte closure), taxonomi 1.0 (51 kategorier), snapshot schema 1.1.
Rev 2: baseline rättad; M1 omskriven efter BAS egen K1-tabell; A7/M4 rättade — BAS publicerar en
NE-koppling för K1; licensavsnittet A5 återger BAS uttryckliga villkor för den betalda
maskinläsbara produkten och ingenting mer. **Rev 3 (kirurgisk):** Mastermatris-kontraktet
amenderas till **v1.2** (`docs/mastermatris-v1.2-amendment.md` — profilchart + draft/approved-läge;
v1.1 superseded för SV-02-bygget) och SIE-planens D13 amenderas; rå BAS-filnamn rättade efter M1;
källa (`source_id`) skild från `must_include_origin` i chart-schemat och testkontraktet; M2a:s
publiceringsregel förtydligad utan juridiska slutsatser. M1 och M4 är faktamässigt LOCKED och
öppnas inte igen utan ny motsägande källa. **Rev 3.1 (lock-patch, endast slutliga
kontraktsblockerare):** draft-nedgradering uppfyller V10 (deterministiskt `villkor` **och**
`kontrollfråga`); V14 gjord sann — inga wall-clock-fält i trackade artefakter; två explicita
masterdata-rötter (publik syntetisk / real lokal) utan implicit fallback; `sru.rs` = enbart
load/validering av masterdata, all `#SRU/#TAXAR`-formatmekanik ut ur SV-02; SK-TX01:s exakta
projektion låst; arbetsboken heter `Mastermatris_v1.2.xlsx` från start. Läses tillsammans med:
SkogsKvitto source-of-truth `docs/SIE-plan-2026-09.md` (D13–D17, SV-02-raden — finns inte lokalt i
SIEverk); `docs/mastermatris-v1.1-schema.md` (V1–V14, E1–E6 — finns byte-identiskt i båda repon,
tillsammans med `docs/mastermatris-v1.2-amendment.md`); SkogsKvitto source-of-truth
`docs/SK-05B-forspec.md` (SCENARIO 2026 — finns inte lokalt i SIEverk).*

*SV-02 är masterdata: chart, provenance, SRU, momsregler, motkonton, ruleset, generator,
genererade artefakter och kontraktstester. **Ingen DuckDB** (SV-02D), ingen motor (SV-03), ingen
writer (SV-04), ingen SkogsKvitto-refactor. Rust-evidens körs av Mats/GitHub, aldrig här.*

---

## 1. DATAAUDIT — vad som faktiskt finns och vad som saknas

| # | Underlag | Status i verkligheten | Konsekvens |
|---|---|---|---|
| A1 | `masterdata/Mastermatris_v1.1.xlsx` (skapas nu som `masterdata/real/Mastermatris_v1.2.xlsx`) | **FINNS INTE** i sieverk (repot har `Cargo.*`, `docs/`, `fixtures/`, `src/`, `rust-toolchain.toml`). Arbetsboken har aldrig skapats; Vibeke har inte fyllt något. | SV-02 måste **skapa** arbetsboken med genererade flikar (`Taxonomi`, `Värdelistor`) och förifylld `Konton` — och kan inte nå `Godkänd`-status utan konsult. Se DECISION M3. |
| A2 | `masterdata/taxonomy-1.0.json` (schema §1) | **FINNS INTE**; exportscriptet `scripts/export_taxonomy.py` finns inte heller i SkogsKvitto. Men SK-01B:s golden `apps/core/tests/golden/taxonomy_v1_0.json` innehåller alla 51 kategorier med de fyra beslutsflaggorna (`requires_business_share`, `investment_risk`, `vat_check`, `sensitive`), grupp och `extra_question`. `income_types` (SK-02) och `payment_methods` (SK-03) finns som modellenum i kod, inte i golden. | Se DECISION M5 (export-script vs konverterad golden). |
| A3 | `tools/mastermatris_gen.py`, `data/*.json`, `fixtures/masterdata/` | **FINNS INTE** (D17 är beslutat, inte byggt). | Hela SV-02-bygget. |
| A4 | Vibekes K1-lista (PDF 2026-08-11) | 47 konton, alla följer BAS-klass/siffra (1110…8999). Vilka som är standard-BAS respektive bransch-/fria konton (kandidater: 3410, 3420, 3456, 3493, 4470, 5171, 5180, 8414, 1364, 1624, 2354) avgörs **maskinellt mot BAS 2018 för K1-tabellen**, inte ur minnet — 1973 Skogskonto t.ex. *finns* i BAS. | K1-listan = `must_include`-minimum (D15) och källa för namnen på de nummer som inte finns i K1-tabellen (fria konton). |
| A5 | BAS-kontoplaner och villkor | bas.se listar **tre** aktuella kontoplaner, alla gratis PDF/XLS mot nyhetsbrevsanmälan: **BAS 2026** — "för alla typer av företag *utom* de som upprättar förenklat årsbokslut"; **BAS 2018 för K1** — "för enskilda näringsidkare som upprättar förenklat årsbokslut – K1", märkt *Fullständig*; **BAS 2018 för K1 Mini** (ett konto per bokslutsrad). Betald produkt: "BAS-kontoplanen 2026 i JSON-format", 4 000 kr ex moms; villkoren (Mats har läst dem) säger att API-nyckeln gäller tills BAS tillgängliggör nästa maskinläsbara version, att nyttjanderätten till redan erhållen maskinläsbar kontoplan är "i tiden obegränsad" (4.2), att redovisnings-/affärssystem får integrera och vidarelicensiera den genom integrationen (4.3), och att villkoren kan ändras med 30 dagars varsel. Inget lovar gratis 2027-version. Det gäller den **betalda** produkten; den gratis XLS-filens vidaredistribution i ett publikt repo är inte reglerad av dessa villkor. | K1-profilens auktoritativa BAS-bas är **BAS 2018 för K1**, inte BAS 2026 (M1). Inga andra juridiska slutsatser än de uttryckliga villkoren (M2). Köp av 4 000-kr-produkten görs **inte** förrän BAS svarat på K1-frågan. |
| A6 | "Full lantbrukskontoplan (Ludvig & Co, enskild firma)" (D13) | Branschkontoplan framtagen av LRF Konsult/Ludvig & Co, distribuerad **inuti** bokföringsprogram (Björn Lundén, Edison, Visma: "Lantbruk", "Lantbruk Förenklat årsbokslut"); forum: inte tillgänglig utanför kundrelation. Ingen publik nedladdning, ingen licens. | **INTE CLEARAD för incheckning i sieverk.** Den kan inte vara `Konton`-källa i SV-02. Se DECISION M1. |
| A7 | SRU/fältkoder | **Rättat i rev 2:** BAS publicerar, utöver INK2–4, två NE-kopplingar, däribland "NE — Inkomst av näringsverksamhet, Enskilda näringsidkare – förenklat årsbokslut, K1-regler" (PDF + XLS) med riktiga fyrsiffriga fältkoder mot BAS-konton (t.ex. 7200→B1, 7210→B2, 7280→B9). Dokumentet anger dock "Konton i BAS Förenklat årsbokslut 2023" — giltighet för deklarationsår 2026 är **inte** verifierad. | `sru-2026.json` får innehålla NE-K1-rader **först när giltigheten för 2026 är verifierad** (källa + datum + sha256 i `sources`); osäker giltighet ⇒ `unresolved`/varning, aldrig `valid_from=2026` för att sidan finns idag. SIE 4I kräver inte `#SRU`; SRU-01 kvarstår för produktionsklar SRU. |
| A8 | Momsregler / business share / motkonton / kontantmetod | Regler E1–E6 är **PROPOSED**, ingen är SOURCE VERIFIED eller CONSULTANT REVIEWED (schema §5). `Momsregler`/`Motkonton`/`Redovisningsfall` är tomma (A1). Snapshoten bär `vat_amount` (källa/detekterad moms) separat från avdragsrätt; `payment_method` default `unknown`; `bookkeeping_method` i AccountingProfile. | SV-02 levererar **strukturen** och Vibekes förifyllning som `Utkast`; SV-03 får koda E1–E6 först som IMPLEMENTABLE. Se M3 för hur kedjan ändå kan köras på fredag utan att ljuga om status. |
| A9 | Taxonomitäckning | 51 kategorier (skog 11, mark 11, djur 10, odling 10, base 9), `DEFAULT_CATEGORY` finns, SCENARIO 2026 träffar 18 av dem + `Annat / osäkert`. | Täckningsluckor får bara varna (schema §4); okänd kategori i ett fall = BUILD FAILURE (V1). |
| A10 | D3/#VER-numrering, importfeedback | Öppet (GATE-0-rest). | Påverkar SV-04, inte SV-02. |

**Auditens slutsats (rev 2):** SV-02 kan byggas nu, men bara om tre saker låses: chart-källan
byts från Ludvig & Co till **BAS 2018 för K1** som K1-auktoritet med K1-listan som must_include och
BAS 2026 som cross-check (M1), distributionsomfånget begränsas till profilens konton tills BAS
svarat (M2), och arbetsboken får en uttrycklig **draft-status** som motorn respekterar (M3). Utan M3
kan fredagens kedja inte köras utan att bryta V5/V9/V13.

---

## 2. DECISIONS TO LOCK

- **M1 — Chart-källa (rev 2).** K1-profilens auktoritativa BAS-bas = **BAS 2018 för K1**
  (bas.se: "för enskilda näringsidkare som upprättar förenklat årsbokslut – K1", *Fullständig*,
  gratis XLS) — inte BAS 2026, som BAS uttryckligen avgränsar till företag som *inte* upprättar
  förenklat årsbokslut. Vibekes 47 konton = `must_include` + bransch-/skogsanpassning och källa
  för hennes kontonamn. **BAS 2026** används enbart som **cross-check** av generella konton/
  förändringar (nya/strukna nummer, namnändringar), aldrig som K1-auktoritet utan källa som
  uttryckligen stödjer det. Ludvig & Co-kontoplanen används inte (A6). Generatorn auditerar
  profilens ~60–120 konton mot **både** K1-tabellen och relevanta 2026-förändringar; skiljer sig
  ett konto som motorn behöver (finns i den ena, saknas/annat namn i den andra) ⇒ **BUILD FAILURE
  utan explicit `source_decision`-rad** i `Konton` (`källa` + motivering) — ingen automatisk
  merge. `sources` i artefakten: `"BAS 2018 K1": {name:"BAS 2018 för K1 (fullständig)",
  publisher:"BAS-intressenternas Förening", reference:"bas.se/kontoplaner", retrieved, sha256}`,
  `"BAS 2026": {…, role:"cross-check"}`, `"K1-lista": {name:"Förslag på enkel kontoplan för
  skogsägare som bokför enligt K1", author:"Vibeke (redovisningskonsult)", received:"2026-08-11",
  sha256}`. **Mats-fråga till BAS före köp:** "Är den maskinläsbara BAS 2026-produkten avsedd även
  för enskilda näringsidkare som tillämpar K1/förenklat årsbokslut, eller ska BAS 2018 K1
  fortfarande användas som K1-kontoplan?" — 4 000-kr-produkten köps inte före svaret.

- **M2 — Distributionsomfång och provenance (rev 2, M2a behållen som försiktighetsprincip).**
  `chart-lantbruk-k1-2026.1.json` innehåller **profilens konton** — K1-listans 47 + de konton
  ruleset/motkonton/momsregler faktiskt refererar (moms 2610/2640/2650, öresutjämning 3740,
  bank/kassa/eget uttag, bokslutskonton) — typiskt 60–120 rader, aldrig hela BAS-tabellen. Rå
  BAS-XLS (K1 2018 och 2026) och den **riktiga genererade artefakten** ligger under
  `masterdata/real/**` som är **gitignorerad/lokal** tills publiceringsrätten är löst; endast
  syntetiska genererade artefakter är trackade (`fixtures/masterdata/synthetic/generated/`);
  sieverk-repot är publikt. Ingen text i repo, README eller artefakt får påstå att
  redistribution är juridiskt clearad — provenance anger källa, hämtdatum, sha256 och
  "villkor: ej bekräftade av BAS" tills svar finns; svaret arkiveras i `masterdata/README.md`. De
  uttryckliga villkoren för den betalda maskinläsbara produkten (A5) återges, inga slutsatser
  utöver dem. Konton utanför profilen i retur-SIE ⇒ `CONSULTANT_INTRODUCED_ACCOUNT`-warning
  (redan designat).
  **Engineering-regel tills BAS svarat (rev 3, ingen juridisk slutsats):** "licens ej bekräftad"
  clearar inte publicering av en BAS-härledd profilartefakt i det publika repot. Därför: rå BAS-XLS
  = aldrig commit; full BAS = aldrig commit; den BAS-härledda profilartefakten beskrivs aldrig som
  cleared; **så länge rätten att publicera även subsetet är oklar hålls REAL masterdata lokal/
  opublicerad för LIVE-E2E** (`masterdata/real/` gitignorerad, artefakter genereras lokalt), medan
  publik CI kör mot **juridiskt säkra/syntetiska contract-fixturer** (`fixtures/masterdata/synthetic/`:
  påhittade kontonamn på BAS-strukturens nummerintervall + K1-listans nummer med Vibekes namn —
  hennes lista är vår). **Två explicita rötter (rev 3.1), ingen implicit fallback:** generatorn
  och `inspect-masterdata` tar alltid explicit `--root`/`--input`/`--output`; TRACKED PUBLIC =
  `fixtures/masterdata/synthetic/**` (syntetisk arbetsbok/fixtur, `generated/`, contract-tester);
  REAL LOCAL/GITIGNORED = `masterdata/real/**` (riktig arbetsbok, BAS-källfiler, `generated/`).
  Publik CI regenererar/testar **endast** synthetic-roten; Mats regenererar/testar real-roten för
  LIVE-E2E. Alternativet — privat repo/licens löst före publicering — beslutas separat.
  Fredagens REAL lokala LIVE-E2E får aldrig blandas ihop med "masterdata är clearad för publik GitHub".

- **M3 — Draft-status är ett kontrakt, inte en genväg.** Arbetsboken och alla artefakter bär
  `review_status ∈ {draft, approved}`. Generatorn kör alltid V1–V4, V6–V8, V10–V12, V14 (struktur
  + referensintegritet = BUILD FAILURE). V5/V9/V13 (`Godkänd`, `accounting_reviewer`, godkänt
  exempel per Automatic-fall) är BUILD FAILURE i `--approved`-läge och **nedgraderar** i
  `--draft`-läge: varje fall med `automation=Automatic` utan godkännande skrivs deterministiskt som
  `automation = Conditional`, med `villkor = "Ej accounting_reviewer-godkänd för automatisk
  kontering."` om tomt och `kontrollfråga = "Regeln är inte konsultgranskad."` om tom — befintliga
  ifyllda värden behålls; **båda** fälten uppfyller alltid V10; Rust nedgraderar aldrig (en draft-fil
  med Automatic är ett laddningsfel). Motorn (SV-03) läser
  `review_status` och stämplar varje `AccountingDecision` och manifestet med `ruleset_status:
  "draft"`; SV-04:s writer får i draft-läge bara producera *preliminär* fil (redan planens
  output-läge 2). **Final pack kräver `approved`.** Så kan hela kedjan köras fredag på riktiga
  Testgården-data utan att något beslut låtsas vara granskat.

- **M4 — SRU (rev 3.1).** `sru.rs` = **load + validering av `SruTable`-masterdata, inget annat**:
  ingen `#SRU`, ingen `#TAXAR`, ingen SIE-tokenisering eller -skrivning i SV-02 (formatmekaniken
  går till SRU-01 eller en uttrycklig senare writer-dropp; SIEverks egen SIE-NOTES säger redan att
  `#SRU` hoppas över tills vidare; SIE 4I blockeras inte). `sru-2026.json` får innehålla
  BAS NE-K1-kopplingens rader **endast** efter att giltigheten för deklarationsår 2026 verifierats
  (dokumentet är märkt "BAS Förenklat årsbokslut 2023"); varje rad bär `source`, `source_version`,
  `valid_from` och `verified_at`. Oklar giltighet ⇒ raden skrivs som `unresolved` (ingen kod) och
  byggrapporten varnar; ingen rad märks `valid_from=2026` för att sidan finns idag. SIE 4I
  blockeras inte av SRU; produktionsklar SRU = SRU-01.

- **M5 — Taxonomiexport.** *M5a (rek.)*: minimalt `scripts/export_taxonomy.py` i SkogsKvitto
  (egen mikro-dropp **SK-TX01**: 1 script + 1 test; ingen produktkod rörs). **Exakt projektion
  (rev 3.1, LOCKED):** per kategori exporteras ENDAST `name`, `group`, `requires_business_share`,
  `investment_risk`, `vat_check`, `sensitive`, `extra_question`; **FÖRBJUDET** i exporten:
  `ai_guide`, `review_message`, `export_tab`, `object_hint`, `examples`, `risk_level` (presentation/
  AI-taxonomi får aldrig läcka in i accounting-masterdata). `income_types` = `IncomeType.values`
  + label + legacy-flagga, `payment_methods` = `PaymentMethod.values` + label. Testet jämför
  **fält för fält** mot kanonisk taxonomi/SK-01B-golden (`len == 51`, varje kategori exakt de sju
  fälten med goldens värden, inget förbjudet fält förekommer, enumvärden == modellens), inte
  objektlikhet mot den presentationsrika goldens. *M5b*:
  sieverk-generatorn läser SkogsKvitto-goldens sha256-pinnade kopia och enumvärdena skrivs för
  hand — avråds (V2 skulle validera mot avskrift, inte kod).

- **M6 — Scope-avgränsning.** Planens SV-02-rad säger "parser lär sig `#KPTYP/#DIM/#OBJEKT`".
  Det är writer-/round-trip-mekanik och flyttas till SV-04 så SV-02 förblir ren masterdata
  (inga ändringar i tokenizer/metadata/vouchers/validator).

- **M7 — Granskare.** Vibeke är `accounting_reviewer` för `Konton`-roller, momsregler och
  motkonton; hon har själv sagt att hon vill tipsa om en andra konsult för redovisningsdelen.
  `approved` kräver `granskad_av` + roll (V9). Bokas parallellt med SV-02D/SV-03; blockerar inte
  fredagens draft-kedja.

---

## 3. CONTRACT (låses med M1–M7; masterdata-kontraktet = Mastermatris schema **v1.2**)

**Chart-princip (v1.2):** `LANTBRUK_K1` är ett **komplett och verifierat chart för den stödda
profilen** — enskild näringsidkare, skogsbruk, lantbruk, gårdsverksamhet, K1/förenklat årsbokslut
där tillämpligt — **inte** hela BAS och inte AB/HB/förening/alla branscher. Retur-SIE måste ändå
kunna möta konsultintroducerade konton utanför profilen utan krasch
(`CONSULTANT_INTRODUCED_ACCOUNT`).

```
TRACKED / PUBLIC
masterdata/
  README.md                       provenance, licensstatus ("rights: not confirmed by BAS"), hämtdatum, sha256 per källa
  taxonomy-1.0.json               SkogsKvitto-export (M5a, SK-TX01 — exakt projektion)
fixtures/masterdata/synthetic/
  Mastermatris_v1.2-synthetic.xlsx  syntetisk arbetsbok (schema v1.2): påhittade kontonamn + K1-listans nummer
  generated/                      chart-synthetic-k1-2026.1.json · sru-synthetic.json · vat-rules · counter-accounts
                                  · ruleset-synthetic-2026.1.json · examples-synthetic-2026.1.json
                                  (sieverk-chart/1, -sru/1, -vat/1, -counter/1, -ruleset/1 — samma scheman som real)
tools/
  mastermatris_gen.py             uv run --with openpyxl; --root <dir> --workbook <xlsx> --out <dir> --draft|--approved;
                                  V1–V14; deterministisk; ingen implicit rot
REAL / LOCAL / GITIGNORED
masterdata/real/
  Mastermatris_v1.2.xlsx          den riktiga arbetsboken (skapas nu, direkt enligt schema v1.2 — ingen v1.1-fil har funnits)
  bas-2018-k1.xlsx                auktoritativ K1-tabell (BAS 2018 för K1, fullständig), generatorns K1-input
  bas-2026-crosscheck.xlsx        endast cross-check av generella konton/förändringar, aldrig K1-auktoritet
                                  (ingen fil får heta "bas-2026-enskild-firma": BAS avgränsar 2026-planen från förenklat årsbokslut)
  generated/                      chart-lantbruk-k1-2026.1.json · sru-2026.json · vat-rules-2026.1.json
                                  · counter-accounts-2026.1.json · ruleset-2026.1.json · examples-2026.1.json
  build-report.txt                otrackad byggrapport: riktig tidsstämpel, git SHA, Python-version, varningar
src/
  chart.rs                        ChartProfile{chart_id, version, framework, entity_type, profile_scope, sources[], review_status, accounts[]}
                                  Account{number, name, kind, source_id, source_version, must_include, must_include_origin,
                                          source_decision: Option<{reason, decided_by, decided_at}>, roles}
                                  AccountRole{active,user_selectable,engine_proposable,return_sie_allowed,closing_account,business_groups}
                                  — source_id ∈ sources[] (vem äger kontots nummer/namn: BAS_2018_K1 | BAS_2026 | VIBEKE_K1_LIST),
                                    must_include_origin ∈ {VIBEKE_K1_LIST, RULESET, ENGINE} (varför kontot ingår i profilen);
                                    de två är olika saker: 1973 → source BAS_2018_K1 + origin VIBEKE_K1_LIST; ett fritt
                                    skogskonto som saknas i K1-tabellen → source VIBEKE_K1_LIST + origin VIBEKE_K1_LIST;
                                    source_decision obligatorisk när K1-tabell och 2026-cross-check skiljer sig (M1)
  sru.rs                          SruTable: load + validering av masterdata ENDAST (ingen #SRU/#TAXAR, ingen SIE-mekanik)
  ruleset.rs                      AccountingRuleset{ruleset_version, taxonomy_version, chart, review_status, cases[], vat_rules[], counter_accounts[]}
  lib.rs / main.rs                exponera load-funktioner; nytt CLI-subkommando `inspect-masterdata --root <dir>` (exit 1 vid fel)
tests/chart_contract.rs           (eller in-module) — se §5; körs mot synthetic-roten i CI, mot real-roten lokalt
.github/workflows/ci.yml          regenerera synthetic + `git diff --exit-code fixtures/masterdata/synthetic/generated/`
Cargo.toml                        [package] version = "0.3.0"; INGA nya beroenden (serde/serde_json räcker; xlsx läses av Python-generatorn)
```

**Determinism (rev 3.1, V14 sann):** trackade genererade artefakter innehåller **inget
wall-clock-fält** — `_header` = `{generated: "AUTO-GENERATED — DO NOT EDIT", workbook,
workbook_sha256, taxonomy_version, generator_version, review_status, profile_scope}`; `generated_at`
är struken ur headern. Riktig byggtidpunkt finns bara i den otrackade `build-report.txt`.

Load-time-invarianter i Rust (körs varje gång datafilerna läses, oavsett header): V1 kategori
finns i taxonomy-1.0, V2 enumvärden finns, V3 refererat konto finns/aktivt/proposable, V4 unika
fyrsiffriga nummer med rätt klass, V8 högst en default per (källa, kategori). Trasig datafil ⇒
läsbart fel, aldrig panik (parsers-never-panic-regeln). Pengar förekommer inte i masterdata utom i
exempel (`expected_lines` som ören-strängar `"1250.00"` → `Ore`).

**Bevisar:** att det finns ett versionerat, provenienssäkert och för profilen komplett chart
LANTBRUK_K1 med K1-listan som verifierat minimum och varje konto ägt av en namngiven källa; att varje regelrad pekar på giltigt konto och giltig
taxonomikategori; att draft aldrig kan bli Automatic; att generatorn är deterministisk (V14) och att
Rust vägrar trasiga datafiler utan panik. **Bevisar inte:** att någon redovisningsregel är
korrekt (E1–E6 kvarstår PROPOSED/draft), SRU-täckning för NE, BAS licens/publiceringsrätt (extern),
något om DuckDB/motor/SIE-bytes.

## 4. FILES TO TOUCH / NOT TO TOUCH (sieverk)

TOUCH (exakt, trackade filer — staga aldrig brett): `Cargo.toml` (version), `src/lib.rs`,
`src/main.rs`, `src/chart.rs` (ny), `src/sru.rs` (ny), `src/ruleset.rs` (ny),
`tests/chart_contract.rs` (ny), `tools/mastermatris_gen.py` (ny), `tools/tests/` (generatorns
självtester, ny), `masterdata/README.md` (ny), `masterdata/taxonomy-1.0.json` (ny, från SK-TX01),
`fixtures/masterdata/synthetic/Mastermatris_v1.2-synthetic.xlsx` (ny),
`fixtures/masterdata/synthetic/generated/*.json` (ny), `fixtures/masterdata/synthetic/invalid/*`
(en trasig fixtur per V-regel, ny), `.github/workflows/ci.yml`, `.gitignore` (`masterdata/real/`),
`README.md`, `docs/masterdata.md` (ny),
`docs/mastermatris-v1.2-amendment.md` (ny — kopieras även till SkogsKvitto `docs/` i samma
dokumentdropp som D13-amendmentet), `docs/snapshot-contract.md` (endast status/hänvisning).
**`masterdata/real/**` är GITIGNORED och stagas ALDRIG** — ingen `data/`-rot finns.
NOT TO TOUCH: `src/money.rs`, `src/snapshot.rs`, `src/tokenizer.rs`, `src/metadata.rs`,
`src/accounts.rs`, `src/vouchers.rs`, `src/validator.rs`, `fixtures/*.se`, `fixtures/snapshots/**`,
`Cargo.lock` utöver versionsbump (inga nya beroenden), SkogsKvitto (utom SK-TX01 om M5a).

## 5. TESTKONTRAKT (`chart_contract`, ≈ 25 — mäts)

Alla 47 K1-listans konton finns med `must_include=true` **och `must_include_origin=VIBEKE_K1_LIST`**
(inte `source=K1-lista`: standardkonton har `source_id=BAS_2018_K1`, bara nummer som saknas i
K1-tabellen har `source_id=VIBEKE_K1_LIST`) · varje konto har `source_id` som finns i `sources[]`
och `source_version` · konto som skiljer sig mellan K1-tabell och 2026-cross-check utan
`source_decision` ⇒ laddningsfel · bokslutskonton (7821, 7830, 8999)
`user_selectable=false` ∧ `return_sie_allowed=true` · inaktivt konto föreslås aldrig · konto
utanför profilen i retur-SIE ⇒ `CONSULTANT_INTRODUCED_ACCOUNT`, aldrig fel · helt okänt nummer ⇒
samma warning · ingen regelrad pekar på saknat konto (och datafil med sådan rad ⇒ läsbart
laddningsfel) · okänd taxonomikategori i datafil ⇒ laddningsfel (D8) · varje SRU-rad pekar på
giltigt konto, inga överlapp · inga dubblettnummer per chart-version · klass matchar första siffran
· `review_status=draft` ⇒ inget fall är Automatic · `approved`-fil utan `granskad_av` vägras ·
`taxonomy_version` i ruleset == i taxonomy-1.0 · determinism: två laddningar ger samma
`Debug`/JSON-serialisering · CLI `inspect-masterdata --root fixtures/masterdata/synthetic/generated`
exit 0, exit 1 på varje trasig fixtur under `fixtures/masterdata/synthetic/invalid/` (en per V-regel)
· generatorns Python-självtest: samma arbetsbok ⇒ samma bytes (V14), varje V-regel har ett negativt
fall. **`cargo test --all-targets` är självförsörjande mot trackade syntetiska fixturer och får
aldrig kräva `masterdata/real/**`.**

## 6. EVIDENSRITUAL (ny fast form från SV-02)

```
STATIC REVIEW (Claude, sandbox)        källa/diff/beroenden/fixturer/kontrakt/testkälla räknad — inga körpåståenden
EXECUTED LOCALLY BY MATS (Windows)     cargo fmt --all --check · cargo clippy --all-targets --all-features -- -D warnings
                                       · cargo test --all-targets            (självförsörjande, synthetic only)
                                       SYNTHETIC (trackat):
                                       · uv run --with openpyxl tools/mastermatris_gen.py --root fixtures/masterdata/synthetic
                                           --workbook fixtures/masterdata/synthetic/Mastermatris_v1.2-synthetic.xlsx
                                           --out fixtures/masterdata/synthetic/generated --draft
                                       · git status --short rent efter regenerering
                                       · cargo run -- inspect-masterdata --root fixtures/masterdata/synthetic/generated
                                       REAL (lokalt, gitignored, separat evidens):
                                       · uv run --with openpyxl tools/mastermatris_gen.py --root masterdata/real
                                           --workbook masterdata/real/Mastermatris_v1.2.xlsx --out masterdata/real/generated --draft
                                       · cargo run -- inspect-masterdata --root masterdata/real/generated
                                       · git status --short visar INGET under masterdata/real/
EXECUTED BY GITHUB (Ubuntu + Windows)  build · test · clippy · regenerate synthetic + git diff --exit-code
                                       fixtures/masterdata/synthetic/generated/ · cargo-audit
```
BEFORE = 84 tester (räknade i källa; Mats bekräftar med `cargo test`-output). EXPECTED AFTER =
84 + N, N ≈ 25 — mäts av Mats. Inga siffror i efterrapporten utan kommandoutdata.

## 7. GO/NO-GO

Review 2 (2026-09-02): **M1 LOCKED · M4 LOCKED · M5a LOCKED · M6 LOCKED · M7 LOCKED · M3 LOCKED
när schema v1.2-texten matchar (rev 3 levererar den) · M2a CONDITIONAL — publiceringsrätt extern.**
**SV-02 PRESPEC rev 3.1 = FINAL LOCK (errata 2026-09-02 införda) · NÄSTA: docs-lock-commit →
SK-TX01 (egen mikro-dropp) → SV-02 BUILD · SV-02D = FINAL ARCHITECTURE LOCK, dependency-lock
avsiktligt kvar · SV-03/SV-04 NO-GO · ENGINE NO-GO · BAS-köp NO-GO tills K1-svar.**
Kritisk väg till fredag: M1–M7 låsta i kväll ⇒ SV-02 byggs draft-läge onsdag kväll/torsdag morgon
(Mats kör cargo) ⇒ SV-02D torsdag ⇒ SV-03 draft-beslut torsdag kväll ⇒ SV-04 fredag ⇒ LIVE-E2E-01
med `ruleset_status: draft` och preliminär `.se`. Godkänd (non-draft) kedja kräver Vibeke/M7 och
D3-svaret — det är sant och ska stå så i rapporten.
