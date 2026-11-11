/* White-box test of the native runtime's persistent List. It includes the
   runtime source so the static list primitives are reachable, and supplies an
   empty program so the runtime links without generated program data.
   Prints one measurement line per size and exits nonzero on the first failure. */
#include "runtime.c"

const char *CHIMI_COMPUTATION = "list-runtime-test";
const uint32_t CHIMI_REQUIRES_SCHEDULER = 0;
const char *CHIMI_STRINGS[] = {""};
const uint32_t CHIMI_IDS[] = {0};
const ChimiOrigin CHIMI_ORIGINS[] = {{0}};
const ChimiNode CHIMI_NODES[] = {{0}};
const uint32_t CHIMI_NODE_COUNT = 0;
const ChimiSymbol CHIMI_SYMBOLS[] = {{0}};
const uint32_t CHIMI_SYMBOL_COUNT = 0;
const uint32_t CHIMI_LOCAL_SLOT_COUNT = 0;
const uint32_t CHIMI_REQUIREMENT_SLOT_COUNT = 0;
const ChimiFunction CHIMI_FUNCTIONS[] = {{0}};
const uint32_t CHIMI_FUNCTION_COUNT = 0;
const ChimiConstructor CHIMI_CONSTRUCTORS[] = {{0}};
const ChimiNamedNode CHIMI_NAMED_NODES[] = {{0}};
const ChimiStep CHIMI_STEPS[] = {{0}};
const ChimiArm CHIMI_ARMS[] = {{0}};
const ChimiPattern CHIMI_PATTERNS[] = {{0}};
ChimiNodeFn CHIMI_NODE_FUNCTIONS[] = {NULL};

