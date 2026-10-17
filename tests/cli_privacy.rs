//! HY-SIE-01 — the real, compiled `sieverk` binary prints no identifier, no
//! legal or display name and no user-supplied path on stdout or stderr, on
//! success and on failure. Every sandbox directory and every input file name
//! carries the identifier digits `9001019802` and the name `Kråksjö`, so any
//! echo of any piece of any path is caught by the same secret list. Exit codes,
//! error categories, hashes and counters are asserted to be unchanged.

use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_sieverk");
const CRATE: &str = env!("CARGO_MANIFEST_DIR");
const SYNTHETIC_ROOT: &str = "fixtures/masterdata/synthetic/generated";
const TESTGARDEN_11: &str = "fixtures/snapshots/testgarden-2026-1.1.json";
const TESTGARDEN_12: &str = "fixtures/snapshots/testgarden-2026-1.2.json";
const MINIMAL_11: &str = "fixtures/snapshots/minimal-1.1.json";
/// CASE B canonical Testgården (BRIDGE-04, d1da159).
const CASE_B_SHA256: &str = "3c225ff8332d2942d6b9115c19ab8fec0b47de53f0a0ef8fad404b63259bdba0";

/// Never printed, in any command, on any outcome.
const SECRETS: [&str; 12] = [
    "9001019802",
    "900101-9802",
    "19900101-9802",
    "199001019802",
    "Testgården",
    "Kråksjö",
    "Demo Skogsbruk AB",
    "999999-9999",
    "9999999999",
    "999999-0006",
    "9999990006",
    "hy-sie-01-",
];

/// A fresh directory whose name itself is personal-looking; removed on drop.
struct Sandbox {
    dir: PathBuf,
}

impl Sandbox {
    fn new(tag: &str) -> Self {
        let dir = std::env::temp_dir().join(format!(
            "hy-sie-01-{}-{tag}-9001019802-Kråksjö",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).expect("sandbox");
        Self { dir }
    }
    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(name)
    }
    fn copy(&self, fixture: &str, name: &str) -> PathBuf {
        let to = self.path(name);
        fs::copy(repo(fixture), &to).expect("copy fixture");
        to
    }
    fn write(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let to = self.path(name);
        fs::write(&to, bytes).expect("write");
        to
    }
    /// A copy of the tracked synthetic masterdata root.
    fn masterdata(&self, name: &str) -> PathBuf {
        let to = self.path(name);
        fs::create_dir_all(&to).expect("md dir");
        for entry in fs::read_dir(repo(SYNTHETIC_ROOT)).expect("synthetic root") {
            let entry = entry.expect("entry");
            fs::copy(entry.path(), to.join(entry.file_name())).expect("copy md");
        }
        to
    }
    /// Every textual form of the sandbox directory (must never be printed).
    fn forms(&self) -> Vec<String> {
        let mut forms = vec![self.dir.to_string_lossy().into_owned()];
        if let Ok(c) = fs::canonicalize(&self.dir) {
            let c = c.to_string_lossy().into_owned();
            forms.push(c.trim_start_matches(r"\\?\").to_string());
            forms.push(c);
        }
        let mut all = Vec::new();
        for f in forms {
            all.push(f.replace('\\', "/"));
            all.push(f.replace('/', "\\"));
            all.push(f);
        }
        all
    }
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn repo(rel: &str) -> PathBuf {
    Path::new(CRATE).join(rel)
}

fn run<I, S>(args: I, cwd: &Path) -> Output
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new(BIN)
        .args(args)
        .current_dir(cwd)
        .output()
        .expect("spawn sieverk")
}

/// The privacy check on one process run: nothing secret, no path form, no
/// path-echo key, on either stream. Returns (stdout, stderr) for positive checks.
fn clean(out: &Output, sb: &Sandbox, extra: &[&str]) -> (String, String) {
    let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let forms = sb.forms();
    let needles = SECRETS
        .iter()
        .copied()
        .chain(extra.iter().copied())
        .chain(forms.iter().map(String::as_str));
    for needle in needles {
        assert!(
            !stdout.contains(needle) && !stderr.contains(needle),
            "{needle:?} leaked\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
        );
    }
    for key in ["workspace=", "out=", "File:", "Root:"] {
        assert!(!stdout.contains(key), "{key} echoed: {stdout}");
    }
    (stdout, stderr)
}

fn ok(out: &Output, sb: &Sandbox) -> String {
    let (stdout, stderr) = clean(out, sb, &[]);
    assert!(out.status.success(), "expected success\n{stdout}\n{stderr}");
    assert!(stderr.is_empty(), "nothing on stderr on success: {stderr}");
    stdout
}

fn fails(out: &Output, sb: &Sandbox, extra: &[&str]) -> String {
    let (stdout, stderr) = clean(out, sb, extra);
    assert!(
        !out.status.success(),
        "expected failure\n{stdout}\n{stderr}"
    );
    assert!(!stderr.is_empty(), "a failure explains itself on stderr");
    stderr
}

fn export_args<'a>(
    snapshot: &'a Path,
    root: &'a Path,
    workspace: &'a Path,
    out: &'a Path,
) -> Vec<&'a OsStr> {
    vec![
        OsStr::new("export-sie"),
        OsStr::new("--snapshot"),
        snapshot.as_os_str(),
        OsStr::new("--root"),
        root.as_os_str(),
        OsStr::new("--workspace"),
        workspace.as_os_str(),
        OsStr::new("--out"),
        out.as_os_str(),
        OsStr::new("--generated-on"),
        OsStr::new("2026-09-10"),
    ]
}

