"""SV-02: create a review-only BAS/K1 annual candidate, preserving every source.

This is an input adapter, NOT an accounting approval or production release.
The existing mastermatris_gen.py and SIEverk Rust code remain unchanged.

Requires `xlrd` (legacy .xls) and `openpyxl` (source/reference .xlsx).
Outputs only under gitignored masterdata/real/releases/<new-release-id>.
The target release directory must not already exist. No original is modified.
"""

from __future__ import annotations

import argparse
import csv
import hashlib
import io
import json
import re
import subprocess
import sys
import unicodedata
from collections import defaultdict
from dataclasses import dataclass
from pathlib import Path
from typing import Any

from openpyxl import Workbook, load_workbook

_ACCOUNT = re.compile(r"\d{4}")
_2026_ACCOUNT = re.compile(r"(\d{4})(#?)")
_RELEASE = re.compile(r"20\d{2}(?:\.[0-9]+)?-candidate-20\d{6}")
_EXPECTED_K1_TITLE = "BAS Förenklat årsbokslut (K1)"


class SourceError(Exception):
    """Source data or release preconditions do not meet the fail-closed contract."""


def _text(value: object) -> str:
    if value is None:
        return ""
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    return " ".join(unicodedata.normalize("NFC", str(value)).split())


@dataclass(frozen=True)
class Entry:
    number: str
    name: str
    location: str
    flagged_k2: bool = False


@dataclass(frozen=True)
class SourceTable:
    entries: tuple[Entry, ...]
    unique: dict[str, str]
    repeats: dict[str, tuple[Entry, ...]]
    conflicts: dict[str, tuple[Entry, ...]]
    k2_marked: tuple[str, ...]


def _compile_entries(entries: list[Entry]) -> SourceTable:
    grouped: dict[str, list[Entry]] = defaultdict(list)
    for entry in entries:
        if not _ACCOUNT.fullmatch(entry.number) or not entry.name:
            raise SourceError(f"Invalid account or empty name at {entry.location}")
        grouped[entry.number].append(entry)
    conflicts: dict[str, tuple[Entry, ...]] = {}
    repeats: dict[str, tuple[Entry, ...]] = {}
    unique: dict[str, str] = {}
    for number, instances in sorted(grouped.items()):
        if len(instances) > 1:
            repeats[number] = tuple(instances)
        names = {entry.name for entry in instances}
        if len(names) > 1:
            # Never silently choose either name; preserve all source rows in manifest.
            conflicts[number] = tuple(instances)
        else:
            unique[number] = instances[0].name
    return SourceTable(
        entries=tuple(entries),
        unique=unique,
        repeats=repeats,
        conflicts=conflicts,
        k2_marked=tuple(sorted({e.number for e in entries if e.flagged_k2})),
    )


def read_k1_xls(path: Path) -> SourceTable:
    """BAS Förenklat årsbokslut (K1) 2018: B/C and E/F of legacy .xls."""
    if path.suffix.lower() != ".xls":
        raise SourceError("K1 must be the original .xls, not a renamed .xlsx")
    import xlrd

    book = xlrd.open_workbook(str(path), on_demand=True)
    try:
        if not book.nsheets:
            raise SourceError("K1 workbook is empty")
        entries: list[Entry] = []
        title_seen = False
        for sheet in book.sheets():
            for row_idx in range(sheet.nrows):
                if row_idx < 5:
                    for col_idx in range(min(sheet.ncols, 7)):
                        if _EXPECTED_K1_TITLE.casefold() in _text(sheet.cell_value(row_idx, col_idx)).casefold():
                            title_seen = True
                for num_col, name_col in ((1, 2), (4, 5)):
                    if sheet.ncols <= name_col:
                        continue
                    number = _text(sheet.cell_value(row_idx, num_col))
                    if not _ACCOUNT.fullmatch(number):
                        continue
                    entries.append(
                        Entry(number, _text(sheet.cell_value(row_idx, name_col)),
                              f"{sheet.name}!R{row_idx + 1}C{num_col + 1}")
                    )
        if not title_seen:
            raise SourceError("K1 title not confirmed: wrong source or layout")
        if not entries:
            raise SourceError("K1 account rows missing")
        return _compile_entries(entries)
    finally:
        book.release_resources()


