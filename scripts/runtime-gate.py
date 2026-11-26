#!/usr/bin/env python3
"""The runtime migration's size and speed gates (CHI-305), in one command.

Builds each program in benchmarks/runtime-gate/ with a base compiler and with
this checkout's, then compares:

- instructions under callgrind for loop-bind, map-loop and http (the median
  of --runs runs each; http runs on one carrier with real time), which may
  grow by at most 2%;
- the stripped size of hello, which may grow by at most 50 KB over the
  step-1 baseline (one Rust runtime archive, CHI-306: 357,408 bytes on Linux
  x86-64). The change against the base is printed too, since a step should
  not spend the whole allowance.

Instruction counts, not wall time, so a noisy machine still gives a stable
answer. The http program talks to itself over loopback, so its count moves
by a few tenths of a percent from run to run; take more runs when a result
is close to the line.

Usage, from anywhere inside the repository:
  scripts/runtime-gate.py BASE_REV            build BASE_REV's compiler too
  scripts/runtime-gate.py --base-compiler PATH
Options: --runs N (default 3), --work DIR (default a temporary directory).
This checkout's compiler is `cargo build --release -p chimi`.

Exit status: 0 when every gate holds, 1 when one fails, 2 on a failed build or
bad usage. Needs valgrind and strip.
"""

import argparse
import shutil
import statistics
import subprocess
import sys
import tempfile
from pathlib import Path

SPEED_LIMIT = 0.02
SIZE_LIMIT = 50 * 1024
# decisions.md 2026-11-25, CHI-306; measured on Linux x86-64.
SIZE_BASELINE = 357_408
SPEED_PROGRAMS = ["loop-bind", "map-loop", "http"]
RUNNER_ARGUMENTS = ["--chimi-runner", "--clock=real", "--trace=off", "--carriers=1", "--"]


def run(command, cwd=None, capture=False):
    result = subprocess.run(command, cwd=cwd, text=True, capture_output=capture)
    if result.returncode != 0:
        if capture:
            sys.stderr.write(result.stdout + result.stderr)
        sys.exit(f"runtime-gate: failed: {' '.join(map(str, command))}")
    return result


def build_compiler(checkout: Path) -> Path:
    run(["cargo", "build", "--release", "-p", "chimi", "--quiet"], cwd=checkout)
    return checkout / "target" / "release" / "chimi"


def base_compiler(repository: Path, revision: str, work: Path) -> Path:
    checkout = work / "base"
    run(["git", "-C", repository, "worktree", "add", "--detach", "--force", checkout, revision])
    try:
        built = build_compiler(checkout)
        kept = work / "chimi-base"
        shutil.copy2(built, kept)
        return kept
    finally:
        run(["git", "-C", repository, "worktree", "remove", "--force", checkout])


def build_programs(compiler: Path, sources: Path, out: Path) -> None:
    out.mkdir(parents=True, exist_ok=True)
    for program in SPEED_PROGRAMS + ["hello"]:
        run([compiler, "build", sources / f"{program}.chimi", "-o", out / program], cwd=out, capture=True)


def instructions(executable: Path) -> int:
    result = subprocess.run(
        ["valgrind", "--tool=callgrind", "--callgrind-out-file=/dev/null", executable, *RUNNER_ARGUMENTS],
        text=True, capture_output=True,
    )
    for line in result.stderr.splitlines():
        if "Collected :" in line:
            return int(line.rsplit(":", 1)[1])
    sys.stderr.write(result.stderr)
    sys.exit(f"runtime-gate: no instruction count for {executable}")


def stripped_size(executable: Path, work: Path) -> int:
    stripped = work / f"{executable.parent.name}-{executable.name}.stripped"
    run(["strip", "-o", stripped, executable])
    return stripped.stat().st_size


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("base_rev", nargs="?")
    parser.add_argument("--base-compiler", type=Path)
    parser.add_argument("--runs", type=int, default=3)
    parser.add_argument("--work", type=Path)
    arguments = parser.parse_args()
    if (arguments.base_rev is None) == (arguments.base_compiler is None) or arguments.runs < 1:
        parser.print_usage(sys.stderr)
        return 2

    repository = Path(run(["git", "rev-parse", "--show-toplevel"], capture=True).stdout.strip())
    sources = repository / "benchmarks" / "runtime-gate"
    work = arguments.work or Path(tempfile.mkdtemp(prefix="runtime-gate-"))
    work.mkdir(parents=True, exist_ok=True)

    base = arguments.base_compiler or base_compiler(repository, arguments.base_rev, work)
    new = build_compiler(repository)
    build_programs(base.resolve(), sources, work / "base-programs")
    build_programs(new, sources, work / "new-programs")

    failed = False
    print(f"{'program':<10} {'base':>15} {'new':>15} {'change':>8}  gate")
    for program in SPEED_PROGRAMS:
        # Interleaved, so a machine that slows during the run slows both.
        samples = {"base": [], "new": []}
        for _ in range(arguments.runs):
            for side in samples:
                samples[side].append(instructions(work / f"{side}-programs" / program))
        counts = {side: statistics.median(values) for side, values in samples.items()}
        change = counts["new"] / counts["base"] - 1
        holds = change <= SPEED_LIMIT
        failed |= not holds
        print(f"{program:<10} {counts['base']:>15,.0f} {counts['new']:>15,.0f} {change:>+8.2%}  "
              f"{'ok' if holds else 'FAIL'} (at most +{SPEED_LIMIT:.0%})")

    sizes = {side: stripped_size(work / f"{side}-programs" / "hello", work) for side in ("base", "new")}
    growth = sizes["new"] - SIZE_BASELINE
    holds = growth <= SIZE_LIMIT
    failed |= not holds
    print(f"{'hello':<10} {sizes['base']:>15,} {sizes['new']:>15,} {sizes['new'] - sizes['base']:>+8,}  "
          f"{'ok' if holds else 'FAIL'} (stripped bytes; {growth:+,} over the step-1 baseline "
          f"{SIZE_BASELINE:,}, at most +{SIZE_LIMIT:,})")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