// P1 — export-sie, inspect-sie (and the bare-path form), validate-sie: success
// with absolute paths; then the refusals (workspace exists, output exists).
#[test]
fn p1_export_and_sie_inspection_succeed_without_leaks() {
    let sb = Sandbox::new("p1");
    let snap = sb.copy(TESTGARDEN_12, "snap-9001019802-Kråksjö.json");
    let md = sb.masterdata("md-9001019802");
    let ws = sb.path("ws-9001019802.duckdb");
    let si = sb.path("NY9001019802.si");
    let cwd = Path::new(CRATE);

    let stdout = ok(&run(export_args(&snap, &md, &ws, &si), cwd), &sb);
    assert!(
        stdout.contains(&format!("si_sha256={CASE_B_SHA256}\n")),
        "{stdout}"
    );
    for line in [
        "vouchers=2\n",
        "accounts=4\n",
        "bytes=489\n",
        "entity_type=sole_trader\n",
        "identifier_kind=personal_number\n",
        "fiscal_period=2026-01-01..2026-12-31\n",
    ] {
        assert!(stdout.contains(line), "{line}: {stdout}");
    }
    let written = fs::read(&si).expect("written SI");
    assert_eq!(written.len(), 489);

    for args in [
        vec![OsStr::new("inspect-sie"), si.as_os_str()],
        vec![si.as_os_str()],
    ] {
        let stdout = ok(&run(args, cwd), &sb);
        assert!(stdout.contains("Company:     [REDACTED]\n"), "{stdout}");
        assert!(stdout.contains("Org number:  [REDACTED]\n"), "{stdout}");
        assert!(stdout.contains("Vouchers:    2\n"), "{stdout}");
        assert!(stdout.contains("Ledger rows: 6\n"), "{stdout}");
    }
    let stdout = ok(&run([OsStr::new("validate-sie"), si.as_os_str()], cwd), &sb);
    assert!(stdout.contains("Status:      "), "{stdout}");

    // Same command again: the workspace exists — refused, output untouched.
    let stderr = fails(&run(export_args(&snap, &md, &ws, &si), cwd), &sb, &[]);
    assert!(
        stderr.contains("the workspace already exists — refusing to reuse or overwrite it"),
        "{stderr}"
    );
    assert_eq!(fs::read(&si).expect("SI"), written, "never overwritten");
    // New workspace, existing output file: refused at create_new.
    let ws2 = sb.path("ws2-9001019802.duckdb");
    let stderr = fails(&run(export_args(&snap, &md, &ws2, &si), cwd), &sb, &[]);
    assert!(
        stderr.starts_with("could not create the output file: "),
        "{stderr}"
    );
}

// P2 — inspect-snapshot on 1.1 (legacy org number) and 1.2 (accounting identity).
#[test]
fn p2_snapshot_inspection_redacts_names_and_identifiers() {
    let sb = Sandbox::new("p2");
    let cwd = Path::new(CRATE);
    for (fixture, name, legacy) in [
        (MINIMAL_11, "minimal-9001019802.json", true),
        (TESTGARDEN_11, "tg11-9001019802-Kråksjö.json", true),
        (TESTGARDEN_12, "tg12-9001019802.json", false),
    ] {
        let path = sb.copy(fixture, name);
        let stdout = ok(
            &run([OsStr::new("inspect-snapshot"), path.as_os_str()], cwd),
            &sb,
        );
        let org = if legacy { "[REDACTED]" } else { "—" };
        assert!(
            stdout.contains(&format!("Entity:      [REDACTED] (org.nr {org})\n")),
            "{stdout}"
        );
        assert!(stdout.contains("Income year: 2026\n"), "{stdout}");
    }
}

// P3 — ingest and inspect-workspace succeed; a second ingest and a missing
// workspace are refused with fixed texts.
#[test]
fn p3_workspace_commands_succeed_and_refuse_without_leaks() {
    let sb = Sandbox::new("p3");
    let cwd = Path::new(CRATE);
    let snap = sb.copy(TESTGARDEN_11, "snap-9001019802.json");
    let ws = sb.path("ws-9001019802.duckdb");
    let ingest = || {
        run(
            [
                OsStr::new("ingest"),
                OsStr::new("--snapshot"),
                snap.as_os_str(),
                OsStr::new("--workspace"),
                ws.as_os_str(),
            ],
            cwd,
        )
    };
    let stdout = ok(&ingest(), &sb);
    assert!(stdout.starts_with("snapshot_sha256="), "{stdout}");
    assert!(stdout.contains("receipts=21\n"), "{stdout}");

    let stdout = ok(
        &run(
            [
                OsStr::new("inspect-workspace"),
                OsStr::new("--workspace"),
                ws.as_os_str(),
            ],
            cwd,
        ),
        &sb,
    );
    assert!(stdout.contains("workspace_schema=1\n"), "{stdout}");

    let stderr = fails(&ingest(), &sb, &[]);
    assert!(stderr.contains("the workspace already exists"), "{stderr}");

    let missing = sb.path("missing-9001019802.duckdb");
    let stderr = fails(
        &run(
            [
                OsStr::new("inspect-workspace"),
                OsStr::new("--workspace"),
                missing.as_os_str(),
            ],
            cwd,
        ),
        &sb,
        &[],
    );
    assert_eq!(stderr.trim_end(), "the workspace does not exist");
    assert!(!missing.exists(), "inspect never creates");
}

