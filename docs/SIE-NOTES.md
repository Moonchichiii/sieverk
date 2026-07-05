# SIE format notes — the one-screen version

Working notes for parsing SIE 4 files. Synthetic fixtures in `fixtures/`.
Official spec: sie.se (PDF, worth downloading once — but this covers week one).

## File anatomy

Plain text, **one tag per line**, tags start with `#`. Fields are separated by
spaces/tabs; strings containing spaces are wrapped in `"..."` (embedded quotes
are escaped as `\"`). Line endings are traditionally CRLF. `{ }` braces appear
in two roles: object lists inside `#TRANS` rows, and block delimiters around a
voucher's transaction rows.

**Encoding: `#FORMAT PC8` = IBM codepage 437.** Not UTF-8, not Latin-1.
Decode bytes → CP437 → String before doing anything else. Caveat: some real
programs emit ISO-8859-1 or UTF-8 despite the tag, so a hardened reader
eventually sniffs (valid UTF-8? BOM?) before assuming. Fixtures here are
byte-authentic CP437.

## Tags that matter for SIETYP 4, roughly in file order

| Tag | Meaning |
|---|---|
| `#FLAGGA 0` | "has this file been imported yet" flag |
| `#PROGRAM "name" ver` | producing software |
| `#FORMAT PC8` | encoding declaration (always PC8) |
| `#GEN 20260705` | generation date, YYYYMMDD |
| `#SIETYP 4` | file type 1–4 (see below) |
| `#FNAMN "Company AB"` | company name |
| `#ORGNR 999999-9999` | org number — **one per file**, always |
| `#RAR 0 20260101 20261231` | fiscal year; 0 = current, -1 = previous |
| `#KPTYP EUBAS97` | chart-of-accounts type (BAS variant) |
| `#VALUTA SEK` | currency |
| `#KONTO 1930 "Företagskonto"` | account definition |
| `#SRU 1930 7281` | tax-form field mapping (skip for now) |
| `#DIM` / `#OBJEKT` | dimensions/objects, e.g. cost centres (skip for now) |
| `#IB 0 1930 125000.00` | opening balance (balance accounts, 1xxx–2xxx) |
| `#UB 0 1930 180000.00` | closing balance |
| `#RES 0 3011 -45000.00` | year result per account (result accounts, 3xxx+) |
| `#VER A 1 20260312 "text"` | voucher header: series, number, date, text |
| `#TRANS 1930 {} -1250.00` | transaction row inside a `{ }` block |
| `#RTRANS` / `#BTRANS` | added/removed correction rows (later edge case) |
| `#KSUMMA` | optional CRC checksum, appears twice (skip for now) |

**Spec rule:** readers must *ignore unknown tags*, never crash. Collect them
as warnings instead — this is also just good parser hygiene.

## Sign conventions (the part everyone gets wrong once)

- In `#TRANS`: **positive = debit, negative = credit.**
- A voucher is valid iff its TRANS amounts **sum to exactly zero.**
- In `#RES`: revenue accounts (3xxx) show up **negative** (credit balance).
- Amounts always use `.` as decimal separator, typically 2 decimals.
- **Money is never f64.** Use `rust_decimal` or integer öre (i64). One
  accumulated float error in an accounting tool is one too many.

## SIE types (why "4" is the only one you care about)

- **Type 1:** year-end balances only. **Type 2:** + period balances.
- **Type 3:** + object/profit-centre balances.
- **Type 4:** transactions — every voucher of the year.
  - "4E" (export) = full file: balances **and** vouchers (like `minimal_valid.se`).
  - "4I" (import) = vouchers only, from a support system into accounting
    software — this is what a future SkogsKvitto export would produce.
  - Both say `#SIETYP 4` in the file; the E/I distinction is about content.

## Suggested parse order

1. Read bytes → decode CP437 → lines.
2. Tokenizer: line → `(tag, Vec<field>)`, respecting quotes and `{ }`.
3. Header pass: FLAGGA/PROGRAM/FORMAT/SIETYP/FNAMN/ORGNR/RAR → `Metadata`.
4. `#KONTO` → `HashMap<u32, Account>` (account numbers fit in u32; keep the
   raw string too — some files have non-numeric oddities).
5. `#VER` + following `{ ... }` block → `Voucher { rows: Vec<Trans> }`.
6. Validation pass: per-voucher zero-sum, dates inside `#RAR`, TRANS accounts
   declared in `#KONTO`, duplicate voucher numbers per series.

## Fixtures

- `minimal_valid.se` — SIE 4E-style: metadata, 7 accounts, IB/UB/RES,
  two balanced vouchers. Org.nr is fictional (999999-prefixed).
- `invalid_unbalanced.se` — one voucher off by 100.00. Your validator's
  first catch: `VOUCHER_NOT_BALANCED: A-1 sums to -100.00`.

## Definition of done, week one

`cargo run -- inspect-sie fixtures/minimal_valid.se` prints type, company,
year, account/voucher/row counts, `Status: Valid` — and the unbalanced fixture
prints `VOUCHER_NOT_BALANCED` with the 100.00 difference. Nothing else.
No DuckDB, no SolidJS, no Django contact. Those come after this works.
