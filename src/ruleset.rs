//! Accounting ruleset masterdata (SV-02): VAT rules, counter-account rules and
//! the accounting cases generated from the Mastermatris, plus `load_masterdata`
//! which reads a generated root and re-runs the cross-file invariants
//! (V1–V4, V6–V8, V10 shape, draft/approved). No engine decisions live here —
//! that is SV-03. Rust never downgrades a case: a draft ruleset that still
//! contains `Automatic` is a contract violation and refuses to load.

use std::collections::{BTreeSet, HashSet};
use std::fs;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::chart::{
    is_account_number, parse_chart, ChartProfile, Header, MasterdataError, Review, REVIEW_APPROVED,
    REVIEW_DRAFT,
};
use crate::sru::{parse_sru, validate_sru, SruTable};

pub const VAT_SCHEMA: &str = "sieverk-vat/1";
pub const COUNTER_SCHEMA: &str = "sieverk-counter/1";
pub const RULESET_SCHEMA: &str = "sieverk-ruleset/1";
pub const AUTOMATIONS: [&str; 3] = ["Automatic", "Conditional", "Manual"];
pub const VAT_RATES: [&str; 4] = ["25", "12", "6", "0"];
pub const COUNTER_RULE_DEFAULT: &str = "enligt_motkonton";

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VatRule {
    pub id: String,
    pub name: String,
    pub direction: String,
    pub rates: Vec<String>,
    pub vat_account: String,
    pub deductibility: String,
    pub manual_review: bool,
    #[serde(default)]
    pub condition: Option<String>,
    pub review: Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct VatRules {
    #[serde(rename = "_header")]
    pub header: Header,
    pub schema: String,
    pub rules: Vec<VatRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CounterRule {
    pub source: String,
    pub key: String,
    #[serde(default)]
    pub bookkeeping_method: Option<String>,
    #[serde(default)]
    pub account: Option<String>,
    pub automation: String,
    #[serde(default)]
    pub question: Option<String>,
    pub review: Review,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct CounterRules {
    #[serde(rename = "_header")]
    pub header: Header,
    pub schema: String,
    pub rows: Vec<CounterRule>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountingCase {
    pub case_id: String,
    pub source: String,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub income_type: Option<String>,
    pub is_default: bool,
    pub automation: String,
    #[serde(default)]
    pub downgraded_from_automatic: bool,
    #[serde(default)]
    pub account: Option<String>,
    #[serde(default)]
    pub vat_rule: Option<String>,
    #[serde(default = "default_counter_rule")]
    pub counter_rule: String,
    #[serde(default)]
    pub sru_override: Option<String>,
    #[serde(default)]
    pub condition: Option<String>,
    #[serde(default)]
    pub question: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    pub review: Review,
}

fn default_counter_rule() -> String {
    COUNTER_RULE_DEFAULT.to_string()
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChartRef {
    pub chart_id: String,
    pub version: String,
}

/// The dropdown universe the generator validated the cases against, carried in
/// the artefact so V1/V2 can be re-run from `--root` alone.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct TaxonomyUniverse {
    pub version: String,
    pub categories: Vec<String>,
    pub income_types: Vec<String>,
    pub payment_methods: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AccountingRuleset {
    #[serde(rename = "_header")]
    pub header: Header,
    pub schema: String,
    pub ruleset_version: String,
    pub taxonomy_version: String,
    pub chart: ChartRef,
    pub review_status: String,
    pub taxonomy: TaxonomyUniverse,
    pub cases: Vec<AccountingCase>,
}

/// Everything SV-03 needs from a masterdata root, validated as a whole.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Masterdata {
    pub chart: ChartProfile,
    pub sru: SruTable,
    pub vat: VatRules,
    pub counter: CounterRules,
    pub ruleset: AccountingRuleset,
}

fn schema_check(actual: &str, expected: &str) -> Result<(), MasterdataError> {
    if actual == expected {
        Ok(())
    } else {
        Err(MasterdataError::new(
            "schema",
            format!("expected {expected}, got {actual}"),
        ))
    }
}

pub fn parse_vat_rules(bytes: &[u8]) -> Result<VatRules, MasterdataError> {
    let vat: VatRules = serde_json::from_slice(bytes)
        .map_err(|e| MasterdataError::new("", format!("invalid VAT JSON: {e}")))?;
    schema_check(&vat.schema, VAT_SCHEMA)?;
    let mut ids = HashSet::new();
    for r in &vat.rules {
        let path = format!("rules/{}", r.id);
        if !ids.insert(r.id.clone()) {
            return Err(MasterdataError::new(&path, "duplicate vat rule id"));
        }
        if r.direction != "ingående" && r.direction != "utgående" {
            return Err(MasterdataError::new(
                &path,
                "direction must be ingående or utgående",
            ));
        }
        if r.rates.is_empty() || r.rates.iter().any(|s| !VAT_RATES.contains(&s.as_str())) {
            return Err(MasterdataError::new(
                &path,
                "rates must be within {25,12,6,0}",
            ));
        }
        if !["full", "ingen", "manuell"].contains(&r.deductibility.as_str()) {
            return Err(MasterdataError::new(
                &path,
                "deductibility must be full, ingen or manuell",
            ));
        }
    }
    Ok(vat)
}

pub fn parse_counter_rules(bytes: &[u8]) -> Result<CounterRules, MasterdataError> {
    let counter: CounterRules = serde_json::from_slice(bytes)
        .map_err(|e| MasterdataError::new("", format!("invalid counter-account JSON: {e}")))?;
    schema_check(&counter.schema, COUNTER_SCHEMA)?;
    for r in &counter.rows {
        let path = format!("rows/{}/{}", r.source, r.key);
        if r.source != "receipt" && r.source != "income" {
            return Err(MasterdataError::new(
                &path,
                "source must be receipt or income",
            ));
        }
        if !AUTOMATIONS.contains(&r.automation.as_str()) {
            return Err(MasterdataError::new(&path, "unknown automation"));
        }
        if r.account.is_none() && r.automation != "Manual" {
            return Err(MasterdataError::new(
                &path,
                "account missing but automation is not Manual",
            ));
        }
        if r.automation != "Automatic" && r.question.as_deref().unwrap_or("").is_empty() {
            return Err(MasterdataError::new(
                &path,
                "question required when automation is not Automatic",
            ));
        }
    }
    Ok(counter)
}

pub fn parse_ruleset(bytes: &[u8]) -> Result<AccountingRuleset, MasterdataError> {
    let ruleset: AccountingRuleset = serde_json::from_slice(bytes)
        .map_err(|e| MasterdataError::new("", format!("invalid ruleset JSON: {e}")))?;
    schema_check(&ruleset.schema, RULESET_SCHEMA)?;
    if ruleset.review_status != REVIEW_DRAFT && ruleset.review_status != REVIEW_APPROVED {
        return Err(MasterdataError::new(
            "review_status",
            "must be draft or approved",
        ));
    }
    if ruleset.taxonomy.version != ruleset.taxonomy_version {
        return Err(MasterdataError::new(
            "taxonomy.version",
            "does not match taxonomy_version",
        ));
    }
    let mut ids = HashSet::new();
    let mut defaults: BTreeSet<(String, String)> = BTreeSet::new();
    for c in &ruleset.cases {
        let path = format!("cases/{}", c.case_id);
        if !ids.insert(c.case_id.clone()) {
            return Err(MasterdataError::new(&path, "duplicate case_id")); // V11
        }
        if !AUTOMATIONS.contains(&c.automation.as_str()) {
            return Err(MasterdataError::new(&path, "unknown automation"));
        }
        // v1.2 §C: the generator downgrades; a draft artefact still carrying
        // Automatic was hand-edited or built wrong — refuse, never repair.
        if ruleset.review_status == REVIEW_DRAFT && c.automation == "Automatic" {
            return Err(MasterdataError::new(
                &path,
                "draft ruleset contains an Automatic case (contract violation)",
            ));
        }
        let subject = match c.source.as_str() {
            "receipt" => match &c.category {
                Some(cat) if ruleset.taxonomy.categories.contains(cat) => cat.clone(),
                Some(cat) => {
                    return Err(MasterdataError::new(
                        &path,
                        format!("category {cat:?} is not in the taxonomy"), // V1
                    ));
                }
                None => return Err(MasterdataError::new(&path, "receipt case without category")),
            },
            "income" => match &c.income_type {
                Some(t) if ruleset.taxonomy.income_types.contains(t) => t.clone(),
                Some(t) => {
                    return Err(MasterdataError::new(
                        &path,
                        format!("income_type {t:?} is not in the export"), // V2
                    ));
                }
                None => {
                    return Err(MasterdataError::new(
                        &path,
                        "income case without income_type",
                    ))
                }
            },
            other => {
                return Err(MasterdataError::new(
                    &path,
                    format!("source must be receipt or income, got {other:?}"),
                ))
            }
        };
        if c.is_default && !defaults.insert((c.source.clone(), subject)) {
            return Err(MasterdataError::new(
                &path,
                "more than one default case for this subject",
            )); // V8
        }
        // V10 shape.
        match c.automation.as_str() {
            "Automatic" if c.account.is_none() || c.vat_rule.is_none() => {
                return Err(MasterdataError::new(
                    &path,
                    "Automatic requires account and vat_rule",
                ))
            }
            "Conditional" if blank(&c.condition) || blank(&c.question) => {
                return Err(MasterdataError::new(
                    &path,
                    "Conditional requires condition and question",
                ))
            }
            "Manual" if blank(&c.question) => {
                return Err(MasterdataError::new(&path, "Manual requires question"))
            }
            _ => {}
        }
    }
    Ok(ruleset)
}

fn blank(value: &Option<String>) -> bool {
    value.as_deref().is_none_or(|s| s.trim().is_empty())
}

/// Cross-file invariants once every file parsed on its own.
pub fn validate_masterdata(md: &Masterdata) -> Result<(), MasterdataError> {
    if md.ruleset.chart.chart_id != md.chart.chart_id
        || md.ruleset.chart.version != md.chart.version
    {
        return Err(MasterdataError::new(
            "ruleset.chart",
            "does not match the chart's id/version",
        ));
    }
    for (name, status) in [
        ("sru", &md.sru.header.review_status),
        ("vat", &md.vat.header.review_status),
        ("counter", &md.counter.header.review_status),
        ("ruleset", &md.ruleset.review_status),
    ] {
        if status != &md.chart.review_status {
            return Err(MasterdataError::new(
                format!("{name}.review_status"),
                "differs from the chart's review_status",
            ));
        }
    }
    validate_sru(&md.sru, &md.chart)?;
    // V9 (v1.2 §C): a warning in draft — the generator reports it and the
    // artefact stays loadable; hard for approved masterdata.
    if md.chart.review_status == REVIEW_APPROVED {
        for a in &md.chart.accounts {
            if let Some(err) = a.review.approval_error(&format!("chart/{}", a.number)) {
                return Err(err);
            }
        }
        for r in &md.sru.rows {
            if let Some(err) = r.review.approval_error(&format!("sru/{}", r.account)) {
                return Err(err);
            }
        }
        for r in &md.vat.rules {
            if let Some(err) = r.review.approval_error(&format!("vat/{}", r.id)) {
                return Err(err);
            }
        }
        for r in &md.counter.rows {
            if let Some(err) = r
                .review
                .approval_error(&format!("counter/{}/{}", r.source, r.key))
            {
                return Err(err);
            }
        }
        for c in &md.ruleset.cases {
            if let Some(err) = c.review.approval_error(&format!("cases/{}", c.case_id)) {
                return Err(err);
            }
        }
    }
    for r in &md.vat.rules {
        let path = format!("vat/{}", r.id);
        if md.chart.account(&r.vat_account).is_none() {
            return Err(MasterdataError::new(
                &path,
                "vat_account is not in the chart",
            )); // V7
        }
    }
    for r in &md.counter.rows {
        let path = format!("counter/{}/{}", r.source, r.key);
        if r.source == "receipt" && !md.ruleset.taxonomy.payment_methods.contains(&r.key) {
            return Err(MasterdataError::new(
                &path,
                "payment_method is not in the export",
            )); // V2
        }
        if let Some(acc) = &r.account {
            require_account(&md.chart, acc, &path, r.automation != "Manual")?;
        }
    }
    let vat_ids: HashSet<&str> = md.vat.rules.iter().map(|r| r.id.as_str()).collect();
    for c in &md.ruleset.cases {
        let path = format!("cases/{}", c.case_id);
        if let Some(acc) = &c.account {
            require_account(&md.chart, acc, &path, c.automation != "Manual")?;
        }
        if let Some(v) = &c.vat_rule {
            if !vat_ids.contains(v.as_str()) {
                return Err(MasterdataError::new(
                    &path,
                    format!("vat_rule {v:?} is not defined"),
                ));
            }
        }
        if c.counter_rule != COUNTER_RULE_DEFAULT {
            require_account(&md.chart, &c.counter_rule, &path, c.automation != "Manual")?;
        }
        if let Some(sru) = &c.sru_override {
            if !is_account_number(sru) {
                return Err(MasterdataError::new(
                    &path,
                    "sru_override must be four digits",
                ));
            }
        }
    }
    Ok(())
}

fn require_account(
    chart: &ChartProfile,
    number: &str,
    path: &str,
    proposable_needed: bool,
) -> Result<(), MasterdataError> {
    match chart.account(number) {
        None => Err(MasterdataError::new(
            path,
            format!("account {number} is not in the chart"), // V3
        )),
        Some(a) if !a.roles.active => Err(MasterdataError::new(
            path,
            format!("account {number} is inactive"),
        )),
        Some(a) if proposable_needed && !a.roles.engine_proposable => Err(MasterdataError::new(
            path,
            format!("account {number} is not engine_proposable"),
        )),
        Some(_) => Ok(()),
    }
}

/// Find exactly one file matching `prefix*.json` in `root`.
fn single_file(root: &Path, prefix: &str) -> Result<PathBuf, MasterdataError> {
    let mut matches: Vec<PathBuf> = fs::read_dir(root)
        .map_err(|e| {
            MasterdataError::new(root.display().to_string(), format!("cannot read root: {e}"))
        })?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with(prefix) && n.ends_with(".json"))
        })
        .collect();
    matches.sort();
    match matches.len() {
        1 => Ok(matches.remove(0)),
        0 => Err(MasterdataError::new(
            root.display().to_string(),
            format!("no {prefix}*.json in root"),
        )),
        n => Err(MasterdataError::new(
            root.display().to_string(),
            format!("{n} files match {prefix}*.json; expected exactly one"),
        )),
    }
}

fn read(path: &Path) -> Result<Vec<u8>, MasterdataError> {
    fs::read(path)
        .map_err(|e| MasterdataError::new(path.display().to_string(), format!("cannot read: {e}")))
}

fn at(path: &Path, err: MasterdataError) -> MasterdataError {
    MasterdataError::new(
        format!(
            "{}{}{}",
            path.display(),
            if err.path.is_empty() { "" } else { "#" },
            err.path
        ),
        err.message,
    )
}

/// Load a generated masterdata root (explicit path — there is no default root).
pub fn load_masterdata(root: &Path) -> Result<Masterdata, MasterdataError> {
    let chart_path = single_file(root, "chart-")?;
    let sru_path = single_file(root, "sru-")?;
    let vat_path = single_file(root, "vat-rules-")?;
    let counter_path = single_file(root, "counter-accounts-")?;
    let ruleset_path = single_file(root, "ruleset-")?;
    let md = Masterdata {
        chart: parse_chart(&read(&chart_path)?).map_err(|e| at(&chart_path, e))?,
        sru: parse_sru(&read(&sru_path)?).map_err(|e| at(&sru_path, e))?,
        vat: parse_vat_rules(&read(&vat_path)?).map_err(|e| at(&vat_path, e))?,
        counter: parse_counter_rules(&read(&counter_path)?).map_err(|e| at(&counter_path, e))?,
        ruleset: parse_ruleset(&read(&ruleset_path)?).map_err(|e| at(&ruleset_path, e))?,
    };
    validate_masterdata(&md)?;
    Ok(md)
}

impl Masterdata {
    pub fn is_draft(&self) -> bool {
        self.chart.is_draft()
    }

    pub fn default_case_for_category(&self, category: &str) -> Option<&AccountingCase> {
        self.ruleset.cases.iter().find(|c| {
            c.source == "receipt" && c.is_default && c.category.as_deref() == Some(category)
        })
    }

    pub fn default_case_for_income_type(&self, income_type: &str) -> Option<&AccountingCase> {
        self.ruleset.cases.iter().find(|c| {
            c.source == "income" && c.is_default && c.income_type.as_deref() == Some(income_type)
        })
    }
}
