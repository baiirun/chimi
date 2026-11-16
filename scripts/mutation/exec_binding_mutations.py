"""Mutants of std/exec part 2 (CHI-250): the exec binding package, the
runtime's child-wait API, and the Chimi wrapper.

Not listed, as equivalent: the NUL checks on program, argument and variable text (the defect message is
the only difference a test can see, and Chimi text rarely holds U+0000), and
the non-scheduler branch of `chimi_binding_wait_child` (a binding call always
forces the scheduler).

Not covered by a test: a directory without search permission (`access` in
the spawn's directory check; tests run as root in CI, where it always
passes), a child stopped when no handle can be made for it (needs 2^20
open handles), and the report files closed in children (FD_CLOEXEC in
`open_report`; a child holding one open changes nothing a test can see
before the runner exits).

Run: scripts/mutation/mutate.py scripts/mutation/exec_binding_mutations.py
"""

CRATE = "compiler/chimi"
CARGO_TEST_ARGS = ["--release"]

EXEC = "runtime/native/bindings/exec/exec.c"
RUNTIME = "runtime/native/runtime.c"
MODULE = "std/exec.chimi"


def exec_test(name, file, edits):
    return dict(name=name, file=file, edits=edits, targets=["std_exec"])


MUTATIONS = [
    exec_test("a released child is not killed", EXEC, [
        ("  if (!child->reaped) {\n    kill(child->pid, SIGKILL);\n", "  if (!child->reaped) {\n")]),
    exec_test("a released child is not reaped", EXEC, [
        ("    while (waitpid(child->pid, &status, 0) < 0 && errno == EINTR) {\n    }\n", "    (void)status;\n")]),
    exec_test("a second wait asks the kernel again", EXEC, [
        ("    if (child->reaped) return status_result(call, child->status);\n", "")]),
    exec_test("a signal death reads as an exit", EXEC, [
        ("  if (WIFSIGNALED(status)) {", "  if (0) {")]),
    exec_test("a name is not looked up on PATH", EXEC, [
        ("  if (strchr(program, '/') != NULL) {", "  if (1) {")]),
    exec_test("a directory found on PATH counts as the program", EXEC, [
        ("  if (S_ISDIR(info.st_mode) || access(path, X_OK) != 0) {", "  if (access(path, X_OK) != 0) {")]),
    exec_test("a replaced environment keeps the parent's", EXEC, [
        ("  if (!builder->clear_environment)\n", "  if (1)\n")]),
    exec_test("the first value of a variable wins", EXEC, [
        ("    if (!later) entries[count++] = builder->variables.items[variable];",
         "    if (!later || later) entries[count++] = builder->variables.items[variable];")]),
    exec_test("the working directory is ignored", EXEC, [
        ("    if (error == 0 && builder->directory != NULL)\n      error = posix_spawn_file_actions_addchdir_np(&actions, builder->directory);\n", "")]),
    exec_test("a bad directory is found only by the child", EXEC, [
        ("    if (stat(builder->directory, &info) != 0 || !S_ISDIR(info.st_mode) || access(builder->directory, X_OK) != 0) {", "    if (0) {")]),
    exec_test("Stdio.Null is inherited", EXEC, [
        ("  if (mode == STDIO_NULL)\n    return posix_spawn_file_actions_addopen", "  if (mode == STDIO_NULL)\n    return true || posix_spawn_file_actions_addopen")]),
    exec_test("a relative program stays relative", EXEC, [
        ("  if (path == NULL || path[0] == '/') return path;", "  return path;")]),
    exec_test("a file the kernel cannot run is a permission", EXEC, [
        ("error == EACCES ? EXEC_PERMISSION_DENIED : EXEC_FAILED;", "error == EACCES || error == ENOEXEC ? EXEC_PERMISSION_DENIED : EXEC_FAILED;")]),
    exec_test("terminate sends SIGKILL", EXEC, [
        ("  if (child != NULL && !child->reaped) kill(child->pid, SIGTERM);", "  if (child != NULL && !child->reaped) kill(child->pid, SIGKILL);")]),
    exec_test("a cancelled wait is not cancelled", EXEC, [
        ("      case CHIMI_BINDING_WAIT_CANCELLED: return chimi_binding_cancelled(call);\n", "")]),
    exec_test("a child wait does not wait", RUNTIME, [
        ("    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_CHILD_WAIT, .value = pid};\n    if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return CHIMI_BINDING_WAIT_CANCELLED;\n",
         "    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_CHILD_WAIT, .value = pid, .status = CHIMI_RESPONSE_OK};\n")]),
    exec_test("the child's signal mask is inherited", EXEC, [
        ("    if (error == 0) error = posix_spawnattr_setflags(&attributes, POSIX_SPAWN_SETSIGMASK);", "")]),
    exec_test("Stdio.Null stdin is opened for writing", EXEC, [
        ("fd == STDIN_FILENO ? O_RDONLY : O_WRONLY", "O_WRONLY")]),
    exec_test("stdin is never connected", MODULE, [
        ("    stdio,\n    command.stdout,\n    command.stderr,\n  )\n}", "    command.stdin,\n    command.stdout,\n    command.stderr,\n  )\n}")]),
    exec_test("a second wait after a cancelled one asks the dead pid", EXEC, [
        ("      child->reaped = true;\n", "")]),
    exec_test("no SIGCHLD handler", RUNTIME, [
        ("  action.sa_handler = on_child_signal;\n  sigemptyset(&action.sa_mask);\n  action.sa_flags = SA_RESTART | SA_NOCLDSTOP;\n  sigaction(SIGCHLD, &action, NULL);\n", "")]),
    exec_test("start does not release the child", MODULE, [
        ("fn(child) { child.release() }", "fn(_child) { Void }")]),
    exec_test("stdout and stderr swap", MODULE, [
        ("stdio_code(command.stdout), stdio_code(command.stderr)", "stdio_code(command.stderr), stdio_code(command.stdout)")]),
]