// F1 — every command with a missing input fails with its fixed category text.
#[test]
fn f1_missing_inputs_fail_with_fixed_categories() {
    let sb = Sandbox::new("f1");
    let cwd = Path::new(CRATE);
    let missing_si = sb.path("NY9001019802-missing.si");
    let missing_json = sb.path("snap-9001019802-missing.json");
    let missing_root = sb.path("md-9001019802-missing");
    let ws = sb.path("ws-9001019802.duckdb");
    let out = sb.path("out-9001019802.si");
    let md = sb.masterdata("md-9001019802");

    let cases: Vec<(Vec<&OsStr>, &str)> = vec![
        (
            vec![OsStr::new("inspect-sie"), missing_si.as_os_str()],
            "could not read the SIE file: ",
        ),
        (
            vec![missing_si.as_os_str()],
            "could not read the SIE file: ",
        ),
        (
            vec![OsStr::new("validate-sie"), missing_si.as_os_str()],
            "could not read the SIE file: ",
        ),
        (
            vec![OsStr::new("inspect-snapshot"), missing_json.as_os_str()],
            "could not read the snapshot file: ",
        ),
        (
            vec![
                OsStr::new("ingest"),
                OsStr::new("--snapshot"),
                missing_json.as_os_str(),
                OsStr::new("--workspace"),
                ws.as_os_str(),
            ],
            "could not read the snapshot file: ",
        ),
        (
            export_args(&missing_json, &md, &ws, &out),
            "could not read the snapshot file: ",
        ),
        (
            vec![
                OsStr::new("inspect-masterdata"),
                OsStr::new("--root"),
                missing_root.as_os_str(),
            ],
            "masterdata invalid: <masterdata-root>: cannot read root: ",
        ),
    ];
    for (args, category) in cases {
        let stderr = fails(&run(args, cwd), &sb, &[]);
        assert!(stderr.starts_with(category), "{category}: {stderr}");
    }
    assert!(!ws.exists(), "no workspace for a missing snapshot");
    assert!(!out.exists());

    // Export with a missing masterdata root.
    let snap = sb.copy(TESTGARDEN_12, "snap-9001019802.json");
    let stderr = fails(
        &run(export_args(&snap, &missing_root, &ws, &out), cwd),
        &sb,
        &[],
    );
    assert!(
        stderr.starts_with("masterdata invalid: <masterdata-root>: cannot read root: "),
        "{stderr}"
    );
}

// F2 — DuckDB's own error text (a non-database file, a directory) is scrubbed
// or withheld; the category survives.
#[test]
fn f2_library_errors_from_duckdb_never_carry_the_path() {
    let sb = Sandbox::new("f2");
    let cwd = Path::new(CRATE);
    let garbage = sb.write(
        "ws-9001019802-Kråksjö.duckdb",
        &b"this is not a duckdb database file at all".repeat(200),
    );
    let dir = sb.path("dir-9001019802.duckdb");
    fs::create_dir_all(&dir).expect("dir");
    for target in [&garbage, &dir] {
        let stderr = fails(
            &run(
                [
                    OsStr::new("inspect-workspace"),
                    OsStr::new("--workspace"),
                    target.as_os_str(),
                ],
                cwd,
            ),
            &sb,
            &[],
        );
        assert!(
            stderr.starts_with("duckdb: ")
                || stderr.starts_with("workspace is corrupt: ")
                || stderr.starts_with("the workspace "),
            "category kept: {stderr}"
        );
        eprintln!("F2 observed: {}", stderr.trim_end());
    }
    // ingest onto an existing (garbage) path is refused before DuckDB writes.
    let snap = sb.copy(TESTGARDEN_11, "snap-9001019802.json");
    let stderr = fails(
        &run(
            [
                OsStr::new("ingest"),
                OsStr::new("--snapshot"),
                snap.as_os_str(),
                OsStr::new("--workspace"),
                garbage.as_os_str(),
            ],
            cwd,
        ),
        &sb,
        &[],
    );
    assert!(stderr.contains("the workspace already exists"), "{stderr}");
}

// F3 — masterdata failures: the root is never shown; trusted tracked file names
// are; any other file name becomes a fixed role label.
#[test]
fn f3_masterdata_errors_show_only_trusted_file_names() {
    let sb = Sandbox::new("f3");
    let cwd = Path::new(CRATE);
    let inspect = |root: &Path| {
        run(
            [
                OsStr::new("inspect-masterdata"),
                OsStr::new("--root"),
                root.as_os_str(),
            ],
            cwd,
        )
    };

    // Empty root.
    let empty = sb.path("md-empty-9001019802");
    fs::create_dir_all(&empty).expect("dir");
    let stderr = fails(&inspect(&empty), &sb, &[]);
    assert!(
        stderr.starts_with("masterdata invalid: <masterdata-root>: no chart-"),
        "{stderr}"
    );

    // Trusted tracked name, broken content: the name is shown.
    let trusted = sb.masterdata("md-trusted-9001019802");
    fs::write(
        trusted.join("chart-synthetic-k1-2026.1.json"),
        b"{ not json",
    )
    .expect("break");
    let stderr = fails(&inspect(&trusted), &sb, &[]);
    assert!(
        stderr.starts_with("masterdata invalid: <masterdata-root>/chart-synthetic-k1-2026.1.json"),
        "{stderr}"
    );

    // Untrusted name, broken content: only the role label.
    let untrusted = sb.masterdata("md-untrusted-9001019802");
    fs::remove_file(untrusted.join("chart-synthetic-k1-2026.1.json")).expect("rm");
    fs::write(
        untrusted.join("chart-9001019802-Kråksjö.json"),
        b"{ not json",
    )
    .expect("w");
    let stderr = fails(&inspect(&untrusted), &sb, &[]);
    assert!(
        stderr.starts_with("masterdata invalid: <masterdata-root>/<masterdata chart file>"),
        "{stderr}"
    );

    // Untrusted name, valid content: loads, and nothing about the name is printed.
    let renamed = sb.masterdata("md-renamed-9001019802");
    fs::rename(
        renamed.join("chart-synthetic-k1-2026.1.json"),
        renamed.join("chart-9001019802.json"),
    )
    .expect("rename");
    let stdout = ok(&inspect(&renamed), &sb);
    assert!(stdout.contains("Chart:"), "{stdout}");

    // export-sie with a broken root reports the same way.
    let snap = sb.copy(TESTGARDEN_12, "snap-9001019802.json");
    let ws = sb.path("ws-9001019802.duckdb");
    let out = sb.path("out-9001019802.si");
    let stderr = fails(
        &run(export_args(&snap, &untrusted, &ws, &out), cwd),
        &sb,
        &[],
    );
    assert!(
        stderr.starts_with("masterdata invalid: <masterdata-root>/<masterdata chart file>"),
        "{stderr}"
    );
}

