//! chart_contract (SV-02 §5) — self-contained against the TRACKED synthetic
//! root. Never reads masterdata/real/**. Every load-time rule has a tracked
//! broken root under fixtures/masterdata/synthetic/invalid/ (the locked
//! evidence form); the in-memory mutations below are additional unit checks
//! that also pin the error text.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::Value;

use sieverk::chart::{parse_chart, AccountStatus, ChartProfile};
use sieverk::ruleset::{
    load_masterdata, parse_counter_rules, parse_ruleset, parse_vat_rules, validate_masterdata,
    AccountingRuleset, Masterdata,
};
use sieverk::sru::{parse_sru, validate_sru};

fn synthetic_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/masterdata/synthetic/generated")
}

fn invalid_root(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("fixtures/masterdata/synthetic/invalid")
        .join(name)
}

fn file(root: &Path, prefix: &str) -> Vec<u8> {
    let mut found: Vec<PathBuf> = fs::read_dir(root)
        .expect("test value")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .expect("test value")
                .to_str()
                .expect("test value")
                .starts_with(prefix)
        })
        .collect();
    assert_eq!(found.len(), 1, "exactly one {prefix}*.json expected");
    fs::read(found.remove(0)).expect("test value")
}

fn json(root: &Path, prefix: &str) -> Value {
    serde_json::from_slice(&file(root, prefix)).expect("test value")
}

fn bytes(v: &Value) -> Vec<u8> {
    serde_json::to_vec(v).expect("test value")
}

fn good() -> Masterdata {
    load_masterdata(&synthetic_root()).expect("synthetic root must load")
}

// ---------------------------------------------------------------------------
// The tracked root loads and says what the contract says
// ---------------------------------------------------------------------------

#[test]
fn synthetic_root_loads_as_draft() {
    let md = good();
    assert!(md.is_draft());
    assert_eq!(md.chart.chart_id, "SYNTHETIC_K1");
    assert_eq!(md.ruleset.taxonomy_version, "1.0");
    assert_eq!(md.ruleset.taxonomy.categories.len(), 51);
}

#[test]
fn all_k1_list_accounts_are_must_include_with_origin_vibeke() {
    let md = good();
    let mut k1: Vec<&str> = md.chart.must_include_numbers();
    k1.sort_unstable();
    // The tracked provenance JSON is Vibeke's list (PDF 2026-08-11): 45 accounts.
    let list: Value = serde_json::from_slice(
        &fs::read(
            Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("masterdata/vibeke-k1-lista-2026-08-11.json"),
        )
        .expect("test value"),
    )
    .expect("test value");
    let mut expected: Vec<&str> = list["accounts"]
        .as_array()
        .expect("test value")
        .iter()
        .map(|a| a["number"].as_str().expect("test value"))
        .collect();
    expected.sort_unstable();
    assert_eq!(expected.len(), 45, "Vibeke's list has 45 accounts");
    assert_eq!(k1, expected, "must_include set == Vibeke's list, exactly");
    for n in k1 {
        let a = md.chart.account(n).expect("test value");
        assert_eq!(
            a.must_include_origin.as_deref(),
            Some("VIBEKE_K1_LIST"),
            "{n}"
        );
        assert!(!a.source_id.is_empty());
        assert!(
            md.chart.sources.contains_key(&a.source_id),
            "{n}: source declared"
        );
    }
}

#[test]
fn closing_accounts_are_not_user_selectable_but_return_allowed() {
    let md = good();
    for n in ["7821", "7830", "8999"] {
        let a = md.chart.account(n).expect("test value");
        assert!(a.roles.closing_account, "{n}");
        assert!(!a.roles.user_selectable, "{n}");
        assert!(a.roles.return_sie_allowed, "{n}");
    }
}

#[test]
fn inactive_account_is_never_proposable() {
    let md = good();
    assert!(!md.chart.proposable("5999"));
    assert!(md.chart.proposable("5360"));
}

