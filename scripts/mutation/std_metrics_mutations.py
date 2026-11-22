"""Mutants of std/metrics (Phase 7c, CHI-289): the recording functions, the
runner's provider in std source, and the interpreter's and the native
runtime's aggregates.

Run: scripts/mutation/mutate.py scripts/mutation/std_metrics_mutations.py
"""

CRATE = "compiler/chimi"

S = "std/metrics.chimi"
H = "src/execution/host_operations.rs"
E = "src/execution.rs"
C = "runtime/native/runtime.c"


def mutant(name, file, edits, filter="", targets=("std_metrics",)):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("a negative count is allowed", S, [(
        "  case by < 0 {",
        "  case by < -1 {",
    )]),
    mutant("gauge builds an observation", S, [(
        "    kind: Kind.Gauge, name: name, value: value, labels: labels",
        "    kind: Kind.Observe, name: name, value: value, labels: labels",
    )]),
    mutant("the provider drops the labels", S, [(
        "          labels_key(measurement.labels),",
        '          "{}",',
    )]),
    mutant("seconds are milliseconds", S, [(
        "  Float.from_int(Duration.to_nanoseconds(duration)) / 1000000000.0",
        "  Float.from_int(Duration.to_nanoseconds(duration)) / 1000000.0",
    )]),
    mutant("the interpreter replaces a count", H, [(
        "            Series::Count(total) => *total = total.saturating_add(value as i64),",
        "            Series::Count(total) => *total = value as i64,",
    )]),
    mutant("the interpreter adds gauges", H, [(
        "            Series::Gauge(last) => *last = value,",
        "            Series::Gauge(last) => *last += value,",
    )]),
    mutant("the interpreter puts a bound's value in the next bucket", H, [(
        "                    .position(|bound| value <= *bound)",
        "                    .position(|bound| value < *bound)",
    )]),
    mutant("the interpreter lets a name change kind", H, [(
        "        if *self.kinds.entry(name.to_owned()).or_insert(kind) != kind {",
        "        if *self.kinds.entry(name.to_owned()).or_insert(kind) != kind && false {",
    )]),
    mutant("the interpreter keeps one label set too many", H, [(
        "            if *count == METRIC_SERIES_PER_NAME {",
        "            if *count > METRIC_SERIES_PER_NAME {",
    )]),
    mutant("the interpreter ignores the metrics file", E, [(
        "        if let Some(path) = &self.config.metrics_file {",
        "        if let Some(path) = &None::<String> {",
    )]),
    mutant("the native runtime replaces a count", C, [(
        "      series->total = series->total > INT64_MAX - by ? INT64_MAX : series->total + by;",
        "      series->total = by;",
    )]),
    mutant("the native runtime adds gauges", C, [(
        "    case 1: series->last = value; break;",
        "    case 1: series->last += value; break;",
    )]),
    mutant("the native runtime puts a bound's value in the next bucket", C, [(
        "      while (bucket < CHIMI_METRIC_BOUND_COUNT && !(value <= metric_bounds[bucket])) ++bucket;",
        "      while (bucket < CHIMI_METRIC_BOUND_COUNT && !(value < metric_bounds[bucket])) ++bucket;",
    )]),
    mutant("the native runtime lets a name change kind", C, [(
        "  if (metric->kind != kind) {",
        "  if (false) {",
    )]),
    mutant("the native runtime keeps one label set too many", C, [(
        "    if (metric->series == CHIMI_METRIC_SERIES_PER_NAME) {",
        "    if (metric->series > CHIMI_METRIC_SERIES_PER_NAME) {",
    )]),
    mutant("the native runtime matches a series by name only", C, [(
        "         memcmp(series->name, name, name_length) == 0 && memcmp(series->labels, labels, labels_length) == 0;",
        "         memcmp(series->name, name, name_length) == 0;",
    )]),
    mutant("the native runtime writes the series unsorted", C, [(
        "  qsort(order, metric_series_count, sizeof(*order), metric_series_order);",
        "",
    )]),
    mutant("the native runtime records without the lock", C, [(
        "  pthread_mutex_lock(&metrics_lock);\n  metrics_record(kind, name, name_length, value, labels, labels_length);\n  pthread_mutex_unlock(&metrics_lock);",
        "  metrics_record(kind, name, name_length, value, labels, labels_length);",
    )], targets=("std_metrics", "runtime_state")),
    mutant("the native metrics file may be the log file under another spelling", C, [(
        "    if (stat(metrics_path, &metrics_status) == 0 && !S_ISCHR(metrics_status.st_mode))",
        "    if (false)",
    )]),
    mutant("the native metrics file may be the profile file", C, [(
        "                       (profile_path && strcmp(metrics_path, profile_path) == 0) ||",
        "",
    )]),
    mutant("the native sum can become infinite", C, [(
        "      series->sum = isfinite(sum) ? sum : sum > 0 ? DBL_MAX : -DBL_MAX;",
        "      series->sum = sum;",
    )]),
    mutant("the interpreter's sum saturates at zero", H, [(
        "                *sum = if total.is_finite() { total } else { f64::MAX.copysign(total) };",
        "                *sum = if total.is_finite() { total } else { 0.0 };",
    )]),
    mutant("the native metrics file may be the memory report under another spelling", C, [(
        "      if (later[file] && stat(later[file], &open_status) == 0 &&",
        "      if (false && later[file] && stat(later[file], &open_status) == 0 &&",
    )]),
]
