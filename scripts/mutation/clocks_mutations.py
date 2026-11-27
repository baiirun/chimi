"""Mutants of std/clock and the execution clock in Rust (CHI-305 step 5,
phase 7, seventh PR): the route that sends the clock host methods to Rust,
virtual wall time, a shifted clock's timestamp, a wait's timer on a user
Clock, and the Instant arithmetic. (clock_mutations.py and
clock_timeout_mutations.py cover sleeping and timeouts.)

Run: scripts/mutation/mutate.py scripts/mutation/clocks_mutations.py
Results: scripts/mutation/clocks_results.txt.
"""

CRATE = "compiler/chimi"

CLOCKS = "../../runtime/chimi-runtime/src/intrinsics/clocks.rs"
DISPATCH = "../../runtime/chimi-runtime/src/intrinsics/dispatch.rs"


def mutant(name, file, edits, targets=("clock", "wait")):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter="")


MUTATIONS = [
    mutant("the execution clock's now goes to C", DISPATCH, [(
        "(*callable).host >= Core::HOST_CLOCK_NOW",
        "(*callable).host > Core::HOST_CLOCK_NOW",
    )]),
    mutant("virtual wall time ignores --clock-start", CLOCKS, [(
        "let Some(wall) = (*runtime).clock_start.checked_add(now) else {",
        "let Some(wall) = Some(now) else {",
    )], ("clock", "cli")),
    mutant("a shifted clock's timestamp is its now", CLOCKS, [(
        ".payload.integer.checked_add((*callable).shift) {",
        ".payload.integer.checked_add(0) {",
    )]),
    mutant("a wait's user-clock timer sleeps until zero", CLOCKS, [(
        "call.call((*callable).inner, &mut [instant((*callable).shift)])",
        "call.call((*callable).inner, &mut [instant(0)])",
    )]),
    mutant("Clock.between subtracts the wrong way", CLOCKS, [(
        "            Intrinsic::CLOCK_BETWEEN if shape(&[ValueKind::INSTANT, ValueKind::INSTANT]) => {\n                match integer(1).checked_sub(integer(0)) {",
        "            Intrinsic::CLOCK_BETWEEN if shape(&[ValueKind::INSTANT, ValueKind::INSTANT]) => {\n                match integer(0).checked_sub(integer(1)) {",
    )], ("clock", "language")),
]
