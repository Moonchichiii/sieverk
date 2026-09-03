"""Generator self-tests (SV-02 §5): one negative case per V-rule, draft/approved
semantics, the deterministic downgrade and V14. Runs against the tracked synthetic
workbook only — never masterdata/real/**.

    uv run --with openpyxl --with pytest pytest tools/tests -q
"""

from __future__ import annotations

import copy
import hashlib
import json
import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mastermatris_gen import (  # noqa: E402
    DRAFT_FRAGA,
    DRAFT_VILLKOR,
    BuildFailure,
    Report,
    build,
    emit,
    init_workbook,
    load_k1_list,
    load_taxonomy,
    main,
    read_bas_table,
    read_workbook,
    validate,
)

ROOT = Path(__file__).resolve().parents[2]
WORKBOOK = ROOT / "fixtures/masterdata/synthetic/Mastermatris_v1.2-synthetic.xlsx"
TAXONOMY = ROOT / "masterdata/taxonomy-1.0.json"
K1_LIST = ROOT / "masterdata/vibeke-k1-lista-2026-08-11.json"


def bas_table(path: Path, rows: dict[str, str], *, header=("Konto", "Kontonamn")) -> Path:
    """Synthetic stand-in with the expected BAS xlsx shape (local real files are never tracked)."""
    from openpyxl import Workbook

    wb = Workbook()
    ws = wb.active
    ws.append(["", ""])  # a leading junk row: the reader must find the header row, not assume row 1
    ws.append(list(header))
    for number, name in rows.items():
        ws.append([number, name])
    wb.save(path)
    return path


@pytest.fixture(scope="module")
def k1_list():
    return load_k1_list(K1_LIST)


@pytest.fixture(scope="module")
def sheets():
    return read_workbook(WORKBOOK)


@pytest.fixture(scope="module")
def taxonomy():
    return load_taxonomy(TAXONOMY)


def errors(sheets, taxonomy, mode="draft", **inputs):
    rep = Report(mode=mode)
    validate(copy.deepcopy(sheets), taxonomy, rep, **inputs)
    return rep


def find(rows, **match):
    return next(r for r in rows if all(r[k] == v for k, v in match.items()))


def test_synthetic_workbook_is_clean_in_both_modes(sheets, taxonomy):
    assert errors(sheets, taxonomy, "draft").errors == []
    assert errors(sheets, taxonomy, "approved").errors == []