// F4 — argument errors never echo a value (paths, identifiers, option=value).
#[test]
fn f4_argument_errors_never_echo_values() {
    let sb = Sandbox::new("f4");
    let cwd = Path::new(CRATE);
    let snap = sb.copy(TESTGARDEN_12, "snap-9001019802.json");
    let md = sb.masterdata("md-9001019802");
    let ws = sb.path("ws-9001019802.duckdb");
    let out = sb.path("out-9001019802.si");
    let stray = sb.path("stray-9001019802.json");

    let mut positional = vec![
        OsStr::new("ingest"),
        OsStr::new("--snapshot"),
        snap.as_os_str(),
        OsStr::new("--workspace"),
        ws.as_os_str(),
    ];
    positional.push(stray.as_os_str());
    let stderr = fails(&run(positional, cwd), &sb, &[]);
    assert!(
        stderr.starts_with("unexpected positional argument\n"),
        "{stderr}"
    );

    let mut bad_date = export_args(&snap, &md, &ws, &out);
    let last = bad_date.len() - 1;
    bad_date[last] = stray.as_os_str();
    let stderr = fails(&run(bad_date, cwd), &sb, &[]);
    assert!(
        stderr.starts_with("--generated-on is not YYYY-MM-DD\n"),
        "{stderr}"
    );

    let mut with_value = export_args(&snap, &md, &ws, &out);
    with_value.push(OsStr::new("--org-number=900101-9802"));
    let stderr = fails(&run(with_value, cwd), &sb, &[]);
    assert!(stderr.starts_with("unknown option\n"), "{stderr}");

    let mut named = export_args(&snap, &md, &ws, &out);
    named.extend([OsStr::new("--company-name"), OsStr::new("Testgården")]);
    let stderr = fails(&run(named, cwd), &sb, &[]);
    assert!(
        stderr.starts_with("unknown option --company-name\n"),
        "{stderr}"
    );

    for args in [
        vec![
            OsStr::new("inspect-masterdata"),
            OsStr::new("--root"),
            md.as_os_str(),
            stray.as_os_str(),
        ],
        vec![OsStr::new("frobnicate"), stray.as_os_str()],
    ] {
        let stderr = fails(&run(args, cwd), &sb, &[]);
        assert!(stderr.starts_with("usage: sieverk"), "{stderr}");
    }
    assert!(!ws.exists() && !out.exists());
}

// F5 — relative paths (cwd = sandbox): the same export succeeds with the same
// bytes, and relative-path failures leak nothing either.
#[test]
fn f5_relative_paths_succeed_and_fail_without_leaks() {
    let sb = Sandbox::new("f5");
    sb.copy(TESTGARDEN_12, "snap-9001019802.json");
    sb.masterdata("md-9001019802");
    let rel = |s: &str| PathBuf::from(s);
    let (snap, md, ws, out) = (
        rel("snap-9001019802.json"),
        rel("md-9001019802"),
        rel("ws-9001019802.duckdb"),
        rel("NY9001019802.si"),
    );
    let stdout = ok(&run(export_args(&snap, &md, &ws, &out), &sb.dir), &sb);
    assert!(
        stdout.contains(&format!("si_sha256={CASE_B_SHA256}\n")),
        "{stdout}"
    );

    let stderr = fails(
        &run(["inspect-sie", "missing-9001019802.si"], &sb.dir),
        &sb,
        &[],
    );
    assert!(
        stderr.starts_with("could not read the SIE file: "),
        "{stderr}"
    );

    sb.write("rel-9001019802.duckdb", b"not a database");
    let stderr = fails(
        &run(
            ["inspect-workspace", "--workspace", "rel-9001019802.duckdb"],
            &sb.dir,
        ),
        &sb,
        &[],
    );
    eprintln!("F5 observed: {}", stderr.trim_end());
}

// F6 — a snapshot whose identity or content is invalid fails without echoing
// the value: our own "got …" messages and serde's "invalid type: string …".
type Mutate = fn(&mut serde_json::Value);

