#!/usr/bin/env python3
"""
Mastermatris-generator (SV-02, docs/mastermatris-v1.1-schema.md + v1.2-amendment).

Läser en Mastermatris-arbetsbok (schema v1.2) och SkogsKvittos taxonomiexport,
kör V1–V14 och skriver de genererade masterdata-artefakterna som deterministiska
JSON-filer (sorterade nycklar, sorterade rader, inga tidsstämplar — V14).

Två lägen (v1.2 §C):
  --draft     V1–V4, V6–V8, V10–V12, V14 hårda; V5/V9/V13 varningar; varje
              Automatic-fall utan godkännande nedgraderas deterministiskt till
              Conditional med villkor/kontrollfråga så att V10 håller.
  --approved  V1–V14 hårda; Automatic tillåtet.

Inga implicita rötter (v1.2 §D): --root, --workbook, --taxonomy, --k1-list, --out
anges alltid. BUILD FAILURE (exit 2, ingen fil skrivs) vid hårt fel. Byggrapport med
riktig tidsstämpel skrivs till <root>/build-report.txt — den är otrackad.

M1 (LANTBRUK_K1, README-nyckel profile=lantbruk_k1): --k1-table (BAS 2018 för K1, den
auktoritativa K1-tabellen) och --crosscheck (BAS 2026, endast cross-check) är då
OBLIGATORISKA; saknas de ⇒ BUILD FAILURE, aldrig tyst överhoppad cross-check. Varje
konto med source_id=BAS_2018_K1 måste finnas i K1-tabellen med samma namn; skiljer sig
K1-tabellen och 2026-cross-checken (saknas/annat namn) för ett profilkonto krävs en
explicit source_decision — ingen automatisk merge. Den tabellformen som förväntas är
en flik med kolumnerna "Konto" och "Kontonamn" (rubrikrad); hittas inte kolumnerna ⇒
läsbart fel. Vibekes K1-lista (--k1-list, tracked JSON) är must_include-minimum:
varje konto där måste finnas med must_include=true och must_include_origin=VIBEKE_K1_LIST.

    uv run --with openpyxl tools/mastermatris_gen.py \\
        --root fixtures/masterdata/synthetic \\
        --workbook fixtures/masterdata/synthetic/Mastermatris_v1.2-synthetic.xlsx \\
        --taxonomy masterdata/taxonomy-1.0.json \\
        --out fixtures/masterdata/synthetic/generated --draft

    tools/mastermatris_gen.py init-workbook --profile lantbruk-k1 --taxonomy ... \
        --k1-list masterdata/vibeke-k1-lista-2026-08-11.json \
        --k1-table masterdata/real/bas-2018-k1.xlsx --crosscheck masterdata/real/bas-2026-crosscheck.xlsx \
        --workbook masterdata/real/Mastermatris_v1.2.xlsx
        skapar arbetsboken med genererade flikar (Taxonomi, Värdelistor) OCH en förifylld
        Konton-flik: Vibekes konton (namn ur K1-tabellen när kontot finns där, annars
        hennes namn som fritt konto) + de konton reglerna behöver. --profile synthetic
        ger bara rubriker (används av det syntetiska fixturet).
"""

from __future__ import annotations

import argparse
import datetime as dt
import hashlib
import json
import platform
import re
import subprocess
import sys
from collections import Counter, defaultdict
from dataclasses import dataclass, field
from decimal import Decimal, InvalidOperation
from pathlib import Path
from typing import Any

GENERATOR_VERSION = "mastermatris-gen/1"
WORKBOOK_SCHEMA = "1.2"
STATUSES = ("Utkast", "Godkänd", "Struken")
ROLES = ("accounting_reviewer", "author")
AUTOMATIONS = ("Automatic", "Conditional", "Manual")
DIRECTIONS = ("ingående", "utgående")
DEDUCTIBILITY = ("full", "ingen", "manuell")
KINDS = ("tillgång", "skuld", "eget_kapital", "intäkt", "kostnad")
BUSINESS_GROUPS = ("skog", "djur", "odling", "mark", "base")
BOOKKEEPING = ("cash", "invoice")
VAT_RATES = ("25", "12", "6", "0")
SOURCE_IDS = ("BAS_2018_K1", "BAS_2026", "VIBEKE_K1_LIST", "SYNTHETIC")
ORIGINS = ("VIBEKE_K1_LIST", "RULESET", "ENGINE")
KIND_BY_CLASS = {"1": "tillgång", "2": None, "3": "intäkt", "4": "kostnad", "5": "kostnad", "6": "kostnad", "7": "kostnad", "8": None}
DRAFT_VILLKOR = "Ej accounting_reviewer-godkänd för automatisk kontering."
DRAFT_FRAGA = "Regeln är inte konsultgranskad."
REVIEW_COLUMNS = ("status", "granskad_av", "granskad_roll", "granskad_datum", "kommentar")

SHEETS: dict[str, tuple[str, ...]] = {
    "README": ("nyckel", "värde"),
    "Källor": ("source_id", "namn", "utgivare", "referens", "version", "hämtad", "sha256", "rättigheter"),
    "Taxonomi": ("kategori", "grupp", "requires_business_share", "investment_risk", "vat_check", "sensitive"),
    "Värdelistor": ("lista", "värde", "etikett"),
    "Konton": (
        "konto", "namn", "kontoklass", "active", "user_selectable", "engine_proposable", "return_sie_allowed",
        "closing_account", "business_groups", "must_include", "must_include_origin", "source_id", "source_version",
        "source_decision", *REVIEW_COLUMNS,
    ),
    "SRU": ("konto", "sru_kod", "blankett", "giltig_från", "giltig_till", "källa", "källversion", "verifierad", *REVIEW_COLUMNS),
    "Momsregler": ("momsregel_id", "namn", "riktning", "tillåtna_satser", "momskonto", "avdragsrätt", "villkor", "manual_review", *REVIEW_COLUMNS),
    "Motkonton": ("källa", "nyckel", "bokföringsmetod", "konto", "automation", "fråga", *REVIEW_COLUMNS),
    "Redovisningsfall": (
        "case_id", "källa", "kategori", "income_type", "är_default", "automation", "konto", "momsregel_id",
        "motkonto_regel", "sru_override", "villkor", "kontrollfråga", "beskrivning", *REVIEW_COLUMNS,
    ),
    "Tester": (
        "exempel_id", "case_id", "total", "moms", "öresutjämning", "payment_method", "income_type", "betald",
        "bokföringsmetod", "momsregistrerad", "förväntad_status", "förväntade_rader", "förväntade_findings", *REVIEW_COLUMNS,
    ),
}


