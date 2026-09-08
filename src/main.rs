use std::env;
use std::fs;
use std::path::Path;
use std::process::ExitCode;

use sieverk::accounts::{parse_accounts, AccountData};
use sieverk::decode_sie_bytes;
use sieverk::metadata::{parse_metadata, Metadata};
use sieverk::money::Ore;
use sieverk::ruleset::load_masterdata;
use sieverk::snapshot::{parse_snapshot, Snapshot};
use sieverk::validator::{validate, Report, Severity};
use sieverk::vouchers::{parse_vouchers, VoucherData};
use sieverk::workspace::{IngestOutcome, Workspace};

struct Parsed {
    meta: Metadata,
    acc: AccountData,
    vou: VoucherData,
    report: Report,
    tag_lines: usize,
}

fn load_and_parse(path: &str) -> Result<Parsed, String> {
    let bytes = fs::read(path).map_err(|e| format!("could not read {path}: {e}"))?;
    let text = decode_sie_bytes(bytes);
    let meta = parse_metadata(&text);
    let acc = parse_accounts(&text);
    let vou = parse_vouchers(&text);
    let report = validate(&meta, &acc, &vou);
    let tag_lines = text
        .lines()
        .filter(|l| l.trim_start().starts_with('#'))
        .count();
    Ok(Parsed {
        meta,
        acc,
        vou,
        report,
        tag_lines,
    })
}

fn main() -> ExitCode {
    let argv: Vec<String> = env::args().skip(1).collect();
    // DuckDB workspace (SV-02D): explicit paths, no defaults, strict options.
    match argv.first().map(String::as_str) {
        Some("ingest") => {
            return match parse_options(&argv[1..], &["--snapshot", "--workspace"]) {
                Ok(opts) => finish(ingest_workspace(Path::new(&opts[0]), Path::new(&opts[1]))),
                Err(e) => {
                    eprintln!("{e}");
                    eprintln!("usage: sieverk ingest --snapshot <snapshot.json> --workspace <file.duckdb>");
                    ExitCode::FAILURE
                }
            };
        }
        Some("inspect-workspace") => {
            return match parse_options(&argv[1..], &["--workspace"]) {
                Ok(opts) => finish(inspect_workspace(Path::new(&opts[0]))),
                Err(e) => {
                    eprintln!("{e}");
                    eprintln!("usage: sieverk inspect-workspace --workspace <file.duckdb>");
                    ExitCode::FAILURE
                }
            };
        }
        _ => {}
    }
    // Masterdata takes an explicit root — there is no default root, by
    // contract (mastermatris v1.2 §D).
    if argv.first().map(String::as_str) == Some("inspect-masterdata") {
        return match argv.as_slice() {
            [_, flag, root] if flag.as_str() == "--root" => inspect_masterdata(Path::new(root)),
            _ => {
                eprintln!("usage: sieverk inspect-masterdata --root <generated-dir>");
                ExitCode::FAILURE
            }
        };
    }
    let mut args = argv.into_iter();
    // The small CLI still does not justify a parser-generator dependency.
    // A bare path is treated as inspect-sie, which also keeps the CI
    // smoke step (`cargo run -- fixtures/...`) working unchanged.
    let (command, path) = match (args.next(), args.next()) {
        (Some(cmd), Some(path))
            if cmd == "inspect-sie" || cmd == "validate-sie" || cmd == "inspect-snapshot" =>
        {
            (cmd, path)
        }
        (Some(path), None) => ("inspect-sie".to_string(), path),
        _ => {
            eprintln!("usage: sieverk <inspect-sie|validate-sie> <file.se>");
            eprintln!("       sieverk inspect-snapshot <snapshot.json>");
            eprintln!("       sieverk inspect-masterdata --root <generated-dir>");
            eprintln!("       sieverk ingest --snapshot <snapshot.json> --workspace <file.duckdb>");
            eprintln!("       sieverk inspect-workspace --workspace <file.duckdb>");
            return ExitCode::FAILURE;
        }
    };

    if command == "inspect-snapshot" {
        return match inspect_snapshot(&path) {
            Ok(text) => {
                print!("{text}");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("{e}");
                ExitCode::FAILURE
            }
        };
    }

    let parsed = match load_and_parse(&path) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::FAILURE;
        }
    };

    match command.as_str() {
        "validate-sie" => run_validate(&path, &parsed),
        _ => run_inspect(&path, &parsed),
    }
}

