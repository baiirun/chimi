"""Mutants of the std/process and std/supervisor data constructors in Rust
(CHI-305 step 5, phase 7, ninth PR): the route that sends them to Rust, the
range checks, the defect values, and the double-registration check.

Run: scripts/mutation/mutate.py scripts/mutation/processes_mutations.py
Results: scripts/mutation/processes_results.txt.
"""

CRATE = "compiler/chimi"

PROCESSES = "../../runtime/chimi-runtime/src/intrinsics/processes.rs"
DISPATCH = "../../runtime/chimi-runtime/src/intrinsics/dispatch.rs"
FILTER = "native_process_data_constructors"


def mutant(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["cli"], filter=FILTER)


MUTATIONS = [
    mutant("the supervisor constructors go to C", DISPATCH, [(
        "| Intrinsic::PROCESS_HANDOFF_SPEC..=Intrinsic::SUPERVISOR_RESTARTS => {",
        "| Intrinsic::PROCESS_HANDOFF_SPEC..=Intrinsic::PROCESS_HANDOFF_SPEC => {",
    )]),
    mutant("a spec accepts a zero capacity", PROCESSES, [(
        "            Intrinsic::PROCESS_SPEC if shape(&spec_shape) => {\n                if integer(1) < 1 {",
        "            Intrinsic::PROCESS_SPEC if shape(&spec_shape) => {\n                if integer(1) < 0 {",
    )]),
    mutant("a handoff spec accepts a negative shutdown", PROCESSES, [(
        "                        b\"Process.handoff_spec: shutdown_within must be at least zero, got \",",
        "                        b\"Process.spec: shutdown_within must be at least zero, got \",",
    )]),
    mutant("a restart window of zero is accepted", PROCESSES, [(
        "                if integer(1) <= 0 {", "                if integer(1) < 0 {",
    )]),
    mutant("a negative restart limit is accepted", PROCESSES, [(
        "                if integer(0) < 0 {", "                if integer(0) < -1 {",
    )]),
    mutant("a policy reports its member count as the capacity", PROCESSES, [(
        "                        .integer(i64::from(members))", "                        .integer(capacity)",
    )]),
    mutant("members equal to capacity are refused", PROCESSES, [(
        "                if i64::from(members) > capacity {", "                if i64::from(members) >= capacity {",
    )]),
    mutant("a named spec can be registered again", PROCESSES, [(
        "                if (*name).payload.constructed.length != 0 {", "                if false {",
    )]),
    mutant("a duration defect reports the integer", PROCESSES, [(
        "    let message: *const c_char = Message::new().text(heading).text(text.as_bytes()).finish();",
        "    let message: *const c_char = Message::new().text(heading).integer(nanoseconds).finish();",
    )]),
]