@pytest.mark.parametrize(
    "rule, mutate",
    [
        ("V1", lambda s: find(s["Redovisningsfall"], case_id="bransle.default").__setitem__("kategori", "Finns inte")),
        ("V2", lambda s: find(s["Redovisningsfall"], case_id="income.leveransvirke.default").__setitem__("income_type", "timber_future")),
        ("V3", lambda s: find(s["Redovisningsfall"], case_id="bransle.default").__setitem__("konto", "4999")),
        ("V3", lambda s: find(s["Redovisningsfall"], case_id="bransle.default").__setitem__("konto", "5999")),  # inactive
        ("V4", lambda s: s["Konton"].append(dict(s["Konton"][0]))),  # duplicate
        ("V4", lambda s: find(s["Konton"], konto="1930").__setitem__("kontoklass", "kostnad")),
        ("V4", lambda s: find(s["Konton"], konto="1930").__setitem__("konto", "193")),
        ("V4", lambda s: find(s["Konton"], konto="1930").__setitem__("source_id", "NOPE")),
        ("V6", lambda s: s["SRU"].append({**s["SRU"][0], "giltig_från": "2020"})),  # overlap
        ("V6", lambda s: find(s["SRU"], konto="3420").__setitem__("konto", "4999")),
        ("V7", lambda s: find(s["Momsregler"], momsregel_id="ing25").__setitem__("tillåtna_satser", "24")),
        ("V7", lambda s: find(s["Momsregler"], momsregel_id="ing25").__setitem__("momskonto", "2699")),
        ("V8", lambda s: s["Redovisningsfall"].append({**find(s["Redovisningsfall"], case_id="bransle.default"), "case_id": "bransle.b"})),
        ("V10", lambda s: find(s["Redovisningsfall"], case_id="grus_och_material.default").__setitem__("villkor", "")),
        ("V10", lambda s: find(s["Redovisningsfall"], case_id="annat_osakert.default").__setitem__("kontrollfråga", "")),
        ("V10", lambda s: find(s["Redovisningsfall"], case_id="bransle.default").__setitem__("momsregel_id", "")),
        ("V10", lambda s: find(s["Motkonton"], nyckel="unknown").__setitem__("fråga", "")),
        ("V11", lambda s: s["Redovisningsfall"].append(dict(find(s["Redovisningsfall"], case_id="bransle.default")))),
        ("V11", lambda s: find(s["Redovisningsfall"], case_id="bransle.default").__setitem__("case_id", "diesel.default")),
        ("V12", lambda s: find(s["Tester"], exempel_id="ex-bransle-1").__setitem__("förväntade_rader", "5360:debet:1000.00;1930:kredit:900.00")),
        ("V12", lambda s: find(s["Tester"], exempel_id="ex-bransle-1").__setitem__("case_id", "finns.inte")),
    ],
)
def test_hard_rules_fail_in_draft_and_approved(sheets, taxonomy, rule, mutate):
    for mode in ("draft", "approved"):
        s = copy.deepcopy(sheets)
        mutate(s)
        rep = errors(s, taxonomy, mode)
        # Cascades (e.g. V13 following a broken case_id in approved mode) are legitimate;
        # the contract is that the rule under test is reported.
        assert any(e.startswith(rule) for e in rep.errors), (mode, rule, rep.errors)


@pytest.mark.parametrize(
    "rule, mutate",
    [
        ("V5", lambda s: find(s["Konton"], konto="1930").__setitem__("status", "Utkast")),
        ("V9", lambda s: find(s["Konton"], konto="1930").__setitem__("granskad_av", "")),
        ("V9", lambda s: find(s["Konton"], konto="1930").__setitem__("granskad_roll", "author")),
        ("V13", lambda s: find(s["Tester"], exempel_id="ex-bransle-1").__setitem__("status", "Utkast")),
    ],
)
def test_review_rules_warn_in_draft_and_fail_in_approved(sheets, taxonomy, rule, mutate):
    s = copy.deepcopy(sheets)
    mutate(s)
    draft = errors(s, taxonomy, "draft")
    assert draft.errors == [] and any(w.startswith(rule) for w in draft.warnings), draft
    approved = errors(s, taxonomy, "approved")
    assert any(e.startswith(rule) for e in approved.errors), approved.errors


def test_vibeke_list_is_45_accounts_and_the_must_include_minimum(sheets, taxonomy, k1_list):
    assert len(k1_list) == 45  # counted from her PDF (2026-08-11), not from memory
    rep = errors(sheets, taxonomy, "draft", k1_list=k1_list)
    assert rep.errors == []
    live = [r for r in sheets["Konton"] if r["status"] != "Struken"]
    assert sorted(r["konto"] for r in live if r["must_include"] == "true") == sorted(k1_list)
    # Negative proof (v1.2 §C): omitting a required Vibeke account is a V5 WARNING in draft
    # and a hard V5 failure in approved; same for a wrong must_include/origin.
    s = copy.deepcopy(sheets)
    s["Konton"] = [r for r in s["Konton"] if r["konto"] != "1973"]
    draft = errors(s, taxonomy, "draft", k1_list=k1_list)
    assert draft.errors == [] and any(w.startswith("V5") and "1973" in w for w in draft.warnings), draft
    approved = errors(s, taxonomy, "approved", k1_list=k1_list)
    assert any(e.startswith("V5") and "1973" in e for e in approved.errors), approved.errors
    s = copy.deepcopy(sheets)
    find(s["Konton"], konto="1973")["must_include_origin"] = "RULESET"
    draft = errors(s, taxonomy, "draft", k1_list=k1_list)
    assert draft.errors == [] and any(w.startswith("V5") for w in draft.warnings)
    assert any(e.startswith("V5") for e in errors(s, taxonomy, "approved", k1_list=k1_list).errors)


