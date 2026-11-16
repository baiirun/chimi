"""Mutants of `Select.first` and `std/select` (CHI-203): the scheduler's
`CHIMI_REQUEST_SELECT`, the channel and inbox watchers, the native runtime's
event commit and user-clock timers, and the checker's select contracts.

Run: scripts/mutation/mutate.py scripts/mutation/select_mutations.py
Each mutant must turn a test in tests/select.rs red. The scheduler mutants edit
runtime/chimi-fiber, which the native build links, so they are judged by what
a compiled Chimi program does. Results: scripts/mutation/select_results.txt.

Left out as equivalent: waking the watcher on abort (only the selecting task
owns the receiver, so it cannot be parked when its channel aborts), cancelling
the committed user timer too (its child has already ended), and the
`clear_select` in `disarm_wake_sources` (every later request clears it).
Withdrawing an abandoned select on any request is in
select_fiber_mutations.py, because its proof is a scheduler test.
"""

CRATE = "compiler/chimi"

FIBER = "../../runtime/chimi-fiber/src"
SELECT = f"{FIBER}/scheduler/select.rs"
CHANNEL = f"{FIBER}/channel.rs"
SCHEDULER = f"{FIBER}/scheduler.rs"
PROCESS = f"{FIBER}/scheduler/process.rs"
SCOPE = f"{FIBER}/scheduler/scope.rs"
RUNTIME = "runtime/native/runtime.c"
CONTRACTS = "src/typing/select.rs"
TYPING = "src/typing.rs"
JOINS = "src/typing/failure_sets.rs"


def select(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["select"])


MUTATIONS = [
    # List order decides among ready sources.
    select("sources are polled last to first", SELECT, [(
        "        for (index, source) in sources.iter().enumerate() {",
        "        for (index, source) in sources.iter().enumerate().rev() {")]),
    select("the reply names the first source", SELECT, [(
        "                    request.value = index as i64;",
        "                    request.value = 0;")]),
    # A parked select wakes on the earliest deadline.
    select("the wake timer is the latest deadline", SELECT, [(
        "at.min(deadline)", "at.max(deadline)")]),
    select("a parked select arms no wake timer", SELECT, [(
        "            let _ = self.start_wake_timer(owner, deadline);",
        "            let _ = deadline;")]),
    select("a passed deadline is not ready", SELECT, [(
        "                if deadline <= now {", "                if deadline < now {")]),
    # Channel readiness and its watcher.
    select("a closed or aborted channel is not ready", CHANNEL, [(
        "        let ready = channel.lifecycle != Lifecycle::Open\n            || !channel.queue.is_empty()",
        "        let ready = !channel.queue.is_empty()")]),
    select("a waiting sender does not make a channel ready", CHANNEL, [(
        "            || !channel.queue.is_empty()\n            || !channel.waiting_senders.is_empty();",
        "            || !channel.queue.is_empty();")]),
    select("a queued value does not wake the watcher", CHANNEL, [(
        "                    channel.queue.push_back(value);\n"
        "                    transition.wakes.extend(channel.select_watcher.take());\n",
        "                    channel.queue.push_back(value);\n")]),
    select("a parked sender does not wake the watcher", CHANNEL, [(
        "                        .push_back(WaitingSend { task, value });\n"
        "                    transition.wakes.extend(channel.select_watcher.take());\n",
        "                        .push_back(WaitingSend { task, value });\n")]),
    select("an inbox select registers no wake", PROCESS, [(
        "        let ready = record.stopping.is_some() || !record.mailbox.is_empty();\n"
        "        if !ready {\n            record.receiving = true;\n        }\n",
        "        let ready = record.stopping.is_some() || !record.mailbox.is_empty();\n")]),
    select("a finished child does not wake a selecting parent", SCHEDULER, [(
        "                self.wake_selecting_parent(parent, task);\n", "")]),
    select("a task source is ready before its child ends", SELECT, [(
        "                if matches!(self.tasks[&task].phase, TaskPhase::Terminal(_)) {",
        "                if true {")]),
    # Withdrawal.
    select("a firing scope cannot withdraw a select", SCOPE, [(
        "            | super::REQUEST_PROCESS_RECEIVE\n            | super::REQUEST_SELECT\n    )\n}\n\nimpl",
        "            | super::REQUEST_PROCESS_RECEIVE\n    )\n}\n\nimpl")]),
    select("user timers are never cancelled", RUNTIME, [(
        "    cancel_user_timers(runtime, sources, user_timer, count, committed, span, call_node, environment);",
        "    (void)cancel_user_timers;")]),
    select("a user clock timer uses the execution clock", RUNTIME, [(
        "        if (on_execution_clock(provider)) {\n          *entry",
        "        if (true) {\n          *entry")]),
    select("a committed receive skips the mapping function", RUNTIME, [(
        "  ChimiValue *mapped[] = {taken->value};\n  return invoke(",
        "  ChimiValue *mapped[] = {taken->value};\n  return taken;\n  return invoke(")]),
    select("losing user timers are cancelled before the take", RUNTIME, [(
        "    taken = select_take(runtime, event, timer_task, span, call_node, environment);\n  }\n",
        "    cancel_user_timers(runtime, sources, user_timer, count, committed, span, call_node, environment);\n"
        "    taken = select_take(runtime, event, timer_task, span, call_node, environment);\n  }\n")]),
    select("Select.first accepts an empty list at run time", RUNTIME, [(
        "  if (count == 0) return defect(\"`Select.first` needs at least one event\", call_node);",
        "  if (count == 0) return success(void_value());")]),
    # The checker.
    select("an empty list literal is accepted", CONTRACTS, [(
        "            && elements.is_empty()", "            && elements.is_empty() && false")]),
    select("a receive event cannot fail with Aborted", CONTRACTS, [(
        "self.failure_parameters(vec![failures], vec![aborted]);",
        "self.failure_parameters(vec![failures], Vec::new());")]),
    select("a task event drops the task's failures", CONTRACTS, [(
        "self.failure_parameters(vec![task_failures, failures], Vec::new());",
        "self.failure_parameters(vec![failures], Vec::new());")]),
    select("an event is not task-local", TYPING, [(
        "            Some(StandardSymbol::Type(StandardType::Task | StandardType::SelectEvent)) => {",
        "            Some(StandardSymbol::Type(StandardType::Task)) => {")]),
    select("event failure sets do not join", JOINS, [(
        "StandardType::Option | StandardType::Task | StandardType::SelectEvent",
        "StandardType::Option | StandardType::Task")]),
]
