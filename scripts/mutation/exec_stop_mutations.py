"""Mutants of std/exec part 4 (CHI-250): stopping a child still running when
its `Exec.start` ends (SIGTERM, `stop_within`, then SIGKILL), and the exit
backstop.

Not covered by a test: the kqueue ticker (macOS; CI runs the same tests
there), a ticker that cannot be opened (the stop then falls back to the
release's SIGKILL and blocking reap), and a negative `stop_within` from
native code (`Duration` is checked on the Chimi side first).

Run: scripts/mutation/mutate.py scripts/mutation/exec_stop_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

EXEC = "runtime/native/bindings/exec/exec.c"
MODULE = "std/exec.chimi"


def exec_test(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_exec"])


MUTATIONS = [
    exec_test("no SIGTERM first", EXEC, [
        ("    kill(child->pid, SIGTERM);\n    ended = await_exit", "    ended = await_exit")]),
    exec_test("no SIGKILL after the time", EXEC, [
        ("    kill(child->pid, SIGKILL);\n    (void)await_exit", "    (void)await_exit")]),
    exec_test("the deadline is ignored", EXEC, [
        ("    if (ticker < 0 || monotonic_ms() >= deadline) return false;", "    if (ticker < 0) return false;")]),
    exec_test("the stop blocks the scheduler", EXEC, [
        ("    if (chimi_binding_wait(call, CHIMI_BINDING_NO_HANDLE, ticker, CHIMI_BINDING_READABLE) != CHIMI_BINDING_READY)\n      return false;",
         "    usleep(STOP_TICK_MS * 1000);")]),
    exec_test("zero still waits", EXEC, [
        ("  if (grace > 0) {\n    kill(child->pid, SIGTERM);", "  if (1) {\n    kill(child->pid, SIGTERM);")]),
    exec_test("the start does not stop its child", MODULE, [
        ("      native_stop(process, Duration.to_milliseconds(command.stop_within))\n", "")]),
    exec_test("stop_within is not kept", MODULE, [
        ("    command.stderr,\n    duration,\n  )", "    command.stderr,\n    command.stop_within,\n  )")]),
    exec_test("the backstop leaves the child", EXEC, [
        ("  if (!child->reaped) {\n    kill(child->pid, SIGKILL);\n", "  if (!child->reaped) {\n")]),
]
