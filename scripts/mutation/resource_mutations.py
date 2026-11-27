"""Mutants of `Resource.scoped` (G12) and of freeing a timed-out scope's
channels (CHI-215): the scheduler's masked resource entries and root-scope
cancellation, the channel registry's arrival stamps, the native runtime's
`resource_scoped`, and the interpreter's outcome order.

Run: scripts/mutation/mutate.py scripts/mutation/resource_mutations.py
The scheduler and runtime.c mutants are judged by compiled programs in
tests/resources.rs; the outcome-order mutants by the std-tests/resource
programs, which run on both backends. Results:
scripts/mutation/resource_results.txt.

Left out as unobservable: the interpreter calling `release` after a failed
acquire and ignoring its outcome. The interpreter has no scheduler and no
side effect a program can see besides its outcome.
"""

CRATE = "compiler/chimi"

FIBER = "../../runtime/chimi-fiber/src"
SCOPE = f"{FIBER}/scheduler/scope.rs"
SCHEDULER = f"{FIBER}/scheduler.rs"
CHANNEL = f"{FIBER}/channel.rs"
PROCESS = f"{FIBER}/scheduler/process.rs"
RUNTIME = "runtime/native/runtime.c"
SCOPES = "../../runtime/chimi-runtime/src/intrinsics/scopes.rs"
INTERPRETER = "src/execution/resource_operations.rs"


def resources(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["resources"])


def outcomes(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_modules"])


MUTATIONS = [
    # Masking: only scopes above the innermost masked entry are deliverable.
    resources("a mask does not hold off the scopes beneath it", SCOPE, [(
        "        let base = scopes.iter().rposition(Scope::is_mask).map_or(0, |index| index + 1);",
        "        let base = 0;")]),
    resources("releasing does not mask the entry", SCOPE, [(
        "        self.tasks.get_mut(&task).expect(\"live task\").scopes[index].kind =\n"
        "            ScopeKind::Resource { masked: true };",
        "        let _ = index;")]),
    resources("leaving a resource never reports the held-off cancellation", SCOPE, [(
        "            record.scopes[index].delivered = true;\n            return Some(true);",
        "            let _ = index;\n            return Some(false);")]),
    # Task cancellation waits for releases.
    resources("cancelling a task that holds a resource tears it down at once", SCOPE, [(
        "        if self.forcing > 0 || no_fiber || !holds_resource || stopping {",
        "        if true {")]),
    resources("the root scope also cancels children started outside every scope", SCOPE, [(
        "                    None => false,",
        "                    None => true,")]),
    resources("a recorded cancellation never decides the outcome", SCHEDULER, [(
        "            && (recorded.is_defect() || !outcome.is_defect())",
        "            && false")]),
    # CHI-215: what a cancelled scope gained is aborted, and nothing else.
    resources("a cancelled scope exit keeps what the scope gained", SCHEDULER, [(
        "                request.value != 0,",
        "                false,")]),
    resources("a cancelled scope aborts capabilities held before it", CHANNEL, [(
        "            let receiver = channel.receiver_owner == Some(task) && channel.receiver_since > since;",
        "            let receiver = channel.receiver_owner == Some(task);")]),
    resources("a child's hand-back is not backdated to its start", SCHEDULER, [(
        "        if record.parent == Some(new_owner) {",
        "        if false && record.parent == Some(new_owner) {")]),
    # The native boundary.
    resources("resource_scoped ignores the cancellation its exit delivers", SCOPES, [(
        "            if cancel || exit.value == 1 {\n                return cancelled_unless_defect(outcome);",
        "            if cancel {\n                return cancelled_unless_defect(outcome);")]),
    resources("a cancelled use skips its release", SCOPES, [(
        "        if (*used).kind == ControlKind::CANCEL && cancel_is_terminal() {\n            return used;",
        "        if (*used).kind == ControlKind::CANCEL {\n            return used;")]),
    # Outcome order, on both backends.
    outcomes("native: a release failure does not replace the use's outcome", SCOPES, [(
        "        if matches!((*released).kind, ControlKind::FAILURE | ControlKind::CANCEL) {",
        "        if matches!((*released).kind, ControlKind::CANCEL) {")]),
    outcomes("native: a release defect outranks a use defect", SCOPES, [(
        "        if (*used).kind == ControlKind::DEFECT {\n            return used;\n        }\n        if (*released).kind == ControlKind::DEFECT {\n            return released;\n        }",
        "        if (*released).kind == ControlKind::DEFECT {\n            return released;\n        }\n        if (*used).kind == ControlKind::DEFECT {\n            return used;\n        }")]),
    outcomes("native: a release defect does not replace a use failure", SCOPES, [(
        "        if (*released).kind == ControlKind::DEFECT {\n            return released;\n        }\n        if (*used).kind == ControlKind::CANCEL {",
        "        if (*used).kind == ControlKind::CANCEL {")]),
    outcomes("interpreter: a release failure does not replace the use's outcome", INTERPRETER, [(
        "        (_, released @ (Control::Failure(..) | Control::Cancellation(_))) => released,",
        "        (_, released @ Control::Cancellation(_)) => released,")]),
    outcomes("interpreter: a release defect outranks a use defect", INTERPRETER, [(
        "        (used @ Control::Defect { .. }, _) => used,\n        (_, released @ Control::Defect { .. }) => released,",
        "        (_, released @ Control::Defect { .. }) => released,\n        (used @ Control::Defect { .. }, _) => used,")]),
    # Processes and Wait (review findings).
    resources("a containment cancel does not wake a selecting release", SCHEDULER, [(
        "        if outcome.is_defect()\n            && let Some(parent) = parent\n        {\n            self.wake_selecting_parent(parent, task);",
        "        if false\n            && let Some(parent) = parent\n        {\n            self.wake_selecting_parent(parent, task);")]),
    resources("the deadline forces only direct children", SCHEDULER, [(
        "                Some(TaskPhase::Draining(_)) => self.force_live_descendants(child),",
        "                Some(TaskPhase::Draining(_)) => {}")]),
    resources("a release's receive is not woken by the stop", PROCESS, [(
        "        let releasing = !forced && !receiving && self.holds_mask(task);",
        "        let releasing = !forced && self.holds_mask(task);")]),
    resources("parks inside a release linger under shutdown", PROCESS, [(
        "            && self.stopping(task).is_some_and(|stopping| stopping.told)\n            && !self.holds_mask(task)",
        "            && self.stopping(task).is_some_and(|stopping| stopping.told)")]),
    resources("shutdown is delivered into a running release", PROCESS, [(
        "        let releasing = !forced && !receiving && self.holds_mask(task);",
        "        let releasing = false;")]),
    resources("a deferred shutdown is never told after the release", SCOPE, [(
        "            return Some(!self.holds_mask(task) && self.tell_deferred_shutdown(task));",
        "            return Some(false);")]),
    resources("the deadline leaves children of a finished body running", PROCESS, [(
        "        if matches!(self.tasks[&task].phase, TaskPhase::Draining(_)) && !self.has_live_descendants(task) {",
        "        if matches!(self.tasks[&task].phase, TaskPhase::Draining(_)) {")]),
    resources("a process body is torn down at once on a child defect", SCOPE, [(
        "        if self.forcing > 0 || no_fiber || !holds_resource || stopping {",
        "        if self.forcing > 0 || no_fiber || !holds_resource || process {")]),
    resources("shutdown cancels the helpers a release started", PROCESS, [(
        "            .filter(|child| !self.kept_for_release(task, *child))",
        "            .filter(|_| true)")]),
]