#[test]
fn f6_invalid_identity_values_are_never_echoed() {
    let sb = Sandbox::new("f6");
    let cwd = Path::new(CRATE);
    let md = sb.masterdata("md-9001019802");
    let base: serde_json::Value =
        serde_json::from_slice(&fs::read(repo(TESTGARDEN_12)).expect("fixture")).expect("json");
    let variants: [(&str, Mutate, [&str; 2]); 4] = [
        (
            "bad-luhn",
            |v| v["entity"]["accounting_identity"]["business_identifier"] = "900101-9803".into(),
            ["900101-9803", "9001019803"],
        ),
        (
            "bad-name",
            |v| v["entity"]["accounting_identity"]["legal_name"] = "Smedjebacken Ägare\u{7}".into(),
            ["Smedjebacken", "Ägare"],
        ),
        (
            "amount-text",
            |v| v["receipts"][0]["total_amount"] = "Erik Svensson".into(),
            ["Erik", "Svensson"],
        ),
        (
            "type-error",
            |v| v["receipts"][0]["id"] = "Anna Andersson".into(),
            ["Anna", "Andersson"],
        ),
    ];
    for (tag, mutate, extra) in variants {
        let mut v = base.clone();
        mutate(&mut v);
        let snap = sb.write(
            &format!("snap-{tag}-9001019802.json"),
            &serde_json::to_vec(&v).expect("json"),
        );
        let stderr = fails(
            &run([OsStr::new("inspect-snapshot"), snap.as_os_str()], cwd),
            &sb,
            &extra,
        );
        assert!(stderr.starts_with("snapshot rejected: "), "{stderr}");
        let ws = sb.path(&format!("ws-{tag}-9001019802.duckdb"));
        let out = sb.path(&format!("out-{tag}-9001019802.si"));
        let stderr = fails(&run(export_args(&snap, &md, &ws, &out), cwd), &sb, &extra);
        assert!(stderr.starts_with("ingest failed: "), "{stderr}");
        assert!(!out.exists());
    }
}

/// A malformed SIE file whose broken fields carry names and an identifier.
/// CP437 bytes: 0x86 = å, 0x94 = ö (so "Kr\x86ksj\x94" decodes to Kråksjö).
fn malformed_sie() -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in [
        &b"#FLAGGA 0"[..],
        b"#PROGRAM \"SIEverk\" \"0.4.0\"",
        b"#FORMAT PC8",
        b"#GEN 20260910",
        b"#SIETYP 4",
        b"#ORGNR 900101-9802",
        b"#FNAMN \"Erik Andersson\"",
        b"#RAR 0 Erik Svensson",
        b"#RAR x Anna 900101-9802",
        b"#VALUTA Erik",
        b"#KONTO 1930 \"Bank\"",
        b"#KONTO 5360 \"Drivmedel\"",
        b"#VER \"\" \"1\" Erik \"Anna Andersson\"",
        b"{",
        b"#TRANS 5360 {} Andersson",
        b"#TRANS Erik {} 100.00",
        b"#TRANS 1930 {} -100.00",
        b"}",
        b"#VER \"B\" \"3\" 20260101 \"Erik Svensson\"",
        b"{",
        b"#TRANS 1930 {} 50.00",
        b"}",
        b"#VER \"ERIK\" \"2\" 20260602 \"Kr\x86ksj\x94 900101-9802\"",
        b"#VER broken Erik",
        b"{",
        b"}",
    ] {
        bytes.extend_from_slice(line);
        bytes.extend_from_slice(b"\r\n");
    }
    bytes
}

const NAMES: [&str; 5] = ["Erik", "Anna", "Andersson", "Svensson", "ERIK"];

// F7 — inspect-sie and validate-sie on a malformed file: findings and parser
// notes keep their codes and counts, never the names, the identifier or the
// raw SIE line.
#[test]
fn f7_malformed_sie_lines_never_reach_the_output() {
    let sb = Sandbox::new("f7");
    let cwd = Path::new(CRATE);
    let si = sb.write("NY9001019802-broken.si", &malformed_sie());

    let stdout = ok(&run([OsStr::new("inspect-sie"), si.as_os_str()], cwd), &sb);
    let (stdout, _) = clean_with(&stdout, &sb, &NAMES);
    for line in [
        "Company:     [REDACTED]\n",
        "Org number:  [REDACTED]\n",
        "Fiscal year: [REDACTED] – [REDACTED] (index 0)\n",
        "Currency:    [REDACTED]\n",
        "Vouchers:    3\n",
        "Findings:    1 errors, 4 warnings\n",
        "Status:      Invalid\n",
    ] {
        assert!(stdout.contains(line), "{line}: {stdout}");
    }

    let out = run([OsStr::new("validate-sie"), si.as_os_str()], cwd);
    let (stdout, stderr) = clean(&out, &sb, &NAMES);
    assert!(
        !out.status.success(),
        "an error keeps exit code 1: {stdout}"
    );
    assert!(stderr.is_empty(), "{stderr}");
    for expected in [
        "ERROR   VOUCHER_NOT_BALANCED: B-3 sums to 50.00\n",
        "WARNING VOUCHER_DATE_MALFORMED: -1 has a malformed date '[REDACTED]'\n",
        "WARNING TRANS_ACCOUNT_NOT_DECLARED: -1 uses account [REDACTED] which has no #KONTO row\n",
        "WARNING VOUCHER_DATE_OUTSIDE_YEAR: [REDACTED] is dated 20260602 outside fiscal year [REDACTED] – [REDACTED]\n",
        "WARNING VOUCHER_DATE_OUTSIDE_YEAR: B-3 is dated 20260101 outside fiscal year [REDACTED] – [REDACTED]\n",
        "PARSER  #RAR row is malformed\n",
        "PARSER  #TRANS amount '[REDACTED]' is not a valid amount\n",
        "PARSER  #VER [REDACTED] was never closed before the next #VER\n",
        "PARSER  #VER header is malformed\n",
        "PARSER  unexpected '{' with no #VER header before it\n",
        "Errors:      1  Warnings: 4\n",
        "Status:      Invalid\n",
    ] {
        assert!(stdout.contains(expected), "{expected:?}\n{stdout}");
    }
    assert!(!stdout.contains("broken"), "raw SIE line leaked: {stdout}");
    assert!(!stdout.contains("{}"), "raw #TRANS line leaked: {stdout}");
}

