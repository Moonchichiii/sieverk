//! engine_cases (SV-03 slice 1) — real DuckDB workspace file + real synthetic
//! masterdata root. No mocks. Per-row expectations are counted from the
//! fixtures (rev 2.1 §B, final lock erratum 2), not from unique-subject
//! coverage. No accounting lines exist in this slice.

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use sieverk::engine::{
    assess_cases, project_cases, CaseSource, CounterRuleRef, DecisionStatus, EngineCase,
    EngineError, FindingCode, Severity, SourceFacts,
};
use sieverk::money::Ore;
use sieverk::ruleset::{load_masterdata, Masterdata};
use sieverk::workspace::Workspace;

const TESTGARDEN: &str = "fixtures/snapshots/testgarden-2026-1.1.json";
const SYNTHETIC_ROOT: &str = "fixtures/masterdata/synthetic/generated";

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

/// A unique, absent path under the OS temp dir; removed on drop.
struct TempDb(PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join("sieverk-engine-tests");
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{}-{name}.duckdb", std::process::id()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("duckdb.wal"));
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(self.0.with_extension("duckdb.wal"));
    }
}

fn masterdata() -> Masterdata {
    load_masterdata(&repo(SYNTHETIC_ROOT)).expect("synthetic masterdata loads")
}

/// Ingest a snapshot fixture into a fresh workspace file and return it open.
fn workspace_from(db: &TempDb, fixture: &str) -> Workspace {
    let raw = fs::read(repo(fixture)).expect("fixture");
    let mut ws = Workspace::create(db.path()).expect("create");
    ws.ingest(&raw).expect("ingest");
    ws
}

fn testgarden(db: &TempDb) -> Workspace {
    workspace_from(db, TESTGARDEN)
}

fn date(s: &str) -> NaiveDate {
    NaiveDate::parse_from_str(s, "%Y-%m-%d").expect("date")
}

fn codes(a: &sieverk::engine::CaseAssessment) -> Vec<FindingCode> {
    a.findings.iter().map(|f| f.code).collect()
}

// ---------------------------------------------------------------------------
// H1 — projection in contract order, facts verbatim
// ---------------------------------------------------------------------------

#[test]
fn h1_project_cases_testgarden_in_contract_order_with_verbatim_facts() {
    let db = TempDb::new("h1");
    let ws = testgarden(&db);
    let cases = project_cases(&ws).expect("project");
    assert_eq!(cases.len(), 27);
    for (i, c) in cases.iter().enumerate() {
        assert_eq!(c.case_seq, i as i32, "dense case_seq");
    }
    assert!(cases[..21].iter().all(|c| c.source == CaseSource::Receipt));
    assert!(cases[21..].iter().all(|c| c.source == CaseSource::Income));

    let receipts = ws.read_receipts().expect("receipts");
    let incomes = ws.read_income_entries().expect("incomes");
    for (c, r) in cases[..21].iter().zip(&receipts) {
        assert_eq!(c.source_key, r.source_key.clone().expect("1.1 key"));
        assert_eq!(
            (c.row_id, c.property_id, c.ordinal_number),
            (r.id, r.property_id, r.ordinal_number)
        );
        assert_eq!(c.date, r.date);
        assert_eq!(c.subject, r.category);
        assert_eq!(
            (
                c.context.requires_business_share,
                c.context.investment_risk,
                c.context.vat_check,
                c.context.sensitive
            ),
            (
                r.requires_business_share,
                r.investment_risk,
                r.vat_check,
                r.sensitive
            )
        );
        match &c.facts {
            SourceFacts::Receipt {
                total,
                vat,
                rounding,
                net,
                payment_method,
                entry_type,
                area,
                has_image,
            } => {
                assert_eq!(
                    (*total, *vat, *rounding, *net),
                    (r.total, r.vat, r.rounding, r.net)
                );
                assert_eq!(payment_method, &r.payment_method);
                assert_eq!(
                    (entry_type.as_str(), area.as_str(), *has_image),
                    (r.entry_type.as_str(), r.area.as_str(), r.has_image)
                );
            }
            SourceFacts::Income { .. } => panic!("receipt case carries income facts"),
        }
    }
    for (c, e) in cases[21..].iter().zip(&incomes) {
        assert_eq!(c.source_key, e.source_key.clone().expect("1.1 key"));
        assert_eq!(
            (c.row_id, c.property_id, c.ordinal_number),
            (e.id, e.property_id, None)
        );
        assert_eq!(c.date, e.date);
        assert_eq!(c.subject.as_deref(), Some(e.income_type.as_str()));
        assert_eq!(
            c.context,
            sieverk::engine::CaseContext::default(),
            "income context is all None"
        );
        match &c.facts {
            SourceFacts::Income {
                ex_vat,
                vat,
                inc_vat,
                payment_date,
                income_type,
                document_count,
            } => {
                assert_eq!((*ex_vat, *vat, *inc_vat), (e.ex_vat, e.vat, e.inc_vat));
                assert_eq!(payment_date, &e.payment_date);
                assert_eq!(
                    (income_type.as_str(), *document_count),
                    (e.income_type.as_str(), e.document_count)
                );
            }
            SourceFacts::Receipt { .. } => panic!("income case carries receipt facts"),
        }
    }
    // Spot facts from the fixture: ordinal max 19 (never 1..21), Mackens date, rounding ±, one None payment_date.
    assert_eq!(
        cases.iter().filter_map(|c| c.ordinal_number).max(),
        Some(19)
    );
    assert!(cases.iter().any(|c| c.date == date("2026-08-20")));
    let roundings: HashSet<i64> = cases
        .iter()
        .filter_map(|c| match &c.facts {
            SourceFacts::Receipt { rounding, .. } if rounding.0 != 0 => Some(rounding.0),
            _ => None,
        })
        .collect();
    assert_eq!(
        roundings,
        [30i64, -20i64].into_iter().collect::<HashSet<i64>>()
    );
    assert_eq!(
        cases
            .iter()
            .filter(|c| matches!(
                &c.facts,
                SourceFacts::Income {
                    payment_date: None,
                    ..
                }
            ))
            .count(),
        1
    );
}

