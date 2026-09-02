# Mastermatris schema v1.2 — amendment till `mastermatris-v1.1-schema.md` (rev 3.1 lock-patch, 2026-09-02)

*v1.2 är den auktoritativa specen för SV-02-bygget. v1.1 är **superseded** i de punkter nedan;
allt som inte nämns här gäller oförändrat från v1.1 (flikar, kolumner, V-reglernas innehåll,
§5 E1–E6 PROPOSED, §6 arbetsordning). Detta är en docs-kontraktskorrigering, inte produktkod.
Dokumentet checkas in i sieverk (`docs/`) och SkogsKvitto (`docs/`) i samma dokumentdropp.*

## A. Principer (ersätter v1.1 §0 M2 och M3)

- **M2 (v1.2): "Endast rader som klarar strukturell validering emitteras; approved-status styr
  automation, inte existens."** v1.1:s "bara godkända rader emitteras" är superseded. Ett
  `Utkast`-fall får emitteras i draft-läge (se C) — aldrig som `Automatic`.
- **M3 (v1.2): "Konto ur profilens chart."** v1.1:s "konto ur full master chart" är superseded.
  Chartet `LANTBRUK_K1` är **komplett och verifierat för den stödda profilen** — enskild
  näringsidkare, skogsbruk, lantbruk, gårdsverksamhet, K1/förenklat årsbokslut där tillämpligt —
  inte hela BAS, inte AB/HB/förening/alla branscher. Retur-SIE med konton utanför profilen ⇒
  `CONSULTANT_INTRODUCED_ACCOUNT`-warning, aldrig krasch (oförändrat).

## B. `Konton` (utökar v1.1 §2.4) och chart-artefakten (v1.1 §3)

Nya obligatoriska kolumner/fält per konto:

| Kolumn / fält | Regel |
|---|---|
| `source_id` | ∈ arbetsbokens `Källor`-flik (ny): `BAS_2018_K1` (auktoritativ K1-tabell "BAS 2018 för K1, fullständig"), `BAS_2026` (endast cross-check), `VIBEKE_K1_LIST` (Vibekes PDF 2026-08-11). **Vem äger kontots nummer/namn.** |
| `source_version` | t.ex. `2018`, `2026`, `2026-08-11` |
| `must_include` | bool (v1.1) |
| `must_include_origin` | ∈ {`VIBEKE_K1_LIST`, `RULESET`, `ENGINE`} — **varför kontot ingår i profilen**; obligatorisk när `must_include=true` |
| `source_decision` | tom, eller `reason | decided_by | decided_at` — **obligatorisk** när K1-tabellen och 2026-cross-checken skiljer sig för kontot (saknas i den ena, annat namn, struket). Ingen automatisk merge. |
| `källa` (v1.1) | superseded av `source_id`; får finnas kvar som läsbar text |

Exempel: `1973 Skogskonto` → `source_id=BAS_2018_K1`, `must_include_origin=VIBEKE_K1_LIST`. Ett
fritt skogskonto som saknas i K1-tabellen → `source_id=VIBEKE_K1_LIST`, `must_include_origin=VIBEKE_K1_LIST`.
Artefakten `chart-lantbruk-k1-<version>.json` bär `profile_scope`, `sources[]` (id, name, publisher,
reference, retrieved, sha256, `rights: "not confirmed by BAS"` tills svar) och per konto de fem
fälten ovan; top-level `sources[]` ensamt räcker inte.

## C. Bygglägen (ersätter v1.1 §4:s absoluta V5/V9/V13)

Generatorn körs med exakt ett läge; artefakternas `_header.review_status` och rulesetets
`review_status` bär läget.

```
--draft
  V1–V4, V6–V8, V10–V12, V14   HÅRDA (BUILD FAILURE, ingen fil skrivs)
  V5, V9, V13                  VARNING i byggrapporten
  Automatic-input              nedgraderas DETERMINISTISKT till Conditional så att V10 uppfylls:
                                 automation     = Conditional
                                 villkor        = "Ej accounting_reviewer-godkänd för automatisk kontering."
                                 kontrollfråga  = "Regeln är inte konsultgranskad."
                               (befintligt villkor/kontrollfråga i källraden behålls om ifyllda; saknas de
                               injiceras exakt texterna ovan — båda kontraktsfälten är ALLTID giltiga)
  Automatic i output           ALDRIG
  ruleset_status               "draft"  → motorn stämplar varje AccountingDecision och manifestet
  SIE-output (senare)          ENDAST PRELIMINÄR; final pack FÖRBJUDET

--approved
  V1–V14                       HÅRDA
  Automatic                    TILLÅTET endast här, och endast för fall med Godkänd + granskad_av
                               (roll accounting_reviewer) + minst ett Godkänt exempel (V13)
  ruleset_status               "approved"
```

Rust load-time (SV-02 `ruleset.rs`): en datafil med `review_status=draft` som innehåller ett
`Automatic`-fall ⇒ läsbart laddningsfel (kontraktsbrott), aldrig tyst nedgradering i Rust —
nedgraderingen sker i generatorn, deterministiskt (V14).

## D. Övrigt

- **V14 (determinism) är sann (rev 3.1):** trackade genererade artefakter innehåller **inget
  wall-clock-fält**. `_header` i v1.1 §3 ändras till `{generated, workbook, workbook_sha256,
  taxonomy_version, generator_version, review_status, profile_scope}` — `generated_at` och
  `tool`-tidsstämplar är strukna. Riktig byggtidpunkt, git SHA och miljö skrivs enbart till en
  otrackad byggrapport (`build-report.txt`/evidens).
- **Två explicita rötter:** TRACKED PUBLIC `fixtures/masterdata/synthetic/**` (syntetisk arbetsbok +
  `generated/` + contract-tester) och REAL LOCAL/GITIGNORED `masterdata/real/**` (riktig arbetsbok,
  BAS-källfiler, `generated/`). Generatorn tar explicit `--root/--workbook/--out`; ingen implicit
  fallback mellan rötterna. Publik CI kör synthetic; LIVE-E2E kör real.
- **Arbetsboken heter `Mastermatris_v1.2.xlsx`** från första fil — ingen v1.1-arbetsbok har funnits,
  ingen bakåtkompatibilitet att bevara; generator, header och docs refererar v1.2.
- **SRU-mekanik:** `sru.rs` i SV-02 laddar/validerar enbart SRU-masterdata; `#SRU/#TAXAR` och all
  SIE-formatmekanik ligger utanför SV-02 (SRU-01/senare writer-dropp).

- §3 artefakt-header får `"review_status": "draft"|"approved"` och `"profile_scope"`.
- Nytt filpar i `masterdata/real/`: `bas-2018-k1.xlsx` (auktoritativ K1-input) och
  `bas-2026-crosscheck.xlsx` (endast cross-check), gitignorerade med hela roten; ingen fil får heta
  "bas-2026-enskild-firma".
- Publiceringsregel tills BAS svarat (SV-02 M2a): REAL masterdata lokal/opublicerad
  (`masterdata/real/`), publik CI mot syntetiska contract-fixturer. Ingen juridisk slutsats.
- SRU-flik: raderna får `source`, `source_version`, `valid_from`, `verified_at`; NE-K1-rader
  endast efter verifierad 2026-giltighet, annars `unresolved` (SV-02 M4).
- §7 öppna frågor: "källa och licens för den fulla lantbrukskontoplanen" är **stängd** (Ludvig & Co
  används inte; profilchart enligt M3 v1.2); "räcker NE för profilen" kvarstår → SRU-01.
