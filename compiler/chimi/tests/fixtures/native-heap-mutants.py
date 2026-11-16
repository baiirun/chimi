"""Applies each per-owner heap mutant to runtime.c on its own, runs the tests that
guard the decision it breaks, and restores the file. A mutant is killed when its
test command fails or times out (300 s for a C harness).

usage, from the repository root:
  python3 compiler/chimi/tests/fixtures/native-heap-mutants.py . [mutant-name ...]

Results are recorded in docs/architecture/supervised-process-runtime.md
("The heap as built in runtime.c")."""
import subprocess, sys, pathlib, tempfile

ROOT = pathlib.Path(sys.argv[1]).resolve()
RUNTIME = ROOT / "compiler/chimi/runtime/native/runtime.c"
SCRATCH = pathlib.Path(tempfile.mkdtemp(prefix="chimi-heap-mutants-"))
CC = ["cc", "-std=c11", "-I", str(ROOT / "compiler/chimi/runtime/native"),
      "-I", str(ROOT / "runtime/chimi-fiber/include")]
ASAN = ["-O1", "-g", "-fsanitize=address,undefined", "-fno-omit-frame-pointer"]
ENV = {"ASAN_OPTIONS": "detect_leaks=0", "UBSAN_OPTIONS": "halt_on_error=1", "PATH": "/usr/bin:/bin:/usr/local/bin:/opt/homebrew/bin"}

def harness(flags, fixture, args):
    exe = SCRATCH / "mut-bin"
    def run():
        build = subprocess.run(CC + flags + [str(ROOT / f"compiler/chimi/tests/fixtures/{fixture}"), "-o", str(exe)],
                               capture_output=True)
        if build.returncode: return False, "build failed"
        out = subprocess.run([str(exe)] + args, capture_output=True, timeout=300, env=ENV)
        return out.returncode == 0, (out.stderr.decode()[-300:] or out.stdout.decode()[-200:])
    return run

def cargo(*tests):
    def run():
        out = subprocess.run(["cargo", "test", "-q", "-p", "chimi", "--test", "cli", "--", *tests],
                             cwd=ROOT, capture_output=True, timeout=900)
        text = out.stdout.decode()
        failed = [l for l in text.splitlines() if "FAILED" in l or "panicked" in l]
        return out.returncode == 0, "\n".join(failed[:4])
    return run

HEAP = harness(ASAN, "native-heap.c", [])
HEAP_CACHED = harness(["-O2"], "native-heap.c", [])
FUZZ = harness(ASAN, "native-list-fuzz.c", ["88172645463325252", "40000"])
RELEASE = cargo("native_cancelling_a_child", "native_dropping_a_channel", "native_trace_reports_a_released",
                "native_children_keep", "native_a_receiver_that", "native_cancelled_sends",
                "native_aborting_a_channel", "native_concurrency_pipeline")
E2E = cargo("native_concurrency_pipeline_runs", "native_task_result_can_be_read",
            "native_a_task_keeps", "native_a_queued_message", "native_child_defect", "native_channel", "native_task")