#[test]
fn outside_profile_return_accounts_are_a_warning_class_never_an_error() {
    let md = good();
    assert_eq!(
        md.chart.classify("4010"),
        AccountStatus::ConsultantIntroduced
    );
    assert_eq!(md.chart.classify("1930"), AccountStatus::InProfile);
    assert_eq!(md.chart.classify("19x0"), AccountStatus::Malformed);
    assert_eq!(md.chart.classify(""), AccountStatus::Malformed);
}

#[test]
fn draft_ruleset_has_no_automatic_and_downgrades_carry_both_v10_fields() {
    let md = good();
    assert!(md.ruleset.cases.iter().all(|c| c.automation != "Automatic"));
    let downgraded: Vec<_> = md
        .ruleset
        .cases
        .iter()
        .filter(|c| c.downgraded_from_automatic)
        .collect();
    assert!(!downgraded.is_empty());
    for c in downgraded {
        assert_eq!(c.automation, "Conditional", "{}", c.case_id);
        assert_eq!(
            c.condition.as_deref(),
            Some("Ej accounting_reviewer-godkänd för automatisk kontering.")
        );
        assert_eq!(
            c.question.as_deref(),
            Some("Regeln är inte konsultgranskad.")
        );
    }
}

#[test]
fn every_case_points_at_existing_accounts_and_vat_rules() {
    let md = good();
    for c in &md.ruleset.cases {
        if let Some(a) = &c.account {
            assert!(md.chart.account(a).is_some(), "{}", c.case_id);
        }
        if let Some(v) = &c.vat_rule {
            assert!(md.vat.rules.iter().any(|r| &r.id == v), "{}", c.case_id);
        }
    }
    assert!(md.default_case_for_category("Bränsle").is_some());
    assert!(md.default_case_for_income_type("leveransvirke").is_some());
    assert!(md.default_case_for_category("Finns inte").is_none());
}

#[test]
fn sru_rows_point_at_chart_accounts_and_unverified_rows_are_not_data() {
    let md = good();
    validate_sru(&md.sru, &md.chart).expect("test value");
    for r in &md.sru.rows {
        assert!(md.chart.account(&r.account).is_some());
        assert!(!r.verified, "synthetic rows are never verified");
        assert!(md.sru.verified_for(&r.account, 2026).is_none());
    }
}

#[test]
fn tracked_headers_carry_no_wall_clock_field() {
    for prefix in [
        "chart-",
        "sru-",
        "vat-rules-",
        "counter-accounts-",
        "ruleset-",
        "examples-",
    ] {
        let v = json(&synthetic_root(), prefix);
        let header = v["_header"].as_object().expect("test value");
        assert!(
            header.get("generated_at").is_none(),
            "{prefix}: generated_at forbidden (V14)"
        );
        assert!(header.get("tool").is_none());
        let mut keys: Vec<&str> = header.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(
            keys,
            [
                "generated",
                "generator_version",
                "profile_scope",
                "review_status",
                "taxonomy_version",
                "workbook",
                "workbook_sha256"
            ],
            "{prefix}: header is exactly the rev 3.1 contract"
        );
        assert_eq!(header["review_status"], "draft");
        assert_eq!(header["taxonomy_version"], "1.0");
    }
}

#[test]
fn loading_twice_is_identical() {
    assert_eq!(good(), good());
}

// ---------------------------------------------------------------------------
// One mutation per load-time rule — readable error, never a panic
// ---------------------------------------------------------------------------

fn chart_err(mutate: impl FnOnce(&mut Value)) -> String {
    let mut v = json(&synthetic_root(), "chart-");
    mutate(&mut v);
    parse_chart(&bytes(&v)).expect_err("must fail").to_string()
}

fn ruleset_err(mutate: impl FnOnce(&mut Value)) -> String {
    let mut v = json(&synthetic_root(), "ruleset-");
    mutate(&mut v);
    parse_ruleset(&bytes(&v))
        .expect_err("must fail")
        .to_string()
}

fn cross_err(mutate: impl FnOnce(&mut Masterdata)) -> String {
    let mut md = good();
    mutate(&mut md);
    validate_masterdata(&md).expect_err("must fail").to_string()
}