def read_bas_crosscheck_xlsx(path: Path, year: int) -> SourceTable:
    """Official BAS annual chart: main A/B + sub C/D; '#' is K2 metadata."""
    if path.suffix.lower() != ".xlsx":
        raise SourceError("Annual cross-check source must be .xlsx")
    book = load_workbook(path, read_only=True, data_only=True)
    try:
        expected_title = f"KONTOPLAN BAS {year}"
        entries: list[Entry] = []
        title_seen = False
        sheet_seen = False
        for sheet in book.worksheets:
            if sheet.title.strip().casefold() != f"Kontoplan {year}".casefold():
                continue
            sheet_seen = True
            for row_idx, row in enumerate(sheet.iter_rows(values_only=True), start=1):
                if row_idx <= 3 and any(expected_title.casefold() in _text(v).casefold() for v in row):
                    title_seen = True
                if row_idx <= 5:
                    continue
                for num_col, name_col in ((0, 1), (2, 3)):
                    raw = _text(row[num_col]) if len(row) > num_col else ""
                    match = _2026_ACCOUNT.fullmatch(raw)
                    if match is None:
                        continue
                    number, marker = match.groups()
                    entries.append(Entry(
                        number=number,
                        name=_text(row[name_col]) if len(row) > name_col else "",
                        location=f"{sheet.title}!R{row_idx}C{num_col + 1}",
                        flagged_k2=bool(marker),
                    ))
        if not sheet_seen or not title_seen:
            raise SourceError(f"Cannot confirm BAS {year} sheet/title: wrong annual source")
        if not entries:
            raise SourceError(f"BAS {year} contains no account entries")
        return _compile_entries(entries)
    finally:
        book.close()


def _sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            digest.update(block)
    return digest.hexdigest().upper()


def _validate_sha(path: Path, expected: str | None) -> str:
    if not path.is_file() or path.is_symlink():
        raise SourceError(f"Missing or linked source: {path}")
    result = _sha256(path)
    if expected and result != expected.upper():
        raise SourceError(f"SHA-256 differs for {path.name}")
    return result


def _read_vibeke(path: Path) -> dict[str, str]:
    try:
        raw = json.loads(path.read_text(encoding="utf-8"))
        if raw["schema"] != "sieverk-k1-list/1":
            raise SourceError("Unexpected Vibeke schema")
        entries = raw["accounts"]
        if len(entries) != 45:
            raise SourceError("Vibeke list must contain 45 accounts")
        numbered = [(_text(e["number"]), _text(e["name"])) for e in entries]
        if len({n for n, _ in numbered}) != len(numbered):
            raise SourceError("Duplicate account in Vibeke list")
        if any(not _ACCOUNT.fullmatch(n) or not name for n, name in numbered):
            raise SourceError("Invalid account in Vibeke list")
        return dict(numbered)
    except (ValueError, OSError, KeyError, TypeError) as exc:
        raise SourceError("Cannot read Vibeke account list") from exc


def _csv_bytes(rows: list[list[str]]) -> bytes:
    stream = io.StringIO(newline="")
    writer = csv.writer(stream, delimiter=";")
    writer.writerows(rows)
    return stream.getvalue().encode("utf-8-sig")


def _normalised_xlsx(table: SourceTable) -> bytes:
    workbook = Workbook()
    sheet = workbook.active
    sheet.title = "Konton"
    sheet.append(("Konto", "Kontonamn"))
    for account, name in sorted(table.unique.items()):
        sheet.append((account, name))
    sheet.freeze_panes = "A2"
    sheet.auto_filter.ref = sheet.dimensions
    sheet.column_dimensions["A"].width = 15
    sheet.column_dimensions["B"].width = 60
    # Account codes as text, never formulas or scientific notation.
    for row in sheet.iter_rows(min_row=2, min_col=1, max_col=1):
        row[0].number_format = "@"
    result = io.BytesIO()
    workbook.save(result)
    workbook.close()
    return result.getvalue()


