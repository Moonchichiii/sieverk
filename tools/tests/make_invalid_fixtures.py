#!/usr/bin/env python3
"""
Skapar fixtures/masterdata/synthetic/invalid/<regel>/ — en trasig, komplett rot per
laddningsregel (SV-02 §5, mastermatris v1.1 §4 "load-time i Rust"). Varje rot är
generated/ med exakt en avsiktlig skada; `sieverk inspect-masterdata --root <rot>`
ska ge exit 1 och tests/chart_contract.rs bevisar att varje rot vägras utan panik.

Deterministiskt: samma generated/ ⇒ samma invalid/. Kör från repo-roten:

    uv run tools/tests/make_invalid_fixtures.py
"""

from __future__ import annotations

import json
import shutil
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GENERATED = ROOT / "fixtures/masterdata/synthetic/generated"
INVALID = ROOT / "fixtures/masterdata/synthetic/invalid"


def _load(root: Path, prefix: str) -> tuple[Path, dict]:
    path = next(root.glob(f"{prefix}*.json"))
    return path, json.loads(path.read_text(encoding="utf-8"))


def _save(path: Path, doc: dict) -> None:
    path.write_text(
        json.dumps(doc, ensure_ascii=False, indent=2, sort_keys=True) + "\n",
        encoding="utf-8",
        newline="\n",
    )


def _case(doc: dict, **match):
    return next(c for c in doc["cases"] if all(c[k] == v for k, v in match.items()))


def _account(doc: dict, number: str):
    return next(a for a in doc["accounts"] if a["number"] == number)


def _approve_all(root: Path) -> None:
    """Flip a copy of the draft root to review_status=approved (all headers + files)."""
    for path in root.glob("*.json"):
        doc = json.loads(path.read_text(encoding="utf-8"))
        doc["_header"]["review_status"] = "approved"
        if "review_status" in doc:
            doc["review_status"] = "approved"
        _save(path, doc)


def v1_unknown_category(root):
    p, d = _load(root, "ruleset-"); _case(d, case_id="bransle.default")["category"] = "Finns inte i taxonomin"; _save(p, d)

def v2_unknown_income_type(root):
    p, d = _load(root, "ruleset-"); _case(d, case_id="income.leveransvirke.default")["income_type"] = "timber_future"; _save(p, d)

def v2_unknown_payment_method(root):
    p, d = _load(root, "counter-accounts-"); d["rows"][0]["key"] = "crypto"; d["rows"][0]["source"] = "receipt"; _save(p, d)

def v3_missing_account(root):
    p, d = _load(root, "ruleset-"); _case(d, case_id="bransle.default")["account"] = "4999"; _save(p, d)

def v3_inactive_account(root):
    p, d = _load(root, "ruleset-"); _case(d, case_id="bransle.default")["account"] = "5999"; _save(p, d)

def v4_duplicate_account(root):
    p, d = _load(root, "chart-"); d["accounts"].append(dict(d["accounts"][0])); _save(p, d)

def v4_bad_class(root):
    p, d = _load(root, "chart-"); _account(d, "1930")["kind"] = "kostnad"; _save(p, d)

def v4_three_digit_account(root):
    p, d = _load(root, "chart-"); _account(d, "1930")["number"] = "193"; _save(p, d)

def v4_unknown_source(root):
    p, d = _load(root, "chart-"); _account(d, "1930")["source_id"] = "NOT_DECLARED"; _save(p, d)

def v6_sru_unknown_account(root):
    p, d = _load(root, "sru-"); d["rows"][0]["account"] = "4999"; _save(p, d)

def v6_sru_overlap(root):
    p, d = _load(root, "sru-"); dup = dict(d["rows"][0]); dup["valid_from"] = 2020; d["rows"].append(dup); _save(p, d)

def v7_bad_vat_rate(root):
    p, d = _load(root, "vat-rules-"); d["rules"][0]["rates"] = ["24"]; _save(p, d)

def v7_unknown_vat_account(root):
    p, d = _load(root, "vat-rules-"); d["rules"][0]["vat_account"] = "2699"; _save(p, d)