#[test]
fn v4_duplicate_account_number() {
    let e = chart_err(|v| {
        let first = v["accounts"][0].clone();
        v["accounts"]
            .as_array_mut()
            .expect("test value")
            .push(first);
    });
    assert!(e.contains("duplicate"), "{e}");
}

#[test]
fn v4_three_digit_account_number() {
    let e = chart_err(|v| v["accounts"][0]["number"] = Value::from("193"));
    assert!(e.contains("four digits"), "{e}");
}

#[test]
fn v4_kind_contradicts_class() {
    let e = chart_err(|v| {
        let a = v["accounts"]
            .as_array_mut()
            .expect("test value")
            .iter_mut()
            .find(|a| a["number"] == "1930")
            .expect("test value");
        a["kind"] = Value::from("kostnad");
    });
    assert!(e.contains("contradicts"), "{e}");
}

#[test]
fn provenance_unknown_source_id() {
    let e = chart_err(|v| v["accounts"][0]["source_id"] = Value::from("NOT_DECLARED"));
    assert!(e.contains("source_id"), "{e}");
}

#[test]
fn source_version_missing_or_empty_is_refused() {
    let e = chart_err(|v| v["accounts"][0]["source_version"] = Value::from(""));
    assert!(e.contains("source_version"), "{e}");
    let mut v = json(&synthetic_root(), "chart-");
    v["accounts"][0]
        .as_object_mut()
        .expect("test value")
        .remove("source_version");
    assert!(
        parse_chart(&bytes(&v)).is_err(),
        "missing key must fail deserialisation"
    );
    assert!(load_masterdata(&invalid_root("source-version-missing")).is_err());
}

#[test]
fn source_decision_is_structured_never_a_bare_string() {
    let mut v = json(&synthetic_root(), "chart-");
    v["accounts"][0]["source_decision"] = serde_json::json!({
        "reason": "K1-namnet behålls", "decided_by": "Mats", "decided_at": "2026-09-03"
    });
    let chart = parse_chart(&bytes(&v)).expect("structured decision loads");
    assert_eq!(
        chart.accounts[0]
            .source_decision
            .as_ref()
            .expect("test value")
            .decided_at,
        "2026-09-03"
    );
    let e = chart_err(|v| {
        v["accounts"][0]["source_decision"] = Value::from("K1-namnet behålls | Mats | 2026-09-03")
    });
    assert!(!e.is_empty());
    let e = chart_err(|v| {
        v["accounts"][0]["source_decision"] =
            serde_json::json!({"reason": "x", "decided_by": "", "decided_at": "2026-09-03"})
    });
    assert!(e.contains("source_decision"), "{e}");
    assert!(load_masterdata(&invalid_root("source-decision-malformed")).is_err());
}

#[test]
fn provenance_must_include_without_origin() {
    let e = chart_err(|v| {
        let a = v["accounts"]
            .as_array_mut()
            .expect("test value")
            .iter_mut()
            .find(|a| a["must_include"] == true)
            .expect("test value");
        a["must_include_origin"] = Value::Null;
    });
    assert!(e.contains("must_include_origin"), "{e}");
}

#[test]
fn v9_tracked_root_approved_without_reviewer_is_refused() {
    let e = load_masterdata(&invalid_root("v9-approved-without-reviewer"))
        .expect_err("must fail")
        .to_string();
    assert!(e.contains("accounting_reviewer"), "{e}");
}

#[test]
fn v1_unknown_category_in_case() {
    let e = ruleset_err(|v| v["cases"][1]["category"] = Value::from("Finns inte i taxonomin"));
    assert!(e.contains("not in the taxonomy"), "{e}");
}

#[test]
fn v2_unknown_income_type_in_case() {
    let e = ruleset_err(|v| {
        let c = v["cases"]
            .as_array_mut()
            .expect("test value")
            .iter_mut()
            .find(|c| c["source"] == "income")
            .expect("test value");
        c["income_type"] = Value::from("timber_future");
    });
    assert!(e.contains("not in the export"), "{e}");
}