MUTANTS = [
    ("minor-pass-rescans-survivors", "pass cost independent of old survivors",
     "  if (area->role == AREA_SURVIVOR && !carrier->collecting_full) return;\n", "", HEAP),
    ("full-pass-every-pass", "full-pass trigger (baseline + minimum growth)",
     "  bool full = scope->has_baseline && survivors >= 2 * scope->full_baseline &&\n              survivors - scope->full_baseline >= REGION_FULL_MIN_GROWTH;",
     "  bool full = scope->has_baseline; (void)survivors;", HEAP),
    ("full-pass-without-minimum-growth", "full-pass trigger (baseline + minimum growth)",
     "  bool full = scope->has_baseline && survivors >= 2 * scope->full_baseline &&\n              survivors - scope->full_baseline >= REGION_FULL_MIN_GROWTH;",
     "  bool full = scope->has_baseline && survivors >= 2 * scope->full_baseline;", HEAP),
    ("never-full-pass", "memory depends on live state, not uptime",
     "  bool full = scope->has_baseline && survivors >= 2 * scope->full_baseline &&\n              survivors - scope->full_baseline >= REGION_FULL_MIN_GROWTH;",
     "  bool full = false; (void)survivors;", HEAP),
    ("adopt-into-base", "adopted messages join the innermost region",
     "  if (scope) {\n    area->role = AREA_YOUNG;", "  if (0) {\n    area->role = AREA_YOUNG;", HEAP),
    ("region-exit-drops-survivors", "region exit hands survivors to the enclosing region",
     "  area_merge(parent ? &parent->young : &heap->base, &scope->survivors);",
     "  area_release_all(&scope->survivors); scope->live_bytes = 0;", HEAP),
    ("pass-ignores-foreign-pointers", "a pointer into another heap is reported",
     "    if (area->heap != carrier->collecting_scope->heap && area->role != AREA_STATIC) carrier->collecting_foreign = true;\n",
     "", HEAP),
    ("copy-ignores-foreign-pointers", "a pointer into another heap is reported",
     "  if (!copy_owns(copy, header->area)) {\n    copy->status = HEAP_FOREIGN_POINTER;",
     "  if (0) {\n    copy->status = HEAP_FOREIGN_POINTER;", HEAP),
    ("copy-without-forwarding-table", "a copy preserves sharing",
     "  void *existing = copy_table_find(copy, pointer);", "  void *existing = NULL;", HEAP),
    ("copy-whole-vector-of-a-window", "a window copy rebuilds only the window",
     "  uint32_t length = list->as.list.length;\n  ChimiValue **values = malloc(((size_t)length + 1) * sizeof(*values));\n  if (!values) exhausted_memory();\n  for (uint32_t index = 0; index < length; ++index)\n    values[index] = copy_forward(copy, list_get(list, index), ALLOCATION_VALUE);",
     "  uint32_t length = list->as.list.length;\n  uint32_t whole = source.length, start = list->as.list.start;\n  ChimiValue **values = malloc(((size_t)whole + 1) * sizeof(*values));\n  if (!values) exhausted_memory();\n  list->as.list.start = 0; list->as.list.length = whole;\n  for (uint32_t index = 0; index < whole; ++index)\n    values[index] = copy_forward(copy, list_get(list, index), ALLOCATION_VALUE);\n  list->as.list.length = length; length = whole; (void)start;",
     HEAP),
    ("fragment-record-reserves-64k", "small messages cost their size (ts-df67fb)",
     "enum { FRAGMENT_ROOTS = 2 };", "enum { FRAGMENT_ROOTS = 8192 };", HEAP),
    ("heap-destroy-skips-adopted", "teardown frees every allocation",
     "  while (heap->adopted) {\n    AllocationArea *area = heap->adopted;\n    heap->adopted = area->next;\n    area_release_all(area);\n    area_free(area);\n  }",
     "  heap->adopted = NULL;", HEAP),
    ("compaction-into-current-area", "list compaction allocates in the list's own area",
     "      list_copy_window(list, 0, list->as.list.length, allocation_header(list)->area);",
     "      list_copy_window(list, 0, list->as.list.length, NULL);", FUZZ),
    ("task-result-consumes", "Task.result copies without consuming",
     "  fragment_copy_into_current((RuntimeFragment *)(intptr_t)(raw & ~(int64_t)CHILD_RESULT_TAG), roots);",
     "  fragment_adopt((RuntimeFragment *)(intptr_t)(raw & ~(int64_t)CHILD_RESULT_TAG), roots);", E2E),
    ("task-start-shares-captures", "a child runs on a copy of its captures",
     "      CapabilityTransfer transfer = {.child = request.task};\n      bool transferred = transfer_captured_value(&transfer, captures->roots[0], 0);",
     "      captures->roots[0] = arguments[0];\n      CapabilityTransfer transfer = {.child = request.task};\n      bool transferred = transfer_captured_value(&transfer, captures->roots[0], 0);", E2E),
    ("send-shares-the-original", "a message crosses as a copy",
     "      for (;;) {\n        ChimiSchedulerRequest request = {\n          .kind = CHIMI_REQUEST_CHANNEL_SEND,",
     "      message->roots[0] = arguments[1];\n      for (;;) {\n        ChimiSchedulerRequest request = {\n          .kind = CHIMI_REQUEST_CHANNEL_SEND,", E2E),
    ("no-heap-switch-on-resume", "the current heap follows the resumed fiber",
     "  if (command != CHIMI_FIBER_CANCEL) command = chimi_fiber_request(request, NULL);\n  carrier->fiber = fiber;",
     "  if (command != CHIMI_FIBER_CANCEL) command = chimi_fiber_request(request, NULL);\n  (void)fiber;", E2E),
    ("no-reclamation-under-scheduler", "loops reclaim under the scheduler",
     "      if (allocation_scope_collect(&passes, state, NULL) != HEAP_OK) {",
     "      if (!CHIMI_REQUIRES_SCHEDULER && allocation_scope_collect(&passes, state, NULL) != HEAP_OK) {", E2E),
    ("release-at-one-reference", "a channel is released at its last endpoint reference",
     "  if (--channel_endpoints[slot].endpoints != 0) return;",
     "  if (--channel_endpoints[slot].endpoints > 1) return;", RELEASE),
    ("endpoint-copy-not-counted", "an endpoint copy in transit is a reference",
     "    channel_endpoint_retain(((ChimiValue *)duplicate)->as.capability.channel);\n", "    (void)0;\n", RELEASE),
    ("never-queue-release", "dropped channels free their queued messages",
     "  released_channels[released_channel_count++] = channel;\n", "\n", RELEASE),
    ("endpoint-free-not-counted", "dropped channels free their queued messages",
     "    if (is_channel_endpoint(value)) channel_endpoint_drop(value->as.capability.channel);\n",
     "    (void)value;\n", RELEASE),
    ("no-task-context-hook", "an unrun child's captures are freed",
     "    .release_task_context = release_task_context,\n", "", RELEASE),
    ("small-block-not-zeroed", "reused blocks are zeroed like calloc",
     "      memset(block, 0, rounded);\n", "", HEAP_CACHED),
]

def main():
    selected = set(sys.argv[2:])
    original = RUNTIME.read_text()
    results = []
    try:
        for name, decision, old, new, test in MUTANTS:
            if selected and name not in selected: continue
            if original.count(old) != 1:
                results.append((name, decision, "NOT APPLIED", "pattern count %d" % original.count(old)))
                continue
            RUNTIME.write_text(original.replace(old, new))
            try:
                passed, detail = test()
            except subprocess.TimeoutExpired:
                passed, detail = False, "timeout"
            verdict = "SURVIVED" if passed else "killed"
            results.append((name, decision, verdict, detail.strip().splitlines()[-1] if detail.strip() else ""))
            print(f"{name}: {verdict} {results[-1][3]}", flush=True)
    finally:
        RUNTIME.write_text(original)
    for row in results: print(" | ".join(row))

main()
