#!/usr/bin/env python3
"""Mutation runner: proves a crate's tests reject plausible wrong implementations.

Each mutant is a small source edit that reintroduces a bug the design exists to
prevent (a wrong comparison, a skipped cleanup step, a reordered stop). The
runner applies one mutant at a time and runs the crate's tests. A mutant must
turn at least one test red; one that leaves every test green has SURVIVED and
means a behaviour the design depends on is untested.

Usage, from anywhere inside the repository:

    scripts/mutation/mutate.py scripts/mutation/heap_mutations.py \\
        scripts/mutation/kernel_mutations.py
    scripts/mutation/mutate.py --revision HEAD~3 scripts/mutation/heap_mutations.py
    scripts/mutation/mutate.py --only "ignore the owner tag" scripts/mutation/heap_mutations.py

The runner never edits the checkout. It exports the committed tree at
--revision (default HEAD) into a scratch directory and mutates files there, so
uncommitted work by anyone else is neither tested nor touched. Builds share one
target directory under the scratch directory (--scratch, default
$TMPDIR/chimi-mutation), so later runs are incremental.

A mutant list is a Python file defining:

    CRATE = "prototypes/supervised-process-runtime"  # repository-relative
    CARGO_TEST_ARGS = ["--release"]                    # optional
    MUTATIONS = [dict(name=..., file=..., edits=[(old, new), ...],
                      targets=["heap"], filter="")]

`file` is relative to CRATE. Every `old` must occur exactly once in the file at
that revision; a mutant whose text no longer matches is reported STALE so the
list is updated instead of silently testing nothing. `targets` are
`cargo test --test` names ("lib" means `--lib`); empty runs every target.
`filter` is passed to the test binaries. A mutant may instead give `command`, an
argv run in CRATE with the shared target directory (for example a `cargo run` of
a lab binary with flags the default tests do not pass). A command mutant must
also give `expect`, a list of substrings of the failure it is meant to cause: it
is KILLED only when the command exits nonzero and its output contains one of
them. A command that fails because the mutated source did not build (a Rust or C
compile error, or a native build failure the lab reports as a failed scenario) is
KILLED-BUILD, and any other failure is KILLED-OTHER; neither proves the scenario
guards the mutant. Each distinct command must pass at the baseline.

Before any mutant, the unmutated tests must pass (BASELINE). The baseline runs
exactly the test selections (targets and filter) and commands the chosen mutants
use, not the whole crate. A selection that passed at the same commit is
recorded under the scratch directory and not run again, so splitting a list
into several --only runs costs one baseline in total. --fresh-baseline ignores
those records. Output is one line
per mutant: KILLED (a test failed), KILLED-BUILD (the mutant did not compile,
which proves nothing about the tests), KILLED-OTHER (a command mutant failed,
but not with any `expect` text), SURVIVED, NO-TESTS (the targets and filter
selected no test, so passing proves nothing), STALE, or TIMEOUT.
Exit status is 0 only when the baseline passes and every mutant is KILLED or
TIMEOUT.
"""

import argparse
import hashlib
import os
import re
import runpy
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def repository_root() -> Path:
    output = subprocess.run(
        ["git", "rev-parse", "--show-toplevel"],
        cwd=Path(__file__).resolve().parent,
        check=True,
        capture_output=True,
        text=True,
    )
    return Path(output.stdout.strip())


def export_tree(root: Path, revision: str, destination: Path) -> str:
    commit = subprocess.run(
        ["git", "rev-parse", "--verify", f"{revision}^{{commit}}"],
        cwd=root,
        check=True,
        capture_output=True,
        text=True,
    ).stdout.strip()
    if destination.exists():
        shutil.rmtree(destination)
    destination.mkdir(parents=True)
    archive = subprocess.Popen(["git", "archive", commit], cwd=root, stdout=subprocess.PIPE)
    # -m stamps every file with the extraction time. git archive would otherwise
    # restore the commit time, which is older than artifacts the shared target
    # directory built from the last mutant, and cargo would reuse that build.
    subprocess.run(["tar", "-x", "-m", "-C", str(destination)], stdin=archive.stdout, check=True)
    if archive.wait() != 0:
        sys.exit(f"git archive {commit} failed")
    return commit


