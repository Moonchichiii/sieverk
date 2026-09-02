# Mastermatris v1.1 — dataschema och verifieringsflöde

*v1.1, 2026-09-01 — godkänt som arbetskontrakt (D17 godkänt samma dag). Ändringar mot
v1.0: granskarroll i stället för personregel (M2/V9), strukturerad provenance i den
genererade kontoplanen (§3), och alla redovisningsregler i §5 markerade PROPOSED med
livscykel före SV-03. Planering, ingen kod. Gäller beslut D8, D13–D17 i
`SIE-plan-2026-09.md`. Mastermatrisen är den andra matrisen: Regelmatris v1.0 äger
kategorierna (`taxonomy.py`), Mastermatrisen äger hur en godkänd kategori bokförs.*

---

## 0. Principer

| # | Princip |
|---|---|
| M1 | **Refererar, äger inte.** Varje rad pekar på ett exakt kategorinamn ur SkogsKvittos taxonomiexport (eller ett `IncomeType`-värde). Okänd referens ⇒ BUILD FAILURE. Matrisen ändrar aldrig taxonomin. |
| M2 | **Bara godkända rader emitteras** (samma regel som Regelmatrisen). `Godkänd` kräver `granskad_av` **och** `granskad_roll = accounting_reviewer` — en teknisk roll, ingen personkontroll i generatorn. I v1 är båda fritext i arbetsboken; generatorn kontrollerar bara att de är satta och att rollen är rätt. |
| M3 | **Konto ur full master chart.** En regelrad får bara peka på ett konto som finns i `Konton` med `status=Godkänd`, `active=true` och — för Automatic/Conditional-fall — `engine_proposable=true`. |
| M4 | **UI-kategori ≠ konto.** En kategori kan ha flera redovisningsfall; högst ett är `default`. Utan strukturerade svar (GATE-0 §8B) väljer motorn default-fallet och lyfter alternativen som kontrollfråga. |
| M5 | **Okänt ⇒ Manual, aldrig fallback-konto.** Kategori utan godkänt fall är tillåten i bygget (rapporteras som täckningslucka) och blir `UNMAPPED_CATEGORY` i motorn. |
| M6 | **Exempel är tester.** Fliken `Tester` blir körbara fixturer; ett exempel som inte balanserar stoppar bygget. |
| M7 | **Deterministiskt bygge.** Samma arbetsbok ⇒ byte-identiska JSON-filer (sorterade nycklar, stabil radordning). CI regenererar och kräver ren git. |
| M8 | **Belopp som strängar med två decimaler** i alla genererade filer; generatorn räknar med `Decimal`. |

---

## 1. Indata: SkogsKvittos taxonomiexport

Generatorn läser aldrig `taxonomy.py` direkt. SkogsKvitto exporterar ett JSON-kontrakt
(script i `scripts/`, körs av Mats, checkas in i sieverk som `masterdata/taxonomy-1.0.json`):

```
{
  "schema": "skogskvitto-taxonomy-export/1",
  "taxonomy_version": "1.0",
  "generated_from": "apps/core/taxonomy.py",
  "categories": [
    {"name": "Skogsbilväg", "group": "skog",
     "requires_business_share": true, "investment_risk": true,
     "vat_check": false, "sensitive": false}
  ],
  "income_types": [{"value": "leveransvirke", "label": "Leveransvirke", "legacy": false}],
  "payment_methods": [{"value": "company_account", "label": "Företagskonto / företagskort"}]
}
```

- `categories` = exakt `CATEGORIES` med de fyra beslutspåverkande flaggorna (D16); SK-01B:s
  golden är samma sanning ur testets perspektiv — exporten och golden ska vara lika, testat.
- `income_types` finns först när SK-02 landat, `payment_methods` när SK-03 landat.
  **Konsekvens för ordningen:** utgiftsfall (kvitton) kan Vibeke fylla nu; inkomst- och
  motkontofall godkänns först när exporten innehåller värdena. Ingen "planerad" lista i
  generatorn — den validerar mot kod.

---

## 2. Arbetsboken `Mastermatris_v1.1.xlsx`

Alla dataflikar har kolumnerna `status` (`Utkast|Godkänd|Struken`), `granskad_av`,
`granskad_roll` (`accounting_reviewer|author`), `granskad_datum`, `kommentar` sist. Rubrikrad 1, data från rad 2, inga sammanslagna celler,
dropdowns via datavalidering mot referensflikarna. Två flikar är **genererade och låsta**
(`Taxonomi`, `Värdelistor`) — Vibeke fyller aldrig i dem.

