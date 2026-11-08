//! completeness (SIE-COMPLETE-01) — real Testgården snapshot 1.2, real
//! DuckDB workspace, real synthetic masterdata, real engine run, real SIE 4I
//! bytes. Every snapshot row is a voucher or an explicit exclusion, exactly
//! once; every divergence between snapshot, persisted run, re-derivation and
//! SIE file is refused. No mocks, no customer data.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::NaiveDate;
use sieverk::completeness::{
    completeness_report, CaseOutcome, CompletenessError, CompletenessReport, Coverage, Field,
};
use sieverk::engine::{run_engine, CaseSource, DecisionStatus, Exclusion};
use sieverk::export_context::VerifiedExportContext;
use sieverk::money::Ore;
use sieverk::ruleset::{load_masterdata, Masterdata};
use sieverk::sie_writer::{write_sie4i, ExportMetadata, ProgramId, SieExport};
use sieverk::snapshot::Snapshot;
use sieverk::workspace::{Workspace, WorkspaceError};

const TESTGARDEN: &str = "fixtures/snapshots/testgarden-2026-1.2.json";
const SYNTHETIC_ROOT: &str = "fixtures/masterdata/synthetic/generated";
/// SIE-INCOME-01 golden — unchanged by SIE-COMPLETE-01 (the rounding receipt
/// never had a voucher; it now has an explicit reason instead of none).
const INCOME01_SHA256: &str = "6a257db30765f6f9ece895ab0c700f89d7d47d78e4a397f953f5e41d766f7395";

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

struct TempDb(PathBuf);

impl TempDb {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join("sieverk-completeness-tests");
        fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join(format!("{}-{name}.duckdb", std::process::id()));
        let _ = fs::remove_file(&path);
        let _ = fs::remove_file(path.with_extension("duckdb.wal"));
        Self(path)
    }
}

impl Drop for TempDb {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.0);
        let _ = fs::remove_file(self.0.with_extension("duckdb.wal"));
    }
}

fn masterdata() -> Masterdata {
    load_masterdata(&repo(SYNTHETIC_ROOT)).expect("synthetic masterdata")
}

struct Run {
    ws: Workspace,
    snapshot: Snapshot,
    digest: String,
    export: SieExport,
}

/// The canonical path: create → ingest_snapshot → run_engine → bind → write.
fn run(db: &TempDb, md: &Masterdata) -> Run {
    run_raw(db, md, &fs::read(repo(TESTGARDEN)).expect("fixture"))
}

/// Testgården 1.2 keeping only the listed receipt and income ids.
fn testgarden_only(receipts: &[i64], incomes: &[i64]) -> Vec<u8> {
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(repo(TESTGARDEN)).expect("fixture")).expect("json");
    v["receipts"]
        .as_array_mut()
        .expect("receipts")
        .retain(|r| receipts.contains(&r["id"].as_i64().expect("id")));
    v["income_entries"]
        .as_array_mut()
        .expect("incomes")
        .retain(|i| incomes.contains(&i["id"].as_i64().expect("id")));
    serde_json::to_vec(&v).expect("json")
}

fn run_raw(db: &TempDb, md: &Masterdata, raw: &[u8]) -> Run {
    let mut ws = Workspace::create(&db.0).expect("create");
    let ingested = ws.ingest_snapshot(raw).expect("ingest_snapshot");
    run_engine(&mut ws, md).expect("run_engine");
    let ctx = VerifiedExportContext::bind(&ingested, &ws).expect("bind");
    let meta = ExportMetadata {
        generated_on: NaiveDate::from_ymd_opt(2026, 9, 10).expect("date"),
        program: ProgramId {
            name: "SIEverk".to_string(),
            version: "0.4.0".to_string(),
        },
    };
    let export = write_sie4i(&ws, md, &ctx, &meta).expect("export");
    Run {
        snapshot: ingested.snapshot().clone(),
        digest: ingested.snapshot_sha256().to_string(),
        ws,
        export,
    }
}

fn report(r: &Run, md: &Masterdata) -> Result<CompletenessReport, CompletenessError> {
    completeness_report(&r.ws, md, &r.snapshot, &r.digest, &r.export)
}