def v8_two_defaults(root):
    p, d = _load(root, "ruleset-"); dup = dict(_case(d, case_id="bransle.default")); dup["case_id"] = "bransle.second"; d["cases"].append(dup); _save(p, d)

def v9_approved_without_reviewer(root):
    _approve_all(root)
    p, d = _load(root, "chart-"); a = _account(d, "1930"); a["review"]["status"] = "Godkänd"; a["review"]["reviewed_by"] = None; _save(p, d)

def v10_conditional_without_condition(root):
    p, d = _load(root, "ruleset-"); _case(d, case_id="grus_och_material.default")["condition"] = None; _save(p, d)

def v10_manual_without_question(root):
    p, d = _load(root, "ruleset-"); _case(d, case_id="annat_osakert.default")["question"] = None; _save(p, d)

def v11_duplicate_case_id(root):
    p, d = _load(root, "ruleset-"); d["cases"].append(dict(_case(d, case_id="bransle.default"))); _save(p, d)

def draft_with_automatic(root):
    p, d = _load(root, "ruleset-"); _case(d, case_id="bransle.default")["automation"] = "Automatic"; _save(p, d)

def review_status_mismatch(root):
    p, d = _load(root, "vat-rules-"); d["_header"]["review_status"] = "approved"; _save(p, d)

def chart_reference_mismatch(root):
    p, d = _load(root, "ruleset-"); d["chart"]["version"] = "2027.1"; _save(p, d)

def taxonomy_version_mismatch(root):
    p, d = _load(root, "ruleset-"); d["taxonomy"]["version"] = "0.9"; _save(p, d)

def source_decision_malformed(root):
    p, d = _load(root, "chart-"); _account(d, "1930")["source_decision"] = {"reason": "x", "decided_by": "", "decided_at": "2026-09-02"}; _save(p, d)

def source_version_missing(root):
    p, d = _load(root, "chart-"); del _account(d, "1930")["source_version"]; _save(p, d)

def malformed_json(root):
    p, _ = _load(root, "chart-"); p.write_text("{ this is not json", encoding="utf-8")


FIXTURES = {
    "v1-unknown-category": v1_unknown_category,
    "v2-unknown-income-type": v2_unknown_income_type,
    "v2-unknown-payment-method": v2_unknown_payment_method,
    "v3-missing-account": v3_missing_account,
    "v3-inactive-account": v3_inactive_account,
    "v4-duplicate-account": v4_duplicate_account,
    "v4-bad-class": v4_bad_class,
    "v4-three-digit-account": v4_three_digit_account,
    "v4-unknown-source": v4_unknown_source,
    "v6-sru-unknown-account": v6_sru_unknown_account,
    "v6-sru-overlap": v6_sru_overlap,
    "v7-bad-vat-rate": v7_bad_vat_rate,
    "v7-unknown-vat-account": v7_unknown_vat_account,
    "v8-two-defaults": v8_two_defaults,
    "v9-approved-without-reviewer": v9_approved_without_reviewer,
    "v10-conditional-without-condition": v10_conditional_without_condition,
    "v10-manual-without-question": v10_manual_without_question,
    "v11-duplicate-case-id": v11_duplicate_case_id,
    "source-decision-malformed": source_decision_malformed,
    "source-version-missing": source_version_missing,
    "draft-with-automatic": draft_with_automatic,
    "review-status-mismatch": review_status_mismatch,
    "chart-reference-mismatch": chart_reference_mismatch,
    "taxonomy-version-mismatch": taxonomy_version_mismatch,
    "malformed-json": malformed_json,
}
# V5/V12/V13/V14 are generator-time rules (workbook content / determinism) and have
# no load-time artefact form; they are proven in tools/tests/test_generator.py.


def main() -> int:
    if INVALID.exists():
        shutil.rmtree(INVALID)
    for name, mutate in FIXTURES.items():
        dst = INVALID / name
        shutil.copytree(GENERATED, dst)
        mutate(dst)
    print(f"wrote {len(FIXTURES)} invalid roots to {INVALID}", file=sys.stderr)
    return 0


if __name__ == "__main__":
    sys.exit(main())