// ---------------------------------------------------------------------------
// H2 — determinism across runs and files
// ---------------------------------------------------------------------------

#[test]
fn h2_projection_and_assessment_are_deterministic() {
    let a = TempDb::new("h2a");
    let b = TempDb::new("h2b");
    let md = masterdata();
    let wa = testgarden(&a);
    let wb = testgarden(&b);
    let ea = wa.read_entity().expect("entity");
    let ca1 = project_cases(&wa).expect("p");
    let ca2 = project_cases(&wa).expect("p");
    let cb = project_cases(&wb).expect("p");
    assert_eq!(ca1, ca2);
    assert_eq!(ca1, cb);
    let aa1 = assess_cases(&ca1, &ea, &md).expect("a");
    let aa2 = assess_cases(&ca1, &ea, &md).expect("a");
    let ab = assess_cases(&cb, &wb.read_entity().expect("entity"), &md).expect("a");
    assert_eq!(aa1, aa2);
    assert_eq!(aa1, ab);
    // finding_no is dense 0..n-1 in every assessment
    for a in &aa1 {
        for (i, f) in a.findings.iter().enumerate() {
            assert_eq!(f.finding_no, i as i32);
        }
    }
}

// ---------------------------------------------------------------------------
// H3 — UNMAPPED_CATEGORY per row vs unique subjects (erratum 2 numbers)
// ---------------------------------------------------------------------------