class BuildFailure(Exception):
    """Hard validation failure — nothing is written."""


@dataclass
class Report:
    mode: str
    errors: list[str] = field(default_factory=list)
    warnings: list[str] = field(default_factory=list)

    def error(self, rule: str, message: str) -> None:
        self.errors.append(f"{rule}: {message}")

    def warn(self, rule: str, message: str) -> None:
        self.warnings.append(f"{rule}: {message}")

    def soft(self, rule: str, message: str) -> None:
        """V5/V9/V13: hard in --approved, warning in --draft (v1.2 §C)."""
        if self.mode == "approved":
            self.error(rule, message)
        else:
            self.warn(rule, message)


# ---------------------------------------------------------------------------
# Workbook reading — plain rows, no openpyxl types leaking out
# ---------------------------------------------------------------------------


def _cell(value: Any) -> str:
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    if isinstance(value, float) and value.is_integer():
        return str(int(value))
    if isinstance(value, (dt.date, dt.datetime)):
        return value.date().isoformat() if isinstance(value, dt.datetime) else value.isoformat()
    return str(value).strip()


def _bool(text: str, *, default: bool = False) -> bool:
    if text == "":
        return default
    if text.lower() in ("true", "1", "ja", "yes", "x"):
        return True
    if text.lower() in ("false", "0", "nej", "no"):
        return False
    raise BuildFailure(f"ogiltigt booleskt värde {text!r}")


def read_workbook(path: Path) -> dict[str, list[dict[str, str]]]:
    from openpyxl import load_workbook

    wb = load_workbook(path, read_only=True, data_only=True)
    sheets: dict[str, list[dict[str, str]]] = {}
    for name, columns in SHEETS.items():
        if name not in wb.sheetnames:
            raise BuildFailure(f"flik saknas: {name}")
        ws = wb[name]
        rows = list(ws.iter_rows(values_only=True))
        header = tuple(_cell(c) for c in (rows[0] if rows else ()))
        header = tuple(h for h in header if h)
        if header != columns:
            raise BuildFailure(f"flik {name}: rubrikrad {header} matchar inte schema {columns}")
        out = []
        for raw in rows[1:]:
            values = [_cell(c) for c in raw[: len(columns)]]
            if all(v == "" for v in values):
                continue
            out.append(dict(zip(columns, values + [""] * (len(columns) - len(values)), strict=True)))
        sheets[name] = out
    return sheets


def load_k1_list(path: Path) -> dict[str, dict[str, str]]:
    """Vibeke's list (tracked JSON) — the must_include minimum, keyed by number."""
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema") != "sieverk-k1-list/1":
        raise BuildFailure(f"K1-listan har fel schema: {data.get('schema')!r}")
    accounts = {a["number"]: a for a in data["accounts"]}
    if len(accounts) != len(data["accounts"]):
        raise BuildFailure("K1-listan innehåller dubbletter")
    return accounts


def read_bas_table(path: Path, label: str) -> dict[str, str]:
    """A BAS chart file (local, gitignored): first sheet whose header row has
    'Konto' and 'Kontonamn'. Returns number → name. Missing file or columns ⇒
    readable BuildFailure — the cross-check is never skipped silently."""
    from openpyxl import load_workbook

    if not path.is_file():
        raise BuildFailure(f"{label}: filen saknas: {path} (M1 kräver den — cross-checken hoppas aldrig över)")
    wb = load_workbook(path, read_only=True, data_only=True)
    for ws in wb.worksheets:
        rows = ws.iter_rows(values_only=True)
        for raw in rows:
            header = [_cell(c).lower() for c in raw]
            if "konto" in header and "kontonamn" in header:
                ki, ni = header.index("konto"), header.index("kontonamn")
                table: dict[str, str] = {}
                for r in rows:
                    number, name = _cell(r[ki]) if ki < len(r) else "", _cell(r[ni]) if ni < len(r) else ""
                    if re.fullmatch(r"\d{4}", number):
                        table[number] = name
                if not table:
                    raise BuildFailure(f"{label}: inga fyrsiffriga konton under rubrikraden i {path.name}")
                return table
    raise BuildFailure(f"{label}: hittar ingen flik med kolumnerna 'Konto' och 'Kontonamn' i {path.name}")


def load_taxonomy(path: Path) -> dict[str, Any]:
    data = json.loads(path.read_text(encoding="utf-8"))
    if data.get("schema") != "skogskvitto-taxonomy-export/1":
        raise BuildFailure(f"taxonomiexport har fel schema: {data.get('schema')!r}")
    return data


# ---------------------------------------------------------------------------
# Validation V1–V13 (V14 is the determinism self-test in tools/tests)
# ---------------------------------------------------------------------------


def _live(rows: list[dict[str, str]]) -> list[dict[str, str]]:
    return [r for r in rows if r["status"] != "Struken"]


def _check_review(rep: Report, sheet: str, rows: list[dict[str, str]], key: str) -> None:
    for r in rows:
        ident = f"{sheet}/{r[key]}"
        if r["status"] not in STATUSES:
            rep.error("V9", f"{ident}: ogiltig status {r['status']!r}")
        if r["status"] == "Godkänd":
            if not r["granskad_av"]:
                rep.soft("V9", f"{ident}: Godkänd utan granskad_av")
            elif r["granskad_roll"] != "accounting_reviewer":
                rep.soft("V9", f"{ident}: Godkänd med granskad_roll={r['granskad_roll']!r} (kräver accounting_reviewer)")


