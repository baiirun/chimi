"""Mutants of the root task's outcome and the entry lookup in Rust (CHI-305
step 5, phase 8, fourteenth PR).

Run: scripts/mutation/mutate.py scripts/mutation/root_outcome_mutations.py
Results: scripts/mutation/root_outcome_results.txt.
The root-overflow, escalation and --test mutants are in their own lists,
retargeted to task_entry.rs and startup.rs.
"""

CRATE = "compiler/chimi"

TASK_ENTRY = "../../runtime/chimi-runtime/src/intrinsics/task_entry.rs"
STARTUP = "../../runtime/chimi-runtime/src/startup.rs"


def m(name, file, old, new, target, test):
    return dict(name=name, file=file, edits=[(old, new)], targets=[target], filter=test)


MUTATIONS = [
    m("a cancelled root ends as a scheduler failure", TASK_ENTRY,
      "        if state == FiberResult::CANCELLED {\n            return cancelled();",
      "        if false {\n            return cancelled();",
      "std_signal", "a_stop_signal_runs_the_releases_of_main_itself"),
    m("a deadlock forgets its waiters", TASK_ENTRY,
      " && !(*runtime).deadlock_waiters.is_null() {",
      " && false {",
      "cli", "native_virtual_deadlock_lists_every_waiter"),
    m("a capability violation ends as a scheduler failure", TASK_ENTRY,
      "            if !violation.is_null() {",
      "            if false {",
      "cli", "native_backstop_a_stale_copy_at_its_owner_defects"),
    m("a parked root ends as a scheduler failure", TASK_ENTRY,
      "        if state == FiberResult::PARKED || (defected && code < 0) {",
      "        if defected && code < 0 {",
      "cli", "native_the_root_ends_with_its_own_defect_or_as_stuck"),
    m("a scheduler defect code is read as a stuck root's", TASK_ENTRY,
      "        if state == FiberResult::PARKED || (defected && code < 0) {",
      "        if state == FiberResult::PARKED {",
      "cli", "native_deadlocked_tasks_defect_the_root"),
    m("a root stack overflow ends as a scheduler failure", TASK_ENTRY,
      "        if state == FiberResult::STACK_OVERFLOW {",
      "        if false {",
      "cli", "native_a_root_overflowing_inside_a_loop_ends_with_a_stack_overflow_defect"),
    m("a defected task's result word is dropped", TASK_ENTRY,
      "        if defected && code > 0 {\n            return task_reply(code);",
      "        if false {\n            return task_reply(code);",
      "cli", "native_the_root_ends_with_its_own_defect_or_as_stuck"),
    m("a zero-input main takes application arguments", STARTUP,
      "        if (*found).parameters_length == 0 && options.first_argument < argc {",
      "        if false {",
      "cli", "native_the_entry_is_main_and_refuses_arguments_it_cannot_take"),
    m("a zero-input main refuses an empty argument list", STARTUP,
      "        if (*found).parameters_length == 0 && options.first_argument < argc {",
      "        if (*found).parameters_length == 0 && options.first_argument <= argc {",
      "cli", "native_the_entry_is_main_and_refuses_arguments_it_cannot_take"),
    m("the entry is the first function", STARTUP,
      ".is_some_and(|body| core::ptr::fn_addr_eq(body, entry))",
      ".is_some()",
      "cli", "native_the_entry_is_main_and_refuses_arguments_it_cannot_take"),
]