#[test]
fn h3_unmapped_category_counts_per_row_and_per_unique_subject() {
    let db = TempDb::new("h3");
    let ws = testgarden(&db);
    let md = masterdata();
    let cases = project_cases(&ws).expect("p");
    let assessments = assess_cases(&cases, &ws.read_entity().expect("e"), &md).expect("a");

    let unmapped =
        |a: &sieverk::engine::CaseAssessment| codes(a).contains(&FindingCode::UnmappedCategory);
    let receipt_rows = assessments[..21].iter().filter(|a| unmapped(a)).count();
    let income_rows = assessments[21..].iter().filter(|a| unmapped(a)).count();
    assert_eq!(
        receipt_rows, 17,
        "16 rows without any default + the Annat / osäkert row"
    );
    assert_eq!(income_rows, 3);

    // Unique-subject view, asserted separately from per-row counts.
    let mut receipt_subjects_unmapped = HashSet::new();
    let mut receipt_subjects_mapped = HashSet::new();
    for (c, a) in cases[..21].iter().zip(&assessments[..21]) {
        let s = c
            .subject
            .clone()
            .expect("Testgården receipts have categories");
        if unmapped(a) {
            receipt_subjects_unmapped.insert(s);
        } else {
            receipt_subjects_mapped.insert(s);
        }
    }
    assert_eq!(
        receipt_subjects_unmapped.len(),
        15,
        "14 without default + Annat / osäkert"
    );
    assert_eq!(
        receipt_subjects_mapped,
        ["Bränsle", "Skogsvård", "Grus och material"]
            .iter()
            .map(|s| s.to_string())
            .collect::<HashSet<String>>()
    );
    // The 4 mapped receipt rows: Bränsle (1), Skogsvård (1), Grus och material (2).
    assert_eq!(21 - receipt_rows, 4);

    // Annat / osäkert: matched default case WITHOUT account ⇒ rule_case_id Some AND UNMAPPED_CATEGORY AND Manual.
    let annat = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Annat / osäkert"))
        .expect("row");
    let a = &assessments[annat];
    assert_eq!(a.rule_case_id.as_deref(), Some("annat_osakert.default"));
    assert!(unmapped(a));
    assert_eq!(a.status_ceiling, DecisionStatus::Manual);
    assert_eq!(
        a.findings
            .iter()
            .find(|f| f.code == FindingCode::UnmappedCategory)
            .and_then(|f| f.question.clone())
            .as_deref(),
        Some("Vad avser köpet/intäkten?")
    );
    // A receipt with no default case at all: rule_case_id None.
    let none = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("El och drift"))
        .expect("row");
    assert_eq!(assessments[none].rule_case_id, None);
    assert!(unmapped(&assessments[none]));

    // Income: leveransvirke ×2 + efterlikvid mapped; hunting_lease, avverkningsratt, grot unmapped.
    let income_mapped: Vec<&str> = cases[21..]
        .iter()
        .zip(&assessments[21..])
        .filter(|(_, a)| !unmapped(a))
        .map(|(c, _)| c.subject.as_deref().expect("type"))
        .collect();
    assert_eq!(
        income_mapped,
        vec!["leveransvirke", "efterlikvid", "leveransvirke"]
    );
}

// ---------------------------------------------------------------------------
// H4 — draft ruleset: never Automatic
// ---------------------------------------------------------------------------

#[test]
fn h4_draft_ruleset_never_yields_automatic() {
    let db = TempDb::new("h4");
    let ws = testgarden(&db);
    let md = masterdata();
    assert!(md.is_draft());
    let cases = project_cases(&ws).expect("p");
    let assessments = assess_cases(&cases, &ws.read_entity().expect("e"), &md).expect("a");
    assert!(assessments
        .iter()
        .all(|a| a.status_ceiling != DecisionStatus::Automatic));
    // Bränsle: rule case Conditional (downgraded in draft) + counter row company_account Automatic ⇒ Conditional, not Automatic.
    let bransle = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row");
    let a = &assessments[bransle];
    assert_eq!(a.rule_case_id.as_deref(), Some("bransle.default"));
    assert_eq!(a.vat_rule_id.as_deref(), Some("ing25"));
    assert_eq!(a.status_ceiling, DecisionStatus::Conditional);
    assert_eq!(
        codes(a),
        vec![FindingCode::MissingEvidence],
        "only the warning"
    );
}

// ---------------------------------------------------------------------------
// H5 — locked counter lookup on Testgården (erratum 1: unknown keeps its ref)
// ---------------------------------------------------------------------------

#[test]
fn h5_counter_lookup_on_testgarden_rows() {
    let db = TempDb::new("h5");
    let ws = testgarden(&db);
    let md = masterdata();
    let cases = project_cases(&ws).expect("p");
    let assessments = assess_cases(&cases, &ws.read_entity().expect("e"), &md).expect("a");
    let by_pm = |pm: &str| -> Vec<&sieverk::engine::CaseAssessment> {
        cases
            .iter()
            .zip(&assessments)
            .filter(|(c, _)| matches!(&c.facts, SourceFacts::Receipt { payment_method: Some(p), .. } if p == pm))
            .map(|(_, a)| a)
            .collect()
    };

    let unknown = by_pm("unknown");
    assert_eq!(unknown.len(), 1);
    assert_eq!(
        unknown[0].counter_rule,
        Some(CounterRuleRef {
            source: CaseSource::Receipt,
            key: "unknown".to_string(),
            bookkeeping_method: None
        }),
        "one candidate ⇒ reference preserved even without an account"
    );
    assert!(codes(unknown[0]).contains(&FindingCode::UnresolvedCounterAccount));
    assert_eq!(unknown[0].status_ceiling, DecisionStatus::Manual);

    let supplier = by_pm("supplier_credit");
    assert_eq!(supplier.len(), 1);
    assert_eq!(
        supplier[0].counter_rule,
        Some(CounterRuleRef {
            source: CaseSource::Receipt,
            key: "supplier_credit".to_string(),
            bookkeeping_method: Some("cash".to_string())
        })
    );
    assert!(codes(supplier[0]).contains(&FindingCode::UnresolvedCounterAccount));
    assert_eq!(supplier[0].status_ceiling, DecisionStatus::Manual);

    let company = by_pm("company_account");
    assert_eq!(company.len(), 18);
    for a in &company {
        assert_eq!(
            a.counter_rule.as_ref().map(|r| r.key.as_str()),
            Some("company_account")
        );
        assert!(!codes(a).contains(&FindingCode::UnresolvedCounterAccount));
    }
    let private = by_pm("private");
    assert_eq!(private.len(), 1);
    assert_eq!(
        private[0].counter_rule.as_ref().map(|r| r.key.as_str()),
        Some("private")
    );
    assert!(!codes(private[0]).contains(&FindingCode::UnresolvedCounterAccount));

    // Income cases never get a counter reference in Slice 1.
    assert!(assessments[21..].iter().all(|a| a.counter_rule.is_none()));
    // "1930" never appears anywhere in any assessment (there are no lines, and no field holds an account).
    for a in &assessments {
        assert!(!format!("{a:?}").contains("1930"));
    }
}

