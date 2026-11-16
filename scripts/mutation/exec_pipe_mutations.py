"""Mutants of std/exec part 3 (CHI-250): pipes to a child's standard
streams, `Exec.capture`, and fakes with `Exec.piped`.

Not covered by a test: the macOS F_SETNOSIGPIPE branch (CI's Linux runs take
the blocked-signal branch; macOS runs the same tests on the other one), and a
pipe handle that cannot be made (needs 2^20 open handles).

Run: scripts/mutation/mutate.py scripts/mutation/exec_pipe_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

EXEC = "runtime/native/bindings/exec/exec.c"
MODULE = "std/exec.chimi"


def exec_test(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_exec"])


MUTATIONS = [
    exec_test("pipe ends reach other children", EXEC, [
        ("  int moved = fcntl(fd, F_DUPFD_CLOEXEC, STDERR_FILENO + 1);", "  int moved = fcntl(fd, F_DUPFD, STDERR_FILENO + 1);")]),
    exec_test("the parent keeps the child's ends", EXEC, [
        ("    if (ends[fd][1] >= 0) close(ends[fd][1]);\n", "")]),
    exec_test("the parent's end blocks", EXEC, [
        ("fcntl(ends[0], F_SETFL, flags | O_NONBLOCK) == 0", "fcntl(ends[0], F_SETFL, flags) == 0")]),
    exec_test("a write to a gone reader raises SIGPIPE", EXEC, [
        ("  pthread_sigmask(SIG_BLOCK, &pipe_signal, &previous);\n", "  pthread_sigmask(SIG_BLOCK, NULL, &previous);\n")]),
    exec_test("a broken pipe is Failed", EXEC, [
        ("return chimi_binding_failure(call, EXEC_BROKEN_PIPE, strerror(EPIPE));", "return chimi_binding_failure(call, EXEC_FAILED, strerror(EPIPE));")]),
    exec_test("the end of output is a failure", EXEC, [
        ("    if (count == 0) {\n      result = chimi_binding_ok(call, chimi_binding_none(call));",
         "    if (count == 0) {\n      result = chimi_binding_failure(call, EXEC_FAILED, \"eof\");")]),
    exec_test("stdin and stdout pipes swap ends", EXEC, [
        ("  ends[0] = target == STDIN_FILENO ? write_end : read_end;\n  ends[1] = target == STDIN_FILENO ? read_end : write_end;",
         "  ends[0] = read_end;\n  ends[1] = write_end;")]),
    exec_test("a fake read after a stop is Failed", MODULE, [
        ("    StreamError.Aborted -> ExecError.Closed(program)\n", "")]),
    exec_test("release leaves the pipes open", MODULE, [
        ("      close_pipe(input_pipe)\n      close_pipe(output_pipe)\n      close_pipe(errors_pipe)\n", "")]),
    exec_test("capture inherits stdin", MODULE, [
        ("stdin(command, Stdio.Null)", "command")]),
    exec_test("capture swaps its streams", MODULE, [
        ("output: Task.result(written), error_output: Task.result(failed)", "output: Task.result(failed), error_output: Task.result(written)")]),
    exec_test("capture reads output to its end first", MODULE, [
        ("    let written = read_in_task(output(child))\n", "    let written_now = everything(output(child))\n    let written = Task.start(fn() { written_now })\n")]),
    exec_test("a fake input's Closed reads as Closed", MODULE, [
        ("Failure(fake_failure(error, program, ExecError.BrokenPipe(program)))", "Failure(fake_failure(error, program, ExecError.Closed(program)))")]),
    exec_test("a fake's error output is its output", MODULE, [
        ("    Some(fake_output(error_output)),", "    Some(fake_output(output)),")]),
]
