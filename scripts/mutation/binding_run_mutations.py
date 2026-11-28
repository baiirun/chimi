"""Mutants of R4 step 3 PR 5c-1: one binding call at a time runs on a handle,
and a call lets its handles go while it waits. Since CHI-305 phase 8 the
call path is Rust (chimi-runtime intrinsics/binding_calls.rs).

Run: scripts/mutation/mutate.py --timeout 300 scripts/mutation/binding_run_mutations.py
Results: scripts/mutation/binding_run_results.txt.
"""

CRATE = "compiler/chimi"

CALLS = "../../runtime/chimi-runtime/src/intrinsics/binding_calls.rs"
THREADS = "the_runtime_state_holds_across_carrier_threads"


def m(name, old, new, targets=("runtime_state",), filter=THREADS):
    return dict(name=name, file=CALLS, edits=[(old, new)], targets=list(targets), filter=filter)


MUTATIONS = [
    m("two calls run on one handle at once",
      ".any(|index| held(call, index) && !chimi_handles_runner_v1(slot_of(call, index)).is_null())",
      ".any(|index| held(call, index) && false)"),
    m("a call that stops running wakes no waiter",
      "        if any && !running {\n            libc::pthread_cond_broadcast",
      "        if false && any && !running {\n            libc::pthread_cond_broadcast"),
    m("a call that starts running wakes the waiters instead",
      "        if any && !running {\n            libc::pthread_cond_broadcast",
      "        if any && running {\n            libc::pthread_cond_broadcast"),
    m("a descriptor wait keeps its handles",
      "        run(call, false);\n        let answer = wait_fd(call, handle, fd, interest);",
      "        let answer = wait_fd(call, handle, fd, interest);"),
    m("a descriptor wait does not take its handles back",
      "        let answer = wait_fd(call, handle, fd, interest);\n        run(call, true);",
      "        let answer = wait_fd(call, handle, fd, interest);"),
    m("a child wait keeps its handles",
      "        run(call, false);\n        let answer = wait_pid(call, handle, pid);",
      "        let answer = wait_pid(call, handle, pid);"),
    m("a child wait does not take its handles back",
      "        let answer = wait_pid(call, handle, pid);\n        run(call, true);",
      "        let answer = wait_pid(call, handle, pid);"),
    m("a call never runs on its handles",
      "        let runner = if running { call.cast::<c_void>() } else { ptr::null() };",
      "        let runner: *const c_void = ptr::null();"),
    m("a request keeps the call's handles",
      "        run(call, false);\n        let cancelled = scheduler::request(request);",
      "        let cancelled = scheduler::request(request);",
      targets=("bindings",), filter="a_forced_body_parked_in_a_binding_call_leaves_its_handle_to_other_calls"),
    # The verifier's survivor: a request that never takes the handles back.
    m("a request does not take the call's handles back",
      "        let cancelled = scheduler::request(request);\n        run(call, true);",
      "        let cancelled = scheduler::request(request);",
      targets=("bindings",), filter=""),
]