// K1 — Testgården: 27 rows, 4 vouchers, 23 explicit exclusions, 0 silent
#[test]
fn k1_testgarden_every_row_is_accounted_for_exactly_once() {
    let db = TempDb::new("k1");
    let md = masterdata();
    let r = run(&db, &md);
    // The SIE-INCOME-01 golden is preserved byte for byte.
    assert_eq!(r.export.bytes.len(), 789);
    assert_eq!(r.export.sha256, INCOME01_SHA256);

    let rep = report(&r, &md).expect("complete");
    assert_eq!(rep.cases.len(), 27);
    assert_eq!((rep.receipts.cases, rep.income.cases), (21, 6));
    assert_eq!((rep.vouchers(), rep.excluded(), rep.silent()), (4, 23, 0));
    assert_eq!((rep.receipts.vouchers, rep.income.vouchers), (2, 2));

    // Exactly the snapshot's rows, each once.
    let expected: BTreeSet<String> = r
        .snapshot
        .receipts
        .iter()
        .map(|x| format!("receipt:{}", x.id))
        .chain(
            r.snapshot
                .income_entries
                .iter()
                .map(|x| format!("income:{}", x.id)),
        )
        .collect();
    let got: Vec<&str> = rep.cases.iter().map(|c| c.source_key.as_str()).collect();
    let unique: BTreeSet<String> = got.iter().map(|s| s.to_string()).collect();
    assert_eq!(got.len(), unique.len(), "no row twice");
    assert_eq!(unique, expected, "no row missing, none extra");

    // Voucher numbers 1..N in emission order, exactly the writer's.
    let vouchers: Vec<(usize, i32)> = rep
        .cases
        .iter()
        .filter_map(|c| match c.outcome {
            CaseOutcome::Voucher { ver_no } => Some((ver_no, c.case_seq)),
            CaseOutcome::Excluded { .. } => None,
        })
        .collect();
    assert_eq!(
        vouchers.iter().map(|v| v.0).collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
    assert_eq!(
        vouchers.iter().map(|v| v.1).collect::<Vec<_>>(),
        r.export.voucher_case_seqs
    );

    // The last formerly silent row: the +0.30 rounding receipt.
    let grus = rep
        .cases
        .iter()
        .find(|c| c.source_key == "receipt:18")
        .expect("receipt 18");
    assert_eq!(grus.status, DecisionStatus::Manual);
    assert_eq!(
        grus.outcome,
        CaseOutcome::Excluded {
            reason: Exclusion::RoundingNotImplemented
        }
    );
    // Every exclusion is Manual with a reason; every voucher is not Manual.
    for c in &rep.cases {
        match c.outcome {
            CaseOutcome::Voucher { .. } => assert_ne!(c.status, DecisionStatus::Manual),
            CaseOutcome::Excluded { .. } => assert_eq!(c.status, DecisionStatus::Manual),
        }
    }
    assert_eq!(
        rep.excluded_by_reason(),
        vec![
            (Exclusion::BlockingFindings, 22),
            (Exclusion::ManualAutomation, 0),
            (Exclusion::EntityNotVatRegistered, 0),
            (Exclusion::VatBranchNotImplemented, 0),
            (Exclusion::RoundingNotImplemented, 1),
        ]
    );
    // Amounts reconcile exactly per source.
    for t in [rep.receipts, rep.income] {
        assert_eq!(
            Ore(t.exported_amount.0 + t.excluded_amount.0),
            t.amount,
            "{t:?}"
        );
    }
    // The two income vouchers: 120 000.00 + 47 500.00 kr gross.
    assert_eq!(rep.income.exported_amount, Ore(16_750_000));
    let gross: i64 = rep
        .cases
        .iter()
        .filter(|c| c.source == CaseSource::Income)
        .map(|c| c.amount.0)
        .sum();
    assert_eq!(rep.income.amount, Ore(gross));
}

// K2 — the manifest is deterministic, complete and carries no free text
#[test]
fn k2_manifest_is_deterministic_and_carries_no_free_text() {
    let md = masterdata();
    let (a, b) = (TempDb::new("k2a"), TempDb::new("k2b"));
    let ra = run(&a, &md);
    let rb = run(&b, &md);
    let ma = report(&ra, &md).expect("a").manifest_json().expect("json");
    let mb = report(&rb, &md).expect("b").manifest_json().expect("json");
    assert_eq!(ma, mb, "same input, same manifest bytes");
    assert_eq!(ma.last(), Some(&b'\n'));

    let v: serde_json::Value = serde_json::from_slice(&ma).expect("valid JSON");
    assert_eq!(v["format"], "sieverk-completeness/1");
    assert_eq!(v["si_sha256"], INCOME01_SHA256);
    assert_eq!(v["si_bytes"], 789);
    assert_eq!(v["ruleset_status"], "draft");
    assert_eq!(v["summary"]["cases"], 27);
    assert_eq!(v["summary"]["vouchers"], 4);
    assert_eq!(v["summary"]["excluded"], 23);
    assert_eq!(v["summary"]["silent"], 0);
    assert_eq!(
        v["summary"]["excluded_by_reason"]["ROUNDING_NOT_IMPLEMENTED"],
        1
    );
    let cases = v["cases"].as_array().expect("cases");
    assert_eq!(cases.len(), 27);
    for c in cases {
        match c["outcome"].as_str() {
            Some("voucher") => {
                assert!(c["ver_no"].is_u64() && c["reason"].is_null(), "{c}")
            }
            Some("excluded") => {
                assert!(c["ver_no"].is_null() && c["reason"].is_string(), "{c}")
            }
            other => panic!("unexpected outcome {other:?}"),
        }
    }

    // No identity, name or free text from the snapshot reaches the manifest.
    let text = String::from_utf8(ma).expect("utf8");
    let raw: serde_json::Value =
        serde_json::from_slice(&fs::read(repo(TESTGARDEN)).expect("fixture")).expect("json");
    let mut forbidden: Vec<String> = vec![
        "900101-9802".into(),
        "9001019802".into(),
        "Testgården".into(),
    ];
    for r in raw["receipts"].as_array().expect("receipts") {
        for k in ["vendor", "note", "category"] {
            if let Some(s) = r[k].as_str().filter(|s| s.len() > 2) {
                forbidden.push(s.to_string());
            }
        }
    }
    for i in raw["income_entries"].as_array().expect("incomes") {
        for k in ["buyer_name", "description", "invoice_number"] {
            if let Some(s) = i[k].as_str().filter(|s| s.len() > 2) {
                forbidden.push(s.to_string());
            }
        }
    }
    for needle in &forbidden {
        assert!(!text.contains(needle.as_str()), "{needle:?} in manifest");
    }
}

// K3 — a snapshot row without a persisted case is refused
#[test]
fn k3_snapshot_row_without_a_case_is_refused() {
    let db = TempDb::new("k3");
    let md = masterdata();
    let mut r = run(&db, &md);
    let mut extra = r.snapshot.receipts[0].clone();
    extra.id = 999_999;
    r.snapshot.receipts.push(extra);
    let e = report(&r, &md).expect_err("missing case");
    assert!(
        matches!(&e, CompletenessError::MissingCase { source_key } if source_key == "receipt:999999"),
        "{e}"
    );
}

// K4 — a persisted case without a snapshot row is refused
#[test]
fn k4_case_without_a_snapshot_row_is_refused() {
    let db = TempDb::new("k4");
    let md = masterdata();
    let mut r = run(&db, &md);
    let gone = r.snapshot.income_entries.pop().expect("income");
    let e = report(&r, &md).expect_err("unexpected case");
    assert!(
        matches!(&e, CompletenessError::UnexpectedCase { source_key } if *source_key == format!("income:{}", gone.id)),
        "{e}"
    );
}

// K5 — a snapshot that lists one row twice is refused
#[test]
fn k5_duplicate_snapshot_row_is_refused() {
    let db = TempDb::new("k5");
    let md = masterdata();
    let mut r = run(&db, &md);
    let dup = r.snapshot.income_entries[0].clone();
    r.snapshot.income_entries.push(dup);
    let e = report(&r, &md).expect_err("duplicate");
    assert!(
        matches!(&e, CompletenessError::DuplicateSourceRow { .. }),
        "{e}"
    );
}

// K6 — another snapshot digest is refused
#[test]
fn k6_snapshot_digest_mismatch_is_refused() {
    let db = TempDb::new("k6");
    let md = masterdata();
    let mut r = run(&db, &md);
    r.digest = "0".repeat(64);
    let e = report(&r, &md).expect_err("digest");
    assert!(matches!(e, CompletenessError::SnapshotMismatch), "{e}");
}

// K7 — the SIE file must hold exactly the line-bearing cases, unaltered
#[test]
fn k7_tampered_export_is_refused() {
    let db = TempDb::new("k7");
    let md = masterdata();
    let mut r = run(&db, &md);
    let good = r.export.clone();
    type Tamper = fn(&mut SieExport);
    let tampers: [(&str, Tamper); 5] = [
        ("dropped voucher", |x| {
            x.voucher_case_seqs.pop();
        }),
        ("reordered", |x| x.voucher_case_seqs.swap(0, 1)),
        ("count", |x| x.vouchers += 1),
        ("foreign case", |x| x.voucher_case_seqs[0] = 0),
        ("bytes", |x| {
            let n = x.bytes.len() - 3;
            x.bytes[n] = b'9';
        }),
    ];
    for (name, tamper) in tampers {
        r.export = good.clone();
        tamper(&mut r.export);
        let e = report(&r, &md).expect_err(name);
        assert!(
            matches!(e, CompletenessError::VoucherMismatch { .. }),
            "{name}: {e}"
        );
    }
    r.export = good;
    report(&r, &md).expect("untampered export is complete");
}

// K8 — masterdata that would decide differently than the persisted run is refused
#[test]
fn k8_rederivation_divergence_is_refused() {
    let db = TempDb::new("k8");
    let md = masterdata();
    let r = run(&db, &md);
    let mut other = masterdata();
    other
        .ruleset
        .cases
        .iter_mut()
        .find(|c| c.case_id == "bransle.default")
        .expect("case")
        .automation = "Manual".to_string();
    let e = report(&r, &other).expect_err("diverges");
    assert!(
        matches!(
            e,
            CompletenessError::RunMismatch {
                field: Field::Status,
                ..
            }
        ),
        "{e}"
    );
}

// K9 — a tampered persisted run never reaches the accounting (verify_run first)
#[test]
fn k9_tampered_persisted_run_is_refused() {
    let db = TempDb::new("k9");
    let md = masterdata();
    let r = run(&db, &md);
    r.ws.connection()
        .execute_batch("UPDATE findings SET message = 'x' WHERE finding_no = 0 AND case_seq = 0")
        .expect("tamper");
    let e = report(&r, &md).expect_err("tampered");
    assert!(
        matches!(e, CompletenessError::Workspace(WorkspaceError::Corrupt(_))),
        "{e}"
    );
}

// K10 — SIE-OUT-01 coverage: partial (Testgården), full (only line-bearing
// rows), empty (no rows). `full` only when nothing is excluded.
#[test]
fn k10_coverage_is_full_only_without_exclusions() {
    let md = masterdata();
    let db = TempDb::new("k10-partial");
    let rep = report(&run(&db, &md), &md).expect("partial");
    assert_eq!(rep.coverage(), Coverage::Partial);

    let db = TempDb::new("k10-full");
    let r = run_raw(&db, &md, &testgarden_only(&[1, 4], &[2, 6]));
    let rep = report(&r, &md).expect("full");
    assert_eq!((rep.cases.len(), rep.vouchers(), rep.excluded()), (4, 4, 0));
    assert_eq!(rep.coverage(), Coverage::Full);
    let v: serde_json::Value =
        serde_json::from_slice(&rep.manifest_json().expect("json")).expect("valid");
    assert_eq!(v["summary"]["coverage"], "full");

    // One excluded row more ⇒ partial again (fail-closed).
    let db = TempDb::new("k10-one-more");
    let r = run_raw(&db, &md, &testgarden_only(&[1, 4, 18], &[2, 6]));
    let rep = report(&r, &md).expect("partial");
    assert_eq!((rep.excluded(), rep.coverage()), (1, Coverage::Partial));

    let db = TempDb::new("k10-empty");
    let r = run_raw(&db, &md, &testgarden_only(&[], &[]));
    let rep = report(&r, &md).expect("empty");
    assert_eq!((rep.cases.len(), rep.coverage()), (0, Coverage::Empty));
}