def _source_summary(source: SourceTable) -> dict[str, Any]:
    return {
        "rows": len(source.entries),
        "unambiguous_accounts": len(source.unique),
        "repeated_numbers": len(source.repeats),
        "conflicts": {
            key: [{"name": e.name, "location": e.location} for e in entries]
            for key, entries in sorted(source.conflicts.items())
        },
        "k2_marked": list(source.k2_marked),
    }


def _require_gitignored_repository(root: Path, target: Path) -> None:
    # The real BAS sources must never be committed or copied to a public location.
    real = (root / "masterdata" / "real").resolve()
    dest = target.resolve()
    if dest == real or real not in dest.parents:
        raise SourceError("Output must be under masterdata/real/")
    check = subprocess.run(
        ["git", "-C", str(root), "check-ignore", "-q", "--", "masterdata/real/releases/safety-probe.txt"],
        capture_output=True, check=False,
    )
    if check.returncode != 0:
        raise SourceError("masterdata/real/ is NOT Git-ignored; refuse to write source data")


def _create_new(path: Path, data: bytes) -> None:
    with path.open("xb") as f:
        f.write(data)


def _comparison_rows(vibeke: dict[str, str], k1: SourceTable, annual: SourceTable) -> list[list[str]]:
    rows = [["Konto", "Vibeke namn", "BAS 2018 K1", "K1 namn", "BAS annual", "BAS annual namn", "Namnkonflikt", "Granskningsstatus"]]
    for number, name in sorted(vibeke.items()):
        k1_name = k1.unique.get(number)
        annual_name = annual.unique.get(number)
        rows.append([
            number, name, "JA" if k1_name is not None else "NEJ", k1_name or "",
            "TVETYDIGT" if number in annual.conflicts else "JA" if annual_name is not None else "NEJ",
            annual_name or "",
            "JA" if number in k1.conflicts or number in annual.conflicts else "NEJ",
            "EJ GODKÄNT – MANUELL GRANSKNING",
        ])
    return rows


def _review_text(*, version: str, k1: SourceTable, annual: SourceTable,
                 vibeke: dict[str, str], year: int) -> str:
    absent_k1 = sorted(set(vibeke) - set(k1.unique))
    absent_annual = sorted(set(vibeke) - set(annual.unique))
    both_absent = sorted(set(absent_k1) & set(absent_annual))
    lines = [
        f"# SIEverk BAS masterdata candidate {version}", "",
        "**STATUS: DRAFT / NOT APPROVED FOR PRODUCTION ACCOUNTING.**", "",
        "This is a new review package only. Neither BAS source, Vibeke's list, nor prior masterdata is changed.",
        "The real `Mastermatris_v1.2.xlsx` produced here contains ONLY the initial account prefill, not approved accounting decisions, VAT rules, SRU rules or reviewer signoff.", "",
        "## Sources", "",
        "- BAS 2018 K1 is authoritative for the supported K1 chart, not for every user-defined account.",
        f"- BAS {year} is CROSS-CHECK ONLY, never an automatic substitution for 2018 K1.",
        "- Vibeke's 45 account list is a preserved must_include minimum, including accounts absent from either BAS source.", "",
        "## Numbers", "",
        f"- BAS 2018 K1: {len(k1.entries)} rows, {len(k1.unique)} unambiguous unique accounts.",
        f"- BAS {year}: {len(annual.entries)} rows, {len(annual.unique)} unambiguous unique accounts; {len(annual.conflicts)} conflicted numbers withheld from the normalized reference.",
        f"- Vibeke missing in K1: {len(absent_k1)}; missing in BAS {year}: {len(absent_annual)}; missing in both: {len(both_absent)}.", "",
        "## Unresolved source conflicts", "",
    ]
    if annual.conflicts or k1.conflicts:
        for label, table in (("K1", k1), (f"BAS {year}", annual)):
            for number, entries in sorted(table.conflicts.items()):
                lines.append(f"- {label}, {number}: " + " | ".join(f"{e.name} ({e.location})" for e in entries))
    else:
        lines.append("- None detected")
    lines += [
        "", "## Required human decisions", "",
        "1. Review every entry in `vibeke-comparison.csv`; reconcile missing-account and naming decisions.",
        "2. Resolve every source conflict with source location evidence (do not discard or automatically choose a name).",
        "3. Populate the Mastermatris business rules and approved examples; obtain a real accounting_reviewer signoff.",
        "4. Run existing generator with `--approved` only after real approval, and run `sieverk inspect-masterdata` on the resulting six files.",
        "5. Do not set `STORAGE_APPROVED=True` or enable customer export just because this candidate exists.", "",
        "## No mutation", "",
        "DATA MIGRATION: NO. EXISTING ROW MUTATION: NO. PRODUCTION WRITE: NO.",
        "No GitHub push/Actions, no Fly/Neon/B2 operations performed.", "",
    ]
    return "\n".join(lines)