/// `clean` for text already captured (the success path returns stdout only).
fn clean_with(stdout: &str, sb: &Sandbox, extra: &[&str]) -> (String, String) {
    for needle in SECRETS
        .iter()
        .copied()
        .chain(extra.iter().copied())
        .chain(sb.forms().iter().map(String::as_str))
    {
        assert!(!stdout.contains(needle), "{needle:?} leaked\n{stdout}");
    }
    (stdout.to_string(), String::new())
}

fn edit_json(path: &Path, edit: impl FnOnce(&mut serde_json::Value)) {
    let mut v: serde_json::Value =
        serde_json::from_slice(&fs::read(path).expect("read json")).expect("json");
    edit(&mut v);
    fs::write(path, serde_json::to_vec(&v).expect("json")).expect("write json");
}

// F8 — masterdata free text (profile scope, workbook name) is never printed,
// and values echoed by masterdata errors (ours and serde's) are redacted.
#[test]
fn f8_masterdata_free_text_and_echoed_values_are_redacted() {
    let sb = Sandbox::new("f8");
    let cwd = Path::new(CRATE);
    let inspect = |root: &Path| {
        run(
            [
                OsStr::new("inspect-masterdata"),
                OsStr::new("--root"),
                root.as_os_str(),
            ],
            cwd,
        )
    };
    let chart = "chart-synthetic-k1-2026.1.json";

    let md = sb.masterdata("md-free-9001019802");
    edit_json(&md.join(chart), |c| {
        c["profile_scope"] = "Erik Svensson 900101-9802".into();
        c["_header"]["workbook"] = "Anna-Andersson.xlsx".into();
    });
    let out = inspect(&md);
    let (stdout, stderr) = clean(&out, &sb, &NAMES);
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(
        stdout.contains("Profile:       (free text withheld)\n"),
        "{stdout}"
    );
    assert!(
        stdout.contains("Workbook:      (free text withheld) sha256 "),
        "{stdout}"
    );
    assert!(
        stdout.contains("Chart:         SYNTHETIC_K1 2026.1 (draft)\n"),
        "{stdout}"
    );

    let md = sb.masterdata("md-status-9001019802");
    edit_json(&md.join(chart), |c| {
        c["review_status"] = "Erik Svensson".into()
    });
    let stderr = fails(&inspect(&md), &sb, &NAMES);
    assert!(
        stderr.contains("#review_status: must be draft or approved, got \"[REDACTED]\""),
        "{stderr}"
    );

    let md = sb.masterdata("md-type-9001019802");
    edit_json(&md.join(chart), |c| {
        c["accounts"][0]["must_include"] = "Anna Andersson".into()
    });
    let stderr = fails(&inspect(&md), &sb, &NAMES);
    assert!(
        stderr.contains("invalid type: string \"[REDACTED]\""),
        "{stderr}"
    );

    // A name inside a ruleset case id: whatever the outcome, nothing leaks.
    let md = sb.masterdata("md-case-9001019802");
    edit_json(&md.join("ruleset-2026.1.json"), |r| {
        r["cases"][0]["case_id"] = "Erik Svensson".into()
    });
    let out = inspect(&md);
    let _ = clean(&out, &sb, &NAMES);
}

/// `word` occurs in `text` as a whole word (ASCII alphanumeric boundaries).
fn has_word(text: &str, word: &str) -> bool {
    let b = text.as_bytes();
    let mut from = 0;
    while let Some(pos) = text[from..].find(word) {
        let at = from + pos;
        let end = at + word.len();
        let left = at == 0 || !b[at - 1].is_ascii_alphanumeric();
        let right = end == b.len() || !b[end].is_ascii_alphanumeric();
        if left && right {
            return true;
        }
        from = at + 1;
    }
    false
}

fn assert_no_names(out: &Output, names: &[&str]) {
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    for name in names {
        assert!(
            !has_word(&stdout, name) && !has_word(&stderr, name),
            "{name:?} leaked\n--- stdout\n{stdout}\n--- stderr\n{stderr}"
        );
    }
}

/// Voucher series that are names: lowercase, mixed case and two-letter upper.
fn named_series_sie() -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in [
        "#FLAGGA 0",
        "#SIETYP 4",
        "#RAR 0 20260101 20261231",
        "#KONTO 1930 \"Bank\"",
        "#VER \"anna\" \"1\" 20260101 \"x\"",
        "{",
        "#TRANS 1930 {} 50.00",
        "}",
        "#VER \"erik\" \"2\" 20260101 \"x\"",
        "#VER \"Anna\" \"3\" 20260101 \"x\"",
        "{",
        "#TRANS 1930 {} 10.00",
        "}",
        "#VER \"BO\" \"4\" 20260101 \"x\"",
        "{",
        "#TRANS 1930 {} 10.00",
        "}",
        "#VER \"lars\" \"5\" 2026x101 \"x\"",
        "{",
        "#TRANS 1930 {} 10.00",
        "#TRANS 1930 {} -10.00",
        "}",
    ] {
        bytes.extend_from_slice(line.as_bytes());
        bytes.extend_from_slice(b"\r\n");
    }
    bytes
}

