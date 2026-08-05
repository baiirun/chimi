"""Terminal shell for the disposable Go-versus-Rust compiler host spike."""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import platform
import re
import shutil
import subprocess
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path
from typing import Any


BOLD = "\x1b[1m"
DIM = "\x1b[2m"
RESET = "\x1b[0m"
ROOT = Path(__file__).resolve().parents[2]
SPIKE = Path(__file__).resolve().parent


@dataclass(frozen=True)
class Candidate:
    name: str
    source: Path


@dataclass
class CandidateResult:
    name: str
    fresh_build_seconds: float
    edit_build_seconds: float
    binary_bytes: int
    peak_rss_bytes: int | None
    metrics: dict[str, Any]


def run_command(command: list[str], cwd: Path) -> subprocess.CompletedProcess[str]:
    return subprocess.run(command, cwd=cwd, text=True, capture_output=True, check=True)


def timed(command: list[str], cwd: Path) -> float:
    started = time.perf_counter()
    run_command(command, cwd)
    return time.perf_counter() - started


def copy_candidate(candidate: Candidate, destination: Path) -> Path:
    target = destination / candidate.name
    shutil.copytree(candidate.source, target)
    return target


def append_rebuild_marker(candidate: Candidate, source: Path) -> None:
    if candidate.name == "go":
        path = source / "main.go"
    else:
        path = source / "src" / "main.rs"
    with path.open("a", encoding="utf-8") as file:
        file.write("\n// one-file rebuild marker\n")


def build_candidate(candidate: Candidate, scratch: Path) -> tuple[Path, float, float]:
    source = copy_candidate(candidate, scratch)
    if candidate.name == "go":
        binary = scratch / "chimi-host-go"
        command = ["go", "build", "-trimpath", "-ldflags=-s -w", "-o", str(binary), "."]
    else:
        command = ["cargo", "build", "--release", "--quiet"]
        binary = source / "target" / "release" / "chimi-compiler-host-rust-spike"

    clean = timed(command, source)
    append_rebuild_marker(candidate, source)
    edited = timed(command, source)
    return binary, clean, edited


def run_measured(binary: Path, declarations: int, iterations: int) -> tuple[dict[str, Any], int | None]:
    command = [str(binary), "--declarations", str(declarations), "--iterations", str(iterations)]
    time_program = Path("/usr/bin/time")
    if time_program.exists() and platform.system() == "Darwin":
        completed = run_command([str(time_program), "-l", *command], ROOT)
        match = re.search(r"^\s*(\d+)\s+maximum resident set size$", completed.stderr, re.MULTILINE)
        peak_rss = int(match.group(1)) if match else None
    else:
        completed = run_command(command, ROOT)
        peak_rss = None
    return json.loads(completed.stdout), peak_rss


def compare(declarations: int, iterations: int) -> list[CandidateResult]:
    missing = [tool for tool in ("go", "cargo") if shutil.which(tool) is None]
    if missing:
        raise RuntimeError(f"missing required toolchain: {', '.join(missing)}")

    candidates = (
        Candidate("go", SPIKE / "go"),
        Candidate("rust", SPIKE / "rust"),
    )
    results: list[CandidateResult] = []
    with tempfile.TemporaryDirectory(prefix="chimi-host-spike-") as directory:
        scratch = Path(directory)
        for candidate in candidates:
            binary, clean, edited = build_candidate(candidate, scratch)
            metrics, peak_rss = run_measured(binary, declarations, iterations)
            version_command = ["go", "version"] if candidate.name == "go" else ["rustc", "--version"]
            metrics["toolchain"] = run_command(version_command, ROOT).stdout.strip()
            results.append(
                CandidateResult(
                    candidate.name,
                    clean,
                    edited,
                    binary.stat().st_size,
                    peak_rss,
                    metrics,
                )
            )
    validate(results)
    return results


def validate(results: list[CandidateResult]) -> None:
    keys = ("declarations", "input_bytes", "token_count", "node_count", "round_trip", "checksum")
    expected = {key: results[0].metrics[key] for key in keys}
    for result in results:
        actual = {key: result.metrics[key] for key in keys}
        if actual != expected or not actual["round_trip"]:
            raise RuntimeError(
                "candidate implementations did not produce equivalent retained syntax\n"
                f"expected: {expected}\n{result.name}: {actual}"
            )


def mib(value: int | None) -> str:
    if value is None:
        return "unavailable"
    return f"{value / (1024 * 1024):.2f} MiB"


def source_digest() -> str:
    digest = hashlib.sha256()
    for path in sorted(SPIKE.rglob("*")):
        if path.is_file() and "__pycache__" not in path.parts:
            digest.update(path.relative_to(SPIKE).as_posix().encode("utf-8"))
            digest.update(path.read_bytes())
    return digest.hexdigest()


