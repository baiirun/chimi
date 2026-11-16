"""Mutants of `std/test` recorders and scripts (Phase 5, CHI-208, part 3b):
the log on both backends, handing each script reply out once, reporting an
exhausted script as an assertion, and rejecting values that are not
shareable (`T4403`).

Run: scripts/mutation/mutate.py scripts/mutation/test_log_mutations.py
Each mutant runs tests/testing.rs, the end-to-end conformance for both backends.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
I = "src/execution/core_operations.rs"
E = "src/executable.rs"
CAP = "src/capability.rs"


def mutant(name, file, edits, targets=("testing",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("native: a record is dropped", C, [(
        "  log->values[log->length++] = fragment;",
        "  if (log->length == 0) log->values[log->length++] = fragment;",
    )]),
    mutant("native: recorded misses the last value", C, [(
        "      ChimiValue *list = list_from_values(values, log->length);",
        "      ChimiValue *list = list_from_values(values, log->length ? log->length - 1 : 0);",
    )]),
    mutant("native: next repeats its first reply", C, [(
        "      fragment_adopt(log->values[log->taken], &root);\n      log->values[log->taken++] = NULL;",
        "      fragment_copy_into_current(log->values[log->taken], &root);",
    )]),
    mutant("native: an exhausted script returns Void", C, [(
        "      if (log->taken == log->length)\n        return defect(",
        "      if (log->taken == log->length)\n        return success(void_value());\n      if (0)\n        return defect(",
    )]),
    mutant("native: a recorder renders as a monitor", C, [(
        '    case VALUE_TEST_RECORDER: debug_text(buffer, "<Recorder>"); return;',
        '    case VALUE_TEST_RECORDER: debug_text(buffer, "<Monitor>"); return;',
    )]),
    mutant("interpreter: recorded reverses the log", I, [(
        "                Control::Success(RuntimeValue::List(values.0.iter().cloned().collect()))",
        "                Control::Success(RuntimeValue::List(values.0.iter().rev().cloned().collect()))",
    )]),
    mutant("interpreter: next does not count its replies", I, [(
        "                        values.1 += 1;\n",
        "",
    )]),
    mutant("assertion set: an exhausted script is a plain defect", E, [(
        "                        | StandardFunction::TestCaptureDefect\n                        | StandardFunction::TestNext",
        "                        | StandardFunction::TestCaptureDefect",
    )]),
    mutant("T4403: a recorded value is never checked", CAP, [(
        '                            self.check_test_log_value(module_id, "Test.record", argument, value);',
        "",
    )]),
    mutant("T4403: a script is never checked", CAP, [(
        '                            self.check_test_log_value(module_id, "Test.script", argument, replies);',
        "",
    )]),
]
