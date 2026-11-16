"""Mutants of std/exec part 4 (CHI-250): stopping a child still running when
its `Exec.start` ends (SIGTERM, `stop_within`, then SIGKILL), and the exit
backstop.

Not covered by a test: the kqueue ticker (macOS; CI runs the same tests
there); a ticker that cannot be opened (the stop then sends SIGKILL only);
a negative `stop_within` reaching native code (`Exec.stop_within` rejects it
first); and a SIGTERM sent before an immediate SIGKILL when `stop_within` is
zero, which no test can see, since the SIGKILL lands before any handler;
and the SIGKILL phase's parked wait (without it the release's blocking reap
waits the moment a killed child takes to go).

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
        ("    kill(child->pid, SIGTERM);\n    ended = await_exit_until", "    ended = await_exit_until")]),
    exec_test("no SIGKILL after the time", EXEC, [
        ("    kill(child->pid, SIGKILL);\n    await_exit(call);", "    await_exit(call);")]),
    exec_test("the deadline is ignored", EXEC, [
        ("    if (monotonic_ms() >= deadline) return false;\n", "")]),
    exec_test("the stop blocks the scheduler", EXEC, [
        ("    if (chimi_binding_wait(call, CHIMI_BINDING_NO_HANDLE, ticker, CHIMI_BINDING_READABLE) != CHIMI_BINDING_READY)\n      return false;",
         "    usleep(STOP_TICK_MS * 1000);")]),
    exec_test("the stop waits again after a cancel", EXEC, [
        ("    if (waited == STOP_CANCELLED) return chimi_binding_cancelled(call);\n", "")]),
    exec_test("the start does not stop its child", MODULE, [
        ("      native_stop(process, Duration.to_milliseconds(command.stop_within))\n", "")]),
    exec_test("the pipes close after the stop", MODULE, [
        ("      close_pipe(input_pipe)\n      close_pipe(output_pipe)\n      close_pipe(errors_pipe)\n      native_stop(process, Duration.to_milliseconds(command.stop_within))\n      native_release(process)\n",
         "      native_stop(process, Duration.to_milliseconds(command.stop_within))\n      native_release(process)\n      close_pipe(input_pipe)\n      close_pipe(output_pipe)\n      close_pipe(errors_pipe)\n")]),
    exec_test("stop_within is not kept", MODULE, [
        ("    command.stderr,\n    duration,\n  )", "    command.stderr,\n    command.stop_within,\n  )")]),
    exec_test("a negative stop_within is kept", MODULE, [
        ("  case duration < Duration.zero {", "  case duration < Duration.milliseconds(-1000000) {")]),
    exec_test("the backstop leaves the child", EXEC, [
        ("  if (!child->reaped) {\n    kill(child->pid, SIGKILL);\n", "  if (!child->reaped) {\n")]),
]
