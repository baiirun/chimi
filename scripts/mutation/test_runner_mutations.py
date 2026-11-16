"""Mutants of the `chimi test` runner and the `std/test` assertions (Phase 5,
CHI-208): classifying outcomes, choosing a test's entry on each backend, the
report, the diff, and the assertions themselves.

Run: scripts/mutation/mutate.py scripts/mutation/test_runner_mutations.py
Each mutant runs tests/testing.rs (or the named unit tests); it must turn one red.
"""

CRATE = "compiler/chimi"

T = "src/testing.rs"
C = "runtime/native/runtime.c"
I = "src/execution/core_operations.rs"
B = "src/backend.rs"


def runner(name, file, edits, targets=("testing",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    runner("classify: no defect is an assertion", T, [(
        "let assertion = node.is_some_and(|node| self.assertion_calls.contains(&node));",
        "let assertion = false && node.is_some_and(|node| self.assertion_calls.contains(&node));",
    )]),
    runner("classify: a deadlock is an ordinary defect", T, [(
        "} else if message.starts_with(\"virtual-time deadlock\") {",
        "} else if message.starts_with(\"never\") {",
    )]),
    runner("classify: a failure's rendering is dropped", T, [(
        "RawOutcome::ExpectedFailure { rendering } => failure(FailureKind::Failure, rendering),",
        "RawOutcome::ExpectedFailure { .. } => failure(FailureKind::Failure, String::new()),",
    )]),
    runner("report: passing tests always show their output", T, [(
        "if !result.output.is_empty() && (show_output || result.failure.is_some()) {",
        "if !result.output.is_empty() {",
    )]),
    runner("report: no diff for multi-line renderings", T, [(
        "} else if actual.contains('\\n') || expected.contains('\\n') {",
        "} else if false {",
    )]),
    runner("diff: changed lines are not marked", T, [(
        "writeln!(output, \"+ {}\", right[j]).unwrap();",
        "writeln!(output, \"  {}\", right[j]).unwrap();",
    )], targets=("lib",), filter="testing::"),
    runner("parallel: results come back in finishing order", T, [(
        "            results[position] = Some(result);",
        "            let free = results.iter().position(Option::is_none).unwrap();\n            let _ = position;\n            results[free] = Some(result);",
    )]),
    runner("filter: filters are ignored", T, [(
        "filters.is_empty() || filters.iter().any(|filter| identity.contains(filter))",
        "true || filters.iter().any(|filter| identity.contains(filter))",
    )]),
    runner("interpreter: every test runs the first entry", B, [(
        "        core.entry = entry;\n",
        "        let _ = entry;\n",
    )]),
    runner("native: --test is ignored", C, [(
        "  if (test_index >= 0) {\n    entry_function",
        "  if (test_index >= 0 && false) {\n    entry_function",
    )]),
    runner("native: assert_equal passes on unequal values", C, [(
        "if ((chimi_value_compare(arguments[0], arguments[1]) == 0) == wanted)",
        "if ((chimi_value_compare(arguments[0], arguments[1]) == 0) == wanted || wanted)",
    )]),
    runner("native: a failed assert is not a defect", C, [(
        "      if (arguments[0]->as.boolean) return success(void_value());",
        "      return success(void_value());",
    )]),
    runner("interpreter: assert_not_equal never fails", I, [(
        "                if equal == wanted {",
        "                if equal == wanted || !wanted {",
    )]),
]
