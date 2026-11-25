"""Mutants of R4 step 3 PR 5a-1: the C runtime's shared state under several
carrier threads.

Run: scripts/mutation/mutate.py scripts/mutation/carrier_threads_mutations.py
Each mutant must turn the_runtime_state_holds_across_carrier_threads in
runtime_state.rs red: a lock or atomic removed fails under ThreadSanitizer, the
rest fail one of the fixture's checks.
Results: scripts/mutation/carrier_threads_results.txt.
"""

CRATE = "compiler/chimi"

R = "runtime/native/runtime.c"


def m(name, edits):
    return dict(name=name, file=R, edits=edits, targets=["runtime_state"],
                filter="the_runtime_state_holds_across_carrier_threads")


MUTATIONS = [
    m("the random stream has no lock", [
        ("static uint64_t random_next(void) {\n  pthread_mutex_lock(&random_lock);\n",
         "static uint64_t random_next(void) {\n"),
        ("  state[3] = rotate_left(state[3], 45);\n  pthread_mutex_unlock(&random_lock);\n",
         "  state[3] = rotate_left(state[3], 45);\n")]),
    m("the test logs have no lock", [(
        "  pthread_mutex_lock(&test_log_lock);\n  ChimiControl *control = invoke_test_log_locked(",
        "  ChimiControl *control = invoke_test_log_locked("), (
        "invoke_test_log_locked(intrinsic, arguments, argument_count, call_node);\n"
        "  pthread_mutex_unlock(&test_log_lock);",
        "invoke_test_log_locked(intrinsic, arguments, argument_count, call_node);")]),
    m("an event is written without the trace lock", [(
        "  FILE *output = runtime->telemetry;\n  pthread_mutex_lock(&trace_lock);\n"
        "  ++runtime->trace_events_constructed;\n  fputs(\"{\\\"schema\\\":\\\"chimi.runtime.v1\\\",\\\"event\\\":\", output); json_string(output, event);",
        "  FILE *output = runtime->telemetry;\n"
        "  ++runtime->trace_events_constructed;\n  fputs(\"{\\\"schema\\\":\\\"chimi.runtime.v1\\\",\\\"event\\\":\", output); json_string(output, event);"), (
        "  if (outcome) { fputs(\"\\\"outcome\\\":\", output); json_string(output, control_name(outcome)); }\n"
        "  fputs(\"}}\\n\", output);\n  pthread_mutex_unlock(&trace_lock);",
        "  if (outcome) { fputs(\"\\\"outcome\\\":\", output); json_string(output, control_name(outcome)); }\n"
        "  fputs(\"}}\\n\", output);")]),
    m("a scheduler event is written without the trace lock", [(
        "  FILE *output = runtime->telemetry;\n  pthread_mutex_lock(&trace_lock);\n"
        "  ++runtime->trace_events_constructed;\n  fputs(\"{\\\"schema\\\":\\\"chimi.runtime.v1\\\",\\\"event\\\":\", output);\n"
        "  json_string(output, scheduler_event_name(event));",
        "  FILE *output = runtime->telemetry;\n"
        "  ++runtime->trace_events_constructed;\n  fputs(\"{\\\"schema\\\":\\\"chimi.runtime.v1\\\",\\\"event\\\":\", output);\n"
        "  json_string(output, scheduler_event_name(event));")]),
    m("span ids come from a plain counter", [(
        "static uint64_t begin_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent, const char *operation) {\n"
        "  if (runtime->silent) return parent;\n"
        "  uint64_t span = atomic_fetch_add_explicit(&runtime->next_span, 1, memory_order_relaxed);",
        "static uint64_t begin_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent, const char *operation) {\n"
        "  if (runtime->silent) return parent;\n"
        "  uint64_t span = (*(uint64_t *)&runtime->next_span)++;")]),
    m("profile counts are plain adds", [(
        "static void profile_count(uint64_t *field, uint64_t by) { __atomic_fetch_add(field, by, __ATOMIC_RELAXED); }",
        "static void profile_count(uint64_t *field, uint64_t by) { *field += by; }")]),
    m("heap owners come from a plain counter", [(
        "  heap->owner = atomic_fetch_add_explicit(&next_heap_owner, 1, memory_order_relaxed);",
        "  heap->owner = (*(uint64_t *)&next_heap_owner)++;")]),
    m("the sum skips carrier 0", [(
        "  HeapTotals sum = {0};\n  for (size_t index = 0; index < carrier_count; ++index) {",
        "  HeapTotals sum = {0};\n  for (size_t index = 1; index < carrier_count; ++index) {")]),
    m("a carrier thread does not bind its carrier", [(
        "static void carrier_thread_enter(size_t index) {\n  carrier_bind(carrier_at(index));",
        "static void carrier_thread_enter(size_t index) {\n  (void)index;")]),
    m("a task on an unbound thread runs", [(
        "  if (carrier_count > 1 && !(carrier->bound && pthread_equal(carrier->thread, pthread_self()))) {",
        "  if (false) {")]),
    m("carriers_create leaves the count at one", [(
        "  for (size_t index = 0; index + 1 < count; ++index) carrier_others[index].fiber.heap = &main_heap;\n"
        "  carrier_count = count;",
        "  for (size_t index = 0; index + 1 < count; ++index) carrier_others[index].fiber.heap = &main_heap;")]),
]
