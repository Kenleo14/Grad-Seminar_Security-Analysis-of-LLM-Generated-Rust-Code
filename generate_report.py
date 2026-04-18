#!/usr/bin/env python3
"""
generate_report.py

Reads combined_report.json from an input directory and produces:
1. performance_report.csv: Raw success counts for Stages 1-4.
2. error_summary_report.csv: Detailed failure reasons for each strategy.
3. performance_chart.png: Grouped bar chart of success counts.
"""

import hashlib
import json
import argparse
import os
from pathlib import Path
import pandas as pd
import matplotlib.pyplot as plt
import seaborn as sns


def _stable_id(path_str: str) -> str:
    return hashlib.sha256(path_str.encode("utf-8")).hexdigest()[:12]


CVE_RULE_ID = "rust/lock-drop-before-list-traversal"
SUMMARY_PREFIX = ("rust/summary/", "rust/telemetry/")


def _load_sarif(sarif_path: Path):
    """Parse a SARIF file and return findings split by source.

    Returns:
        extended_count  — findings from rust-security-extended (excludes CVE custom rule)
        extended_rules  — sorted unique rule IDs from rust-security-extended
        cve_count       — findings from CVE-2025-68260 custom query only
        cve_rules       — sorted unique rule IDs from custom query (usually just one)
    """
    if not sarif_path.exists():
        return 0, [], 0, []
    try:
        with open(sarif_path, encoding="utf-8") as f:
            data = json.load(f)
        results = [r for run in data.get("runs", []) for r in run.get("results", [])]

        extended_ids, cve_ids = set(), set()
        extended_count, cve_count = 0, 0

        for r in results:
            rid = r.get("ruleId", "")
            if not rid or any(rid.startswith(p) for p in SUMMARY_PREFIX):
                continue
            if rid == CVE_RULE_ID:
                cve_ids.add(rid)
                cve_count += 1
            else:
                extended_ids.add(rid)
                extended_count += 1

        return extended_count, sorted(extended_ids), cve_count, sorted(cve_ids)
    except Exception:
        return 0, [], 0, []

