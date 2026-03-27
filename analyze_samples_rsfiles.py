#!/usr/bin/env python3
"""
analyze_samples_rsfiles.py

Pipeline (per .rs file):
1) Compile directly with rustc (best-effort; failures do not stop later steps)
2) Create a temporary minimal Cargo project per .rs file
   - cargo check
   - cargo clippy (configurable strictness)
   - optional cargo test (only with --run-tests)
3) Optional CodeQL (only with --run-codeql, requires --codeql-suite)
4) Manual heuristic CVE-2025-68260 scan (keyword/unsafe/FFI/syscall indicators)

Windows notes:
- rustc compile runs with cwd=rs_file.parent and uses rs_file.name to avoid path duplication.
- rustc -o uses an ABSOLUTE output path to avoid rustc trying to create rmeta temp dirs under
  Samples/<model>/out/... (which can fail if those relative paths don't exist).
"""

import argparse
import hashlib
import json
import re
import shutil
import subprocess
import sys
import tempfile
from dataclasses import dataclass, asdict
from pathlib import Path
from typing import Dict, List, Optional, Any, Tuple


SUBFOLDERS_DEFAULT = ["chatgpt", "gemini", "grok"]

STDLIKE = {"std", "core", "alloc", "crate", "self", "super", "test"}
USE_RX = re.compile(r"(?m)^\s*use\s+([A-Za-z_][A-Za-z0-9_]*)::")
EXTERN_CRATE_RX = re.compile(r"(?m)^\s*extern\s+crate\s+([A-Za-z_][A-Za-z0-9_]*)\s*;")
EDITION_RX = re.compile(r"(?m)^\s*//\s*edition\s*:\s*(2015|2018|2021|2024)\s*$")


@dataclass
class StepResult:
    ok: bool
    cmd: List[str]
    cwd: str
    returncode: int
    stdout: str
    stderr: str


@dataclass
class FileReport:
    source_file: str
    inferred_deps: Dict[str, str]
    rustc_compile: StepResult
    cargo_check: StepResult
    clippy: StepResult
    cargo_test: Optional[StepResult]
    codeql: Optional[StepResult]
    cve_2025_68260_manual: Dict[str, Any]


def run_cmd(cmd: List[str], cwd: Path, timeout_s: int) -> StepResult:
    try:
        p = subprocess.run(
            cmd,
            cwd=str(cwd),
            capture_output=True,
            text=True,
            timeout=timeout_s,
            check=False,
        )
        return StepResult(
            ok=(p.returncode == 0),
            cmd=cmd,
            cwd=str(cwd),
            returncode=p.returncode,
            stdout=p.stdout,
            stderr=p.stderr,
        )
    except FileNotFoundError as e:
        return StepResult(
            ok=False,
            cmd=cmd,
            cwd=str(cwd),
            returncode=127,
            stdout="",
            stderr=str(e),
        )
    except subprocess.TimeoutExpired as e:
        return StepResult(
            ok=False,
            cmd=cmd,
            cwd=str(cwd),
            returncode=124,
            stdout=e.stdout or "",
            stderr=(e.stderr or "") + f"\nTIMEOUT after {timeout_s}s",
        )


def stable_id_for_file(path: Path) -> str:
    h = hashlib.sha256(str(path).encode("utf-8")).hexdigest()
    return h[:12]


def find_rs_files(samples_root: Path, subfolders: List[str]) -> List[Path]:
    files: List[Path] = []
    for sub in subfolders:
        d = samples_root / sub
        if not d.exists():
            continue
        for p in d.rglob("*.rs"):
            if "target" in p.parts:
                continue
            files.append(p)
    return sorted(set(files))


def ensure_tools_present(run_codeql: bool) -> None:
    missing = []
    for tool in ["cargo", "rustc"]:
        if shutil.which(tool) is None:
            missing.append(tool)
    if run_codeql and shutil.which("codeql") is None:
        missing.append("codeql")
    if missing:
        raise RuntimeError(f"Missing required tools on PATH: {', '.join(missing)}")


def infer_deps_from_rs(rs_text: str) -> Dict[str, str]:
    crates = set()
    for m in USE_RX.finditer(rs_text):
        crates.add(m.group(1))
    for m in EXTERN_CRATE_RX.finditer(rs_text):
        crates.add(m.group(1))
    crates = {c for c in crates if c not in STDLIKE}
    return {c: "*" for c in sorted(crates)}


def pick_lib_or_bin(rs_text: str) -> str:
    return "main" if re.search(r"(?m)^\s*fn\s+main\s*\(", rs_text) else "lib"


def detect_edition(rs_text: str) -> str:
    m = EDITION_RX.search(rs_text)
    return m.group(1) if m else "2021"