def validate(
    sheets: dict[str, list[dict[str, str]]],
    taxonomy: dict[str, Any],
    rep: Report,
    k1_list: dict[str, dict[str, str]] | None = None,
    k1_table: dict[str, str] | None = None,
    crosscheck: dict[str, str] | None = None,
) -> dict[str, Any]:
    """Returns the model used for emission; appends to rep.

    k1_list: Vibeke's must_include minimum (V5 — missing account is hard).
    k1_table/crosscheck: BAS 2018 K1 + BAS 2026, required for profile=lantbruk_k1 (M1).
    """
    categories = {c["name"]: c for c in taxonomy["categories"]}
    income_types = {i["value"]: i for i in taxonomy["income_types"]}
    payment_methods = {p["value"] for p in taxonomy["payment_methods"]}

    # Generated, locked sheets must equal the export (they are dropdown sources).
    tax_sheet = {r["kategori"]: r for r in sheets["Taxonomi"]}
    if set(tax_sheet) != set(categories):
        rep.error("V1", "fliken Taxonomi matchar inte taxonomy-1.0.json (regenerera arbetsboken)")
    for name, row in tax_sheet.items():
        c = categories.get(name)
        if c and (
            row["grupp"] != c["group"]
            or any(_bool(row[f]) != c[f] for f in ("requires_business_share", "investment_risk", "vat_check", "sensitive"))
        ):
            rep.error("V1", f"Taxonomi/{name}: flaggor avviker från exporten")
    values = defaultdict(set)
    for r in sheets["Värdelistor"]:
        values[r["lista"]].add(r["värde"])
    if values["income_type"] != set(income_types):
        rep.error("V2", "Värdelistor/income_type matchar inte exporten")
    if values["payment_method"] != payment_methods:
        rep.error("V2", "Värdelistor/payment_method matchar inte exporten")

    # Konton (V4, provenance, v1.2 §B)
    source_ids = {r["source_id"] for r in sheets["Källor"]}
    for sid in source_ids:
        if sid not in SOURCE_IDS:
            rep.error("V4", f"Källor: okänt source_id {sid!r}")
    accounts: dict[str, dict[str, str]] = {}
    for r in _live(sheets["Konton"]):
        k = r["konto"]
        if not re.fullmatch(r"\d{4}", k):
            rep.error("V4", f"Konton/{k!r}: kontot måste vara exakt 4 siffror")
            continue
        if k in accounts:
            rep.error("V4", f"Konton/{k}: dubblett")
            continue
        if r["kontoklass"] not in KINDS:
            rep.error("V4", f"Konton/{k}: ogiltig kontoklass {r['kontoklass']!r}")
        expected = KIND_BY_CLASS[k[0]]
        if expected and r["kontoklass"] != expected:
            rep.error("V4", f"Konton/{k}: klass {r['kontoklass']!r} motsäger första siffran ({expected})")
        if k[0] == "2" and r["kontoklass"] not in ("skuld", "eget_kapital"):
            rep.error("V4", f"Konton/{k}: klass 2 måste vara skuld eller eget_kapital")
        if k[0] == "8" and r["kontoklass"] not in ("intäkt", "kostnad"):
            rep.error("V4", f"Konton/{k}: klass 8 måste vara intäkt eller kostnad")
        if not r["namn"]:
            rep.error("V4", f"Konton/{k}: namn saknas")
        if r["source_id"] not in source_ids:
            rep.error("V4", f"Konton/{k}: source_id {r['source_id']!r} finns inte i fliken Källor")
        if _bool(r["must_include"]) and r["must_include_origin"] not in ORIGINS:
            rep.error("V4", f"Konton/{k}: must_include kräver must_include_origin ∈ {ORIGINS}")
        if r["source_decision"]:
            try:
                parse_source_decision(r["source_decision"])
            except BuildFailure as exc:
                rep.error("V4", f"Konton/{k}: {exc}")
        if not (r["source_version"] or next((s["version"] for s in sheets["Källor"] if s["source_id"] == r["source_id"]), "")):
            rep.error("V4", f"Konton/{k}: source_version saknas (obligatorisk per v1.2 §B) och källan har ingen version")
        for g in filter(None, r["business_groups"].split(";")):
            if g not in BUSINESS_GROUPS:
                rep.error("V4", f"Konton/{k}: okänd business_group {g!r}")
        accounts[k] = r
    _check_review(rep, "Konton", _live(sheets["Konton"]), "konto")
    must_include = [k for k, r in accounts.items() if _bool(r["must_include"])]
    for k in must_include:  # V5 — approval part is soft (v1.2 §C)
        if accounts[k]["status"] != "Godkänd":
            rep.soft("V5", f"Konton/{k}: must_include-konto är inte Godkänd ({accounts[k]['status']})")
    if k1_list is not None:  # V5 — presence part: warning in --draft, hard in --approved (v1.2 §C)
        for k in sorted(k1_list):
            r = accounts.get(k)
            if r is None:
                rep.soft("V5", f"Konton/{k}: kontot ur Vibekes K1-lista saknas i Konton (must_include-minimum)")
            elif not _bool(r["must_include"]) or r["must_include_origin"] != "VIBEKE_K1_LIST":
                rep.soft("V5", f"Konton/{k}: kontot ur Vibekes K1-lista måste ha must_include=true och must_include_origin=VIBEKE_K1_LIST")

    # M1 — LANTBRUK_K1 provenance cross-check (never skipped silently)
    meta = {r["nyckel"]: r["värde"] for r in sheets["README"]}
    if meta.get("profile") == "lantbruk_k1":
        if k1_table is None or crosscheck is None:
            rep.error("M1", "profile=lantbruk_k1 kräver --k1-table (BAS 2018 K1) och --crosscheck (BAS 2026); ingen cross-check hoppas över")
        else:
            for k, r in sorted(accounts.items()):
                where = f"Konton/{k}"
                if r["source_id"] == "BAS_2018_K1":
                    if k not in k1_table:
                        rep.error("M1", f"{where}: source_id=BAS_2018_K1 men kontot finns inte i K1-tabellen")
                        continue
                    if k1_table[k] != r["namn"] and not r["source_decision"]:
                        rep.error("M1", f"{where}: namnet {r['namn']!r} avviker från K1-tabellen ({k1_table[k]!r}) utan source_decision")
                    in_2026 = crosscheck.get(k)
                    if (in_2026 is None or in_2026 != k1_table[k]) and not r["source_decision"]:
                        detail = "saknas i BAS 2026" if in_2026 is None else f"heter {in_2026!r} i BAS 2026"
                        rep.error("M1", f"{where}: K1-tabellen och 2026-cross-checken skiljer sig ({detail}) — explicit source_decision krävs, ingen automatisk merge")
                elif r["source_id"] == "VIBEKE_K1_LIST" and k in k1_table:
                    rep.error("M1", f"{where}: kontot finns i K1-tabellen — source_id ska vara BAS_2018_K1, inte VIBEKE_K1_LIST")
                elif r["source_id"] == "BAS_2026":
                    if k not in crosscheck:
                        rep.error("M1", f"{where}: source_id=BAS_2026 men kontot finns inte i cross-check-tabellen")
                    elif not r["source_decision"]:
                        rep.error("M1", f"{where}: BAS 2026 är endast cross-check — ett konto med source_id=BAS_2026 kräver source_decision")

    def account_ok(k: str, where: str, rule: str, *, proposable_needed: bool) -> None:
        a = accounts.get(k)
        if a is None:
            rep.error(rule, f"{where}: konto {k!r} saknas i Konton eller är Struken")
            return
        if not _bool(a["active"], default=True):
            rep.error(rule, f"{where}: konto {k} är active=false")
        if proposable_needed and not _bool(a["engine_proposable"]):
            rep.error(rule, f"{where}: konto {k} är engine_proposable=false för ett Automatic/Conditional-fall")

    # SRU (V6)
    sru_rows = _live(sheets["SRU"])
    for r in sru_rows:
        account_ok(r["konto"], f"SRU/{r['konto']}", "V6", proposable_needed=False)
        if not re.fullmatch(r"\d{4}", r["sru_kod"]):
            rep.error("V6", f"SRU/{r['konto']}: sru_kod {r['sru_kod']!r} måste vara 4 siffror")
    by_account = defaultdict(list)
    for r in sru_rows:
        by_account[r["konto"]].append(r)
    for k, rows in by_account.items():
        spans = sorted((int(r["giltig_från"] or 0), int(r["giltig_till"] or 9999)) for r in rows)
        for (a0, a1), (b0, _) in zip(spans, spans[1:], strict=False):
            if b0 <= a1:
                rep.error("V6", f"SRU/{k}: överlappande giltighet")
    _check_review(rep, "SRU", sru_rows, "konto")

    # Momsregler (V7)
    vat_rules = {}
    for r in _live(sheets["Momsregler"]):
        rid = r["momsregel_id"]
        if rid in vat_rules:
            rep.error("V7", f"Momsregler/{rid}: dubblett")
        if r["riktning"] not in DIRECTIONS:
            rep.error("V7", f"Momsregler/{rid}: ogiltig riktning {r['riktning']!r}")
        rates = [s for s in r["tillåtna_satser"].split(";") if s]
        if not rates or any(s not in VAT_RATES for s in rates):
            rep.error("V7", f"Momsregler/{rid}: satser {rates} utanför {{25,12,6,0}}")
        account_ok(r["momskonto"], f"Momsregler/{rid}", "V7", proposable_needed=False)
        if r["avdragsrätt"] not in DEDUCTIBILITY:
            rep.error("V7", f"Momsregler/{rid}: ogiltig avdragsrätt {r['avdragsrätt']!r}")
        vat_rules[rid] = r
    _check_review(rep, "Momsregler", _live(sheets["Momsregler"]), "momsregel_id")

    # Motkonton (V3, V10)
    counter_rows = _live(sheets["Motkonton"])
    for r in counter_rows:
        where = f"Motkonton/{r['källa']}/{r['nyckel']}"
        if r["källa"] not in ("receipt", "income"):
            rep.error("V3", f"{where}: källa måste vara receipt eller income")
        if r["källa"] == "receipt" and r["nyckel"] not in payment_methods:
            rep.error("V2", f"{where}: payment_method saknas i exporten")
        if r["källa"] == "income" and r["nyckel"] not in ("betald", "obetald"):
            rep.error("V3", f"{where}: nyckel måste vara betald eller obetald")
        if r["bokföringsmetod"] and r["bokföringsmetod"] not in BOOKKEEPING:
            rep.error("V3", f"{where}: ogiltig bokföringsmetod")
        if r["automation"] not in AUTOMATIONS:
            rep.error("V10", f"{where}: ogiltig automation")
        if r["konto"]:
            account_ok(r["konto"], where, "V3", proposable_needed=r["automation"] != "Manual")
        elif r["automation"] != "Manual":
            rep.error("V10", f"{where}: konto saknas men automation ≠ Manual")
        if r["automation"] != "Automatic" and not r["fråga"]:
            rep.error("V10", f"{where}: fråga obligatorisk när automation ≠ Automatic")
    _check_review(rep, "Motkonton", counter_rows, "nyckel")

    # Redovisningsfall (V1, V2, V3, V8, V10, V11)
    cases = {}
    defaults: Counter[tuple[str, str]] = Counter()
    for r in _live(sheets["Redovisningsfall"]):
        cid = r["case_id"]
        where = f"Redovisningsfall/{cid}"
        if cid in cases:
            rep.error("V11", f"{where}: dubblett-case_id")
            continue
        if r["källa"] not in ("receipt", "income"):
            rep.error("V11", f"{where}: källa måste vara receipt eller income")
        if r["källa"] == "receipt":
            if r["kategori"] not in categories:
                rep.error("V1", f"{where}: kategori {r['kategori']!r} saknas i taxonomy-1.0.json")
            subject = _slug(r["kategori"])
        else:
            if r["income_type"] not in income_types:
                rep.error("V2", f"{where}: income_type {r['income_type']!r} saknas i exporten")
            subject = f"income.{r['income_type']}"
        if subject and not cid.startswith(subject):
            rep.error("V11", f"{where}: case_id börjar inte med {subject!r}")
        if r["automation"] not in AUTOMATIONS:
            rep.error("V10", f"{where}: ogiltig automation {r['automation']!r}")
        if _bool(r["är_default"]):
            defaults[(r["källa"], r["kategori"] or r["income_type"])] += 1
        if r["konto"]:
            account_ok(r["konto"], where, "V3", proposable_needed=r["automation"] != "Manual")
        if r["momsregel_id"] and r["momsregel_id"] not in vat_rules:
            rep.error("V3", f"{where}: momsregel {r['momsregel_id']!r} saknas")
        if r["motkonto_regel"] and r["motkonto_regel"] != "enligt_motkonton":
            account_ok(r["motkonto_regel"], f"{where} (motkonto)", "V3", proposable_needed=r["automation"] != "Manual")
        if r["automation"] == "Automatic" and not (r["konto"] and r["momsregel_id"] and (r["motkonto_regel"] or "enligt_motkonton")):
            rep.error("V10", f"{where}: Automatic kräver konto, momsregel och motkontoregel")
        if r["automation"] == "Conditional" and not (r["villkor"] and r["kontrollfråga"]):
            rep.error("V10", f"{where}: Conditional kräver villkor och kontrollfråga")
        if r["automation"] == "Manual" and not r["kontrollfråga"]:
            rep.error("V10", f"{where}: Manual kräver kontrollfråga")
        if r["momsregel_id"] and vat_rules.get(r["momsregel_id"], {}).get("avdragsrätt") == "manuell" and r["automation"] == "Automatic":
            rep.error("V10", f"{where}: momsregel med avdragsrätt=manuell kan inte vara Automatic")
        cases[cid] = r
    for (src, subj), n in defaults.items():
        if n > 1:
            rep.error("V8", f"{src}/{subj}: {n} rader har är_default")
    _check_review(rep, "Redovisningsfall", _live(sheets["Redovisningsfall"]), "case_id")

    # Tester (V12, V13)
    examples = _live(sheets["Tester"])
    approved_examples: Counter[str] = Counter()
    for r in examples:
        where = f"Tester/{r['exempel_id']}"
        if r["case_id"] not in cases:
            rep.error("V12", f"{where}: okänt case_id {r['case_id']!r}")
        try:
            total, vat, rounding = (Decimal(r[c] or "0") for c in ("total", "moms", "öresutjämning"))
        except InvalidOperation:
            rep.error("V12", f"{where}: belopp är inte decimaltal")
            continue
        debit = credit = Decimal("0")
        for line in filter(None, [x.strip() for x in r["förväntade_rader"].replace("\n", ";").split(";")]):
            m = re.fullmatch(r"(\d{4}):(debet|kredit):(-?\d+(?:\.\d{1,2})?)", line)
            if not m:
                rep.error("V12", f"{where}: rad {line!r} har inte formen konto:debet|kredit:belopp")
                continue
            account_ok(m.group(1), where, "V12", proposable_needed=False)
            if m.group(2) == "debet":
                debit += Decimal(m.group(3))
            else:
                credit += Decimal(m.group(3))
        if debit != credit:
            rep.error("V12", f"{where}: Σ debet {debit} ≠ Σ kredit {credit}")
        net = total - vat - rounding
        if net < 0:
            rep.error("V12", f"{where}: total ≠ net + moms + öresutjämning (net negativ)")
        if r["status"] == "Godkänd":
            approved_examples[r["case_id"]] += 1
    _check_review(rep, "Tester", examples, "exempel_id")
    for cid, r in cases.items():  # V13
        if r["automation"] == "Automatic" and approved_examples[cid] == 0:
            rep.soft("V13", f"Redovisningsfall/{cid}: Automatic utan minst ett Godkänt exempel")

    # Coverage warnings (never blocking)
    covered = {r["kategori"] for r in cases.values() if r["källa"] == "receipt" and _bool(r["är_default"])}
    for name in sorted(categories):
        if name not in covered:
            rep.warn("täckning", f"kategori {name!r} saknar default-fall")
    for k in sorted(accounts):
        if k not in by_account:
            rep.warn("SRU", f"konto {k} saknar SRU-rad")
    for rid in sorted(vat_rules):
        if not any(c["momsregel_id"] == rid for c in cases.values()):
            rep.warn("moms", f"momsregel {rid} används inte av något fall")
    drafts = sum(1 for sheet in ("Konton", "Momsregler", "Motkonton", "Redovisningsfall", "Tester") for r in sheets[sheet] if r["status"] == "Utkast")
    if drafts:
        rep.warn("status", f"{drafts} Utkast-rader kvar")

    return {
        "accounts": accounts,
        "sru": sru_rows,
        "vat_rules": vat_rules,
        "counter": counter_rows,
        "cases": cases,
        "examples": examples,
        "approved_examples": approved_examples,
        "categories": categories,
    }


