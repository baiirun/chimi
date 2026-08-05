"""Terminal browser for the disposable frontend performance contract."""

from __future__ import annotations

import argparse
import json
import sys

sys.dont_write_bytecode = True

from model import WORKLOADS, BenchmarkCase, build_case, edit_preview, result_template, source_preview


BOLD = "\x1b[1m"
DIM = "\x1b[2m"
RESET = "\x1b[0m"


def cases() -> tuple[BenchmarkCase, ...]:
    return tuple(build_case(workload) for workload in WORKLOADS)


def render_intro(total: int) -> None:
    print("\x1b[2J\x1b[H", end="")
    print(f"{BOLD}Chimi frontend performance contract{RESET}")
    print()
    print(f"{BOLD}This is not showing benchmark results yet.{RESET}")
    print("It is a proposed test plan for the future compiler frontend.")
    print()
    print(f"The prototype contains {total} generated scenarios. Each scenario says:")
    print("  1. what source or editor action the compiler will receive;")
    print("  2. why that situation could expose a performance problem;")
    print("  3. what the benchmark will measure;")
    print("  4. what behavior we propose treating as acceptable.")
    print()
    print(f"{BOLD}What you are reviewing{RESET}")
    print("  - Is this a realistic situation Chimi should stay fast under?")
    print("  - Is the proposed pass condition useful or arbitrary?")
    print("  - Is an important compiler or editor situation missing?")
    print()
    print(f"{DIM}Generated inputs exist now. Timings arrive when the real parser exists.{RESET}")
    print()
    print(f"{BOLD}[n]{RESET} first scenario  {BOLD}[a]{RESET} all summary  {BOLD}[q]{RESET} quit")


def render(case: BenchmarkCase, index: int, total: int, view: str) -> None:
    print("\x1b[2J\x1b[H", end="")
    workload = case.workload
    print(f"{BOLD}Proposed benchmark scenario{RESET}  {DIM}{index + 1}/{total} — no timings yet{RESET}")
    print()
    print(f"{BOLD}{workload.title}{RESET}")
    print(f"{workload.question}")
    print()
    print(f"{BOLD}Situation{RESET}")
    print(f"  Generate {len(case.before)} file(s): {case.input_lines:,} lines / {case.input_bytes:,} bytes.")
    if case.edit:
        print(f"  Then {case.edit.description}.")
    else:
        print("  Run the future frontend on the unchanged generated project.")
    print()
    print(f"{BOLD}Future compiler work under observation{RESET}")
    print(f"  {' -> '.join(workload.phases)}")
    print()
    print(f"{BOLD}What we would record{RESET}")
    for metric in workload.metrics:
        print(f"  - {metric}")
    print()
    print(f"{BOLD}Proposed pass condition{RESET}")
    print(f"  {workload.target_behavior}")

    print()
    if view == "schema":
        print(f"{BOLD}Raw result shape the future benchmark would fill{RESET}")
        encoded = json.dumps(result_template(case), indent=2)
        lines = encoded.splitlines()
        print("\n".join(lines[:14]))
        if len(lines) > 14:
            print(f"{DIM}... {len(lines) - 14} more lines{RESET}")
    elif view == "details":
        print(f"{BOLD}Correctness requirements that timing cannot violate{RESET}")
        for invariant in workload.invariants:
            print(f"  - {invariant}")
    elif case.edit:
        before, after = edit_preview(case)
        print(f"{BOLD}Generated change{RESET}")
        print(f"  {DIM}before{RESET} {before}")
        print(f"  {DIM}after {RESET} {after}")
    else:
        path = sorted(case.before)[0]
        print(f"{BOLD}Generated input sample{RESET}  {DIM}{path}{RESET}")
        print(source_preview(case, lines=5))

    if view == "card":
        print()
        print(f"{BOLD}Your decision{RESET}: keep, change, or remove this scenario?")

    print()
    print(
        f"{BOLD}[n]{RESET} next  {BOLD}[p]{RESET} previous  "
        f"{BOLD}[d]{RESET} technical requirements  {BOLD}[r]{RESET} raw result shape  "
        f"{BOLD}[h]{RESET} help  {BOLD}[q]{RESET} quit"
    )

def print_summary(all_cases: tuple[BenchmarkCase, ...]) -> None:
    print("Chimi frontend performance workload summary")
    print()
    for case in all_cases:
        edit = case.edit.description if case.edit else "none"
        print(
            f"{case.workload.id:30} "
            f"files={len(case.before):4} "
            f"lines={case.input_lines:8,} "
            f"bytes={case.input_bytes:10,} "
            f"edit={edit}"
        )
    print()
    print(f"{len(all_cases)} deterministic workloads generated in memory")


def interactive(all_cases: tuple[BenchmarkCase, ...]) -> None:
    index: int | None = None
    view = "card"
    while True:
        if index is None:
            render_intro(len(all_cases))
        else:
            render(all_cases[index], index, len(all_cases), view)
        try:
            command = input("> ").strip().lower()
        except (EOFError, KeyboardInterrupt):
            print()
            return
        if command == "q":
            return
        if command in {"n", ""}:
            index = 0 if index is None else (index + 1) % len(all_cases)
            view = "card"
        elif command == "p":
            index = len(all_cases) - 1 if index is None else (index - 1) % len(all_cases)
            view = "card"
        elif command == "h":
            index = None
            view = "card"
        elif command == "d" and index is not None:
            view = "card" if view == "details" else "details"
        elif command == "r":
            if index is not None:
                view = "card" if view == "schema" else "schema"
        elif command == "a":
            print("\x1b[2J\x1b[H", end="")
            print_summary(all_cases)
            input("\npress enter to return ")


def main() -> int:
    parser = argparse.ArgumentParser(
        description="Browse the disposable Chimi frontend performance contract"
    )
    parser.add_argument("--all", action="store_true", help="generate and summarize every workload")
    parser.add_argument(
        "--result-template",
        metavar="WORKLOAD_ID",
        help="print the JSON result template for one workload",
    )
    args = parser.parse_args()
    all_cases = cases()

    if args.result_template:
        case = next(
            (item for item in all_cases if item.workload.id == args.result_template),
            None,
        )
        if case is None:
            parser.error(f"unknown workload: {args.result_template}")
        print(json.dumps(result_template(case), indent=2))
        return 0
    if args.all:
        print_summary(all_cases)
        return 0

    interactive(all_cases)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
