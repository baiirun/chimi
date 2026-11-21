"""Mutants of R4 step 3 PR 6a: the lab's `--carriers` and block mark epochs.

Run: scripts/mutation/mutate.py --timeout 600 scripts/mutation/carrier_measure_mutations.py
Results: scripts/mutation/carrier_measure_results.txt.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
PERFORMANCE = "src/process_lab/performance.rs"
MOD = "src/process_lab/mod.rs"
BIN = "src/bin/process_lab.rs"


def lab(name, file, old, new, filter=""):
    return dict(name=name, file=file, edits=[(old, new)], targets=["process_lab"], filter=filter)


def epoch(name, old, new):
    # The carrier-threads fixture takes epochs and span ids on four threads
    # and checks that none repeats; the program suites cannot see a repeat.
    return dict(name=name, file=C, edits=[(old, new)], targets=["runtime_state"],
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
          "  if (carrier->next_epoch == carrier->end_epoch) {",
          "  if (false) {"),
    epoch("blocks overlap between carriers",
          "atomic_fetch_add_explicit(&next_mark_epoch, MARK_EPOCH_BLOCK, memory_order_relaxed);",
          "atomic_fetch_add_explicit(&next_mark_epoch, 1, memory_order_relaxed);"),
    epoch("a carrier never takes a span block",
          "  if (carrier->end_span - carrier->next_span < count) {",
          "  if (false) {"),
    epoch("span blocks overlap between carriers",
          "    carrier->next_span = atomic_fetch_add_explicit(&runtime->next_span, size, memory_order_relaxed);",
          "    carrier->next_span = atomic_fetch_add_explicit(&runtime->next_span, 1, memory_order_relaxed);"),
    epoch("one carrier's epochs restart at every block",
          "    carrier->next_epoch = start;\n",
          "    carrier->next_epoch = start - (start > 1 ? 1 : 0);\n"),
]