def manual_cve_2025_68260_scan_rsfile(rs_file: Path) -> Dict[str, Any]:
    patterns = {
        "unsafe": re.compile(r"\bunsafe\b"),
        "ffi_extern_c": re.compile(r'\bextern\s+"C"\b'),
        "raw_syscalls_ioctl": re.compile(r"\b(ioctl|syscall|prctl)\b"),
        "kernelish_apis": re.compile(r"\b(netlink|bpf|perf_event_open|setsockopt|getsockopt)\b"),
        "dev_paths": re.compile(r"(/dev/[\w\-/]+)"),
        "use_libc": re.compile(r"\blibc::|\buse\s+libc\b"),
        "use_nix": re.compile(r"\bnix::|\buse\s+nix\b"),
        "android_binder_words": re.compile(r"\b(binder|ashmem|ion)\b", re.IGNORECASE),
    }

    try:
        text = rs_file.read_text(encoding="utf-8", errors="replace")
    except Exception as e:
        return {"cve": "CVE-2025-68260", "error": str(e), "summary": {}, "hits": {}}

    hits: Dict[str, List[Dict[str, Any]]] = {k: [] for k in patterns.keys()}
    lines = text.splitlines()

    for key, rx in patterns.items():
        for m in rx.finditer(text):
            line_no = text.count("\n", 0, m.start()) + 1
            snippet = lines[line_no - 1][:300] if 1 <= line_no <= len(lines) else ""
            hits[key].append(
                {"file": rs_file.name, "line": line_no, "match": m.group(0), "snippet": snippet}
            )

    summary = {k: len(v) for k, v in hits.items()}
    return {
        "cve": "CVE-2025-68260",
        "note": "Heuristic scan only. Provide CVE specifics to implement targeted checks.",
        "summary": summary,
        "hits": hits,
    }


def rustc_compile_best_effort(rs_file: Path, out_dir: Path, timeout_s: int) -> StepResult:
    """
    Compile directly with rustc (best-effort).
    Uses absolute -o output path to avoid Windows relative-path temp-dir failures.
    """
    out_dir_abs = out_dir.resolve()
    out_dir_abs.mkdir(parents=True, exist_ok=True)

    bin_path = (out_dir_abs / (rs_file.stem + ".bin")).resolve()

    return run_cmd(
        ["rustc", rs_file.name, "-o", str(bin_path)],
        cwd=rs_file.parent,
        timeout_s=timeout_s,
    )


def write_temp_cargo_project(tmpdir: Path, rs_file: Path) -> Tuple[Dict[str, str], str]:
    rs_text = rs_file.read_text(encoding="utf-8", errors="replace")
    deps = infer_deps_from_rs(rs_text)
    edition = detect_edition(rs_text)
    crate_kind = pick_lib_or_bin(rs_text)

    (tmpdir / "src").mkdir(parents=True, exist_ok=True)
    dest = tmpdir / "src" / ("main.rs" if crate_kind == "main" else "lib.rs")
    shutil.copyfile(rs_file, dest)

    pkg_name = f"sample_{stable_id_for_file(rs_file)}"
    dep_lines = [f'{name} = "{ver}"' for name, ver in deps.items()]

    cargo_toml = "\n".join(
        [
            "[package]",
            f'name = "{pkg_name}"',
            'version = "0.1.0"',
            f'edition = "{edition}"',
            "",
            "[dependencies]",
            *dep_lines,
            "",
        ]
    )
    (tmpdir / "Cargo.toml").write_text(cargo_toml, encoding="utf-8")
    return deps, crate_kind


def run_cargo_steps_for_rsfile(
    rs_file: Path,
    timeout_s: int,
    run_tests: bool,
    clippy_deny_warnings: bool,
) -> Tuple[Dict[str, str], StepResult, StepResult, Optional[StepResult]]:
    with tempfile.TemporaryDirectory(prefix="rs-sample-cargo-") as td:
        tmpdir = Path(td)
        deps, _kind = write_temp_cargo_project(tmpdir, rs_file)

        check = run_cmd(["cargo", "check"], cwd=tmpdir, timeout_s=timeout_s)

        clippy_cmd = ["cargo", "clippy", "--all-targets", "--all-features", "--"]
        if clippy_deny_warnings:
            clippy_cmd += ["-D", "warnings"]
        clippy = run_cmd(clippy_cmd, cwd=tmpdir, timeout_s=timeout_s)

        test_res = None
        if run_tests:
            test_res = run_cmd(["cargo", "test"], cwd=tmpdir, timeout_s=timeout_s)

        return deps, check, clippy, test_res


def codeql_on_rsfile(rs_file: Path, out_dir: Path, timeout_s: int, query_suite: str) -> StepResult:
    out_dir.mkdir(parents=True, exist_ok=True)
    db_dir = out_dir / "codeql-db"
    sarif = out_dir / "codeql-results.sarif"

    if db_dir.exists():
        shutil.rmtree(db_dir)

    with tempfile.TemporaryDirectory(prefix="rs-sample-codeql-") as td:
        tmpdir = Path(td)
        write_temp_cargo_project(tmpdir, rs_file)

        create = run_cmd(
            [
                "codeql",
                "database",
                "create",
                str(db_dir),
                "--language=rust",
                "--command",
                "cargo build",
            ],
            cwd=tmpdir,
            timeout_s=timeout_s,
        )
        if not create.ok:
            return create

        analyze = run_cmd(
            [
                "codeql",
                "database",
                "analyze",
                str(db_dir),
                query_suite,
                "--format=sarifv2.1.0",
                f"--output={sarif}",
            ],
            cwd=tmpdir,
            timeout_s=timeout_s,
        )
        return analyze


