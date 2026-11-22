"""Mutants of the metrics document's run times and `chimi metrics export
--otlp` (Phase 7c step 7, PR 3).

Run: scripts/mutation/mutate.py scripts/mutation/metrics_export_mutations.py
"""

CRATE = "compiler/chimi"

EXPORT = "src/metrics_export.rs"
RUNTIME = "runtime/native/runtime.c"


def mutant(name, file, edits, filter=""):
    return dict(name=name, file=file, edits=edits, targets=["std_metrics"], filter=filter)


MUTATIONS = [
    mutant("the document's end ignores the run's time", RUNTIME, [(
        "  if (__builtin_add_overflow(runtime->start_unix_ns, chimi_fiber_now(), &ended)) ended = INT64_MAX;",
        "  ended = runtime->start_unix_ns;",
    )]),
    mutant("the start is taken when tracing only", RUNTIME, [(
        "  install_stop_signals(&runtime);\n  runtime.start_unix_ns = start_unix_nanos(&runtime);",
        "  install_stop_signals(&runtime);\n  if (!runtime.silent) runtime.start_unix_ns = start_unix_nanos(&runtime);",
    )]),
    mutant("a count is not monotonic", EXPORT, [(
        ",\\\"aggregationTemporality\\\":{CUMULATIVE},\\\"isMonotonic\\\":true\"",
        ",\\\"aggregationTemporality\\\":{CUMULATIVE},\\\"isMonotonic\\\":false\"",
    )]),
    mutant("label sets become separate metrics", EXPORT, [(
        "metrics.chunk_by(|left, right| left.name == right.name && left.kind == right.kind)",
        "metrics.chunk_by(|_, _| false)",
    )]),
    mutant("data points end at the start", EXPORT, [(
        "            times.start, times.end\n",
        "            times.start, times.start\n",
    )]),
    mutant("dropped measurements go unreported", EXPORT, [(
        "        if dropped > 0 {",
        "        if dropped > u64::MAX - 1 {",
    )]),
    mutant("an untimed document exports", EXPORT, [(
        "        _ => Err(MetricsError(format!(\n            \"the document has no {key}; it was written by a runtime without run times\"\n        ))),",
        "        _ => Ok(\"0\"),",
    )]),
    mutant("a time before 1970 is called missing", EXPORT, [(
        "        Some(JsonData::Number(number)) if number.starts_with('-') => Err(MetricsError(format!(",
        "        Some(JsonData::Number(number)) if number.starts_with('?') => Err(MetricsError(format!(",
    )]),
]
