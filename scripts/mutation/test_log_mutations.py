"""Mutants of `std/test` recorders and scripts (Phase 5, CHI-208, part 3b):
the log on both backends, handing each script reply out once, reporting an
exhausted script as an assertion, and rejecting values that are not
shareable (`T4403`).

Run: scripts/mutation/mutate.py scripts/mutation/test_log_mutations.py
Each mutant runs tests/testing.rs, the end-to-end conformance for both backends.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
TEST_LOG = "../../runtime/chimi-runtime/src/intrinsics/test_log.rs"
DEBUG_RENDER = "../../runtime/chimi-runtime/src/intrinsics/debug.rs"
I = "src/execution/core_operations.rs"
E = "src/executable.rs"
CAP = "src/capability.rs"


def mutant(name, file, edits, targets=("testing",), filter=""):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("native: a record is dropped", TEST_LOG, [(
        "                    Ok(copy) => logs.of(arguments[0]).values.push(copy),",
        "                    Ok(copy) => fragment::free(current_carrier(), copy),",
    )]),
    mutant("native: a script keeps only its first reply", TEST_LOG, [(
        "                        Ok(copy) => logs.of(script).values.push(copy),",
        "                        Ok(copy) if index == 0 => logs.of(script).values.push(copy),\n"
        "                        Ok(copy) => fragment::free(current_carrier(), copy),",
    )]),
    mutant("native: recorded misses the last value", TEST_LOG, [(
        "        let length = values.len() as u32;",
        "        let length = values.len().saturating_sub(1) as u32;",
    )]),
    mutant("native: recorded adopts the log", TEST_LOG, [(
        "                success(list_of(&mut logs.of(arguments[0]).values, true))",
        "                success(list_of(&mut logs.of(arguments[0]).values, false))",
    )]),
    mutant("native: next repeats its first reply", TEST_LOG, [(
        "                fragment::adopt(current_carrier(), log.values[log.taken], &mut root);\n"
        "                log.values[log.taken] = ptr::null_mut();\n"
        "                log.taken += 1;",
        "                fragment::copy_into_current(current_carrier(), log.values[log.taken], &mut root);",
    )]),
    mutant("native: an exhausted script returns Void", TEST_LOG, [(
        "                if log.taken == log.values.len() {\n",
        "                if log.taken == log.values.len() {\n                    return success(void_value());\n",
    )]),
    mutant("native: an exhausted script miscounts", TEST_LOG, [(
        "                        .integer(log.taken as i64)\n                        .text(b\" values\");",
        "                        .integer(log.taken as i64 + 1)\n                        .text(b\" values\");",
    )]),
    mutant("native: a recorder renders as a monitor", DEBUG_RENDER, [(
        'K::TEST_RECORDER => self.text(b"<Recorder>"),',
        'K::TEST_RECORDER => self.text(b"<Monitor>"),',
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