def parse_source_decision(cell: str) -> dict[str, str]:
    """Workbook cell `reason | decided_by | decided_at` → structured object (v1.2 §B).

    Exactly three non-empty parts; anything else is a readable failure — a
    decision without a person or a date is not a decision."""
    parts = [p.strip() for p in cell.split("|")]
    if len(parts) != 3 or any(not p for p in parts):
        raise BuildFailure(
            f"source_decision {cell!r} måste ha formen 'reason | decided_by | decided_at' med tre ifyllda delar"
        )
    reason, decided_by, decided_at = parts
    if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", decided_at):
        raise BuildFailure(f"source_decision: decided_at {decided_at!r} måste vara ÅÅÅÅ-MM-DD")
    return {"reason": reason, "decided_by": decided_by, "decided_at": decided_at}


def _slug(text: str) -> str:
    text = text.lower().replace("å", "a").replace("ä", "a").replace("ö", "o").replace("é", "e")
    text = re.sub(r"[^a-z0-9]+", "_", text).strip("_")
    return text


# ---------------------------------------------------------------------------
# Emission — deterministic, header without wall clock (v1.2 §D / V14)
# ---------------------------------------------------------------------------


def _header(meta: dict[str, str], workbook: Path, workbook_sha: str, taxonomy_version: str, mode: str) -> dict[str, Any]:
    return {
        "generated": "AUTO-GENERATED — DO NOT EDIT",
        "workbook": workbook.name,
        "workbook_sha256": workbook_sha,
        "taxonomy_version": taxonomy_version,
        "generator_version": GENERATOR_VERSION,
        "review_status": mode,
        "profile_scope": meta.get("profile_scope", ""),
    }