fn run_inspect(path: &str, p: &Parsed) -> ExitCode {
    fn show(v: &Option<String>) -> &str {
        v.as_deref().unwrap_or("—")
    }

    println!("sieverk — SIE inspection");
    println!("File:        {path}");
    println!(
        "SIE type:    {}",
        p.meta.sie_type.map_or("—".to_string(), |t| t.to_string())
    );
    println!("Company:     {}", show(&p.meta.company_name));
    println!("Org number:  {}", show(&p.meta.org_number));
    for fy in &p.meta.fiscal_years {
        println!(
            "Fiscal year: {} – {} (index {})",
            fy.start, fy.end, fy.index
        );
    }
    println!("Currency:    {}", show(&p.meta.currency));
    println!("Accounts:    {}", p.acc.accounts.len());
    println!("Balances:    {} rows", p.acc.balances.len());
    println!("Vouchers:    {}", p.vou.vouchers.len());
    println!(
        "Ledger rows: {}",
        p.vou.vouchers.iter().map(|v| v.rows.len()).sum::<usize>()
    );
    println!("Tag lines:   {}", p.tag_lines);

    let parser_warnings = p.meta.warnings.len() + p.acc.warnings.len() + p.vou.warnings.len();
    println!("Parser notes: {parser_warnings}");
    println!(
        "Findings:    {} errors, {} warnings",
        p.report.error_count(),
        p.report.warning_count()
    );
    println!("Status:      {}", p.report.status());

    ExitCode::SUCCESS
}

