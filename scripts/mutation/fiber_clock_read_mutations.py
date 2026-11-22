"""Mutants of `chimi_fiber_now`, the clock read a running fiber makes without
a scheduler request (Phase 7c step 7, PR 1).

Run: scripts/mutation/mutate.py scripts/mutation/fiber_clock_read_mutations.py
"""

CRATE = "runtime/chimi-fiber"


def mutant(name, file, edits, filter="reads_the", targets=("lib",)):
    return dict(name=name, file=file, edits=edits, targets=list(targets), filter=filter)


MUTATIONS = [
    mutant("the real clock reading ignores its origin", "src/scheduler/clock.rs", [(
        "            Self::Real { origin } => Instant(host_monotonic_nanos().saturating_sub(origin)),",
        "            Self::Real { origin } => Instant(host_monotonic_nanos().saturating_sub(origin / 2)),",
    )]),
    mutant("a fiber reads virtual time as the origin", "src/scheduler/clock.rs", [(
        "            Self::Virtual { now } => Reading::Virtual { now: *now },",
        "            Self::Virtual { now } => Reading::Virtual { now: { let _ = now; Instant::ORIGIN } },",
    )]),
    mutant("a resume publishes no clock", "src/scheduler.rs", [(
        "        clock::set_resume_clock(clock);\n",
        "        let _ = clock;\n",
    )]),
]
