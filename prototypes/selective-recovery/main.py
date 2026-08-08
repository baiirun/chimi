#!/usr/bin/env python3
"""PROTOTYPE: interactive selective-recovery syntax comparison."""

import argparse
import sys

from model import FORMS, render


def clear() -> None:
    print("\033[2J\033[H", end="")


def interactive() -> None:
    form_index = 0
    example_index = 0
    decisions = ["" for _ in FORMS]

    while True:
        clear()
        print("\033[1mChimi selective recovery\033[0m")
        print("\033[2mCompare syntax; decisions remain in memory only.\033[0m\n")
        print(render(form_index, example_index, tuple(decisions)))
        print(
            "\n\033[1m[f/b]\033[0m form  "
            "\033[1m[n/p]\033[0m scenario  "
            "\033[1m[k/d/u]\033[0m keep/discard/undecide  "
            "\033[1m[q]\033[0m quit"
        )
        choice = input("> ").strip().lower()
        if choice == "q":
            return
        if choice == "f":
            form_index = (form_index + 1) % len(FORMS)
            example_index = 0
        elif choice == "b":
            form_index = (form_index - 1) % len(FORMS)
            example_index = 0
        elif choice == "n":
            example_index = (example_index + 1) % len(FORMS[form_index].examples)
        elif choice == "p":
            example_index = (example_index - 1) % len(FORMS[form_index].examples)
        elif choice == "k":
            decisions[form_index] = "keep"
        elif choice == "d":
            decisions[form_index] = "discard"
        elif choice == "u":
            decisions[form_index] = ""


def print_all() -> None:
    decisions = tuple("" for _ in FORMS)
    for form_index, form in enumerate(FORMS):
        for example_index, _ in enumerate(form.examples):
            print(render(form_index, example_index, decisions))
            print("\n" + "=" * 72 + "\n")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--all", action="store_true")
    arguments = parser.parse_args()
    if arguments.all:
        print_all()
    elif sys.stdin.isatty() and sys.stdout.isatty():
        interactive()
    else:
        parser.error("interactive mode requires a terminal; use --all")


if __name__ == "__main__":
    main()
