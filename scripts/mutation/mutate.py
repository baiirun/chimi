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
`filter` is passed to the test binaries.

Before any mutant, the unmutated tests must pass (BASELINE). Output is one line
per mutant: KILLED (a test failed), KILLED-BUILD (the mutant did not compile,
which proves nothing about the tests), SURVIVED, STALE, or TIMEOUT.
Exit status is 0 only when the baseline passes and every mutant is KILLED or
TIMEOUT.
"""

import argparse
import os
import runpy
import shutil
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
    environment = dict(os.environ, CARGO_TARGET_DIR=str(target_dir))
    return subprocess.run(
        command, cwd=crate, env=environment, capture_output=True, text=True, timeout=timeout
    )


def build_failed(output: str) -> bool:
    return "error[E" in output or "could not compile" in output or "error: linking" in output


def run_list(path: Path, tree: Path, target_dir: Path, only, timeout) -> bool:
    spec = runpy.run_path(str(path))
    crate = tree / spec["CRATE"]
    args = spec.get("CARGO_TEST_ARGS", [])
    mutations = [m for m in spec["MUTATIONS"] if not only or m["name"] in only]
    print(f"# {path.name}: {len(mutations)} mutants against {spec['CRATE']}", flush=True)

    started = time.monotonic()
    baseline = cargo_test(crate, args, [], "", target_dir, timeout)
    if baseline.returncode != 0:
        print(f"BASELINE FAILED ({time.monotonic() - started:.0f}s)", flush=True)
        print(baseline.stdout[-4000:] + baseline.stderr[-4000:])
        return False
    print(f"BASELINE ok ({time.monotonic() - started:.0f}s)", flush=True)

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
        scope = " ".join(targets) or "all targets"
        if test_filter:
            scope += f" -- {test_filter}"
        source.write_text(mutated)
        started = time.monotonic()
        try:
            result = cargo_test(crate, args, targets, test_filter, target_dir, timeout)
            output = result.stdout + result.stderr
            if result.returncode == 0:
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
    options = parser.parse_args()

    root = repository_root()
    tree = options.scratch / "tree"
    commit = export_tree(root, options.revision, tree)
    print(f"# mutation run at {commit}", flush=True)
    ok = True
    for path in options.lists:
        ok &= run_list(path.resolve(), tree, options.scratch / "target", options.only,
                       options.timeout)
    print("# result:", "every mutant killed" if ok else "FAILED (see SURVIVED/STALE/BASELINE)")
    return 0 if ok else 1


if __name__ == "__main__":
    sys.exit(main())