def main():
    parser = argparse.ArgumentParser(description="Generate CSV statistics and visualization.")
    parser.add_argument("--input", required=True, help="Directory containing combined_report.json")
    parser.add_argument("--output", required=True, help="Directory to save CSVs and chart images")
    args = parser.parse_args()

    input_dir = Path(args.input)
    output_dir = Path(args.output)
    report_file = input_dir / "combined_report.json"

    LLM_NAMES = {"chatgpt": "ChatGPT", "gemini": "Gemini", "grok": "Grok"}
    folder_parts = [p.lower() for p in input_dir.parts]
    llm_title = next((LLM_NAMES[k] for k in LLM_NAMES if k in folder_parts), input_dir.name)

    if not report_file.exists():
        print(f"Error: {report_file} not found.")
        return

    output_dir.mkdir(parents=True, exist_ok=True)

    # 1. Load the Data
    with open(report_file, 'r', encoding='utf-8') as f:
        data = json.load(f)

    records = []
    error_details = []

    for file_data in data.get('files', []):
        source = file_data.get('source_file', '')
        
        # Determine strategy
        if 'zeroShot' in source:
            strategy = 'Zero-Shot'
        elif 'chainThought' in source:
            strategy = 'Chain-of-Thought'
        elif 'constraintBased' in source:
            strategy = 'Constraint-Based'
        else:
            strategy = 'Unknown'

        cve_manual = file_data.get('cve_2025_68260_manual', {})
        cve_summary = cve_manual.get('summary', {})

        # Extract success booleans
        s2_check_ok = file_data.get('cargo_check', {}).get('ok', False)
        s2_clippy_ok = file_data.get('clippy', {}).get('ok', False)
        s3_ok = file_data.get('codeql', {}).get('ok', False) if file_data.get('codeql') else False

        # Load CodeQL SARIF findings split by source
        sarif_path = input_dir / "codeql" / _stable_id(source) / "codeql-results.sarif"
        ext_count, ext_rules, cve_count, cve_rules = _load_sarif(sarif_path)
        ext_findings_str = ", ".join(ext_rules) if ext_rules else ("none" if s3_ok else "")
        cve_findings_str = ", ".join(cve_rules) if cve_rules else ("none" if s3_ok else "")

        unsafe_count = cve_summary.get('unsafe', 0)

        records.append({
            'Strategy': strategy,
            'File': source,
            'Station 2: Project Compile (cargo check)': s2_check_ok,
            'Station 2: Idiomatic (cargo clippy)': s2_clippy_ok,
            'Station 3: CodeQL (CWE) Pass': int(s3_ok and ext_count == 0),
            'Station 3: CodeQL (CVE) Pass': int(s3_ok and cve_count == 0),
            'Station 3: rust-security-extended Findings': ext_count,
            'Station 3: rust-security-extended Rules': ext_findings_str,
            'Station 3: CVE-2025-68260 Hits': cve_count,
            'Station 3: CVE-2025-68260 Rules': cve_findings_str,
            'Station 4: Unsafe Block Count': unsafe_count,
        })

        # Capture Error Details
        codeql_data = file_data.get('codeql') or {}
        codeql_stderr = codeql_data.get('stderr', '').strip()
        if s3_ok:
            s3_issue = ""
        elif "skipped" in codeql_stderr.lower():
            s3_issue = codeql_stderr.split("\n")[0]   # first line only ("CodeQL skipped: ...")
        else:
            s3_issue = codeql_stderr

        error_details.append({
            'Strategy': strategy,
            'File': source,
            'S2 Check Error': file_data.get('cargo_check', {}).get('stderr', '').strip() if not s2_check_ok else '',
            'S2 Clippy Issue': file_data.get('clippy', {}).get('stderr', '').strip() if not s2_clippy_ok else '',
            'S3 CodeQL Issue': s3_issue,
            'S3 rust-security-extended Findings': ext_findings_str,
            'S3 CVE-2025-68260 Findings': cve_findings_str,
            'S4 Heuristic Hits': str(cve_summary) if any(cve_summary.values()) else ''
        })

    df = pd.DataFrame(records)
    err_df = pd.DataFrame(error_details)

    # 2. Performance Report (Raw Counts)
    perf_summary = df.groupby('Strategy').agg(
        Total_Samples=('File', 'size'),
        Stage2_check=('Station 2: Project Compile (cargo check)', 'sum'),
        Stage2_clippy=('Station 2: Idiomatic (cargo clippy)', 'sum'),
        Stage3_Pass_CWE=('Station 3: CodeQL (CWE) Pass', 'sum'),
        Stage3_Pass_CVE=('Station 3: CodeQL (CVE) Pass', 'sum'),
        Stage3_Extended_Findings=('Station 3: rust-security-extended Findings', 'sum'),
        Stage3_CVE_2025_68260_Hits=('Station 3: CVE-2025-68260 Hits', 'sum'),
        Stage4_Unsafe_Count=('Station 4: Unsafe Block Count', 'sum'),
    )
    perf_csv = output_dir / "performance_report.csv"
    perf_summary.to_csv(perf_csv)
    print(f"✅ Performance report saved to: {perf_csv}")

    # 3. Detailed Error Summary — one row per file, not aggregated, so findings are visible
    error_csv = output_dir / "error_summary_report.csv"
    err_df.to_csv(error_csv, index=False)
    print(f"✅ Error details saved to: {error_csv}")

    # 4. Visualization — two subplots (different Y-axis scales)
    sns.set_theme(style="whitegrid")
    fig, (ax1, ax2) = plt.subplots(2, 1, figsize=(13, 11))
    fig.suptitle(f'{llm_title} Performance by Prompting Strategy', fontsize=15, fontweight='bold', y=0.98)

    # --- Subplot 1: Pass counts (Stage 2 + S3 split by CWE vs CVE query) ---
    pass_cols = ['Stage2_check', 'Stage2_clippy', 'Stage3_Pass_CWE', 'Stage3_Pass_CVE']
    pass_labels = {
        'Stage2_check':      'S2: Project Compile (cargo check)',
        'Stage2_clippy':     'S2: Idiomatic (cargo clippy)',
        'Stage3_Pass_CWE':   'S3: CodeQL (CWE) — No Findings',
        'Stage3_Pass_CVE':   'S3: CodeQL (CVE-2025-68260) — No Findings',
    }
    pass_df = perf_summary[pass_cols].rename(columns=pass_labels).reset_index()
    melted_pass = pass_df.melt(id_vars='Strategy', var_name='Stage', value_name='Samples Passing')

    palette1 = ["#4CAF50", "#FF9800", "#9C27B0", "#1565C0"]
    sns.barplot(data=melted_pass, x='Strategy', y='Samples Passing',
                hue='Stage', palette=palette1, ax=ax1)
    ax1.set_title('Compilation & Static Analysis — Pass Counts', fontsize=12)
    ax1.set_ylabel('Number of Samples Passing', fontsize=11)
    ax1.set_xlabel('')
    ax1.set_ylim(0, perf_summary['Total_Samples'].max() + 2)
    ax1.legend(title='Evaluation Stage', bbox_to_anchor=(1.01, 1), loc='upper left', fontsize=9)
    for p in ax1.patches:
        if p.get_height() > 0:
            ax1.annotate(f'{int(p.get_height())}',
                         (p.get_x() + p.get_width() / 2., p.get_height()),
                         ha='center', va='bottom', fontsize=9, xytext=(0, 3),
                         textcoords='offset points')

    # --- Subplot 2: Count metrics (split by source + unsafe block count) ---
    count_cols = ['Stage3_Extended_Findings', 'Stage3_CVE_2025_68260_Hits', 'Stage4_Unsafe_Count']
    count_labels = {
        'Stage3_Extended_Findings':    'S3: rust-security-extended Findings',
        'Stage3_CVE_2025_68260_Hits':  'S3: CVE-2025-68260 Custom Query Hits',
        'Stage4_Unsafe_Count':         'S4: Unsafe Block Count',
    }
    count_df = perf_summary[count_cols].rename(columns=count_labels).reset_index()
    melted_count = count_df.melt(id_vars='Strategy', var_name='Metric', value_name='Count')

    palette2 = ["#F44336", "#1565C0", "#FF9800"]  # red, dark-blue, orange
    sns.barplot(data=melted_count, x='Strategy', y='Count',
                hue='Metric', palette=palette2, ax=ax2)
    ax2.set_title('Security Metrics — Counts per Strategy', fontsize=12)
    ax2.set_ylabel('Count (summed across samples)', fontsize=11)
    ax2.set_xlabel('Prompting Strategy', fontsize=11)
    ax2.legend(title='Metric', bbox_to_anchor=(1.01, 1), loc='upper left', fontsize=9)
    for p in ax2.patches:
        if p.get_height() > 0:
            ax2.annotate(f'{int(p.get_height())}',
                         (p.get_x() + p.get_width() / 2., p.get_height()),
                         ha='center', va='bottom', fontsize=9, xytext=(0, 3),
                         textcoords='offset points')

    chart_path = output_dir / "performance_chart.png"
    plt.tight_layout()
    plt.savefig(chart_path, dpi=300, bbox_inches='tight')
    print(f"✅ Visualization saved to: {chart_path}")

if __name__ == "__main__":
    main()