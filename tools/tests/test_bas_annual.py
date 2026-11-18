"""Targeted, synthetic-only tests for the BAS annual review importer.

No real BAS workbook, customer file, Fly, B2 or SIEverk production state is used.
"""

from __future__ import annotations

import io
import json
import sys
from pathlib import Path
from types import SimpleNamespace

import pytest
from openpyxl import Workbook, load_workbook

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from bas_annual import (  # noqa: E402
    Entry,
    SourceError,
    _compile_entries,
    _comparison_rows,
    _normalised_xlsx,
    _read_vibeke,
    read_bas_crosscheck_xlsx,
    read_k1_xls,
)


def _annual_file(path: Path) -> Path:
    book = Workbook()
    ws = book.active
    ws.title = "Kontoplan 2026"
    ws["A1"] = "KONTOPLAN BAS 2026"
    ws["A2"] = "v 1.1"
    ws["A5"] = "Huvudkonton"
    ws["C5"] = "Underkonton"
    ws["A6"] = "10"
    ws["B6"] = "TILLGÅNGAR"
    ws["A7"] = "1110#"
    ws["B7"] = "Byggnader"
    ws["C7"] = "1111#"
    ws["D7"] = "Byggnader på egen mark"
    ws["C8"] = "1364"
    ws["D8"] = "Insatser skogsägarförening"
    ws["A9"] = "2087"
    ws["B9"] = "Bunden överkursfond"
    ws["A10"] = "2087"
    ws["B10"] = "Insatsemission"
    ws["A11"] = "1930"
    ws["B11"] = "Bank"
    ws["A12"] = "1930"
    ws["B12"] = "Bank"
    book.save(path)
    book.close()
    return path


def test_annual_four_column_format_preserves_conflict_and_k2_marker(tmp_path: Path) -> None:
    source = read_bas_crosscheck_xlsx(_annual_file(tmp_path / "BAS_2026.xlsx"), 2026)
    assert len(source.entries) == 7
    assert source.unique["1110"] == "Byggnader"
    assert source.unique["1111"] == "Byggnader på egen mark"
    assert source.unique["1364"] == "Insatser skogsägarförening"
    assert "2087" not in source.unique
    assert [e.name for e in source.conflicts["2087"]] == ["Bunden överkursfond", "Insatsemission"]
    assert source.k2_marked == ("1110", "1111")
    assert len(source.repeats["1930"]) == 2


def test_normalized_xlsx_contains_only_unambiguous_accounts(tmp_path: Path) -> None:
    source = read_bas_crosscheck_xlsx(_annual_file(tmp_path / "BAS_2026.xlsx"), 2026)
    contents = _normalised_xlsx(source)
    wb = load_workbook(io.BytesIO(contents), read_only=True)
    try:
        rows = list(wb.active.values)
        assert rows[0] == ("Konto", "Kontonamn")
        assert not any(row[0] == "2087" for row in rows)
        assert ("1110", "Byggnader") in rows
    finally:
        wb.close()


def test_annual_incorrect_title_fails_closed(tmp_path: Path) -> None:
    path = _annual_file(tmp_path / "BAS_2026.xlsx")
    book = load_workbook(path)
    book.active["A1"] = "SOMETHING ELSE"
    book.save(path)
    book.close()
    with pytest.raises(SourceError, match="title"):
        read_bas_crosscheck_xlsx(path, 2026)


def test_duplicate_with_conflicting_names_is_not_silently_chosen() -> None:
    source = _compile_entries([Entry("2087", "Bunden överkursfond", "A9"), Entry("2087", "Insatsemission", "A10")])
    assert source.unique == {}
    assert len(source.conflicts["2087"]) == 2
    assert all(e.location in ("A9", "A10") for e in source.conflicts["2087"])


def test_unknown_or_empty_account_name_fails_closed() -> None:
    with pytest.raises(SourceError, match="Invalid account"):
        _compile_entries([Entry("1930", "", "A7")])


def test_comparison_retains_vibeke_even_when_missing_in_both() -> None:
    v = {"1364": "Insatser skogsägarförening", "1110": "Byggnader"}
    k1 = _compile_entries([Entry("1110", "Byggnader", "B4")])
    annual = _compile_entries([Entry("1110", "Byggnader", "A4")])
    rows = _comparison_rows(v, k1, annual)
    assert rows[1][0] == "1110"
    assert rows[2][0] == "1364"
    assert rows[2][2] == "NEJ" and rows[2][4] == "NEJ"
    assert "EJ GODKÄNT" in rows[2][-1]


