/* Differential fuzz of the native runtime's persistent List against a model.
   It includes the runtime source so the static list primitives are reachable,
   and supplies an empty program so the runtime links without generated
   program data.

   A pool of live versions is mutated at random: push (short and long runs),
   slice, and rebuild from values. Each new version overwrites a random slot,
   so older versions become garbage while their siblings still share
   structure with them. Every model version is a plain int64 array. Collections
   run periodically with every live version rooted, then every version is
   checked element by element, so a collection that frees shared structure, a
   push that writes into a vector another window still sees, or a slice that
   reads the wrong offset all turn into a mismatch (or a sanitizer report).

   The pool also enters and leaves a nested scope at random, as a loop inside a
   loop does. Slicing a version built in the enclosing scope from inside the
   nested one compacts it in the enclosing scope's area, and versions built on
   both sides of the boundary keep sharing structure across it. Versions that
   survive a collection are promoted, so later minor collections skip them while
   newer versions share their nodes, and periodic full collections sweep them.

   usage: native-list-fuzz SEED OPERATIONS
   Prints "fuzz ok ..." and exits 0, or prints the first mismatch and exits 1. */
#include "runtime.c"

const char *CHIMI_COMPUTATION = "list-fuzz-test";
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

enum { VERSIONS = 64, MAX_LENGTH = 200000, COLLECT_EVERY = 199, SPOT_CHECK_EVERY = 50,
       NESTED_TOGGLE_ONE_IN = 400 };

typedef struct {
  ChimiValue *list;
  int64_t *items;
  uint32_t length;
} Version;

static long step;
static uint64_t seed;

#define FAIL(...)                                                         \
  do {                                                                    \
    fprintf(stderr, "seed %llu step %ld: ", (unsigned long long)seed, step); \
    fprintf(stderr, __VA_ARGS__);                                         \
    fputc('\n', stderr);                                                  \
    exit(1);                                                              \
  } while (0)

static void check(const Version *version, int slot) {
  if (list_length(version->list) != version->length)
    FAIL("version %d: length %u, model %u", slot, list_length(version->list), version->length);
  for (uint32_t index = 0; index < version->length; ++index) {
    ChimiValue *value = list_get(version->list, index);
    if (!value || value->kind != VALUE_INTEGER || value->as.integer != version->items[index])
      FAIL("version %d: index %u of %u differs from the model", slot, index, version->length);
  }
  if (list_get(version->list, version->length) != NULL)
    FAIL("version %d: index %u past the end is not empty", slot, version->length);
}

static uint64_t rng_state;
static uint64_t next_random(void) {
  rng_state ^= rng_state << 13;
  rng_state ^= rng_state >> 7;
  rng_state ^= rng_state << 17;
  return rng_state;
}
static uint32_t below(uint32_t bound) { return bound ? (uint32_t)(next_random() % bound) : 0; }

/* A model array of `capacity` items whose first `length` are copied from `items`. */
static int64_t *copy_items(const int64_t *items, uint32_t length, uint32_t capacity) {
  int64_t *copy = malloc(((size_t)capacity + 1) * sizeof(*copy));
  if (length) memcpy(copy, items, (size_t)length * sizeof(*copy));
  return copy;
}

static Version push_run(const Version *source, int64_t *counter) {
  uint32_t count = 1 + below(below(4) == 0 ? 2000 : 40);
  Version next = {source->list, copy_items(source->items, source->length, source->length + count),
                  source->length + count};
  for (uint32_t offset = 0; offset < count; ++offset) {
    int64_t item = (*counter)++;
    next.items[source->length + offset] = item;
    next.list = list_push(next.list, integer_value(item));
  }
  return next;
}

static Version slice(const Version *source) {
  uint32_t from = below(source->length + 1);
  uint32_t to = from + below(source->length - from + 1);
  return (Version){list_slice(source->list, from, to), copy_items(source->items + from, to - from, to - from),
                   to - from};
}