def machine_memory_bytes() -> int | None:
    if platform.system() == "Darwin":
        value = run_command(["sysctl", "-n", "hw.memsize"], ROOT).stdout.strip()
        return int(value)
    page_size = os.sysconf("SC_PAGE_SIZE") if "SC_PAGE_SIZE" in os.sysconf_names else None
    pages = os.sysconf("SC_PHYS_PAGES") if "SC_PHYS_PAGES" in os.sysconf_names else None
    return page_size * pages if page_size is not None and pages is not None else None


def processor_name() -> str:
    if platform.system() == "Darwin":
        return run_command(["sysctl", "-n", "machdep.cpu.brand_string"], ROOT).stdout.strip()
    return platform.processor() or "unknown"


def render_results(results: list[CandidateResult], details: bool) -> None:
    print("\x1b[2J\x1b[H", end="")
    print(f"{BOLD}Chimi compiler host spike{RESET}\n")
    first = results[0].metrics
    print(f"{BOLD}Equivalent workload{RESET}")
    print(f"  source:       {first['input_bytes']:,} bytes")
    print(f"  tokens:       {first['token_count']:,}")
    print(f"  arena nodes:  {first['node_count']:,}")
    print(f"  checksum:     {first['checksum']}")
    print(f"  round-trip:   {first['round_trip']}\n")

    print(f"{BOLD}Measurements{RESET}")
    print("  candidate   fresh build   edit build   p50 frontend   p95 frontend   throughput")
    for item in results:
        metrics = item.metrics
        print(
            f"  {item.name:<9}   {item.fresh_build_seconds:>7.3f}s"
            f"      {item.edit_build_seconds:>7.3f}s"
            f"      {metrics['p50_microseconds'] / 1000:>7.2f}ms"
            f"        {metrics['p95_microseconds'] / 1000:>7.2f}ms"
            f"      {metrics['throughput_mib_per_second']:>7.1f} MiB/s"
        )

    print()
    print("  candidate   allocated/run   retained heap   represented payload   peak RSS   binary")
    for item in results:
        metrics = item.metrics
        print(
            f"  {item.name:<9}   {mib(metrics['allocated_bytes_per_run']):>13}"
            f"   {mib(metrics['retained_heap_bytes']):>13}"
            f"   {mib(metrics['represented_payload_bytes']):>19}"
            f"   {mib(item.peak_rss_bytes):>10}"
            f"   {mib(item.binary_bytes):>8}"
        )

    if details:
        print(f"\n{BOLD}Raw result state{RESET}")
        payload = {
            "environment": {
                "platform": platform.platform(),
                "machine": platform.machine(),
                "processor": processor_name(),
                "logical_cores": os.cpu_count(),
                "memory_bytes": machine_memory_bytes(),
                "spike_source_sha256": source_digest(),
            },
            "candidates": [
                {
                    "name": item.name,
                    "fresh_build_seconds": item.fresh_build_seconds,
                    "edit_build_seconds": item.edit_build_seconds,
                    "binary_bytes": item.binary_bytes,
                    "peak_rss_bytes": item.peak_rss_bytes,
                    "metrics": item.metrics,
                }
                for item in results
            ],
        }
        print(json.dumps(payload, indent=2, sort_keys=True))

    print(f"\n{DIM}Local measurements are comparative evidence, not universal language benchmarks.{RESET}")
    print(f"\n{BOLD}[r]{RESET} rerun  {BOLD}[d]{RESET} toggle raw details  {BOLD}[q]{RESET} quit")


def interactive(declarations: int, iterations: int) -> None:
    print("\x1b[2J\x1b[H", end="")
    print(f"{BOLD}Chimi compiler host spike{RESET}\n")
    print("This builds Go and Rust implementations of the same compact compiler workload.")
    print("The comparison is rejected unless their retained syntax state is identical.\n")
    print(f"{BOLD}[r]{RESET} run comparison  {BOLD}[q]{RESET} quit")
    results: list[CandidateResult] | None = None
    details = False
    while True:
        try:
            action = input("> ").strip().lower()
        except (EOFError, KeyboardInterrupt):
            return
        if action == "q":
            return
        if action == "r":
            print("building and measuring…")
            results = compare(declarations, iterations)
            render_results(results, details)
        elif action == "d" and results is not None:
            details = not details
            render_results(results, details)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--all", action="store_true", help="run once and print full results")
    parser.add_argument("--declarations", type=int, default=10_000)
    parser.add_argument("--iterations", type=int, default=30)
    arguments = parser.parse_args()
    if arguments.declarations < 1 or arguments.iterations < 1:
        parser.error("declarations and iterations must be positive")
    if arguments.all:
        render_results(compare(arguments.declarations, arguments.iterations), details=True)
    else:
        interactive(arguments.declarations, arguments.iterations)


if __name__ == "__main__":
    main()
