"""Mutants of `std/test` fake process addresses (Phase 5, CHI-208, part 4b):
a send to a test address lands in its mailbox, a full mailbox refuses with
the message, `received` empties it, and the defects for a capacity below one,
a message carrying a Receiver, and the interpreter.

Run: scripts/mutation/mutate.py scripts/mutation/mailbox_mutations.py
Each mutant runs tests/testing.rs, the end-to-end conformance for both backends.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
CAPABILITIES = "../../runtime/chimi-runtime/src/intrinsics/capabilities.rs"
TEST_LOG = "../../runtime/chimi-runtime/src/intrinsics/test_log.rs"
DEBUG_RENDER = "../../runtime/chimi-runtime/src/intrinsics/debug.rs"
X = "src/execution.rs"
# Process.send is Rust since CHI-305 step 5, phase 7.
KERNEL = "../../runtime/chimi-runtime/src/intrinsics/process_kernel.rs"


def mutant(name, file, edits, targets=("testing",), filter="mailbox"):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("native: a send to a test address goes to the scheduler", KERNEL, [(
        "        if target.kind == ADDRESS_TEST {\n            return send_to_test_mailbox(",
        "        if false {\n            return send_to_test_mailbox(",
    )]),
    mutant("native: a mailbox holds one more than its capacity", TEST_LOG, [(
        "        if log.values.len() as i64 >= log.limit {",
        "        if log.values.len() as i64 > log.limit {",
    )]),
    mutant("native: Full carries no message", TEST_LOG, [(
        "            return failure(standard_value(StandardConstructor::PROCESS_SEND_FULL, &[message]));",
        "            return failure(standard_value(StandardConstructor::PROCESS_SEND_FULL + 1, &[message]));",
    )]),
    mutant("native: received keeps the messages", TEST_LOG, [(
        "                let list = list_of(&mut log.values, false);\n                log.values.clear();",
        "                let list = list_of(&mut log.values, true);",
    )]),
    mutant("native: a receiver is let through", TEST_LOG, [(
        "        if capabilities::carries_capability((*copy).roots[0].cast()) {",
        "        if false && capabilities::carries_capability((*copy).roots[0].cast()) {",
    )]),
    mutant("native: the capability check sees nothing", CAPABILITIES, [(
        "    count != 0\n",
        "    false\n",
    )]),
    mutant("native: a capacity of zero is allowed", TEST_LOG, [(
        "                if capacity < 1 {",
        "                if capacity < 0 {",
    )]),
    mutant("native: the mailbox's address names another log", TEST_LOG, [(
        "                (*address).payload.address.target = (*mailbox).payload.address.target;",
        "                (*address).payload.address.target = (*mailbox).payload.address.target + 1;",
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
