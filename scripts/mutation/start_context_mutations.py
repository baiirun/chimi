"""Mutants of start-context ownership (CHI-308 step 4): once a fiber issues
a SUPERVISOR_START, START_PROCESS or START_SUPERVISOR request, the scheduler
owns every specification context it names, served or not, and the host never
frees one it sent.

Run: scripts/mutation/mutate.py scripts/mutation/start_context_mutations.py
Each must fail a chimi-fiber unit test in process_tests.rs: the three
`released_by_unserved_start` tests (a caller defected by its own queued
release, or by a capability violation in a handoff),
`a_forced_body_abandoned_at_a_start_still_releases_its_context`,
`a_start_issued_in_a_cancellation_unwind_still_releases_its_context` and
`a_policy_refused_for_its_depth_releases_its_deepest_context`.
Results: scripts/mutation/start_context_results.txt.

Not listed: the release on a scope refusal. Starts are not cancellation
points, so `refuse_for_scope` never refuses one; the release keeps the rule
whole if that changes. The host side (runtime.c passes `true` to
`marshal_finish` after every issued start) frees nothing the scheduler
released only under a sanitizer, so no ordinary test sees a revert; the
sanitized suite's leak and double-free checks cover it.
"""

CRATE = "runtime/chimi-fiber"

S = "src/scheduler.rs"
ABI = "src/scheduler/process_abi.rs"
PROCESS = "src/scheduler/process.rs"


def unit(name, file, edits, test_filter="process_tests"):
    return dict(name=name, file=file, edits=edits, targets=["lib"], filter=test_filter)


MUTATIONS = [
    unit("a start its caller's releases defect keeps its contexts", S, [(
        "        if !self.apply_task_releases(owner) {\n            self.release_spec_contexts(unserved_start);\n",
        "        if !self.apply_task_releases(owner) {\n",
    )]),
    unit("a forced body abandoned at a start keeps its contexts", S, [(
        "                self.release_abandoned_start(event.request);\n",
        "",
    )]),
    unit("an unwind abandoned at a start keeps its contexts", S, [(
        "            self.release_abandoned_start(cleanup.request);\n",
        "",
    )]),
    unit("an abandoned start's contexts are not released", S, [(
        "            self.release_spec_contexts(Some(contexts));\n        }\n    }\n\n    /// Records an end",
        "            drop(contexts);\n        }\n    }\n\n    /// Records an end",
    )]),
    # A depth cut-off like `read_policy`'s, at the 64 levels it refuses.
    unit("a policy past the depth limit keeps its deepest contexts", ABI, [
        ("    policy: *const ChimiSupervisorPolicy,\n    visit: &mut impl FnMut(*mut c_void),\n) {\n",
         "    policy: *const ChimiSupervisorPolicy,\n    visit: &mut impl FnMut(*mut c_void),\n) {\n    static DEPTH: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);\n    if DEPTH.fetch_add(1, std::sync::atomic::Ordering::Relaxed) > MAX_POLICY_DEPTH {\n        return;\n    }\n"),
    ]),
    unit("a handoff refused for a violation keeps its context", ABI, [(
        "                        self.release_spec_context(spec.context);\n",
        "",
    )]),
    unit("a start process names no context", ABI, [(
        "                contexts.push(spec.context);\n",
        "                let _ = spec;\n",
    )]),
    unit("a supervisor start names no contexts", ABI, [(
        "            unsafe { visit_policy_contexts(input.policy, &mut |context| contexts.push(context)) }\n",
        "            {}\n",
    )]),
    unit("a nested supervisor start is not a start", ABI, [(
        "            | super::REQUEST_SUPERVISOR_START_PROCESS\n            | super::REQUEST_SUPERVISOR_START_SUPERVISOR\n",
        "            | super::REQUEST_SUPERVISOR_START_PROCESS\n",
    )]),
    unit("unserved contexts are not released", PROCESS, [(
        "            self.release_spec_context(context);\n        }\n    }\n\n    /// Releases an accepted message",
        "            let _ = context;\n        }\n    }\n\n    /// Releases an accepted message",
    )]),
]
