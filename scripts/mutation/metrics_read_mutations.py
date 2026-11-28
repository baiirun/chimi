"""Mutants of reading the runner's metrics aggregate mid-run (CHI-296, PR 2).

Run: scripts/mutation/mutate.py scripts/mutation/metrics_read_mutations.py
"""

CRATE = "compiler/chimi"

HOST = "../../runtime/chimi-runtime/src/intrinsics/metrics_host.rs"
RUN = "../../runtime/chimi-runtime/src/run.rs"
INTERPRETER = "src/execution/host_operations.rs"
STD = "std/metrics.chimi"


def mutant(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_metrics"], filter="")


MUTATIONS = [
    mutant("a native read does not turn recording on", HOST, [(
        "            READING.store(true, Ordering::Release);\n",
        "",
    )]),
    mutant("native recording ignores the read flag", HOST, [(
        "    !unsafe { (*runtime).metrics_file }.is_null() || READING.load(Ordering::Acquire)",
        "    !unsafe { (*runtime).metrics_file }.is_null()",
    )]),
    mutant("a native read is stamped with the run's start", HOST, [(
        "    let ended = unsafe { (*runtime).start_unix_ns.checked_add(chimi_fiber_now()) }.unwrap_or(i64::MAX);",
        "    let ended = unsafe { (*runtime).start_unix_ns };",
    )]),
    mutant("a native read's start is not the run's", RUN, [(
        "        chimi_runtime_metrics_started_v1((*runtime).start_unix_ns);\n",
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
