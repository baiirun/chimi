"""Mutants of supervised-process stage U10 (ts-1765ba): the forcing program,
memory flatness over restarts and memberships (MF02, MF03, and the
`memberships_live` report field), and the performance harness (its gates and
the pure helpers behind them). Each mutant breaks one guarantee a U10
scenario or the harness's unit tests claim, and must turn it red with the
failure it names.

Run: scripts/mutation/mutate.py scripts/mutation/process_u10_mutations.py
Results: compiler/chimi/tests/process-lab/MUTATION-RESULTS.txt.

Scenario mutants run one scenario through the lab binary with `--through
U10`, because CURRENT_STAGE stays at U9 until ts-8f201b decides PG02. Gate
mutants run a debug lab, whose time limits are 10 times the release ones; the
PG03 mutant adds 1 ms of real waiting per restart, far above even the debug
limit of 400 µs, so a loaded machine cannot hide it.
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
KERNEL = "../../runtime/chimi-fiber/src/scheduler/process.rs"
ABI = "../../runtime/chimi-fiber/src/scheduler/process_abi.rs"
PERFORMANCE = "src/process_lab/performance.rs"
LAB = "src/process_lab/mod.rs"

TRANSCRIPT = "expected.transcript line"
OUTCOME = "expected.outcome line"


def lab(name, file, scenario, expect, edits):
    """`expect` is the failure the mutant must cause, as the scenario's detail
    prints it; a compile error or another failure does not count as a kill."""
    command = ["cargo", "run", "--quiet", "--bin", "chimi-process-lab", "--",
               "--scenario", scenario, "--through", "U10"]
    return dict(name=f"{scenario}: {name}", file=file, edits=edits, command=command,
                expect=expect)


def unit(name, file, edits):
    return dict(name=f"harness: {name}", file=file, edits=edits, targets=["lib"],
                filter="process_lab::performance")


FINISH_REMOVAL = """                    if self.kernel.memberships.contains_key(&membership) {
                        self.release_slot(membership);
                        self.kernel.memberships.remove(&membership);
                    }
                    self.kernel.retired.remove(&membership);"""

MUTATIONS = [
    # FP01: the composite program's outcome and transcript.
    lab("admission allows one member over capacity", KERNEL, "FP01",
        [TRANSCRIPT, OUTCOME], [(
            "        if members >= supervision.policy.capacity.get() as usize {",
            "        if members > supervision.policy.capacity.get() as usize {")]),
    # MF02: an incarnation's heap must be destroyed when it ends.
    lab("an ended incarnation keeps its heap", RUNTIME, "MF02",
        ["heap_chunks_live differs"], [(
            "    process_heap_peak_value_bytes = heap->peak_live_bytes;\n  heap_destroy(heap);\n",
            "    process_heap_peak_value_bytes = heap->peak_live_bytes;\n")]),
    # MF03: removal must release the membership record, the capacity slot,
    # and the report must carry the kernel's own count.
    lab("removal keeps the membership record", KERNEL, "MF03",
        ["memberships_live is not 0"], [(
            FINISH_REMOVAL,
            FINISH_REMOVAL.replace(
                "                        self.kernel.memberships.remove(&membership);\n", ""))]),
    lab("removal keeps the capacity slot", KERNEL, "MF03",
        [OUTCOME], [(
            "            supervision.dynamic.retain(|m| *m != membership);\n", "")]),
    lab("memberships_live ignores the kernel", ABI, "MF03",
        ["memberships_live is not 0"], [(
            "    LAST_RUN_MEMBERSHIPS.store(scheduler.process_metrics().memberships, Ordering::Relaxed);\n",
            "    LAST_RUN_MEMBERSHIPS.store(usize::MAX, Ordering::Relaxed);\n")]),
    # Gates.
    lab("trace-off runs give the scheduler a trace mode", RUNTIME, "PG10",
        ["pg10_trace_off_events"], [(
            ".trace_mode = runtime->silent ? 0 : (runtime->full_trace ? 2 : 1),",
            ".trace_mode = runtime->silent ? 1 : (runtime->full_trace ? 2 : 1),")]),
    lab("each restart waits 1 ms of real time", KERNEL, "PG03",
        ["pg03_us_per_restart"], [(
            "        supervision.restart_times.push_back(now);\n",
            "        std::thread::sleep(std::time::Duration::from_millis(1));\n"
            "        supervision.restart_times.push_back(now);\n")]),
    # Harness helpers, through its unit tests.
    unit("p95 takes the median", PERFORMANCE, [(
        "    samples[samples.len() * 95 / 100]", "    samples[samples.len() / 2]")]),
    unit("an overhead ratio may fall below 1", PERFORMANCE, [(
        "    (measured / baseline).max(1.0)", "    measured / baseline")]),
    unit("the stop phase starts at the last termination", PERFORMANCE, [(
        "        if slot.is_some() {\n            continue;\n        }",
        "        if slot.is_some() && event != \"process_terminated\" {\n            continue;\n        }")]),
    unit("the baseline warns at exactly 1.5 times", PERFORMANCE, [(
        "            (gate.value > saved * 1.5).then(|| {",
        "            (gate.value >= saved * 1.5).then(|| {")]),
    dict(name="harness: a gate passes at its limit", file=LAB, targets=["lib"],
         filter="process_lab::performance", edits=[(
             "            passed: value < limit,", "            passed: value <= limit,")]),
]