const SERIES_NAMES: [&str; 6] = ["anna", "erik", "Anna", "BO", "lars", "ANNA"];

// F9 — HY-SIE-01 REV3: user-controlled names in lowercase, mixed case and
// upper case never survive as "structural" words: voucher labels in findings
// and parser notes, masterdata element references (case ids, now shown as
// `cases/[REDACTED]`) and chart ids. Codes, counts and exit codes are unchanged.
#[test]
fn f9_names_in_any_case_never_pass_as_structure() {
    let sb = Sandbox::new("f9");
    let cwd = Path::new(CRATE);
    let si = sb.write("NY9001019802-series.si", &named_series_sie());

    let out = run([OsStr::new("validate-sie"), si.as_os_str()], cwd);
    let (stdout, stderr) = clean(&out, &sb, &[]);
    assert_no_names(&out, &SERIES_NAMES);
    assert!(!out.status.success(), "errors keep exit code 1: {stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    for expected in [
        "ERROR   VOUCHER_NOT_BALANCED: [REDACTED] sums to 50.00\n",
        "ERROR   VOUCHER_NOT_BALANCED: [REDACTED] sums to 10.00\n",
        "WARNING VOUCHER_DATE_MALFORMED: [REDACTED] has a malformed date '[REDACTED]'\n",
        "PARSER  #VER [REDACTED] was never closed before the next #VER\n",
        "Errors:      3  Warnings: 1\n",
        "Status:      Invalid\n",
    ] {
        assert!(stdout.contains(expected), "{expected:?}\n{stdout}");
    }
    assert_eq!(
        stdout.matches("VOUCHER_NOT_BALANCED").count(),
        3,
        "{stdout}"
    );

    let out = run([OsStr::new("inspect-sie"), si.as_os_str()], cwd);
    let (stdout, _) = clean(&out, &sb, &[]);
    assert_no_names(&out, &SERIES_NAMES);
    assert!(out.status.success());
    assert!(
        stdout.contains("Findings:    3 errors, 1 warnings\n"),
        "{stdout}"
    );

    let inspect = |root: &Path| {
        run(
            [
                OsStr::new("inspect-masterdata"),
                OsStr::new("--root"),
                root.as_os_str(),
            ],
            cwd,
        )
    };
    // A case id that is a name, in an error that carries the element path.
    let md = sb.masterdata("md-case-9001019802");
    edit_json(&md.join("ruleset-2026.1.json"), |r| {
        r["cases"][0]["case_id"] = "anna".into();
        r["cases"][0]["automation"] = "nonsense".into();
    });
    let out = inspect(&md);
    let (_, stderr) = clean(&out, &sb, &[]);
    assert_no_names(&out, &SERIES_NAMES);
    assert!(!out.status.success());
    assert!(
        stderr.contains("ruleset-2026.1.json#cases/[REDACTED]: unknown automation"),
        "{stderr}"
    );

    // A chart id that is a name: never shown, whatever the outcome.
    let md = sb.masterdata("md-chart-9001019802");
    edit_json(&md.join("chart-synthetic-k1-2026.1.json"), |c| {
        c["chart_id"] = "ANNA".into();
    });
    let out = inspect(&md);
    let (stdout, _) = clean(&out, &sb, &[]);
    assert_no_names(&out, &SERIES_NAMES);
    if out.status.success() {
        assert!(stdout.contains("Chart:         [REDACTED] "), "{stdout}");
    }
}

/// Voucher series that are ordinary English words — the REV3 gap: `may-1`
/// passed as vocabulary structure although the label rule rejected it.
fn english_word_series_sie() -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in [
        "#FLAGGA 0",
        "#SIETYP 4",
        "#RAR 0 20260101 20261231",
        "#KONTO 1930 \"Bank\"",
        "#VER \"may\" \"1\" 20260101 \"Test\"",
        "{",
        "#TRANS 1930 {} 50.00",
        "}",
        "#VER \"mine\" \"1\" 20260101 \"Test\"",
        "#VER \"may\" \"2\" 2026x101 \"Test\"",
        "{",
        "#TRANS 1930 {} 10.00",
        "#TRANS 1930 {} -10.00",
        "}",
        "#VER \"case\" \"3\" 20260101 \"Test\"",
        "{",
        "#TRANS 1930 {} 10.00",
        "}",
        "#VER \"A\" \"9\" 20260101 \"Test\"",
        "{",
        "#TRANS 1930 {} 7.00",
        "}",
    ] {
        bytes.extend_from_slice(line.as_bytes());
        bytes.extend_from_slice(b"\r\n");
    }
    bytes
}

/// The ids inserted below (none of them occurs in any fixed message text).
const ENGLISH_WORD_IDS: [&str; 3] = ["may", "mine", "case"];

