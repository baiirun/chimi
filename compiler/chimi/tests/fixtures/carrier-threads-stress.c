/* The C runtime's process-wide state under several carrier threads
   (parallel-scheduler.md, step 3 PR 5a). Four threads each enter their own
   carrier and heap and, at once:

   - copy values into fragments that another carrier adopts or frees, so a
     carrier's memory counters go below zero and only their sum is the
     process's;
   - take span ids, emit runtime and scheduler telemetry events, count the operation profile, draw
     from the `random` stream and record into one shared test Recorder;
   - read one shared file handle, which one of them closes half way;
   - open, use and close C binding handles in one shared table, take lines
     from standard input, and write console lines.

   Then the threads share two binding handles, alone and in pairs given in
   either order: one call at a time runs on a handle, and a call waiting on
   a descriptor lets its handle go to the others.

   The checks: every counter sums back to where it started, span ids and
   telemetry sequence numbers are unique and each event is written whole,
   the profile and the recorder saw every call, the random draws are the
   seeded stream's, and the closed handle reads as closed. Built with
   ThreadSanitizer when the compiler supports it, so an access outside the
   locks fails the run. Last, every thread slices one module constant,
   which must not be compacted under the others.

   Three single-threaded checks come first: a close during a read leaves the
   descriptor open until the read gives it back, a module constant compacts
   on one carrier as any list does, and a task started on a thread that
   never entered its carrier aborts, and carriers that fail at once exit
   once with the fatal code.

   usage: carrier-threads-stress     prints "ok" */
#include "runtime.c"

#include <sys/wait.h>

const char *CHIMI_COMPUTATION = "carrier-test";
const uint32_t CHIMI_REQUIRES_SCHEDULER = 0;
const char *CHIMI_STRINGS[] = {""};
const uint32_t CHIMI_STRING_LENGTHS[] = {0};
const uint32_t CHIMI_IDS[] = {0};
const ChimiOrigin CHIMI_ORIGINS[] = {{0}};
const ChimiNode CHIMI_NODES[] = {{0}};
const uint32_t CHIMI_NODE_COUNT = 0;
const ChimiSymbol CHIMI_SYMBOLS[] = {{0}};
const uint32_t CHIMI_SYMBOL_COUNT = 0;
const uint32_t CHIMI_CLOCK_REQUIREMENT_SLOT = 0;
const uint32_t CHIMI_REQUIREMENT_SLOT_COUNT = 1;
const ChimiFunction CHIMI_FUNCTIONS[] = {{0}};
const uint32_t CHIMI_FUNCTION_COUNT = 0;
const uint32_t CHIMI_TEST_FUNCTIONS[] = {0};
const uint32_t CHIMI_TEST_COUNT = 0;
const uint32_t CHIMI_ASSERTION_NODES[] = {0};
const uint32_t CHIMI_ASSERTION_NODE_COUNT = 0;
const ChimiConstructor CHIMI_CONSTRUCTORS[] = {{0}};
const ChimiNamedNode CHIMI_NAMED_NODES[] = {{0}};
const ChimiStep CHIMI_STEPS[] = {{0}};
const ChimiArm CHIMI_ARMS[] = {{0}};
const ChimiPattern CHIMI_PATTERNS[] = {{0}};
ChimiNodeFn CHIMI_NODE_FUNCTIONS[] = {NULL};
const ChimiConstantValue CHIMI_CONSTANT_VALUES[] = {{0}};
const uint32_t CHIMI_CONSTANT_VALUE_COUNT = 0;
/* The fixture's own C bindings, defined below: open, use and close a handle. */
static ChimiBindingResult *binding_open(ChimiBindingCall *call);
static ChimiBindingResult *binding_use(ChimiBindingCall *call);
static ChimiBindingResult *binding_close(ChimiBindingCall *call);
static ChimiBindingResult *binding_pair(ChimiBindingCall *call);
static ChimiBindingResult *binding_wait_on(ChimiBindingCall *call);
static ChimiBindingResult *binding_wait_child_on(ChimiBindingCall *call);
const ChimiBinding CHIMI_BINDINGS[] = {
  {"open", binding_open}, {"use", binding_use}, {"close", binding_close},
  {"pair", binding_pair}, {"wait_on", binding_wait_on}, {"wait_child_on", binding_wait_child_on}};
const uint32_t CHIMI_BINDING_COUNT = 6;
enum { BINDING_OPEN, BINDING_USE, BINDING_CLOSE, BINDING_PAIR, BINDING_WAIT_ON, BINDING_WAIT_CHILD_ON };