### 2.1 `README`
Version, källor, statuslegend, arbetsordning (§5), kontaktpersoner.

### 2.2 `Taxonomi` *(genererad ur §1, låst)*
`kategori | grupp | requires_business_share | investment_risk | vat_check | sensitive`
— dropdown-källa för `Redovisningsfall.kategori`.

### 2.3 `Värdelistor` *(genererad, låst)*
`income_type`, `payment_method`, `automation` (`Automatic|Conditional|Manual`),
`riktning` (`ingående|utgående`), `avdragsrätt` (`full|ingen|manuell`), `kontoklass`
(`tillgång|skuld|eget_kapital|intäkt|kostnad`), `business_group` (`skog|djur|odling|mark|base`),
`bokföringsmetod` (`cash|invoice`).

### 2.4 `Konton` — full lantbrukskontoplan (D13–D15)

| Kolumn | Typ | Regel |
|---|---|---|
| `konto` | text, 4 siffror | unik per arbetsbok; klass måste matcha första siffran (1 tillgång, 2 skuld/EK, 3 intäkt, 4–7 kostnad, 8 finansiellt/resultat) |
| `namn` | text | obligatorisk |
| `kontoklass` | enum | |
| `active` | bool | default true |
| `user_selectable` | bool | får väljas av användaren i SkogsKvitto (framtida UI) |
| `engine_proposable` | bool | får föreslås av motorn |
| `return_sie_allowed` | bool | får förekomma i retur-SIE utan warning (bokslutskonton = true) |
| `closing_account` | bool | bokslutskonto (7821, 7830, 8999 …) |
| `business_groups` | text, `;`-separerad | tom = alla grupper |
| `must_include` | bool | true för raderna ur Vibekes K1-lista (D15) |
| `källa` | text | `K1-lista`, `Ludvig & Co`, `BAS 2026`, … — enkel text i arbetsboken; blir strukturerad provenance i artefakten (§3) |

### 2.5 `SRU`

`konto | sru_kod | blankett (NE) | giltig_från (taxeringsår) | giltig_till (tom = tills vidare)`
— ett konto får ha flera rader bara om giltighetsintervallen inte överlappar.

### 2.6 `Momsregler`

`momsregel_id (slug) | namn | riktning | tillåtna_satser (t.ex. 25 eller 25;12) | momskonto | avdragsrätt | villkor (fritext) | manual_review (bool)`
— `momskonto` ∈ `Konton`. Avdragsrätt `manuell` ⇒ fallet blir minst Conditional.

### 2.7 `Motkonton`

| Kolumn | Regel |
|---|---|
| `källa` | `receipt` eller `income` |
| `nyckel` | receipt: `payment_method`-värde; income: `betald|obetald` |
| `bokföringsmetod` | `cash`, `invoice` eller tom (= båda) |
| `konto` | ∈ `Konton`, eller tom om `automation=Manual` |
| `automation` | enum |
| `fråga` | obligatorisk när `automation ≠ Automatic` |

Exempel på rader Vibeke ska ta ställning till (inte förifyllda svar): `receipt/unknown` ⇒
Manual utan konto; `receipt/private`; `receipt/supplier_credit` under `cash`; `income/obetald`
under `cash` (bokförs alls vid registrering?).

### 2.8 `Redovisningsfall` — kärnan

| Kolumn | Typ | Regel |
|---|---|---|
| `case_id` | slug `kategori.variant` (t.ex. `skogsbilvag.underhall`, `skogsbilvag.ny_anlaggning`, `income.leveransvirke`) | unik |
| `källa` | `receipt` / `income` | |
| `kategori` | dropdown `Taxonomi` | obligatorisk när källa = receipt |
| `income_type` | dropdown `Värdelistor` | obligatorisk när källa = income |
| `är_default` | bool | högst en per (källa, kategori/income_type) |
| `automation` | enum | se V10 |
| `konto` | ∈ `Konton` | tom tillåten bara för Manual |
| `momsregel_id` | ∈ `Momsregler` | tom tillåten bara för Manual |
| `motkonto_regel` | `enligt_motkonton` (default) eller explicit konto | explicit konto ∈ `Konton` |
| `sru_override` | tom eller sru-kod | normalt tom (SRU följer kontot) |
| `villkor` | fritext | obligatorisk när `automation=Conditional` — *när gäller detta fall* |
| `kontrollfråga` | fritext | obligatorisk när `automation ≠ Automatic`; får återanvända taxonomins `extra_question` ordagrant |
| `beskrivning` | fritext | vad fallet betyder för användaren/konsulten |

### 2.9 `Tester` — Vibekes exempel blir fixturer

