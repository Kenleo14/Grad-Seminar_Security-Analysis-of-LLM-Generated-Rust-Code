#!/usr/bin/env python3
"""
generate_report.py

Reads combined_report.json from an input directory and produces:
1. performance_report.csv: Raw success counts for Stages 1-4.
2. error_summary_report.csv: Detailed failure reasons for each strategy.
3. performance_chart.png: Grouped bar chart of success counts.
"""

import json
import argparse
import os
from pathlib import Path
import pandas as pd
import matplotlib.pyplot as plt
import seaborn as sns

def main():
    parser = argparse.ArgumentParser(description="Generate CSV statistics and visualization.")
    parser.add_argument("--input", required=True, help="Directory containing combined_report.json")
    parser.add_argument("--output", required=True, help="Directory to save CSVs and chart images")
    args = parser.parse_args()

    input_dir = Path(args.input)
    output_dir = Path(args.output)
    report_file = input_dir / "combined_report.json"

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

        # Stage 4 Logic: Success = No heuristic "hits" found for CVE patterns
        cve_manual = file_data.get('cve_2025_68260_manual', {})
        cve_summary = cve_manual.get('summary', {})
        # Success if all monitored counts (unsafe, syscalls, etc) are 0
        stage4_success = all(count == 0 for count in cve_summary.values()) if cve_summary else False

        # Extract success booleans
        s1_ok = file_data.get('rustc_compile', {}).get('ok', False)
        s2_check_ok = file_data.get('cargo_check', {}).get('ok', False)
        s2_clippy_ok = file_data.get('clippy', {}).get('ok', False)
        s3_ok = file_data.get('codeql', {}).get('ok', False) if file_data.get('codeql') else False

        records.append({
            'Strategy': strategy,
            'File': source,
            'Station 1: Raw Compile (rustc)': s1_ok,
            'Station 2: Project Compile (cargo check)': s2_check_ok,
            'Station 2: Idiomatic (cargo clippy)': s2_clippy_ok,
            'Station 3: Security Audit (CodeQL)': s3_ok,
            'Station 4: CVE Mitigation (Heuristic)': stage4_success
        })

        # Capture Error Details
        error_details.append({
            'Strategy': strategy,
            'S1 Error': file_data.get('rustc_compile', {}).get('stderr', '').strip() if not s1_ok else '',
            'S2 Check Error': file_data.get('cargo_check', {}).get('stderr', '').strip() if not s2_check_ok else '',
            'S2 Clippy Issue': file_data.get('clippy', {}).get('stderr', '').strip() if not s2_clippy_ok else '',
            'S3 CodeQL Issue': file_data.get('codeql', {}).get('stderr', '').strip() if file_data.get('codeql') and not s3_ok else '',
            'S4 Heuristic Hits': str(cve_summary) if not stage4_success else ''
        })

    df = pd.DataFrame(records)
    err_df = pd.DataFrame(error_details)

    # 2. Performance Report (Raw Counts)
    perf_summary = df.groupby('Strategy').agg(
        Total_Samples=('File', 'size'),
        Stage1_rustc=('Station 1: Raw Compile (rustc)', 'sum'),
        Stage2_check=('Station 2: Project Compile (cargo check)', 'sum'),
        Stage2_clippy=('Station 2: Idiomatic (cargo clippy)', 'sum'),
        Stage3_CodeQL=('Station 3: Security Audit (CodeQL)', 'sum'),
        Stage4_CVE_Heuristic=('Station 4: CVE Mitigation (Heuristic)', 'sum')
    )
    perf_csv = output_dir / "performance_report.csv"
    perf_summary.to_csv(perf_csv)
    print(f"✅ Performance report saved to: {perf_csv}")

    # 3. Detailed Error Summary
    # We aggregate unique errors per strategy to keep the CSV readable
    error_summary = err_df.groupby('Strategy').agg(lambda x: " | ".join(set(filter(None, x))))
    error_csv = output_dir / "error_summary_report.csv"
    error_summary.to_csv(error_csv)
    print(f"✅ Error details saved to: {error_csv}")

    # 4. Visualization
    plot_df = perf_summary.drop(columns=['Total_Samples']).reset_index()
    melted_df = plot_df.melt(id_vars='Strategy', var_name='Metric', value_name='Success Count')

    sns.set_theme(style="whitegrid")
    plt.figure(figsize=(12, 7))
    ax = sns.barplot(data=melted_df, x='Strategy', y='Success Count', hue='Metric', palette="viridis")

    plt.title('LLM Performance by Prompting Strategy (Raw Success Counts)', fontsize=14, pad=15)
    plt.ylabel('Number of Successful Samples', fontsize=12)
    plt.ylim(0, perf_summary['Total_Samples'].max() + 2)
    plt.legend(title='Evaluation Stage', bbox_to_anchor=(1.05, 1), loc='upper left')

    for p in ax.patches:
        if p.get_height() > 0:
            ax.annotate(f'{int(p.get_height())}', (p.get_x() + p.get_width() / 2., p.get_height()),
                        ha='center', va='bottom', fontsize=9, xytext=(0, 3), textcoords='offset points')

    chart_path = output_dir / "performance_chart.png"
    plt.tight_layout()
    plt.savefig(chart_path, dpi=300)
    print(f"✅ Visualization saved to: {chart_path}")

if __name__ == "__main__":
    main()