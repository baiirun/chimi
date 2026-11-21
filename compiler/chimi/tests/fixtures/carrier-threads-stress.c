/* The C runtime's process-wide state under several carrier threads
   (parallel-scheduler.md, step 3 PR 5a). Four threads each enter their own
   carrier and heap and, at once:

   - copy values into fragments that another carrier adopts or frees, so a
     carrier's memory counters go below zero and only their sum is the
     process's;
   - take span ids, emit telemetry events, count the operation profile, draw
     from the `random` stream and record into one shared test Recorder;
   - read one shared file handle, which one of them closes half way.

   The checks: every counter sums back to where it started, span ids and
   telemetry sequence numbers are unique and each event is written whole,
   the profile and the recorder saw every call, the random draws are the
   seeded stream's, and the closed handle reads as closed. Built with
   ThreadSanitizer when the compiler supports it, so an access outside the
   locks fails the run. Two single-threaded checks come first: a close during
   a read leaves the descriptor open until the read gives it back, and a task
   started on a thread that never entered its carrier aborts.

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
const ChimiBinding CHIMI_BINDINGS[] = {{0}};
const uint32_t CHIMI_BINDING_COUNT = 0;

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
static uint64_t draws[THREADS * ROUNDS];

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
    emit_event(&runtime, "operation_started", 0, span, 1, "carrier_round", NULL);
    profile_leave(profile_enter(0, "carrier_round"));
    draws[index * ROUNDS + round] = random_next();
    ChimiValue *arguments[] = {recorder, list};
    CHECK(invoke_test_log(CHIMI_INTRINSIC_TEST_RECORD, arguments, 2, 0)->kind == CONTROL_SUCCESS,
          "Test.record failed");
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
  CHECK(fs_close(next), "close the second handle");
  errno = 0;
  CHECK(fcntl(other, F_GETFD) == -1 && errno == EBADF, "an unused handle's close did not close");
}

static void *start_a_task_unbound(void *unused) {
  (void)unused;
  (void)chimi_task_entry(NULL);
  return NULL;
}

/* With several carriers, a thread that never entered its own carrier aliases
   carrier 0; starting a task there aborts before touching it. */
static void a_task_on_an_unbound_thread_aborts(void) {
  fflush(NULL);
  pid_t child = fork();
  CHECK(child >= 0, "fork");
  if (child == 0) {
    int quiet = open("/dev/null", O_WRONLY);
    if (quiet >= 0) dup2(quiet, STDERR_FILENO);
    pthread_t thread;
    if (pthread_create(&thread, NULL, start_a_task_unbound, NULL) != 0) _exit(2);
    pthread_join(thread, NULL);
    _exit(0);
  }
  int status = 0;
  CHECK(waitpid(child, &status, 0) == child, "wait for the child");
  CHECK(WIFSIGNALED(status) && WTERMSIG(status) == SIGABRT, "the unbound task ran (status %d)", status);
}

int main(void) {
  /* A race that corrupts a table can leave a thread spinning; fail instead. */
  alarm(60);
  carrier_bind(&carrier_first);
  decode_constants();
  a_close_during_a_read_waits_for_it();
  carriers_create(THREADS + 1);
  CHECK(carrier_count == THREADS + 1, "%zu carriers", carrier_count);
  a_task_on_an_unbound_thread_aborts();

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
  /* Each thread recorded ROUNDS fragments, which carrier 0 freed above, and
     each producer copied ROUNDS more, which its consumer took. */
  for (size_t index = 0; index < THREADS; ++index) {
    int64_t expected = index % 2 == 0 ? 2 * ROUNDS : 0;
    CHECK(carrier_at(index + 1)->heap_totals.live_fragments == expected, "carrier %zu counts %" PRId64 " fragments",
          index + 1, carrier_at(index + 1)->heap_totals.live_fragments);
  }
  CHECK(carrier_first.heap_totals.live_fragments == before.live_fragments - THREADS * ROUNDS,
        "carrier 0 counts %" PRId64 " fragments", carrier_first.heap_totals.live_fragments);

  qsort(spans, THREADS * ROUNDS, sizeof(uint64_t), compare_words);
  for (size_t index = 0; index < THREADS * ROUNDS; ++index)
    CHECK(spans[index] == index + 2, "span ids are not 2 to %d: %" PRIu64 " at %zu",
          THREADS * ROUNDS + 1, spans[index], index);

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
  CHECK(events == THREADS * ROUNDS && runtime.trace_events_constructed == events, "%" PRIu64 " events", events);

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
  puts("ok");
  return 0;
}
