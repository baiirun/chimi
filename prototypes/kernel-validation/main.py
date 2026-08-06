#!/usr/bin/env python3
"""Terminal shell for the disposable Chimi kernel validator."""

import argparse
import subprocess
from pathlib import Path

from model import Evidence, ProbeResult, SEAMS, STAGES, evaluate


BOLD = "\033[1m"
DIM = "\033[2m"
RESET = "\033[0m"


def repository_root() -> Path:
    directory = Path.cwd()
    while directory.parent != directory:
        if (directory / "CONTEXT.md").exists():
            return directory
        directory = directory.parent
    raise RuntimeError("cannot locate repository root")


def run_probes(root: Path) -> tuple[ProbeResult, ...]:
    results = []
    for stage in STAGES:
        completed = subprocess.run(
            stage.command,
            cwd=root,
            capture_output=True,
            text=True,
            check=False,
        )
        output = completed.stdout + completed.stderr
        summary = next(
            (line for line in reversed(output.splitlines()) if line.strip()),
            "no output",
        )
        results.append(
            ProbeResult(
                stage=stage,
                passed=completed.returncode == 0
                and stage.expected_summary in output,
                summary=summary,
            )
        )
    return tuple(results)


def clear() -> None:
    print("\033[2J\033[H", end="")


def render_summary(results: tuple[ProbeResult, ...]) -> None:
    verdict = evaluate(results)
    clear()
    print(f"{BOLD}Chimi executable-kernel validation{RESET}\n")
    print(f"{BOLD}all probes pass:{RESET}       {verdict.probes_pass}")
    print(f"{BOLD}design coherent:{RESET}       {verdict.coherent}")
    print(f"{BOLD}specification-ready:{RESET}   {verdict.specification_ready}")
    print(f"{BOLD}end-to-end compiler:{RESET}   {verdict.end_to_end_compiler}\n")
    if verdict.end_to_end_compiler:
        print("The Rust compiler connects source, resolved names, typed contracts,")
        print("verified executable core, and observable execution.")
    else:
        print("The kernel remains specification-ready, but one or more compiler")
        print("handoffs or executable probes are not connected.")


def render_stages(results: tuple[ProbeResult, ...]) -> None:
    clear()
    print(f"{BOLD}Executable probe results{RESET}\n")
    for result in results:
        status = "PASS" if result.passed else "FAIL"
        print(f"{status:4}  {result.stage.name:20} {DIM}{result.summary}{RESET}")


def render_seams() -> None:
    clear()
    print(f"{BOLD}Pipeline handoffs{RESET}\n")
    for seam in SEAMS:
        marker = {
            Evidence.CONNECTED: "CONNECTED  ",
            Evidence.INDEPENDENT: "MODELED    ",
            Evidence.DEFERRED: "DEFERRED   ",
        }[seam.evidence]
        print(f"{marker} {seam.source} -> {seam.target}")
        print(f"             {DIM}{seam.detail}{RESET}")


def render_gaps(results: tuple[ProbeResult, ...]) -> None:
    verdict = evaluate(results)
    clear()
    print(f"{BOLD}What this validation does not prove{RESET}\n")
    for blocker in verdict.blockers:
        print(f"- {blocker}")
    print("- The permanent backend and production runtime architecture remain open.")
    print("- Incremental semantic performance and production durability remain later work.")


def interactive(results: tuple[ProbeResult, ...]) -> None:
    view = "summary"
    while True:
        if view == "summary":
            render_summary(results)
        elif view == "stages":
            render_stages(results)
        elif view == "seams":
            render_seams()
        else:
            render_gaps(results)
        print(
            f"\n{BOLD}[s]{RESET} summary  {BOLD}[p]{RESET} probes  "
            f"{BOLD}[h]{RESET} handoffs  {BOLD}[g]{RESET} gaps  {BOLD}[q]{RESET} quit"
        )
        command = input("> ").strip().lower()
        if command == "q":
            return
        view = {"s": "summary", "p": "stages", "h": "seams", "g": "gaps"}.get(
            command, view
        )


def check(results: tuple[ProbeResult, ...]) -> bool:
    verdict = evaluate(results)
    for result in results:
        status = "PASS" if result.passed else "FAIL"
        print(f"{status:4}  {result.stage.name:20} {result.summary}")
    print()
    for seam in SEAMS:
        print(f"{seam.evidence.value:11} {seam.source} -> {seam.target}")
    print()
    print(f"specification-ready={str(verdict.specification_ready).lower()}")
    print(f"end-to-end-compiler={str(verdict.end_to_end_compiler).lower()}")
    return verdict.specification_ready and verdict.end_to_end_compiler


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    results = run_probes(repository_root())
    if args.check:
        raise SystemExit(0 if check(results) else 1)
    interactive(results)


if __name__ == "__main__":
    main()
