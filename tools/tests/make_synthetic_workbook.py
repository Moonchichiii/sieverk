#!/usr/bin/env python3
"""
Skapar fixtures/masterdata/synthetic/Mastermatris_v1.2-synthetic.xlsx.

Det syntetiska kontraktsfixturet övar exakt samma schema, valideringar och
artefakter som den riktiga masterdatan, men publicerar ingen BAS-härledd data:
kontonamn är antingen påhittade ("Syntetiskt …") eller tagna ur Vibekes egen
K1-lista (2026-08-11), som är vår att använda. Kontonummer följer BAS-strukturen
så att klassregler och referensintegritet testas på riktigt.

Körs från repo-roten (deterministisk: samma kod ⇒ samma arbetsbok, modulo
openpyxl:s zip-tidsstämplar som sätts fasta här):

    uv run --with openpyxl tools/tests/make_synthetic_workbook.py \\
        --taxonomy masterdata/taxonomy-1.0.json \\
        --workbook fixtures/masterdata/synthetic/Mastermatris_v1.2-synthetic.xlsx
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from mastermatris_gen import SHEETS, init_workbook, load_k1_list, load_taxonomy  # noqa: E402

REVIEWER = ("Godkänd", "Syntetisk granskare", "accounting_reviewer", "2026-09-02", "syntetisk fixtur")

K1_LIST_PATH = Path(__file__).resolve().parents[2] / "masterdata" / "vibeke-k1-lista-2026-08-11.json"
# Vibeke's K1 list (2026-08-11) is read from the tracked provenance JSON — one source of truth.
K1_LIST = [(a["number"], a["name"], a["kind"]) for a in load_k1_list(K1_LIST_PATH).values()]
SYNTHETIC_ACCOUNTS = [
    ("1910", "Syntetiskt kassakonto", "tillgång"),
    ("2440", "Syntetiskt leverantörsskuldkonto", "skuld"),
    ("3740", "Syntetiskt öresutjämningskonto", "intäkt"),
    ("5999", "Syntetiskt inaktivt konto", "kostnad"),
]
CLOSING = {"7821", "7830", "8999", "2019"}


def account_row(number: str, name: str, kind: str, *, synthetic: bool) -> list:
    active = number != "5999"
    return [
        number, name, kind, active, number not in CLOSING, active and number not in CLOSING, True, number in CLOSING,
        "", not synthetic, "" if synthetic else "VIBEKE_K1_LIST", "SYNTHETIC" if synthetic else "VIBEKE_K1_LIST",
        "2026.1" if synthetic else "2026-08-11", "", *REVIEWER,
    ]


def rows() -> dict[str, list[list]]:
    return {
        "README": [
            ["schema", "1.2"], ["profile", "synthetic"], ["chart_id", "SYNTHETIC_K1"], ["chart_version", "2026.1"], ["ruleset_version", "2026.1"],
            ["framework", "BAS-struktur (syntetisk)"], ["entity_type", "enskild_firma"],
            ["profile_scope", "syntetiskt kontraktsfixtur — inte redovisningsmässigt granskat"],
        ],
        "Källor": [
            ["SYNTHETIC", "Syntetiska konton", "sieverk", "fixtures/masterdata/synthetic", "2026.1", "", "", "synthetic — no third-party rights"],
            ["VIBEKE_K1_LIST", "Förslag på enkel kontoplan för skogsägare som bokför enligt K1", "Vibeke (redovisningskonsult)", "PDF 2026-08-11", "2026-08-11", "2026-08-11", "", "author's own list, provided to Valunds"],
        ],
        "Konton": [account_row(n, name, kind, synthetic=False) for n, name, kind in K1_LIST]
        + [account_row(n, name, kind, synthetic=True) for n, name, kind in SYNTHETIC_ACCOUNTS],
        "SRU": [
            ["3420", "9101", "NE", 2026, "", "SYNTHETIC", "2026.1", "", *REVIEWER],
            ["4470", "9102", "NE", 2026, "", "SYNTHETIC", "2026.1", "", *REVIEWER],
        ],
        "Momsregler": [
            ["ing25", "Ingående moms 25 %", "ingående", "25", "2640", "full", "", False, *REVIEWER],
            ["ing12", "Ingående moms 12 %", "ingående", "12", "2640", "full", "", False, *REVIEWER],
            ["ing_manuell", "Ingående moms, manuell avdragsrätt", "ingående", "25;12;6;0", "2640", "manuell", "avdragsrätt avgörs av konsult", True, *REVIEWER],
            ["utg25", "Utgående moms 25 %", "utgående", "25", "2610", "full", "", False, *REVIEWER],
        ],
        "Motkonton": [
            ["receipt", "company_account", "", "1930", "Automatic", "", *REVIEWER],
            ["receipt", "private", "", "2013", "Automatic", "", *REVIEWER],
            ["receipt", "unknown", "", "", "Manual", "Hur betalades kvittot?", *REVIEWER],
            ["receipt", "supplier_credit", "cash", "", "Manual", "Är fakturan betald, och i så fall när?", *REVIEWER],
            ["receipt", "supplier_credit", "invoice", "2440", "Conditional", "Bokförs skulden vid fakturadatum?", *REVIEWER],
            ["income", "betald", "", "1930", "Automatic", "", *REVIEWER],
            ["income", "obetald", "cash", "", "Manual", "Är inkomsten betald?", *REVIEWER],
        ],
        "Redovisningsfall": [
            ["bransle.default", "receipt", "Bränsle", "", True, "Automatic", "5360", "ing25", "enligt_motkonton", "", "", "", "Bränsle till maskin/fordon", *REVIEWER],
            ["skogsvard.default", "receipt", "Skogsvård", "", True, "Automatic", "4470", "ing25", "enligt_motkonton", "", "", "", "Skogsvård via entreprenör", *REVIEWER],
            ["grus_och_material.default", "receipt", "Grus och material", "", True, "Conditional", "5180", "ing25", "enligt_motkonton", "", "Underhåll av befintlig anläggning", "Underhåll eller ny anläggning?", "Grus kan vara underhåll eller investering", *REVIEWER],
            ["annat_osakert.default", "receipt", "Annat / osäkert", "", True, "Manual", "", "", "enligt_motkonton", "", "", "Vad avser köpet/intäkten?", "Konsulten avgör", *REVIEWER],
            ["income.leveransvirke.default", "income", "", "leveransvirke", True, "Automatic", "3420", "utg25", "enligt_motkonton", "", "", "", "Leveransvirke", *REVIEWER],
            ["income.efterlikvid.default", "income", "", "efterlikvid", True, "Conditional", "3493", "utg25", "enligt_motkonton", "", "Avräkning med betaldatum", "Har efterlikviden betalats ut?", "Efterlikvid", *REVIEWER],
        ],
        "Tester": [
            ["ex-bransle-1", "bransle.default", "1250.00", "250.00", "0.00", "company_account", "", True, "cash", True, "Automatic",
             "5360:debet:1000.00;2640:debet:250.00;1930:kredit:1250.00", "", *REVIEWER],
            ["ex-skogsvard-1", "skogsvard.default", "31250.00", "6250.00", "0.00", "company_account", "", True, "cash", True, "Automatic",
             "4470:debet:25000.00;2640:debet:6250.00;1930:kredit:31250.00", "", *REVIEWER],
            ["ex-leveransvirke-1", "income.leveransvirke.default", "125000.00", "25000.00", "0.00", "", "leveransvirke", True, "cash", True, "Automatic",
             "1930:debet:125000.00;3420:kredit:100000.00;2610:kredit:25000.00", "", *REVIEWER],
        ],
    }


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--taxonomy", required=True, type=Path)
    parser.add_argument("--workbook", required=True, type=Path)
    args = parser.parse_args(argv)
    from openpyxl import load_workbook

    init_workbook(args.taxonomy, args.workbook)
    wb = load_workbook(args.workbook)
    load_taxonomy(args.taxonomy)
    for sheet, data in rows().items():
        ws = wb[sheet]
        if sheet == "README":
            ws.delete_rows(2, ws.max_row)
        for row in data:
            assert len(row) == len(SHEETS[sheet]), (sheet, row)
            ws.append(row)
    # openpyxl stamps the zip with the current time; pin it so the fixture is reproducible.
    wb.properties.created = wb.properties.modified = __import__("datetime").datetime(2026, 9, 2, 0, 0, 0)
    wb.save(args.workbook)
    print(f"wrote {args.workbook}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
