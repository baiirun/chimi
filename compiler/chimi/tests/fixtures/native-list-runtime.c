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
const ChimiConstantValue CHIMI_CONSTANT_VALUES[] = {{0}};
const uint32_t CHIMI_CONSTANT_VALUE_COUNT = 0;
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
  /* A full pass, because a minor one never revisits the list once it survived. */
  allocation_scope_sweep(&scope, NULL, NULL, true);
  CHECK(scope.live_bytes == 0, "dropped list left %zu live bytes", scope.live_bytes);
  allocation_scope_end(&scope, NULL, NULL);
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
  allocation_scope_end(&scope, NULL, NULL);
}

/* Checks that `list` holds values[from], values[from + 1], ... for `length` elements. */
static void expect_window(const ChimiValue *list, uint32_t from, uint32_t length) {
  CHECK(list_length(list) == length, "window length %u, expected %u", list_length(list), length);
  for (uint32_t index = 0; index < length; ++index)
    CHECK(element(list, index) == (int64_t)(from + index), "window index %u", index);
  CHECK(list_get(list, length) == NULL, "read past the end of a window");
}

static void test_slices_are_windows_over_the_original(void) {
  uint32_t length = 33900;
  ChimiValue *list = push_sequence(list_from_values(NULL, 0), length, 0);
  static const uint32_t bounds[][2] = {{0, 0}, {0, 1}, {5, 5}, {0, 32}, {31, 33}, {32, 64},
                                       {1000, 1100}, {1, 33900}, {0, 33899}, {33867, 33900}};
  for (uint32_t which = 0; which < sizeof(bounds) / sizeof(bounds[0]); ++which) {
    uint32_t from = bounds[which][0], to = bounds[which][1];
    ChimiValue *window = list_slice(list, from, to);
    expect_window(window, from, to - from);
    /* A window of a window composes offsets. */
    if (to - from >= 2) expect_window(list_slice(window, 1, to - from - 1), from + 1, to - from - 2);
  }
  expect_sequence(list, length, 0);
}

static void test_append_to_a_prefix_leaves_the_original_unchanged(void) {
  /* `[..init, last]` then appending to `init`: the slot after the window
     still holds `last` in the original. */
  static const uint32_t lengths[] = {1, 2, 32, 33, 64, 65, 1057, 33825};
  for (uint32_t which = 0; which < sizeof(lengths) / sizeof(lengths[0]); ++which) {
    uint32_t length = lengths[which];
    ChimiValue *list = push_sequence(list_from_values(NULL, 0), length, 0);
    ChimiValue *init = list_slice(list, 0, length - 1);
    ChimiValue *left = list_push(init, integer_value(-1));
    ChimiValue *right = push_sequence(init, 70, -1000);
    expect_sequence(list, length, 0);
    expect_window(init, 0, length - 1);
    CHECK(list_length(left) == length && element(left, length - 1) == -1, "left successor");
    CHECK(list_length(right) == length + 69 && element(right, length - 1) == -1000 &&
          element(right, length + 68) == -931, "right successor");
    /* Appending to a suffix window extends the shared vector. */
    ChimiValue *rest = list_slice(list, 1, length);
    ChimiValue *extended = list_push(rest, integer_value((int64_t)length));
    expect_window(extended, 1, length);
    expect_sequence(list, length, 0);
  }
}

/* Pushing onto a prefix window whose end lies inside the trie, not the tail,
   writes a path copy: the shared vector and the window are untouched. */