`exempel_id | case_id | indata (kolumner: total, moms, öresutjämning, payment_method, income_type, betald, bokföringsmetod, momsregistrerad) | förväntad_status | förväntade_rader (konto:debet|kredit per rad, en rad per cell-rad) | förväntade_findings | kommentar`

Ett exempel per Automatic-fall är minimikrav för `Godkänd` på fallet (V13).

---

## 3. Genererade artefakter (sieverk-repot, `data/`)

Alla filer inleds med `"_header": {"generated": "AUTO-GENERATED — DO NOT EDIT", "workbook": "Mastermatris_v1.1.xlsx", "workbook_sha256": "…", "taxonomy_version": "1.0", "tool": "mastermatris-gen/1", "generated_at": "…"}`.

| Fil | Innehåll |
|---|---|
| `masterdata/taxonomy-1.0.json` | kopia av §1 (indata, incheckad) |
| `data/chart-lantbruk-k1-2026.1.json` | `{schema:"sieverk-chart/1", chart_id:"LANTBRUK_K1", version:"2026.1", framework:"BAS", entity_type:"enskild_firma", sources:{"K1-lista":{name, version, reference}, …}, accounts:[{number, name, kind, active, user_selectable, engine_proposable, return_sie_allowed, closing_account, business_groups[], must_include, source:"K1-lista"}]}` — `källa`-texten blir nyckel i `sources`; `version`/`reference` fylls i `README`-fliken per källa, inte per konto, så frågan "varför finns kontot och ur vilken version" kan besvaras utan att belasta Vibekes rader |
| `data/sru-2026.json` | `{schema:"sieverk-sru/1", rows:[{account, sru, form:"NE", valid_from, valid_to|null}]}` |
| `data/vat-rules-2026.1.json` | `{schema:"sieverk-vat/1", rules:[{id, direction, rates:["25"], vat_account, deductibility, manual_review, condition}]}` |
| `data/counter-accounts-2026.1.json` | `{schema:"sieverk-counter/1", rows:[{source, key, bookkeeping_method|null, account|null, automation, question|null}]}` |
| `data/ruleset-2026.1.json` | `{schema:"sieverk-ruleset/1", ruleset_version:"2026.1", taxonomy_version, chart:{chart_id, version}, cases:[{case_id, source, category|null, income_type|null, is_default, automation, account|null, vat_rule|null, counter_account_rule, sru_override|null, condition|null, question|null, description}], coverage:{categories_total, categories_with_default, income_types_total, income_types_with_default}}` |
| `fixtures/masterdata/examples-2026.1.json` | `{examples:[{id, case_id, input{…}, expected_status, expected_lines[{account, debit|credit}], expected_findings[]}]}` |

Versioner: `chart.version` och `ruleset_version` följer `ÅÅÅÅ.n`; `taxonomy_version` följer
Regelmatrisen. Alla tre stämplas i varje `AccountingDecision` och i `manifest.json`.

---

## 4. Verifiering vid bygge

Generatorn är ett Python-verktyg (`tools/mastermatris_gen.py`, körs `uv run --with openpyxl`,
mönstret från `scripts/decrypt_backup.py`) i sieverk-repot (D17). **BUILD FAILURE**
(exit ≠ 0, ingen fil skrivs) vid:

| # | Regel |
|---|---|
| V1 | `Redovisningsfall.kategori` saknas i `taxonomy-1.0.json` (exakt strängmatch) |
| V2 | `income_type`/`payment_method` saknas i exporten |
| V3 | Refererat konto (fall, motkonto, momskonto, explicit motkonto) saknas i `Konton`, är `Struken`, `active=false`, eller `engine_proposable=false` för ett Automatic/Conditional-fall |
| V4 | Dubblett-`konto` i `Konton`; konto ej 4 siffror; klass motsäger första siffran |
| V5 | Något `must_include`-konto ur K1-listan saknas eller är inte `Godkänd` |
| V6 | SRU-rad pekar på okänt konto; överlappande giltighet för samma konto |
| V7 | Momsregel med sats utanför {25,12,6,0}, okänt momskonto eller ogiltig avdragsrätt |
| V8 | Fler än ett `är_default` per (källa, kategori/income_type) |
| V9 | `Godkänd` utan `granskad_av`, eller med `granskad_roll ≠ accounting_reviewer` |
| V10 | Automatic-fall utan konto, momsregel eller motkontoregel; Conditional utan `villkor` + `kontrollfråga`; Manual utan `kontrollfråga` |
| V11 | Dubblett-`case_id`, eller `case_id` som inte börjar med kategorins/incometypens slug |
| V12 | Exempel refererar okänt `case_id`, eller `Σ debet ≠ Σ kredit`, eller `total ≠ net + moms + öresutjämning` i indata |
| V13 | Automatic-fall utan minst ett `Godkänd` exempel |
| V14 | Två körningar på samma arbetsbok ger olika bytes (determinism självtest) |

