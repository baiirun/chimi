"""Mutants of `chimi test --trace` (Phase 5, CHI-208, part 5d): one
telemetry file per test, named by the test, cleared per traced run, with the
root computation named after the selected test on both backends.

Run: scripts/mutation/mutate.py scripts/mutation/test_trace_mutations.py
"""

CRATE = "compiler/chimi"

T = "src/testing.rs"
M = "src/main.rs"
C = "runtime/native/runtime.c"


def mutant(name, file, edits, targets=("testing",), filter="trace_"):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("native root computation stays test 0", C, [(
        "    runtime.computation = CHIMI_TEST_COMPUTATIONS[test_index];\n",
        "",
    )]),
    mutant("native gets no telemetry file", T, [(
        "                .arg(format!(\n                    \"--telemetry-file={}\",",
        "                .arg(format!(\n                    \"--memory-report-file={}\",",
    )]),
    mutant("native full traces only boundaries", T, [(
        "                    TraceLevel::Full => \"--trace=full\",",
        "                    TraceLevel::Full => \"--trace=boundaries\",",
    )]),
    mutant("interpreter traces at full for boundaries", T, [(
        "            Self::Boundaries => TraceMode::Boundaries,",
        "            Self::Boundaries => TraceMode::Full,",
    )]),
    mutant("interpreter writes no file", T, [(
        "                    std::fs::write(&path, report.render_telemetry_json_lines())",
        "                    Ok::<(), std::io::Error>(())",
    )]),
    mutant("native execution id is generated", T, [(
        "            .arg(format!(\"--execution-id={}\", Self::execution_id(index)))\n",
        "",
    )]),
    mutant("trace names share one file", T, [(
        "        let mut name = format!(\"{index}-\");",
        "        let mut name = String::new();",
    )]),
    mutant("a stale trace survives", M, [(
        "            let cleared = match std::fs::remove_dir_all(&directory) {",
        "            let cleared = match Ok::<(), std::io::Error>(()) {",
    )]),
    mutant("the report names a passing test's trace", T, [(
        "        if let Some(trace) = result.trace.as_ref().filter(|_| result.failure.is_some()) {",
        "        if let Some(trace) = result.trace.as_ref() {",
    )]),
    mutant("json omits the trace", T, [(
        "            output.push_str(\",\\\"trace\\\":\");",
        "            output.push_str(\",\\\"tracefile\\\":\");",
    )]),
    mutant("a bad trace level is off", M, [(
        "                _ => return usage(\"--trace must be off, boundaries, or full\"),",
        "                _ => None,",
    )]),
    mutant("off traces boundaries", M, [(
        "                \"off\" => None,",
        "                \"off\" => Some(chimi::testing::TraceLevel::Boundaries),",
    )]),
]