def _review(r: dict[str, str]) -> dict[str, Any]:
    return {
        "status": r["status"],
        "reviewed_by": r["granskad_av"] or None,
        "reviewed_role": r["granskad_roll"] or None,
        "reviewed_at": r["granskad_datum"] or None,
    }


def emit(sheets: dict[str, list[dict[str, str]]], model: dict[str, Any], taxonomy: dict[str, Any], workbook: Path, workbook_sha: str, mode: str) -> dict[str, bytes]:
    meta = {r["nyckel"]: r["värde"] for r in sheets["README"]}
    for key in ("chart_id", "chart_version", "ruleset_version", "framework", "entity_type", "profile_scope", "profile"):
        if not meta.get(key):
            raise BuildFailure(f"README-fliken saknar nyckel {key!r}")
    header = _header(meta, workbook, workbook_sha, taxonomy["taxonomy_version"], mode)
    sources = {
        r["source_id"]: {
            "name": r["namn"],
            "publisher": r["utgivare"],
            "reference": r["referens"],
            "version": r["version"],
            "retrieved": r["hämtad"] or None,
            "sha256": r["sha256"] or None,
            "rights": r["rättigheter"] or "not confirmed",
        }
        for r in sheets["Källor"]
    }
    accounts = []
    for k in sorted(model["accounts"]):
        r = model["accounts"][k]
        accounts.append(
            {
                "number": k,
                "name": r["namn"],
                "kind": r["kontoklass"],
                "source_id": r["source_id"],
                "source_version": r["source_version"] or sources.get(r["source_id"], {}).get("version", ""),
                "must_include": _bool(r["must_include"]),
                "must_include_origin": r["must_include_origin"] or None,
                "source_decision": parse_source_decision(r["source_decision"]) if r["source_decision"] else None,
                "roles": {
                    "active": _bool(r["active"], default=True),
                    "user_selectable": _bool(r["user_selectable"]),
                    "engine_proposable": _bool(r["engine_proposable"]),
                    "return_sie_allowed": _bool(r["return_sie_allowed"]),
                    "closing_account": _bool(r["closing_account"]),
                    "business_groups": sorted(filter(None, r["business_groups"].split(";"))),
                },
                "review": _review(r),
            }
        )
    chart = {
        "_header": header,
        "schema": "sieverk-chart/1",
        "chart_id": meta["chart_id"],
        "version": meta["chart_version"],
        "framework": meta["framework"],
        "entity_type": meta["entity_type"],
        "profile_scope": meta["profile_scope"],
        "review_status": mode,
        "sources": sources,
        "accounts": accounts,
    }
    sru = {
        "_header": header,
        "schema": "sieverk-sru/1",
        "rows": sorted(
            (
                {
                    "account": r["konto"],
                    "sru": r["sru_kod"],
                    "form": r["blankett"],
                    "valid_from": int(r["giltig_från"]) if r["giltig_från"] else None,
                    "valid_to": int(r["giltig_till"]) if r["giltig_till"] else None,
                    "source": r["källa"] or None,
                    "source_version": r["källversion"] or None,
                    "verified_at": r["verifierad"] or None,
                    "verified": bool(r["verifierad"]),
                    "review": _review(r),
                }
                for r in model["sru"]
            ),
            key=lambda x: (x["account"], x["valid_from"] or 0),
        ),
    }
    vat = {
        "_header": header,
        "schema": "sieverk-vat/1",
        "rules": [
            {
                "id": rid,
                "name": r["namn"],
                "direction": r["riktning"],
                "rates": [s for s in r["tillåtna_satser"].split(";") if s],
                "vat_account": r["momskonto"],
                "deductibility": r["avdragsrätt"],
                "manual_review": _bool(r["manual_review"]),
                "condition": r["villkor"] or None,
                "review": _review(r),
            }
            for rid, r in sorted(model["vat_rules"].items())
        ],
    }
    counter = {
        "_header": header,
        "schema": "sieverk-counter/1",
        "rows": sorted(
            (
                {
                    "source": r["källa"],
                    "key": r["nyckel"],
                    "bookkeeping_method": r["bokföringsmetod"] or None,
                    "account": r["konto"] or None,
                    "automation": r["automation"],
                    "question": r["fråga"] or None,
                    "review": _review(r),
                }
                for r in model["counter"]
            ),
            key=lambda x: (x["source"], x["key"], x["bookkeeping_method"] or ""),
        ),
    }
    cases = []
    for cid in sorted(model["cases"]):
        r = model["cases"][cid]
        automation = r["automation"]
        villkor, fraga = r["villkor"], r["kontrollfråga"]
        approved = r["status"] == "Godkänd" and r["granskad_roll"] == "accounting_reviewer" and model["approved_examples"][cid] > 0
        downgraded = False
        if automation == "Automatic" and (mode == "draft" or not approved):
            # v1.2 §C: deterministic downgrade so V10 holds; Rust never does this.
            automation = "Conditional"
            villkor = villkor or DRAFT_VILLKOR
            fraga = fraga or DRAFT_FRAGA
            downgraded = True
        cases.append(
            {
                "case_id": cid,
                "source": r["källa"],
                "category": r["kategori"] or None,
                "income_type": r["income_type"] or None,
                "is_default": _bool(r["är_default"]),
                "automation": automation,
                "downgraded_from_automatic": downgraded,
                "account": r["konto"] or None,
                "vat_rule": r["momsregel_id"] or None,
                "counter_rule": r["motkonto_regel"] or "enligt_motkonton",
                "sru_override": r["sru_override"] or None,
                "condition": villkor or None,
                "question": fraga or None,
                "description": r["beskrivning"] or None,
                "review": _review(r),
            }
        )
    ruleset = {
        "_header": header,
        "schema": "sieverk-ruleset/1",
        "ruleset_version": meta["ruleset_version"],
        "taxonomy_version": taxonomy["taxonomy_version"],
        "chart": {"chart_id": meta["chart_id"], "version": meta["chart_version"]},
        "review_status": mode,
        # The dropdown universe the cases were validated against — carried in the
        # artefact so Rust can re-run V1/V2 with `--root` alone (no implicit
        # taxonomy path). The generator proves it equals taxonomy-1.0.json.
        "taxonomy": {
            "version": taxonomy["taxonomy_version"],
            "categories": sorted(model["categories"]),
            "income_types": sorted(i["value"] for i in taxonomy["income_types"]),
            "payment_methods": sorted(p["value"] for p in taxonomy["payment_methods"]),
        },
        "cases": cases,
    }
    examples = {
        "_header": header,
        "schema": "sieverk-examples/1",
        "examples": [
            {
                "id": r["exempel_id"],
                "case_id": r["case_id"],
                "input": {
                    "total": r["total"] or None,
                    "vat": r["moms"] or None,
                    "rounding": r["öresutjämning"] or None,
                    "payment_method": r["payment_method"] or None,
                    "income_type": r["income_type"] or None,
                    "paid": _bool(r["betald"]) if r["betald"] else None,
                    "bookkeeping_method": r["bokföringsmetod"] or None,
                    "vat_registered": _bool(r["momsregistrerad"]) if r["momsregistrerad"] else None,
                },
                "expected_status": r["förväntad_status"],
                "expected_lines": [
                    {"account": m.group(1), m.group(2).replace("debet", "debit").replace("kredit", "credit"): m.group(3)}
                    for m in (
                        re.fullmatch(r"(\d{4}):(debet|kredit):(-?\d+(?:\.\d{1,2})?)", x.strip())
                        for x in r["förväntade_rader"].replace("\n", ";").split(";")
                        if x.strip()
                    )
                    if m
                ],
                "expected_findings": [x.strip() for x in r["förväntade_findings"].split(";") if x.strip()],
                "review": _review(r),
            }
            for r in sorted(model["examples"], key=lambda x: x["exempel_id"])
        ],
    }
    slug_chart = meta["chart_id"].lower().replace("_", "-")
    files = {
        f"chart-{slug_chart}-{meta['chart_version']}.json": chart,
        f"sru-{meta['chart_version'].split('.')[0]}.json": sru,
        f"vat-rules-{meta['ruleset_version']}.json": vat,
        f"counter-accounts-{meta['ruleset_version']}.json": counter,
        f"ruleset-{meta['ruleset_version']}.json": ruleset,
        f"examples-{meta['ruleset_version']}.json": examples,
    }
    return {name: (json.dumps(doc, ensure_ascii=False, indent=2, sort_keys=True) + "\n").encode("utf-8") for name, doc in files.items()}


# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------


def build(
    workbook: Path,
    taxonomy_path: Path,
    mode: str,
    k1_list_path: Path | None = None,
    k1_table_path: Path | None = None,
    crosscheck_path: Path | None = None,
) -> tuple[dict[str, bytes], Report]:
    rep = Report(mode=mode)
    sheets = read_workbook(workbook)
    taxonomy = load_taxonomy(taxonomy_path)
    k1_list = load_k1_list(k1_list_path) if k1_list_path else None
    k1_table = read_bas_table(k1_table_path, "--k1-table") if k1_table_path else None
    crosscheck = read_bas_table(crosscheck_path, "--crosscheck") if crosscheck_path else None
    model = validate(sheets, taxonomy, rep, k1_list=k1_list, k1_table=k1_table, crosscheck=crosscheck)
    if rep.errors:
        raise BuildFailure("\n".join(rep.errors))
    workbook_sha = hashlib.sha256(workbook.read_bytes()).hexdigest()
    return emit(sheets, model, taxonomy, workbook, workbook_sha, mode), rep


def _git_sha(root: Path) -> str:
    try:
        return subprocess.run(["git", "rev-parse", "HEAD"], cwd=root, capture_output=True, text=True, check=False).stdout.strip() or "unknown"
    except OSError:
        return "unknown"