// ---------------------------------------------------------------------------
// H6 — counter edge cases via in-memory masterdata/entity mutations
// ---------------------------------------------------------------------------

#[test]
fn h6_counter_lookup_edge_cases() {
    let db = TempDb::new("h6");
    let ws = testgarden(&db);
    let cases = project_cases(&ws).expect("p");
    let entity = ws.read_entity().expect("e");

    // Two candidates (generic + cash-specific) for company_account ⇒ fail closed.
    let mut md = masterdata();
    let generic = md
        .counter
        .rows
        .iter()
        .find(|r| r.key == "company_account")
        .expect("row")
        .clone();
    let mut specific = generic.clone();
    specific.bookkeeping_method = Some("cash".to_string());
    md.counter.rows.push(specific);
    let err = assess_cases(&cases, &entity, &md).expect_err("ambiguous");
    assert!(
        matches!(&err, EngineError::AmbiguousCounterRule { key, candidates: 2, .. } if key == "company_account"),
        "{err}"
    );

    // entity.bookkeeping_method = None: a method-specific row does not match ⇒ finding for supplier_credit;
    // generic rows still match.
    let md = masterdata();
    let mut no_method = entity.clone();
    no_method.bookkeeping_method = None;
    let assessments = assess_cases(&cases, &no_method, &md).expect("a");
    let supplier = cases
        .iter()
        .position(|c| matches!(&c.facts, SourceFacts::Receipt { payment_method: Some(p), .. } if p == "supplier_credit"))
        .expect("row");
    assert_eq!(
        assessments[supplier].counter_rule, None,
        "0 candidates ⇒ no reference"
    );
    assert!(codes(&assessments[supplier]).contains(&FindingCode::UnresolvedCounterAccount));
    let company = cases
        .iter()
        .position(|c| matches!(&c.facts, SourceFacts::Receipt { payment_method: Some(p), .. } if p == "company_account"))
        .expect("row");
    assert!(assessments[company].counter_rule.is_some());

    // payment_method = None (schema-1.0 style) ⇒ finding, no reference.
    let mut none_pm = cases[company].clone();
    if let SourceFacts::Receipt { payment_method, .. } = &mut none_pm.facts {
        *payment_method = None;
    }
    let a = assess_cases(std::slice::from_ref(&none_pm), &entity, &md).expect("a");
    assert_eq!(a[0].counter_rule, None);
    assert!(codes(&a[0]).contains(&FindingCode::UnresolvedCounterAccount));
    assert_eq!(a[0].status_ceiling, DecisionStatus::Manual);
}

// ---------------------------------------------------------------------------
// H7 — ceiling composition
// ---------------------------------------------------------------------------

