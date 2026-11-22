/* Four-thread stress of std/metrics' aggregate (observability.md, "The
   runner's aggregate"). Each thread adds to one shared counter and to label
   sets of its own, so the name and series tables grow several times while
   the other threads probe them. At the end every total must be exact. Built
   with ThreadSanitizer when the compiler supports it, so an access outside
   metrics_lock fails the run.

   usage: metrics-stress     prints "ok" */
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

enum { THREADS = 4, ROUNDS = 2000, SLOTS = 200 };

static void *recorder(void *argument) {
  int thread = (int)(intptr_t)argument;
  char labels[64];
  for (int round = 0; round < ROUNDS; ++round) {
    metrics_add(0, "\"shared\"", 8, 1.0, "{}", 2);
    int length = snprintf(labels, sizeof labels, "{\"thread\":\"%d\",\"slot\":\"%d\"}", thread,
                          round % SLOTS);
    metrics_add(0, "\"own\"", 5, 1.0, labels, (size_t)length);
    metrics_add(2, "\"seconds\"", 9, 0.5, "{}", 2);
  }
  return NULL;
}

int main(void) {
  pthread_t threads[THREADS];
  for (int thread = 0; thread < THREADS; ++thread)
    CHECK(pthread_create(&threads[thread], NULL, recorder, (void *)(intptr_t)thread) == 0, "start %d", thread);
  for (int thread = 0; thread < THREADS; ++thread) CHECK(pthread_join(threads[thread], NULL) == 0, "join");
  CHECK(metric_series_count == 2 + THREADS * SLOTS, "%zu series", metric_series_count);
  for (size_t index = 0; index < metric_series_count; ++index) {
    const MetricSeries *series = &metric_series[index];
    if (series->name_length == 8)
      CHECK(series->total == THREADS * ROUNDS, "shared total %" PRId64, series->total);
    else if (series->name_length == 5)
      CHECK(series->total == ROUNDS / SLOTS, "own total %" PRId64, series->total);
    else
      CHECK(series->count == (uint64_t)THREADS * ROUNDS && series->sum == 0.5 * THREADS * ROUNDS, "observed");
  }
  puts("ok");
  return 0;
}
