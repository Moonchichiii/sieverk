use std::env;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use chrono::NaiveDate;

use sieverk::accounts::{parse_accounts, AccountData};
use sieverk::chart::MasterdataError;
use sieverk::decode_sie_bytes;
use sieverk::engine::run_engine;
use sieverk::export_context::VerifiedExportContext;
use sieverk::metadata::{parse_metadata, Metadata};
use sieverk::money::Ore;
use sieverk::ruleset::load_masterdata;
use sieverk::sie_writer::{write_sie4i, ExportMetadata, ProgramId};
use sieverk::snapshot::{parse_snapshot, Snapshot};
use sieverk::validator::{validate, Report, Severity};
use sieverk::vouchers::{parse_vouchers, VoucherData};
use sieverk::workspace::{IngestOutcome, Workspace, WorkspaceError};

struct Parsed {
    meta: Metadata,
    acc: AccountData,
    vou: VoucherData,
    report: Report,
    tag_lines: usize,
}

fn load_and_parse(path: &str) -> Result<Parsed, String> {
    let roles = [Role::new(Path::new(path), "<sie-file>")];
    let bytes = fs::read(path).map_err(|e| {
        format!(
            "could not read the SIE file: {}",
            detail(&e.to_string(), &roles)
        )
    })?;
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
        Some("export-sie") => {
            return match parse_export_options(&argv[1..]) {
                Ok(opts) => finish(export_sie(&opts)),
                Err(e) => {
                    eprintln!("{e}");
                    eprintln!("{EXPORT_USAGE}");
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
            eprintln!("{EXPORT_USAGE}");
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
        "validate-sie" => run_validate(&parsed, &path),
        _ => {
            print!("{}", render_inspect(&parsed));
            ExitCode::SUCCESS
        }
    }
}

/// A parser warning without the raw SIE line it quotes: the parsers append
/// `: <line>` to every message that carries one, and none of their own message
/// texts contains `": "` — so only the text before it is ever shown.
fn parser_note(warning: &str) -> &str {
    warning
        .split_once(": ")
        .map_or(warning, |(message, _line)| message)
}

/// `inspect-sie` report. HY-SIE-01: `#FNAMN` (the legal name) and `#ORGNR`
/// (a personnummer for an enskild firma) are never printed — only whether
/// they are present — and the user-supplied path is not echoed.
fn render_inspect(p: &Parsed) -> String {
    fn presence(v: &Option<String>) -> &'static str {
        if v.is_some() {
            REDACTED
        } else {
            "—"
        }
    }

    let mut out = String::new();
    out.push_str("sieverk — SIE inspection\n");
    out.push_str(&format!(
        "SIE type:    {}\n",
        p.meta.sie_type.map_or("—".to_string(), |t| t.to_string())
    ));
    out.push_str(&format!(
        "Company:     {}\n",
        presence(&p.meta.company_name)
    ));
    out.push_str(&format!("Org number:  {}\n", presence(&p.meta.org_number)));
    // Raw #RAR dates and #VALUTA are file text: shown only in their exact shape.
    let date = |d: &str| {
        if is_plain_date(d) {
            d.to_string()
        } else {
            REDACTED.to_string()
        }
    };
    for fy in &p.meta.fiscal_years {
        out.push_str(&format!(
            "Fiscal year: {} – {} (index {})\n",
            date(&fy.start),
            date(&fy.end),
            fy.index
        ));
    }
    // #VALUTA is file text: only an explicitly approved currency code is shown
    // (a three-letter uppercase word such as `EVA` could be a name).
    let currency = match p.meta.currency.as_deref() {
        None => "—",
        Some(c) if TRUSTED_CURRENCIES.contains(&c) => c,
        Some(_) => REDACTED,
    };
    out.push_str(&format!("Currency:    {currency}\n"));
    out.push_str(&format!("Accounts:    {}\n", p.acc.accounts.len()));
    out.push_str(&format!("Balances:    {} rows\n", p.acc.balances.len()));
    out.push_str(&format!("Vouchers:    {}\n", p.vou.vouchers.len()));
    out.push_str(&format!(
        "Ledger rows: {}\n",
        p.vou.vouchers.iter().map(|v| v.rows.len()).sum::<usize>()
    ));
    out.push_str(&format!("Tag lines:   {}\n", p.tag_lines));

    let parser_warnings = p.meta.warnings.len() + p.acc.warnings.len() + p.vou.warnings.len();
    out.push_str(&format!("Parser notes: {parser_warnings}\n"));
    out.push_str(&format!(
        "Findings:    {} errors, {} warnings\n",
        p.report.error_count(),
        p.report.warning_count()
    ));
    out.push_str(&format!("Status:      {}\n", p.report.status()));
    out
}

/// `validate-sie` report. HY-SIE-01: finding messages and parser notes are
/// built from file content (voucher labels, dates, accounts, raw lines), so
/// they go through the `Data` redaction; a parser note never carries the raw
/// SIE line it was raised on. Codes, counts and the exit code are unchanged.
fn run_validate(p: &Parsed, path: &str) -> ExitCode {
    let roles = [Role::new(Path::new(path), "<sie-file>")];
    println!("sieverk — SIE validation");

    for f in &p.report.findings {
        let sev = match f.severity {
            Severity::Error => "ERROR  ",
            Severity::Warning => "WARNING",
        };
        println!(
            "{sev} {}: {}",
            f.code.as_str(),
            data_detail(&f.message, &roles)
        );
    }
    for w in p
        .meta
        .warnings
        .iter()
        .chain(p.acc.warnings.iter())
        .chain(p.vou.warnings.iter())
    {
        println!("PARSER  {}", data_detail(parser_note(w), &roles));
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
    let roles = [Role::new(Path::new(path), "<snapshot>")];
    let bytes = fs::read(path).map_err(|e| {
        format!(
            "could not read the snapshot file: {}",
            detail(&e.to_string(), &roles)
        )
    })?;
    let snap = parse_snapshot(&bytes)
        .map_err(|e| format!("snapshot rejected: {}", data_detail(&e.to_string(), &roles)))?;
    Ok(render_snapshot(&snap))
}

/// HY-SIE-01: neither the display name (often a person's name for an enskild
/// firma) nor any identifier is printed, not even partially.
fn render_snapshot(s: &Snapshot) -> String {
    let receipts_total: Ore = s.receipts.iter().map(|r| r.total_amount).sum();
    let receipts_vat: Ore = s.receipts.iter().map(|r| r.vat_amount).sum();
    let income_inc: Ore = s.income_entries.iter().map(|e| e.amount_inc_vat).sum();
    let org = if s.entity.org_number.is_some() {
        REDACTED
    } else {
        "—"
    };

    let mut out = String::new();
    out.push_str("sieverk — snapshot inspection\n");
    out.push_str(&format!("Schema:      {}\n", s.schema_version));
    out.push_str(&format!("Entity:      {REDACTED} (org.nr {org})\n"));
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
            eprintln!("masterdata invalid: {}", masterdata_error(&e, root));
            return ExitCode::FAILURE;
        }
    };
    // HY-SIE-01: masterdata values are shown only in their exact technical
    // shape; free text (profile scope, workbook name) is never printed.
    println!("sieverk — masterdata inspection");
    println!(
        "Chart:         {} {} ({})",
        shaped(&md.chart.chart_id, is_chart_id),
        shaped(&md.chart.version, is_version),
        shaped(&md.chart.review_status, is_review_status)
    );
    println!("Profile:       {FREE_TEXT_WITHHELD}");
    println!(
        "Workbook:      {FREE_TEXT_WITHHELD} sha256 {}",
        shaped(&md.chart.header.workbook_sha256, is_sha256_hex)
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
        shaped(&md.ruleset.ruleset_version, is_version),
        shaped(&md.ruleset.taxonomy_version, is_version)
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
            // HY-SIE-01: an unexpected value is never echoed — it may be a path.
            return Err(if is_option_name(arg) {
                format!("unknown option {arg}")
            } else if arg.starts_with('-') {
                "unknown option".to_string()
            } else {
                "unexpected positional argument".to_string()
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
    let roles = [
        Role::new(snapshot, "<snapshot>"),
        Role::new(workspace, "<workspace>"),
    ];
    let raw = fs::read(snapshot).map_err(|e| {
        format!(
            "could not read the snapshot file: {}",
            detail(&e.to_string(), &roles)
        )
    })?;
    let mut ws = Workspace::create(workspace).map_err(|e| workspace_error(&e, &roles))?;
    let outcome = ws
        .ingest(&raw)
        .map_err(|e| format!("ingest failed: {}", workspace_error(&e, &roles)))?;
    ws.close().map_err(|e| workspace_error(&e, &roles))?;
    match outcome {
        IngestOutcome::Ingested {
            snapshot_sha256,
            properties,
            receipts,
            income_entries,
            audit_items,
        } => Ok(format!(
            "snapshot_sha256={snapshot_sha256}\nproperties={properties}\n\
             receipts={receipts}\nincome_entries={income_entries}\naudit_items={audit_items}\n"
        )),
        IngestOutcome::AlreadyIngested { snapshot_sha256 } => Err(format!(
            "workspace already holds snapshot {snapshot_sha256} (unexpected after create)"
        )),
    }
}

/// `inspect-workspace`: open (never create) and report through the typed
/// DuckDB readback only — no snapshot JSON, no row dumps, no org number.
fn inspect_workspace(workspace: &Path) -> Result<String, String> {
    let roles = [Role::new(workspace, "<workspace>")];
    let err = |e: WorkspaceError| workspace_error(&e, &roles);
    let ws = Workspace::open(workspace).map_err(err)?;
    let schema = ws.schema_version().map_err(err)?;
    let meta = ws.read_meta().map_err(err)?;
    // Read (and thereby validate) entity_context, but print nothing from it:
    // display_name can be a person's name for an enskild firma.
    ws.read_entity().map_err(err)?;
    let properties = ws.read_properties().map_err(err)?.len();
    let receipts = ws.read_receipts().map_err(err)?.len();
    let income_entries = ws.read_income_entries().map_err(err)?.len();
    let audit_items = ws.read_audit_chain().map_err(err)?.len();
    let fingerprint = ws.logical_fingerprint().map_err(err)?;
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

const EXPORT_USAGE: &str =
    "usage: sieverk export-sie --snapshot <snapshot.json> --root <generated-dir> \
--workspace <new.duckdb> --out <file.SI> --generated-on YYYY-MM-DD";

/// The complete, explicit input of one `export-sie` run. Identity is not an
/// input any more (SK-SIE-BRIDGE-04): `#FNAMN`, `#ORGNR` and `#RAR 0` come only
/// from the snapshot's verified accounting identity and fiscal period.
#[derive(Debug)]
struct ExportOptions {
    snapshot: PathBuf,
    root: PathBuf,
    workspace: PathBuf,
    out: PathBuf,
    generated_on: NaiveDate,
}

/// Strict: each option exactly once, nothing unknown (so `--company-name` and
/// `--org-number` are refused as unknown options).
fn parse_export_options(args: &[String]) -> Result<ExportOptions, String> {
    let opts = parse_options(
        args,
        &[
            "--snapshot",
            "--root",
            "--workspace",
            "--out",
            "--generated-on",
        ],
    )?;
    let generated_on = NaiveDate::parse_from_str(&opts[4], "%Y-%m-%d")
        .map_err(|_| "--generated-on is not YYYY-MM-DD".to_string())?;
    Ok(ExportOptions {
        snapshot: PathBuf::from(&opts[0]),
        root: PathBuf::from(&opts[1]),
        workspace: PathBuf::from(&opts[2]),
        out: PathBuf::from(&opts[3]),
        generated_on,
    })
}

/// `export-sie`: the canonical generation path — snapshot read once → new
/// workspace → ingest_snapshot → masterdata → engine (schema 3) →
/// VerifiedExportContext::bind → writer → bytes on disk (`create_new`).
/// stdout carries evidence only: never the identifier or the legal name.
fn export_sie(opts: &ExportOptions) -> Result<String, String> {
    let roles = [
        Role::new(&opts.snapshot, "<snapshot>"),
        Role::new(&opts.root, "<masterdata-root>"),
        Role::new(&opts.workspace, "<workspace>"),
        Role::new(&opts.out, "<out>"),
    ];
    let raw = fs::read(&opts.snapshot).map_err(|e| {
        format!(
            "could not read the snapshot file: {}",
            detail(&e.to_string(), &roles)
        )
    })?;
    let masterdata = load_masterdata(&opts.root)
        .map_err(|e| format!("masterdata invalid: {}", masterdata_error(&e, &opts.root)))?;
    let mut ws = Workspace::create(&opts.workspace).map_err(|e| workspace_error(&e, &roles))?;
    let ingested = ws
        .ingest_snapshot(&raw)
        .map_err(|e| format!("ingest failed: {}", workspace_error(&e, &roles)))?;
    let evidence = run_engine(&mut ws, &masterdata).map_err(|e| {
        data_sanitized(&e.to_string(), &roles).unwrap_or_else(|| withheld("engine run"))
    })?;
    let context = VerifiedExportContext::bind(&ingested, &ws).map_err(|e| {
        data_sanitized(&e.to_string(), &roles).unwrap_or_else(|| withheld("export context"))
    })?;
    let meta = ExportMetadata {
        generated_on: opts.generated_on,
        program: ProgramId {
            name: "SIEverk".to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
    };
    let export = write_sie4i(&ws, &masterdata, &context, &meta).map_err(|e| {
        data_sanitized(&e.to_string(), &roles).unwrap_or_else(|| withheld("SIE writer"))
    })?;
    let mut file = fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&opts.out)
        .map_err(|e| {
            format!(
                "could not create the output file: {}",
                detail(&e.to_string(), &roles)
            )
        })?;
    file.write_all(&export.bytes).map_err(|e| {
        format!(
            "could not write the output file: {}",
            detail(&e.to_string(), &roles)
        )
    })?;
    ws.close().map_err(|e| workspace_error(&e, &roles))?;
    let period = context.fiscal_period();
    Ok(format!(
        "snapshot_sha256={}\ndecision_sha256={}\nentity_type={}\nidentifier_kind={}\n\
         fiscal_period={}..{}\nvouchers={}\naccounts={}\nbytes={}\nsi_sha256={}\n",
        evidence.run.provenance.snapshot_sha256,
        evidence.run.decision_sha256,
        context.entity_type().as_str(),
        context.identifier_kind().as_str(),
        period.start,
        period.end,
        export.vouchers,
        export.accounts,
        export.bytes.len(),
        export.sha256
    ))
}

// -- HY-SIE-01: nothing personal reaches stdout or stderr ---------------------
//
// Rules: identities and names print as `[REDACTED]`; a user-supplied path is
// never echoed; error text from a library is first scrubbed of every known
// form of the user's paths and then CHECKED — anything that still looks like a
// path, an identifier or a piece of the user's paths is withheld entirely
// (fail-closed: less useful, never leaky).

/// What every identity or name prints as.
const REDACTED: &str = "[REDACTED]";
/// What a masterdata free-text field prints as.
const FREE_TEXT_WITHHELD: &str = "(free text withheld)";

/// `value` if it has exactly the expected technical shape, else `[REDACTED]`.
fn shaped(value: &str, shape: fn(&str) -> bool) -> &str {
    if shape(value) {
        value
    } else {
        REDACTED
    }
}

/// Only a trusted chart id (`SYNTHETIC_K1`) — an uppercase word could be a name.
fn is_chart_id(s: &str) -> bool {
    TRUSTED_CHART_IDS.contains(&s)
}

/// `2026.1`, `1.0`: digits and dots.
fn is_version(s: &str) -> bool {
    (1..=16).contains(&s.len())
        && s.starts_with(|c: char| c.is_ascii_digit())
        && s.bytes().all(|c| c.is_ascii_digit() || c == b'.')
}

/// The two review states the masterdata contract defines.
fn is_review_status(s: &str) -> bool {
    s == "draft" || s == "approved"
}

/// A sha256 as 64 lowercase hex digits.
fn is_sha256_hex(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}
/// What an error detail becomes when it cannot be shown safely.
const WITHHELD: &str = "(details withheld)";

/// Masterdata file names that may appear in masterdata errors: exactly the
/// tracked synthetic files (`fixtures/masterdata/synthetic/generated`). Any
/// other name is replaced by a fixed role label — prefix or pattern alone is
/// never trusted to prove a file name carries no personal data.
const TRUSTED_MASTERDATA_FILES: [&str; 5] = [
    "chart-synthetic-k1-2026.1.json",
    "sru-2026.json",
    "vat-rules-2026.1.json",
    "counter-accounts-2026.1.json",
    "ruleset-2026.1.json",
];

/// A user-supplied path and the fixed label that stands in for it.
#[derive(Clone, Copy)]
struct Role<'a> {
    path: &'a Path,
    label: &'static str,
}

impl<'a> Role<'a> {
    fn new(path: &'a Path, label: &'static str) -> Self {
        Self { path, label }
    }
}

/// `"<what> failed (details withheld)"` — the fail-closed form of an error.
fn withheld(what: &str) -> String {
    format!("{what} failed {WITHHELD}")
}

/// Library text (DuckDB, the OS): paths scrubbed, quoted values redacted, then
/// checked — or `WITHHELD` if anything unsafe remains.
fn detail(text: &str, roles: &[Role]) -> String {
    sanitized(text, roles).unwrap_or_else(|| WITHHELD.to_string())
}

/// Library text: `None` if anything unsafe remains after scrubbing.
fn sanitized(text: &str, roles: &[Role]) -> Option<String> {
    let scrubbed = redact_free_text(&scrub_paths(text, roles));
    is_safe_detail(&scrubbed, roles).then_some(scrubbed)
}

/// Text built from data (snapshot, masterdata, SIE content, engine, writer,
/// workspace contents): paths scrubbed, then rendered through the fixed
/// message templates, then checked.
fn data_detail(text: &str, roles: &[Role]) -> String {
    data_sanitized(text, roles).unwrap_or_else(|| WITHHELD.to_string())
}

/// Data text: `None` if it matches no fixed message template, or if anything
/// unsafe remains after rendering.
fn data_sanitized(text: &str, roles: &[Role]) -> Option<String> {
    let rendered = render_data(&scrub_paths(text, roles))?;
    is_safe_detail(&rendered, roles).then_some(rendered)
}

// -- HY-SIE-01 REV4: data text is shown only through fixed message templates --
//
// A message built from data is shown only if it matches one of SIEverk's own
// message templates (`DATA_TEMPLATES`, taken from the library's message
// literals). The template's fixed text is shown as written; every place where
// the library inserts a value is a typed hole, and a hole shows its content
// only in a strict technical shape — otherwise `[REDACTED]`. No word list ever
// approves a value: an id, a voucher series or a name is redacted even when it
// is also an English word (`may`, `mine`, `case`). Text that matches no
// template is withheld entirely.
//
// Holes: `{v}` value — a number, an amount, a version, a date, a sha256, one
// of our `<role>` labels, a known SIE tag, `receipt:N`/`income:N`, `true`/
// `false`; a quoted value keeps its quotes and shows its inside only in such a
// shape. `{l}` voucher label — `is_voucher_label`. `{c}` code value — a field
// path of the serde field names, an exact code token or a `CODE_FORMS` form.
// `{m}` nested message — rendered through the templates again. `{x}` library
// text (OS, DuckDB) — the library rules of `redact_free_text`. `{e}` a serde
// expectation built from code (`expected …`). `{{` and `}}` are literal braces.

/// Every message the library builds from data, as fixed text with typed holes.
const DATA_TEMPLATES: [&str; 255] = [
    // validator findings and parser notes (validate-sie)
    "{l} sums to {v}",
    "{l} appears more than once",
    "{l} is dated {v} outside fiscal year {v} – {v}",
    "{l} has a malformed date '{v}'",
    "{l} uses account {v} which has no #KONTO row",
    "#VER {l} was never closed before the next #VER",
    "#VER {l} block never closes before end of file",
    "#VER header is malformed",
    "unexpected '{{' with no #VER header before it",
    "unexpected '}}' with no open voucher block",
    "#TRANS outside a voucher block",
    "#TRANS is missing its account number",
    "nested '{{' inside a #TRANS object list",
    "#TRANS object list never closes",
    "#TRANS amount '{v}' is not a valid amount",
    "#TRANS is missing its amount",
    "#KONTO row is missing its account number",
    "{v} row is malformed",
    "#SIETYP has a non-numeric value",
    "#SIETYP is missing its value",
    // snapshot
    "{c}: {m}",
    "invalid snapshot JSON: {m}",
    "expected an amount string with exactly two decimals, got {v}",
    "amount out of range: {v}",
    "unsupported schema {v} (supported: {v})",
    "expected a four-digit year, got {v}",
    "receipt:{v} refers to property {v} which is not in properties[]",
    "amount overflow while checking net",
    "receipt:{v} net {v} != total {v} - vat {v} - rounding {v} (= {v})",
    "amount overflow while checking sum",
    "income:{v} inc {v} != ex {v} + vat {v} (= {v})",
    "only allowed in schema 1.2",
    "not allowed in schema 1.2 (the accounting identity is entity.accounting_identity)",
    "required key in schema 1.2",
    "must be a string",
    "unknown entity type",
    "unknown identifier kind",
    "identifier kind does not match the entity type",
    "must not be empty",
    "must not contain control characters",
    "not a valid identifier for its kind (expected NNNNNN-NNNN)",
    "expected YYYY-MM-DD",
    "not a calendar date",
    "start is after end",
    "period length overflow",
    "longer than 18 calendar months",
    // masterdata: chart, SRU, VAT, counter accounts, ruleset, loading
    "invalid chart JSON: {m}",
    "invalid SRU JSON: {m}",
    "invalid VAT JSON: {m}",
    "invalid counter-account JSON: {m}",
    "invalid ruleset JSON: {m}",
    "expected sieverk-chart/1, got {v}",
    "expected sieverk-sru/1, got {v}",
    "expected sieverk-vat/1, got {v}",
    "expected sieverk-counter/1, got {v}",
    "expected sieverk-ruleset/1, got {v}",
    "must be draft or approved, got {v}",
    "must be draft or approved",
    "does not match review_status",
    "chart has no accounts",
    "account number must be exactly four digits",
    "duplicate account number",
    "unknown kind {v}",
    "kind {v} contradicts first digit (expected {v})",
    "class 2 must be skuld or eget_kapital",
    "class 8 must be intäkt or kostnad",
    "name is empty",
    "source_id {v} is not declared in sources",
    "source_version is mandatory (v1.2 §B)",
    "source_decision needs reason, decided_by and decided_at",
    "must_include requires must_include_origin ∈ {v}",
    "Godkänd utan granskad_av i rollen accounting_reviewer",
    "account must be four digits",
    "sru code must be four digits",
    "valid_to before valid_from",
    "account is not in the chart",
    "overlapping validity for the same account",
    "duplicate vat rule id",
    "direction must be ingående or utgående",
    "rates must be within {{25,12,6,0}}",
    "deductibility must be full, ingen or manuell",
    "source must be receipt or income",
    "source must be receipt or income, got {v}",
    "unknown automation",
    "account missing but automation is not Manual",
    "question required when automation is not Automatic",
    "does not match taxonomy_version",
    "duplicate case_id",
    "draft ruleset contains an Automatic case (contract violation)",
    "category {v} is not in the taxonomy",
    "receipt case without category",
    "income_type {v} is not in the export",
    "income case without income_type",
    "more than one default case for this subject",
    "Automatic requires account and vat_rule",
    "Conditional requires condition and question",
    "Manual requires question",
    "does not match the chart's id/version",
    "differs from the chart's review_status",
    "vat_account is not in the chart",
    "payment_method is not in the export",
    "vat_rule {v} is not defined",
    "sru_override must be four digits",
    "account {v} is not in the chart",
    "account {v} is inactive",
    "account {v} is not engine_proposable",
    "cannot read root: {x}",
    "cannot read: {x}",
    "no chart-*.json in root",
    "no sru-*.json in root",
    "no vat-rules-*.json in root",
    "no counter-accounts-*.json in root",
    "no ruleset-*.json in root",
    "{v} files match chart-*.json; expected exactly one",
    "{v} files match sru-*.json; expected exactly one",
    "{v} files match vat-rules-*.json; expected exactly one",
    "{v} files match counter-accounts-*.json; expected exactly one",
    "{v} files match ruleset-*.json; expected exactly one",
    // engine
    "workspace: {m}",
    "case {v}: source_key {v} is not the canonical {v}",
    "case {v}: property {v} is not in the workspace",
    "case {v}: money invariant failed: {m}",
    "case {v}: {v} counter rules match payment_method {v}; refusing to choose",
    "case {v}: account {v} may not be proposed: {m}",
    "case {v}: masterdata: {m}",
    "workspace taxonomy {v} does not match ruleset taxonomy {v}",
    "net + vat + rounding: i64 overflow",
    "ex_vat + vat: i64 overflow",
    "net {v} + vat {v} + rounding {v} != total {v} (öre)",
    "ex_vat {v} + vat {v} != inc_vat {v} (öre)",
    "unknown automation {v}",
    "not in the chart",
    "not engine_proposable",
    "business_groups do not intersect entity.operations",
    "receipt has no payment_method",
    "no counter rule for payment_method {v}",
    "receipt has no subject",
    "income has no subject",
    "no default rule case for receipt subject {v}",
    "no default rule case for income subject {v}",
    "default rule case {v} for receipt subject {v} has no account",
    "default rule case {v} for income subject {v} has no account",
    "counter rule for payment_method {v} has no account",
    "receipt row carries legacy entry_type income",
    "income_type timber_sale is unspecified (legacy)",
    "receipt has no image",
    "income entry has no documents",
    "net × rate overflows i64",
    "net × rate + 50 overflows i64",
    "vat rule {v} requires manual review",
    "vat rule {v} has manual deductibility",
    "vat rule {v} allows {v} rates",
    "rule case {v} has no vat rule",
    "vat rule {v} does not resolve",
    "source vat 0 but expected {v} öre at {v} %",
    "expected vat is zero at this net amount; a zero input-VAT line is not representable",
    "source vat {v} does not equal expected {v} öre at {v} %",
    "net + vat overflows i64",
    "net {v} + vat {v} != total {v} (öre)",
    "schema {v} workspace without a run row cannot receive a run",
    // export context and SIE writer
    "no verified engine run for this workspace",
    "snapshot provenance mismatch (workspace): the snapshot is not the one this workspace was built from",
    "snapshot provenance mismatch (engine_run): the snapshot is not the one this workspace was built from",
    "the snapshot carries no accounting identity",
    "the snapshot carries no fiscal period",
    "workspace schema {v} holds no accounting decisions; nothing to export",
    "workspace holds no engine run; nothing to export",
    "engine run did not verify: {m}",
    "persisted evidence is inconsistent: {m}",
    "masterdata does not match the run's provenance: {c} persisted {v}, loaded {v}",
    "account {v} is not in the run's chart",
    "account {v} has no usable name",
    "case {v}: voucher does not balance (signed sum {v} öre)",
    "case {v}: amount arithmetic overflows i64",
    "{c}: control character at index {v}",
    "{c}: literal backslash is not representable",
    "{c}: {v} cannot be encoded as CP437",
    "invalid export metadata: {m}",
    "case {v}: {m}",
    "io: {x}",
    "the export context was not bound to this workspace's verified run",
    "SIE export is supported only for sole traders (entity type {c})",
    "case {v}: voucher date is outside the fiscal period",
    "Manual decision carries lines",
    "Automatic decision without lines",
    "program name and version are required",
    "business identifier in the export context is not NNNNNN-NNNN",
    // workspace
    "workspace {v} already exists — refusing to reuse or overwrite it",
    "workspace {v} does not exist",
    "workspace schema {v} is not supported (this build expects {v})",
    "workspace is corrupt: {m}",
    "workspace has no ingested snapshot",
    "workspace belongs to another snapshot ({v}); refusing {v}",
    "snapshot rejected: {m}",
    "{c}: {v} is not a YYYY-MM-DD date",
    "duckdb: {x}",
    "workspace holds no engine run",
    "workspace already holds an engine run ({v}); refusing a second run",
    "workspace_meta is unreadable ({x})",
    "workspace_meta must have exactly one row, found {v}",
    "snapshot_meta must have 0 or 1 rows, found {v}",
    "{c} is not a schema v1 table",
    "schema {v} workspace without a snapshot cannot be ingested into",
    "run-schema workspace lacks table {c}",
    "engine run present but snapshot_meta has no row",
    "engine_run_meta.run_seq must be 0, found {v}",
    "engine run names snapshot {v} but the workspace holds {v}",
    "engine_run_meta must have 0 or 1 rows, found {v} (more than one run row)",
    "engine_run_meta must have 0 or 1 rows, found {v}",
    "snapshot_meta must have exactly 1 row for a run, found {v}",
    "schema 1 workspace already contains engine table {c}",
    "provenance snapshot {v} is not this workspace's snapshot {v}",
    "stored ruleset_status {v} is not draft or approved",
    "stored decision_sha256 is not 64 lowercase hex characters",
    "decision digest mismatch: stored {v} recomputed {v}",
    "snapshot is not valid JSON: {m}",
    "snapshot must be a JSON object",
    "cannot serialise canonical JSON: {m}",
    "canonical string encoding failed: {m}",
    "decision_lines must be empty in a schema 2 run, found {v}",
    "schema {v} is not a run schema",
    "expected {v} cases (receipts {v} + incomes {v}), found {v}",
    "case_seq at position {v} is {v} (must be dense from 0)",
    "duplicate source_key {v}",
    "case {v}: status {v} is not in the lifecycle",
    "case {v}: Automatic under draft provenance",
    "case {v}: vat_rule_id without rule_case_id",
    "case {v}: partial counter reference",
    "case {v}: counter_source {c} != source {c}",
    "case {v}: income counter reference is not allowed in Slice 2",
    "case {v}: counter_key {v} != payment_method {v}",
    "case {v}: counter reference without payment_method",
    "case {v} does not mirror receipt {v} (selector_position {v})",
    "case {v} does not mirror income {v} (snapshot_position {v})",
    "finding {v}: orphan case_seq {v}",
    "finding {v}: case_seq {v} out of order",
    "finding {v}: finding_no {v} but expected {v} (dense per case)",
    "finding {v}: unknown code {v}",
    "finding {v}: code {c} is not allowed in a schema {v} run",
    "finding {v}: code {c} must have severity {c}, found {c}",
    "finding {v}: duplicate code {c} in one case",
    "finding {v}: code {c} out of fixed rank order",
    "case {v}: blocking finding but status {c}",
    "line {v}: orphan case_seq {v}",
    "line {v}: case_seq {v} out of order",
    "line {v}: line_no {v} but expected {v} (dense per case)",
    "line {v}: account {v} is not four digits",
    "line {v}: unknown role {v}",
    "line {v}: negative side",
    "line {v}: exactly one side must be positive",
    "line {v}: debit sum overflow",
    "line {v}: credit sum overflow",
    "case {v}: Manual case carries lines",
    "case {v}: lines do not balance (debit {v} credit {v})",
    "case {v}: Automatic case without lines",
];

/// serde / serde_json error texts (inside `invalid … JSON: {m}`).
const SERDE_TEMPLATES: [&str; 45] = [
    "{m} at line {v} column {v}",
    "EOF while parsing a list",
    "EOF while parsing an object",
    "EOF while parsing a string",
    "EOF while parsing a value",
    "expected `:`",
    "expected `,` or `]`",
    "expected `,` or `}}`",
    "expected ident",
    "expected value",
    "invalid escape",
    "invalid number",
    "number out of range",
    "invalid unicode code point",
    "control character (\\u0000-\\u001F) found while parsing a string",
    "key must be a string",
    "lone leading surrogate in hex escape",
    "trailing comma",
    "trailing characters",
    "unexpected end of hex escape",
    "recursion limit exceeded",
    "missing field `{c}`",
    "duplicate field `{c}`",
    "unknown field `{v}`, expected {e}",
    "unknown field `{v}`, there are no fields",
    "unknown variant `{v}`, expected {e}",
    "unknown variant `{v}`, there are no variants",
    "invalid type: {m}, expected {e}",
    "invalid value: {m}, expected {e}",
    "invalid length {v}, expected {e}",
    "string {v}",
    "integer `{v}`",
    "floating point `{v}`",
    "boolean `{v}`",
    "character `{v}`",
    "null",
    "unit value",
    "Option value",
    "byte array",
    "newtype struct",
    "sequence",
    "map",
    "unit variant",
    "newtype variant",
    "struct variant",
];

/// What a `{c}` hole may hold besides a field path or a code token: the field
/// labels the SIE writer reports (`#KONTO 1930`, `#VER case 3`).
const CODE_FORMS: [&str; 5] = [
    "#KONTO {v}",
    "#PROGRAM name",
    "#PROGRAM version",
    "#FNAMN",
    "#VER case {v}",
];

/// Masterdata element references. Ids taken from masterdata content (a case
/// id, a VAT rule id, a counter key) are `{v}` holes, so they are redacted;
/// account numbers and indexes stay.
const ELEMENT_TEMPLATES: [&str; 10] = [
    "cases/{v}",
    "rules/{v}",
    "vat/{v}",
    "chart/{v}",
    "sru/{v}",
    "rows/{v}",
    "rows/{c}/{v}",
    "counter/{c}/{v}",
    "accounts[{v}]/{v}",
    "rows[{v}]/{v}",
];

/// Code tokens a `{c}` hole may show: workspace table names, finding codes,
/// severities, statuses, sources, roles, entity types and provenance fields —
/// constants of SIEverk's own code, never user text. Sorted (binary search).
const CODE_TOKENS: [&str; 45] = [
    "Automatic",
    "Conditional",
    "LEGACY_INCOME_RECEIPT",
    "MISSING_EVIDENCE",
    "Manual",
    "UNMAPPED_CATEGORY",
    "UNRESOLVED_COUNTER_ACCOUNT",
    "UNRESOLVED_VAT",
    "UNSPECIFIED_TIMBER_SALE",
    "VAT_MISMATCH",
    "accounting_cases",
    "approved",
    "audit_chain",
    "blocking",
    "chart_id",
    "chart_version",
    "counter",
    "decision_lines",
    "draft",
    "engine_run_meta",
    "entity_context",
    "entity_operations",
    "expense",
    "findings",
    "generator_version",
    "income",
    "income_entries",
    "info",
    "input_vat",
    "limited_company",
    "limited_partnership",
    "other_legal_person",
    "output_vat",
    "properties",
    "receipt",
    "receipts",
    "rounding",
    "ruleset_status",
    "ruleset_version",
    "snapshot_meta",
    "sole_trader",
    "taxonomy_version",
    "trading_partnership",
    "warning",
    "workspace_meta",
];

/// The field names of SIEverk's serde structs (snapshot, masterdata), the
/// segments a `{c}` field path may consist of (`receipts[0].net_amount`,
/// `_header.review_status`). Sorted (binary search).
const FIELD_NAMES: [&str; 146] = [
    "_header",
    "account",
    "accounting_identity",
    "accounting_profile",
    "accounts",
    "active",
    "all_properties_locked",
    "amount_ex_vat",
    "amount_inc_vat",
    "app",
    "area",
    "audit_chain",
    "automation",
    "bookkeeping_method",
    "business_groups",
    "business_identifier",
    "buyer_name",
    "case_id",
    "cases",
    "categories",
    "category",
    "category_context",
    "chart",
    "chart_id",
    "closing_account",
    "condition",
    "confirmed_at",
    "counter",
    "counter_rule",
    "county",
    "date",
    "decided_at",
    "decided_by",
    "declaration_year",
    "deductibility",
    "default_payment_method",
    "description",
    "direction",
    "display_name",
    "document",
    "document_count",
    "downgraded_from_automatic",
    "end",
    "engine_proposable",
    "entity",
    "entity_type",
    "entry_type",
    "environment",
    "event",
    "fiscal_period",
    "form",
    "framework",
    "generated_at",
    "generator_version",
    "has_image",
    "header",
    "id",
    "identifier_kind",
    "income_entries",
    "income_type",
    "income_types",
    "income_year",
    "investment_risk",
    "invoice_number",
    "is_default",
    "key",
    "kind",
    "legal_name",
    "lock",
    "locked_at",
    "manual_review",
    "message",
    "must_include",
    "must_include_origin",
    "name",
    "net_amount",
    "note",
    "number",
    "operation",
    "ordinal_number",
    "org_number",
    "owner_id",
    "path",
    "payment_date",
    "payment_method",
    "payment_methods",
    "profile_scope",
    "properties",
    "property_id",
    "publisher",
    "question",
    "rates",
    "reason",
    "receipts",
    "received_date",
    "reference",
    "requires_business_share",
    "retrieved",
    "return_sie_allowed",
    "review",
    "review_status",
    "reviewed_at",
    "reviewed_by",
    "reviewed_role",
    "rights",
    "roles",
    "rounding_amount",
    "rows",
    "rules",
    "ruleset",
    "ruleset_version",
    "schema",
    "schema_version",
    "sensitive",
    "sha256",
    "sie_series",
    "slug",
    "source",
    "source_decision",
    "source_id",
    "source_key",
    "source_version",
    "sources",
    "sru",
    "sru_override",
    "start",
    "status",
    "tax_year",
    "taxonomy",
    "taxonomy_version",
    "total_amount",
    "user_selectable",
    "valid_from",
    "valid_to",
    "vat",
    "vat_account",
    "vat_amount",
    "vat_check",
    "vat_registered",
    "vat_rule",
    "vendor",
    "verified",
    "verified_at",
    "version",
    "workbook",
    "workbook_sha256",
];

/// Type descriptions serde reports after `expected` (always built from code).
const SERDE_EXPECTED: [&str; 18] = [
    "a boolean",
    "a borrowed string",
    "a map",
    "a sequence",
    "a string",
    "any valid JSON value",
    "f32",
    "f64",
    "i16",
    "i32",
    "i64",
    "i8",
    "option",
    "u16",
    "u32",
    "u64",
    "u8",
    "unit",
];

/// The fixed labels that stand in for user-supplied paths.
const ROLE_LABELS: [&str; 5] = [
    "<masterdata-root>",
    "<out>",
    "<sie-file>",
    "<snapshot>",
    "<workspace>",
];

/// SIE record tags that may appear in parser notes — never any `#WORD`.
const SIE_TAGS: [&str; 31] = [
    "#ADRESS", "#BKOD", "#BTRANS", "#DIM", "#ENHET", "#FLAGGA", "#FNAMN", "#FNR", "#FORMAT",
    "#FTYP", "#GEN", "#IB", "#KONTO", "#KPTYP", "#KSUMMA", "#KTYP", "#OBJEKT", "#OIB", "#OMFATTN",
    "#ORGNR", "#OUB", "#PBUDGET", "#PROGRAM", "#PROSA", "#PSALDO", "#RAR", "#RES", "#RTRANS",
    "#SIETYP", "#TRANS", "#VER",
];

/// Chart ids `inspect-masterdata` may show (the tracked synthetic chart).
const TRUSTED_CHART_IDS: [&str; 1] = ["SYNTHETIC_K1"];

/// Currency codes `inspect-sie` may show for `#VALUTA`; any other value —
/// even three uppercase letters — prints as `[REDACTED]`.
const TRUSTED_CURRENCIES: [&str; 6] = ["SEK", "EUR", "NOK", "DKK", "USD", "GBP"];

/// One piece of a parsed template: fixed text or a typed hole.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Part<'a> {
    Text(&'a str),
    Hole(char),
}

/// Parse a template into fixed text and holes. `{{`/`}}` are literal braces;
/// literal text containing them is kept as separate one-brace pieces.
fn template_parts(template: &'static str) -> Vec<Part<'static>> {
    let mut parts = Vec::new();
    let mut rest = template;
    while !rest.is_empty() {
        if let Some(after) = rest.strip_prefix("{{") {
            parts.push(Part::Text("{"));
            rest = after;
        } else if let Some(after) = rest.strip_prefix("}}") {
            parts.push(Part::Text("}"));
            rest = after;
        } else if rest.len() >= 3 && rest.as_bytes()[0] == b'{' && rest.as_bytes()[2] == b'}' {
            parts.push(Part::Hole(char::from(rest.as_bytes()[1])));
            rest = &rest[3..];
        } else {
            let end = rest.find(['{', '}']).unwrap_or(rest.len()).max(1);
            parts.push(Part::Text(&rest[..end]));
            rest = &rest[end..];
        }
    }
    parts
}

/// How many fixed characters a template shows.
fn fixed_len(parts: &[Part]) -> usize {
    parts
        .iter()
        .map(|p| match p {
            Part::Text(t) => t.chars().count(),
            Part::Hole(_) => 0,
        })
        .sum()
}

/// A rendering and how many characters of it are shown as written (fixed
/// text and holes in their strict shape). Every rendering is safe; among
/// several readings of the same text the most informative one is kept.
type Rendered = (usize, String);

/// The whole text, rendered through the data templates (or a bare value).
fn render_data(text: &str) -> Option<String> {
    if is_strict_value(text) {
        return Some(text.to_string());
    }
    render_with(text, &[&DATA_TEMPLATES, &SERDE_TEMPLATES], 0).map(|(_, out)| out)
}

/// Render `text` through the matching template that shows the most, or
/// `None`. A template must show at least one fixed character; nesting is
/// bounded.
fn render_with(text: &str, sets: &[&[&'static str]], depth: usize) -> Option<Rendered> {
    if depth > 6 || text.is_empty() {
        return None;
    }
    let mut best: Option<Rendered> = None;
    for template in sets.iter().flat_map(|set| set.iter()) {
        let parts = template_parts(template);
        if fixed_len(&parts) == 0 {
            continue;
        }
        if let Some(found) = match_parts(text, &parts, depth) {
            if best.as_ref().is_none_or(|(score, _)| found.0 > *score) {
                best = Some(found);
            }
        }
    }
    best
}

/// Match `text` against template parts: fixed text must appear exactly, a
/// hole takes a non-empty span and is rendered by its type. Of all ways to
/// split the text, the one that shows the most is kept.
fn match_parts(text: &str, parts: &[Part], depth: usize) -> Option<Rendered> {
    match parts.split_first() {
        None => text.is_empty().then(|| (0, String::new())),
        Some((Part::Text(fixed), rest)) => {
            let after = text.strip_prefix(fixed)?;
            match_parts(after, rest, depth)
                .map(|(score, tail)| (score + fixed.chars().count(), format!("{fixed}{tail}")))
        }
        Some((Part::Hole(kind), rest)) => {
            let mut best: Option<Rendered> = None;
            for end in (1..=text.len()).filter(|&e| text.is_char_boundary(e)) {
                let fits = match rest.first() {
                    None => end == text.len(),
                    Some(Part::Text(next)) => text[end..].starts_with(next),
                    Some(Part::Hole(_)) => false,
                };
                if !fits {
                    continue;
                }
                if let Some((tail_score, tail)) = match_parts(&text[end..], rest, depth) {
                    let (hole_score, hole) = render_hole(*kind, &text[..end], depth);
                    let score = hole_score + tail_score;
                    if best.as_ref().is_none_or(|(b, _)| score > *b) {
                        best = Some((score, format!("{hole}{tail}")));
                    }
                }
            }
            best
        }
    }
}

/// A hole's content in its strict shape, or `[REDACTED]` (inside its quotes).
fn render_hole(kind: char, content: &str, depth: usize) -> Rendered {
    let shown = match kind {
        'l' => is_voucher_label(content),
        'v' => is_strict_value(content),
        'e' => is_serde_expectation(content),
        'x' => {
            let out = redact_free_text(content);
            return (out.chars().count(), out);
        }
        'c' => {
            if is_strict_value(content) || is_code_token(content) || is_field_path(content) {
                true
            } else if let Some(found) = render_with(content, &[&CODE_FORMS], depth + 1) {
                return found;
            } else {
                false
            }
        }
        'm' => {
            if is_strict_value(content) {
                true
            } else if let Some(found) =
                render_with(content, &[&DATA_TEMPLATES, &SERDE_TEMPLATES], depth + 1)
            {
                return found;
            } else {
                false
            }
        }
        _ => false,
    };
    if shown {
        (content.chars().count(), content.to_string())
    } else {
        (0, redacted_value(content))
    }
}

/// `[REDACTED]`, keeping the quotes of a quoted value (`"[REDACTED]"`).
fn redacted_value(content: &str) -> String {
    match quoted_inner(content) {
        Some((quote, _)) => format!("{quote}{REDACTED}{quote}"),
        None => REDACTED.to_string(),
    }
}

/// The quote character and inside of `"…"`, `'…'` or `` `…` ``.
fn quoted_inner(s: &str) -> Option<(char, &str)> {
    let quote = s.chars().next()?;
    if !matches!(quote, '"' | '\'' | '`') || s.len() < 2 || !s.ends_with(quote) {
        return None;
    }
    Some((quote, &s[1..s.len() - 1]))
}

/// A value that cannot carry personal content: a number, an amount, a version
/// or a list of them, a date, a sha256, `receipt:N`/`income:N`, one of our
/// `<role>` labels, a known SIE tag or a boolean. A quoted value: its inside
/// in such a shape, or a single punctuation character (`'{'`). Never a word.
fn is_strict_value(s: &str) -> bool {
    if let Some((_, inner)) = quoted_inner(s) {
        return is_strict_bare(inner)
            || (inner.len() == 1 && inner.bytes().all(|c| c.is_ascii_punctuation()));
    }
    is_strict_bare(s)
}

fn is_strict_bare(s: &str) -> bool {
    is_role_label(s)
        || is_sie_tag(s)
        || s == "true"
        || s == "false"
        || is_sha256_hex(s)
        || is_plain_date(s)
        || is_iso_date(s)
        || is_source_key(s)
        || (!s.is_empty() && s.split(", ").all(is_number))
}

/// `12`, `-5000.00`, `+1.50`, `2026.1`, `1.10506.1`: digit groups (1–9 digits,
/// at most four) separated by dots, optional sign, never identifier-shaped.
fn is_number(s: &str) -> bool {
    let body = s.strip_prefix(['-', '+']).unwrap_or(s);
    let groups: Vec<&str> = body.split('.').collect();
    !body.is_empty()
        && groups.len() <= 4
        && groups
            .iter()
            .all(|g| (1..=9).contains(&g.len()) && g.bytes().all(|c| c.is_ascii_digit()))
        && !has_identifier_shape(s)
}

/// A valid `YYYY-MM-DD` date.
fn is_iso_date(s: &str) -> bool {
    s.len() == 10 && NaiveDate::parse_from_str(s, "%Y-%m-%d").is_ok()
}

/// `receipt:N` / `income:N` with a numeric id (snapshot ids are integers).
fn is_source_key(s: &str) -> bool {
    ["receipt:", "income:"].iter().any(|prefix| {
        s.strip_prefix(prefix)
            .is_some_and(|id| (1..=9).contains(&id.len()) && id.bytes().all(|c| c.is_ascii_digit()))
    })
}

/// An exact constant of SIEverk's own code (`CODE_TOKENS`).
fn is_code_token(s: &str) -> bool {
    CODE_TOKENS.binary_search(&s).is_ok()
}

/// A field path of serde field names: `receipts[0].net_amount`,
/// `_header.review_status`, `properties[]`.
fn is_field_path(s: &str) -> bool {
    !s.is_empty()
        && s.split('.').all(|segment| {
            let (name, index) = match segment.split_once('[') {
                Some((name, rest)) => (name, Some(rest)),
                None => (segment, None),
            };
            FIELD_NAMES.binary_search(&name).is_ok()
                && index.is_none_or(|rest| {
                    rest.strip_suffix(']').is_some_and(|digits| {
                        digits.len() <= 9 && digits.bytes().all(|c| c.is_ascii_digit())
                    })
                })
        })
}

/// What serde writes after `expected`: a type description, `struct Name`, or a
/// list of backticked field names / code tokens (`` `a`, `b` or `c` ``).
fn is_serde_expectation(s: &str) -> bool {
    if SERDE_EXPECTED.contains(&s) {
        return true;
    }
    if let Some(name) = s.strip_prefix("struct ") {
        return (1..=40).contains(&name.len())
            && name.starts_with(|c: char| c.is_ascii_uppercase())
            && name.bytes().all(|c| c.is_ascii_alphanumeric());
    }
    let list = s.strip_prefix("one of ").unwrap_or(s);
    !list.is_empty()
        && list
            .split(", ")
            .flat_map(|item| item.split(" or "))
            .all(|item| {
                item.strip_prefix('`')
                    .and_then(|i| i.strip_suffix('`'))
                    .is_some_and(|name| {
                        FIELD_NAMES.binary_search(&name).is_ok() || is_code_token(name)
                    })
            })
}

/// HY-SIE-01: library text (DuckDB, the OS) keeps its own words, but a quoted
/// span (`"…"`, `'…'`, `` `…` ``) is a value: shown only if numeric, a single
/// punctuation character or one of our `<role>` labels. An unclosed quote
/// redacts the rest of the text.
fn redact_free_text(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let mut out = String::with_capacity(text.len());
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '"' || c == '\'' || c == '`' {
            let Some(close) = chars[i + 1..].iter().position(|&d| d == c) else {
                out.push(c);
                out.push_str(REDACTED);
                return out;
            };
            let inner: String = chars[i + 1..i + 1 + close].iter().collect();
            out.push(c);
            out.push_str(if quoted_value_ok(&inner) {
                &inner
            } else {
                REDACTED
            });
            out.push(c);
            i += close + 2;
        } else {
            out.push(c);
            i += 1;
        }
    }
    out
}

/// A quoted value in library text that may be shown: numeric, a single
/// punctuation character, or an exact role label.
fn quoted_value_ok(inner: &str) -> bool {
    let numeric = !inner.is_empty()
        && inner
            .bytes()
            .all(|c| c.is_ascii_digit() || c == b'.' || c == b'-' || c == b'+')
        && (is_plain_date(inner) || !has_identifier_shape(inner));
    // A single punctuation character (`'{'`) is structure, not content.
    let punctuation = inner.len() == 1 && inner.bytes().all(|c| c.is_ascii_punctuation());
    numeric || punctuation || is_role_label(inner)
}

/// Exactly one of our own `<role>` labels.
fn is_role_label(s: &str) -> bool {
    ROLE_LABELS.contains(&s)
}

/// `YYYYMMDD` with a plausible year, month and day.
fn is_plain_date(s: &str) -> bool {
    s.len() == 8
        && s.bytes().all(|c| c.is_ascii_digit())
        && NaiveDate::parse_from_str(s, "%Y%m%d").is_ok_and(|d| {
            let y = chrono::Datelike::year(&d);
            (1900..=2099).contains(&y)
        })
}

/// A known SIE record tag (`#VER`, `#KONTO`, `#TRANS` …) — never any `#WORD`.
fn is_sie_tag(s: &str) -> bool {
    SIE_TAGS.contains(&s)
}

/// A validator voucher label `<series>-<number>`: the series is empty, ONE
/// uppercase letter or 1–3 digits; the number is digits (`A-1`, `-3`, `12-4`).
/// Any longer series (`BO`, `SK`, `anna`, `may`) is redacted — a series is
/// file text and could be a name, whether or not it is also an English word.
fn is_voucher_label(s: &str) -> bool {
    s.split_once('-').is_some_and(|(series, number)| {
        let series_ok = series.is_empty()
            || (series.len() == 1 && series.bytes().all(|c| c.is_ascii_uppercase()))
            || (series.len() <= 3 && series.bytes().all(|c| c.is_ascii_digit()));
        series_ok
            && number.len() <= 9
            && number.bytes().all(|c| c.is_ascii_digit())
            && !(series.is_empty() && number.is_empty())
    })
}

/// Every textual form in which `path` may appear in a message: as given,
/// absolute, canonical (also without the Windows verbatim prefix), via the
/// canonical parent, each with both separator styles.
fn path_forms(path: &Path) -> Vec<String> {
    let mut forms = vec![path.to_string_lossy().into_owned()];
    if let Ok(abs) = std::path::absolute(path) {
        forms.push(abs.to_string_lossy().into_owned());
    }
    if let Ok(canon) = fs::canonicalize(path) {
        forms.push(canon.to_string_lossy().into_owned());
    }
    if let (Some(parent), Some(name)) = (path.parent(), path.file_name()) {
        let parent = if parent.as_os_str().is_empty() {
            Path::new(".")
        } else {
            parent
        };
        if let Ok(canon) = fs::canonicalize(parent) {
            forms.push(canon.join(name).to_string_lossy().into_owned());
        }
    }
    let mut all = Vec::new();
    for form in forms {
        let plain = form
            .strip_prefix(r"\\?\UNC\")
            .map(|rest| format!(r"\\{rest}"))
            .or_else(|| form.strip_prefix(r"\\?\").map(str::to_string))
            .unwrap_or_else(|| form.clone());
        for f in [form, plain] {
            all.push(f.replace('\\', "/"));
            all.push(f.replace('/', "\\"));
            all.push(f);
        }
    }
    all
}

/// The pieces of `path` that must never appear in shown text: every named
/// component (as given and absolute) and the file stem.
fn path_components(path: &Path) -> Vec<String> {
    let mut out = Vec::new();
    let absolute = std::path::absolute(path).ok();
    for p in std::iter::once(path).chain(absolute.as_deref()) {
        for c in p.components() {
            if let std::path::Component::Normal(s) = c {
                out.push(s.to_string_lossy().into_owned());
            }
        }
    }
    if let Some(stem) = path.file_stem() {
        out.push(stem.to_string_lossy().into_owned());
    }
    out
}

/// Replace every known form of the user's paths, longest first, ASCII
/// case-insensitively (Windows paths are case-insensitive).
fn scrub_paths(text: &str, roles: &[Role]) -> String {
    let mut needles: Vec<(String, &'static str)> = Vec::new();
    for role in roles {
        for form in path_forms(role.path) {
            needles.push((form, role.label));
        }
        for part in [role.path.file_name(), role.path.file_stem()]
            .into_iter()
            .flatten()
        {
            needles.push((part.to_string_lossy().into_owned(), role.label));
        }
    }
    needles.retain(|(n, _)| n.len() >= 3);
    needles.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(&b.0)));
    needles.dedup_by(|a, b| a.0 == b.0);
    needles
        .iter()
        .fold(text.to_string(), |acc, (needle, label)| {
            replace_ascii_ci(&acc, needle, label)
        })
}

/// ASCII case-insensitive replace; byte offsets are preserved because only
/// ASCII letters change case.
fn replace_ascii_ci(haystack: &str, needle: &str, with: &str) -> String {
    let hay = haystack.to_ascii_lowercase();
    let pat = needle.to_ascii_lowercase();
    let mut out = String::with_capacity(haystack.len());
    let mut i = 0;
    while let Some(pos) = hay[i..].find(&pat) {
        let at = i + pos;
        out.push_str(&haystack[i..at]);
        out.push_str(with);
        i = at + pat.len();
    }
    out.push_str(&haystack[i..]);
    out
}

/// The fail-closed check on (already scrubbed) text: no path separator, no
/// identifier-shaped digits, no piece of the user's paths.
fn is_safe_detail(text: &str, roles: &[Role]) -> bool {
    if text.contains('/') || text.contains('\\') || has_identifier_shape(text) {
        return false;
    }
    // Ignore our own labels, then look for any remaining piece of a path.
    let mut bare = text.to_string();
    for role in roles {
        bare = bare.replace(role.label, " ");
    }
    let bare = bare.to_ascii_lowercase();
    !roles
        .iter()
        .flat_map(|r| path_components(r.path))
        .filter(|c| c.len() >= 4)
        .any(|c| bare.contains(&c.to_ascii_lowercase()))
}

/// Identifier-shaped digits: a run of 8+ digits (10/12-digit personnummer,
/// organisationsnummer) or `NNNNNN-NNNN` / `NNNNNNNN-NNNN` (also with `+`).
/// Digits inside a 64-character hex token (a sha256) and a lone valid
/// `YYYYMMDD` date are not identifiers.
fn has_identifier_shape(text: &str) -> bool {
    let b = text.as_bytes();
    let mut hex_skip = vec![false; b.len()];
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_hexdigit() {
            let start = i;
            while i < b.len() && b[i].is_ascii_hexdigit() {
                i += 1;
            }
            if i - start == 64 {
                hex_skip[start..i].iter_mut().for_each(|s| *s = true);
            }
        } else {
            i += 1;
        }
    }
    let mut i = 0;
    while i < b.len() {
        if b[i].is_ascii_digit() && !hex_skip[i] {
            let start = i;
            while i < b.len() && b[i].is_ascii_digit() {
                i += 1;
            }
            let run = i - start;
            let tail = &b[i..];
            let pn_tail = tail.len() >= 5
                && (tail[0] == b'-' || tail[0] == b'+')
                && tail[1..5].iter().all(u8::is_ascii_digit)
                && tail.get(5).is_none_or(|c| !c.is_ascii_digit());
            // A plain YYYYMMDD date on its own is a date, not an identifier.
            if run == 8 && !pn_tail && is_plain_date(&text[start..i]) {
                continue;
            }
            if run >= 8 {
                return true;
            }
            if (run == 6 || run == 8) && pn_tail {
                return true;
            }
        } else {
            i += 1;
        }
    }
    false
}

/// A bare `--option-name` (lowercase ASCII, digits, dashes) may be echoed;
/// anything else (a value, `--opt=value`, a path) is not.
fn is_option_name(arg: &str) -> bool {
    arg.strip_prefix("--").is_some_and(|name| {
        !name.is_empty()
            && name
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-')
    })
}

/// Workspace errors without the workspace path: fixed text for the path-
/// carrying variants, the category plus a checked detail for the rest.
fn workspace_error(e: &WorkspaceError, roles: &[Role]) -> String {
    match e {
        WorkspaceError::AlreadyExists(_) => {
            "the workspace already exists — refusing to reuse or overwrite it".to_string()
        }
        WorkspaceError::NotFound(_) => "the workspace does not exist".to_string(),
        WorkspaceError::Duckdb(inner) => format!("duckdb: {}", detail(inner, roles)),
        WorkspaceError::Corrupt(why) => {
            format!("workspace is corrupt: {}", data_detail(why, roles))
        }
        WorkspaceError::Snapshot(inner) => {
            format!("snapshot rejected: {}", data_detail(inner, roles))
        }
        other => data_sanitized(&other.to_string(), roles).unwrap_or_else(|| withheld("workspace")),
    }
}

/// Masterdata errors without the user's root: the root becomes
/// `<masterdata-root>`, a file name is shown only if it is one of the trusted
/// tracked names (otherwise a fixed role label), the element reference only if
/// it is plain ASCII structure, and the message only if it passes the check.
fn masterdata_error(e: &MasterdataError, root: &Path) -> String {
    let roles = [Role::new(root, "<masterdata-root>")];
    let root_text = root.to_string_lossy();
    let location = if e.path.is_empty() {
        None
    } else if e.path == root_text {
        Some("<masterdata-root>".to_string())
    } else if let Some(rest) = e.path.strip_prefix(root_text.as_ref()) {
        let rest = rest.trim_start_matches(['/', '\\']);
        let (file, element) = match rest.split_once('#') {
            Some((file, element)) => (file, Some(element)),
            None => (rest, None),
        };
        let file = if TRUSTED_MASTERDATA_FILES.contains(&file) {
            file
        } else {
            masterdata_role(file)
        };
        let element = element.map_or_else(String::new, |el| format!("#{}", render_element(el)));
        Some(format!("<masterdata-root>/{file}{element}"))
    } else {
        Some("<masterdata file>".to_string())
    };
    let message = data_detail(&e.message, &roles);
    match location {
        Some(loc) => format!("{loc}: {message}"),
        None => message,
    }
}

/// The fixed label for an untrusted masterdata file name — a constant chosen
/// by prefix, never text taken from the name.
fn masterdata_role(file: &str) -> &'static str {
    for (prefix, label) in [
        ("chart-", "<masterdata chart file>"),
        ("sru-", "<masterdata sru file>"),
        ("vat-rules-", "<masterdata vat-rules file>"),
        ("counter-accounts-", "<masterdata counter-accounts file>"),
        ("ruleset-", "<masterdata ruleset file>"),
    ] {
        if file.starts_with(prefix) {
            return label;
        }
    }
    "<masterdata file>"
}

/// A masterdata element reference: a field path as written (`accounts[12]`,
/// `_header.review_status`), an `ELEMENT_TEMPLATES` form whose ids are
/// redacted (`cases/[REDACTED]`, `accounts[3]/1930`), or `(element withheld)`.
fn render_element(el: &str) -> String {
    if is_field_path(el) {
        return el.to_string();
    }
    render_with(el, &[&ELEMENT_TEMPLATES], 0)
        .map_or_else(|| "(element withheld)".to_string(), |(_, out)| out)
}

#[cfg(test)]
mod tests {
    use super::{
        export_sie, fixed_len, has_identifier_shape, ingest_workspace, inspect_snapshot,
        inspect_workspace, is_chart_id, is_option_name, is_review_status, is_sha256_hex,
        is_strict_value, is_version, is_voucher_label, load_and_parse, masterdata_error,
        masterdata_role, parse_export_options, parse_options, parser_note, redact_free_text,
        render_data, render_element, render_inspect, sanitized, scrub_paths, template_parts,
        ExportOptions, Part, Role, CODE_FORMS, CODE_TOKENS, DATA_TEMPLATES, ELEMENT_TEMPLATES,
        FIELD_NAMES, REDACTED, SERDE_TEMPLATES, WITHHELD,
    };
    use chrono::NaiveDate;
    use sieverk::chart::MasterdataError;
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

    // HY-SIE-01: no display name, no identifier (not even partly), no path.
    #[test]
    fn cli_inspect_snapshot_redacts_identity_and_name() {
        for (fixture, secrets) in [
            (
                "fixtures/snapshots/minimal-1.1.json",
                &["999999-9999", "9999", "Kråksjö"][..],
            ),
            (
                "fixtures/snapshots/testgarden-2026-1.1.json",
                &["999999-0006", "Testgården"][..],
            ),
            (
                "fixtures/snapshots/testgarden-2026-1.2.json",
                &["900101-9802", "9001019802", "Testgården"][..],
            ),
        ] {
            let text = inspect_snapshot(fixture).expect("valid fixture inspects cleanly");
            assert!(
                text.contains(&format!("Entity:      {REDACTED} (org.nr ")),
                "{text}"
            );
            assert!(!text.contains("File:"), "{text}");
            assert!(!text.contains("fixtures"), "{text}");
            for secret in secrets {
                assert!(!text.contains(secret), "{secret} leaked: {text}");
            }
        }
        let legacy = inspect_snapshot("fixtures/snapshots/minimal-1.1.json").expect("1.1");
        assert!(legacy.contains(&format!("(org.nr {REDACTED})")), "{legacy}");
        let v12 = inspect_snapshot("fixtures/snapshots/testgarden-2026-1.2.json").expect("1.2");
        assert!(
            v12.contains("(org.nr —)"),
            "1.2 has no legacy org number: {v12}"
        );
    }

    #[test]
    fn cli_inspect_snapshot_reports_invalid_file_without_path() {
        let e = inspect_snapshot("fixtures/snapshots/invalid-net.json")
            .expect_err("invalid fixture must not inspect");
        assert!(e.starts_with("snapshot rejected: "), "{e}");
        assert!(e.contains("receipts[0].net_amount"), "{e}");
        assert!(e.contains("receipt:1001"), "{e}");
        assert!(!e.contains("invalid-net"), "{e}");
        assert!(!e.contains("fixtures"), "{e}");
    }

    #[test]
    fn cli_inspect_snapshot_reports_missing_file() {
        let e = inspect_snapshot("fixtures/snapshots/does-not-exist.json")
            .expect_err("missing file is an error");
        assert!(e.starts_with("could not read the snapshot file: "), "{e}");
        assert!(!e.contains("does-not-exist"), "{e}");
    }

    // -- HY-SIE-01: inspect-sie and the sanitizer --------------------------------

    #[test]
    fn cli_inspect_sie_redacts_company_and_org_number() {
        let parsed = load_and_parse("fixtures/minimal_valid.se").expect("fixture");
        let text = render_inspect(&parsed);
        assert!(
            text.contains(&format!("Company:     {REDACTED}\n")),
            "{text}"
        );
        assert!(
            text.contains(&format!("Org number:  {REDACTED}\n")),
            "{text}"
        );
        for secret in ["999999-9999", "Demo Skogsbruk AB", "File:", "minimal_valid"] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        // Counts and verdict are unchanged evidence.
        assert!(text.contains("Accounts:    7\n"), "{text}");
        assert!(text.contains("Vouchers:    2\n"), "{text}");
        assert!(text.contains("Status:      "), "{text}");
        // `Parsed` deliberately has no Debug (it holds #FNAMN/#ORGNR), so no expect_err.
        let Err(e) = load_and_parse("fixtures/no-such-NY9001019802.se") else {
            panic!("a missing SIE file must be an error");
        };
        assert!(e.starts_with("could not read the SIE file: "), "{e}");
        assert!(!e.contains("9001019802") && !e.contains("NY"), "{e}");
    }

    #[test]
    fn identifier_shapes_are_detected_but_sha256_is_not() {
        for leak in [
            "900101-9802",
            "19900101-9802",
            "9001019802",
            "199001019802",
            "x900101+9802y",
            "5560000000",
            "file 12345678 here",
        ] {
            assert!(has_identifier_shape(leak), "{leak}");
        }
        for fine in [
            "3c225ff8332d2942d6b9115c19ab8fec0b47de53f0a0ef8fad404b63259bdba0",
            "2026-01-01..2026-12-31",
            "receipt:1001",
            "case 21: amount 1234567",
            "12345-1234",
            "dated 20260602 outside fiscal year 20260101 – 20261231",
            "19900101",
            "",
        ] {
            assert!(!has_identifier_shape(fine), "{fine}");
        }
        // 8+ digits inside a hex token that is not exactly 64 long still count.
        assert!(has_identifier_shape("abc90010198021def"));
    }

    #[test]
    fn sanitizer_scrubs_known_paths_and_withholds_the_rest() {
        let ws = Path::new("tmp-hy/ws-9001019802.duckdb");
        let roles = [Role::new(ws, "<workspace>")];
        // A detail naming exactly the user's path (any separator, any case) is shown scrubbed.
        assert_eq!(
            sanitized("could not open tmp-hy/ws-9001019802.duckdb", &roles).as_deref(),
            Some("could not open <workspace>")
        );
        assert_eq!(
            sanitized(r"could not open TMP-HY\WS-9001019802.DUCKDB", &roles).as_deref(),
            Some("could not open <workspace>")
        );
        assert_eq!(
            scrub_paths("lock on ws-9001019802.duckdb held", &roles),
            "lock on <workspace> held"
        );
        // The bare file stem is a known form too.
        assert_eq!(
            sanitized("temp ws-9001019802 left behind", &roles).as_deref(),
            Some("temp <workspace> left behind")
        );
        // Anything that still looks like a path or identifier is withheld.
        for unsafe_text in [
            "could not open /other/place/file.duckdb",
            r"C:\Users\x\ws.duckdb",
            "IO Error: 900101-9802",
            "temp 9001019802 left behind",
            "mentions tmp-hy somewhere",
        ] {
            assert_eq!(sanitized(unsafe_text, &roles), None, "{unsafe_text}");
        }
        assert_eq!(WITHHELD, "(details withheld)");
    }

    #[test]
    fn data_text_keeps_template_text_and_redacts_values() {
        let data = |t: &str| render_data(t).unwrap_or_else(|| WITHHELD.to_string());
        for (input, expected) in [
            ("-1 sums to -5000.00", "-1 sums to -5000.00"),
            ("A-1 sums to -100.00", "A-1 sums to -100.00"),
            (
                "A-1 has a malformed date 'Erik'",
                "A-1 has a malformed date '[REDACTED]'",
            ),
            (
                "-2 is dated 20260602 outside fiscal year Erik – Svensson",
                "-2 is dated 20260602 outside fiscal year [REDACTED] – [REDACTED]",
            ),
            (
                "invalid snapshot JSON: invalid type: string \"Anna Svensson\", expected u32 at line 3 column 5",
                "invalid snapshot JSON: invalid type: string \"[REDACTED]\", expected u32 at line 3 column 5",
            ),
            (
                "invalid snapshot JSON: unknown field `vendor_name`, expected `vendor` or `note` at line 2 column 9",
                "invalid snapshot JSON: unknown field `[REDACTED]`, expected `vendor` or `note` at line 2 column 9",
            ),
            (
                "invalid snapshot JSON: missing field `receipts` at line 1 column 2",
                "invalid snapshot JSON: missing field `receipts` at line 1 column 2",
            ),
            (
                "#VER ERIK-2 was never closed before the next #VER",
                "#VER [REDACTED] was never closed before the next #VER",
            ),
            (
                "#VER SK-12 block never closes before end of file",
                "#VER [REDACTED] block never closes before end of file",
            ),
            (
                "#VER A-12 block never closes before end of file",
                "#VER A-12 block never closes before end of file",
            ),
            (
                "receipts[0].net_amount: receipt:1001 net 999.00 != total 1250.00 - vat 250.00 - rounding 0.00 (= 1000.00)",
                "receipts[0].net_amount: receipt:1001 net 999.00 != total 1250.00 - vat 250.00 - rounding 0.00 (= 1000.00)",
            ),
            (
                "must be draft or approved, got \"x\"",
                "must be draft or approved, got \"[REDACTED]\"",
            ),
            (
                "#TRANS amount '12.50' is not a valid amount",
                "#TRANS amount '12.50' is not a valid amount",
            ),
            (
                "#TRANS amount 'tolv' is not a valid amount",
                "#TRANS amount '[REDACTED]' is not a valid amount",
            ),
            (
                "unexpected '{' with no #VER header before it",
                "unexpected '{' with no #VER header before it",
            ),
            ("#IB row is malformed", "#IB row is malformed"),
            ("#ANNA row is malformed", "[REDACTED] row is malformed"),
            (
                "workspace <workspace> does not exist",
                "workspace <workspace> does not exist",
            ),
            (
                "#FNAMN: 'Ω' cannot be encoded as CP437",
                "#FNAMN: '[REDACTED]' cannot be encoded as CP437",
            ),
            (
                "#KONTO 1930: control character at index 3",
                "#KONTO 1930: control character at index 3",
            ),
            (
                "workspace is corrupt: finding 2: code UNRESOLVED_VAT out of fixed rank order",
                "workspace is corrupt: finding 2: code UNRESOLVED_VAT out of fixed rank order",
            ),
            (
                "duckdb: IO Error: Cannot open \"Erik\"",
                "duckdb: IO Error: Cannot open \"[REDACTED]\"",
            ),
        ] {
            assert_eq!(data(input), expected, "{input}");
        }
        // Text that matches no template is withheld entirely.
        for unknown in [
            "Kråksjö gård",
            "lars",
            "anna sums",
            "the <snapshot> file",
            "invalid \"Erik rest",
            "",
        ] {
            assert_eq!(render_data(unknown), None, "{unknown}");
        }
        // Library text keeps its own words; quoted values are still redacted.
        assert_eq!(
            redact_free_text("IO Error: The file \"<workspace>\" exists"),
            "IO Error: The file \"<workspace>\" exists"
        );
        assert_eq!(
            redact_free_text("Constraint Error: Duplicate key \"Erik\""),
            "Constraint Error: Duplicate key \"[REDACTED]\""
        );
        assert_eq!(
            redact_free_text("Binder Error: column `anna`"),
            "Binder Error: column `[REDACTED]`"
        );
        assert!(is_voucher_label("A-1") && is_voucher_label("12-4") && is_voucher_label("-3"));
        assert!(!is_voucher_label("ERIK-2") && !is_voucher_label("a-1") && !is_voucher_label("-"));
        assert!(
            !is_voucher_label("SK-12") && !is_voucher_label("BO-4") && !is_voucher_label("anna-1")
        );
    }

    #[test]
    fn parser_notes_never_carry_the_raw_line() {
        assert_eq!(
            parser_note("#RAR row is malformed: #RAR 0 Erik Svensson"),
            "#RAR row is malformed"
        );
        assert_eq!(
            parser_note("#TRANS amount 'x' is not a valid amount: #TRANS 1930 {} x"),
            "#TRANS amount 'x' is not a valid amount"
        );
        assert_eq!(
            parser_note("#VER A-1 block never closes before end of file"),
            "#VER A-1 block never closes before end of file"
        );
    }

    #[test]
    fn masterdata_fields_are_shown_only_in_their_technical_shape() {
        assert!(is_chart_id("SYNTHETIC_K1"));
        assert!(!is_chart_id("Erik Svensson") && !is_chart_id("synthetic"));
        assert!(is_version("2026.1") && is_version("1.0"));
        assert!(!is_version("v1") && !is_version("Erik"));
        assert!(is_sha256_hex(
            "35684d165b131e841a999d9da2ef39e2ec966ecc9ffc363de166f94876e1b0ef"
        ));
        assert!(!is_sha256_hex("Erik"));
        assert_eq!(render_element("cases/Erik"), "cases/[REDACTED]");
    }

    // HY-SIE-01 REV3/REV4: a value from data is never trusted for its case or
    // for being an English word — `may`, `mine`, `case` are redacted like `anna`.
    #[test]
    fn dynamic_ids_are_redacted_even_when_they_are_english_words() {
        for id in [
            "may", "mine", "can", "case", "rule", "receipt", "anna", "Anna", "ANNA", "BO", "lars",
        ] {
            for (template, expected) in [
                ("{id}-1 sums to 50.00", "[REDACTED] sums to 50.00"),
                ("{id}-2 appears more than once", "[REDACTED] appears more than once"),
                (
                    "#VER {id}-1 was never closed before the next #VER",
                    "#VER [REDACTED] was never closed before the next #VER",
                ),
                (
                    "#VER {id}-1 block never closes before end of file",
                    "#VER [REDACTED] block never closes before end of file",
                ),
                (
                    "case 3: property {id} is not in the workspace",
                    "case 3: property [REDACTED] is not in the workspace",
                ),
                (
                    "case 3: masterdata: rule case {id} has no vat rule",
                    "case 3: masterdata: rule case [REDACTED] has no vat rule",
                ),
                (
                    "case 3: masterdata: vat rule {id} requires manual review",
                    "case 3: masterdata: vat rule [REDACTED] requires manual review",
                ),
                (
                    "case 3: masterdata: default rule case {id} for receipt subject \"{id}\" has no account",
                    "case 3: masterdata: default rule case [REDACTED] for receipt subject \"[REDACTED]\" has no account",
                ),
                (
                    "workspace is corrupt: case 2: counter_key {id} != payment_method {id}",
                    "workspace is corrupt: case 2: counter_key [REDACTED] != payment_method [REDACTED]",
                ),
                (
                    "workspace is corrupt: duplicate source_key receipt:{id}",
                    "workspace is corrupt: duplicate source_key [REDACTED]",
                ),
                (
                    "workspace is corrupt: case 1 does not mirror receipt receipt:{id} (selector_position 0)",
                    "workspace is corrupt: case 1 does not mirror receipt [REDACTED] (selector_position 0)",
                ),
                ("account {id} is not in the chart", "account [REDACTED] is not in the chart"),
                (
                    "expected sieverk-chart/1, got {id}",
                    "expected sieverk-chart/1, got [REDACTED]",
                ),
                (
                    "receipts[0].{id}: must be a string",
                    "[REDACTED]: must be a string",
                ),
                (
                    "invalid ruleset JSON: unknown variant `{id}`, expected `draft` or `approved` at line 4 column 2",
                    "invalid ruleset JSON: unknown variant `[REDACTED]`, expected `draft` or `approved` at line 4 column 2",
                ),
            ] {
                let input = template.replace("{id}", id);
                let shown = render_data(&input).unwrap_or_else(|| WITHHELD.to_string());
                assert_eq!(shown, expected, "{input}");
            }
            assert_eq!(render_element(&format!("cases/{id}")), "cases/[REDACTED]");
            assert_eq!(render_element(&format!("rules/{id}")), "rules/[REDACTED]");
            assert_eq!(
                render_element(&format!("counter/receipt/{id}")),
                "counter/receipt/[REDACTED]"
            );
            assert_eq!(
                render_element(&format!("cases/{id}.default")),
                "cases/[REDACTED]"
            );
            assert!(!is_strict_value(id) && !is_voucher_label(&format!("{id}-1")));
        }
        assert!(!is_chart_id("ANNA") && !is_review_status("anna") && !is_review_status("may"));
    }

    #[test]
    fn template_tables_are_well_formed() {
        for set in [
            &DATA_TEMPLATES[..],
            &SERDE_TEMPLATES[..],
            &CODE_FORMS[..],
            &ELEMENT_TEMPLATES[..],
        ] {
            for template in set {
                let parts = template_parts(template);
                assert!(fixed_len(&parts) > 0, "{template}: no fixed text");
                assert!(
                    parts
                        .windows(2)
                        .all(|w| !matches!(w, [Part::Hole(_), Part::Hole(_)])),
                    "{template}: adjacent holes"
                );
                for part in &parts {
                    match part {
                        Part::Hole(kind) => {
                            assert!("vlcmxe".contains(*kind), "{template}: hole {kind}")
                        }
                        Part::Text(t) => assert!(
                            *t == "{" || *t == "}" || !t.contains(['{', '}']),
                            "{template}: stray brace"
                        ),
                    }
                }
            }
        }
        for list in [&CODE_TOKENS[..], &FIELD_NAMES[..]] {
            assert!(list.windows(2).all(|w| w[0] < w[1]), "sorted and unique");
            assert!(list.iter().all(|w| w.is_ascii() && !w.contains(' ')));
        }
        // Templates show fixed text only: no name, no identifier.
        for template in DATA_TEMPLATES.iter().chain(SERDE_TEMPLATES.iter()) {
            assert!(!has_identifier_shape(template), "{template}");
            assert!(
                !template.to_ascii_lowercase().contains("vibeke"),
                "{template}"
            );
        }
    }

    #[test]
    fn option_names_only_are_echoed() {
        assert!(is_option_name("--force"));
        assert!(is_option_name("--org-number"));
        assert!(!is_option_name("--org-number=900101-9802"));
        assert!(!is_option_name("--"));
        assert!(!is_option_name("-x"));
        assert!(!is_option_name("C:/x"));
        assert!(!is_option_name("--Path"));
    }

    #[test]
    fn masterdata_errors_show_only_trusted_file_names() {
        let root = Path::new("md-9001019802-Kråksjö");
        let at = |file: &str, element: &str, message: &str| {
            let path = if element.is_empty() {
                root.join(file).display().to_string()
            } else {
                format!("{}#{element}", root.join(file).display())
            };
            masterdata_error(&MasterdataError::new(path, message), root)
        };
        assert_eq!(
            at("chart-synthetic-k1-2026.1.json", "accounts[12]", "duplicate account number"),
            "<masterdata-root>/chart-synthetic-k1-2026.1.json#accounts[12]: duplicate account number"
        );
        assert_eq!(
            at(
                "chart-9001019802.json",
                "accounts[3]",
                "duplicate account number"
            ),
            "<masterdata-root>/<masterdata chart file>#accounts[3]: duplicate account number"
        );
        assert_eq!(
            at("chart-Kråksjö.json", "", "invalid chart JSON: expected value at line 1 column 1"),
            "<masterdata-root>/<masterdata chart file>: invalid chart JSON: expected value at line 1 column 1"
        );
        assert_eq!(
            at("ruleset-2026.1.json", "Kråksjö", "unknown automation"),
            "<masterdata-root>/ruleset-2026.1.json#(element withheld): unknown automation"
        );
        assert_eq!(
            at("ruleset-2026.1.json", "cases/may", "unknown automation"),
            "<masterdata-root>/ruleset-2026.1.json#cases/[REDACTED]: unknown automation"
        );
        assert_eq!(
            at("chart-synthetic-k1-2026.1.json", "accounts[3]/1930", "duplicate account number"),
            "<masterdata-root>/chart-synthetic-k1-2026.1.json#accounts[3]/1930: duplicate account number"
        );
        assert_eq!(masterdata_role("unknown.json"), "<masterdata file>");
        let whole = masterdata_error(
            &MasterdataError::new(root.display().to_string(), "no chart-*.json in root"),
            root,
        );
        assert_eq!(whole, "<masterdata-root>: no chart-*.json in root");
        let foreign = masterdata_error(
            &MasterdataError::new("/elsewhere/x.json", "duplicate case_id"),
            root,
        );
        assert_eq!(foreign, "<masterdata file>: duplicate case_id");
        // Ids from masterdata content are redacted, whatever words they are made of.
        assert_eq!(render_element("cases/bransle.default"), "cases/[REDACTED]");
        assert_eq!(render_element("cases/anna"), "cases/[REDACTED]");
        assert_eq!(render_element("accounts[12]"), "accounts[12]");
        assert_eq!(
            render_element("_header.review_status"),
            "_header.review_status"
        );
        assert_eq!(render_element("x.json"), "(element withheld)");
        assert_eq!(render_element("900101-9802"), "(element withheld)");
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
        assert!(!text.contains("workspace="), "no path echo: {text}");
        assert!(!text.contains("sieverk-cli-tests"), "{text}");
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
        assert_eq!(
            e,
            "the workspace already exists — refusing to reuse or overwrite it"
        );
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
        assert!(e.starts_with("could not read the snapshot file: "), "{e}");
        assert!(!e.contains("does-not-exist"), "{e}");
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
        assert_eq!(e, "the workspace does not exist");
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
        assert_eq!(e, "unexpected positional argument");
        let e = parse_options(
            &args(&[
                "--snapshot",
                "s",
                "--workspace",
                "w",
                "--org-number=900101-9802",
            ]),
            req,
        )
        .expect_err("option with value");
        assert_eq!(e, "unknown option");
        let e = parse_options(&args(&[]), &["--workspace"]).expect_err("nothing given");
        assert!(e.contains("missing required option --workspace"), "{e}");
    }

    // -- SV-04 Slice 1 + SK-SIE-BRIDGE-04: export-sie -----------------------------

    const SYNTHETIC_ROOT: &str = "fixtures/masterdata/synthetic/generated";
    const TESTGARDEN_12: &str = "fixtures/snapshots/testgarden-2026-1.2.json";
    /// vNext canonical Testgården (BRIDGE-04, CASE B — blank series,
    /// writer-assigned numbers 1..N): locked from the real writer bytes.
    const TESTGARDEN_VNEXT_SHA256: &str =
        "3c225ff8332d2942d6b9115c19ab8fec0b47de53f0a0ef8fad404b63259bdba0";

    fn export_opts(name: &str, snapshot: &str) -> (TempDb, TempDb, ExportOptions) {
        let db = TempDb::new(name);
        let out = TempDb::new(&format!("{name}-out"));
        let out_path = out.path().with_extension("SI");
        let _ = std::fs::remove_file(&out_path);
        let opts = ExportOptions {
            snapshot: PathBuf::from(snapshot),
            root: PathBuf::from(SYNTHETIC_ROOT),
            workspace: db.path().to_path_buf(),
            out: out_path,
            generated_on: NaiveDate::from_ymd_opt(2026, 9, 10).expect("date"),
        };
        (db, out, opts)
    }

    #[test]
    fn cli_export_sie_generates_the_vnext_testgarden_file() {
        let (_db, _out, opts) = export_opts("export", TESTGARDEN_12);
        let text = export_sie(&opts).expect("export");
        let bytes = std::fs::read(&opts.out).expect("out file");
        assert_eq!(bytes.len(), 489, "{text}");
        assert!(
            text.contains(&format!("si_sha256={TESTGARDEN_VNEXT_SHA256}\n")),
            "{text}"
        );
        assert!(
            text.contains("vouchers=2\n") && text.contains("accounts=4\n"),
            "{text}"
        );
        assert!(text.contains("entity_type=sole_trader\n"), "{text}");
        assert!(text.contains("identifier_kind=personal_number\n"), "{text}");
        assert!(
            text.contains("fiscal_period=2026-01-01..2026-12-31\n"),
            "{text}"
        );
        // Evidence never carries the identifier, the legal name or a path.
        for secret in [
            "900101-9802",
            "9001019802",
            "Testgården",
            "workspace=",
            "out=",
            "sieverk-cli-tests",
        ] {
            assert!(!text.contains(secret), "{secret} leaked: {text}");
        }
        // Existing output path is refused; the workspace already exists too.
        let e = export_sie(&opts).expect_err("second run must refuse");
        assert!(
            e.contains("already exists") || e.contains("could not create the output file"),
            "{e}"
        );
        assert!(!e.contains("sieverk-cli-tests"), "{e}");
        let _ = std::fs::remove_file(&opts.out);
    }

    #[test]
    fn cli_export_sie_refuses_a_snapshot_without_accounting_identity() {
        let (_db, _out, opts) = export_opts("export-11", TESTGARDEN);
        let e = export_sie(&opts).expect_err("1.1 has no accounting identity");
        assert!(e.contains("no accounting identity"), "{e}");
        assert!(!opts.out.exists(), "no file is written on error");
    }

    #[test]
    fn cli_export_sie_option_parsing_is_strict() {
        let base = [
            "--snapshot",
            "s.json",
            "--root",
            "r",
            "--workspace",
            "w.duckdb",
            "--out",
            "o.SI",
            "--generated-on",
            "2026-09-10",
        ];
        let ok = parse_export_options(&args(&base)).expect("complete");
        assert_eq!(ok.out, PathBuf::from("o.SI"));
        for removed in [
            ["--company-name", "Testgården"],
            ["--org-number", "999999-0006"],
        ] {
            let mut with = base.to_vec();
            with.extend(removed);
            let e = parse_export_options(&args(&with)).expect_err("identity options are gone");
            assert!(e.contains("unknown option"), "{e}");
        }
        let mut bad_date = base.to_vec();
        bad_date[9] = "20260910";
        assert_eq!(
            parse_export_options(&args(&bad_date)).expect_err("date"),
            "--generated-on is not YYYY-MM-DD",
            "the rejected value is never echoed"
        );
        let missing: Vec<&str> = base[..8].to_vec();
        assert!(parse_export_options(&args(&missing))
            .expect_err("missing")
            .contains("--generated-on"));
        let mut unknown = base.to_vec();
        unknown.push("--force");
        assert!(parse_export_options(&args(&unknown))
            .expect_err("unknown")
            .contains("unknown option"));
    }
}