#[test]
fn v8_two_defaults_for_the_same_subject() {
    let e = ruleset_err(|v| {
        let mut dup = v["cases"][1].clone();
        dup["case_id"] = Value::from("bransle.second");
        v["cases"].as_array_mut().expect("test value").push(dup);
    });
    assert!(e.contains("default"), "{e}");
}

#[test]
fn v11_duplicate_case_id() {
    let e = ruleset_err(|v| {
        let dup = v["cases"][1].clone();
        v["cases"].as_array_mut().expect("test value").push(dup);
    });
    assert!(e.contains("duplicate case_id"), "{e}");
}

#[test]
fn v10_conditional_without_condition() {
    let e = ruleset_err(|v| {
        let c = v["cases"]
            .as_array_mut()
            .expect("test value")
            .iter_mut()
            .find(|c| c["automation"] == "Conditional")
            .expect("test value");
        c["condition"] = Value::Null;
    });
    assert!(e.contains("Conditional requires"), "{e}");
}

#[test]
fn draft_ruleset_with_automatic_is_refused_never_downgraded() {
    let e = ruleset_err(|v| v["cases"][1]["automation"] = Value::from("Automatic"));
    assert!(e.contains("draft ruleset contains an Automatic"), "{e}");
}

#[test]
fn taxonomy_version_mismatch_inside_ruleset() {
    let e = ruleset_err(|v| v["taxonomy"]["version"] = Value::from("0.9"));
    assert!(e.contains("taxonomy_version"), "{e}");
}

#[test]
fn v3_case_pointing_at_missing_account() {
    let e = cross_err(|md| md.ruleset.cases[1].account = Some("4999".to_string()));
    assert!(e.contains("not in the chart"), "{e}");
}

#[test]
fn v3_case_pointing_at_inactive_account() {
    let e = cross_err(|md| md.ruleset.cases[1].account = Some("5999".to_string()));
    assert!(e.contains("inactive"), "{e}");
}

#[test]
fn v6_sru_row_for_unknown_account() {
    let e = cross_err(|md| md.sru.rows[0].account = "4999".to_string());
    assert!(e.contains("not in the chart"), "{e}");
}

#[test]
fn v6_overlapping_sru_validity() {
    let e = cross_err(|md| {
        let mut dup = md.sru.rows[0].clone();
        dup.valid_from = Some(2020);
        md.sru.rows.push(dup);
    });
    assert!(e.contains("overlapping"), "{e}");
}

#[test]
fn v7_vat_rule_with_unknown_account() {
    let e = cross_err(|md| md.vat.rules[0].vat_account = "2699".to_string());
    assert!(e.contains("vat_account"), "{e}");
}

#[test]
fn v7_bad_rate_is_rejected_at_parse() {
    let mut v = json(&synthetic_root(), "vat-rules-");
    v["rules"][0]["rates"] = serde_json::json!(["24"]);
    let e = parse_vat_rules(&bytes(&v))
        .expect_err("must fail")
        .to_string();
    assert!(e.contains("rates"), "{e}");
}

#[test]
fn counter_rule_without_account_must_be_manual() {
    let mut v = json(&synthetic_root(), "counter-accounts-");
    let row = v["rows"]
        .as_array_mut()
        .expect("test value")
        .iter_mut()
        .find(|r| r["automation"] == "Manual")
        .expect("test value");
    row["automation"] = Value::from("Conditional");
    let e = parse_counter_rules(&bytes(&v))
        .expect_err("must fail")
        .to_string();
    assert!(e.contains("account missing"), "{e}");
}

#[test]
fn ruleset_chart_reference_must_match_chart() {
    let e = cross_err(|md| md.ruleset.chart.version = "2027.1".to_string());
    assert!(e.contains("ruleset.chart"), "{e}");
}

#[test]
fn review_status_must_agree_across_files() {
    let e = cross_err(|md| md.vat.header.review_status = "approved".to_string());
    assert!(e.contains("review_status"), "{e}");
}