static Version rebuild(const Version *source) {
  ChimiValue **values = malloc(((size_t)source->length + 1) * sizeof(*values));
  for (uint32_t index = 0; index < source->length; ++index)
    values[index] = list_get(source->list, index);
  Version next = {list_from_values(values, source->length),
                  copy_items(source->items, source->length, source->length), source->length};
  free(values);
  return next;
}

static ChimiValue *all_versions(const Version *versions) {
  ChimiValue *roots[VERSIONS];
  for (int slot = 0; slot < VERSIONS; ++slot) roots[slot] = versions[slot].list;
  return list_from_values(roots, VERSIONS);
}

static void check_all(const Version *versions) {
  for (int slot = 0; slot < VERSIONS; ++slot) check(&versions[slot], slot);
}

/* A pass boundary may move the versions, so each one is read back from the root. */
static void reload_versions(Version *versions, const ChimiValue *root) {
  for (int slot = 0; slot < VERSIONS; ++slot) versions[slot].list = list_get(root, slot);
}

static void collect_and_check_all(AllocationScope *scope, Version *versions) {
  ChimiValue *root = all_versions(versions);
  allocation_scope_collect(scope, &root, NULL);
  reload_versions(versions, root);
  check_all(versions);
}

/* Ends the nested scope as a loop does: a full sweep rooted at every version, after
   which the survivors join the enclosing scope. */
static void leave_nested(AllocationScope *nested, Version *versions) {
  ChimiControl *control = success(all_versions(versions));
  allocation_scope_end(nested, NULL, &control);
  reload_versions(versions, control->value);
  check_all(versions);
}

int main(int argc, char **argv) {
  if (argc != 3) {
    fputs("usage: native-list-fuzz SEED OPERATIONS\n", stderr);
    return 2;
  }
  seed = strtoull(argv[1], NULL, 10);
  long operations = strtol(argv[2], NULL, 10);
  rng_state = seed ? seed : 88172645463325252ull;

  Version versions[VERSIONS];
  for (int slot = 0; slot < VERSIONS; ++slot)
    versions[slot] = (Version){list_from_values(NULL, 0), NULL, 0};
  AllocationScope scope, nested;
  allocation_scope_begin(&scope);
  bool in_nested = false;
  int64_t counter = 0;
  long pushes = 0, slices = 0, rebuilds = 0, collections = 0, nestings = 0;
  uint32_t longest = 0;

  for (step = 0; step < operations; ++step) {
    if (below(NESTED_TOGGLE_ONE_IN) == 0) {
      if (in_nested) leave_nested(&nested, versions);
      else allocation_scope_begin(&nested), ++nestings;
      in_nested = !in_nested;
    }
    int source = (int)below(VERSIONS), target = (int)below(VERSIONS);
    uint32_t choice = below(100);
    Version next;
    if (choice < 70) next = push_run(&versions[source], &counter), ++pushes;
    else if (choice < 95) next = slice(&versions[source]), ++slices;
    else next = rebuild(&versions[source]), ++rebuilds;
    if (next.length > MAX_LENGTH) {
      free(next.items);
      continue;
    }
    if (next.length > longest) longest = next.length;
    free(versions[target].items);
    versions[target] = next;
    if (step % COLLECT_EVERY == 0)
      collect_and_check_all(in_nested ? &nested : &scope, versions), ++collections;
    if (step % SPOT_CHECK_EVERY == 0) check(&versions[target], target);
  }
  if (in_nested) leave_nested(&nested, versions);
  collect_and_check_all(&scope, versions);
  ++collections;

  printf("fuzz ok seed=%llu operations=%ld pushes=%ld slices=%ld rebuilds=%ld collections=%ld "
         "nestings=%ld longest=%u\n",
         (unsigned long long)seed, operations, pushes, slices, rebuilds, collections, nestings,
         longest);
  return 0;
}
