"""Mutants of `std/test` fake process addresses (Phase 5, CHI-208, part 4b):
a send to a test address lands in its mailbox, a full mailbox refuses with
the message, `received` empties it, and the defects for a capacity below one,
a message carrying a Receiver, and the interpreter.

Run: scripts/mutation/mutate.py scripts/mutation/mailbox_mutations.py
Each mutant runs tests/testing.rs, the end-to-end conformance for both backends.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
DEBUG_RENDER = "../../runtime/chimi-runtime/src/intrinsics/debug.rs"
X = "src/execution.rs"


def mutant(name, file, edits, targets=("testing",), filter="mailbox"):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("native: a send to a test address goes to the scheduler", C, [(
        "      if (arguments[0]->as.address.kind == CHIMI_ADDRESS_TEST)\n        return send_to_test_mailbox(",
        "      if (0)\n        return send_to_test_mailbox(",
    )]),
    mutant("native: a mailbox holds one more than its capacity", C, [(
        "  if (log->length >= log->limit)",
        "  if (log->length > log->limit)",
    )]),
    mutant("native: Full carries no message", C, [(
        "    return failure(standard_value(CHIMI_CONSTRUCTOR_PROCESS_SEND_FULL, fields, 1));\n  void *roots[] = {message};",
        "    return failure(standard_value(CHIMI_CONSTRUCTOR_PROCESS_SEND_UNAVAILABLE, fields, 1));\n  void *roots[] = {message};",
    )]),
    mutant("native: received keeps the messages", C, [(
        "      free(values);\n      log->length = 0;",
        "      free(values);\n      log->length = log->length;",
    )]),
    mutant("native: a receiver is let through", C, [(
        "  if (carried_count) {\n    fragment_free(fragment);\n    return defect(\"Process.send: a test mailbox",
        "  if (0) {\n    fragment_free(fragment);\n    return defect(\"Process.send: a test mailbox",
    )]),
    mutant("native: a capacity of zero is allowed", C, [(
        "      if (arguments[0]->as.integer < 1)\n        return defect(numeric_message(\"Test.mailbox:",
        "      if (arguments[0]->as.integer < 0)\n        return defect(numeric_message(\"Test.mailbox:",
    )]),
    mutant("native: the address renders as direct", DEBUG_RENDER, [(
        'ADDRESS_TEST => Some(b"test"),',
        'ADDRESS_TEST => None,',
    )]),
    mutant("native: a mailbox renders as a script", DEBUG_RENDER, [(
        'K::TEST_MAILBOX => self.text(b"<Mailbox>"),',
        'K::TEST_MAILBOX => self.text(b"<Script>"),',
    )]),
    mutant("interpreter: Test.mailbox names the wrong requirement", X, [(
        '"Test.{} requires the native backend, as processes do"',
        '"Test.{} requires the supervised process runtime"',
    )]),
]
