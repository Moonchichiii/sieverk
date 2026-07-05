# SkogsKvitto → Ledger Engine: Snapshot Contract v1.1 (FINAL)

**Status:** accepted. Three amendments from external review (2026-07-05)
folded in — see §7. Still paper design on the Django side: the snapshot
builder remains a post-August-1 task. This document exists so the Rust
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
exactly 2 decimals in SEK** (`"1250.00"`) — never JSON numbers; the Rust side
parses straight into `rust_decimal`. Empty Django strings (`""`) normalize to
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
   Snapshots aggregate all properties, so `(property A, #17)` and
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

---
*v1.1 FINAL, 2026-07-05. Change protocol: bump `schema_version`, engine keeps
parsers for old versions until confirmed unused.*
