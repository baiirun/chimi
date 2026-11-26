/* White-box test of the native runtime's persistent Map tree, which Set shares.
   It includes the runtime source so the static map primitives are reachable, and
   supplies an empty program so the runtime links without generated program data.
   Prints "ok" and exits nonzero on the first failure. */
#include "runtime.c"

const char *CHIMI_COMPUTATION = "map-runtime-test";
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
const ChimiConstantValue CHIMI_CONSTANT_VALUES[] = {{0}};
const uint32_t CHIMI_CONSTANT_VALUE_COUNT = 0;
const ChimiBinding CHIMI_BINDINGS[] = {{0}};
const uint32_t CHIMI_BINDING_COUNT = 0;
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

/* The AVL bound: a tree of `n` nodes is at most 1.45 log2(n + 2) high. Keeping
   only the keys on the leftmost (or rightmost) path and removing every other key
   leaves a path as long as the old height unless removal rebalances. Each
   order of removal reaches the rebalancing on one side of the descent. */
static void test_removals_that_keep_one_path_rebalance(bool leftmost, bool ascending) {
  enum { COUNT = 4096 };
  MapNode *map = NULL;
  for (int64_t key = 0; key < COUNT; ++key) map = map_put_node(map, integer_value(key), void_value());
  static bool on_path[COUNT];
  memset(on_path, 0, sizeof(on_path));
  size_t path_length = 0;
  for (const MapNode *node = map; node; node = leftmost ? node->left : node->right) {
    on_path[node->key->as.integer] = true;
    ++path_length;
  }
  for (int64_t step = 0; step < COUNT; ++step) {
    int64_t key = ascending ? step : COUNT - 1 - step;
    if (on_path[key]) continue;
    MapNode *removed = map_remove_node(map, integer_value(key));
    CHECK(removed != map, "key %" PRId64 " was held", key);
    map = removed;
  }
  CHECK(map_size(map) == (int64_t)path_length, "%" PRId64 " keys left, expected %zu", map_size(map),
        path_length);
  double bound = 1.45 * log2((double)path_length + 2);
  CHECK(map->height <= bound, "height %u for %zu keys", map->height, path_length);
  MapEntries entries = map_entries(map);
  int64_t previous = -1;
  for (size_t index = 0; index < entries.length; ++index) {
    int64_t key = entries.nodes[index]->key->as.integer;
    CHECK(key > previous && on_path[key], "entry %zu is %" PRId64, index, key);
    previous = key;
  }
  free(entries.nodes);
}

/* The AVL invariant at every node: children at most one apart in height, the
   stored height one more than the taller child, and keys in order. Returns the
   height. */
static uint16_t check_avl(const MapNode *node, int64_t low, int64_t high) {
  if (!node) return 0;
  int64_t key = node->key->as.integer;
  CHECK(key > low && key < high, "key %" PRId64 " out of order", key);
  uint16_t left = check_avl(node->left, low, key), right = check_avl(node->right, key, high);
  CHECK(left <= right + 1 && right <= left + 1, "heights %u and %u under key %" PRId64, left, right, key);
  CHECK(node->height == 1 + (left > right ? left : right), "height %u at key %" PRId64, node->height, key);
  return node->height;
}

/* Insertions and removals in a scrambled order reach every rotation, single and
   double, on both sides; the invariant holds after each change. */
static void test_scrambled_changes_keep_the_invariant(void) {
  enum { COUNT = 512 };
  MapNode *map = NULL;
  for (int64_t step = 0; step < COUNT; ++step) {
    map = map_put_node(map, integer_value((step * 197) % COUNT), void_value());
    check_avl(map, -1, COUNT);
  }
  for (int64_t step = 0; step < COUNT; step += 2) {
    map = map_remove_node(map, integer_value((step * 311) % COUNT));
    check_avl(map, -1, COUNT);
  }
  CHECK(map_size(map) == COUNT / 2, "%" PRId64 " keys left", map_size(map));
}

int main(void) {
  test_scrambled_changes_keep_the_invariant();
  for (int ascending = 0; ascending < 2; ++ascending) {
    test_removals_that_keep_one_path_rebalance(true, ascending);
    test_removals_that_keep_one_path_rebalance(false, ascending);
  }
  puts("ok");
  return 0;
}
