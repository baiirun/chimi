"""Mutants of R4 step 3 PR 6a: the lab's `--carriers` and block mark epochs.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_measure_mutations.py
Results: scripts/mutation/carrier_measure_results.txt.
"""

CRATE = "compiler/chimi"

# The core node evaluators, `invoke` and the node frame are Rust since
# CHI-305 step 5, phase 7.
EVAL = "../../runtime/chimi-runtime/src/eval.rs"

C = "runtime/native/runtime.c"
PERFORMANCE = "src/process_lab/performance.rs"
MOD = "src/process_lab/mod.rs"
BIN = "src/bin/process_lab.rs"
COLLECTOR = "../../runtime/chimi-runtime/src/heap/collector.rs"


def lab(name, file, old, new, filter=""):
    return dict(name=name, file=file, edits=[(old, new)], targets=["process_lab"], filter=filter)


def epoch(name, old, new, file=C):
    # The carrier-threads fixture takes epochs and span ids on four threads
    # and checks that none repeats; the program suites cannot see a repeat.
    # Mark epochs are the Rust collector's since CHI-305 step 5 phase 2.
    return dict(name=name, file=file, edits=[(old, new)], targets=["runtime_state"],
                filter="across_carrier_threads")


MUTATIONS = [
    lab("every gate runs on several carriers", PERFORMANCE,
        '    matches!(id, "PG04" | "PG05" | "PG08")',
        "    !id.is_empty()",
        filter="a_virtual_clock_gate"),
    lab("no gate runs on several carriers", PERFORMANCE,
        '    matches!(id, "PG04" | "PG05" | "PG08")',
        "    id.is_empty()",
        filter="parked_memory"),
    lab("several carriers keep the virtual clock", PERFORMANCE,
        "            .arg(if run.real_clock || carriers() > 1 {",
        "            .arg(if run.real_clock {",
        filter="parked_memory"),
    lab("the count never reaches the runner", PERFORMANCE,
        "        if carriers() > 1 {\n            command.arg",
        "        if false {\n            command.arg",
        filter="parked_memory"),
    lab("the lab ignores the carrier count", MOD,
        "        && options.carriers > 1\n",
        "        && false\n",
        filter="a_virtual_clock_gate"),
    lab("a signed count is accepted", BIN,
        "                    .filter(|count| count.bytes().all(|byte| byte.is_ascii_digit()))\n",
        "",
        filter="malformed_arguments"),
    lab("0 carriers is accepted", BIN,
        "                    .filter(|count| (1..=256).contains(count))",
        "                    .filter(|count| *count <= 256)",
        filter="malformed_arguments"),
    lab("no upper bound", BIN,
        "                    .filter(|count| (1..=256).contains(count))",
        "                    .filter(|count| *count >= 1)",
        filter="malformed_arguments"),
    lab("--carriers without performance is accepted", BIN,
        "    if carriers_given && !measures_gates {",
        "    if false {",
        filter="without_performance"),
    epoch("a carrier never takes a block",
          "        if (*here).next_epoch == (*here).end_epoch {",
          "        if false {", COLLECTOR),
    epoch("blocks overlap between carriers",
          "            let start = NEXT_MARK_EPOCH.fetch_add(block, Relaxed);",
          "            let start = NEXT_MARK_EPOCH.fetch_add(1, Relaxed);", COLLECTOR),
    epoch("a carrier never takes a span block",
          "        if (*here).end_span.wrapping_sub((*here).next_span) < count {",
          "        if false {", EVAL),
    epoch("span blocks overlap between carriers",
          "            (*here).next_span = counter.fetch_add(size, Relaxed);",
          "            (*here).next_span = counter.fetch_add(1, Relaxed);", EVAL),
    epoch("one carrier's epochs restart at every block",
          "            (*here).next_epoch = start;\n",
          "            (*here).next_epoch = start - u64::from(start > 1);\n", COLLECTOR),
]
