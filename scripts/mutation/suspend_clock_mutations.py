"""Mutants of the real clock that counts host suspend time (CHI-216).

Run: scripts/mutation/mutate.py scripts/mutation/suspend_clock_mutations.py
Each mutant must turn the chimi-fiber clock tests red.
Results: scripts/mutation/suspend_clock_results.txt.
"""

CRATE = "runtime/chimi-fiber"

CLOCK = "src/scheduler/clock.rs"


def clock(name, edits):
    return dict(name=name, file=CLOCK, edits=edits, targets=["lib"], filter="scheduler::clock")


MUTATIONS = [
    clock("a real wait is not capped", [(
        "    Duration::from_nanos(left).min(MAX_REAL_WAIT)",
        "    Duration::from_nanos(left)")]),
    clock("a real wait sleeps the whole cap even when the deadline is nearer", [(
        "    Duration::from_nanos(left).min(MAX_REAL_WAIT)",
        "    Duration::from_nanos(left).max(MAX_REAL_WAIT)")]),
    clock("a past deadline wraps to a long wait", [(
        "    let left = u64::try_from(deadline.0.saturating_sub(now.0)).unwrap_or(0);",
        "    let left = deadline.0.saturating_sub(now.0) as u64;")]),
    clock("the real clock reads seconds as milliseconds", [(
        "        .saturating_mul(1_000_000_000)",
        "        .saturating_mul(1_000_000)")]),
    clock("the real clock drops the nanoseconds", [(
        "        .saturating_add(time.tv_nsec)",
        "        .saturating_add(0)")]),
]
