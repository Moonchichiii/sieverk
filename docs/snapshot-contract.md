# SkogsKvitto → Ledger Engine: Snapshot Contract v1.3

**Status:** accepted; v1.3 (2026-09-01) adds schema `"1.1"` and supersedes
rule §5.5 — see §9. The engine reads both `"1.0"` and `"1.1"`
(`src/snapshot.rs`, SV-01). The Django snapshot builder is SK-05 in
`SIE-plan-2026-09.md`. This document exists so the Rust
sidecar and the Django app agree on the boundary *before* either side writes
integration code.

Grounded in the actual codebase (`current-Skogskvitto-clean-20260705-1409`),
not in assumed models. Every field below names its Django source.

---

## 1. The unit of a snapshot: (entity, income year) — not (property, year)

**The tension found in the code:** SIE files carry exactly one `#ORGNR` — they
are scoped to the legal entity. But `TaxYear` is mid-migration to being scoped
per **Property** (`UniqueConstraint(property, year)`, with "temporary nullable
for migration; tightened in next step" comments in both `TaxYear.property` and
`Receipt.property`). A premium user with two fastigheter will have two
`TaxYear` rows for 2026, each independently lockable.

**Resolution — no Django change required:** the migration is correct and can
proceed as designed. Property-level year containers are the right *product*
model (per-fastighet kassabok, per-fastighet locking, and skogsavdrag is
computed per fastighet anyway — downstream, by the accountant, per the
`IncomeEntry` docstring). The *snapshot* simply aggregates one level up:

- One snapshot = one `(owner, year)` = the whole näringsverksamhet.
- It contains **all** of that owner's properties and their TaxYears for the year.
- `property` becomes an attribute on every receipt/income row — which maps
  naturally onto an SIE dimension (resultatenhet/kostnadsställe, `#DIM`/
  `#OBJEKT`) in a future export. The `Receipt.area` field ("Kostnadsställe"
  per its own help_text) is a second, orthogonal dimension candidate.

**The entity itself already exists in the code.** `OperationProfile` is
OneToOne with User and carries `org_number` (optional, normalized on save,
"belongs to the operation, not the identity"), `farm_name`, `county`, and the
`has_skog/has_djur/has_odling/has_mark` flags. No new Business/Organization
model is needed. Note for the far future: for enskild firma the org.nr *is*
the personnummer — the field stays optional, and an SIE export draft without
it simply omits `#ORGNR` and warns, rather than ever requiring it.

## 2. Lock semantics at entity level (derived, never stored)

`TaxYear.status` is per property. The snapshot reports per-property lock state
and derives the entity view:

- `all_properties_locked` = every included TaxYear has `status == "locked"`.
- Which outputs are permitted at which lock state is governed by the
  three-mode rule (§5.6): analysis always, preliminary drafts before lock
  (loudly labeled), final export packs only when everything is locked. A hard
  lock-gate on *all* SIE output would be circular — the draft is what the
  accountant reviews *before* the year gets locked.
- `ArchiveEvent` (ARCHIVE_LOCKED / ARCHIVE_UNLOCKED, with `occurred_at`)
  already provides the audit trail; the snapshot carries the events verbatim.

## 3. The JSON shape

Conventions: UTF-8, ISO-8601 dates/timestamps, **all money as strings with
exactly 2 decimals in SEK** (`"1250.00"`) — never JSON numbers, never one or
three decimals, no thousands separators, no whitespace; the engine rejects
anything else with the field path (`receipts[3].total_amount`) and parses the
rest losslessly into `Ore(i64)` (exact integer öre — the engine's money
type; `rust_decimal` was evaluated and rejected, see Cargo.toml). Empty
Django strings (`""`) normalize to
`null` at this boundary. Internal integer PKs are included solely so
reconciliation reports can point back at specific rows.

```json
{
  "schema_version": "1.0",
  "generated_at": "2026-07-05T14:09:00+02:00",
  "source": { "app": "skogskvitto", "environment": "prod" },

  "entity": {
    "owner_id": 42,
    "display_name": "Kråksjö gård",
    "org_number": null,
    "operation": ["skog", "mark"],
    "county": "Kronoberg"
  },

  "income_year": 2026,

  "lock": {
    "all_properties_locked": false,
    "declaration_year": 2027
  },

  "properties": [
    {
      "id": 7,
      "name": "Kråksjö säteri",
      "slug": "kraksjo-sateri",
      "is_default": true,
      "tax_year": { "id": 55, "status": "open", "locked_at": null }
    }
  ],

  "receipts": [
    {
      "id": 1001,
      "property_id": 7,
      "ordinal_number": 17,
      "date": "2026-03-12",
      "vendor": "OKQ8",
      "entry_type": "expense",
      "area": "fordon",
      "category": "Drivmedel",
      "total_amount": "1250.00",
      "vat_amount": "250.00",
      "rounding_amount": "0.00",
      "net_amount": "1000.00",
      "note": null,
      "confirmed_at": "2026-03-12T18:22:11+01:00",
      "has_image": true
    }
  ],

  "income_entries": [
    {
      "id": 501,
      "property_id": 7,
      "income_type": "timber_sale",
      "date": "2026-04-18",
      "buyer_name": "VIDA",
      "description": "Slutavverkning skifte 3",
      "amount_ex_vat": "45000.00",
      "vat_amount": "11250.00",
      "amount_inc_vat": "56250.00",
      "invoice_number": "A-2231",
      "payment_date": "2026-05-02",
      "document_count": 2
    }
  ],

  "audit_chain": [
    {
      "kind": "event",
      "event_type": "submitted_to_accountant",
      "property_id": 7,
      "occurred_at": "2027-01-15T09:00:00+01:00",
      "recipient_role": "redovisningskonsult",
      "note": null
    },
    {
      "kind": "document",
      "document_type": "accountant_report",
      "property_id": 7,
      "received_date": "2027-02-20",
      "original_filename": "arsbokslut_2026.pdf",
      "checksum_sha256": "…",
      "storage_backend": "b2"
    }
  ]
}
```

