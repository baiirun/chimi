"""CHI-266: console writes wait in the IO reactor instead of blocking every task."""

CRATE = "compiler/chimi"

RUNTIME = "runtime/native/runtime.c"

MUTATIONS = [
    dict(
        name="every stream writes directly, blocking the scheduler",
        file=RUNTIME,
        edits=[("    ? CONSOLE_QUEUED : CONSOLE_DIRECT;", "    ? CONSOLE_DIRECT : CONSOLE_DIRECT;")],
        targets=["console_output"],
    ),
    dict(
        name="a write after poll takes the whole queue, not PIPE_BUF",
        file=RUNTIME,
        edits=[("!console_write_front(stream, PIPE_BUF)", "!console_write_front(stream, SIZE_MAX)")],
        targets=["console_output"],
    ),
    dict(
        name="a writer returns once its line is queued",
        file=RUNTIME,
        edits=[("    if (queue->resolved >= line.end) break;\n", "    break;\n")],
        targets=["console_output"],
    ),
    dict(
        name="the end of the program drops lines still queued",
        file=RUNTIME,
        edits=[
            ("static void console_drain_all(void) {\n", "static void console_drain_all(void) {\n  return;\n"),
        ],
        targets=["console_output"],
    ),
    dict(
        name="a writer waits for readable instead of writable",
        file=RUNTIME,
        edits=[(".value = stream->fd, .token = CHIMI_IO_WRITABLE}", ".value = stream->fd, .token = CHIMI_IO_READABLE}")],
        targets=["console_output"],
    ),
    dict(
        name="standard error keeps its own queue when it names standard output's file",
        file=RUNTIME,
        edits=[("      stream->queue = output->queue;\n", "      (void)output;\n")],
        targets=["console_output"],
    ),
    dict(
        name="a failure also fails lines already written",
        file=RUNTIME,
        edits=[("    if (line->end > queue->resolved) line->error = error;", "    line->error = error;")],
        targets=["console_output"],
    ),
    dict(
        name="the queue is not listed as shared runtime state",
        file="tests/runtime_state.rs",
        edits=[('    "console_queues",\n', "")],
        targets=["runtime_state"],
    ),
]