def test_k1_legacy_xls_scans_b_c_and_e_f_without_modifying_file(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    # Isolated fake xlrd workbook: protects shape without requiring licensed BAS bytes.
    cells = [
        ["", "BAS Förenklat årsbokslut (K1) – Kontoplan 2018", "", "", "", "", ""],
        ["", "BAS-konton", "", "Rad", "Underkonton", "", "Rad"],
        ["", 1110.0, "Byggnader", "B2", 1119.0, "Avskrivningar", "B2"],
        ["", 1130.0, "Mark", "B3", "", "", ""],
    ]
    class Sheet:
        name = "Blad1"
        nrows = len(cells)
        ncols = 7
        def cell_value(self, i: int, j: int) -> object:
            return cells[i][j]
    class Book:
        nsheets = 1
        def sheets(self) -> list[Sheet]:
            return [Sheet()]
        def release_resources(self) -> None:
            pass
    monkeypatch.setitem(sys.modules, "xlrd", SimpleNamespace(open_workbook=lambda *args, **kwargs: Book()))
    path = tmp_path / "Kontoplan_K1_2018.xls"
    path.write_bytes(b"synthetic-placeholder")
    source = read_k1_xls(path)
    assert len(source.unique) == 3
    assert source.unique["1119"] == "Avskrivningar"
    assert source.unique["1130"] == "Mark"
    assert path.read_bytes() == b"synthetic-placeholder"


def test_vibeke_45_accounts_no_duplicate(tmp_path: Path) -> None:
    source = tmp_path / "vibeke.json"
    accounts = [{"number": f"{1000 + i}", "name": f"Exempel {i}"} for i in range(45)]
    source.write_text(json.dumps({"schema": "sieverk-k1-list/1", "accounts": accounts}), encoding="utf-8")
    assert len(_read_vibeke(source)) == 45
    accounts[1]["number"] = accounts[0]["number"]
    source.write_text(json.dumps({"schema": "sieverk-k1-list/1", "accounts": accounts}), encoding="utf-8")
    with pytest.raises(SourceError, match="Duplicate"):
        _read_vibeke(source)


def test_candidate_creates_new_private_release_without_overwriting(tmp_path: Path, monkeypatch: pytest.MonkeyPatch) -> None:
    """Synthetic end-to-end orchestration with the generator stubbed, never real BAS data."""
    import importlib.util
    import shutil

    repo = tmp_path / "sieverk"
    tools = repo / "tools"
    tools.mkdir(parents=True)
    src = Path(__file__).resolve().parents[1] / "bas_annual.py"
    shutil.copyfile(src, tools / "bas_annual.py")
    spec = importlib.util.spec_from_file_location("bas_annual_e2e", tools / "bas_annual.py")
    assert spec is not None and spec.loader is not None
    module = importlib.util.module_from_spec(spec)
    monkeypatch.setitem(sys.modules, "bas_annual_e2e", module)
    spec.loader.exec_module(module)

    data = repo / "masterdata"
    real = data / "real"
    real.mkdir(parents=True)
    accounts = [{"number": str(1000 + i), "name": f"Synthetic {i}"} for i in range(45)]
    (data / "vibeke-k1-lista-2026-08-11.json").write_text(
        json.dumps({"schema": "sieverk-k1-list/1", "accounts": accounts}), encoding="utf-8"
    )
    (data / "taxonomy-1.0.json").write_text('"synthetic"', encoding="utf-8")
    original_k1 = real / "k1.xls"
    original_k1.write_bytes(b"SYNTHETIC not real BAS")
    original_annual = _annual_file(real / "annual.xlsx")
    monkeypatch.setattr(module, "read_k1_xls", lambda p: module._compile_entries([
        module.Entry("1110", "Byggnader", "SYNTHETIC R1"),
        module.Entry("1130", "Mark", "SYNTHETIC R2"),
    ]))
    monkeypatch.setattr(module, "_require_gitignored_repository", lambda *args: None)
    def fake_init(*args: object) -> None:
        out = args[1]
        assert isinstance(out, Path) and not out.exists()
        wb = Workbook()
        wb.active.title = "README"
        wb.active.append(("status", "DRAFT"))
        wb.save(out)
        wb.close()
    monkeypatch.setitem(sys.modules, "mastermatris_gen", SimpleNamespace(
        RULESET_CORE_ACCOUNTS=("1910", "2440", "3740"), init_workbook=fake_init
    ))
    candidate = module.create_candidate(
        repo=repo, k1_path=original_k1, annual_path=original_annual,
        year=2026, release_id="2026.1-candidate-20261114",
        expected_k1_count=2, expected_annual_count=5,
    )
    assert (candidate / "manifest.json").is_file()
    m = json.loads((candidate / "manifest.json").read_text(encoding="utf-8"))
    assert m["approved_for_production"] is False
    assert "2087" in m["bas_crosscheck"]["conflicts"]
    assert (candidate / "Mastermatris_v1.2.xlsx").is_file()
    assert (candidate / "normalized" / "bas-2018-k1.xlsx").is_file()
    assert original_k1.read_bytes() == b"SYNTHETIC not real BAS"
    assert len(_read_vibeke(data / "vibeke-k1-lista-2026-08-11.json")) == 45
    with pytest.raises(module.SourceError, match="already exists"):
        module.create_candidate(
            repo=repo, k1_path=original_k1, annual_path=original_annual,
            year=2026, release_id="2026.1-candidate-20261114",
        )