#[test]
fn h7_status_ceiling_is_the_least_confident_input() {
    let db = TempDb::new("h7");
    let ws = testgarden(&db);
    let entity = ws.read_entity().expect("e");
    let cases = project_cases(&ws).expect("p");
    let bransle = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row");

    // Rule case Conditional + counter row forced Manual ⇒ Manual.
    let mut md = masterdata();
    for r in md
        .counter
        .rows
        .iter_mut()
        .filter(|r| r.key == "company_account")
    {
        r.automation = "Manual".to_string();
        r.question = Some("Bekräfta motkonto".to_string());
    }
    let a = assess_cases(std::slice::from_ref(&cases[bransle]), &entity, &md).expect("a");
    assert_eq!(a[0].status_ceiling, DecisionStatus::Manual);
    assert!(a[0].counter_rule.is_some());

    // Counter row Automatic with an account (company_account → 1930) + rule case Conditional ⇒ Conditional under draft.
    let md = masterdata();
    let a = assess_cases(std::slice::from_ref(&cases[bransle]), &entity, &md).expect("a");
    assert_eq!(a[0].status_ceiling, DecisionStatus::Conditional);

    // Blocking finding wins regardless of rules (unknown payment method on a mapped category).
    let mut blocked = cases[bransle].clone();
    if let SourceFacts::Receipt { payment_method, .. } = &mut blocked.facts {
        *payment_method = Some("unknown".to_string());
    }
    let a = assess_cases(std::slice::from_ref(&blocked), &entity, &md).expect("a");
    assert_eq!(a[0].status_ceiling, DecisionStatus::Manual);

    // Income ignores counter automation entirely (no reference, ceiling from rule case + draft only).
    let leverans = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("leveransvirke"))
        .expect("row");
    let a = assess_cases(std::slice::from_ref(&cases[leverans]), &entity, &md).expect("a");
    assert_eq!(a[0].counter_rule, None);
    assert_eq!(a[0].status_ceiling, DecisionStatus::Conditional);
    assert_eq!(a[0].vat_rule_id.as_deref(), Some("utg25"));
}

// ---------------------------------------------------------------------------
// H8 / H16 — zero lines, no floats (source scans of the production module)
// ---------------------------------------------------------------------------

#[test]
fn h8_h16_engine_source_has_no_lines_and_no_floats() {
    let src = fs::read_to_string(repo("src/engine.rs")).expect("engine.rs");
    assert!(
        !src.contains("ProposedLine"),
        "Slice 1 defines and constructs no lines"
    );
    assert!(!src.contains("LineRole"));
    assert!(!src.contains("AccountingDecision"));
    for token in ["f32", "f64", "as f32", "as f64"] {
        assert!(
            !src.contains(token),
            "no floats in the production path: {token}"
        );
    }
    // No E1–E6 behaviour: the engine never touches VAT rules or rates.
    assert!(!src.contains("vat_account"));
    assert!(!src.contains("rates"));
    assert!(!src.contains("3740"));
    assert!(!src.contains("1930"));
}

// ---------------------------------------------------------------------------
// H9 — proposal guard (erratum 4: only for Automatic/Conditional references)
// ---------------------------------------------------------------------------

#[test]
fn h9_proposal_guard_applies_to_proposable_references_only() {
    let db = TempDb::new("h9");
    let ws = testgarden(&db);
    let entity = ws.read_entity().expect("e");
    let cases = project_cases(&ws).expect("p");
    let bransle = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row");
    let one = std::slice::from_ref(&cases[bransle]);

    // Rule case (Conditional) pointing at the inactive synthetic account 5999 ⇒ guard rejects.
    let mut md = masterdata();
    md.ruleset
        .cases
        .iter_mut()
        .find(|c| c.case_id == "bransle.default")
        .expect("case")
        .account = Some("5999".to_string());
    let err = assess_cases(one, &entity, &md).expect_err("inactive");
    assert!(
        matches!(&err, EngineError::ProposalGuard { account, reason, .. } if account == "5999" && reason == "inactive"),
        "{err}"
    );

    // engine_proposable=false on 5360 ⇒ rejected for a Conditional rule case ...
    let mut md = masterdata();
    md.chart
        .accounts
        .iter_mut()
        .find(|a| a.number == "5360")
        .expect("acc")
        .roles
        .engine_proposable = false;
    let err = assess_cases(one, &entity, &md).expect_err("not proposable");
    assert!(
        matches!(&err, EngineError::ProposalGuard { reason, .. } if reason == "not engine_proposable"),
        "{err}"
    );
    // ... but NOT for a Manual reference (M3 Manual exception): same masterdata, rule case set to Manual.
    let case = md
        .ruleset
        .cases
        .iter_mut()
        .find(|c| c.case_id == "bransle.default")
        .expect("case");
    case.automation = "Manual".to_string();
    case.question = Some("Kontrollera kontering".to_string());
    let a = assess_cases(one, &entity, &md).expect("Manual reference is not a proposal");
    assert_eq!(a[0].status_ceiling, DecisionStatus::Manual);
    assert_eq!(a[0].rule_case_id.as_deref(), Some("bransle.default"));

    // business_groups: disjoint ⇒ rejected; empty ⇒ passes; empty entity.operations ⇒ only empty-group accounts pass.
    let mut md = masterdata();
    md.chart
        .accounts
        .iter_mut()
        .find(|a| a.number == "5360")
        .expect("acc")
        .roles
        .business_groups = vec!["djur".to_string()];
    let mut skog_only = entity.clone();
    skog_only.operations = vec!["skog".to_string()];
    let err = assess_cases(one, &skog_only, &md).expect_err("disjoint groups");
    assert!(
        matches!(&err, EngineError::ProposalGuard { reason, .. } if reason.contains("business_groups")),
        "{err}"
    );
    let mut skog_djur = entity.clone();
    skog_djur.operations = vec!["skog".to_string(), "djur".to_string()];
    assess_cases(one, &skog_djur, &md).expect("intersecting groups pass");
    let mut no_ops = entity.clone();
    no_ops.operations = Vec::new();
    assess_cases(one, &no_ops, &md).expect_err("empty operations: grouped account fails");
    let md = masterdata(); // 5360 has empty business_groups in the synthetic chart
    assess_cases(one, &no_ops, &md).expect("empty operations: empty-group account passes");

    // Counter row account (1930 for company_account, Automatic) is guarded too.
    let mut md = masterdata();
    md.chart
        .accounts
        .iter_mut()
        .find(|a| a.number == "1930")
        .expect("acc")
        .roles
        .active = false;
    let err = assess_cases(one, &entity, &md).expect_err("counter account inactive");
    assert!(
        matches!(&err, EngineError::ProposalGuard { account, .. } if account == "1930"),
        "{err}"
    );
}

