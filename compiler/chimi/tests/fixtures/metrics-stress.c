/* Four-thread stress of std/metrics' aggregate (observability.md, "The
   runner's aggregate"). Each thread adds to one shared counter and to label
   sets of its own, so the aggregate grows while the other threads read it.
   At the end every total in the document must be exact. The aggregate and
   its lock are Rust (chimi-runtime intrinsics/metrics_host.rs), which
   ThreadSanitizer cannot see inside, so a missing lock shows up as a lost
   count or a crash in the map rather than a race report.

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

extern void chimi_runtime_metrics_add_v1(int64_t kind, const char *name, size_t name_length, double value,
                                         const char *labels, size_t labels_length);
static ChimiRuntime runtime;

enum { THREADS = 4, ROUNDS = 2000, SLOTS = 200 };

static void *recorder(void *argument) {
  int thread = (int)(intptr_t)argument;
  char labels[64];
  for (int round = 0; round < ROUNDS; ++round) {
    chimi_runtime_metrics_add_v1(0, "\"shared\"", 8, 1.0, "{}", 2);
    int length = snprintf(labels, sizeof labels, "{\"thread\":\"%d\",\"slot\":\"%d\"}", thread,
                          round % SLOTS);
    chimi_runtime_metrics_add_v1(0, "\"own\"", 5, 1.0, labels, (size_t)length);
    chimi_runtime_metrics_add_v1(2, "\"seconds\"", 9, 0.5, "{}", 2);
  }
  return NULL;
}

int main(void) {
  pthread_t threads[THREADS];
  for (int thread = 0; thread < THREADS; ++thread)
    CHECK(pthread_create(&threads[thread], NULL, recorder, (void *)(intptr_t)thread) == 0, "start %d", thread);
  for (int thread = 0; thread < THREADS; ++thread) CHECK(pthread_join(threads[thread], NULL) == 0, "join");
  ChimiOwnedText document = chimi_runtime_metrics_document_v1(&runtime, 0);
  char *text = strndup(document.bytes, document.length);
  chimi_text_release_v1(document);
  CHECK(text != NULL, "copy the document");
  char shared[64], observed[128];
  snprintf(shared, sizeof shared, "{\"name\":\"shared\",\"labels\":{},\"kind\":\"count\",\"value\":%d}",
           THREADS * ROUNDS);
  snprintf(observed, sizeof observed,
           "{\"name\":\"seconds\",\"labels\":{},\"kind\":\"observe\",\"count\":%d,\"sum\":%d.0,",
           THREADS * ROUNDS, THREADS * ROUNDS / 2);
  CHECK(strstr(text, shared) != NULL, "shared total: %s", text);
  CHECK(strstr(text, observed) != NULL, "observed: %s", text);
  /* Each thread's label sets, ROUNDS / SLOTS each, and nothing dropped. */
  char own[32];
  snprintf(own, sizeof own, "\"kind\":\"count\",\"value\":%d}", ROUNDS / SLOTS);
  int owned = 0;
  for (const char *at = strstr(text, "{\"name\":\"own\""); at; at = strstr(at + 1, "{\"name\":\"own\"")) {
    const char *end = strchr(at, '}');
    end = end ? strchr(end + 1, '}') : NULL;
    CHECK(end != NULL && strncmp(end - strlen(own) + 1, own, strlen(own)) == 0, "own total near %.80s", at);
    ++owned;
  }
  CHECK(owned == THREADS * SLOTS, "%d own series", owned);
  CHECK(strstr(text, "\"dropped\":{\"label_sets\":0,\"kind_conflicts\":0}") != NULL, "dropped: %s", text);
  free(text);
  puts("ok");
  return 0;
}
