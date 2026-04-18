#!/usr/bin/env python3
"""
Generate manual_review.csv pre-filled with triage columns from grep patterns.
Leaves inner_lock, lock_on_unlink, verdict, notes blank for manual review.
"""

import csv
import re
import os
from pathlib import Path

SAMPLES_ROOT = Path("Samples")
OUTPUT_CSV = Path("manual_review.csv")

# Triage patterns
PATTERNS = {
    "raw_ptr_list": re.compile(r"list_del|\.next\s*=|\.prev\s*="),
    "has_unsafe":   re.compile(r"\bunsafe\b"),
    "arc_from_raw": re.compile(r"Arc::from_raw|from_raw"),
}

# Map folder name fragments to readable labels
LLM_MAP = {
    "chatgpt": "chatgpt",
    "gemini":  "gemini",
    "grok":    "grok",
}

STRATEGY_MAP = {
    "chainThought": "chainThought",
    "constraintBased": "constraintBased",
    "zeroShot": "zeroShot",
}

def detect_llm(path: Path) -> str:
    for part in path.parts:
        for key in LLM_MAP:
            if key in part.lower():
                return LLM_MAP[key]
    return "unknown"

def detect_strategy(path: Path) -> str:
    for part in path.parts:
        for key in STRATEGY_MAP:
            if key.lower() in part.lower():
                return STRATEGY_MAP[key]
    return "unknown"

def check_file(path: Path) -> dict[str, str]:
    text = path.read_text(errors="replace")
    return {
        "raw_ptr_list": "yes" if PATTERNS["raw_ptr_list"].search(text) else "no",
        "has_unsafe":   "yes" if PATTERNS["has_unsafe"].search(text) else "no",
        "arc_from_raw": "yes" if PATTERNS["arc_from_raw"].search(text) else "no",
    }

def main():
    rs_files = sorted(SAMPLES_ROOT.rglob("*.rs"))
    if not rs_files:
        print(f"No .rs files found under {SAMPLES_ROOT}/")
        return

    fieldnames = [
        "file", "llm", "strategy",
        "raw_ptr_list", "has_unsafe", "arc_from_raw",
        "inner_lock", "lock_on_unlink", "verdict", "notes",
    ]

    rows = []
    for f in rs_files:
        triage = check_file(f)
        rows.append({
            "file":           str(f),
            "llm":            detect_llm(f),
            "strategy":       detect_strategy(f),
            "raw_ptr_list":   triage["raw_ptr_list"],
            "has_unsafe":     triage["has_unsafe"],
            "arc_from_raw":   triage["arc_from_raw"],
            "inner_lock":     "",
            "lock_on_unlink": "",
            "verdict":        "",
            "notes":          "",
        })

    with open(OUTPUT_CSV, "w", newline="") as f:
        writer = csv.DictWriter(f, fieldnames=fieldnames)
        writer.writeheader()
        writer.writerows(rows)

    yes_raw  = sum(1 for r in rows if r["raw_ptr_list"] == "yes")
    yes_unsafe = sum(1 for r in rows if r["has_unsafe"] == "yes")
    yes_arc  = sum(1 for r in rows if r["arc_from_raw"] == "yes")

    print(f"Written {len(rows)} rows to {OUTPUT_CSV}")
    print(f"  raw_ptr_list = yes : {yes_raw}/{len(rows)}")
    print(f"  has_unsafe   = yes : {yes_unsafe}/{len(rows)}")
    print(f"  arc_from_raw = yes : {yes_arc}/{len(rows)}")

if __name__ == "__main__":
    main()