#define CHECK(condition, ...)                                         \
  do {                                                                \
    if (!(condition)) {                                               \
      fprintf(stderr, "%s:%d: check failed: %s: ", __FILE__, __LINE__, #condition); \
      fprintf(stderr, __VA_ARGS__);                                   \
      fputc('\n', stderr);                                            \
      exit(1);                                                        \
    }                                                                 \
  } while (0)

static int64_t element(const ChimiValue *list, uint32_t index) {
  ChimiValue *value = list_get(list, index);
  CHECK(value && value->kind == VALUE_INTEGER, "index %u of %u", index, list_length(list));
  return value->as.integer;
}

/* Checks that `list` holds offset, offset + 1, ... in order. */
static void expect_sequence(const ChimiValue *list, uint32_t length, int64_t offset) {
  CHECK(list->kind == VALUE_LIST && list_length(list) == length, "length %u, expected %u",
        list_length(list), length);
  for (uint32_t index = 0; index < length; ++index)
    CHECK(element(list, index) == offset + index, "index %u holds %" PRId64, index, element(list, index));
  CHECK(list_get(list, length) == NULL, "read past the end of length %u", length);
}

static ChimiValue *push_sequence(ChimiValue *list, uint32_t count, int64_t first) {
  for (uint32_t index = 0; index < count; ++index) list = list_push(list, integer_value(first + index));
  return list;
}

static void test_push_and_get_across_trie_heights(void) {
  /* 1,100,000 elements need a root of height 4 (above 32^4 = 1,048,576 trie slots). */
  uint32_t length = 1100000;
  ChimiValue *list = push_sequence(list_from_values(NULL, 0), length, 0);
  expect_sequence(list, length, 0);
  CHECK(list->as.list.root->height == 4, "root height %u", list->as.list.root->height);
}

static void test_old_versions_are_unchanged(void) {
  static const uint32_t lengths[] = {0, 1, 31, 32, 33, 64, 1024, 1056, 1057, 32800, 33824};
  enum { VERSION_COUNT = sizeof(lengths) / sizeof(lengths[0]) };
  ChimiValue *versions[VERSION_COUNT];
  ChimiValue *list = list_from_values(NULL, 0);
  for (uint32_t version = 0; version < VERSION_COUNT; ++version) {
    uint32_t previous = version ? lengths[version - 1] : 0;
    list = push_sequence(list, lengths[version] - previous, previous);
    versions[version] = list;
  }
  for (uint32_t version = 0; version < VERSION_COUNT; ++version) {
    /* Two different successors of one version must not see each other. */
    ChimiValue *left = push_sequence(versions[version], 40, -1000000);
    ChimiValue *right = list_push(versions[version], integer_value(-7));
    expect_sequence(versions[version], lengths[version], 0);
    CHECK(list_length(left) == lengths[version] + 40, "left length");
    CHECK(element(left, lengths[version]) == -1000000, "left successor");
    CHECK(element(right, lengths[version]) == -7, "right successor");
    CHECK(list_length(right) == lengths[version] + 1, "right length");
  }
  expect_sequence(list, lengths[VERSION_COUNT - 1], 0);
}

static void test_bulk_construction_matches_push(void) {
  enum { MAXIMUM = 70000 };
  ChimiValue **values = malloc(MAXIMUM * sizeof(*values));
  CHECK(values != NULL, "allocate test values");
  for (uint32_t index = 0; index < MAXIMUM; ++index) values[index] = integer_value(index);
  static const uint32_t lengths[] = {0, 1, 2, 31, 32, 33, 63, 64, 65, 1023, 1024, 1025, 1056, 1057,
                                     1088, 32768, 32800, 32801, 33824, 33825, 65536, 65568, MAXIMUM};
  for (uint32_t which = 0; which < sizeof(lengths) / sizeof(lengths[0]); ++which) {
    uint32_t length = lengths[which];
    ChimiValue *bulk = list_from_values(values, length);
    expect_sequence(bulk, length, 0);
    /* Appending to a bulk-built list relies on the same trie shape as push. */
    expect_sequence(push_sequence(bulk, 2000, length), length + 2000, 0);
    ChimiValue *pushed = push_sequence(list_from_values(NULL, 0), length, 0);
    uint32_t bulk_height = bulk->as.list.root ? bulk->as.list.root->height : 0;
    uint32_t pushed_height = pushed->as.list.root ? pushed->as.list.root->height : 0;
    CHECK(bulk_height == pushed_height, "length %u: bulk height %u, push height %u", length,
          bulk_height, pushed_height);
  }
  free(values);
}

/* Builds `length` elements by repeated append inside a scope, keeps only the
   final list, and reports total and retained bytes per element. */
static void measure_accumulation(uint32_t length, double *allocated_per_element,
                                 double *live_per_element) {
  AllocationScope scope;
  allocation_scope_begin(&scope);
  struct timespec started, finished;
  clock_gettime(CLOCK_MONOTONIC, &started);
  ChimiValue *list = push_sequence(list_from_values(NULL, 0), length, 0);
  clock_gettime(CLOCK_MONOTONIC, &finished);
  allocation_scope_collect(&scope, list, NULL);
  /* Reading every element after collection proves marking kept all list nodes. */
  expect_sequence(list, length, 0);
  double elapsed = (double)(finished.tv_sec - started.tv_sec) * 1e9 +
                   (double)(finished.tv_nsec - started.tv_nsec);
  *allocated_per_element = (double)scope.total_allocated_bytes / length;
  *live_per_element = (double)scope.live_bytes / length;
  printf("append length=%u ns_per_append=%.1f allocated_bytes_per_element=%.1f "
         "live_bytes_per_element=%.1f\n",
         length, elapsed / length, *allocated_per_element, *live_per_element);
  allocation_scope_collect(&scope, NULL, NULL);
  CHECK(scope.live_bytes == 0, "dropped list left %zu live bytes", scope.live_bytes);
  allocation_scope_end(&scope);
}

static void test_accumulation_is_linear_and_reclaimed(void) {
  double small_allocated, small_live, large_allocated, large_live;
  measure_accumulation(10000, &small_allocated, &small_live);
  measure_accumulation(1000000, &large_allocated, &large_live);
  /* A flat array copied on every append allocates about 4 * length bytes per
     element (about 4 MB at one million). A persistent vector stays constant. */
  CHECK(large_allocated < 512, "%.1f bytes allocated per appended element", large_allocated);
  CHECK(large_allocated < small_allocated * 1.25, "per-element cost grew from %.1f to %.1f",
        small_allocated, large_allocated);
  CHECK(large_live < 128, "%.1f live bytes per retained element", large_live);
}

static void test_collection_keeps_shared_old_version(void) {
  AllocationScope scope;
  allocation_scope_begin(&scope);
  ChimiValue *old = push_sequence(list_from_values(NULL, 0), 5000, 0);
  ChimiValue *newer = push_sequence(old, 5000, 5000);
  size_t before = scope.live_bytes;
  allocation_scope_collect(&scope, old, NULL);
  CHECK(scope.live_bytes < before, "collecting the newer version freed nothing");
  expect_sequence(old, 5000, 0);
  (void)newer;
  allocation_scope_end(&scope);
}

int main(void) {
  test_push_and_get_across_trie_heights();
  test_old_versions_are_unchanged();
  test_bulk_construction_matches_push();
  test_accumulation_is_linear_and_reclaimed();
  test_collection_keeps_shared_old_version();
  puts("ok");
  return 0;
}
