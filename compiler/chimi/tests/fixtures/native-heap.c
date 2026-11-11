/* White-box test of the native runtime's per-owner heaps, regions, and fragments.
   It reproduces the scenarios of the supervised-process prototype's heap tests
   (prototypes/supervised-process-runtime/tests/heap.rs and c/stress.c) against
   runtime.c, plus the port requirements of
   docs/architecture/supervised-process-runtime.md that Chimi programs cannot
   observe directly. It includes the runtime source so the static heap primitives
   are reachable, and supplies an empty program so the runtime links.

   usage: native-heap            run every check; prints "ok"
          native-heap measure    also print per-message copy and adopt timings */
#include "runtime.c"

const char *CHIMI_COMPUTATION = "heap-test";
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
const ChimiConstantValue CHIMI_CONSTANT_VALUES[] = {{0}};
const uint32_t CHIMI_CONSTANT_VALUE_COUNT = 0;

#define CHECK(condition, ...)                                                       \
  do {                                                                              \
    if (!(condition)) {                                                             \
      fprintf(stderr, "%s:%d: check failed: %s: ", __FILE__, __LINE__, #condition); \
      fprintf(stderr, __VA_ARGS__);                                                 \
      fputc('\n', stderr);                                                          \
      exit(1);                                                                      \
    }                                                                               \
  } while (0)

static ChimiValue *int_value(int64_t raw) { return integer_value(raw); }
static ChimiValue *items(ChimiValue *const *values, uint32_t length) {
  return list_from_values(values, length);
}
static ChimiValue *pair(ChimiValue *first, ChimiValue *second) {
  ChimiValue *values[] = {first, second};
  return items(values, 2);
}
static ChimiValue *item(const ChimiValue *list, uint32_t index) { return list_get(list, index); }
static int64_t integer(const ChimiValue *value) {
  CHECK(value && value->kind == VALUE_INTEGER, "expected an Int");
  return value->as.integer;
}
static RuntimeHeap *enter(RuntimeHeap *heap) {
  RuntimeHeap *previous = current_heap;
  current_heap = heap;
  return previous;
}
static RuntimeHeap *owner_of(const void *value) { return allocation_header(value)->area->heap; }
/* Starts a new peak, so a check sees this test's memory, not an earlier test's. */
static void reset_peak(void) { heap_totals.peak_footprint_bytes = heap_totals.footprint_bytes; }

static RuntimeFragment *copy_value(ChimiValue *value) {
  void *roots[] = {value};
  AllocationKind kinds[] = {ALLOCATION_VALUE};
  RuntimeFragment *fragment = NULL;
  CHECK(fragment_copy(roots, kinds, 1, &fragment) == HEAP_OK, "copy of an owned graph failed");
  return fragment;
}
static ChimiValue *adopt_value(RuntimeFragment *fragment) {
  void *roots[FRAGMENT_ROOTS];
  fragment_adopt(fragment, roots);
  return roots[0];
}

typedef struct { uint64_t scanned; size_t promoted; uint64_t full; } PassWork;

/* Builds a state of `size` distinct Ints, then runs `passes` passes that each
   allocate garbage and keep the state. Returns the region's collection work. */
static PassWork work_for_state(uint32_t size, uint32_t passes) {
  RuntimeHeap *heap = heap_create();
  RuntimeHeap *previous = enter(heap);
  AllocationScope region;
  allocation_scope_begin(&region);
  ChimiValue **values = malloc(sizeof(*values) * size);
  for (uint32_t index = 0; index < size; ++index) values[index] = int_value(index);
  ChimiValue *state = items(values, size);
  free(values);
  for (uint32_t pass = 0; pass < passes; ++pass) {
    (void)pair(int_value(pass), int_value(pass + 1));
    CHECK(allocation_scope_collect(&region, state, NULL) == HEAP_OK, "pass %u", pass);
  }
  int64_t sum = 0;
  for (uint32_t index = 0; index < size; ++index) sum += integer(item(state, index));
  CHECK(sum == (int64_t)size * (size - 1) / 2, "state changed: sum %lld", (long long)sum);
  PassWork work = {region.scanned_allocations, region.promoted_bytes, region.full_collections};
  allocation_scope_end(&region, NULL, NULL);
  enter(previous);
  heap_destroy(heap);
  return work;
}

/* Prototype: pass_cost_does_not_grow_with_state_that_already_survived. */
static void test_pass_cost_does_not_grow_with_state_that_already_survived(void) {
  uint32_t size = 100000;
  PassWork small = work_for_state(10, 10000);
  PassWork large = work_for_state(size, 10000);
  /* The large state's allocations: one per Int, its trie nodes, and the list. */
  uint64_t state_allocations = size + size / 16 + 2;
  size_t state_bytes = (size_t)size * (sizeof(ChimiValue) + sizeof(void *)) + 64 * 1024;
  /* The state is marked and promoted a bounded number of times (when it is
     promoted, and by at most two full passes), not once per pass. */
  CHECK(large.scanned < small.scanned + 3 * state_allocations,
        "scanned small %llu, large %llu", (unsigned long long)small.scanned,
        (unsigned long long)large.scanned);
  CHECK(large.promoted < small.promoted + 3 * state_bytes, "promoted small %zu, large %zu",
        small.promoted, large.promoted);
}

/* Prototype: a_million_passes_keep_peak_memory_flat, plus the port's full-pass
   trigger: a small state replaced on every pass must not run a full pass on every
   other pass. */
static void test_a_million_passes_keep_peak_memory_flat(void) {
  reset_peak();
  RuntimeHeap *heap = heap_create();
  RuntimeHeap *previous = enter(heap);
  AllocationScope region;
  allocation_scope_begin(&region);
  ChimiValue *state = int_value(0);
  size_t peak_after_warmup = 0, allocations_after_warmup = 0;
  uint32_t passes = 1000000;
  for (uint32_t pass = 0; pass < passes; ++pass) {
    int64_t next = integer(state) + 1;
    (void)pair(int_value(pass), int_value(pass));
    state = int_value(next);
    CHECK(allocation_scope_collect(&region, state, NULL) == HEAP_OK, "pass %u", pass);
    if (pass == 10000) {
      peak_after_warmup = heap_totals.peak_footprint_bytes;
      allocations_after_warmup = region.survivors.count;
    }
  }
  CHECK(integer(state) == passes, "state %lld", (long long)integer(state));
  CHECK(heap_totals.peak_footprint_bytes == peak_after_warmup,
        "peak footprint grew from %zu to %zu bytes", peak_after_warmup,
        heap_totals.peak_footprint_bytes);
  CHECK(region.survivors.count <= 2 * allocations_after_warmup + 1,
        "survivors grew from %zu to %zu", allocations_after_warmup, region.survivors.count);
  CHECK(region.full_collections > 0 && region.full_collections <= passes / 64,
        "%llu full passes in %u passes", (unsigned long long)region.full_collections, passes);
  allocation_scope_end(&region, NULL, NULL);
  enter(previous);
  heap_destroy(heap);
}

/* Prototype: values_allocated_before_the_loop_survive_its_collections. */
static void test_values_allocated_before_the_loop_survive_its_collections(void) {
  RuntimeHeap *heap = heap_create();
  RuntimeHeap *previous = enter(heap);
  ChimiValue *outer = pair(int_value(7), int_value(8));
  AllocationScope region;
  allocation_scope_begin(&region);
  ChimiValue *state = int_value(0);
  for (int pass = 0; pass < 1000; ++pass) {
    (void)items((ChimiValue *[]){int_value(1)}, 1);
    CHECK(allocation_scope_collect(&region, state, NULL) == HEAP_OK, "pass %d", pass);
  }
  allocation_scope_end(&region, state, NULL);
  CHECK(integer(item(outer, 0)) == 7 && integer(item(outer, 1)) == 8, "outer value changed");
  enter(previous);
  heap_destroy(heap);
}

/* Prototype: inner_passes_never_move_values_of_the_enclosing_region_and_exit_hands_
   survivors_out. Under AddressSanitizer a freed outer value or a survivor dropped at
   exit fails as a use after free. */
static void test_inner_passes_keep_outer_values_and_exit_hands_survivors_out(void) {
  HeapTotals before = heap_totals;
  RuntimeHeap *heap = heap_create();
  RuntimeHeap *previous = enter(heap);
  AllocationScope outer_region, inner_region;
  allocation_scope_begin(&outer_region);
  ChimiValue *outer_value = pair(int_value(7), int_value(8));
  ChimiValue *outer_state = int_value(0);
  allocation_scope_begin(&inner_region);
  ChimiValue *inner_state = pair(outer_value, int_value(-1));
  for (int pass = 0; pass < 1000; ++pass) {
    (void)items((ChimiValue *[]){int_value(pass)}, 1);
    inner_state = pair(outer_value, int_value(pass));
    CHECK(allocation_scope_collect(&inner_region, inner_state, NULL) == HEAP_OK, "inner %d", pass);
    CHECK(item(inner_state, 0) == outer_value, "inner pass %d lost the outer value", pass);
  }
  allocation_scope_end(&inner_region, inner_state, NULL);
  CHECK(integer(item(outer_value, 0)) == 7 && integer(item(outer_value, 1)) == 8, "outer changed");
  CHECK(allocation_header(inner_state)->area == &outer_region.young,
        "inner survivors did not join the enclosing region's young generation");

  /* The inner survivors now belong to the outer region, whose passes must keep them
     while the state reaches them and reuse the memory of the garbage. */
  outer_state = pair(inner_state, outer_state);
  reset_peak();
  size_t peak_at_warmup = 0;
  for (int pass = 0; pass < 2000; ++pass) {
    char text[4097];
    memset(text, 'x', 4096);
    text[4096] = '\0';
    (void)text_value(text);
    (void)pair(int_value(pass), int_value(pass));
    CHECK(allocation_scope_collect(&outer_region, outer_state, NULL) == HEAP_OK, "outer %d", pass);
    if (pass == 100) peak_at_warmup = heap_totals.peak_footprint_bytes;
  }
  CHECK(heap_totals.peak_footprint_bytes == peak_at_warmup, "outer garbage was not reclaimed");
  ChimiValue *inner_result = item(outer_state, 0);
  ChimiValue *kept = item(inner_result, 0);
  CHECK(integer(item(kept, 0)) == 7 && integer(item(kept, 1)) == 8, "kept outer value changed");
  CHECK(integer(item(inner_result, 1)) == 999, "inner result changed");
  allocation_scope_end(&outer_region, NULL, NULL);
  CHECK(heap->innermost == NULL, "regions left open");
  enter(previous);
  heap_destroy(heap);
  CHECK(heap_totals.live_allocations == before.live_allocations &&
          heap_totals.footprint_bytes == before.footprint_bytes,
        "a region exit lost or leaked allocations");
}

/* Prototype: adopted_fragments_join_the_innermost_region_and_its_passes_reclaim_them. */
static void test_adopted_fragments_join_the_innermost_region_and_are_reclaimed(void) {
  RuntimeHeap *sender = heap_create(), *receiver = heap_create();
  RuntimeHeap *previous = enter(sender);
  ChimiValue *source = pair(int_value(41), int_value(1));
  enter(receiver);
  AllocationScope region;
  allocation_scope_begin(&region);
  reset_peak();
  size_t peak_at_warmup = 0;
  for (int round = 0; round < 10000; ++round) {
    enter(sender);
    RuntimeFragment *fragment = copy_value(source);
    enter(receiver);
    ChimiValue *adopted = adopt_value(fragment);
    CHECK(owner_of(adopted) == receiver, "an adopted value does not belong to the receiver");
    CHECK(allocation_header(adopted)->area->scope == &region,
          "an adopted fragment did not join the innermost region");
    /* Keep the newest message as the state; every older one is garbage. */
    ChimiValue *state = pair(adopted, int_value(round));
    CHECK(allocation_scope_collect(&region, state, NULL) == HEAP_OK, "round %d", round);
    ChimiValue *message = item(state, 0);
    CHECK(integer(item(message, 0)) + integer(item(message, 1)) == 42, "round %d message", round);
    if (round == 3000) peak_at_warmup = heap_totals.peak_footprint_bytes;
  }
  CHECK(heap_totals.peak_footprint_bytes == peak_at_warmup, "adopted fragments were not reclaimed");
  CHECK(heap_totals.live_fragments == 0, "%zu fragments in transit", heap_totals.live_fragments);
  allocation_scope_end(&region, NULL, NULL);
  enter(previous);
  heap_destroy(receiver);
  heap_destroy(sender);
}

/* Prototype: copying_preserves_sharing. */
static void test_copying_preserves_sharing(void) {
  RuntimeHeap *heap = heap_create(), *receiver = heap_create();
  RuntimeHeap *previous = enter(heap);
  /* Each level points twice to the level below: 2^20 paths, 21 distinct values. A
     copy that follows paths instead of values makes a million copies and fails the
     size check below instead of running for hours (the prototype used 2^40). */
  ChimiValue *value = int_value(1);
  for (int level = 0; level < 20; ++level) value = pair(value, value);
  RuntimeFragment *fragment = copy_value(value);
  enter(receiver);
  ChimiValue *copied = adopt_value(fragment);
  CHECK(item(copied, 0) == item(copied, 1), "a shared value was copied twice");
  CHECK(owner_of(copied) == receiver, "the copy does not belong to the receiver");
  size_t bytes = 0;
  for (AllocationArea *area = receiver->adopted; area; area = area->next) bytes += area->bytes;
  CHECK(bytes < 64 * 1024, "the copy holds %zu bytes", bytes);
  enter(previous);
  heap_destroy(receiver);
  heap_destroy(heap);
}

/* Prototype: a_pointer_into_another_heap_is_reported_not_followed. */
static void test_a_pointer_into_another_heap_is_reported_not_followed(void) {
  RuntimeHeap *other = heap_create(), *heap = heap_create();
  RuntimeHeap *previous = enter(other);
  ChimiValue *foreign = int_value(5);
  enter(heap);
  ChimiValue *mixed = pair(int_value(1), foreign);
  void *roots[] = {mixed};
  AllocationKind kinds[] = {ALLOCATION_VALUE};
  RuntimeFragment *fragment = (RuntimeFragment *)1;
  size_t fragments = heap_totals.live_fragments;
  CHECK(fragment_copy(roots, kinds, 1, &fragment) == HEAP_FOREIGN_POINTER && fragment == NULL,
        "a copy followed a pointer into another heap");
  CHECK(heap_totals.live_fragments == fragments, "a failed copy kept its fragment");
  AllocationScope region;
  allocation_scope_begin(&region);
  ChimiValue *state = items((ChimiValue *[]){foreign}, 1);
  CHECK(allocation_scope_collect(&region, state, NULL) == HEAP_FOREIGN_POINTER,
        "a pass followed a pointer into another heap");
  allocation_scope_end(&region, NULL, NULL);
  CHECK(integer(foreign) == 5, "the foreign value was freed");
  enter(previous);
  heap_destroy(heap);
  heap_destroy(other);
}

/* Prototype: destroying_heaps_and_unadopted_fragments_frees_every_chunk. */
static void test_destroying_heaps_and_unadopted_fragments_frees_everything(void) {
  HeapTotals before = heap_totals;
  RuntimeHeap *heap = heap_create();
  RuntimeHeap *previous = enter(heap);
  ChimiValue *value = pair(int_value(1), int_value(2));
  RuntimeFragment *unread = copy_value(value);
  RuntimeFragment *adopted_outside = copy_value(value);
  (void)adopt_value(adopted_outside);
  enter(previous);
  heap_destroy(heap);
  fragment_free(unread);
  CHECK(heap_totals.live_allocations == before.live_allocations, "%zu allocations leaked",
        heap_totals.live_allocations - before.live_allocations);
  CHECK(heap_totals.footprint_bytes == before.footprint_bytes, "%zu bytes leaked",
        heap_totals.footprint_bytes - before.footprint_bytes);
  CHECK(heap_totals.live_fragments == before.live_fragments, "fragments leaked");
  CHECK(heap_totals.fragment_footprint_bytes == before.fragment_footprint_bytes,
        "fragment bytes leaked");
  CHECK(heap_totals.live_heaps == before.live_heaps, "heaps leaked");
}

/* Port requirement (Task.result adoption): copying a fragment into a heap leaves it
   intact, so every call gets its own copy, including one inside a pass that a later
   pass reclaims. */
static void test_a_fragment_copies_into_a_heap_without_being_consumed(void) {
  RuntimeHeap *child = heap_create(), *caller = heap_create();
  RuntimeHeap *previous = enter(child);
  ChimiValue *shared = text_value("result");
  ChimiControl *control = success(pair(shared, shared));
  void *roots[] = {control};
  AllocationKind kinds[] = {ALLOCATION_CONTROL};
  RuntimeFragment *result = NULL;
  CHECK(fragment_copy(roots, kinds, 1, &result) == HEAP_OK, "copy");
  heap_destroy(child);
  enter(caller);
  AllocationScope region;
  allocation_scope_begin(&region);
  ChimiValue *state = int_value(0);
  for (int call = 0; call < 1000; ++call) {
    void *copied[FRAGMENT_ROOTS];
    fragment_copy_into_current(result, copied);
    ChimiControl *seen = copied[0];
    CHECK(owner_of(seen) == caller, "the copy is not the caller's");
    CHECK(seen->kind == CONTROL_SUCCESS && strcmp(item(seen->value, 0)->as.text, "result") == 0,
          "call %d read a damaged result", call);
    CHECK(item(seen->value, 0) == item(seen->value, 1), "the copy lost sharing");
    state = call % 2 ? seen->value : int_value(call);
    CHECK(allocation_scope_collect(&region, state, NULL) == HEAP_OK, "pass %d", call);
  }
  allocation_scope_end(&region, NULL, NULL);
  CHECK(heap_totals.live_fragments >= 1, "copying consumed the fragment");
  fragment_free(result);
  enter(previous);
  heap_destroy(caller);
}

/* Port requirement (list windows): a copy rebuilds a compact vector from the window,
   so sending a short slice of a long list copies only the slice. */
static void test_a_copy_of_a_window_copies_only_the_window(void) {
  RuntimeHeap *heap = heap_create();
  RuntimeHeap *previous = enter(heap);
  ChimiValue **values = malloc(sizeof(*values) * 100000);
  for (uint32_t index = 0; index < 100000; ++index) values[index] = int_value(index);
  ChimiValue *list = items(values, 100000);
  free(values);
  /* A window of a quarter of the list shares the whole vector. */
  ChimiValue *window = list_slice(list, 40000, 65000);
  CHECK(window->as.list.vector.root == list->as.list.vector.root, "the window should share");
  ChimiValue *tiny = list_value(list->as.list.vector, 50000, 10);
  size_t before = heap_totals.fragment_footprint_bytes;
  RuntimeFragment *fragment = copy_value(tiny);
  size_t bytes = heap_totals.fragment_footprint_bytes - before;
  CHECK(bytes < 4096, "a 10-element window copied %zu bytes", bytes);
  ChimiValue *copied = fragment->roots[0];
  for (uint32_t index = 0; index < 10; ++index)
    CHECK(integer(item(copied, index)) == 50000 + index, "index %u", index);
  fragment_free(fragment);
  enter(previous);
  heap_destroy(heap);
}

/* ts-df67fb: queued small messages cost bytes proportional to their size, not a
   64 KiB chunk each, and adoption does not walk the message. */
static void test_queued_small_messages_cost_bytes_proportional_to_their_size(void) {
  RuntimeHeap *sender = heap_create(), *receiver = heap_create();
  RuntimeHeap *previous = enter(sender);
  enum { QUEUED = 1000 };
  RuntimeFragment *queue[QUEUED];
  size_t before = heap_totals.fragment_footprint_bytes;
  for (int index = 0; index < QUEUED; ++index) queue[index] = copy_value(int_value(index));
  size_t queued = heap_totals.fragment_footprint_bytes - before;
  CHECK(queued < 256 * 1024, "1000 queued Int messages hold %zu bytes", queued);
  enter(receiver);
  AllocationScope region;
  allocation_scope_begin(&region);
  int64_t sum = 0;
  for (int index = 0; index < QUEUED; ++index) {
    AllocationHeader *first = queue[index]->area.allocations;
    ChimiValue *message = adopt_value(queue[index]);
    CHECK(allocation_header(message)->area == &((RuntimeFragment *)first->area)->area,
          "adoption moved the message's allocations");
    sum += integer(message);
  }
  CHECK(sum == (int64_t)QUEUED * (QUEUED - 1) / 2, "sum %lld", (long long)sum);
  CHECK(heap_totals.fragment_footprint_bytes == before, "adopted messages still count as queued");
  allocation_scope_end(&region, NULL, NULL);
  enter(previous);
  heap_destroy(receiver);
  heap_destroy(sender);
}

/* Prototype: c/stress.c. Regions, adoption, and minor and full passes compose: values
   from before a region never change, an adopted copy equals its source and keeps its
   contents across later passes, and nothing leaks. */
static uint64_t rng_state = UINT64_C(0x9E3779B97F4A7C15);
static uint32_t below(uint32_t bound) {
  rng_state ^= rng_state << 13;
  rng_state ^= rng_state >> 7;
  rng_state ^= rng_state << 17;
  return (uint32_t)(rng_state % bound);
}
static uint64_t digest(const ChimiValue *value, uint32_t depth) {
  CHECK(depth < 64, "digest too deep");
  switch (value->kind) {
    case VALUE_INTEGER: return (uint64_t)value->as.integer;
    case VALUE_TEXT: return 1000 * (uint64_t)strlen(value->as.text);
    case VALUE_LIST: {
      uint64_t sum = 1, length = list_length(value);
      /* Large lists repeat one value; sample their ends to stay linear. */
      if (length > 8) return sum + length * digest(item(value, 0), depth + 1) +
                             digest(item(value, (uint32_t)length - 1), depth + 1);
      for (uint32_t index = 0; index < length; ++index)
        sum = sum * 31 + digest(item(value, index), depth + 1);
      return sum;
    }
    default: CHECK(0, "kind %d", (int)value->kind);
  }
  return 0;
}
static ChimiValue *large_value(uint32_t length, ChimiValue *fill) {
  ChimiValue **values = malloc(sizeof(*values) * length);
  for (uint32_t index = 0; index < length; ++index) values[index] = fill;
  ChimiValue *value = items(values, length);
  free(values);
  return value;
}
static void test_regions_fragments_and_passes_compose(void) {
  HeapTotals before = heap_totals;
  RuntimeHeap *heap = heap_create(), *peer = heap_create();
  RuntimeHeap *previous = enter(heap);
  ChimiValue *outer = int_value(7);
  AllocationScope outer_region;
  allocation_scope_begin(&outer_region);
  ChimiValue *outer_young = int_value(11);
  ChimiValue *state = pair(outer, outer_young);
  for (int pass = 0; pass < 300; ++pass) {
    AllocationScope inner_region;
    allocation_scope_begin(&inner_region);
    ChimiValue *inner = pair(outer_young, outer);
    for (int step = 0; step < 5; ++step) {
      ChimiValue *text = text_value("hello");
      ChimiValue *large = large_value(3000 + below(3000), below(2) ? text : inner);
      inner = items((ChimiValue *[]){inner, large, large, text}, 4);
      CHECK(allocation_scope_collect(&inner_region, inner, NULL) == HEAP_OK, "inner %d.%d", pass, step);
    }
    allocation_scope_end(&inner_region, inner, NULL);
    CHECK(integer(outer) == 7 && integer(outer_young) == 11, "pass %d: an older value changed", pass);
    uint64_t want = digest(inner, 0);
    /* Round-trip through the peer heap, as a request and its reply would. */
    RuntimeFragment *request = copy_value(inner);
    enter(peer);
    ChimiValue *at_peer = adopt_value(request);
    RuntimeFragment *reply = copy_value(at_peer);
    enter(heap);
    ChimiValue *adopted = adopt_value(reply);
    CHECK(digest(adopted, 0) == want, "pass %d: the adopted copy differs", pass);
    ChimiValue *previous_state = state;
    state = items((ChimiValue *[]){adopted, outer_young, below(4) ? previous_state : state, outer}, 4);
    if (below(3) == 0) state = items((ChimiValue *[]){adopted, outer_young, outer}, 3);
    CHECK(allocation_scope_collect(&outer_region, state, NULL) == HEAP_OK, "outer pass %d", pass);
    CHECK(item(state, list_length(state) - 1) == outer, "pass %d: a pre-region value moved", pass);
    outer_young = item(state, 1);
    CHECK(integer(outer_young) == 11, "pass %d: outer young value lost", pass);
    CHECK(digest(item(state, 0), 0) == want, "pass %d: a pass changed the adopted value", pass);
  }
  CHECK(outer_region.collections - outer_region.full_collections > 0 &&
          outer_region.full_collections > 0,
        "expected minor and full passes, got %llu passes with %llu full",
        (unsigned long long)outer_region.collections,
        (unsigned long long)outer_region.full_collections);
  CHECK(outer_region.live_bytes < 16u * 1024 * 1024, "live bytes %zu grew with passes",
        outer_region.live_bytes);
  allocation_scope_end(&outer_region, NULL, NULL);
  enter(previous);
  heap_destroy(peer);
  heap_destroy(heap);
  CHECK(heap_totals.live_allocations == before.live_allocations &&
          heap_totals.footprint_bytes == before.footprint_bytes &&
          heap_totals.live_fragments == before.live_fragments,
        "allocations leaked");
}

static double seconds_now(void) {
  struct timespec now;
  clock_gettime(CLOCK_MONOTONIC, &now);
  return (double)now.tv_sec + (double)now.tv_nsec * 1e-9;
}

/* Prints the footprint of 1,000 queued Int messages, and the cost of one Int message:
   the sender's copy and pass, adoption, and one receiver pass. These are the steps
   of the prototype's 138 ns "copy cost per message" row for an Int. Its 316 ns
   figure adds a fiber switch. Not a check: timings depend on the machine. */
static void measure_messages(void) {
  reset_peak();
  RuntimeHeap *sender = heap_create(), *receiver = heap_create();
  RuntimeHeap *previous = enter(sender);
  RuntimeFragment *queue[1000];
  size_t before = heap_totals.fragment_footprint_bytes;
  for (int index = 0; index < 1000; ++index) queue[index] = copy_value(int_value(index));
  printf("measure queued_int_messages=1000 fragment_footprint_bytes=%zu\n",
         heap_totals.fragment_footprint_bytes - before);
  for (int index = 0; index < 1000; ++index) fragment_free(queue[index]);
  /* Both sides run in a loop region, as a sending and a receiving loop would. */
  AllocationScope sending;
  allocation_scope_begin(&sending);
  enter(receiver);
  AllocationScope region;
  allocation_scope_begin(&region);
  ChimiValue *state = int_value(0);
  enum { MESSAGES = 1000000 };
  double started = seconds_now();
  for (int index = 0; index < MESSAGES; ++index) {
    enter(sender);
    RuntimeFragment *fragment = copy_value(int_value(index));
    allocation_scope_collect(&sending, NULL, NULL);
    enter(receiver);
    ChimiValue *message = adopt_value(fragment);
    state = int_value(integer(state) + integer(message));
    allocation_scope_collect(&region, state, NULL);
  }
  double elapsed = seconds_now() - started;
  printf("measure int_message_ns=%.1f peak_footprint_bytes=%zu full_passes=%llu\n",
         elapsed * 1e9 / MESSAGES, heap_totals.peak_footprint_bytes,
         (unsigned long long)region.full_collections);
  allocation_scope_end(&region, NULL, NULL);
  enter(sender);
  allocation_scope_end(&sending, NULL, NULL);
  enter(previous);
  heap_destroy(receiver);
  heap_destroy(sender);
}

int main(int argc, char **argv) {
  test_pass_cost_does_not_grow_with_state_that_already_survived();
  test_a_million_passes_keep_peak_memory_flat();
  test_values_allocated_before_the_loop_survive_its_collections();
  test_inner_passes_keep_outer_values_and_exit_hands_survivors_out();
  test_adopted_fragments_join_the_innermost_region_and_are_reclaimed();
  test_copying_preserves_sharing();
  test_a_pointer_into_another_heap_is_reported_not_followed();
  test_destroying_heaps_and_unadopted_fragments_frees_everything();
  test_a_fragment_copies_into_a_heap_without_being_consumed();
  test_a_copy_of_a_window_copies_only_the_window();
  test_queued_small_messages_cost_bytes_proportional_to_their_size();
  test_regions_fragments_and_passes_compose();
  if (argc > 1 && strcmp(argv[1], "measure") == 0) measure_messages();
  puts("ok");
  return 0;
}