// ---------------------------------------------------------------------------
// H10 — canonical source keys, property references, money invariants
// ---------------------------------------------------------------------------

#[test]
fn h10_schema_1_0_keys_are_derived_and_structural_errors_fail_closed() {
    // Schema 1.0: source_key None ⇒ derived; context all None.
    let db = TempDb::new("h10-1-0");
    let ws = workspace_from(&db, "fixtures/snapshots/minimal-1.0.json");
    let cases = project_cases(&ws).expect("1.0 projects");
    assert!(!cases.is_empty());
    for c in &cases {
        let prefix = match c.source {
            CaseSource::Receipt => "receipt",
            CaseSource::Income => "income",
        };
        assert_eq!(c.source_key, format!("{prefix}:{}", c.row_id));
        assert_eq!(c.context, sieverk::engine::CaseContext::default());
    }
    let md = masterdata();
    let entity = ws.read_entity().expect("e");
    assert_eq!(
        entity.bookkeeping_method, None,
        "1.0 has no accounting profile"
    );
    let a = assess_cases(&cases, &entity, &md).expect("1.0 assesses");
    assert!(a
        .iter()
        .all(|x| x.status_ceiling != DecisionStatus::Automatic));

    // 1.1 row with a non-canonical stored key ⇒ InvalidSourceKey.
    let db = TempDb::new("h10-badkey");
    let ws = testgarden(&db);
    ws.connection()
        .execute_batch("UPDATE receipts SET source_key = 'receipt:999' WHERE selector_position = 3")
        .expect("mutate");
    let err = project_cases(&ws).expect_err("must fail closed");
    assert!(
        matches!(&err, EngineError::InvalidSourceKey { case_seq: 3, found, .. } if found == "receipt:999"),
        "{err}"
    );

    // Unknown property reference ⇒ MissingProperty.
    let db = TempDb::new("h10-prop");
    let ws = testgarden(&db);
    ws.connection()
        .execute_batch("UPDATE income_entries SET property_id = 4242 WHERE snapshot_position = 0")
        .expect("mutate");
    let err = project_cases(&ws).expect_err("must fail closed");
    assert!(
        matches!(
            &err,
            EngineError::MissingProperty {
                case_seq: 21,
                property_id: 4242
            }
        ),
        "{err}"
    );

    // Broken copied-money invariant ⇒ MoneyInvariant.
    let db = TempDb::new("h10-money");
    let ws = testgarden(&db);
    ws.connection()
        .execute_batch("UPDATE receipts SET net_ore = net_ore + 1 WHERE selector_position = 0")
        .expect("mutate");
    let err = project_cases(&ws).expect_err("must fail closed");
    assert!(
        matches!(&err, EngineError::MoneyInvariant { case_seq: 0, .. }),
        "{err}"
    );

    // Empty workspace ⇒ NotIngested wrapped, no panic.
    let db = TempDb::new("h10-empty");
    let ws = Workspace::create(db.path()).expect("create");
    let err = project_cases(&ws).expect_err("empty");
    assert!(
        matches!(
            &err,
            EngineError::Workspace(sieverk::workspace::WorkspaceError::NotIngested)
        ),
        "{err}"
    );
}