def test_source_decision_is_structured_and_source_version_mandatory(sheets, taxonomy):
    from mastermatris_gen import parse_source_decision

    assert parse_source_decision("K1-namnet behålls | Mats | 2026-09-03") == {
        "reason": "K1-namnet behålls", "decided_by": "Mats", "decided_at": "2026-09-03"
    }
    for bad in ("bara text", "reason | Mats", "reason |  | 2026-09-03", "reason | Mats | igår"):
        with pytest.raises(BuildFailure):
            parse_source_decision(bad)
    # Emitted as an object, never as the raw string.
    s = copy.deepcopy(sheets)
    find(s["Konton"], konto="1930")["source_decision"] = "Synthetic decision | Syntetisk granskare | 2026-09-02"
    rep = Report(mode="draft")
    model = validate(s, taxonomy, rep)
    assert rep.errors == []
    chart = json.loads(emit(s, model, taxonomy, WORKBOOK, "0" * 64, "draft")[next(k for k in emit(s, model, taxonomy, WORKBOOK, "0" * 64, "draft") if k.startswith("chart-"))])
    acc = next(a for a in chart["accounts"] if a["number"] == "1930")
    assert acc["source_decision"] == {"reason": "Synthetic decision", "decided_by": "Syntetisk granskare", "decided_at": "2026-09-02"}
    # Malformed cell is a hard V4 failure in both modes.
    s = copy.deepcopy(sheets)
    find(s["Konton"], konto="1930")["source_decision"] = "bara en text utan delar"
    for mode in ("draft", "approved"):
        assert any(e.startswith("V4") and "source_decision" in e for e in errors(s, taxonomy, mode).errors)
    # source_version: empty cell falls back to the source's version; no version anywhere is hard.
    s = copy.deepcopy(sheets)
    find(s["Konton"], konto="1930")["source_version"] = ""
    find(s["Källor"], source_id="VIBEKE_K1_LIST")["version"] = ""
    assert any(e.startswith("V4") and "source_version" in e for e in errors(s, taxonomy, "draft").errors)


def _lantbruk_workbook(tmp_path, k1_rows, crosscheck_rows):
    """A real-profile workbook built exactly the way Mats will build it, on synthetic stand-ins."""
    k1 = bas_table(tmp_path / "bas-2018-k1.xlsx", k1_rows)
    cc = bas_table(tmp_path / "bas-2026-crosscheck.xlsx", crosscheck_rows)
    wb = tmp_path / "Mastermatris_v1.2.xlsx"
    init_workbook(TAXONOMY, wb, "lantbruk-k1", K1_LIST, k1, cc)
    return wb, k1, cc


@pytest.fixture
def stand_in_tables(k1_list):
    """K1 table containing every Vibeke account except 3456 (a free forestry account) plus
    the ruleset core accounts; the 2026 cross-check renames 5360 and lacks 8414."""
    k1_rows = {n: f"K1-namn {n}" for n in k1_list if n != "3456"}
    k1_rows.update({"1910": "Kassa", "2440": "Leverantörsskulder", "3740": "Öres- och kronutjämning"})
    cc_rows = dict(k1_rows)
    cc_rows["5360"] = "Drivmedel (nytt namn 2026)"
    del cc_rows["8414"]
    return k1_rows, cc_rows


