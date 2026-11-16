/* Two-thread stress of the shared handle table (parallel-scheduler.md,
   "Handle counts"). Thread A holds one anchor reference to each key, retains a
   further reference, and hands it to thread B, which drops it, the way a copy
   made on one carrier is freed on another. B also retains and drops keys of its
   own, so the table grows and deletes while A works. A drop that removes the last
   reference while the anchor is still held would be a premature release: B
   checks that none happens, and A checks that dropping each anchor at the end is
   the last reference. Built with ThreadSanitizer when the compiler supports it,
   so an access outside the table's lock fails the run.

   usage: handle-table-stress     prints "ok" */
#include "runtime.c"

const char *CHIMI_COMPUTATION = "heap-test";
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

enum { KEYS = 512, ROUNDS = 200, OWN_KEYS = 4096 };

/* A single-producer, single-consumer ring of handed-over keys. */
static uint64_t ring[1024];
static _Atomic size_t ring_head;
static _Atomic size_t ring_tail;
static _Atomic bool producer_done;

static bool locked_drop(uint64_t key) {
  pthread_mutex_lock(&handle_table.lock);
  bool last = handle_table_drop(key);
  pthread_mutex_unlock(&handle_table.lock);
  return last;
}

static void *producer(void *unused) {
  (void)unused;
  for (int round = 0; round < ROUNDS; ++round) {
    for (uint64_t key = 1; key <= KEYS; ++key) {
      channel_endpoint_retain(key);
      size_t head = atomic_load_explicit(&ring_head, memory_order_relaxed);
      while (head - atomic_load_explicit(&ring_tail, memory_order_acquire) == 1024) {}
      ring[head % 1024] = key;
      atomic_store_explicit(&ring_head, head + 1, memory_order_release);
    }
  }
  atomic_store_explicit(&producer_done, true, memory_order_release);
  return NULL;
}

static void *consumer(void *unused) {
  (void)unused;
  uint64_t own = 0;
  for (;;) {
    size_t tail = atomic_load_explicit(&ring_tail, memory_order_relaxed);
    if (tail == atomic_load_explicit(&ring_head, memory_order_acquire)) {
      if (atomic_load_explicit(&producer_done, memory_order_acquire) &&
          tail == atomic_load_explicit(&ring_head, memory_order_acquire))
        break;
      continue;
    }
    uint64_t key = ring[tail % 1024];
    atomic_store_explicit(&ring_tail, tail + 1, memory_order_release);
    CHECK(!locked_drop(key), "key %" PRIu64 " reached zero while its anchor was held", key);
    /* Keys of B's own, far from A's, retained twice and dropped twice. */
    uint64_t mine = (UINT64_C(1) << 40) + (own++ % OWN_KEYS) + 1;
    channel_endpoint_retain(mine);
    channel_endpoint_retain(mine);
    CHECK(!locked_drop(mine), "own key %" PRIu64 " reached zero early", mine);
    CHECK(locked_drop(mine), "own key %" PRIu64 " kept a reference", mine);
  }
  return NULL;
}

int main(void) {
  channel_endpoints_reset();
  for (uint64_t key = 1; key <= KEYS; ++key) channel_endpoint_retain(key);
  pthread_t threads[2];
  CHECK(pthread_create(&threads[0], NULL, producer, NULL) == 0, "start producer");
  CHECK(pthread_create(&threads[1], NULL, consumer, NULL) == 0, "start consumer");
  for (int index = 0; index < 2; ++index) pthread_join(threads[index], NULL);
  for (uint64_t key = 1; key <= KEYS; ++key)
    CHECK(locked_drop(key), "key %" PRIu64 " kept a reference after its anchor", key);
  CHECK(!handles_live(), "%zu keys left in the table", (size_t)handle_table.count);
  puts("ok");
  return 0;
}