// ---------------------------------------------------------------------------
// H11 — MISSING_EVIDENCE is a warning with no effect of its own
// ---------------------------------------------------------------------------

#[test]
fn h11_missing_evidence_is_a_warning_only() {
    let db = TempDb::new("h11");
    let ws = testgarden(&db);
    let md = masterdata();
    let cases = project_cases(&ws).expect("p");
    let assessments = assess_cases(&cases, &ws.read_entity().expect("e"), &md).expect("a");
    for a in &assessments {
        let me = a
            .findings
            .iter()
            .find(|f| f.code == FindingCode::MissingEvidence)
            .expect("all Testgården rows lack evidence");
        assert_eq!(me.severity, Severity::Warning);
    }
    // Bränsle: only finding is MISSING_EVIDENCE ⇒ ceiling stays Conditional (rules + draft), not Manual.
    let bransle = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row");
    assert_eq!(
        codes(&assessments[bransle]),
        vec![FindingCode::MissingEvidence]
    );
    assert_eq!(
        assessments[bransle].status_ceiling,
        DecisionStatus::Conditional
    );
    // A receipt with an image and an income with documents ⇒ no MISSING_EVIDENCE.
    let mut with_image = cases[bransle].clone();
    if let SourceFacts::Receipt { has_image, .. } = &mut with_image.facts {
        *has_image = true;
    }
    let a = assess_cases(
        std::slice::from_ref(&with_image),
        &ws.read_entity().expect("e"),
        &md,
    )
    .expect("a");
    assert!(a[0].findings.is_empty());
}

// ---------------------------------------------------------------------------
// H12 — no VAT findings in Slice 1; VAT ids preserved
// ---------------------------------------------------------------------------

#[test]
fn h12_no_vat_findings_but_vat_rule_ids_preserved() {
    let db = TempDb::new("h12");
    let ws = testgarden(&db);
    let md = masterdata();
    let cases = project_cases(&ws).expect("p");
    let assessments = assess_cases(&cases, &ws.read_entity().expect("e"), &md).expect("a");
    for a in &assessments {
        assert!(!codes(a)
            .iter()
            .any(|c| matches!(c, FindingCode::UnresolvedVat | FindingCode::VatMismatch)));
    }
    let ids: HashSet<Option<String>> = assessments.iter().map(|a| a.vat_rule_id.clone()).collect();
    assert!(ids.contains(&Some("ing25".to_string())));
    assert!(ids.contains(&Some("utg25".to_string())));
    assert!(ids.contains(&None));
    // vat_check=true on a receipt does not create a finding in Slice 1 (preserved only).
    let bransle = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row");
    let mut flagged = cases[bransle].clone();
    flagged.context.vat_check = Some(true);
    let a = assess_cases(
        std::slice::from_ref(&flagged),
        &ws.read_entity().expect("e"),
        &md,
    )
    .expect("a");
    assert_eq!(codes(&a[0]), vec![FindingCode::MissingEvidence]);
}

// ---------------------------------------------------------------------------
// H13 — finding order by rank, dense finding_no (erratum 3)
// ---------------------------------------------------------------------------

#[test]
fn h13_finding_order_is_rank_then_dense_numbering() {
    let db = TempDb::new("h13");
    let ws = testgarden(&db);
    let md = masterdata();
    let cases = project_cases(&ws).expect("p");
    let assessments = assess_cases(&cases, &ws.read_entity().expect("e"), &md).expect("a");
    let unknown = cases
        .iter()
        .position(|c| matches!(&c.facts, SourceFacts::Receipt { payment_method: Some(p), .. } if p == "unknown"))
        .expect("El och drift, unknown");
    let f = &assessments[unknown].findings;
    assert_eq!(
        f.iter().map(|x| (x.finding_no, x.code)).collect::<Vec<_>>(),
        vec![
            (0, FindingCode::UnresolvedCounterAccount),
            (1, FindingCode::UnmappedCategory),
            (2, FindingCode::MissingEvidence),
        ]
    );
    // Rank is the sort key, not the stored number: a case with only MISSING_EVIDENCE has finding_no 0.
    let bransle = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row");
    assert_eq!(assessments[bransle].findings[0].finding_no, 0);
    assert_eq!(FindingCode::UnresolvedVat.as_str(), "UNRESOLVED_VAT");
    assert!(FindingCode::UnresolvedVat < FindingCode::MissingEvidence);
}