def write_build_report(root: Path, out: Path, mode: str, files: dict[str, bytes], rep: Report) -> None:
    lines = [
        f"mastermatris build report — {dt.datetime.now(dt.UTC).isoformat(timespec='seconds')}",
        f"mode={mode} generator={GENERATOR_VERSION} python={platform.python_version()} git={_git_sha(root)}",
        f"out={out}",
        "files:",
        *[f"  {name}  sha256={hashlib.sha256(data).hexdigest()}  bytes={len(data)}" for name, data in sorted(files.items())],
        f"warnings ({len(rep.warnings)}):",
        *[f"  {w}" for w in rep.warnings],
    ]
    (root / "build-report.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")


CLOSING_ACCOUNTS = ("2019", "7821", "7830", "8999")
# Needs-driven extras every ruleset needs beyond Vibeke's list (VAT, cash,
# supplier debt, öresutjämning). Never padding: each is referenced by rules.
RULESET_CORE_ACCOUNTS = ("1910", "2440", "3740")


def prefill_konton(
    k1_list: dict[str, dict[str, str]], k1_table: dict[str, str], crosscheck: dict[str, str]
) -> list[list[Any]]:
    """LANTBRUK_K1 Konton rows per M1: Vibeke's accounts + ruleset core accounts.

    Name and source come from the K1 table when the number exists there
    (source_id=BAS_2018_K1), otherwise from Vibeke's list as a free account
    (source_id=VIBEKE_K1_LIST). Where K1 and the 2026 cross-check disagree the
    row is flagged in `kommentar` — the build will refuse it until a human
    writes a source_decision. Nothing is merged automatically."""
    rows: list[list[Any]] = []
    numbers = list(k1_list) + [n for n in RULESET_CORE_ACCOUNTS if n not in k1_list]
    for n in sorted(numbers):
        vibeke = k1_list.get(n)
        in_k1 = n in k1_table
        if not in_k1 and vibeke is None:
            continue  # a core account absent from the K1 table is a human decision, not a prefill
        name = k1_table[n] if in_k1 else vibeke["name"]
        kind = vibeke["kind"] if vibeke else KIND_BY_CLASS.get(n[0]) or ("skuld" if n[0] == "2" else "kostnad")
        comment = ""
        if in_k1 and crosscheck.get(n) != k1_table[n]:
            comment = "KRÄVER source_decision: " + ("saknas i BAS 2026" if n not in crosscheck else f"BAS 2026: {crosscheck[n]!r}")
        closing = n in CLOSING_ACCOUNTS
        rows.append(
            [
                n, name, kind, True, not closing, not closing, True, closing, "",
                vibeke is not None, "VIBEKE_K1_LIST" if vibeke else "RULESET",
                "BAS_2018_K1" if in_k1 else "VIBEKE_K1_LIST", "2018" if in_k1 else "2026-08-11", "",
                "Utkast", "", "", "", comment,
            ]
        )
    return rows