#[test]
fn garbage_and_missing_roots_never_panic() {
    assert!(parse_chart(b"{").is_err());
    assert!(parse_ruleset(b"[]").is_err());
    assert!(parse_sru(b"").is_err());
    assert!(load_masterdata(Path::new("/definitely/not/a/root")).is_err());
    let tmp = std::env::temp_dir().join("sieverk-empty-root");
    let _ = fs::create_dir_all(&tmp);
    let e = load_masterdata(&tmp).expect_err("must fail").to_string();
    assert!(e.contains("no chart-"), "{e}");
}

#[test]
fn every_tracked_invalid_root_fails_to_load_without_panic() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("fixtures/masterdata/synthetic/invalid");
    let mut roots: Vec<PathBuf> = fs::read_dir(&dir)
        .expect("invalid/ exists")
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_dir())
        .collect();
    roots.sort();
    assert_eq!(
        roots.len(),
        25,
        "one tracked broken root per load-time rule"
    );
    for root in roots {
        let err = load_masterdata(&root)
            .err()
            .unwrap_or_else(|| panic!("{} must fail to load", root.display()));
        assert!(!err.to_string().is_empty(), "{}", root.display());
    }
    // And the good root still loads — the loop above is not a tautology.
    assert!(load_masterdata(&invalid_root("v4-duplicate-account")).is_err());
    assert!(good().chart.accounts.len() > 40);
}

#[test]
fn draft_artefact_with_a_v9_warning_row_loads_but_approved_does_not() {
    // v1.2 §C: V9 is a warning in --draft (the generator reports it, the
    // artefact stays loadable) and hard in --approved.
    let mut md = good();
    md.chart.accounts[0].review.status = "Godkänd".to_string();
    md.chart.accounts[0].review.reviewed_by = None;
    validate_masterdata(&md).expect("draft tolerates the V9 warning row");
    let mut approved = md.clone();
    for status in [
        &mut approved.chart.review_status,
        &mut approved.chart.header.review_status,
        &mut approved.sru.header.review_status,
        &mut approved.vat.header.review_status,
        &mut approved.counter.header.review_status,
        &mut approved.ruleset.review_status,
        &mut approved.ruleset.header.review_status,
    ] {
        *status = "approved".to_string();
    }
    let e = validate_masterdata(&approved)
        .expect_err("approved enforces V9")
        .to_string();
    assert!(e.contains("accounting_reviewer"), "{e}");
    approved.chart.accounts[0].review.reviewed_by = Some("Syntetisk granskare".to_string());
    approved.chart.accounts[0].review.reviewed_role = Some("accounting_reviewer".to_string());
    validate_masterdata(&approved).expect("approved with a proper reviewer loads");
}

// ---------------------------------------------------------------------------
// The taxonomy universe inside the ruleset equals the tracked export
// ---------------------------------------------------------------------------

#[test]
fn ruleset_taxonomy_universe_equals_masterdata_export() {
    let md = good();
    let export: Value = serde_json::from_slice(
        &fs::read(Path::new(env!("CARGO_MANIFEST_DIR")).join("masterdata/taxonomy-1.0.json"))
            .expect("test value"),
    )
    .expect("test value");
    let mut names: Vec<String> = export["categories"]
        .as_array()
        .expect("test value")
        .iter()
        .map(|c| c["name"].as_str().expect("test value").to_string())
        .collect();
    names.sort();
    assert_eq!(md.ruleset.taxonomy.categories, names);
    let mut incomes: Vec<String> = export["income_types"]
        .as_array()
        .expect("test value")
        .iter()
        .map(|c| c["value"].as_str().expect("test value").to_string())
        .collect();
    incomes.sort();
    assert_eq!(md.ruleset.taxonomy.income_types, incomes);
    assert_eq!(export["taxonomy_version"], md.ruleset.taxonomy_version);
    let _: AccountingRuleset = md.ruleset.clone();
    let _: &ChartProfile = &md.chart;
}