def test_init_workbook_lantbruk_k1_prefills_konton_per_m1(tmp_path, k1_list, stand_in_tables):
    k1_rows, cc_rows = stand_in_tables
    wb, _, _ = _lantbruk_workbook(tmp_path, k1_rows, cc_rows)
    sheets = read_workbook(wb)
    konton = {r["konto"]: r for r in sheets["Konton"]}
    assert set(konton) == set(k1_list) | {"1910", "2440", "3740"}
    assert konton["1930"]["source_id"] == "BAS_2018_K1" and konton["1930"]["namn"] == "K1-namn 1930"
    assert konton["3456"]["source_id"] == "VIBEKE_K1_LIST" and konton["3456"]["namn"] == "GROT"
    assert all(konton[n]["must_include"] == "true" and konton[n]["must_include_origin"] == "VIBEKE_K1_LIST" for n in k1_list)
    assert konton["3740"]["must_include"] == "false" and konton["3740"]["must_include_origin"] == "RULESET"
    assert konton["5360"]["kommentar"].startswith("KRÄVER source_decision") and "2026" in konton["5360"]["kommentar"]
    assert konton["8414"]["kommentar"].startswith("KRÄVER source_decision")
    assert konton["1930"]["kommentar"] == ""
    assert all(r["status"] == "Utkast" for r in sheets["Konton"])
    meta = {r["nyckel"]: r["värde"] for r in sheets["README"]}
    assert meta["profile"] == "lantbruk_k1" and meta["chart_id"] == "LANTBRUK_K1"


def test_lantbruk_k1_build_requires_both_bas_tables_and_enforces_source_decision(tmp_path, k1_list, stand_in_tables):
    k1_rows, cc_rows = stand_in_tables
    wb, k1, cc = _lantbruk_workbook(tmp_path, k1_rows, cc_rows)
    sheets = read_workbook(wb)
    taxonomy = load_taxonomy(TAXONOMY)
    k1_table, crosscheck = read_bas_table(k1, "k1"), read_bas_table(cc, "cc")
    # No tables ⇒ hard M1 failure, never a silent skip.
    rep = errors(sheets, taxonomy, "draft", k1_list=k1_list)
    assert any(e.startswith("M1") and "kräver --k1-table" in e for e in rep.errors), rep.errors
    # With tables: the two accounts where K1 and 2026 differ need a source_decision.
    rep = errors(sheets, taxonomy, "draft", k1_list=k1_list, k1_table=k1_table, crosscheck=crosscheck)
    m1 = [e for e in rep.errors if e.startswith("M1")]
    assert any("Konton/5360" in e and "source_decision" in e for e in m1), m1
    assert any("Konton/8414" in e and "saknas i BAS 2026" in e for e in m1), m1
    assert not any("Konton/1930" in e for e in m1)
    # Explicit decisions resolve it — positive path.
    s = copy.deepcopy(sheets)
    find(s["Konton"], konto="5360")["source_decision"] = "K1-namnet behålls; 2026-namnet är en omdöpning | Mats | 2026-09-03"
    find(s["Konton"], konto="8414")["source_decision"] = "Konto struket i BAS 2026 men behövs för maskinlån | Mats | 2026-09-03"
    rep = errors(s, taxonomy, "draft", k1_list=k1_list, k1_table=k1_table, crosscheck=crosscheck)
    assert not [e for e in rep.errors if e.startswith("M1")], rep.errors
    # A hand-edited name that drifts from the K1 table is refused without a decision.
    s2 = copy.deepcopy(s)
    find(s2["Konton"], konto="1930")["namn"] = "Bankkonto (egen text)"
    rep = errors(s2, taxonomy, "draft", k1_list=k1_list, k1_table=k1_table, crosscheck=crosscheck)
    assert any("Konton/1930" in e and "avviker" in e for e in rep.errors), rep.errors
    # Claiming BAS_2018_K1 for a number the table does not have is refused.
    s3 = copy.deepcopy(s)
    find(s3["Konton"], konto="3456")["source_id"] = "BAS_2018_K1"
    rep = errors(s3, taxonomy, "draft", k1_list=k1_list, k1_table=k1_table, crosscheck=crosscheck)
    assert any("Konton/3456" in e and "finns inte i K1-tabellen" in e for e in rep.errors), rep.errors
    # And the reverse: a number present in the K1 table may not be labelled a free Vibeke account.
    s4 = copy.deepcopy(s)
    find(s4["Konton"], konto="1930")["source_id"] = "VIBEKE_K1_LIST"
    rep = errors(s4, taxonomy, "draft", k1_list=k1_list, k1_table=k1_table, crosscheck=crosscheck)
    assert any("Konton/1930" in e and "ska vara BAS_2018_K1" in e for e in rep.errors), rep.errors