**Varningar i byggrapporten** (stoppar inte): kategorier utan default-fall (täckning), konton
utan SRU, `Utkast`-rader kvar, momsregler som ingen använder.

**Load-time i Rust** (`chart.rs`/`ruleset.rs`, SV-02): samma referensintegritet (V1–V4, V8) körs
igen vid inläsning — datafilerna kan ha handredigerats trots headern — och testerna i
`chart_contract` bevisar att en trasig datafil ger ett läsbart fel, aldrig panik.

---

## 5. Motorlogik som inte är matrisdata — PROPOSED, inte låst

Skrivs i Rust (SV-03), inte i arbetsboken — så Vibeke slipper "programmera" i Excel.
Ingen av redovisningsreglerna nedan är verifierad ännu. Varje regel går

`PROPOSED → SOURCE VERIFIED (BAS/BFN/Skatteverket-referens angiven) → CONSULTANT REVIEWED (Vibeke eller annan redovisningskonsult) → IMPLEMENTABLE`

och får kodas i SV-03 först som `IMPLEMENTABLE`. Statusen förs i den här tabellen.

| # | Regel (PROPOSED) | Status |
|---|---|---|
| E1 | Momssats **härleds** ur belopp (25/12/6/0 med tolerans) och jämförs mot fallets tillåtna satser ⇒ `VAT_MISMATCH`/`UNRESOLVED_VAT` | PROPOSED |
| E2 | `vat_registered ≠ yes` ⇒ ingen ingående moms bokförs (bruttot på kostnadskontot), Conditional med fråga | PROPOSED |
| E3 | Öresutjämning ⇒ 3740 med tecken (kontot måste finnas i `Konton`; regeln är motorns) | PROPOSED |
| E4 | `bookkeeping_method=unknown` eller `payment_method=unknown` ⇒ motkontoradens `automation` överstyrs till Manual | PROPOSED |
| E5 | Verifikationsdatum: kvittodatum (`cash`, betalt) / `payment_date` (inkomst) | PROPOSED — beror på GATE-0 §13 |
| E6 | `supplier_credit` under kontantmetoden: bokförs vid registrering eller inte | PROPOSED — beror på GATE-0 §13 |

Formatmekanik är **inte** redovisningsregler och kräver ingen konsultgranskning: balans per
verifikation, `#KONTO`-deklaration av använda konton, CP437/PC8, CRLF, numrering enligt D3.

---

## 6. Arbetsordning med Vibeke

1. Mats: SK-01B mergad → taxonomiexport → arbetsbok med `Taxonomi`/`Värdelistor` genererade och `Konton` förifylld ur K1-listan (`must_include=true`) + vald källa för resten av kontoplanen (öppen fråga: Ludvig & Co-kontoplanens tillgänglighet/licens; BAS-kontoplanen är fritt användbar).
2. Vibeke: `Konton`-roller för skog/base, `Momsregler`, `Motkonton`, sedan `Redovisningsfall` för skog + base (utgifter), med ett exempel per Automatic-fall.
3. Andra redovisningskonsult (hon erbjöd sig att tipsa): `granskad_av` + `granskad_roll=accounting_reviewer` på redovisningsraderna — särskilt moms — och statusflytt E1–E6 i §5.
4. Mats: generator → byggrapport → tillbaka till 2 tills V1–V14 är gröna; täckningsluckor är tillåtna.
5. Efter SK-02/SK-03: inkomstfall och motkontorader kompletteras; ny `ruleset_version`.
6. Varje godkänd arbetsboksversion checkas in i sieverk (`masterdata/`) tillsammans med de genererade filerna i samma commit.

---

## 7. Öppna frågor (utöver GATE-0 §13)

- Källa och licens för den fulla lantbrukskontoplanen som fyller `Konton` utöver K1-listan.
- Vill Vibeke arbeta i Excel-arbetsboken direkt, eller i ett förenklat ark per flik som Mats slår ihop?
- SRU-blankett: räcker NE för alla första profilens användare (K1/enskild firma), eller behövs fler scheman i `SRU.blankett` från start?
- Ska `Tester`-exemplen anonymiseras ur Stefans riktiga 2026-data (bästa realism) eller vara helt fiktiva från start?
