//! Snapshot ingestion — the Django → engine boundary (docs/snapshot-contract.md).
//!
//! Two layers on purpose: serde reads JSON into a private raw DTO where money
//! is still a `String`; our own `convert` then walks that DTO with indices and
//! produces the typed `Snapshot` with `Ore`. That is what lets an error say
//! `receipts[3].total_amount` without a path-tracking dependency.

use std::collections::HashSet;
use std::fmt;

use chrono::NaiveDate;
use serde::{Deserialize, Deserializer};

use crate::money::Ore;

const SUPPORTED_SCHEMAS: [&str; 3] = ["1.0", "1.1", "1.2"];

/// Schema 1.2 (SK-SIE-BRIDGE-03): accounting identity + fiscal period.
const SCHEMA_ACCOUNTING_IDENTITY: &str = "1.2";
/// L-PERIOD (BRIDGE-01 §2.1, identical to Django): at most 18 calendar months.
const MAX_PERIOD_MONTHS: u32 = 18;

// ---------------------------------------------------------------------------
// Public, typed model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub schema_version: String,
    pub generated_at: String,
    pub source: Source,
    pub entity: Entity,
    /// Schema 1.2 only; `None` for 1.0/1.1 and for 1.2 with `null`.
    pub fiscal_period: Option<FiscalPeriod>,
    pub income_year: u16,
    pub lock: Lock,
    pub properties: Vec<Property>,
    pub receipts: Vec<Receipt>,
    pub income_entries: Vec<IncomeEntry>,
    pub audit_chain: Vec<AuditEvent>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Source {
    pub app: String,
    pub environment: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entity {
    pub owner_id: i64,
    pub display_name: String,
    pub org_number: Option<String>,
    pub operation: Vec<String>,
    pub county: Option<String>,
    /// Schema 1.1 only.
    pub taxonomy_version: Option<String>,
    /// Schema 1.1 only.
    pub accounting_profile: Option<AccountingProfile>,
    /// Schema 1.2 only; `None` for 1.0/1.1 and for 1.2 with `null`.
    pub accounting_identity: Option<AccountingIdentity>,
}

/// Legal form of the accounting entity (Contract 1.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityType {
    SoleTrader,
    LimitedCompany,
    TradingPartnership,
    LimitedPartnership,
    OtherLegalPerson,
}

impl EntityType {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "sole_trader" => Self::SoleTrader,
            "limited_company" => Self::LimitedCompany,
            "trading_partnership" => Self::TradingPartnership,
            "limited_partnership" => Self::LimitedPartnership,
            "other_legal_person" => Self::OtherLegalPerson,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::SoleTrader => "sole_trader",
            Self::LimitedCompany => "limited_company",
            Self::TradingPartnership => "trading_partnership",
            Self::LimitedPartnership => "limited_partnership",
            Self::OtherLegalPerson => "other_legal_person",
        }
    }
}

/// Kind of business identifier. F/FA tax status is never an input here.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdentifierKind {
    PersonalNumber,
    CoordinationNumber,
    OrganisationNumber,
}

impl IdentifierKind {
    fn parse(value: &str) -> Option<Self> {
        Some(match value {
            "personal_number" => Self::PersonalNumber,
            "coordination_number" => Self::CoordinationNumber,
            "organisation_number" => Self::OrganisationNumber,
            _ => return None,
        })
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::PersonalNumber => "personal_number",
            Self::CoordinationNumber => "coordination_number",
            Self::OrganisationNumber => "organisation_number",
        }
    }
}

/// The accounting identity exactly as sent by Django (schema 1.2). The values
/// are retained unchanged for the authorised export; `Debug` never prints the
/// legal name or the identifier.
#[derive(Clone, PartialEq, Eq)]
pub struct AccountingIdentity {
    pub entity_type: EntityType,
    pub legal_name: String,
    pub identifier_kind: IdentifierKind,
    /// Wire form `NNNNNN-NNNN`, validated (L-WIRE-VALID), never transformed.
    pub business_identifier: String,
}

impl fmt::Debug for AccountingIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountingIdentity")
            .field("entity_type", &self.entity_type)
            .field("identifier_kind", &self.identifier_kind)
            .field("legal_name", &format_args!("[REDACTED]"))
            .field("business_identifier", &format_args!("[REDACTED]"))
            .finish()
    }
}

