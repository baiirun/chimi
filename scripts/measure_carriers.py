#!/usr/bin/env python3
"""Measures programs on 1, 2, 4 ... carriers (parallel-scheduler.md, step 3 PR 6).

These measurements, each at every carrier count given:

- The performance lab's PG04, PG05 and PG08 (`chimi-process-lab performance
  --carriers <n>`), the gates whose workload runs under the real clock.
- Parallel work, `scripts/carriers/app/fan_out.chimi`: eight processes each
  do one job, counting (no allocation) or building lists (allocation). The
  wall time and its speedup over one carrier.
- The host's own ceiling: n one-carrier copies of the counting job at once,
  each with 1/n of the work, so a host with n free cores runs them in 1/n of
  the one-carrier time. Cloud hosts often give less.
- Serial messaging, `scripts/carriers/app/ping_pong.chimi`: ns per round
  trip between main and two echo processes, the difference between two sizes
  divided by the difference in rounds, so process start cancels out.
- Parallel messaging, `scripts/carriers/app/pairs.chimi`: eight client and
  echo pairs, each pair on one carrier, exchange --pair-rounds round trips
  each at once. The wall time; the pairs share only the kernel.

Every timing is the median of --samples runs after one unmeasured run, and
every run must end with its expected outcome. Usage, from the repository
root with release binaries built:

    scripts/measure_carriers.py --chimi target/release/chimi \\
        --lab target/release/chimi-process-lab --carriers 1,2,4
"""

import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

ROOT = Path(__file__).resolve().parent / "carriers"


def build(chimi, entry, output):
    # The build writes its artifacts under target/chimi of the current
    # directory, so it runs in the scratch directory.
    subprocess.run(
        [os.path.abspath(chimi), "build", "--root", str(ROOT), entry, "-o", str(output)],
        check=True,
        cwd=output.parent,
        stdout=subprocess.DEVNULL,
    )


def run_once(executable, carriers, arguments, expected, scratch):
    outcome = scratch / "outcome.jsonl"
    if outcome.exists():
        outcome.unlink()
    command = [str(executable), "--chimi-runner", "--clock=real", "--trace=off"]
    if carriers > 1:
        command.append(f"--carriers={carriers}")
    command += [f"--outcome-file={outcome}", "--", *arguments]
    started = time.perf_counter_ns()
    finished = subprocess.run(command, stdin=subprocess.DEVNULL, capture_output=True)
    elapsed = time.perf_counter_ns() - started
    if finished.returncode != 0:
        sys.exit(f"{' '.join(command)} exited {finished.returncode}: {finished.stderr.decode()}")
    line = json.loads(outcome.read_text().splitlines()[-1])
    if line.get("outcome") != "success" or line.get("value") != expected:
        sys.exit(f"{' '.join(command)} ended with {line}, expected value {expected}")
    return elapsed


def median_time(executable, carriers, arguments, expected, samples, scratch):
    run_once(executable, carriers, arguments, expected, scratch)
    return statistics.median(
        run_once(executable, carriers, arguments, expected, scratch) for _ in range(samples)
    )


def independent_copies(executable, copies, arguments, expected, samples, scratch):
    """The wall time of `copies` one-carrier runs at once: what this host
    gives `copies` threads with nothing shared, the ceiling for a speedup."""

    def once():
        outcomes = [scratch / f"copy-{index}.jsonl" for index in range(copies)]
        for outcome in outcomes:
            if outcome.exists():
                outcome.unlink()
        started = time.perf_counter_ns()
        children = [
            subprocess.Popen(
                [str(executable), "--chimi-runner", "--clock=real", "--trace=off",
                 f"--outcome-file={outcome}", "--", *arguments],
                stdin=subprocess.DEVNULL,
                stdout=subprocess.DEVNULL,
            )
            for outcome in outcomes
        ]
        if any(child.wait() != 0 for child in children):
            sys.exit(f"an independent copy of {executable} failed")
        elapsed = time.perf_counter_ns() - started
        for outcome in outcomes:
            line = json.loads(outcome.read_text().splitlines()[-1])
            if line.get("outcome") != "success" or line.get("value") != expected:
                sys.exit(f"an independent copy of {executable} ended with {line}, expected {expected}")
        return elapsed

    once()
    return statistics.median(once() for _ in range(samples))


def lab_gates(lab, carriers):
    gates = {}
    for scenario in ("PG04", "PG05", "PG08"):
        finished = subprocess.run(
            [lab, "performance", "--scenario", scenario, "--carriers", str(carriers)],
            capture_output=True,
            text=True,
        )
        # A gate over its limit is a result here, not an error; a scenario
        # that measured nothing is.
        report = json.loads(finished.stdout.splitlines()[-1]) if finished.stdout else {}
        measured = report.get("gates", [])
        if not measured:
            sys.exit(f"{scenario} measured nothing on {carriers} carriers: {finished.stdout}{finished.stderr}")
        for gate in measured:
            gates[gate["metric"]] = gate["value"]
            if gate["status"] != "pass":
                gates[gate["metric"] + "_over_limit"] = gate["limit"]
    return gates


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--chimi", required=True)
    parser.add_argument("--lab")
    parser.add_argument("--carriers", default="1,2,4")
    parser.add_argument("--samples", type=int, default=7)
    parser.add_argument("--fan-out-size", type=int, default=2_000_000)
    parser.add_argument("--list-size", type=int, default=20_000_000)
    parser.add_argument("--pair-rounds", type=int, default=20_000)
    options = parser.parse_args()
    counts = [int(count) for count in options.carriers.split(",")]

    results = {"cpus": os.cpu_count(), "carriers": counts, "rows": {}}
    with tempfile.TemporaryDirectory(prefix="chimi-measure-carriers-") as directory:
        scratch = Path(directory)
        fan_out = scratch / "fan_out"
        ping_pong = scratch / "ping_pong"
        pairs = scratch / "pairs"
        build(options.chimi, "app/fan_out", fan_out)
        build(options.chimi, "app/ping_pong", ping_pong)
        build(options.chimi, "app/pairs", pairs)
        for carriers in counts:
            row = {}
            row["fan_out_count_ms"] = median_time(
                fan_out, carriers, [str(options.fan_out_size), "0"],
                8 * options.fan_out_size, options.samples, scratch) / 1e6
            # n copies, each with 1/n of the counting work.
            row["independent_count_ms"] = independent_copies(
                fan_out, carriers, [str(options.fan_out_size // carriers), "0"],
                8 * (options.fan_out_size // carriers), options.samples, scratch) / 1e6
            row["fan_out_lists_ms"] = median_time(
                fan_out, carriers, [str(options.list_size), "1"],
                8 * options.list_size, options.samples, scratch) / 1e6
            small = median_time(ping_pong, carriers, ["10000"], 10_000, options.samples, scratch)
            large = median_time(ping_pong, carriers, ["100000"], 100_000, options.samples, scratch)
            row["ping_pong_ns_per_round_trip"] = (large - small) / 90_000
            row["pairs_ms"] = median_time(
                pairs, carriers, [str(options.pair_rounds)],
                8 * options.pair_rounds, options.samples, scratch) / 1e6
            if options.lab:
                row.update(lab_gates(options.lab, carriers))
            results["rows"][carriers] = row
            print(f"{carriers} carriers: {json.dumps(row)}", file=sys.stderr)
    print(json.dumps(results))


if __name__ == "__main__":
    main()