// F10 — HY-SIE-01 REV4: a dynamic id is never shown because it happens to be
// an English word. Voucher series `may`/`mine`/`case`, a masterdata case id
// `may`, a VAT rule id `mine` and a case account `may` are all redacted; the
// one-letter series `A` and every code, count and exit code are unchanged.
#[test]
fn f10_dynamic_ids_that_are_english_words_are_redacted() {
    let sb = Sandbox::new("f10");
    let cwd = Path::new(CRATE);
    let si = sb.write("NY9001019802-words.si", &english_word_series_sie());

    let out = run([OsStr::new("validate-sie"), si.as_os_str()], cwd);
    let (stdout, stderr) = clean(&out, &sb, &[]);
    assert_no_names(&out, &ENGLISH_WORD_IDS);
    assert!(!out.status.success(), "errors keep exit code 1: {stdout}");
    assert!(stderr.is_empty(), "{stderr}");
    for expected in [
        "ERROR   VOUCHER_NOT_BALANCED: [REDACTED] sums to 50.00\n",
        "WARNING VOUCHER_DATE_MALFORMED: [REDACTED] has a malformed date '[REDACTED]'\n",
        "ERROR   VOUCHER_NOT_BALANCED: [REDACTED] sums to 10.00\n",
        "ERROR   VOUCHER_NOT_BALANCED: A-9 sums to 7.00\n",
        "PARSER  #VER [REDACTED] was never closed before the next #VER\n",
        "Errors:      3  Warnings: 1\n",
        "Status:      Invalid\n",
    ] {
        assert!(stdout.contains(expected), "{expected:?}\n{stdout}");
    }
    assert!(
        !stdout.contains("may-") && !stdout.contains("mine-"),
        "{stdout}"
    );

    let out = run([OsStr::new("inspect-sie"), si.as_os_str()], cwd);
    let (stdout, _) = clean(&out, &sb, &[]);
    assert_no_names(&out, &ENGLISH_WORD_IDS);
    assert!(out.status.success());
    assert!(
        stdout.contains("Findings:    3 errors, 1 warnings\n"),
        "{stdout}"
    );

    let inspect = |root: &Path| {
        run(
            [
                OsStr::new("inspect-masterdata"),
                OsStr::new("--root"),
                root.as_os_str(),
            ],
            cwd,
        )
    };
    // A case id that is an English word: `cases/may` -> `cases/[REDACTED]`.
    let md = sb.masterdata("md-case-9001019802");
    edit_json(&md.join("ruleset-2026.1.json"), |r| {
        r["cases"][0]["case_id"] = "may".into();
        r["cases"][0]["automation"] = "nonsense".into();
    });
    let out = inspect(&md);
    let (_, stderr) = clean(&out, &sb, &[]);
    assert_no_names(&out, &ENGLISH_WORD_IDS);
    assert!(!out.status.success());
    assert!(
        stderr.contains("ruleset-2026.1.json#cases/[REDACTED]: unknown automation"),
        "{stderr}"
    );

    // A VAT rule id that is an English word: `rules/mine` -> `rules/[REDACTED]`.
    let md = sb.masterdata("md-vat-9001019802");
    edit_json(&md.join("vat-rules-2026.1.json"), |v| {
        v["rules"][0]["id"] = "mine".into();
        v["rules"][1]["id"] = "mine".into();
    });
    let out = inspect(&md);
    let (_, stderr) = clean(&out, &sb, &[]);
    assert_no_names(&out, &ENGLISH_WORD_IDS);
    assert!(!out.status.success());
    assert!(
        stderr.contains("vat-rules-2026.1.json#rules/[REDACTED]: duplicate vat rule id"),
        "{stderr}"
    );

    // An account value that is an English word, inside the message text.
    let md = sb.masterdata("md-account-9001019802");
    edit_json(&md.join("ruleset-2026.1.json"), |r| {
        let cases = r["cases"].as_array_mut().expect("cases");
        let case = cases
            .iter_mut()
            .find(|c| c["account"].is_string())
            .expect("a case with an account");
        case["account"] = "may".into();
    });
    let out = inspect(&md);
    let (_, stderr) = clean(&out, &sb, &[]);
    assert_no_names(&out, &ENGLISH_WORD_IDS);
    assert!(!out.status.success());
    assert!(
        stderr.contains(": account [REDACTED] is not in the chart"),
        "{stderr}"
    );
}

/// A minimal valid SIE file whose `#VALUTA` is `currency`.
fn currency_sie(currency: &str) -> Vec<u8> {
    let mut bytes = Vec::new();
    for line in [
        "#FLAGGA 0".to_string(),
        "#SIETYP 4".to_string(),
        "#RAR 0 20260101 20261231".to_string(),
        format!("#VALUTA {currency}"),
        "#KONTO 1930 \"Bank\"".to_string(),
    ] {
        bytes.extend_from_slice(line.as_bytes());
        bytes.extend_from_slice(b"\r\n");
    }
    bytes
}

// F11 — HY-SIE-01: `#VALUTA` is file text. A three-letter uppercase word that
// is a name (`EVA`) is never shown as the currency; an approved code (`SEK`)
// still is. Exit codes are unchanged.
#[test]
fn f11_currency_shows_only_approved_codes() {
    let sb = Sandbox::new("f11");
    let cwd = Path::new(CRATE);

    let si = sb.write("NY9001019802-currency-name.si", &currency_sie("EVA"));
    let out = run([OsStr::new("inspect-sie"), si.as_os_str()], cwd);
    let (stdout, stderr) = clean(&out, &sb, &[]);
    assert_no_names(&out, &["EVA"]);
    assert!(out.status.success(), "{stdout}{stderr}");
    assert!(stdout.contains("Currency:    [REDACTED]\n"), "{stdout}");
    assert!(stdout.contains("Status:      Valid\n"), "{stdout}");

    let si = sb.write("NY9001019802-currency-sek.si", &currency_sie("SEK"));
    let out = run([OsStr::new("inspect-sie"), si.as_os_str()], cwd);
    let (stdout, _) = clean(&out, &sb, &[]);
    assert!(out.status.success(), "{stdout}");
    assert!(stdout.contains("Currency:    SEK\n"), "{stdout}");
}