static void test_append_to_a_prefix_ending_in_the_trie_leaves_the_original_unchanged(void) {
  static const uint32_t ends[] = {31, 32, 33, 1000, 1024, 1025, 32768};
  for (uint32_t which = 0; which < sizeof(ends) / sizeof(ends[0]); ++which) {
    /* `end + 64` keeps the window above a quarter of its vector, so it shares that
       vector, and puts `end` before the 32..64-element tail. */
    uint32_t end = ends[which], length = end + 64;
    ChimiValue *list = push_sequence(list_from_values(NULL, 0), length, 0);
    ChimiValue *prefix = list_slice(list, 0, end);
    CHECK(end < vector_tail_offset(length), "end %u lies in the trie", end);
    ChimiValue *extended = push_sequence(prefix, 40, -1000);
    expect_sequence(list, length, 0);
    expect_window(prefix, 0, end);
    CHECK(list_length(extended) == end + 40, "extended length");
    for (uint32_t index = 0; index < end; ++index)
      CHECK(element(extended, index) == index, "extended keeps index %u", index);
    for (uint32_t offset = 0; offset < 40; ++offset)
      CHECK(element(extended, end + offset) == -1000 + (int64_t)offset, "pushed element %u", offset);
  }
}

static void test_slice_rejects_a_bad_range(void) {
  ChimiValue *list = push_sequence(list_from_values(NULL, 0), 10, 0);
  CHECK(list_slice(list, 3, 2) == NULL, "reversed range");
  CHECK(list_slice(list, 0, 11) == NULL, "range past the end");
}

/* A window shorter than a quarter of its vector copies itself, so it does not
   keep the large vector alive. */
static void test_a_small_window_does_not_retain_its_source(void) {
  AllocationScope scope;
  allocation_scope_begin(&scope);
  ChimiValue *list = push_sequence(list_from_values(NULL, 0), 100000, 0);
  ChimiValue *small = list_slice(list, 50000, 50100);
  allocation_scope_collect(&scope, small, NULL);
  expect_window(small, 50000, 100);
  /* The 100 elements and their copy stay live (about 6 KB); sharing would keep all
     100,000 elements, about 6 MB. */
  CHECK(scope.live_bytes < 16384, "a 100-element window retains %zu bytes", scope.live_bytes);
  allocation_scope_end(&scope, NULL, NULL);
}

/* Dropping the head one element at a time, as `[first, ..rest]` recursion does,
   allocates one list header per step plus the retention copies, which total fewer
   than length / 3 elements. */
static void test_drop_front_is_constant_cost(void) {
  uint32_t length = 200000;
  ChimiValue *list = push_sequence(list_from_values(NULL, 0), length, 0);
  AllocationScope scope;
  allocation_scope_begin(&scope);
  ChimiValue *rest = list;
  for (uint32_t index = 0; index < length; ++index) {
    CHECK(element(rest, 0) == index, "head at step %u", index);
    rest = list_slice(rest, 1, list_length(rest));
  }
  CHECK(list_length(rest) == 0, "drained");
  double per_step = (double)scope.total_allocated_bytes / length;
  printf("drop_front length=%u allocated_bytes_per_step=%.1f\n", length, per_step);
  /* One header per step, plus copies of under length / 3 slots of one pointer each,
     with node headers. A copy per step would cost about length / 2 slots per step. */
  CHECK(per_step <= sizeof(ChimiValue) + 4 * sizeof(void *), "%.1f bytes per drop", per_step);
  allocation_scope_end(&scope, NULL, NULL);
}

static void test_collection_keeps_a_window_readable(void) {
  AllocationScope scope;
  allocation_scope_begin(&scope);
  ChimiValue *list = push_sequence(list_from_values(NULL, 0), 5000, 0);
  ChimiValue *window = list_slice(list, 1234, 4321);
  allocation_scope_collect(&scope, window, NULL);
  expect_window(window, 1234, 4321 - 1234);
  allocation_scope_end(&scope, NULL, NULL);
}

int main(void) {
  test_push_and_get_across_trie_heights();
  test_old_versions_are_unchanged();
  test_bulk_construction_matches_push();
  test_accumulation_is_linear_and_reclaimed();
  test_collection_keeps_shared_old_version();
  test_slices_are_windows_over_the_original();
  test_append_to_a_prefix_leaves_the_original_unchanged();
  test_append_to_a_prefix_ending_in_the_trie_leaves_the_original_unchanged();
  test_slice_rejects_a_bad_range();
  test_a_small_window_does_not_retain_its_source();
  test_drop_front_is_constant_cost();
  test_collection_keeps_a_window_readable();
  puts("ok");
  return 0;
}
