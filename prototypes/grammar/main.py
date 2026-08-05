"""Terminal shell for browsing the disposable Chimi grammar prototype."""

from __future__ import annotations

import argparse
from pathlib import Path
import sys

sys.dont_write_bytecode = True

from model import FixtureAnalysis, analyze_corpus


BOLD = "\x1b[1m"
DIM = "\x1b[2m"
RESET = "\x1b[0m"


def corpus() -> tuple[FixtureAnalysis, ...]:
    repository = Path(__file__).resolve().parents[2]
    return analyze_corpus(repository / "prototypes" / "golden")


def relative_path(analysis: FixtureAnalysis) -> str:
    marker = "prototypes/golden/"
    path = analysis.path.as_posix()
    return path[path.index(marker) + len(marker) :]


def print_summary(analyses: tuple[FixtureAnalysis, ...]) -> bool:
    print("Chimi grammar prototype corpus summary")
    print()
    for analysis in analyses:
        match = "PASS" if analysis.matches_expectation else "MISMATCH"
        issue = ""
        if analysis.lexical_issues:
            first = analysis.lexical_issues[0]
            issue = f" — lexical {first.line}:{first.column} {first.message}"
        elif analysis.syntax_issue:
            issue = (
                f" — syntax {analysis.syntax_issue.line}:{analysis.syntax_issue.column} "
                f"{analysis.syntax_issue.message}"
            )
        print(
            f"{match:8} {relative_path(analysis):48} "
            f"expected={analysis.expected_syntax:7} actual={analysis.syntax_result:7}{issue}"
        )
    print()
    matched = sum(analysis.matches_expectation for analysis in analyses)
    print(f"{matched}/{len(analyses)} fixtures match the draft grammar expectation")
    return matched == len(analyses)


def render(analysis: FixtureAnalysis, index: int, total: int, show_trivia: bool) -> None:
    print("\x1b[2J\x1b[H", end="")
    print(f"{BOLD}Chimi grammar prototype{RESET}  {DIM}{index + 1}/{total}{RESET}")
    print()
    print(f"{BOLD}fixture{RESET}:            {relative_path(analysis)}")
    print(f"{BOLD}expected compile{RESET}:   {analysis.expected_compile}")
    print(f"{BOLD}expected syntax{RESET}:    {analysis.expected_syntax}")
    print(f"{BOLD}recognized syntax{RESET}:  {analysis.syntax_result}")
    print(f"{BOLD}expectation match{RESET}:  {'yes' if analysis.matches_expectation else 'NO'}")
    print(f"{BOLD}diagnostic marker{RESET}:  {analysis.expected_diagnostic or '-'}")
    print(f"{BOLD}constructs{RESET}:         {', '.join(analysis.constructs) or 'expressions only'}")

    issues = list(analysis.lexical_issues)
    if analysis.syntax_issue:
        issues.append(analysis.syntax_issue)
    print(f"{BOLD}issues{RESET}:             {len(issues)}")
    for issue in issues[:3]:
        print(f"  {issue.line}:{issue.column} {issue.message}")

    visible_tokens = analysis.tokens if show_trivia else analysis.significant_tokens
    token_preview = " ".join(
        f"{token.kind}({token.text!r})" for token in visible_tokens[:16] if token.kind != "EOF"
    )
    print()
    print(f"{BOLD}tokens{RESET} {DIM}({'with' if show_trivia else 'without'} trivia){RESET}")
    print(token_preview or "-")

    print()
    print(f"{BOLD}source preview{RESET}")
    source_lines = analysis.source.splitlines()
    for line_number, line in enumerate(source_lines[:12], start=1):
        print(f"{line_number:>3}  {line}")
    if len(source_lines) > 12:
        print(f"{DIM}... {len(source_lines) - 12} more lines{RESET}")

    print()
    print(
        f"{BOLD}[n]{RESET} next  {BOLD}[p]{RESET} previous  "
        f"{BOLD}[t]{RESET} toggle trivia  {BOLD}[a]{RESET} all summary  "
        f"{BOLD}[q]{RESET} quit"
    )


def interactive(analyses: tuple[FixtureAnalysis, ...]) -> None:
    index = 0
    show_trivia = False
    while True:
        render(analyses[index], index, len(analyses), show_trivia)
        try:
            command = input("> ").strip().lower()
        except (EOFError, KeyboardInterrupt):
            print()
            return
        if command == "q":
            return
        if command == "n" or command == "":
            index = (index + 1) % len(analyses)
        elif command == "p":
            index = (index - 1) % len(analyses)
        elif command == "t":
            show_trivia = not show_trivia
        elif command == "a":
            print("\x1b[2J\x1b[H", end="")
            print_summary(analyses)
            input("\npress enter to return ")


def main() -> int:
    parser = argparse.ArgumentParser(description="Browse the disposable Chimi grammar prototype")
    parser.add_argument("--all", action="store_true", help="check every golden fixture and exit")
    args = parser.parse_args()
    analyses = corpus()
    if args.all:
        return 0 if print_summary(analyses) else 1
    interactive(analyses)
    return 0


if __name__ == "__main__":
    sys.exit(main())