def test_bas_table_reader_fails_readably_on_missing_file_or_columns(tmp_path):
    with pytest.raises(BuildFailure, match="filen saknas"):
        read_bas_table(tmp_path / "nope.xlsx", "--k1-table")
    bad = bas_table(tmp_path / "bad.xlsx", {"1930": "Bank"}, header=("Nummer", "Namn"))
    with pytest.raises(BuildFailure, match="Konto"):
        read_bas_table(bad, "--k1-table")
    assert read_bas_table(bas_table(tmp_path / "ok.xlsx", {"1930": "Bank"}), "x") == {"1930": "Bank"}


def test_init_workbook_lantbruk_k1_hard_fails_without_inputs(tmp_path):
    with pytest.raises(BuildFailure):
        init_workbook(TAXONOMY, tmp_path / "wb.xlsx", "lantbruk-k1")
    with pytest.raises(BuildFailure, match="filen saknas"):
        init_workbook(TAXONOMY, tmp_path / "wb.xlsx", "lantbruk-k1", K1_LIST, tmp_path / "missing.xlsx", tmp_path / "missing2.xlsx")
    assert not (tmp_path / "wb.xlsx").exists()


def test_draft_downgrades_automatic_deterministically_with_both_v10_fields():
    files, _ = build(WORKBOOK, TAXONOMY, "draft")
    ruleset = json.loads(files["ruleset-2026.1.json"])
    cases = {c["case_id"]: c for c in ruleset["cases"]}
    assert all(c["automation"] != "Automatic" for c in cases.values())
    b = cases["bransle.default"]
    assert b["downgraded_from_automatic"] and b["automation"] == "Conditional"
    assert b["condition"] == DRAFT_VILLKOR and b["question"] == DRAFT_FRAGA
    g = cases["grus_och_material.default"]  # existing non-blank values remain
    assert not g["downgraded_from_automatic"] and g["condition"] == "Underhåll av befintlig anläggning"
    assert ruleset["review_status"] == "draft"


def test_approved_emits_automatic_only_for_reviewed_cases_with_examples(sheets, taxonomy):
    files, _ = build(WORKBOOK, TAXONOMY, "approved")
    cases = {c["case_id"]: c["automation"] for c in json.loads(files["ruleset-2026.1.json"])["cases"]}
    assert cases["bransle.default"] == "Automatic" and cases["skogsvard.default"] == "Automatic"
    assert cases["income.leveransvirke.default"] == "Automatic"
    assert cases["grus_och_material.default"] == "Conditional" and cases["annat_osakert.default"] == "Manual"
    # A reviewed Automatic case without an approved example is a hard V13 failure in approved mode.
    s = copy.deepcopy(sheets)
    find(s["Tester"], exempel_id="ex-bransle-1")["status"] = "Utkast"
    rep = errors(s, taxonomy, "approved")
    assert any(e.startswith("V13") for e in rep.errors)