## 4. Field mapping (Django source → snapshot)

| Snapshot field | Django source | Notes |
|---|---|---|
| `entity.display_name` | `OperationProfile.farm_name`, fallback `Property(is_default).name`, fallback email local-part | never expose raw email in reports |
| `entity.org_number` | `OperationProfile.org_number` | optional; already normalized by `normalize_org_number` |
| `entity.operation` | `OperationProfile.has_*` flags | matches `GROUP_KEYS` in `core/taxonomy.py` |
| `lock.declaration_year` | `Receipt.declaration_year` logic | always `year + 1` |
| `properties[].tax_year` | `TaxYear(property, year)` | status/locked_at verbatim |
| `receipts[].ordinal_number` | `Receipt.ordinal_number` | löpnummer within (property, year) — the natural verifikationsnummer for a future 4i draft; nullable for legacy rows |
| `receipts[].area` | `Receipt.Area` choices | kostnadsställe → SIE dimension candidate |
| `receipts[].category` | `Receipt.category` | controlled vocabulary from `core/taxonomy.py` (Regelmatris, Status="Godkänd"); Rust maps names → BAS accounts, unknown names → warning, never a guess |
| `receipts[].rounding_amount` | `Receipt.rounding_amount` | signed öresutjämning, ±0.99 kr constraint; maps to BAS 3740 in a future export — do **not** fold into net |
| `receipts[].net_amount` | `Receipt.net_amount` (GeneratedField) | = total − vat − rounding; engine re-verifies and warns on drift rather than trusting blindly |
| `income_entries[].*` | `IncomeEntry` | full three-way split (ex/VAT/inc) already exists — engine verifies ex + vat = inc |
| `audit_chain` | `ArchiveEvent` + `AccountantReport` | metadata + checksums only |

## 5. Engine-side interpretation rules (Rust, not Django)

1. **Income = `income_entries` only.** Receipts are expense-side underlag.
   Any `Receipt.entry_type != "expense"` (income/trip/monthly/work_log/other)
   is passed through but flagged for review in reports — never auto-mapped to
   3xxx accounts. This prevents double-counting against `IncomeEntry`.
2. **Category → BAS mapping lives entirely in the engine** (its own versioned
   table keyed on taxonomy names). Django never learns BAS numbers; the
   taxonomy stays "EN motor: describes, never branches" as its docstring says.
3. **No file bytes cross the boundary.** `has_image` / `document_count` /
   `checksum_sha256` only. Evidence stays in Cloudinary/B2, where
   `AccountantReport.storage_*` fields already point.
4. **The engine is read-only.** It consumes a snapshot and emits reports.
   It never writes to the Django database. Results come back as files/JSON
   that Django stores (`AccountantReport` with
   `document_type="generated_export"` already fits this).
5. **Ordinal numbers are source references, never final voucher numbers.**
   *(Series-per-property below is SUPERSEDED by v1.3 — see §9. Kept for
   history.)* Snapshots aggregate all properties, so `(property A, #17)` and
   `(property B, #17)` legitimately coexist. For SIE output the engine
   assigns one voucher **series per property** (A, B, C… in stable
   property-creation order), with `ordinal_number` as the number within the
   series — SIE series exist for exactly this, and it keeps every voucher
   traceable back to its SkogsKvitto ordinal. Deleted receipts leave gaps;
   the *final* export pack may renumber into unbroken series, and if it does,
   the report must include the old→new mapping table.
6. **Three output modes, three gates.** *Reconciliation/analysis:* runs on
   any lock state — running it before lock is its entire purpose.
   *Preliminary SIE-compatible draft:* allowed before lock, but the file and
   its report are unmistakably marked (PRELIMINÄR / Exportutkast / ej låst)
   and carry the full warning list — this is what goes to the accountant for
   review. *Final export pack:* requires `all_properties_locked == true`, is
   checksummed, and is what lands in Årsarkiv.
