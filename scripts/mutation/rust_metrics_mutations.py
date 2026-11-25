"""Mutants of the shared std/metrics aggregate (CHI-307):
runtime/chimi-runtime/src/metrics.rs and its C glue in runtime.c.

Both backends record into this code, so comparing them cannot see a mutant
here; the runtime fuzz corpus replay (tests/runtime_fuzz.rs), which checks each
document against its recorded one and against the C code the Rust replaced,
must. Each mutant targets only that replay.

Run: scripts/mutation/mutate.py scripts/mutation/rust_metrics_mutations.py
"""

CRATE = "compiler/chimi"

METRICS = "../../runtime/chimi-runtime/src/metrics.rs"
RUNTIME = "runtime/native/runtime.c"


def fuzz(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["runtime_fuzz"], filter="")


MUTATIONS = [
    fuzz("kind conflicts are recorded", METRICS,
         [("        if metric.kind != kind {\n            self.dropped_kind_conflicts += 1;\n            return;\n        }\n", "")]),
    fuzz("kind conflicts are not counted", METRICS,
         [("            self.dropped_kind_conflicts += 1;\n", "")]),
    fuzz("a gauge adds", METRICS,
         [("Series::Gauge(last) => *last = value,", "Series::Gauge(last) => *last += value,")]),
    fuzz("counts wrap", METRICS,
         [("*total = total.saturating_add(value as i64)", "*total = total.wrapping_add(value as i64)")]),
    fuzz("sums are not clamped", METRICS,
         [("*sum = if total.is_finite() { total } else { f64::MAX.copysign(total) };", "*sum = if total.is_finite() { total } else { f64::MAX };")]),
    fuzz("buckets are exclusive", METRICS,
         [(".position(|bound| value <= *bound)", ".position(|bound| value < *bound)")]),
    fuzz("kind 3 is a gauge", METRICS,
         [("                GAUGE => Series::Gauge(0.0),", "                GAUGE | 3 => Series::Gauge(0.0),")]),
    fuzz("observe count not kept", METRICS,
         [("                *count += 1;\n", "")]),
    fuzz("the last bucket count is dropped", METRICS,
         [("for (index, bucket) in buckets.iter().enumerate() {", "for (index, bucket) in buckets.iter().take(BOUNDS.len()).enumerate() {")]),
    fuzz("the start time is the end time", METRICS,
         [("        push_integer(&mut out, start_unix_ns);", "        push_integer(&mut out, time_unix_ns);")]),
    fuzz("negative times lose their sign", METRICS,
         [("    if value < 0 {\n        out.push('-');\n    }\n", "")]),
]