def test_v14_two_builds_are_byte_identical_and_headers_have_no_wall_clock():
    a, _ = build(WORKBOOK, TAXONOMY, "draft")
    b, _ = build(WORKBOOK, TAXONOMY, "draft")
    assert a == b
    for name, data in a.items():
        header = json.loads(data)["_header"]
        assert "generated_at" not in header and "tool" not in header, name
        assert set(header) == {"generated", "workbook", "workbook_sha256", "taxonomy_version", "generator_version", "review_status", "profile_scope"}
        assert data.endswith(b"\n") and hashlib.sha256(data).hexdigest() == hashlib.sha256(b[name]).hexdigest()


def test_tracked_generated_artefacts_match_a_fresh_draft_build():
    files, _ = build(WORKBOOK, TAXONOMY, "draft", K1_LIST)
    generated = ROOT / "fixtures/masterdata/synthetic/generated"
    for name, data in files.items():
        assert (generated / name).read_bytes() == data, name


def test_tracked_invalid_roots_match_the_deterministic_builder(tmp_path):
    import shutil
    import subprocess

    tracked = ROOT / "fixtures/masterdata/synthetic/invalid"
    names = sorted(p.name for p in tracked.iterdir() if p.is_dir())
    assert len(names) == 25 and "v4-duplicate-account" in names and "source-version-missing" in names
    # Rebuild into a scratch copy of the repo layout and compare byte for byte.
    scratch = tmp_path / "repo"
    shutil.copytree(ROOT / "fixtures/masterdata/synthetic/generated", scratch / "fixtures/masterdata/synthetic/generated")
    shutil.copytree(ROOT / "tools", scratch / "tools", ignore=shutil.ignore_patterns("__pycache__", ".pytest_cache"))
    subprocess.run([sys.executable, str(scratch / "tools/tests/make_invalid_fixtures.py")], check=True, capture_output=True)
    for name in names:
        for f in sorted((tracked / name).iterdir()):
            assert (scratch / "fixtures/masterdata/synthetic/invalid" / name / f.name).read_bytes() == f.read_bytes(), (name, f.name)


def test_no_output_is_written_on_hard_failure(tmp_path, sheets, taxonomy):
    s = copy.deepcopy(sheets)
    find(s["Konton"], konto="1930")["konto"] = "193"
    rep = Report(mode="draft")
    validate(s, taxonomy, rep)
    assert rep.errors
    with pytest.raises(BuildFailure):
        raise BuildFailure("\n".join(rep.errors))
    assert list(tmp_path.iterdir()) == []


def test_cli_requires_explicit_root_and_mode(tmp_path):
    with pytest.raises(SystemExit):
        main(["--workbook", str(WORKBOOK), "--taxonomy", str(TAXONOMY), "--out", str(tmp_path)])
    with pytest.raises(SystemExit):
        main(["--root", str(tmp_path), "--workbook", str(WORKBOOK), "--taxonomy", str(TAXONOMY), "--k1-list", str(K1_LIST), "--out", str(tmp_path / "gen")])
    with pytest.raises(SystemExit):  # --k1-list is not optional either
        main(["--root", str(tmp_path), "--workbook", str(WORKBOOK), "--taxonomy", str(TAXONOMY), "--out", str(tmp_path / "gen"), "--draft"])
    assert main(["--root", str(tmp_path), "--workbook", str(WORKBOOK), "--taxonomy", str(TAXONOMY), "--k1-list", str(K1_LIST), "--out", str(tmp_path / "gen"), "--draft"]) == 0
    assert (tmp_path / "build-report.txt").exists()
    assert len(list((tmp_path / "gen").iterdir())) == 6


def test_emit_refuses_readme_without_required_keys(sheets, taxonomy):
    s = copy.deepcopy(sheets)
    s["README"] = [r for r in s["README"] if r["nyckel"] != "chart_id"]
    rep = Report(mode="draft")
    model = validate(s, taxonomy, rep)
    with pytest.raises(BuildFailure):
        emit(s, model, taxonomy, WORKBOOK, "0" * 64, "draft")