def cargo_test(crate: Path, args, targets, test_filter, target_dir: Path, timeout):
    command = ["cargo", "test", "--quiet", *args]
    for target in targets:
        command += ["--lib"] if target == "lib" else ["--test", target]
    if test_filter:
        command += ["--", test_filter]
    return run_command(crate, command, target_dir, timeout)


def run_command(crate: Path, command, target_dir: Path, timeout):
    """Runs `command`, and on a timeout stops everything it started.

    A mutant that makes a program loop forever leaves the test binary and the
    program it built running after cargo is killed, so the command gets its own
    process group and the whole group is killed.
    """
    environment = dict(os.environ, CARGO_TARGET_DIR=str(target_dir))
    process = subprocess.Popen(
        command, cwd=crate, env=environment, stdout=subprocess.PIPE,
        stderr=subprocess.PIPE, text=True, start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(timeout=timeout)
    except subprocess.TimeoutExpired:
        os.killpg(process.pid, signal.SIGKILL)
        process.communicate()
        raise
    return subprocess.CompletedProcess(command, process.returncode, stdout, stderr)


def tests_run(output: str) -> int:
    return sum(int(count) for count in re.findall(r"^running (\d+) tests?$", output, re.M))


def build_failed(output: str) -> bool:
    # Rust's own codes are E0xxx; a test's output can quote Chimi diagnostics
    # such as error[E5005], which say nothing about the build.
    return (
        re.search(r"error\[E0\d{3}\]", output) is not None
        or "could not compile" in output
        or "error: linking" in output
    )


# How a command reports that the mutated source did not build: cargo's own
# errors, the lab's C harness compile failure ("compile heap_boundary.c: ..."),
# a failed native build of a fixture, or a compiler diagnostic line.
COMMAND_BUILD_FAILURES = re.compile(
    r"compile [\w.-]+\.c:|native build failed|fixture does not compile|error:"
)


def command_status(output: str, expect) -> str:
    if any(text in output for text in expect):
        return "KILLED"
    if build_failed(output) or COMMAND_BUILD_FAILURES.search(output):
        return "KILLED-BUILD"
    return "KILLED-OTHER"


def baseline_record(records: Path, commit: str, crate: str, args, selection) -> Path:
    key = hashlib.sha256(repr((commit, crate, tuple(args), selection)).encode()).hexdigest()[:24]
    return records / key


def run_baseline(spec, crate: Path, args, mutations, target_dir: Path, records: Path,
                 commit: str, timeout) -> bool:
    """Requires: every selection a mutant uses. Ensures: each passed unmutated at
    commit, either now or in an earlier run recorded under records."""
    selections = dict.fromkeys(
        ("command", tuple(m["command"])) if "command" in m
        else ("tests", tuple(m.get("targets", [])), m.get("filter", ""))
        for m in mutations
    )
    for selection in selections:
        record = baseline_record(records, commit, spec["CRATE"], args, selection)
        if record.exists():
            continue
        started = time.monotonic()
        if selection[0] == "command":
            result = run_command(crate, list(selection[1]), target_dir, timeout)
            label = " ".join(selection[1])
        else:
            result = cargo_test(crate, args, list(selection[1]), selection[2], target_dir, timeout)
            label = " ".join(selection[1]) or "all targets"
            if selection[2]:
                label += f" -- {selection[2]}"
        if result.returncode != 0:
            print(f"BASELINE FAILED [{label}] ({time.monotonic() - started:.0f}s)", flush=True)
            print(result.stdout[-4000:] + result.stderr[-4000:])
            return False
        print(f"BASELINE ok  [{label}] ({time.monotonic() - started:.0f}s)", flush=True)
        records.mkdir(parents=True, exist_ok=True)
        record.write_text(label + "\n")
    return True


def run_list(path: Path, tree: Path, target_dir: Path, records: Path, commit: str, only,
             timeout) -> bool:
    spec = runpy.run_path(str(path))
    crate = tree / spec["CRATE"]
    args = spec.get("CARGO_TEST_ARGS", [])
    mutations = [m for m in spec["MUTATIONS"] if not only or m["name"] in only]
    print(f"# {path.name}: {len(mutations)} mutants against {spec['CRATE']}", flush=True)

    if not run_baseline(spec, crate, args, mutations, target_dir, records, commit, timeout):
        return False

    ok = True
    for mutation in mutations:
        source = crate / mutation["file"]
        if not source.is_file():
            print(f"STALE        {mutation['name']}: {mutation['file']} does not exist", flush=True)
            ok = False
            continue
        original = source.read_text()
        mutated = original
        stale = [old for old, _ in mutation["edits"] if original.count(old) != 1]
        if stale:
            print(f"STALE        {mutation['name']}: edit text occurs "
                  f"{original.count(stale[0])} times in {mutation['file']}", flush=True)
            ok = False
            continue
        for old, new in mutation["edits"]:
            mutated = mutated.replace(old, new)
        targets = mutation.get("targets", [])
        test_filter = mutation.get("filter", "")
        command = mutation.get("command")
        if command and not mutation.get("expect"):
            print(f"STALE        {mutation['name']}: a command mutant needs `expect`", flush=True)
            ok = False
            continue
        scope = " ".join(targets) or "all targets"
        if test_filter:
            scope += f" -- {test_filter}"
        if command:
            scope = " ".join(command)
        source.write_text(mutated)
        started = time.monotonic()
        try:
            if command:
                result = run_command(crate, command, target_dir, timeout)
            else:
                result = cargo_test(crate, args, targets, test_filter, target_dir, timeout)
            output = result.stdout + result.stderr
            if command and result.returncode == 0:
                status = "SURVIVED"
                ok = False
            elif command:
                status = command_status(output, mutation["expect"])
                if status != "KILLED":
                    ok = False
                    print(output[-600:], flush=True)
            elif result.returncode == 0 and tests_run(output) == 0:
                status = "NO-TESTS"
                ok = False
            elif result.returncode == 0:
                status = "SURVIVED"
                ok = False
            elif build_failed(output):
                status = "KILLED-BUILD"
                ok = False
            else:
                status = "KILLED"
        except subprocess.TimeoutExpired:
            status = "TIMEOUT"
        finally:
            source.write_text(original)
        elapsed = time.monotonic() - started
        print(f"{status:12} {mutation['name']} [{scope}] ({elapsed:.0f}s)", flush=True)
    return ok


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("lists", nargs="+", type=Path, help="mutant list files")
    parser.add_argument("--revision", default="HEAD")
    parser.add_argument("--scratch", type=Path,
                        default=Path(tempfile.gettempdir()) / "chimi-mutation")
    parser.add_argument("--only", action="append", default=[], help="run only this mutant name")
    parser.add_argument("--timeout", type=int, default=600, help="seconds per cargo test run")
    parser.add_argument("--fresh-baseline", action="store_true",
                        help="rerun baselines even if they passed at this commit before")
    options = parser.parse_args()

    root = repository_root()
    tree = options.scratch / "tree"
    commit = export_tree(root, options.revision, tree)
    print(f"# mutation run at {commit}", flush=True)
    records = options.scratch / "baselines"
    if options.fresh_baseline and records.exists():
        shutil.rmtree(records)
    ok = True
    for path in options.lists:
        ok &= run_list(path.resolve(), tree, options.scratch / "target", records, commit,
                       options.only, options.timeout)
    print("# result:", "every mutant killed" if ok else "FAILED (see SURVIVED/NO-TESTS/STALE/BASELINE)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
