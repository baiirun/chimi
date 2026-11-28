"""Mutants of the run in Rust (CHI-305 step 5, phase 8, fifteenth PR): the
benchmark repeats, the one run and the reports written when it ends.

Run: scripts/mutation/mutate.py scripts/mutation/run_program_mutations.py
Results: scripts/mutation/run_program_results.txt.
The clock-origin and metrics-start mutants are in event_times, metrics_export
and metrics_read, retargeted to run.rs.
"""

CRATE = "compiler/chimi"

RUN = "../../runtime/chimi-runtime/src/run.rs"
REPEATS = "native_benchmark_repeats_run_the_entry_afresh_each_time"


def m(name, old, new, target="cli", test=REPEATS):
    return dict(name=name, file=RUN, edits=[(old, new)], targets=[target], filter=test)


MUTATIONS = [
    m("a benchmark prints no mean",
      "        libc::fwrite(line.as_ptr().cast(), 1, mean.len() + 1, stderr);\n",
      ""),
    m("a repeat's sequence numbers carry over",
      "            (*runtime).next_sequence = 0;\n",
      ""),
    m("a repeat's spans carry over",
      "            (*runtime).next_span.store(2, Relaxed);\n",
      ""),
    m("the metrics start at each repeat",
      "            if repeat == 0 {",
      "            if true {"),
    m("a benchmark writes no metrics",
      "        write_failed = write_metrics(runtime, metrics_started, (*runtime).metrics_file) || write_failed;",
      "        write_failed = false || write_failed;"),
    m("a benchmark ignores a failed write",
      "        if write_failed { IO_ERROR } else { 0 }",
      "        0"),
    m("a benchmark repeat traces no end",
      "            computation_event(runtime, c\"computation_completed\", function, control);\n        }",
      "        }"),
    m("the outcome record is not written",
      "        if !(*runtime).outcome.is_null() {",
      "        if false {",
      "cli", "native_the_entry_is_main_and_refuses_arguments_it_cannot_take"),
    m("a failed write exits as the outcome says",
      "        if write_failed {\n            return IO_ERROR;",
      "        if false {\n            return IO_ERROR;",
      "std_metrics", "a_failed_metrics_write_is_a_sink_failure"),
    m("the entry gets no application arguments",
      "        if function.parameters_length == 1 {",
      "        if false {",
      "std_cli", ""),
    m("a run's end is not traced",
      "        computation_event(runtime, c\"computation_completed\", function, control);\n        if !(*runtime).outcome",
      "        if !(*runtime).outcome",
      "trace_show", ""),
    m("the application arguments start at the runner's",
      "values: argv.add(options.first_argument as usize) };",
      "values: argv };",
      "std_cli", ""),
]