/// The fiscal period exactly as sent (schema 1.2); never derived from
/// `income_year`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FiscalPeriod {
    pub start: NaiveDate,
    pub end: NaiveDate,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountingProfile {
    pub vat_registered: String,
    pub bookkeeping_method: String,
    pub default_payment_method: String,
    pub sie_series: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Lock {
    pub all_properties_locked: bool,
    pub declaration_year: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct Property {
    pub id: i64,
    pub name: String,
    pub slug: String,
    pub is_default: bool,
    pub tax_year: TaxYear,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaxYear {
    pub id: i64,
    pub status: String,
    pub locked_at: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Receipt {
    pub id: i64,
    /// Schema 1.1 only (`receipt:<pk>`).
    pub source_key: Option<String>,
    pub property_id: i64,
    pub ordinal_number: Option<u32>,
    pub date: String,
    pub vendor: Option<String>,
    pub entry_type: String,
    pub area: String,
    pub category: Option<String>,
    /// Schema 1.1 only.
    pub category_context: Option<CategoryContext>,
    /// Schema 1.1 only.
    pub payment_method: Option<String>,
    pub total_amount: Ore,
    pub vat_amount: Ore,
    pub rounding_amount: Ore,
    pub net_amount: Ore,
    pub note: Option<String>,
    pub confirmed_at: Option<String>,
    pub has_image: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CategoryContext {
    pub requires_business_share: bool,
    pub investment_risk: bool,
    pub vat_check: bool,
    pub sensitive: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomeEntry {
    pub id: i64,
    /// Schema 1.1 only (`income:<pk>`).
    pub source_key: Option<String>,
    pub property_id: i64,
    pub income_type: String,
    pub date: String,
    pub buyer_name: Option<String>,
    pub description: String,
    pub amount_ex_vat: Ore,
    pub vat_amount: Ore,
    pub amount_inc_vat: Ore,
    pub invoice_number: Option<String>,
    pub payment_date: Option<String>,
    pub document_count: u32,
}

/// The audit chain mixes two record kinds, told apart by `kind`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(tag = "kind")]
pub enum AuditEvent {
    #[serde(rename = "event")]
    Event {
        event_type: String,
        property_id: i64,
        occurred_at: String,
        #[serde(default)]
        recipient_role: Option<String>,
        #[serde(default)]
        note: Option<String>,
    },
    #[serde(rename = "document")]
    Document {
        document_type: String,
        property_id: i64,
        // Django stores these as nullable/blank (a report may be undated, a legacy
        // upload may lack a checksum); the snapshot sends null rather than inventing.
        #[serde(default)]
        received_date: Option<String>,
        #[serde(default)]
        original_filename: Option<String>,
        #[serde(default)]
        checksum_sha256: Option<String>,
        storage_backend: String,
    },
}

/// One error, first found, addressed by JSON path. `path` is empty for pure
/// JSON syntax/type errors, where serde_json's line/column message is the
/// address instead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotError {
    pub path: String,
    pub message: String,
}

impl fmt::Display for SnapshotError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.path.is_empty() {
            write!(f, "{}", self.message)
        } else {
            write!(f, "{}: {}", self.path, self.message)
        }
    }
}

impl std::error::Error for SnapshotError {}

pub fn parse_snapshot(bytes: &[u8]) -> Result<Snapshot, SnapshotError> {
    let raw: RawSnapshot = serde_json::from_slice(bytes).map_err(|e| SnapshotError {
        path: String::new(),
        message: format!("invalid snapshot JSON: {e}"),
    })?;
    convert(raw)
}

// ---------------------------------------------------------------------------
// Raw DTO — exactly the wire shape, money as text
// ---------------------------------------------------------------------------

/// Presence-aware field: absent key -> `None` (via `#[serde(default)]`),
/// `null` -> `Some(None)`, value -> `Some(Some(v))`. Decided inside the single
/// typed serde pass — no second JSON parse, no global `deny_unknown_fields`.
fn present<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    T::deserialize(deserializer).map(Some)
}

#[derive(Deserialize)]
struct RawSnapshot {
    schema_version: String,
    generated_at: String,
    source: Source,
    entity: RawEntity,
    #[serde(default, deserialize_with = "present")]
    fiscal_period: Option<Option<RawFiscalPeriod>>,
    income_year: u16,
    lock: Lock,
    properties: Vec<Property>,
    receipts: Vec<RawReceipt>,
    income_entries: Vec<RawIncomeEntry>,
    audit_chain: Vec<AuditEvent>,
}

#[derive(Deserialize)]
struct RawEntity {
    owner_id: i64,
    display_name: String,
    #[serde(default, deserialize_with = "present")]
    org_number: Option<Option<String>>,
    operation: Vec<String>,
    #[serde(default)]
    county: Option<String>,
    #[serde(default)]
    taxonomy_version: Option<String>,
    #[serde(default)]
    accounting_profile: Option<AccountingProfile>,
    #[serde(default, deserialize_with = "present")]
    accounting_identity: Option<Option<RawAccountingIdentity>>,
}

/// Raw identity: every field is text so an unknown value is our own error with
/// a JSON path and a fixed message, never a serde message echoing the value.
#[derive(Deserialize)]
struct RawAccountingIdentity {
    entity_type: serde_json::Value,
    legal_name: serde_json::Value,
    identifier_kind: serde_json::Value,
    business_identifier: serde_json::Value,
}

#[derive(Deserialize)]
struct RawFiscalPeriod {
    start: serde_json::Value,
    end: serde_json::Value,
}

#[derive(Deserialize)]
struct RawReceipt {
    id: i64,
    #[serde(default)]
    source_key: Option<String>,
    property_id: i64,
    #[serde(default)]
    ordinal_number: Option<u32>,
    date: String,
    #[serde(default)]
    vendor: Option<String>,
    entry_type: String,
    area: String,
    #[serde(default)]
    category: Option<String>,
    #[serde(default)]
    category_context: Option<CategoryContext>,
    #[serde(default)]
    payment_method: Option<String>,
    total_amount: String,
    vat_amount: String,
    rounding_amount: String,
    net_amount: String,
    #[serde(default)]
    note: Option<String>,
    #[serde(default)]
    confirmed_at: Option<String>,
    has_image: bool,
}

#[derive(Deserialize)]
struct RawIncomeEntry {
    id: i64,
    #[serde(default)]
    source_key: Option<String>,
    property_id: i64,
    income_type: String,
    date: String,
    #[serde(default)]
    buyer_name: Option<String>,
    description: String,
    amount_ex_vat: String,
    vat_amount: String,
    amount_inc_vat: String,
    #[serde(default)]
    invoice_number: Option<String>,
    #[serde(default)]
    payment_date: Option<String>,
    document_count: u32,
}

// ---------------------------------------------------------------------------
// Conversion boundary — where strings become öre and invariants are checked
// ---------------------------------------------------------------------------

fn err(path: impl Into<String>, message: impl Into<String>) -> SnapshotError {
    SnapshotError {
        path: path.into(),
        message: message.into(),
    }
}

/// Stricter than `Ore::parse` on purpose: the snapshot is machine-generated
/// with `quantize(0.01)`, so anything but exactly two decimals is drift.
fn is_two_decimal_money(s: &str) -> bool {
    let bytes = s.as_bytes();
    let start = usize::from(matches!(bytes.first(), Some(b'+') | Some(b'-')));
    let Some(dot) = s.find('.') else {
        return false;
    };
    let int_part = &bytes[start..dot];
    let dec_part = &bytes[dot + 1..];
    !int_part.is_empty()
        && int_part.iter().all(u8::is_ascii_digit)
        && dec_part.len() == 2
        && dec_part.iter().all(u8::is_ascii_digit)
}

fn money(path: &str, text: &str) -> Result<Ore, SnapshotError> {
    if !is_two_decimal_money(text) {
        return Err(err(
            path,
            format!("expected an amount string with exactly two decimals, got {text:?}"),
        ));
    }
    Ore::parse(text).ok_or_else(|| err(path, format!("amount out of range: {text:?}")))
}

fn sub(a: Ore, b: Ore) -> Option<Ore> {
    a.0.checked_sub(b.0).map(Ore)
}

fn add(a: Ore, b: Ore) -> Option<Ore> {
    a.0.checked_add(b.0).map(Ore)
}

fn convert(raw: RawSnapshot) -> Result<Snapshot, SnapshotError> {
    if !SUPPORTED_SCHEMAS.contains(&raw.schema_version.as_str()) {
        return Err(err(
            "schema_version",
            format!(
                "unsupported schema {:?} (supported: {})",
                raw.schema_version,
                SUPPORTED_SCHEMAS.join(", ")
            ),
        ));
    }
    if !(1000..=9999).contains(&raw.income_year) {
        return Err(err(
            "income_year",
            format!("expected a four-digit year, got {}", raw.income_year),
        ));
    }

    let (org_number, accounting_identity, fiscal_period) = identity_and_period(
        &raw.schema_version,
        raw.entity.org_number,
        raw.entity.accounting_identity,
        raw.fiscal_period,
    )?;

    let known_properties: HashSet<i64> = raw.properties.iter().map(|p| p.id).collect();

    let mut receipts = Vec::with_capacity(raw.receipts.len());
    for (i, r) in raw.receipts.into_iter().enumerate() {
        let at = |field: &str| format!("receipts[{i}].{field}");
        if !known_properties.contains(&r.property_id) {
            return Err(err(
                at("property_id"),
                format!(
                    "receipt:{} refers to property {} which is not in properties[]",
                    r.id, r.property_id
                ),
            ));
        }
        let total_amount = money(&at("total_amount"), &r.total_amount)?;
        let vat_amount = money(&at("vat_amount"), &r.vat_amount)?;
        let rounding_amount = money(&at("rounding_amount"), &r.rounding_amount)?;
        let net_amount = money(&at("net_amount"), &r.net_amount)?;
        let expected_net = sub(total_amount, vat_amount)
            .and_then(|x| sub(x, rounding_amount))
            .ok_or_else(|| err(at("net_amount"), "amount overflow while checking net"))?;
        if net_amount != expected_net {
            return Err(err(
                at("net_amount"),
                format!(
                    "receipt:{} net {net_amount} != total {total_amount} - vat {vat_amount} - rounding {rounding_amount} (= {expected_net})",
                    r.id
                ),
            ));
        }
        receipts.push(Receipt {
            id: r.id,
            source_key: r.source_key,
            property_id: r.property_id,
            ordinal_number: r.ordinal_number,
            date: r.date,
            vendor: r.vendor,
            entry_type: r.entry_type,
            area: r.area,
            category: r.category,
            category_context: r.category_context,
            payment_method: r.payment_method,
            total_amount,
            vat_amount,
            rounding_amount,
            net_amount,
            note: r.note,
            confirmed_at: r.confirmed_at,
            has_image: r.has_image,
        });
    }

    let mut income_entries = Vec::with_capacity(raw.income_entries.len());
    for (i, e) in raw.income_entries.into_iter().enumerate() {
        let at = |field: &str| format!("income_entries[{i}].{field}");
        let amount_ex_vat = money(&at("amount_ex_vat"), &e.amount_ex_vat)?;
        let vat_amount = money(&at("vat_amount"), &e.vat_amount)?;
        let amount_inc_vat = money(&at("amount_inc_vat"), &e.amount_inc_vat)?;
        let expected_inc = add(amount_ex_vat, vat_amount)
            .ok_or_else(|| err(at("amount_inc_vat"), "amount overflow while checking sum"))?;
        if amount_inc_vat != expected_inc {
            return Err(err(
                at("amount_inc_vat"),
                format!(
                    "income:{} inc {amount_inc_vat} != ex {amount_ex_vat} + vat {vat_amount} (= {expected_inc})",
                    e.id
                ),
            ));
        }
        income_entries.push(IncomeEntry {
            id: e.id,
            source_key: e.source_key,
            property_id: e.property_id,
            income_type: e.income_type,
            date: e.date,
            buyer_name: e.buyer_name,
            description: e.description,
            amount_ex_vat,
            vat_amount,
            amount_inc_vat,
            invoice_number: e.invoice_number,
            payment_date: e.payment_date,
            document_count: e.document_count,
        });
    }

    Ok(Snapshot {
        schema_version: raw.schema_version,
        generated_at: raw.generated_at,
        source: raw.source,
        entity: Entity {
            owner_id: raw.entity.owner_id,
            display_name: raw.entity.display_name,
            org_number,
            operation: raw.entity.operation,
            county: raw.entity.county,
            taxonomy_version: raw.entity.taxonomy_version,
            accounting_profile: raw.entity.accounting_profile,
            accounting_identity,
        },
        fiscal_period,
        income_year: raw.income_year,
        lock: raw.lock,
        properties: raw.properties,
        receipts,
        income_entries,
        audit_chain: raw.audit_chain,
    })
}

// ---------------------------------------------------------------------------
// Schema 1.2 — accounting identity and fiscal period (SK-SIE-BRIDGE-03)
// ---------------------------------------------------------------------------

type IdentityParts = (
    Option<String>,
    Option<AccountingIdentity>,
    Option<FiscalPeriod>,
);

/// L-KEYS: version-specific key presence. 1.0/1.1 keep `org_number` as before
/// and must not carry the two new keys; 1.2 must carry both (value may be
/// `null`) and must not carry `org_number` at all, not even as `null`.
fn identity_and_period(
    schema: &str,
    org_number: Option<Option<String>>,
    identity: Option<Option<RawAccountingIdentity>>,
    period: Option<Option<RawFiscalPeriod>>,
) -> Result<IdentityParts, SnapshotError> {
    if schema != SCHEMA_ACCOUNTING_IDENTITY {
        if identity.is_some() {
            return Err(err(
                "entity.accounting_identity",
                "only allowed in schema 1.2",
            ));
        }
        if period.is_some() {
            return Err(err("fiscal_period", "only allowed in schema 1.2"));
        }
        return Ok((org_number.flatten(), None, None));
    }
    if org_number.is_some() {
        return Err(err(
            "entity.org_number",
            "not allowed in schema 1.2 (the accounting identity is entity.accounting_identity)",
        ));
    }
    let identity = match identity {
        None => {
            return Err(err(
                "entity.accounting_identity",
                "required key in schema 1.2",
            ))
        }
        Some(None) => None,
        Some(Some(raw)) => Some(convert_identity(raw)?),
    };
    let period = match period {
        None => return Err(err("fiscal_period", "required key in schema 1.2")),
        Some(None) => None,
        Some(Some(raw)) => Some(convert_period(raw)?),
    };
    Ok((None, identity, period))
}

fn text_field(value: serde_json::Value, path: &str) -> Result<String, SnapshotError> {
    match value {
        serde_json::Value::String(s) => Ok(s),
        _ => Err(err(path, "must be a string")),
    }
}

fn convert_identity(raw: RawAccountingIdentity) -> Result<AccountingIdentity, SnapshotError> {
    const AT: &str = "entity.accounting_identity";
    let entity_type =
        EntityType::parse(&text_field(raw.entity_type, &format!("{AT}.entity_type"))?)
            .ok_or_else(|| err(format!("{AT}.entity_type"), "unknown entity type"))?;
    let identifier_kind = IdentifierKind::parse(&text_field(
        raw.identifier_kind,
        &format!("{AT}.identifier_kind"),
    )?)
    .ok_or_else(|| err(format!("{AT}.identifier_kind"), "unknown identifier kind"))?;
    let pair_ok = match entity_type {
        EntityType::SoleTrader => matches!(
            identifier_kind,
            IdentifierKind::PersonalNumber | IdentifierKind::CoordinationNumber
        ),
        _ => identifier_kind == IdentifierKind::OrganisationNumber,
    };
    if !pair_ok {
        return Err(err(
            format!("{AT}.identifier_kind"),
            "identifier kind does not match the entity type",
        ));
    }
    let legal_name = text_field(raw.legal_name, &format!("{AT}.legal_name"))?;
    if legal_name.trim().is_empty() {
        return Err(err(format!("{AT}.legal_name"), "must not be empty"));
    }
    if legal_name
        .chars()
        .any(|c| (c as u32) < 0x20 || c as u32 == 0x7F)
    {
        return Err(err(
            format!("{AT}.legal_name"),
            "must not contain control characters",
        ));
    }
    let business_identifier = text_field(
        raw.business_identifier,
        &format!("{AT}.business_identifier"),
    )?;
    if !wire_identifier_valid(&business_identifier, identifier_kind) {
        // Fixed text: the value is never echoed.
        return Err(err(
            format!("{AT}.business_identifier"),
            "not a valid identifier for its kind (expected NNNNNN-NNNN)",
        ));
    }
    Ok(AccountingIdentity {
        entity_type,
        legal_name,
        identifier_kind,
        business_identifier,
    })
}

/// L-WIRE-VALID: exact 11-byte ASCII `DDDDDD-DDDD`, Luhn over the ten digits,
/// plus the kind rule. No trimming, no normalisation, no invented century.
pub(crate) fn wire_identifier_valid(value: &str, kind: IdentifierKind) -> bool {
    let b = value.as_bytes();
    if b.len() != 11 || b[6] != b'-' {
        return false;
    }
    let digits: Vec<u8> = b[..6].iter().chain(&b[7..]).copied().collect();
    if digits.len() != 10 || !digits.iter().all(u8::is_ascii_digit) {
        return false;
    }
    let d: Vec<u32> = digits.iter().map(|c| u32::from(c - b'0')).collect();
    let luhn: u32 = d
        .iter()
        .enumerate()
        .map(|(i, v)| {
            let x = if i % 2 == 0 { v * 2 } else { *v };
            if x > 9 {
                x - 9
            } else {
                x
            }
        })
        .sum();
    if !luhn.is_multiple_of(10) {
        return false;
    }
    let month = d[2] * 10 + d[3];
    let day = d[4] * 10 + d[5];
    match kind {
        IdentifierKind::PersonalNumber => (1..=12).contains(&month) && (1..=31).contains(&day),
        IdentifierKind::CoordinationNumber => (1..=12).contains(&month) && (61..=91).contains(&day),
        IdentifierKind::OrganisationNumber => d[2] >= 2,
    }
}

fn strict_date(value: serde_json::Value, path: &str) -> Result<NaiveDate, SnapshotError> {
    let text = text_field(value, path)?;
    let b = text.as_bytes();
    let shape_ok = b.len() == 10
        && b[4] == b'-'
        && b[7] == b'-'
        && b.iter()
            .enumerate()
            .all(|(i, c)| i == 4 || i == 7 || c.is_ascii_digit());
    if !shape_ok {
        return Err(err(path, "expected YYYY-MM-DD"));
    }
    NaiveDate::parse_from_str(&text, "%Y-%m-%d").map_err(|_| err(path, "not a calendar date"))
}

/// Same day-of-month `months` calendar months later, clamped to the last day of
/// the target month — the algorithm Django uses (BRIDGE-01 §2.1).
fn add_months(day: NaiveDate, months: u32) -> Option<NaiveDate> {
    use chrono::Datelike;
    let index = day.month0() + months;
    let year = day.year() + i32::try_from(index / 12).ok()?;
    let month = index % 12 + 1;
    (1..=day.day())
        .rev()
        .find_map(|d| NaiveDate::from_ymd_opt(year, month, d))
}

fn convert_period(raw: RawFiscalPeriod) -> Result<FiscalPeriod, SnapshotError> {
    let start = strict_date(raw.start, "fiscal_period.start")?;
    let end = strict_date(raw.end, "fiscal_period.end")?;
    if start > end {
        return Err(err("fiscal_period", "start is after end"));
    }
    let limit = add_months(start, MAX_PERIOD_MONTHS)
        .ok_or_else(|| err("fiscal_period", "period length overflow"))?;
    if end >= limit {
        return Err(err("fiscal_period", "longer than 18 calendar months"));
    }
    Ok(FiscalPeriod { start, end })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> Vec<u8> {
        std::fs::read(format!("fixtures/snapshots/{name}"))
            .expect("fixture file should exist — run from the crate root")
    }

    fn parse_fixture(name: &str) -> Result<Snapshot, SnapshotError> {
        parse_snapshot(&fixture(name))
    }

    /// The 1.1 fixture with one value swapped, so each negative case edits
    /// a real snapshot instead of hand-building JSON.
    fn fixture_1_1_with(needle: &str, replacement: &str) -> Vec<u8> {
        let text = String::from_utf8(fixture("minimal-1.1.json")).expect("fixture is UTF-8");
        assert!(
            text.contains(needle),
            "needle {needle:?} not present in fixture"
        );
        text.replacen(needle, replacement, 1).into_bytes()
    }

    fn ore(s: &str) -> Ore {
        Ore::parse(s).expect("test literal should be a valid amount")
    }

    // -- happy paths --------------------------------------------------------

    #[test]
    fn parses_minimal_1_0_with_exact_ore() {
        let snap = parse_fixture("minimal-1.0.json").expect("1.0 fixture parses");
        assert_eq!(snap.schema_version, "1.0");
        assert_eq!(snap.income_year, 2026);
        assert_eq!(snap.entity.display_name, "Kråksjö gård");
        assert_eq!(snap.properties.len(), 1);
        assert_eq!(snap.receipts.len(), 1);
        assert_eq!(snap.income_entries.len(), 1);
        assert_eq!(snap.audit_chain.len(), 2);

        let r = &snap.receipts[0];
        assert_eq!(r.total_amount, Ore(125_000));
        assert_eq!(r.vat_amount, Ore(25_000));
        assert_eq!(r.rounding_amount, Ore(0));
        assert_eq!(r.net_amount, Ore(100_000));
        assert_eq!(r.ordinal_number, Some(17));

        let e = &snap.income_entries[0];
        assert_eq!(e.amount_ex_vat, Ore(4_500_000));
        assert_eq!(e.vat_amount, Ore(1_125_000));
        assert_eq!(e.amount_inc_vat, Ore(5_625_000));
    }

    #[test]
    fn parses_minimal_1_1_including_optional_blocks() {
        let snap = parse_fixture("minimal-1.1.json").expect("1.1 fixture parses");
        assert_eq!(snap.schema_version, "1.1");
        assert_eq!(snap.entity.taxonomy_version.as_deref(), Some("1.0"));
        let profile = snap
            .entity
            .accounting_profile
            .as_ref()
            .expect("1.1 carries the accounting profile");
        assert_eq!(profile.vat_registered, "yes");
        assert_eq!(profile.bookkeeping_method, "cash");
        assert_eq!(profile.default_payment_method, "company_account");
        assert_eq!(profile.sie_series, "A");

        let first = &snap.receipts[0];
        assert_eq!(first.source_key.as_deref(), Some("receipt:1001"));
        assert_eq!(first.payment_method.as_deref(), Some("company_account"));
        let ctx = first
            .category_context
            .as_ref()
            .expect("first receipt carries category_context");
        assert!(ctx.requires_business_share);
        assert!(!ctx.investment_risk);
        assert!(!ctx.vat_check);
        assert!(ctx.sensitive);

        // Negative öresutjämning survives exactly: 999.00 - 200.00 - (-0.20) = 799.20.
        let second = &snap.receipts[1];
        assert_eq!(second.rounding_amount, ore("-0.20"));
        assert_eq!(second.net_amount, ore("799.20"));
        assert_eq!(second.payment_method.as_deref(), Some("unknown"));
        assert!(second.category_context.is_none());
        assert!(second.category.is_none());

        assert_eq!(
            snap.income_entries[0].source_key.as_deref(),
            Some("income:501")
        );
    }

    #[test]
    fn schema_1_0_leaves_1_1_fields_none() {
        let snap = parse_fixture("minimal-1.0.json").expect("1.0 fixture parses");
        assert!(snap.entity.taxonomy_version.is_none());
        assert!(snap.entity.accounting_profile.is_none());
        assert!(snap.receipts[0].source_key.is_none());
        assert!(snap.receipts[0].payment_method.is_none());
        assert!(snap.receipts[0].category_context.is_none());
        assert!(snap.income_entries[0].source_key.is_none());
    }

    #[test]
    fn list_order_is_file_order() {
        let snap = parse_fixture("minimal-1.1.json").expect("1.1 fixture parses");
        let ids: Vec<i64> = snap.receipts.iter().map(|r| r.id).collect();
        assert_eq!(ids, vec![1001, 1002]);
    }

    #[test]
    fn two_parses_of_same_bytes_are_equal() {
        let bytes = fixture("minimal-1.1.json");
        let a = parse_snapshot(&bytes).expect("parses");
        let b = parse_snapshot(&bytes).expect("parses");
        assert_eq!(a, b);
    }

    #[test]
    fn conversion_is_total() {
        // Every raw field has a typed home: the fixture's values must be
        // findable in the typed model, none silently dropped on the way over.
        let snap = parse_fixture("minimal-1.1.json").expect("1.1 fixture parses");
        assert_eq!(snap.generated_at, "2026-08-31T12:00:00+02:00");
        assert_eq!(snap.source.app, "skogskvitto");
        assert_eq!(snap.source.environment, "test");
        assert_eq!(snap.entity.owner_id, 42);
        assert_eq!(snap.entity.org_number.as_deref(), Some("999999-9999"));
        assert_eq!(snap.entity.operation, vec!["skog", "mark"]);
        assert_eq!(snap.entity.county.as_deref(), Some("Kronoberg"));
        assert!(!snap.lock.all_properties_locked);
        assert_eq!(snap.lock.declaration_year, 2027);
        let p = &snap.properties[0];
        assert_eq!(
            (p.id, p.slug.as_str(), p.is_default),
            (7, "kraksjo-sateri", true)
        );
        assert_eq!(p.tax_year.status, "open");
        assert!(p.tax_year.locked_at.is_none());
        let r = &snap.receipts[0];
        assert_eq!(r.date, "2026-03-12");
        assert_eq!(r.vendor.as_deref(), Some("OKQ8"));
        assert_eq!(r.entry_type, "expense");
        assert_eq!(r.area, "fordon");
        assert_eq!(r.category.as_deref(), Some("Bränsle"));
        assert!(r.note.is_none());
        assert_eq!(r.confirmed_at.as_deref(), Some("2026-03-12T18:22:11+01:00"));
        assert!(r.has_image);
        let e = &snap.income_entries[0];
        assert_eq!(e.income_type, "timber_sale");
        assert_eq!(e.buyer_name.as_deref(), Some("Virkesköparen Golden AB"));
        assert_eq!(e.description, "Slutavverkning skifte 3");
        assert_eq!(e.invoice_number.as_deref(), Some("A-2231"));
        assert_eq!(e.payment_date.as_deref(), Some("2026-05-02"));
        assert_eq!(e.document_count, 2);
        match &snap.audit_chain[0] {
            AuditEvent::Event {
                event_type,
                property_id,
                recipient_role,
                ..
            } => {
                assert_eq!(event_type, "submitted_to_accountant");
                assert_eq!(*property_id, 7);
                assert_eq!(recipient_role.as_deref(), Some("redovisningskonsult"));
            }
            other => panic!("expected an event first, got {other:?}"),
        }
        match &snap.audit_chain[1] {
            AuditEvent::Document {
                document_type,
                storage_backend,
                ..
            } => {
                assert_eq!(document_type, "accountant_report");
                assert_eq!(storage_backend, "b2");
            }
            other => panic!("expected a document second, got {other:?}"),
        }
    }

    #[test]
    fn unknown_extra_field_is_ignored() {
        let bytes = fixture_1_1_with(
            r#""income_year": 2026,"#,
            r#""income_year": 2026, "future_field": {"x": 1},"#,
        );
        assert!(parse_snapshot(&bytes).is_ok());
    }

    // -- money format: exactly two decimals, nothing else ---------------------

    fn expect_money_error(bytes: &[u8], path: &str) {
        let e = parse_snapshot(bytes).expect_err("must be rejected");
        assert_eq!(e.path, path);
        assert!(
            e.message.contains("exactly two decimals"),
            "unexpected message: {}",
            e.message
        );
    }

    #[test]
    fn rejects_one_decimal_money() {
        expect_money_error(
            &fixture_1_1_with(r#""vat_amount": "200.00""#, r#""vat_amount": "200.0""#),
            "receipts[1].vat_amount",
        );
    }

    #[test]
    fn rejects_no_decimal_money() {
        expect_money_error(
            &fixture_1_1_with(r#""total_amount": "999.00""#, r#""total_amount": "999""#),
            "receipts[1].total_amount",
        );
    }

    #[test]
    fn rejects_three_decimal_money() {
        expect_money_error(
            &fixture_1_1_with(
                r#""amount_ex_vat": "45000.00""#,
                r#""amount_ex_vat": "45000.000""#,
            ),
            "income_entries[0].amount_ex_vat",
        );
    }

    #[test]
    fn rejects_locale_money() {
        expect_money_error(
            &fixture_1_1_with(
                r#""total_amount": "1250.00""#,
                r#""total_amount": "1 250,00""#,
            ),
            "receipts[0].total_amount",
        );
    }

    #[test]
    fn rejects_padded_money() {
        expect_money_error(
            &fixture_1_1_with(
                r#""rounding_amount": "0.00""#,
                r#""rounding_amount": " 0.00""#,
            ),
            "receipts[0].rounding_amount",
        );
    }

    #[test]
    fn rejects_json_number_money() {
        // A JSON number is a type error at the serde layer: no path of our
        // own, but serde_json's line/column and no panic.
        let bytes = fixture_1_1_with(r#""vat_amount": "250.00""#, r#""vat_amount": 250.0"#);
        let e = parse_snapshot(&bytes).expect_err("must be rejected");
        assert!(e.path.is_empty());
        assert!(e.message.starts_with("invalid snapshot JSON"));
        assert!(
            e.message.contains("line"),
            "serde message carries a position: {}",
            e.message
        );
    }

    // -- invariants -----------------------------------------------------------

    #[test]
    fn rejects_net_mismatch_with_source_key() {
        let e = parse_fixture("invalid-net.json").expect_err("net drift must be rejected");
        assert_eq!(e.path, "receipts[0].net_amount");
        assert!(e.message.contains("receipt:1001"), "{}", e.message);
        assert!(e.message.contains("999.00"), "{}", e.message);
    }

    #[test]
    fn rejects_income_sum_mismatch_with_source_key() {
        let bytes = fixture_1_1_with(
            r#""amount_inc_vat": "56250.00""#,
            r#""amount_inc_vat": "56000.00""#,
        );
        let e = parse_snapshot(&bytes).expect_err("sum drift must be rejected");
        assert_eq!(e.path, "income_entries[0].amount_inc_vat");
        assert!(e.message.contains("income:501"), "{}", e.message);
    }

    #[test]
    fn rejects_unknown_schema_version() {
        let e = parse_fixture("unknown-schema.json").expect_err("schema 2.0 is not supported");
        assert_eq!(e.path, "schema_version");
        assert!(e.message.contains("2.0"), "{}", e.message);
    }

    #[test]
    fn rejects_unknown_property_reference() {
        let bytes = fixture_1_1_with(
            r#""property_id": 7,
      "ordinal_number": 18"#,
            r#""property_id": 8,
      "ordinal_number": 18"#,
        );
        let e = parse_snapshot(&bytes).expect_err("dangling property reference");
        assert_eq!(e.path, "receipts[1].property_id");
        assert!(e.message.contains("receipt:1002"), "{}", e.message);
    }

    #[test]
    fn invalid_money_fixture_is_rejected_at_the_right_field() {
        let e = parse_fixture("invalid-money.json").expect_err("fixture is invalid on purpose");
        assert_eq!(e.path, "receipts[1].vat_amount");
    }

    // -- never panic ------------------------------------------------------------

    #[test]
    fn truncated_json_is_error_not_panic() {
        let mut bytes = fixture("minimal-1.1.json");
        bytes.truncate(bytes.len() / 2);
        let e = parse_snapshot(&bytes).expect_err("truncated input is an error");
        assert!(e.path.is_empty());
        assert!(e.message.starts_with("invalid snapshot JSON"));
    }

    #[test]
    fn empty_input_is_error_not_panic() {
        let e = parse_snapshot(b"").expect_err("empty input is an error");
        assert!(e.message.starts_with("invalid snapshot JSON"));
        assert_eq!(
            e.to_string(),
            e.message,
            "Display without a path is the bare message"
        );
    }

    #[test]
    fn error_display_prefixes_path() {
        let e = err("receipts[0].net_amount", "boom");
        assert_eq!(e.to_string(), "receipts[0].net_amount: boom");
    }

    #[test]
    fn two_decimal_check_is_exact() {
        for ok in ["0.00", "1250.00", "-0.20", "+12.50", "999999999.99"] {
            assert!(is_two_decimal_money(ok), "{ok} should pass");
        }
        for bad in [
            "", ".", "1250", "1250.0", "1250.000", "1,250.00", "1 250,00", " 1.00", "1.00 ",
            "--1.00", "1.0a",
        ] {
            assert!(!is_two_decimal_money(bad), "{bad:?} should fail");
        }
    }

    // -- document nullability (SV-01b) ------------------------------------------

    #[test]
    fn parses_document_with_null_optional_fields() {
        let snap =
            parse_fixture("document-nulls-1.1.json").expect("nullable document fields parse");
        match &snap.audit_chain[2] {
            AuditEvent::Document {
                received_date,
                original_filename,
                checksum_sha256,
                storage_backend,
                ..
            } => {
                assert!(received_date.is_none());
                assert!(original_filename.is_none());
                assert!(checksum_sha256.is_none());
                assert_eq!(storage_backend, "cloudinary");
            }
            other => panic!("expected the undated document third, got {other:?}"),
        }
    }

    #[test]
    fn document_with_missing_optional_keys_parses_as_none() {
        // Keys absent altogether (not just null) are the same contract: None.
        let bytes = fixture_1_1_with(r#""received_date": "2027-02-20","#, "");
        let snap = parse_snapshot(&bytes).expect("absent optional key parses");
        match &snap.audit_chain[1] {
            AuditEvent::Document { received_date, .. } => assert!(received_date.is_none()),
            other => panic!("expected a document second, got {other:?}"),
        }
    }

    #[test]
    fn document_required_fields_stay_required() {
        let bytes = fixture_1_1_with(r#""storage_backend": "b2""#, r#""storage_backend": null"#);
        let e = parse_snapshot(&bytes).expect_err("storage_backend is not optional");
        assert!(e.message.starts_with("invalid snapshot JSON"));
    }

    #[test]
    fn document_received_date_rejects_wrong_type() {
        let bytes = fixture_1_1_with(
            r#""received_date": "2027-02-20""#,
            r#""received_date": 20270220"#,
        );
        let e = parse_snapshot(&bytes).expect_err("a number is not a date string");
        assert!(e.message.starts_with("invalid snapshot JSON"));
    }
}