def create_candidate(
    *, repo: Path, k1_path: Path, annual_path: Path, year: int,
    release_id: str, k1_expected_sha: str | None = None,
    annual_expected_sha: str | None = None,
    expected_k1_count: int | None = None,
    expected_annual_count: int | None = None,
) -> Path:
    """Preflight every known hazard BEFORE creating one new directory."""
    repo = repo.resolve()
    if Path(__file__).resolve().parents[1] != repo:
        raise SourceError("Script must run from its matching SIEverk checkout")
    if not _RELEASE.fullmatch(release_id):
        raise SourceError("Invalid release id (use YYYY.N-candidate-YYYYMMDD)")
    if not 2018 <= year <= 2100:
        raise SourceError("Cross-check year out of range")
    release_root = repo / "masterdata" / "real" / "releases"
    target = release_root / release_id
    _require_gitignored_repository(repo, target)
    if target.exists():
        raise SourceError(f"Release already exists; never overwrite: {target}")
    k1_path = k1_path.resolve(strict=True)
    annual_path = annual_path.resolve(strict=True)
    if k1_path == annual_path:
        raise SourceError("K1 and annual input paths must differ")
    k1_hash = _validate_sha(k1_path, k1_expected_sha)
    annual_hash = _validate_sha(annual_path, annual_expected_sha)
    vibeke_path = repo / "masterdata" / "vibeke-k1-lista-2026-08-11.json"
    taxonomy_path = repo / "masterdata" / "taxonomy-1.0.json"
    vibeke_hash = _validate_sha(vibeke_path, None)
    taxonomy_hash = _validate_sha(taxonomy_path, None)
    vibeke = _read_vibeke(vibeke_path)
    k1 = read_k1_xls(k1_path)
    annual = read_bas_crosscheck_xlsx(annual_path, year)
    if k1.conflicts:
        raise SourceError("K1 authoritative source is ambiguous; no candidate may be produced")
    if expected_k1_count is not None and len(k1.unique) != expected_k1_count:
        raise SourceError(f"Unexpected K1 count: {len(k1.unique)}")
    if expected_annual_count is not None and (len(annual.unique) + len(annual.conflicts)) != expected_annual_count:
        raise SourceError(f"Unexpected annual count: {len(annual.unique) + len(annual.conflicts)}")
    # No ambiguity can influence an auto-prefilled profile account (45+core).
    from mastermatris_gen import RULESET_CORE_ACCOUNTS, init_workbook

    at_risk = (set(vibeke) | set(RULESET_CORE_ACCOUNTS)) & set(annual.conflicts)
    if at_risk:
        raise SourceError(f"Cross-check ambiguous for profile accounts: {sorted(at_risk)}")
    # Raw data and every conflicted row are preserved in the manifest (only
    # the ambiguous number is excluded from the normalization).
    norm_k1 = _normalised_xlsx(k1)
    norm_annual = _normalised_xlsx(annual)
    csv_diff = _csv_bytes(_comparison_rows(vibeke, k1, annual))
    report = _review_text(version=release_id, k1=k1, annual=annual, vibeke=vibeke, year=year).encode("utf-8")
    # Preflight complete. Create exactly one new release directory; no removes.
    release_root.mkdir(parents=True, exist_ok=True)
    target.mkdir(exist_ok=False)
    norm_dir = target / "normalized"
    norm_dir.mkdir(exist_ok=False)
    k1_file = norm_dir / "bas-2018-k1.xlsx"
    annual_file = norm_dir / f"bas-{year}-crosscheck.xlsx"
    _create_new(k1_file, norm_k1)
    _create_new(annual_file, norm_annual)
    _create_new(target / "vibeke-comparison.csv", csv_diff)
    _create_new(target / "review.md", report)
    workbook = target / "Mastermatris_v1.2.xlsx"
    if workbook.exists():
        raise SourceError("Workbook already exists")
    # Existing generator: creates an *unapproved*, editable draft in the new
    # candidate folder; never run --approved here.
    init_workbook(
        taxonomy_path, workbook, "lantbruk-k1", vibeke_path, k1_file, annual_file
    )
    if not workbook.is_file():
        raise SourceError("Existing generator did not create the workbook")
    files = [k1_file, annual_file, target / "vibeke-comparison.csv", target / "review.md", workbook]
    manifest: dict[str, Any] = {
        "schema": "sieverk-bas-candidate/1",
        "release_id": release_id,
        "state": "DRAFT_REVIEW_REQUIRED",
        "approved_for_production": False,
        "sources": {
            "bas_2018_k1": {"name": k1_path.name, "sha256": k1_hash, "count": len(k1.unique)},
            "bas_crosscheck": {"name": annual_path.name, "year": year, "sha256": annual_hash, "count": len(annual.unique) + len(annual.conflicts)},
            "vibeke_k1": {"name": vibeke_path.name, "sha256": vibeke_hash, "count": len(vibeke)},
            "taxonomy": {"name": taxonomy_path.name, "sha256": taxonomy_hash},
        },
        "bas_2018_k1": _source_summary(k1),
        "bas_crosscheck": _source_summary(annual),
        "vibeke_comparison": {
            "missing_in_k1": sorted(set(vibeke) - set(k1.unique)),
            "missing_in_crosscheck": sorted(set(vibeke) - set(annual.unique)),
            "missing_in_both": sorted(set(vibeke) - set(k1.unique) - set(annual.unique)),
        },
        "generated_files": {str(p.relative_to(target)).replace("\\", "/"): _sha256(p) for p in files},
        "notes": "Draft only. No approved masterdata JSONs created. No input overwritten.",
    }
    _create_new(target / "manifest.json", (json.dumps(manifest, indent=2, ensure_ascii=False, sort_keys=True) + "\n").encode("utf-8"))
    return target


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Create non-overwriting BAS K1 + annual comparison and draft Mastermatris")
    parser.add_argument("--repo", required=True, type=Path)
    parser.add_argument("--k1", required=True, type=Path)
    parser.add_argument("--annual", required=True, type=Path)
    parser.add_argument("--annual-year", required=True, type=int)
    parser.add_argument("--release-id", required=True)
    parser.add_argument("--k1-sha256")
    parser.add_argument("--annual-sha256")
    parser.add_argument("--expected-k1-count", type=int)
    parser.add_argument("--expected-annual-count", type=int)
    args = parser.parse_args(argv)
    try:
        target = create_candidate(
            repo=args.repo, k1_path=args.k1, annual_path=args.annual,
            year=args.annual_year, release_id=args.release_id,
            k1_expected_sha=args.k1_sha256, annual_expected_sha=args.annual_sha256,
            expected_k1_count=args.expected_k1_count,
            expected_annual_count=args.expected_annual_count,
        )
    except (SourceError, OSError, ValueError) as exc:
        print(f"BAS CANDIDATE STOP: {exc}", file=sys.stderr)
        return 1
    print(f"BAS CANDIDATE CREATED: {target}")
    print("STATUS: DRAFT / REVIEW REQUIRED — NOT APPROVED FOR LIVE SIE")
    print("No source files overwritten; no production or GitHub Actions changes.")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