7. **`org_number` strictness scales with the mode.** Null is fine for
   reconciliation; a preliminary draft omits `#ORGNR` and emits a warning;
   the final pack requires it unless the user explicitly overrides — and the
   override itself is recorded in the report. The personnummer caution from
   §1 stands: the app never demands the number, the export mode does.

## 6. Out of scope for v1.0 (versioned door left open)

- **Trips/körjournal** — accountant-relevant (milersättning) but a clean
  v1.1 addition; the Trip model is rich (classification, rates, sources) and
  deserves its own mapping pass.
- Vehicles, scan-job internals, subscription/billing data: never.
- Skogsavdrag/skogskonto computation: explicitly not the product
  (per `IncomeEntry`'s own docstring) — the engine reports underlag, full stop.

## 7. Amendment log — v1.0 → v1.1 (external review, accepted)

The v1.0 open question (reconciliation on unlocked years) is answered by
rule §5.6: yes — that is reconciliation's whole point. The same review
surfaced the ordinal-collision risk (now rule §5.5, resolved with
series-per-property rather than global renumbering) and the org.nr strictness
ladder (rule §5.7). The audit-chain events now carry `recipient_role` instead
of the recipient's name — identity stays in Django, the snapshot only needs
to know an event happened and to whom in role terms.

**The contract is closed.** Anything further follows the version protocol
below and belongs in the repo's `docs/`, not in another review round.

## 8. Amendment log — v1.1 → v1.2 (documentation-only, money type)

§3 said the Rust side parses money into `rust_decimal`. The engine never did:
it uses `Ore(i64)` (exact integer öre) throughout, and `rust_decimal` was
explicitly rejected (edition-2024 lockfile drag — see the Cargo.toml
comment). The wire format is unchanged — money was always a two-decimal SEK
string — so `schema_version` stays `"1.0"`; only the prose describing the
consumer's parse target was wrong. No code changes on either side.

## 9. Amendment log — v1.2 → v1.3 (schema `"1.1"`, one series)

Grounded in the read-only code review of 2026-08-31 (`GATE-0-kodkontrakt.md`)
and the consultant's positions (`SIE-plan-2026-09.md`, D2–D4, D16).

1. **`schema_version` becomes `"1.1"`.** Additive only; the engine keeps the
   `"1.0"` parser and treats every 1.1-only field as absent (`None`) in 1.0
   files — never a default that looks filled in. Unknown extra fields are
   ignored (forward compatibility). Unknown `schema_version` is an error.
2. **Rule §5.5 superseded: one voucher series.** The consultant's position is
   that a single series matters most. The property travels as row metadata
   (`property_id`, which must reference `properties[]`) and, later and only
   behind a switch, as an optional `#DIM`. Voucher numbers are assigned per
   export under `voucher_number_strategy ∈ {ENGINE_ASSIGNED,
   IMPORTER_ASSIGNED}` — **open** until the consultant's real import test
   shows what the receiving program does with `#VER` numbers. The permanent
   identity of a row is its source key (`receipt:<pk>` / `income:<pk>`),
   carried in the export manifest; `SK-<ordinal>` stays the human label.
3. **New fields (all 1.1):**
   - `entity.taxonomy_version` (string, e.g. `"1.0"`) and
     `entity.accounting_profile { vat_registered, bookkeeping_method,
     default_payment_method, sie_series }` (strings; `unknown` is a legal
     value that the engine turns into review, never into a guess).
   - `receipts[].source_key`, `receipts[].payment_method`
     (`unknown | company_account | private | supplier_credit`), and
     `receipts[].category_context { requires_business_share, investment_risk,
     vat_check, sensitive } | null` — the four taxonomy flags that affect an
     accounting decision, nothing presentational (no `ai_guide`,
     `review_message`, `export_tab`, `extra_question`).
   - `income_entries[].source_key`. `income_type` gains the split values
     (`avverkningsratt`, `leveransvirke`, `grot`, `efterlikvid`, …);
     `timber_sale` stays valid as legacy and is flagged for manual split.
   - `entity.operation` keeps its 1.0 name (the plan documents call the same
     list *business_groups*).
4. **Two decimals is a hard rule** (§3), not a convention: the snapshot is
   machine-generated with `quantize(0.01)`, so any other format is drift and is
   rejected with the field path. The engine also re-checks
   `net = total − vat − rounding` per receipt and `ex + vat = inc` per income
   entry, naming the source key in the error.
5. **Output side is a draft.** The shape of `decisions.json`, `report.json`
   and `manifest.json` is documented only when the engine and writer exist
   (SV-03/SV-04); nothing about output is final in this version.

Reference fixtures: `fixtures/snapshots/minimal-1.0.json`,
`fixtures/snapshots/minimal-1.1.json` (synthetic: 999999-prefixed org.nr,
fictional names). The Django golden snapshot (SK-05) replaces `minimal-1.1`
as the reference once it exists.

---
*v1.3, 2026-09-01. Change protocol unchanged: bump `schema_version`, engine keeps
parsers for old versions until confirmed unused.*