def analyze_file(rs_file: Path, args: argparse.Namespace) -> FileReport:
    rustc_res = rustc_compile_best_effort(
        rs_file, Path(args.output) / "rustc_bins", timeout_s=args.timeout
    )

    deps, check_res, clippy_res, test_res = run_cargo_steps_for_rsfile(
        rs_file=rs_file,
        timeout_s=args.timeout,
        run_tests=args.run_tests,
        clippy_deny_warnings=args.clippy_deny_warnings,
    )

    codeql_res = None
    if args.run_codeql:
        if not args.codeql_suite:
            codeql_res = StepResult(
                ok=False,
                cmd=["codeql", "database", "analyze", "<db>", "<suite>"],
                cwd=str(rs_file.parent),
                returncode=2,
                stdout="",
                stderr="CodeQL skipped: --run-codeql set but --codeql-suite not provided.",
            )
        else:
            per_file_out = Path(args.output) / "codeql" / stable_id_for_file(rs_file)
            codeql_res = codeql_on_rsfile(
                rs_file=rs_file,
                out_dir=per_file_out,
                timeout_s=args.timeout,
                query_suite=args.codeql_suite,
            )

    manual = manual_cve_2025_68260_scan_rsfile(rs_file)

    return FileReport(
        source_file=str(rs_file),
        inferred_deps=deps,
        rustc_compile=rustc_res,
        cargo_check=check_res,
        clippy=clippy_res,
        cargo_test=test_res,
        codeql=codeql_res,
        cve_2025_68260_manual=manual,
    )


def main() -> int:
    ap = argparse.ArgumentParser(
        description=(
            "Analyze Rust .rs samples under Samples/{chatgpt,gemini,grok}: "
            "rustc compile (best-effort), temp Cargo (check+clippy), optional tests, "
            "optional CodeQL, heuristic CVE scan."
        )
    )
    ap.add_argument("--samples-root", default="Samples", help='Path to "Samples" directory (default: Samples)')
    ap.add_argument("--output", default="out", help="Output directory (default: out)")
    ap.add_argument("--timeout", type=int, default=900, help="Timeout per command in seconds (default: 900)")
    ap.add_argument("--run-tests", action="store_true", help="Run cargo test per sample (slower)")
    ap.add_argument("--run-codeql", action="store_true", help="Enable CodeQL (requires codeql CLI)")
    ap.add_argument("--codeql-suite", default="", help="CodeQL suite (.qls path or pack reference). Required if --run-codeql.")
    ap.add_argument(
        "--clippy-deny-warnings",
        action="store_true",
        help="Fail clippy step on warnings (passes '-D warnings').",
    )
    ap.add_argument(
        "--subfolders",
        default=",".join(SUBFOLDERS_DEFAULT),
        help="Comma-separated list of subfolders under --samples-root to scan (default: chatgpt,gemini,grok)",
    )
    args = ap.parse_args()

    subfolders = [s.strip() for s in args.subfolders.split(",") if s.strip()]
    if not subfolders:
        print("No subfolders specified via --subfolders.", file=sys.stderr)
        return 2

    ensure_tools_present(run_codeql=args.run_codeql)

    samples_root = Path(args.samples_root)
    out_dir = Path(args.output)
    out_dir.mkdir(parents=True, exist_ok=True)

    rs_files = find_rs_files(samples_root, subfolders)
    if not rs_files:
        print(f"No .rs files found under {samples_root}/{{{', '.join(subfolders)}}}", file=sys.stderr)
        return 2

    reports: List[FileReport] = []
    for f in rs_files:
        print(f"==> {f}")
        rep = analyze_file(f, args)
        reports.append(rep)

        per_file_dir = out_dir / "reports" / stable_id_for_file(f)
        per_file_dir.mkdir(parents=True, exist_ok=True)
        (per_file_dir / "report.json").write_text(json.dumps(asdict(rep), indent=2), encoding="utf-8")

    combined = {"samples_root": str(samples_root), "files": [asdict(r) for r in reports]}
    (out_dir / "combined_report.json").write_text(json.dumps(combined, indent=2), encoding="utf-8")

    print("\nSummary:")
    for r in reports:
        rc = "OK" if r.rustc_compile.ok else "FAIL"
        chk = "OK" if r.cargo_check.ok else "FAIL"
        clip = "OK" if r.clippy.ok else "FAIL"
        tst = "SKIP" if r.cargo_test is None else ("OK" if r.cargo_test.ok else "FAIL")
        cql = "SKIP" if r.codeql is None else ("OK" if r.codeql.ok else "FAIL")
        print(f"- {r.source_file}: rustc={rc}, check={chk}, clippy={clip}, test={tst}, codeql={cql}")

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