def init_workbook(
    taxonomy_path: Path,
    workbook: Path,
    profile: str = "synthetic",
    k1_list_path: Path | None = None,
    k1_table_path: Path | None = None,
    crosscheck_path: Path | None = None,
) -> None:
    """Create a workbook with generated Taxonomi/Värdelistor sheets and headers.

    profile=lantbruk-k1 additionally prefills Konton per M1 and requires the
    K1 list, the K1 table and the 2026 cross-check (hard failure otherwise)."""
    from openpyxl import Workbook

    taxonomy = load_taxonomy(taxonomy_path)
    konton: list[list[Any]] = []
    if profile == "lantbruk-k1":
        if not (k1_list_path and k1_table_path and crosscheck_path):
            raise BuildFailure("profile lantbruk-k1 kräver --k1-list, --k1-table och --crosscheck")
        konton = prefill_konton(
            load_k1_list(k1_list_path),
            read_bas_table(k1_table_path, "--k1-table"),
            read_bas_table(crosscheck_path, "--crosscheck"),
        )
    wb = Workbook()
    wb.remove(wb.active)
    for name, columns in SHEETS.items():
        ws = wb.create_sheet(name)
        ws.append(list(columns))
    readme = wb["README"]
    readme.append(["schema", WORKBOOK_SCHEMA])
    readme.append(["profile", "lantbruk_k1" if profile == "lantbruk-k1" else "synthetic"])
    for key, value in (
        ("chart_id", "LANTBRUK_K1" if profile == "lantbruk-k1" else ""),
        ("chart_version", ""),
        ("ruleset_version", ""),
        ("framework", "BAS" if profile == "lantbruk-k1" else ""),
        ("entity_type", "enskild_firma" if profile == "lantbruk-k1" else ""),
        ("profile_scope", ""),
    ):
        readme.append([key, value])
    if profile == "lantbruk-k1":
        wb["Källor"].append(["BAS_2018_K1", "BAS 2018 för K1 (fullständig)", "BAS-intressenternas Förening", "bas.se/kontoplaner", "2018", "", "", "not confirmed by BAS"])
        wb["Källor"].append(["BAS_2026", "BAS-kontoplan 2026 (cross-check only)", "BAS-intressenternas Förening", "bas.se/kontoplaner", "2026", "", "", "not confirmed by BAS"])
        wb["Källor"].append(["VIBEKE_K1_LIST", "Förslag på enkel kontoplan för skogsägare som bokför enligt K1", "Vibeke (redovisningskonsult)", "PDF 2026-08-11", "2026-08-11", "2026-08-11", "", "author's own list, provided to Valunds"])
    for row in konton:
        wb["Konton"].append(row)
    for c in sorted(taxonomy["categories"], key=lambda x: x["name"]):
        wb["Taxonomi"].append([c["name"], c["group"], c["requires_business_share"], c["investment_risk"], c["vat_check"], c["sensitive"]])
    for i in taxonomy["income_types"]:
        wb["Värdelistor"].append(["income_type", i["value"], i["label"]])
    for pm in taxonomy["payment_methods"]:
        wb["Värdelistor"].append(["payment_method", pm["value"], pm["label"]])
    for lista, vals in (("automation", AUTOMATIONS), ("riktning", DIRECTIONS), ("avdragsrätt", DEDUCTIBILITY), ("kontoklass", KINDS), ("business_group", BUSINESS_GROUPS), ("bokföringsmetod", BOOKKEEPING), ("status", STATUSES), ("granskad_roll", ROLES), ("source_id", SOURCE_IDS), ("must_include_origin", ORIGINS)):
        for v in vals:
            wb["Värdelistor"].append([lista, v, v])
    workbook.parent.mkdir(parents=True, exist_ok=True)
    wb.save(workbook)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="Mastermatris → SIEverk masterdata (SV-02).")
    sub = parser.add_subparsers(dest="command")
    init = sub.add_parser("init-workbook", help="skapa arbetsbok med genererade flikar (och förifylld Konton för lantbruk-k1)")
    init.add_argument("--taxonomy", required=True, type=Path)
    init.add_argument("--workbook", required=True, type=Path)
    init.add_argument("--profile", choices=("synthetic", "lantbruk-k1"), default="synthetic")
    init.add_argument("--k1-list", type=Path)
    init.add_argument("--k1-table", type=Path)
    init.add_argument("--crosscheck", type=Path)
    parser.add_argument("--root", type=Path, help="masterdata-rot (explicit; ingen fallback)")
    parser.add_argument("--workbook", type=Path)
    parser.add_argument("--taxonomy", type=Path)
    parser.add_argument("--k1-list", type=Path, help="Vibekes K1-lista (tracked JSON) — must_include-minimum")
    parser.add_argument("--k1-table", type=Path, help="BAS 2018 för K1 (lokal xlsx) — obligatorisk för profile=lantbruk_k1")
    parser.add_argument("--crosscheck", type=Path, help="BAS 2026 (lokal xlsx) — endast cross-check, obligatorisk för lantbruk_k1")
    parser.add_argument("--out", type=Path)
    mode = parser.add_mutually_exclusive_group()
    mode.add_argument("--draft", action="store_true")
    mode.add_argument("--approved", action="store_true")
    args = parser.parse_args(argv)

    if args.command == "init-workbook":
        try:
            init_workbook(args.taxonomy, args.workbook, args.profile, args.k1_list, args.k1_table, args.crosscheck)
        except BuildFailure as exc:
            print(f"BUILD FAILURE (init-workbook):\n{exc}", file=sys.stderr)
            return 2
        print(f"skapade {args.workbook}", file=sys.stderr)
        return 0
    if not (args.root and args.workbook and args.taxonomy and args.k1_list and args.out and (args.draft or args.approved)):
        parser.error("--root, --workbook, --taxonomy, --k1-list, --out och --draft|--approved är alla obligatoriska")
    mode_name = "approved" if args.approved else "draft"
    try:
        files, rep = build(args.workbook, args.taxonomy, mode_name, args.k1_list, args.k1_table, args.crosscheck)
    except BuildFailure as exc:
        print(f"BUILD FAILURE ({mode_name}):\n{exc}", file=sys.stderr)
        return 2
    args.out.mkdir(parents=True, exist_ok=True)
    for name, data in sorted(files.items()):
        (args.out / name).write_bytes(data)
    write_build_report(args.root, args.out, mode_name, files, rep)
    for w in rep.warnings:
        print(f"warning: {w}", file=sys.stderr)
    print(f"wrote {len(files)} files to {args.out} ({mode_name})", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
