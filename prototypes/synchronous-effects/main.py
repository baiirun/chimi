#!/usr/bin/env python3
"""Interactive TUI for the synchronous-effects composition prototype."""

import argparse
import sys

from model import CONSOLE, FILES, PrototypeState, call, guard_diagnostic, initial_state, provision, recover

BOLD = "\033[1m"
DIM = "\033[2m"
RESET = "\033[0m"


def render(state: PrototypeState, diagnostic: str = "") -> str:
    shape = state.callable
    needs = ", ".join(f"{name}: {interface}" for name, interface in shape.needs) or "{}"
    failures = ", ".join(shape.failures) or "{}"
    effects = ", ".join(shape.effects) or "{}"
    runner = ", ".join(state.runner_configuration)
    lines = [
        f"{BOLD}Synchronous effect composition{RESET}",
        f"{DIM}{state.last_action}{RESET}",
        "",
        f"{BOLD}data inputs{RESET}: {', '.join(shape.inputs)}",
        f"{BOLD}success{RESET}: {shape.success}",
        f"{BOLD}errors{RESET}: {failures}",
        f"{BOLD}needs{RESET}: {needs}",
        f"{BOLD}effects{RESET}: {effects}",
        f"{BOLD}runner configuration{RESET}: {runner}",
    ]
    if diagnostic:
        lines.extend(["", f"{BOLD}guard check{RESET}: {diagnostic}"])
    lines.extend(
        [
            "",
            f"{BOLD}[f]{RESET} filesystem call  {BOLD}[c]{RESET} console call  "
            f"{BOLD}[u]{RESET} using/provide",
            f"{BOLD}[r]{RESET} recover one failure  {BOLD}[g]{RESET} guard check  "
            f"{BOLD}[x]{RESET} reset  {BOLD}[q]{RESET} quit",
        ]
    )
    return "\n".join(lines)


def apply(state: PrototypeState, action: str) -> tuple[PrototypeState, str]:
    if action == "f":
        return call(state, FILES), ""
    if action == "c":
        return call(state, CONSOLE), ""
    if action == "u":
        return provision(state), ""
    if action == "r":
        return recover(state), ""
    if action == "g":
        return state, guard_diagnostic(state)
    if action == "x":
        return initial_state(), ""
    return state, "unknown action"


def scenario(actions: str) -> None:
    state = initial_state()
    print(render(state))
    for action in actions:
        if action.isspace() or action == "q":
            continue
        state, diagnostic = apply(state, action)
        print("\n---\n")
        print(render(state, diagnostic))


def interactive() -> None:
    state = initial_state()
    diagnostic = ""
    while True:
        print("\033[2J\033[H" + render(state, diagnostic), flush=True)
        action = input("\n> ").strip().lower()[:1]
        if action == "q":
            return
        state, diagnostic = apply(state, action)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--scenario", help="render a non-interactive action sequence")
    arguments = parser.parse_args()
    if arguments.scenario is not None:
        scenario(arguments.scenario)
    elif sys.stdin.isatty():
        interactive()
    else:
        parser.error("interactive mode requires a terminal; use --scenario")


if __name__ == "__main__":
    main()