// ---------------------------------------------------------------------------
// H14 — legacy triggers
// ---------------------------------------------------------------------------

#[test]
fn h14_legacy_triggers() {
    let db = TempDb::new("h14");
    let ws = testgarden(&db);
    let md = masterdata();
    let entity = ws.read_entity().expect("e");
    let cases = project_cases(&ws).expect("p");
    let bransle = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("Bränsle"))
        .expect("row");
    let mut legacy = cases[bransle].clone();
    if let SourceFacts::Receipt { entry_type, .. } = &mut legacy.facts {
        *entry_type = "income".to_string();
    }
    let a = assess_cases(std::slice::from_ref(&legacy), &entity, &md).expect("a");
    assert!(codes(&a[0]).contains(&FindingCode::LegacyIncomeReceipt));
    assert_eq!(a[0].status_ceiling, DecisionStatus::Manual);

    let leverans = cases
        .iter()
        .position(|c| c.subject.as_deref() == Some("leveransvirke"))
        .expect("row");
    let mut timber = cases[leverans].clone();
    timber.subject = Some("timber_sale".to_string());
    if let SourceFacts::Income { income_type, .. } = &mut timber.facts {
        *income_type = "timber_sale".to_string();
    }
    let a = assess_cases(std::slice::from_ref(&timber), &entity, &md).expect("a");
    assert_eq!(
        codes(&a[0]),
        vec![
            FindingCode::UnmappedCategory,
            FindingCode::UnspecifiedTimberSale,
            FindingCode::MissingEvidence
        ]
    );
    assert_eq!(a[0].status_ceiling, DecisionStatus::Manual);
    assert_eq!(a[0].counter_rule, None);
}

// ---------------------------------------------------------------------------
// H15 — the snapshot JSON is never reopened
// ---------------------------------------------------------------------------

#[test]
fn h15_engine_never_reopens_snapshot_json() {
    let db = TempDb::new("h15");
    let copy = std::env::temp_dir().join(format!(
        "sieverk-engine-{}-snapshot-copy.json",
        std::process::id()
    ));
    fs::copy(repo(TESTGARDEN), &copy).expect("copy");
    let raw = fs::read(&copy).expect("read");
    let mut ws = Workspace::create(db.path()).expect("create");
    ws.ingest(&raw).expect("ingest");
    fs::remove_file(&copy).expect("delete the only JSON the workspace ever saw");
    assert!(!copy.exists());
    let cases = project_cases(&ws).expect("projection needs no JSON");
    let a = assess_cases(&cases, &ws.read_entity().expect("e"), &masterdata())
        .expect("assessment needs no JSON");
    assert_eq!(cases.len(), 27);
    assert_eq!(a.len(), 27);
    let _: &EngineCase = &cases[0];
    let _ = Ore(0);
}

// ---------------------------------------------------------------------------
// H17 — taxonomy version prerequisite (prespec §F.2)
// ---------------------------------------------------------------------------

#[test]
fn h17_taxonomy_version_mismatch_fails_closed_and_none_is_accepted() {
    let db = TempDb::new("h17");
    let ws = testgarden(&db);
    let md = masterdata();
    let cases = project_cases(&ws).expect("p");
    let entity = ws.read_entity().expect("e");
    assert_eq!(entity.taxonomy_version.as_deref(), Some("1.0"));
    assert_eq!(md.ruleset.taxonomy_version, "1.0");
    assess_cases(&cases, &entity, &md).expect("matching taxonomy assesses");

    let mut wrong = entity.clone();
    wrong.taxonomy_version = Some("0.9".to_string());
    let err = assess_cases(&cases, &wrong, &md).expect_err("mismatch must fail closed");
    assert!(
        matches!(&err, EngineError::TaxonomyMismatch { entity_version, ruleset_version }
            if entity_version == "0.9" && ruleset_version == "1.0"),
        "{err}"
    );
    assert!(err.to_string().contains("0.9") && err.to_string().contains("1.0"));

    // Schema 1.0 workspaces carry None and are still assessed.
    let mut none = entity.clone();
    none.taxonomy_version = None;
    assess_cases(&cases, &none, &md).expect("None is accepted");
}
