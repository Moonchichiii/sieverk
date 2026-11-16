//! E3 bridge: real synthetic snapshot -> real DuckDB -> engine -> writer.
//! Export bytes can be checked by independent Django reconciliation.
//! No production masterdata or customer data; the 4470 account is TEST ONLY.
use chrono::NaiveDate;
use sieverk::engine::run_engine;
use sieverk::export_context::VerifiedExportContext;
use sieverk::ruleset::load_masterdata;
use sieverk::sie_writer::{write_sie4i, ExportMetadata, ProgramId};
use sieverk::workspace::Workspace;
use std::fs;
use std::path::{Path, PathBuf};

fn repo(path: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(path)
}

fn exercise(rounding: &str, total: &str, expected: &str, out_var: Option<&str>) {
    let mut snapshot: serde_json::Value = serde_json::from_slice(
        &fs::read(repo("fixtures/snapshots/testgarden-2026-1.2.json")).expect("fixture"),
    )
    .expect("snapshot JSON");
    let receipts = snapshot["receipts"].as_array_mut().expect("receipt array");
    let receipt = receipts
        .iter_mut()
        .find(|r| r["source_key"] == "receipt:1")
        .expect("known test receipt");
    assert_eq!(receipt["net_amount"], "5000.00");
    assert_eq!(receipt["vat_amount"], "1250.00");
    receipt["rounding_amount"] = rounding.into();
    receipt["total_amount"] = total.into();

    let mut md = load_masterdata(&repo("fixtures/masterdata/synthetic/generated"))
        .expect("synthetic masterdata");
    // TEST-ONLY synthetic account already in chart; NOT a customer accounting decision.
    assert!(md
        .chart
        .accounts
        .iter()
        .any(|a| a.number == "4470" && a.roles.active && a.roles.engine_proposable));
    md.counter.rounding_account = Some("4470".to_string());

    let output_dir = std::env::temp_dir().join(format!(
        "sieverk-e3-bridge-{}-{}",
        std::process::id(),
        if rounding.starts_with('-') {
            "neg"
        } else {
            "pos"
        }
    ));
    fs::create_dir_all(&output_dir).expect("temp directory");
    let db = output_dir.join(format!(
        "rounding-{}.duckdb",
        if rounding.starts_with('-') {
            "neg"
        } else {
            "pos"
        }
    ));
    assert!(!db.exists(), "fresh isolated test database");
    let result = (|| {
        let mut ws = Workspace::create(&db).expect("create DuckDB");
        let raw = serde_json::to_vec(&snapshot).expect("synthetic JSON");
        let ingested = ws.ingest_snapshot(&raw).expect("snapshot ingest");
        run_engine(&mut ws, &md).expect("real engine");
        let context = VerifiedExportContext::bind(&ingested, &ws).expect("bind evidence");
        let result = write_sie4i(
            &ws,
            &md,
            &context,
            &ExportMetadata {
                generated_on: NaiveDate::from_ymd_opt(2026, 11, 16).unwrap(),
                program: ProgramId {
                    name: "SIEverk".to_string(),
                    version: "0.4.0".to_string(),
                },
            },
        )
        .expect("real SIE 4I writer");
        let txt = sieverk::decode_sie_bytes(result.bytes.clone());
        assert!(
            txt.contains(expected),
            "fourth SIE #TRANS must match signed rounding"
        );
        let mut found = false;
        for block in txt.split("#VER ").skip(1) {
            if block.contains("20260820") && block.contains("Bränsle") {
                assert_eq!(
                    block
                        .lines()
                        .filter(|line| line.starts_with("#TRANS "))
                        .count(),
                    4
                );
                assert!(block.contains("#TRANS 5360 {} 5000.00"));
                assert!(block.contains("#TRANS 2640 {} 1250.00"));
                assert!(block.contains(&format!("#TRANS 1930 {{}} -{total}")));
                assert!(block.contains(expected));
                found = true;
            }
        }
        assert!(
            found,
            "the original test receipt must produce a real voucher"
        );
        if let Some(var) = out_var {
            if let Some(dest) = std::env::var_os(var) {
                let dest = PathBuf::from(dest);
                assert!(
                    dest.parent().is_some_and(|p| p.is_dir()),
                    "output parent must exist"
                );
                assert!(!dest.exists(), "do not overwrite previous bridge evidence");
                fs::write(&dest, &result.bytes).expect("write synthetic SIE evidence");
            }
        }
    })();
    let _ = fs::remove_file(&db);
    let _ = fs::remove_file(db.with_extension("duckdb.wal"));
    let _ = fs::remove_dir(&output_dir);
    result
}

#[test]
fn e3_real_writer_positive_30_ore() {
    exercise(
        "0.30",
        "6250.30",
        "#TRANS 4470 {} 0.30",
        Some("SIEVERK_E3_BRIDGE_OUT"),
    );
}

#[test]
fn e3_real_writer_negative_30_ore() {
    exercise("-0.30", "6249.70", "#TRANS 4470 {} -0.30", None);
}