#define CHECK(condition, ...)                                                       \
  do {                                                                              \
    if (!(condition)) {                                                             \
      fprintf(stderr, "%s:%d: check failed: %s: ", __FILE__, __LINE__, #condition); \
      fprintf(stderr, __VA_ARGS__);                                                 \
      fputc('\n', stderr);                                                          \
      exit(1);                                                                      \
    }                                                                               \
  } while (0)

/* Threads 0 and 2 copy fragments that threads 1 and 3 take. */
/* Thread 0 closes the shared file at round CLOSE_AT, a read round. */
enum { THREADS = 4, ROUNDS = 2000, READ_EVERY = 32, CLOSE_AT = 1024, RING = 256 };
/* Handles in the shared table, and lines on standard input (each 17 bytes, so
   all of them fit in a pipe's buffer). */
enum { HANDLES = 64, INPUT_LINES = 3000 };

typedef struct {
  RuntimeFragment *slots[RING];
  _Atomic size_t head;
  _Atomic size_t tail;
  _Atomic bool done;
} Ring;

static Ring rings[THREADS / 2];
static ChimiRuntime runtime = {
  .computation = "carrier-test",
  .execution_id = "carrier-test",
  .silent = false,
  .next_span = 2,
};
static ChimiValue *recorder;
static int64_t shared_file;
static _Atomic int64_t closed_reads;
/* Per thread, then per round. */
static uint64_t spans[THREADS * ROUNDS];
/* Three mark epochs per round, so each thread takes more than one block. */
enum { EPOCHS_PER_ROUND = 3 };
static uint64_t epochs[THREADS * ROUNDS * EPOCHS_PER_ROUND];
static uint64_t draws[THREADS * ROUNDS];

/* A binding handle's state. A use counts itself in `users` for its length, so
   the release can check that no call is using the state it frees. `inside`
   counts the calls running on it, which must never be two, and `calls` is
   written without a lock, so ThreadSanitizer sees two calls that overlap. */
typedef struct {
  _Atomic int alive;
  _Atomic int users;
  _Atomic int inside;
  uint64_t calls;
} Resource;
static _Atomic int64_t resources_opened;
static _Atomic int64_t resources_released;

static void resource_release(void *raw) {
  Resource *resource = raw;
  CHECK(atomic_load(&resource->alive) == 1, "a resource was released twice");
  CHECK(atomic_load(&resource->users) == 0, "a resource was released while a call used it");
  atomic_store(&resource->alive, 0);
  atomic_fetch_add(&resources_released, 1);
}
static const ChimiBindingType resource_type = {"resource", resource_release};

static ChimiBindingResult *binding_open(ChimiBindingCall *call) {
  Resource *resource = calloc(1, sizeof(*resource));
  CHECK(resource, "calloc");
  atomic_store(&resource->alive, 1);
  atomic_fetch_add(&resources_opened, 1);
  return chimi_binding_success(call, chimi_binding_make_handle(call, &resource_type, resource));
}

/* Enters (`enter`) or leaves a call's run on `resource`. */
static void resource_run(Resource *resource, bool enter) {
  if (enter) {
    CHECK(atomic_fetch_add(&resource->inside, 1) == 0, "two calls ran on one handle at once");
    ++resource->calls;
  } else {
    atomic_fetch_sub(&resource->inside, 1);
  }
}

static ChimiBindingResult *binding_use(ChimiBindingCall *call) {
  Resource *resource = chimi_binding_handle(call, 0, &resource_type);
  if (resource) {
    resource_run(resource, true);
    atomic_fetch_add(&resource->users, 1);
    for (int spin = 0; spin < 200; ++spin)
      CHECK(atomic_load(&resource->alive) == 1, "a call's resource was released under it");
    atomic_fetch_sub(&resource->users, 1);
    resource_run(resource, false);
  }
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* Runs on two handles at once. */
static ChimiBindingResult *binding_pair(ChimiBindingCall *call) {
  Resource *first = chimi_binding_handle(call, 0, &resource_type);
  Resource *second = chimi_binding_handle(call, 1, &resource_type);
  CHECK(first && second && first != second, "a pair needs two open handles");
  resource_run(first, true);
  resource_run(second, true);
  for (volatile int spin = 0; spin < 200; ++spin) {}
  resource_run(second, false);
  resource_run(first, false);
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* The pipe `wait_on` waits on, the child `wait_child_on` waits for, and
   whether a call is in its wait. */
static int wait_pipe[2];
static pid_t wait_child;
static _Atomic bool waiting;

/* Runs on its handle, waits until the pipe is readable (`child` false) or
   the child has exited, and runs on the handle again. */
static ChimiBindingResult *wait_between_runs(ChimiBindingCall *call, bool child) {
  Resource *resource = chimi_binding_handle(call, 0, &resource_type);
  CHECK(resource, "a wait needs an open handle");
  resource_run(resource, true);
  resource_run(resource, false);
  atomic_store(&waiting, true);
  int waited = child ? chimi_binding_wait_child(call, 0, wait_child)
                     : chimi_binding_wait(call, 0, wait_pipe[0], CHIMI_BINDING_READABLE);
  CHECK(waited == CHIMI_BINDING_READY, "the wait failed: %d", waited);
  CHECK(binding_handles[call->arguments[0]->as.handle.slot].runner == call,
        "the call did not take its handle back after the wait");
  resource_run(resource, true);
  resource_run(resource, false);
  return chimi_binding_success(call, chimi_binding_void(call));
}

static ChimiBindingResult *binding_wait_on(ChimiBindingCall *call) { return wait_between_runs(call, false); }
static ChimiBindingResult *binding_wait_child_on(ChimiBindingCall *call) { return wait_between_runs(call, true); }

static ChimiBindingResult *binding_close(ChimiBindingCall *call) {
  Resource *resource = chimi_binding_handle(call, 0, &resource_type);
  chimi_binding_close_handle(call, 0, &resource_type);
  /* The state stays valid for the rest of the call that closed it. */
  if (resource) CHECK(atomic_load(&resource->alive) == 1, "a close released the state its own call holds");
  CHECK(chimi_binding_handle(call, 0, &resource_type) == NULL, "a closed handle still gave its state");
  return chimi_binding_success(call, chimi_binding_void(call));
}

/* Handles any thread may use or close, as `slot << 32 | generation`; 0 for none. */
static _Atomic uint64_t shared_handles[HANDLES];

/* A copy of shared handle `packed` in the calling carrier's heap, counted as
   a copy of a message would be. */
static ChimiValue *handle_copy(uint64_t packed) {
  ChimiValue *value = new_value(VALUE_NATIVE_HANDLE);
  value->as.handle.slot = (uint32_t)(packed >> 32);
  value->as.handle.generation = (uint32_t)packed;
  value->as.handle.name = resource_type.name;
  counted_handle_retain(native_handle_key(value->as.handle.slot, value->as.handle.generation));
  return value;
}

static void handle_round(size_t index, uint32_t round) {
  size_t at = (round * 7 + index * 13) % HANDLES;
  if (round % 8 == index) {
    ChimiControl *opened = invoke_binding(&runtime, BINDING_OPEN, NULL, 0, 0);
    CHECK(opened->kind == CONTROL_SUCCESS && opened->value->kind == VALUE_NATIVE_HANDLE, "open failed");
    atomic_store(&shared_handles[at],
                 (uint64_t)opened->value->as.handle.slot << 32 | opened->value->as.handle.generation);
    return;
  }
  uint64_t packed = atomic_load(&shared_handles[at]);
  if (packed == 0) return;
  ChimiValue *arguments[] = {handle_copy(packed)};
  uint32_t binding = round % 16 == 5 ? BINDING_CLOSE : BINDING_USE;
  CHECK(invoke_binding(&runtime, binding, arguments, 1, 0)->kind == CONTROL_SUCCESS, "binding %u failed", binding);
}

/* Lines read from standard input, by number, and console lines written. */
static _Atomic uint32_t input_seen[INPUT_LINES];
static _Atomic bool input_done;

/* Takes one line from standard input as process_utf8_lines does: the input's
   lock is held from its first byte to its newline. */
static void input_round(void) {
  if (atomic_load(&input_done)) return;
  char line[32];
  size_t length = 0;
  input_lock(&standard_input);
  int byte;
  while ((byte = input_next(&runtime, &standard_input)) >= 0 && byte != '\n')
    if (length < sizeof(line) - 1) line[length++] = (char)byte;
  input_unlock(&standard_input);
  if (byte < 0) {
    CHECK(byte == INPUT_END && length == 0, "standard input failed or ended mid-line: %d after %zu bytes (%s)", byte, length, strerror(errno));
    atomic_store(&input_done, true);
    return;
  }
  line[length] = '\0';
  unsigned number = 0;
  CHECK(sscanf(line, "input-line-%05u", &number) == 1 && number < INPUT_LINES && length == 16,
        "a torn input line: %s", line);
  atomic_fetch_add(&input_seen[number], 1);
}

static FILE *console_file;

static void console_round(size_t index, uint32_t round) {
  char text[160];
  int length = snprintf(text, sizeof(text), "console-%zu-%04u-%0120d", index, round, 0);
  CHECK(console_write_line(&runtime, &console_streams[1], text, (size_t)length) == 0, "console write failed");
}

static int64_t first_field(const ChimiControl *control) {
  CHECK(control->kind == CONTROL_SUCCESS && control->value->kind == VALUE_TUPLE, "expected a tuple");
  return control->value->as.record.fields[0].value->as.integer;
}

static void ring_put(Ring *ring, RuntimeFragment *fragment) {
  size_t head = atomic_load_explicit(&ring->head, memory_order_relaxed);
  while (head - atomic_load_explicit(&ring->tail, memory_order_acquire) == RING) {}
  ring->slots[head % RING] = fragment;
  atomic_store_explicit(&ring->head, head + 1, memory_order_release);
}

static RuntimeFragment *ring_take(Ring *ring) {
  size_t tail = atomic_load_explicit(&ring->tail, memory_order_relaxed);
  if (tail == atomic_load_explicit(&ring->head, memory_order_acquire)) return NULL;
  RuntimeFragment *fragment = ring->slots[tail % RING];
  atomic_store_explicit(&ring->tail, tail + 1, memory_order_release);
  return fragment;
}

/* Adopts every other fragment into the taker's heap and frees the rest. */
static void consume(RuntimeFragment *fragment, uint64_t *taken) {
  if (++*taken % 2) {
    void *roots[FRAGMENT_ROOTS];
    fragment_adopt(fragment, roots);
    CHECK(((ChimiValue *)roots[0])->kind == VALUE_LIST, "adopted the wrong value");
  } else {
    fragment_free(fragment);
  }
}

static void *carrier_thread(void *raw_index) {
  size_t index = (size_t)(uintptr_t)raw_index;
  carrier_thread_enter(index + 1);
  CHECK(carrier == carrier_at(index + 1), "thread %zu runs carrier %p", index, (void *)carrier);
  RuntimeHeap *heap = heap_create();
  carrier->fiber.heap = heap;
  Ring *ring = &rings[index / 2];
  bool producer = index % 2 == 0;
  uint64_t taken = 0;
  for (uint32_t round = 0; round < ROUNDS; ++round) {
    ChimiValue *values[] = {integer_value(round), integer_value((int64_t)index)};
    ChimiValue *list = list_from_values(values, 2);
    if (producer) {
      void *roots[] = {list};
      AllocationKind kinds[] = {ALLOCATION_VALUE};
      RuntimeFragment *fragment = NULL;
      CHECK(fragment_copy(roots, kinds, 1, &fragment) == HEAP_OK, "copy failed");
      ring_put(ring, fragment);
    } else {
      RuntimeFragment *fragment;
      while ((fragment = ring_take(ring))) consume(fragment, &taken);
    }
    uint64_t span = begin_node(&runtime, 0, 1, "carrier_round");
    spans[index * ROUNDS + round] = span;
    for (uint32_t epoch = 0; epoch < EPOCHS_PER_ROUND; ++epoch)
      epochs[(index * ROUNDS + round) * EPOCHS_PER_ROUND + epoch] = take_mark_epoch();
    emit_event(&runtime, "operation_started", 0, span, 1, "carrier_round", NULL);
    ChimiSchedulerTraceEvent started = {.task = index + 1};
    emit_scheduler_event(&runtime, &started);
    profile_leave(profile_enter(0, "carrier_round"));
    draws[index * ROUNDS + round] = random_next();
    ChimiValue *arguments[] = {recorder, list};
    CHECK(invoke_test_log(CHIMI_INTRINSIC_TEST_RECORD, arguments, 2, 0)->kind == CONTROL_SUCCESS,
          "Test.record failed");
    handle_round(index, round);
    input_round();
    console_round(index, round);
    if (round % READ_EVERY == 0) {
      if (index == 0 && round == CLOSE_AT) CHECK(fs_close(shared_file), "the shared file was closed early");
      int64_t code = first_field(fs_read(&runtime, shared_file));
      if (code == FS_CLOSED) atomic_fetch_add(&closed_reads, 1);
      else CHECK(code == 1, "read returned %" PRId64, code);
    }
  }
  if (producer) {
    atomic_store_explicit(&ring->done, true, memory_order_release);
  } else {
    for (;;) {
      bool done = atomic_load_explicit(&ring->done, memory_order_acquire);
      RuntimeFragment *fragment;
      while ((fragment = ring_take(ring))) consume(fragment, &taken);
      if (done) break;
    }
    CHECK(taken == ROUNDS, "thread %zu took %" PRIu64 " fragments", index, taken);
  }
  heap_destroy(heap);
  carrier->fiber.heap = &main_heap;
  return NULL;
}

/* The two handles the threads share below, as `slot << 32 | generation`. */
static uint64_t pair_handles[2];
enum { PAIR_ROUNDS = 3000 };

/* Round `round`'s second argument: the other handle, except that every
   fifth round names its first handle twice, which must not make the call
   wait for itself. */
static size_t pair_second(uint32_t round, size_t first) { return round % 5 == 1 ? first : 1 - first; }

/* A pair every third round, when its two handles differ, and a use else. */
static uint32_t pair_binding(uint32_t round) {
  return round % 3 == 0 && round % 5 != 1 ? BINDING_PAIR : BINDING_USE;
}

/* Each thread runs uses of either handle and pairs of both, in both
   orders, so two calls that share a handle overlap often. */
static void *pair_thread(void *raw_index) {
  size_t index = (size_t)(uintptr_t)raw_index;
  carrier_thread_enter(index + 1);
  RuntimeHeap *heap = heap_create();
  carrier->fiber.heap = heap;
  for (uint32_t round = 0; round < PAIR_ROUNDS; ++round) {
    size_t first = (round + index) % 2;
    ChimiValue *arguments[] = {handle_copy(pair_handles[first]),
                               handle_copy(pair_handles[pair_second(round, first)])};
    uint32_t binding = pair_binding(round);
    uint32_t count = binding == BINDING_PAIR || round % 5 == 1 ? 2 : 1;
    CHECK(invoke_binding(&runtime, binding, arguments, count, 0)->kind == CONTROL_SUCCESS,
          "binding %u failed", binding);
  }
  heap_destroy(heap);
  carrier->fiber.heap = &main_heap;
  return NULL;
}

static Resource *pair_resource(size_t index) {
  return binding_handles[(uint32_t)(pair_handles[index] >> 32)].state;
}

static void one_call_runs_on_a_handle_at_a_time(void) {
  for (size_t index = 0; index < 2; ++index) {
    ChimiControl *opened = invoke_binding(&runtime, BINDING_OPEN, NULL, 0, 0);
    CHECK(opened->kind == CONTROL_SUCCESS, "open failed");
    pair_handles[index] = (uint64_t)opened->value->as.handle.slot << 32 | opened->value->as.handle.generation;
  }
  pthread_t threads[THREADS];
  for (size_t index = 0; index < THREADS; ++index)
    CHECK(pthread_create(&threads[index], NULL, pair_thread, (void *)(uintptr_t)index) == 0, "start");
  for (size_t index = 0; index < THREADS; ++index) pthread_join(threads[index], NULL);
  uint64_t calls = pair_resource(0)->calls + pair_resource(1)->calls;
  uint64_t pairs = 0;
  for (uint32_t round = 0; round < PAIR_ROUNDS; ++round) pairs += pair_binding(round) == BINDING_PAIR;
  CHECK(calls == THREADS * (PAIR_ROUNDS + pairs), "%" PRIu64 " calls ran", calls);
}

static void *wait_thread(void *raw_binding) {
  uint32_t binding = (uint32_t)(uintptr_t)raw_binding;
  carrier_thread_enter(1);
  RuntimeHeap *heap = heap_create();
  carrier->fiber.heap = heap;
  ChimiValue *arguments[] = {handle_copy(pair_handles[0])};
  CHECK(invoke_binding(&runtime, binding, arguments, 1, 0)->kind == CONTROL_SUCCESS, "binding %u failed", binding);
  heap_destroy(heap);
  carrier->fiber.heap = &main_heap;
  return NULL;
}

/* A use of the handle a call is waiting on, for a descriptor (`child`
   false) or a child, runs before the wait ends; were the handle still
   taken, the use would wait for good and the alarm fail the run. */
static void a_waiting_call_lets_its_handle_go(bool child) {
  CHECK(pipe(wait_pipe) == 0, "pipe");
  if (child) {
    fflush(NULL);
    wait_child = fork();
    CHECK(wait_child >= 0, "fork");
    if (wait_child == 0) {
      char byte;
      _exit(read(wait_pipe[0], &byte, 1) == 1 ? 0 : 1);
    }
  }
  atomic_store(&waiting, false);
  pthread_t thread;
  uint32_t binding = child ? BINDING_WAIT_CHILD_ON : BINDING_WAIT_ON;
  CHECK(pthread_create(&thread, NULL, wait_thread, (void *)(uintptr_t)binding) == 0, "start");
  while (!atomic_load(&waiting)) {}
  uint64_t before = pair_resource(0)->calls;
  ChimiValue *arguments[] = {handle_copy(pair_handles[0])};
  CHECK(invoke_binding(&runtime, BINDING_USE, arguments, 1, 0)->kind == CONTROL_SUCCESS, "use failed");
  CHECK(write(wait_pipe[1], "x", 1) == 1, "write the pipe");
  pthread_join(thread, NULL);
  CHECK(pair_resource(0)->calls == before + 2, "the use and the wait's second run ran");
  if (child) CHECK(waitpid(wait_child, NULL, 0) == wait_child, "reap the child");
  close(wait_pipe[0]);
  close(wait_pipe[1]);
}

/* A module constant that shares a vector eight times its length, so a short
   window of it would compact it in place on one carrier. */
enum { CONSTANT_LENGTH = 8, CONSTANT_VECTOR = 64, SLICE_ROUNDS = 2000 };
static ChimiValue *shared_constant;
/* A whole constant of CONSTANT_VECTOR elements. */
static ChimiValue *shared_whole;

/* A module constant of CONSTANT_VECTOR elements, built whole as
   decode_constant builds one, and a constant of CONSTANT_LENGTH elements
   sharing its vector, which no decoded constant is but which list_slice
   would compact. Both in the static area. */
static ChimiValue *shared_constant_whole(void) {
  AllocationArea *previous = carrier->fiber.target;
  carrier->fiber.target = &static_area;
  ChimiValue *elements[CONSTANT_VECTOR];
  for (uint32_t at = 0; at < CONSTANT_VECTOR; ++at) elements[at] = integer_value(at);
  ChimiValue *whole = list_from_values(elements, CONSTANT_VECTOR);
  carrier->fiber.target = previous;
  return whole;
}
static ChimiValue *constant_window(void) {
  ChimiValue *whole = shared_constant_whole();
  AllocationArea *previous = carrier->fiber.target;
  carrier->fiber.target = &static_area;
  ChimiValue *constant = list_value(whole->as.list.vector, 0, CONSTANT_LENGTH);
  carrier->fiber.target = previous;
  return constant;
}

/* With several carriers the report's peaks are the process's: carrier 1
   allocates what carrier 2 frees, again and again, so carrier 1's own count
   climbs to a thousand times what is ever live at once, while the process
   peaks at one allocation's worth, within the fold tolerance. */
static void a_peak_counts_the_whole_process(void) {
  enum { ROUNDS_OF_HANDOFF = 1000, BYTES = 100 * 1024 };
  HeapTotals start = heap_totals_sum();
  report_process_peaks(&start);
  int64_t live = start.footprint_bytes, fragments = start.fragment_footprint_bytes;
  for (int round = 0; round < ROUNDS_OF_HANDOFF; ++round) {
    carrier_bind(carrier_at(1));
    footprint_add(BYTES);
    fragment_footprint_add(BYTES);
    carrier_bind(carrier_at(2));
    footprint_sub(BYTES);
    fragment_footprint_sub(BYTES);
  }
  carrier_bind(&carrier_first);
  CHECK(carrier_at(1)->heap_totals.peak_footprint_bytes >= (int64_t)ROUNDS_OF_HANDOFF * BYTES,
        "carrier 1's own count did not drift");
  HeapTotals end = heap_totals_sum();
  report_process_peaks(&end);
  int64_t slack = (int64_t)carrier_count * FOOTPRINT_FOLD_SLACK;
  int64_t peak = start.peak_footprint_bytes > live + BYTES ? start.peak_footprint_bytes : live + BYTES;
  int64_t fragment_peak = start.peak_fragment_footprint_bytes > fragments + BYTES
                            ? start.peak_fragment_footprint_bytes : fragments + BYTES;
  CHECK(end.footprint_bytes == live, "the footprint did not come back");
  CHECK(end.peak_footprint_bytes >= peak - slack && end.peak_footprint_bytes <= peak + slack,
        "process peak %" PRId64 ", true peak %" PRId64, end.peak_footprint_bytes, peak);
  CHECK(end.peak_fragment_footprint_bytes >= fragment_peak - slack &&
          end.peak_fragment_footprint_bytes <= fragment_peak + slack,
        "process fragment peak %" PRId64 ", true peak %" PRId64, end.peak_fragment_footprint_bytes, fragment_peak);
}

/* Every count the process-wide ones should hold, within the fold slack. */
static void the_process_counts_follow_the_carriers(void) {
  HeapTotals sum = heap_totals_sum();
  int64_t slack = (int64_t)carrier_count * FOOTPRINT_FOLD_SLACK;
  int64_t process = atomic_load(&process_footprint_bytes);
  int64_t fragments = atomic_load(&process_fragment_footprint_bytes);
  CHECK(process >= sum.footprint_bytes - slack && process <= sum.footprint_bytes + slack,
        "process footprint %" PRId64 ", carriers' sum %" PRId64, process, sum.footprint_bytes);
  CHECK(fragments >= sum.fragment_footprint_bytes - slack && fragments <= sum.fragment_footprint_bytes + slack,
        "process fragment footprint %" PRId64 ", carriers' sum %" PRId64, fragments, sum.fragment_footprint_bytes);
}

/* The report's peak counts young bytes a long pass has not flushed yet, and
   a fall on a carrier that then only waits. Carrier 1 holds a pass of young
   values while carrier 2 rises and falls, so the true peak is both at once;
   then carrier 2 frees a block alone and carrier 1 allocates as much. */
static void a_peak_counts_unflushed_young_bytes_and_lone_falls(void) {
  enum { YOUNG_VALUES = 65536, RISE = 8 * 1024 * 1024, LONE = 2 * RISE };
  int64_t slack = (int64_t)carrier_count * FOOTPRINT_FOLD_SLACK;
  HeapTotals start = heap_totals_sum();
  report_process_peaks(&start);
  carrier_bind(carrier_at(1));
  RuntimeHeap *heap = heap_create();
  RuntimeHeap *previous = carrier->fiber.heap;
  carrier->fiber.heap = heap;
  AllocationScope pass;
  allocation_scope_begin(&pass);
  int64_t before_young = heap_totals_sum().footprint_bytes;
  for (uint32_t index = 0; index < YOUNG_VALUES; ++index) (void)integer_value(index);
  CHECK(carrier->pending_footprint_bytes > 0, "the pass's young bytes were flushed");
  int64_t with_young = heap_totals_sum().footprint_bytes;
  carrier_bind(carrier_at(2));
  footprint_add(RISE);
  int64_t true_peak = heap_totals_sum().footprint_bytes;
  footprint_sub(RISE);
  carrier_bind(carrier_at(1));
  allocation_scope_end(&pass, NULL, NULL);
  carrier->fiber.heap = previous;
  heap_destroy(heap);
  CHECK(with_young - before_young > 4 * slack, "the young pass holds only %" PRId64 " bytes",
        with_young - before_young);
  /* Carrier 2 frees a block it never allocated and then waits; carrier 1
     allocates as much. The program never holds both. */
  int64_t lone = heap_totals_sum().footprint_bytes;
  carrier_bind(carrier_at(2));
  footprint_sub(LONE);
  carrier_bind(carrier_at(1));
  footprint_add(LONE);
  CHECK(true_peak > start.peak_footprint_bytes + slack && true_peak > lone + slack,
        "the overlap is not the program's peak");
  carrier_bind(&carrier_first);
  HeapTotals end = heap_totals_sum();
  report_process_peaks(&end);
  CHECK(end.peak_footprint_bytes >= true_peak - slack && end.peak_footprint_bytes <= true_peak + slack,
        "process peak %" PRId64 ", true peak %" PRId64, end.peak_footprint_bytes, true_peak);
  the_process_counts_follow_the_carriers();
}

/* One carrier sees the epochs and span ids a single shared counter would
   give: consecutive, across a block boundary. */
static void a_carrier_alone_takes_consecutive_ids(void) {
  uint64_t first = take_mark_epoch();
  for (uint64_t taken = 1; taken < 2 * MARK_EPOCH_BLOCK; ++taken)
    CHECK(take_mark_epoch() == first + taken, "one carrier's epochs skip after %" PRIu64, first + taken - 1);
  uint64_t span = take_spans(&runtime, 1);
  CHECK(take_spans(&runtime, 3) == span + 1 && take_spans(&runtime, 1) == span + 4,
        "one carrier's span ids are not consecutive");
}

/* On one carrier, a short window of a constant compacts it, as any list. */
static void a_constant_compacts_on_one_carrier(void) {
  ChimiValue *constant = constant_window();
  ChimiValue *window = list_slice(constant, 1, 5);
  CHECK(window && list_get(window, 0)->as.integer == 1, "a window of the constant");
  CHECK(constant->as.list.vector.length == CONSTANT_LENGTH, "one carrier did not compact the constant");
}

static void *slice_thread(void *raw_index) {
  size_t index = (size_t)(uintptr_t)raw_index;
  carrier_thread_enter(index + 1);
  RuntimeHeap *heap = heap_create();
  carrier->fiber.heap = heap;
  for (uint32_t round = 0; round < SLICE_ROUNDS; ++round) {
    ChimiValue *window = list_slice(shared_constant, 1, 5);
    CHECK(window && list_length(window) == 4, "a window of the constant");
    CHECK(window->as.list.vector.length == 4, "a window kept the constant's whole vector");
    for (uint32_t at = 0; at < 4; ++at)
      CHECK(list_get(window, at)->as.integer == (int64_t)at + 1, "element %u of a window", at);
  }
  /* A long window of a whole constant, the shape decode_constant builds,
     shares its vector, as on one carrier. */
  ChimiValue *half = list_slice(shared_whole, 0, CONSTANT_VECTOR / 2);
  CHECK(half && half->as.list.vector.root == shared_whole->as.list.vector.root, "a long window of a constant was copied");
  /* A list in this carrier's own heap still compacts. */
  ChimiValue *elements[CONSTANT_VECTOR];
  for (uint32_t at = 0; at < CONSTANT_VECTOR; ++at) elements[at] = integer_value(at);
  ChimiValue *own = list_value(list_from_values(elements, CONSTANT_VECTOR)->as.list.vector, 0, CONSTANT_LENGTH);
  CHECK(list_slice(own, 1, 5) && own->as.list.vector.length == CONSTANT_LENGTH, "a carrier's own list did not compact");
  heap_destroy(heap);
  carrier->fiber.heap = &main_heap;
  return NULL;
}

/* Every carrier slices one module constant at once, and a list of its own.
   Each window of the constant is a copy in the slicing carrier's heap, and
   the constant keeps its vector, while the carrier's own list compacts.
   Compacting the constant would rewrite it, and allocate in the static area,
   under the other threads, which ThreadSanitizer reports. */
static void slicing_a_shared_constant_from_every_thread(void) {
  shared_constant = constant_window();
  shared_whole = shared_constant_whole();
  ListNode *root = shared_constant->as.list.vector.root;
  pthread_t threads[THREADS];
  for (size_t index = 0; index < THREADS; ++index)
    CHECK(pthread_create(&threads[index], NULL, slice_thread, (void *)(uintptr_t)index) == 0, "start");
  for (size_t index = 0; index < THREADS; ++index) pthread_join(threads[index], NULL);
  CHECK(shared_constant->as.list.vector.root == root &&
        shared_constant->as.list.vector.length == CONSTANT_VECTOR,
        "the constant was compacted while carriers ran on threads");
}

static int compare_words(const void *left, const void *right) {
  uint64_t a = *(const uint64_t *)left, b = *(const uint64_t *)right;
  return a < b ? -1 : a > b;
}

/* A close while a read holds the descriptor leaves it open, and its slot
   unused, until the read gives it back. */
static void a_close_during_a_read_waits_for_it(void) {
  int fd = open("/dev/null", O_RDONLY);
  CHECK(fd >= 0, "open /dev/null");
  int64_t handle = fs_insert(fd);
  CHECK(fs_take(handle) == fd, "take an open handle");
  CHECK(fs_close(handle), "close an open handle");
  CHECK(fcntl(fd, F_GETFD) != -1, "the descriptor was closed under a read");
  CHECK(fs_take(handle) == -1, "a closed handle gave its descriptor");
  int other = open("/dev/null", O_RDONLY);
  CHECK(other >= 0, "open /dev/null");
  int64_t next = fs_insert(other);
  CHECK(((uint64_t)next & 0xffffffffu) != ((uint64_t)handle & 0xffffffffu),
        "a slot was reused while its descriptor was in use");
  fs_give(handle);
  errno = 0;
  CHECK(fcntl(fd, F_GETFD) == -1 && errno == EBADF, "the last user did not close the descriptor");
  CHECK(!fs_close(handle), "a handle closed twice");
  int third = open("/dev/null", O_RDONLY);
  CHECK(third >= 0, "open /dev/null");
  int64_t reused = fs_insert(third);
  CHECK(((uint64_t)reused & 0xffffffffu) == ((uint64_t)handle & 0xffffffffu), "the freed slot was not reused");
  CHECK(fs_take(handle) == -1, "a closed handle reached the file that reused its slot");
  CHECK(fs_close(reused), "close the third handle");
  CHECK(fs_close(next), "close the second handle");
  errno = 0;
  CHECK(fcntl(other, F_GETFD) == -1 && errno == EBADF, "an unused handle's close did not close");
}

/* Carrier threads lending out one file at once, first with no close until
   they finish, then with thread 0 closing it halfway: every loan is counted,
   so no descriptor is closed under a loan and the close or the last give
   after it releases the descriptor. A lost count leaves it open or closes it
   early. */
enum { LOAN_ROUNDS = 20000 };
static int64_t lent_file;
static int lent_fd;
static uint32_t lent_close_at;

static void *lend_file(void *raw_index) {
  size_t index = (uintptr_t)raw_index;
  for (uint32_t round = 0; round < LOAN_ROUNDS; ++round) {
    if (index == 0 && round == lent_close_at) CHECK(fs_close(lent_file), "the lent file was closed early");
    int fd = fs_take(lent_file);
    if (fd < 0) continue;
    CHECK(fd == lent_fd, "a loan named another descriptor");
    CHECK(fcntl(fd, F_GETFD) != -1, "a descriptor was closed under a loan");
    fs_give(lent_file);
  }
  return NULL;
}

static void lend_one_file(uint32_t close_at) {
  lent_fd = open("/dev/null", O_RDONLY);
  CHECK(lent_fd >= 0, "open /dev/null");
  lent_file = fs_insert(lent_fd);
  lent_close_at = close_at;
  pthread_t threads[THREADS];
  for (size_t index = 0; index < THREADS; ++index)
    CHECK(pthread_create(&threads[index], NULL, lend_file, (void *)(uintptr_t)index) == 0, "start");
  for (size_t index = 0; index < THREADS; ++index) pthread_join(threads[index], NULL);
  if (close_at >= LOAN_ROUNDS) CHECK(fs_close(lent_file), "close the lent file");
  CHECK(fs_take(lent_file) == -1, "a closed file was lent");
  errno = 0;
  CHECK(fcntl(lent_fd, F_GETFD) == -1 && errno == EBADF, "a loan was lost: the descriptor was left open");
}

static void the_file_table_counts_loans_across_threads(void) {
  lend_one_file(LOAN_ROUNDS);
  lend_one_file(LOAN_ROUNDS / 2);
}

static void *start_a_task_unbound(void *unused) {
  (void)unused;
  (void)chimi_task_entry(NULL);
  return NULL;
}

/* With several carriers, a thread that never entered its own carrier aliases
   carrier 0; starting a task there aborts before touching it. Process bodies
   make the same check (require_bound_carrier), but their entry is built only
   into the process kernel, which this fixture does not link. */
static void a_fiber_on_an_unbound_thread_aborts(void *(*start)(void *), const char *what) {
  fflush(NULL);
  pid_t child = fork();
  CHECK(child >= 0, "fork");
  if (child == 0) {
    int quiet = open("/dev/null", O_WRONLY);
    if (quiet >= 0) dup2(quiet, STDERR_FILENO);
    pthread_t thread;
    if (pthread_create(&thread, NULL, start, NULL) != 0) _exit(2);
    pthread_join(thread, NULL);
    _exit(0);
  }
  int status = 0;
  CHECK(waitpid(child, &status, 0) == child, "wait for the child");
  CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT, "the unbound %s ran (status %d)", what, status);
}

int main(void) {
  /* A race that corrupts a table can leave a thread spinning; fail instead. */
  alarm(60);
  carrier_bind(&carrier_first);
  decode_constants();
  a_close_during_a_read_waits_for_it();
  the_file_table_counts_loans_across_threads();
  a_constant_compacts_on_one_carrier();
  a_carrier_alone_takes_consecutive_ids();
  carriers_create(THREADS + 1);
  CHECK(carrier_count == THREADS + 1, "%zu carriers", carrier_count);
  a_peak_counts_the_whole_process();
  a_peak_counts_unflushed_young_bytes_and_lone_falls();
  a_fiber_on_an_unbound_thread_aborts(start_a_task_unbound, "task");

  runtime.telemetry = tmpfile();
  CHECK(runtime.telemetry, "tmpfile");
  profile_rows = calloc(1, sizeof(*profile_rows));
  CHECK(profile_rows, "profile rows");
  random_seed = 7;
  random_seed_given = true;
  recorder = new_test_log(VALUE_TEST_RECORDER);
  int zero = open("/dev/zero", O_RDONLY);
  CHECK(zero >= 0, "open /dev/zero");
  shared_file = fs_insert(zero);
  int input[2];
  CHECK(pipe(input) == 0, "pipe");
  for (unsigned number = 0; number < INPUT_LINES; ++number) {
    char line[18];
    snprintf(line, sizeof(line), "input-line-%05u\n", number);
    CHECK(write(input[1], line, 17) == 17, "fill standard input");
  }
  close(input[1]);
  CHECK(dup2(input[0], STDIN_FILENO) == STDIN_FILENO, "dup2");
  close(input[0]);
  console_file = tmpfile();
  CHECK(console_file, "tmpfile");
  console_streams[1].mode = CONSOLE_DIRECT;
  console_streams[1].file = console_file;
  HeapTotals before = heap_totals_sum();

  pthread_t threads[THREADS];
  for (size_t index = 0; index < THREADS; ++index)
    CHECK(pthread_create(&threads[index], NULL, carrier_thread, (void *)(uintptr_t)index) == 0, "start");
  for (size_t index = 0; index < THREADS; ++index) pthread_join(threads[index], NULL);

  TestLog *log = test_log(recorder);
  CHECK(log->length == THREADS * ROUNDS, "%" PRIu32 " recorded", log->length);
  for (uint32_t index = 0; index < log->length; ++index) fragment_free(log->values[index]);
  log->length = 0;
  HeapTotals after = heap_totals_sum();
  CHECK(after.live_allocations == before.live_allocations, "%" PRId64 " allocations leaked",
        after.live_allocations - before.live_allocations);
  CHECK(after.footprint_bytes == before.footprint_bytes, "%" PRId64 " bytes leaked",
        after.footprint_bytes - before.footprint_bytes);
  CHECK(after.live_fragments == before.live_fragments && after.fragment_footprint_bytes == before.fragment_footprint_bytes,
        "fragments leaked");
  CHECK(after.live_heaps == before.live_heaps, "heaps leaked");
  the_process_counts_follow_the_carriers();
  /* Each thread recorded ROUNDS fragments, which carrier 0 freed above, and
     each producer copied ROUNDS more, which its consumer took. */
  for (size_t index = 0; index < THREADS; ++index) {
    int64_t expected = index % 2 == 0 ? 2 * ROUNDS : 0;
    CHECK(carrier_at(index + 1)->heap_totals.live_fragments == expected, "carrier %zu counts %" PRId64 " fragments",
          index + 1, carrier_at(index + 1)->heap_totals.live_fragments);
  }
  CHECK(carrier_first.heap_totals.live_fragments == before.live_fragments - THREADS * ROUNDS,
        "carrier 0 counts %" PRId64 " fragments", carrier_first.heap_totals.live_fragments);

  /* Several carriers take span ids and mark epochs in blocks, so they are
     unique but not dense. */
  qsort(spans, THREADS * ROUNDS, sizeof(uint64_t), compare_words);
  CHECK(spans[0] >= 2, "span id %" PRIu64 " is below 2", spans[0]);
  for (size_t index = 1; index < THREADS * ROUNDS; ++index)
    CHECK(spans[index] != spans[index - 1], "span id %" PRIu64 " taken twice", spans[index]);
  size_t epoch_count = THREADS * ROUNDS * EPOCHS_PER_ROUND;
  qsort(epochs, epoch_count, sizeof(uint64_t), compare_words);
  CHECK(epochs[0] != 0, "mark epoch 0, which every new header holds, was handed out");
  for (size_t index = 1; index < epoch_count; ++index)
    CHECK(epochs[index] != epochs[index - 1], "mark epoch %" PRIu64 " taken twice", epochs[index]);

  rewind(runtime.telemetry);
  char line[1024];
  uint64_t events = 0;
  while (fgets(line, sizeof(line), runtime.telemetry)) {
    size_t length = strlen(line);
    CHECK(strncmp(line, "{\"schema\":", 10) == 0 && length > 3 && strcmp(line + length - 3, "}}\n") == 0,
          "a torn event: %s", line);
    const char *sequence = strstr(line, "\"sequence\":");
    CHECK(sequence && strtoull(sequence + 11, NULL, 10) == events, "event %" PRIu64 " is out of order: %s",
          events, line);
    ++events;
  }
  CHECK(events == 2 * THREADS * ROUNDS && runtime.trace_events_constructed == events, "%" PRIu64 " events", events);

  CHECK(profile_rows[0].evaluations == THREADS * ROUNDS, "%" PRIu64 " evaluations", profile_rows[0].evaluations);

  uint64_t seeded[THREADS * ROUNDS];
  random_read = false;
  for (size_t index = 0; index < THREADS * ROUNDS; ++index) seeded[index] = random_next();
  qsort(seeded, THREADS * ROUNDS, sizeof(uint64_t), compare_words);
  qsort(draws, THREADS * ROUNDS, sizeof(uint64_t), compare_words);
  CHECK(memcmp(seeded, draws, sizeof(seeded)) == 0, "the draws are not the seeded stream's");

  CHECK(atomic_load(&closed_reads) > 0, "no read saw the close");
  CHECK(fs_take(shared_file) == -1, "the shared file is still open");
  errno = 0;
  CHECK(fcntl(zero, F_GETFD) == -1 && errno == EBADF, "the shared file's descriptor is still open");
  CHECK(atomic_load(&resources_opened) > HANDLES && atomic_load(&resources_released) == atomic_load(&resources_opened),
        "%" PRId64 " resources opened, %" PRId64 " released", atomic_load(&resources_opened),
        atomic_load(&resources_released));
  for (unsigned number = 0; number < INPUT_LINES; ++number)
    CHECK(atomic_load(&input_seen[number]) == 1, "input line %u was read %u times", number,
          atomic_load(&input_seen[number]));

  rewind(console_file);
  uint64_t console_lines = 0;
  while (fgets(line, sizeof(line), console_file)) {
    size_t index = 0;
    unsigned round = 0;
    CHECK(sscanf(line, "console-%zu-%04u-", &index, &round) == 2 && strlen(line) == 136,
          "a torn console line: %s", line);
    ++console_lines;
  }
  CHECK(console_lines == THREADS * ROUNDS, "%" PRIu64 " console lines", console_lines);
  one_call_runs_on_a_handle_at_a_time();
  a_waiting_call_lets_its_handle_go(false);
  a_waiting_call_lets_its_handle_go(true);
  slicing_a_shared_constant_from_every_thread();
  puts("ok");
  return 0;
}