fn run_validate(path: &str, p: &Parsed) -> ExitCode {
    println!("sieverk — SIE validation");
    println!("File:        {path}");

    for f in &p.report.findings {
        let sev = match f.severity {
            Severity::Error => "ERROR  ",
            Severity::Warning => "WARNING",
        };
        println!("{sev} {}: {}", f.code.as_str(), f.message);
    }
    for w in p
        .meta
        .warnings
        .iter()
        .chain(p.acc.warnings.iter())
        .chain(p.vou.warnings.iter())
    {
        println!("PARSER  {w}");
    }

    println!(
        "Errors:      {}  Warnings: {}",
        p.report.error_count(),
        p.report.warning_count()
    );
    println!("Status:      {}", p.report.status());

    if p.report.error_count() > 0 {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

/// Reads and validates a snapshot; returns the report text, or the reason it
/// could not be read. Kept separate from `main` so the CLI is testable
/// without spawning a process.
fn inspect_snapshot(path: &str) -> Result<String, String> {
    let bytes = fs::read(path).map_err(|e| format!("could not read {path}: {e}"))?;
    let snap = parse_snapshot(&bytes).map_err(|e| format!("{path}: {e}"))?;
    Ok(render_snapshot(path, &snap))
}

/// Org numbers are personnummer for enskild firma — never echo them whole.
fn mask_org_number(org: &str) -> String {
    let keep = 4;
    let total = org.chars().count();
    org.chars()
        .enumerate()
        .map(|(i, c)| if i + keep < total { '*' } else { c })
        .collect()
}

fn render_snapshot(path: &str, s: &Snapshot) -> String {
    let receipts_total: Ore = s.receipts.iter().map(|r| r.total_amount).sum();
    let receipts_vat: Ore = s.receipts.iter().map(|r| r.vat_amount).sum();
    let income_inc: Ore = s.income_entries.iter().map(|e| e.amount_inc_vat).sum();
    let org = s
        .entity
        .org_number
        .as_deref()
        .map_or("—".to_string(), mask_org_number);

    let mut out = String::new();
    out.push_str("sieverk — snapshot inspection\n");
    out.push_str(&format!("File:        {path}\n"));
    out.push_str(&format!("Schema:      {}\n", s.schema_version));
    out.push_str(&format!(
        "Entity:      {} (org.nr {org})\n",
        s.entity.display_name
    ));
    out.push_str(&format!("Income year: {}\n", s.income_year));
    out.push_str(&format!(
        "Locked:      {}\n",
        if s.lock.all_properties_locked {
            "all properties locked"
        } else {
            "not all properties locked"
        }
    ));
    out.push_str(&format!("Properties:  {}\n", s.properties.len()));
    out.push_str(&format!(
        "Receipts:    {} (total {receipts_total}, vat {receipts_vat})\n",
        s.receipts.len()
    ));
    out.push_str(&format!(
        "Incomes:     {} (inc {income_inc})\n",
        s.income_entries.len()
    ));
    out.push_str(&format!("Audit chain: {}\n", s.audit_chain.len()));
    out
}

fn inspect_masterdata(root: &Path) -> ExitCode {
    let md = match load_masterdata(root) {
        Ok(md) => md,
        Err(e) => {
            eprintln!("masterdata invalid: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("sieverk — masterdata inspection");
    println!("Root:          {}", root.display());
    println!(
        "Chart:         {} {} ({})",
        md.chart.chart_id, md.chart.version, md.chart.review_status
    );
    println!("Profile:       {}", md.chart.profile_scope);
    println!(
        "Workbook:      {} sha256 {}",
        md.chart.header.workbook, md.chart.header.workbook_sha256
    );
    println!(
        "Accounts:      {} ({} must_include)",
        md.chart.accounts.len(),
        md.chart.must_include_numbers().len()
    );
    println!(
        "SRU rows:      {} ({} verified)",
        md.sru.rows.len(),
        md.sru.rows.iter().filter(|r| r.verified).count()
    );
    println!("VAT rules:     {}", md.vat.rules.len());
    println!("Counter rules: {}", md.counter.rows.len());
    println!(
        "Ruleset:       {} taxonomy {}",
        md.ruleset.ruleset_version, md.ruleset.taxonomy_version
    );
    let automatic = md
        .ruleset
        .cases
        .iter()
        .filter(|c| c.automation == "Automatic")
        .count();
    let downgraded = md
        .ruleset
        .cases
        .iter()
        .filter(|c| c.downgraded_from_automatic)
        .count();
    println!(
        "Cases:         {} ({} Automatic, {} downgraded to Conditional)",
        md.ruleset.cases.len(),
        automatic,
        downgraded
    );
    println!(
        "Status:        {}",
        if md.is_draft() {
            "DRAFT — preliminary output only"
        } else {
            "APPROVED"
        }
    );
    ExitCode::SUCCESS
}

/// Print a command's text on success, its error on failure — never both.
fn finish(result: Result<String, String>) -> ExitCode {
    match result {
        Ok(text) => {
            print!("{text}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

/// Strict `--name value` parsing for the workspace commands: every option in
/// `required` exactly once, nothing else. Values come back in `required`
/// order. No defaults, no positionals, no silent acceptance of a typo.
fn parse_options(args: &[String], required: &[&str]) -> Result<Vec<String>, String> {
    let mut values: Vec<Option<String>> = vec![None; required.len()];
    let mut i = 0;
    while i < args.len() {
        let arg = &args[i];
        let Some(slot) = required.iter().position(|r| *r == arg.as_str()) else {
            return Err(if arg.starts_with('-') {
                format!("unknown option {arg}")
            } else {
                format!("unexpected argument {arg}")
            });
        };
        let Some(value) = args.get(i + 1) else {
            return Err(format!("{arg} requires a value"));
        };
        if value.starts_with("--") {
            return Err(format!("{arg} requires a value"));
        }
        if values[slot].is_some() {
            return Err(format!("{arg} given more than once"));
        }
        values[slot] = Some(value.clone());
        i += 2;
    }
    let mut out = Vec::with_capacity(required.len());
    for (name, value) in required.iter().zip(values) {
        out.push(value.ok_or_else(|| format!("missing required option {name}"))?);
    }
    Ok(out)
}

/// `ingest`: raw snapshot bytes → new workspace file → `Workspace::ingest`.
/// Uses the proven `create()` contract (an existing path is refused) and
/// changes nothing about what happens to an empty file if ingest fails
/// afterwards — that behaviour is tested and documented, not invented here.
/// Output is evidence only: no rows, no names, no org number.
fn ingest_workspace(snapshot: &Path, workspace: &Path) -> Result<String, String> {
    let raw =
        fs::read(snapshot).map_err(|e| format!("could not read {}: {e}", snapshot.display()))?;
    let mut ws = Workspace::create(workspace).map_err(|e| e.to_string())?;
    let outcome = ws.ingest(&raw).map_err(|e| format!("ingest failed: {e}"))?;
    ws.close().map_err(|e| e.to_string())?;
    match outcome {
        IngestOutcome::Ingested {
            snapshot_sha256,
            properties,
            receipts,
            income_entries,
            audit_items,
        } => Ok(format!(
            "workspace={}\nsnapshot_sha256={snapshot_sha256}\nproperties={properties}\n\
             receipts={receipts}\nincome_entries={income_entries}\naudit_items={audit_items}\n",
            workspace.display()
        )),
        IngestOutcome::AlreadyIngested { snapshot_sha256 } => Err(format!(
            "workspace already holds snapshot {snapshot_sha256} (unexpected after create)"
        )),
    }
}

/// `inspect-workspace`: open (never create) and report through the typed
/// DuckDB readback only — no snapshot JSON, no row dumps, no org number.
fn inspect_workspace(workspace: &Path) -> Result<String, String> {
    let ws = Workspace::open(workspace).map_err(|e| e.to_string())?;
    let schema = ws.schema_version().map_err(|e| e.to_string())?;
    let meta = ws.read_meta().map_err(|e| e.to_string())?;
    // Read (and thereby validate) entity_context, but print nothing from it:
    // display_name can be a person's name for an enskild firma.
    ws.read_entity().map_err(|e| e.to_string())?;
    let properties = ws.read_properties().map_err(|e| e.to_string())?.len();
    let receipts = ws.read_receipts().map_err(|e| e.to_string())?.len();
    let income_entries = ws.read_income_entries().map_err(|e| e.to_string())?.len();
    let audit_items = ws.read_audit_chain().map_err(|e| e.to_string())?.len();
    let fingerprint = ws.logical_fingerprint().map_err(|e| e.to_string())?;
    Ok(format!(
        "workspace_schema={schema}\nsnapshot_sha256={}\nsnapshot_schema_version={}\nowner_id={}\n\
         income_year={}\nproperties={properties}\nreceipts={receipts}\n\
         income_entries={income_entries}\naudit_items={audit_items}\nlogical_fingerprint={fingerprint}\n",
        meta.snapshot_sha256,
        meta.schema_version,
        meta.owner_id,
        meta.income_year
    ))
}

#[cfg(test)]
mod tests {
    use super::{
        ingest_workspace, inspect_snapshot, inspect_workspace, mask_org_number, parse_options,
    };
    use sieverk::workspace::{Workspace, WorkspaceError};
    use sieverk::{accounts, decode_sie_bytes, metadata, validator, vouchers};
    use std::path::{Path, PathBuf};

    /// The week-one finish line, part one: the valid fixture comes out
    /// of the full pipeline with a clean verdict.
    #[test]
    fn fixture_valid_passes_validation() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let text = decode_sie_bytes(bytes);
        let report = validator::validate(
            &metadata::parse_metadata(&text),
            &accounts::parse_accounts(&text),
            &vouchers::parse_vouchers(&text),
        );
        assert!(report.findings.is_empty());
        assert_eq!(report.status(), validator::Status::Valid);
    }

    /// The week-one finish line, part two: the broken fixture finally
    /// hears its verdict — with the exact difference, in öre.
    #[test]
    fn fixture_unbalanced_fails_with_exact_difference() {
        let bytes = std::fs::read("fixtures/invalid_unbalanced.se")
            .expect("fixture file should exist — run from the crate root");
        let text = decode_sie_bytes(bytes);
        let report = validator::validate(
            &metadata::parse_metadata(&text),
            &accounts::parse_accounts(&text),
            &vouchers::parse_vouchers(&text),
        );
        assert_eq!(report.error_count(), 1);
        assert_eq!(report.findings[0].code, validator::Code::VoucherNotBalanced);
        assert_eq!(report.findings[0].message, "A-1 sums to -100.00");
        assert_eq!(report.status(), validator::Status::Invalid);
    }

    /// Full pipeline on the real fixture: bytes → CP437 decode →
    /// tokenizer → accounts. Locks the fixture's account inventory and
    /// one exact Decimal amount, å intact and all.
    #[test]
    fn fixture_accounts_survive_full_pipeline() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let acc = accounts::parse_accounts(&decode_sie_bytes(bytes));

        assert_eq!(acc.accounts.len(), 7);
        assert_eq!(acc.balances.len(), 8);
        assert!(acc.warnings.is_empty());

        let skogsvard = acc
            .accounts
            .iter()
            .find(|a| a.number == "6390")
            .expect("account 6390 should exist in the fixture");
        assert_eq!(
            skogsvard.name.as_deref(),
            Some("Skogsvård och övriga kostnader")
        );

        let opening_1930 = acc
            .balances
            .iter()
            .find(|b| b.kind == accounts::BalanceKind::Opening && b.account == "1930")
            .expect("opening balance for 1930 should exist");
        let expected =
            sieverk::money::Ore::parse("125000.00").expect("test literal should be a valid amount");
        assert_eq!(opening_1930.amount, expected);
    }

    /// Full pipeline on the real fixture: bytes → CP437 decode →
    /// tokenizer → vouchers. Locks both vouchers, all six rows, and one
    /// exact amount.
    #[test]
    fn fixture_vouchers_survive_full_pipeline() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let vou = vouchers::parse_vouchers(&decode_sie_bytes(bytes));

        assert_eq!(vou.vouchers.len(), 2);
        assert!(vou.warnings.is_empty());

        let first = &vou.vouchers[0];
        assert_eq!(first.series, "A");
        assert_eq!(first.text.as_deref(), Some("Diesel skogsmaskin"));
        assert_eq!(first.rows.len(), 3);
        let expected =
            sieverk::money::Ore::parse("-1250.00").expect("test literal should be a valid amount");
        assert_eq!(first.rows[0].amount, expected);

        assert_eq!(vou.vouchers[1].rows.len(), 3);
    }

    /// Full pipeline on the real fixture: bytes → CP437 decode →
    /// tokenizer → metadata. If this is green, the whole chain holds.
    #[test]
    fn fixture_metadata_survives_full_pipeline() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let meta = metadata::parse_metadata(&decode_sie_bytes(bytes));

        assert_eq!(meta.sie_type, Some(4));
        assert_eq!(meta.company_name.as_deref(), Some("Demo Skogsbruk AB"));
        assert_eq!(meta.org_number.as_deref(), Some("999999-9999"));
        assert_eq!(meta.fiscal_years.len(), 2);
        assert_eq!(meta.currency.as_deref(), Some("SEK"));
        assert!(meta.warnings.is_empty());
    }

    /// Green from minute one: proves the CP437 round-trip works and the
    /// fixtures are wired up. If this fails, nothing else matters yet.
    #[test]
    fn fixture_survives_cp437_decoding() {
        let bytes = std::fs::read("fixtures/minimal_valid.se")
            .expect("fixture file should exist — run from the crate root");
        let text = decode_sie_bytes(bytes);

        assert!(text.contains("Företagskonto"), "ö did not survive decoding");
        assert!(text.contains("Skogsvård"), "å did not survive decoding");
        assert!(text.contains("Intäkter"), "ä did not survive decoding");
        assert!(text.contains("#SIETYP 4"));
    }

    #[test]
    fn unbalanced_fixture_exists_for_validator_work() {
        let bytes = std::fs::read("fixtures/invalid_unbalanced.se")
            .expect("fixture file should exist — run from the crate root");
        let text = decode_sie_bytes(bytes);

        // -1000.00 + 900.00 = -100.00: your future validator must catch this.
        assert!(text.contains("Reparation traktor"));
    }

    // -- inspect-snapshot -------------------------------------------------------

    #[test]
    fn cli_inspect_snapshot_ok() {
        let text = inspect_snapshot("fixtures/snapshots/minimal-1.1.json")
            .expect("valid fixture inspects cleanly");
        assert!(text.contains("Schema:      1.1"));
        assert!(text.contains("Income year: 2026"));
        assert!(text.contains("Receipts:    2 (total 2249.00, vat 450.00)"));
        assert!(text.contains("Incomes:     1 (inc 56250.00)"));
    }

    #[test]
    fn cli_inspect_snapshot_masks_org_number() {
        let text = inspect_snapshot("fixtures/snapshots/minimal-1.1.json")
            .expect("valid fixture inspects cleanly");
        assert!(text.contains("org.nr *******9999"));
        assert!(!text.contains("999999-9999"));
        assert_eq!(mask_org_number("999999-9999"), "*******9999");
        assert_eq!(mask_org_number("12"), "12");
    }

    #[test]
    fn cli_inspect_snapshot_reports_invalid_file_with_path() {
        let e = inspect_snapshot("fixtures/snapshots/invalid-net.json")
            .expect_err("invalid fixture must not inspect");
        assert!(e.contains("receipts[0].net_amount"), "{e}");
        assert!(e.contains("receipt:1001"), "{e}");
    }

    #[test]
    fn cli_inspect_snapshot_reports_missing_file() {
        let e = inspect_snapshot("fixtures/snapshots/does-not-exist.json")
            .expect_err("missing file is an error");
        assert!(e.starts_with("could not read"), "{e}");
    }

    // -- DuckDB workspace CLI (SV-02D slice 5) ---------------------------------

    const TESTGARDEN: &str = "fixtures/snapshots/testgarden-2026-1.1.json";
    const TESTGARDEN_DIGEST: &str =
        "5d38739ba786009d1b3ad2d00b69582c8ecb349fcf5be8aa2b16dbb113c9c950";

    /// A unique, absent path under the OS temp dir; removed again on drop.
    struct TempDb(PathBuf);

    impl TempDb {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join("sieverk-cli-tests");
            std::fs::create_dir_all(&dir).expect("temp dir");
            let path = dir.join(format!("{}-{name}.duckdb", std::process::id()));
            let _ = std::fs::remove_file(&path);
            let _ = std::fs::remove_file(path.with_extension("duckdb.wal"));
            Self(path)
        }
        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDb {
        fn drop(&mut self) {
            let _ = std::fs::remove_file(&self.0);
            let _ = std::fs::remove_file(self.0.with_extension("duckdb.wal"));
        }
    }

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| (*s).to_string()).collect()
    }

    #[test]
    fn cli_ingest_creates_workspace_from_testgarden() {
        let db = TempDb::new("ingest");
        assert!(!db.path().exists());
        let text = ingest_workspace(Path::new(TESTGARDEN), db.path()).expect("ingest");
        assert!(db.path().is_file());
        assert!(
            text.contains(&format!("snapshot_sha256={TESTGARDEN_DIGEST}\n")),
            "{text}"
        );
        assert!(text.contains("properties=2\n"), "{text}");
        assert!(text.contains("receipts=21\n"), "{text}");
        assert!(text.contains("income_entries=6\n"), "{text}");
        assert!(text.contains("audit_items=0\n"), "{text}");
        assert!(
            !text.contains("999999-0006"),
            "org number must not be printed"
        );
        assert!(!text.contains("Röjarlaget"), "no vendor rows");
        let ws = Workspace::open(db.path()).expect("reopen");
        assert_eq!(ws.read_properties().expect("p").len(), 2);
        assert_eq!(ws.read_receipts().expect("r").len(), 21);
        assert_eq!(ws.read_income_entries().expect("i").len(), 6);
        assert_eq!(ws.read_audit_chain().expect("a").len(), 0);
        assert_eq!(ws.snapshot_sha256().expect("digest"), TESTGARDEN_DIGEST);
    }

    #[test]
    fn cli_ingest_refuses_existing_workspace() {
        let db = TempDb::new("ingest-existing");
        ingest_workspace(Path::new(TESTGARDEN), db.path()).expect("first ingest");
        let before = std::fs::read(db.path()).expect("read");
        let e = ingest_workspace(Path::new(TESTGARDEN), db.path()).expect_err("must refuse");
        assert!(e.contains("already exists"), "{e}");
        assert_eq!(
            std::fs::read(db.path()).expect("read"),
            before,
            "never truncated or overwritten"
        );
        let ws = Workspace::open(db.path()).expect("still a valid workspace");
        assert_eq!(ws.read_receipts().expect("r").len(), 21);
    }

    #[test]
    fn cli_ingest_invalid_snapshot_is_error() {
        let db = TempDb::new("ingest-invalid");
        let e = ingest_workspace(Path::new("fixtures/snapshots/invalid-net.json"), db.path())
            .expect_err("invalid snapshot must fail");
        assert!(e.starts_with("ingest failed:"), "{e}");
        assert!(
            !e.contains("snapshot_sha256="),
            "no success text after an error"
        );
        // Existing contract, documented not invented: create() succeeded before
        // the typed parse refused the snapshot, so an EMPTY workspace file
        // remains — open() works and every read is NotIngested.
        assert!(db.path().is_file());
        let ws = Workspace::open(db.path()).expect("empty workspace opens");
        assert!(matches!(
            ws.snapshot_sha256(),
            Err(WorkspaceError::NotIngested)
        ));
        // A missing snapshot file is refused before any workspace is created.
        let db2 = TempDb::new("ingest-missing-snapshot");
        let e = ingest_workspace(
            Path::new("fixtures/snapshots/does-not-exist.json"),
            db2.path(),
        )
        .expect_err("missing snapshot");
        assert!(e.starts_with("could not read"), "{e}");
        assert!(
            !db2.path().exists(),
            "no workspace file for a missing snapshot"
        );
    }

    #[test]
    fn cli_inspect_workspace_reads_workspace_only() {
        let db = TempDb::new("inspect");
        ingest_workspace(Path::new(TESTGARDEN), db.path()).expect("ingest");
        // From here on only the .duckdb file is consulted.
        let text = inspect_workspace(db.path()).expect("inspect");
        assert!(text.contains("workspace_schema=1\n"), "{text}");
        assert!(
            text.contains(&format!("snapshot_sha256={TESTGARDEN_DIGEST}\n")),
            "{text}"
        );
        assert!(text.contains("snapshot_schema_version=1.1\n"), "{text}");
        assert!(text.contains("income_year=2026\n"), "{text}");
        assert!(
            !text.contains("display_name="),
            "no entity name in evidence output: {text}"
        );
        assert!(!text.contains("Testgården"), "{text}");
        assert!(text.contains("properties=2\n"), "{text}");
        assert!(text.contains("receipts=21\n"), "{text}");
        assert!(text.contains("income_entries=6\n"), "{text}");
        assert!(text.contains("audit_items=0\n"), "{text}");
        let fp = text
            .lines()
            .find_map(|l| l.strip_prefix("logical_fingerprint="))
            .expect("fingerprint line");
        assert_eq!(fp.len(), 64);
        assert!(fp.chars().all(|c| c.is_ascii_hexdigit()));
        let ws = Workspace::open(db.path()).expect("reopen");
        assert_eq!(fp, ws.logical_fingerprint().expect("fp"));
        // Privacy: nothing from the rows leaks — no org number, vendors, notes.
        assert!(!text.contains("999999-0006"), "{text}");
        assert!(!text.contains("Röjarlaget"), "{text}");
        assert!(!text.contains("Norrskogen AB"), "{text}");
        assert!(!text.contains("ReceiptRow"), "{text}");
        assert!(!text.contains("receipt:"), "{text}");
    }

    #[test]
    fn cli_inspect_workspace_missing_path_is_error() {
        let db = TempDb::new("inspect-missing");
        let e = inspect_workspace(db.path()).expect_err("missing file");
        assert!(e.contains("does not exist"), "{e}");
        assert!(!db.path().exists(), "inspect must never create a file");
    }

    #[test]
    fn cli_inspect_workspace_empty_is_not_ingested() {
        let db = TempDb::new("inspect-empty");
        Workspace::create(db.path())
            .expect("create")
            .close()
            .expect("close");
        let e = inspect_workspace(db.path()).expect_err("empty workspace");
        assert!(e.contains("no ingested snapshot"), "{e}");
    }

    #[test]
    fn cli_option_parsing_is_strict() {
        let req = &["--snapshot", "--workspace"];
        let ok = parse_options(
            &args(&["--workspace", "w.duckdb", "--snapshot", "s.json"]),
            req,
        )
        .expect("both present, any order");
        assert_eq!(ok, vec!["s.json".to_string(), "w.duckdb".to_string()]);
        let e = parse_options(&args(&["--snapshot", "s.json"]), req).expect_err("missing");
        assert!(e.contains("missing required option --workspace"), "{e}");
        let e = parse_options(
            &args(&["--snapshot", "a", "--snapshot", "b", "--workspace", "w"]),
            req,
        )
        .expect_err("duplicate");
        assert!(e.contains("--snapshot given more than once"), "{e}");
        let e = parse_options(
            &args(&["--snapshot", "s", "--workspace", "w", "--force"]),
            req,
        )
        .expect_err("unknown");
        assert!(e.contains("unknown option --force"), "{e}");
        let e =
            parse_options(&args(&["--snapshot", "s", "--workspace"]), req).expect_err("no value");
        assert!(e.contains("--workspace requires a value"), "{e}");
        let e = parse_options(&args(&["--snapshot", "--workspace", "w"]), req)
            .expect_err("value looks like an option");
        assert!(e.contains("--snapshot requires a value"), "{e}");
        let e = parse_options(
            &args(&["--snapshot", "s", "--workspace", "w", "extra"]),
            req,
        )
        .expect_err("positional");
        assert!(e.contains("unexpected argument extra"), "{e}");
        let e = parse_options(&args(&[]), &["--workspace"]).expect_err("nothing given");
        assert!(e.contains("missing required option --workspace"), "{e}");
    }
}
