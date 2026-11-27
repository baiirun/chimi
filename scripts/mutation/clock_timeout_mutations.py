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
CLOCKS = "../../runtime/chimi-runtime/src/intrinsics/clocks.rs"
EVAL = "../../runtime/chimi-runtime/src/eval.rs"


def clock(name, file, edits, filter=""):
    return dict(name=name, file=file, edits=edits, targets=["clock"], filter=filter)


MUTATIONS = [
    # A scope converts only its own delivered cancellation.
    clock("a timeout converts any cancellation", CLOCKS, [(
        "            if was_cancelled && exit.value == 1 {",
        "            if was_cancelled {")]),
    clock("a terminal cancellation still exits its scopes", CLOCKS, [(
        "        if was_cancelled && cancel_is_terminal() {\n            return control;\n        }\n",
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
        "        0 | super::REQUEST_TIMER_SLEEP_UNTIL\n            | super::REQUEST_TASK_RESULT\n"
        "            | super::REQUEST_CHANNEL_SEND\n            | super::REQUEST_CHANNEL_RECEIVE\n",
        "        0 | super::REQUEST_TIMER_SLEEP_UNTIL\n            | super::REQUEST_TASK_RESULT\n"
        "            | super::REQUEST_CHANNEL_SEND\n")]),
    # Firing cancels the scope's children.
    clock("a firing scope leaves its children running", SCOPE, [(
        "                self.begin_terminal(child, TaskOutcome::Cancelled, false);\n",
        "")]),
    # A normal exit removes the scope's timer.
    clock("an exited scope keeps its timer", SCOPE, [(
        "        for entry in removed {\n            if let Some(timer) = entry.timer {\n"
        "                self.cancel_timer(timer);\n            }\n",
        "        for entry in removed {\n            if let Some(timer) = entry.timer {\n"
        "                let _ = timer;\n            }\n")]),
    # Under the real clock a loop that never parks polls for its deadlines.
    clock("a busy loop never polls for scope deadlines", EVAL, [(
        "        chimi_fiber_scope_cancel_pending() != 0 || (chimi_fiber_scope_polls() != 0 && {",
        "        chimi_fiber_scope_cancel_pending() != 0 || (false && {")]),
    clock("requests do not fire the task's due scope deadlines", SCOPE, [(
        "        if !self.polls_scope_deadlines(task) {\n            return;\n        }\n        let now",
        "        if true {\n            return;\n        }\n        let now")]),
    # Non-positive limits never start the operation.
    clock("a deadline equal to now starts the operation", CLOCKS, [(
        "        if deadline <= now {",
        "        if deadline < now {")]),
    clock("a deadline carries the deadline instead of the time to it", CLOCKS, [(
        "                let Some(limit) = deadline.checked_sub(now) else {",
        "                let Some(limit) = Some(deadline) else {")]),
    # Time functions read the bound provider.
    clock("time functions bypass the bound provider", CLOCKS, [(
        "        let mut provider = *(*call.environment).requirements.add(CHIMI_CLOCK_REQUIREMENT_SLOT as usize);\n"
        "        if provider.is_null() {\n"
        "            provider = execution_clock_value();\n"
        "        }",
        "        let provider = execution_clock_value();")]),
    clock("starting_at ignores its timestamp", CLOCKS, [(
        "        (*shifted).shift = shift;",
        "        (*shifted).shift = 0;")]),
    # --clock-start is virtual-only.
    clock("--clock-start is accepted with the real clock", RUNTIME, [(
        "    if (clock_start_given && !runtime.virtual_clock) {",
        "    if (clock_start_given && false) {")]),
    # Scope firing is its own boundary event.
    clock("scope firing is reported as a timer fire", SCOPE, [(
        "            kind: ClockEventKind::ScopeFired,",
        "            kind: ClockEventKind::TimerFired,")]),
]
