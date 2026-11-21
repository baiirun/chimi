"""Mutants of R4 step 3 PR 5c-1: one binding call at a time runs on a handle,
and a call lets its handles go while it waits.

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/binding_run_mutations.py
Results: scripts/mutation/binding_run_results.txt.
"""

CRATE = "compiler/chimi"

C = "runtime/native/runtime.c"
THREADS = "the_runtime_state_holds_across_carrier_threads"


def m(name, old, new, targets=("runtime_state",), filter=THREADS):
    return dict(name=name, file=C, edits=[(old, new)], targets=list(targets), filter=filter)


MUTATIONS = [
    m("two calls run on one handle at once",
      "    if (entry->runner != NULL && entry->runner != call) return true;",
      "    (void)entry;"),
    m("a call waits for itself on a handle it names twice",
      "    if (entry->runner != NULL && entry->runner != call) return true;",
      "    if (entry->runner != NULL) return true;"),
    m("a call that stops running wakes no waiter",
      "  if (stopped) pthread_cond_broadcast(&binding_handle_free);",
      "  (void)stopped;"),
    m("a descriptor wait keeps its handles",
      "  binding_run(call, false);\n  int answer = binding_wait_fd(call, handle, fd, interest);",
      "  int answer = binding_wait_fd(call, handle, fd, interest);"),
    m("a descriptor wait does not take its handles back",
      "  int answer = binding_wait_fd(call, handle, fd, interest);\n  binding_run(call, true);",
      "  int answer = binding_wait_fd(call, handle, fd, interest);"),
    m("a child wait keeps its handles",
      "  binding_run(call, false);\n  int answer = binding_wait_pid(call, handle, pid);",
      "  int answer = binding_wait_pid(call, handle, pid);"),
    m("a child wait does not take its handles back",
      "  int answer = binding_wait_pid(call, handle, pid);\n  binding_run(call, true);",
      "  int answer = binding_wait_pid(call, handle, pid);"),
    m("a call never runs on its handles",
      "      entry->runner = call;\n",
      "      (void)entry;\n"),
    m("a terminal request keeps the call's handles",
      "  if (terminal) binding_run(call, false);",
      "  (void)terminal;",
      targets=("bindings",), filter="a_forced_body_parked_in_a_binding_call_leaves_its_handle_to_other_calls"),
]
