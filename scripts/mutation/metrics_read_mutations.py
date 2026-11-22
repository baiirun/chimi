"""Mutants of reading the runner's metrics aggregate mid-run (CHI-296, PR 2).

Run: scripts/mutation/mutate.py scripts/mutation/metrics_read_mutations.py
"""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"
INTERPRETER = "src/execution/host_operations.rs"
STD = "std/metrics.chimi"


def mutant(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_metrics"], filter="")


MUTATIONS = [
    mutant("a native read does not turn recording on", RUNTIME, [(
        "  atomic_store_explicit(&metrics_reading, true, memory_order_release);\n",
        "",
    )]),
    mutant("native recording ignores the read flag", RUNTIME, [(
        "  return runtime->metrics_file != NULL || atomic_load_explicit(&metrics_reading, memory_order_acquire);",
        "  return runtime->metrics_file != NULL;",
    )]),
    mutant("a native read is stamped with the run's start", RUNTIME, [(
        "  if (__builtin_add_overflow(runtime->start_unix_ns, chimi_fiber_now(), &ended)) ended = INT64_MAX;",
        "  ended = runtime->start_unix_ns;",
    )]),
    mutant("a native read's start is not the run's", RUNTIME, [(
        "  metrics_started_unix_ns = runtime.start_unix_ns;\n",
        "",
    )]),
    mutant("an interpreter read does not turn recording on", INTERPRETER, [(
        "                self.metrics.reading = true;\n",
        "",
    )]),
    mutant("the interpreter's recording ignores the read flag", INTERPRETER, [(
        "                    self.config.metrics_file.is_some() || self.metrics.reading,",
        "                    self.config.metrics_file.is_some(),",
    )]),
    mutant("the provider decides once, when main starts", STD, [(
        "fn host() {\n  {\n    record: fn(measurement: Measurement) {\n      case host_enabled() {",
        "fn host() {\n  let enabled = host_enabled()\n  {\n    record: fn(measurement: Measurement) {\n      case enabled {",
    )]),
    mutant("the provider always records", STD, [(
        "      case host_enabled() {",
        "      case True {",
    )]),
]
