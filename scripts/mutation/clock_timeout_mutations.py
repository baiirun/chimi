"""Mutants of the clock, timeout, and cancellation-scope work (CHI-201):
`Instant`, `Timestamp`, the structural `Clock`, `--clock-start`,
`Task.timeout`, `Task.deadline`, and the scheduler's scopes.

Run: scripts/mutation/mutate.py scripts/mutation/clock_timeout_mutations.py
Each mutant must turn a test in tests/clock.rs red. The scheduler mutants edit
runtime/chimi-fiber, which the native build links, so they are judged by what
a compiled Chimi program does. Results: scripts/mutation/clock_timeout_results.txt.
"""

CRATE = "compiler/chimi"

SCOPE = "../../runtime/chimi-fiber/src/scheduler/scope.rs"
RUNTIME = "runtime/native/runtime.c"


def clock(name, file, edits, filter=""):
    return dict(name=name, file=file, edits=edits, targets=["clock"], filter=filter)


MUTATIONS = [
    # A scope converts only its own delivered cancellation.
    clock("a timeout converts any cancellation", RUNTIME, [(
        "    if (control->kind == CONTROL_CANCEL && exit.value == 1) return timed_out(limit, call_node);",
        "    if (control->kind == CONTROL_CANCEL) return timed_out(limit, call_node);")]),
    clock("a terminal cancellation still exits its scopes", RUNTIME, [(
        "  if (control->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal()) return control;\n",
        "")]),
    # Firing delivers the outermost fired scope.
    clock("the innermost fired scope is delivered", SCOPE, [(
        "        scopes.iter().position(|entry| entry.fired)\n",
        "        scopes.iter().rposition(|entry| entry.fired)\n")]),
    # Firing withdraws the current park.
    clock("a firing scope leaves the park in place", SCOPE, [(
        "            self.disarm_wake_sources(task);\n            self.deliver_scope_cancel(task);\n",
        "")]),
    clock("a receive cannot be withdrawn", SCOPE, [(
        "            | super::REQUEST_CHANNEL_RECEIVE\n            | super::REQUEST_PROCESS_RECEIVE",
        "            | super::REQUEST_PROCESS_RECEIVE")]),
    # Firing cancels the scope's children.
    clock("a firing scope leaves its children running", SCOPE, [(
        "                self.begin_terminal(child, TaskOutcome::Cancelled, false);\n",
        "")]),
    # A normal exit removes the scope's timer.
    clock("an exited scope keeps its timer", SCOPE, [(
        "            if let Some(timer) = entry.timer {\n                self.cancel_timer(timer);\n            }\n",
        "            let _ = entry.timer;\n")]),
    # Non-positive limits never start the operation.
    clock("a deadline equal to now starts the operation", RUNTIME, [(
        "  if (deadline <= now) return timed_out(limit, call_node);",
        "  if (deadline < now) return timed_out(limit, call_node);")]),
    clock("a deadline carries the deadline instead of the time to it", RUNTIME, [(
        "        if (__builtin_sub_overflow(deadline, now, &limit))",
        "        limit = deadline; if (0)")]),
    # Time functions read the bound provider.
    clock("time functions bypass the bound provider", RUNTIME, [(
        "  ChimiValue *provider = environment->requirements[CHIMI_CLOCK_REQUIREMENT_SLOT];\n"
        "  if (!provider) provider = execution_clock_value();",
        "  ChimiValue *provider = execution_clock_value();")]),
    clock("starting_at ignores its timestamp", RUNTIME, [(
        "      timestamp->as.callable->shift = shift;",
        "      timestamp->as.callable->shift = 0;")]),
    # --clock-start is virtual-only.
    clock("--clock-start is accepted with the real clock", RUNTIME, [(
        "    if (clock_start_given && !runtime.virtual_clock) {",
        "    if (clock_start_given && false) {")]),
    # Scope firing is its own boundary event.
    clock("scope firing is reported as a timer fire", SCOPE, [(
        "            kind: ClockEventKind::ScopeFired,",
        "            kind: ClockEventKind::TimerFired,")]),
]
