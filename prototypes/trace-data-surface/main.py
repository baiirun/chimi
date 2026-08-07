"""Interactive terminal shell for the trace-data surface prototype."""

from __future__ import annotations

import argparse
import sys

sys.dont_write_bytecode = True

from model import SCENARIOS, Scenario, render_summary, run


BOLD = "\x1b[1m"
DIM = "\x1b[2m"
RESET = "\x1b[0m"


def render(scenario: Scenario, index: int, clear: bool = True) -> None:
    if clear:
        print("\x1b[2J\x1b[H", end="")
    print(f"{BOLD}Chimi trace data surface{RESET}  {DIM}{index + 1}/{len(SCENARIOS)}{RESET}")
    print()
    print(f"{BOLD}{scenario.title}{RESET}")
    print(scenario.question)
    print()
    print(f"{BOLD}Candidate Chimi{RESET}")
    print(scenario.source)
    print()
    print(f"{BOLD}State transitions{RESET}")
    transitions = run(scenario.lines)
    for line_number, transition in enumerate(transitions, start=1):
        status = f"ERROR {transition.error}" if transition.error else "accepted"
        print(f"{DIM}{line_number}: {transition.line}{RESET}")
        print(f"   {status}")
        print(
            "   "
            f"records={transition.state.records} events={transition.state.events} "
            f"operations={dict(transition.state.operations)} "
            f"terminal={transition.state.terminal_seen} "
            f"peak_line_bytes={transition.state.peak_line_bytes}"
        )
    final = transitions[-1].state if transitions else None
    if final is not None and final.terminal_seen and transitions[-1].error is None:
        print(f"{BOLD}deterministic output{RESET}: {render_summary(final)}")
    else:
        print(f"{BOLD}deterministic output{RESET}: none")
    print()
    print(f"{BOLD}Claim{RESET}: {scenario.note}")
    print()
    print(f"{BOLD}[n]{RESET} next  {BOLD}[p]{RESET} previous  {BOLD}[q]{RESET} quit")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--all", action="store_true", help="print every scenario")
    args = parser.parse_args()
    if args.all:
        for index, scenario in enumerate(SCENARIOS):
            render(scenario, index, clear=False)
            if index + 1 < len(SCENARIOS):
                print("\n" + "=" * 80 + "\n")
        return 0

    index = 0
    while True:
        render(SCENARIOS[index], index)
        command = input("> ").strip().lower()
        if command == "q":
            return 0
        if command == "p":
            index = (index - 1) % len(SCENARIOS)
        else:
            index = (index + 1) % len(SCENARIOS)


if __name__ == "__main__":
    raise SystemExit(main())
