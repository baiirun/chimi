/* White-box heap scenarios of the supervised-process conformance lab
   (docs/specification/supervised-process-conformance.md, "Heap: copy-boundary
   safety"). It includes the runtime source so the static heap primitives are
   reachable, and supplies an empty program so the runtime links.

   usage: heap_boundary <scenario ID>   runs that scenario's checks; prints "ok"
   Exits 1 on the first failed check and 2 for an unknown scenario. */
#include "runtime.c"

const char *CHIMI_COMPUTATION = "process-lab-heap-boundary";
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

static RuntimeHeap *enter(RuntimeHeap *heap) {
  RuntimeHeap *previous = current_heap;
  current_heap = heap;
  return previous;
}

static ChimiValue *triple(ChimiValue *first, ChimiValue *second, ChimiValue *third) {
  ChimiValue *values[] = {first, second, third};
  return list_from_values(values, 3);
}

/* A Text of `size` copies of 'x', allocated in the current heap. */
static ChimiValue *long_text(uint32_t size) {
  char *raw = malloc((size_t)size + 1);
  CHECK(raw, "out of memory");
  memset(raw, 'x', size);
  raw[size] = '\0';
  ChimiValue *text = text_value(raw);
  free(raw);
  return text;
}

/* A list of the Ints 0..size-1, allocated in the current heap. */
static ChimiValue *counting_list(uint32_t size) {
  ChimiValue **values = malloc(sizeof(*values) * size);
  CHECK(values, "out of memory");
  for (uint32_t index = 0; index < size; ++index) values[index] = integer_value(index);
  ChimiValue *list = list_from_values(values, size);
  free(values);
  return list;
}

static int64_t list_sum(const ChimiValue *list) {
  int64_t sum = 0;
  for (uint32_t index = 0; index < list->as.list.length; ++index) {
    const ChimiValue *value = list_get(list, index);
    CHECK(value && value->kind == VALUE_INTEGER, "element %u is not an Int", index);
    sum += value->as.integer;
  }
  return sum;
}

/* What crossing the boundary cost: the copy's peak footprint above where it
   started, and the allocations the region pass marked. Neither may depend on
   the size of the graph behind the foreign pointer. */
typedef struct {
  size_t copy_peak_bytes;
  uint64_t pass_scanned;
} BoundaryWork;

/* Heap A holds a graph that points to a Text of `size` bytes and a list of
   `size` Ints, both in heap B. A fragment copy of the graph and a region pass
   over it must both report the foreign pointers and leave heap B's values
   alone. The Text is what a copy would duplicate if it followed a pointer, and
   the list's elements are what a pass would mark. */
static BoundaryWork cross_into_foreign_values(uint32_t size) {
  RuntimeHeap *heap_b = heap_create(), *heap_a = heap_create();
  RuntimeHeap *previous = enter(heap_b);
  ChimiValue *foreign_text = long_text(size);
  ChimiValue *foreign = counting_list(size);
  int64_t foreign_sum = (int64_t)size * (size - 1) / 2;
  enter(heap_a);
  BoundaryWork work = {0};

  ChimiValue *graph = triple(integer_value(1), foreign_text, foreign);
  void *roots[] = {graph};
  AllocationKind kinds[] = {ALLOCATION_VALUE};
  RuntimeFragment *fragment = (RuntimeFragment *)1;
  size_t fragments = heap_totals.live_fragments, footprint = heap_totals.footprint_bytes;
  heap_totals.peak_footprint_bytes = footprint;
  CHECK(fragment_copy(roots, kinds, 1, &fragment) == HEAP_FOREIGN_POINTER,
        "a fragment copy accepted a pointer into another heap");
  CHECK(fragment == NULL, "a failed copy returned a fragment");
  CHECK(heap_totals.live_fragments == fragments, "a failed copy kept its fragment");
  CHECK(heap_totals.footprint_bytes == footprint, "a failed copy kept %zu bytes",
        heap_totals.footprint_bytes - footprint);
  work.copy_peak_bytes = heap_totals.peak_footprint_bytes - footprint;
  CHECK(list_sum(foreign) == foreign_sum, "the copy changed the foreign list");
  CHECK(strlen(foreign_text->as.text) == size, "the copy changed the foreign Text");

  AllocationScope region;
  allocation_scope_begin(&region);
  ChimiValue *state = triple(integer_value(2), foreign_text, foreign);
  (void)triple(integer_value(3), integer_value(4), integer_value(5)); /* garbage */
  CHECK(allocation_scope_collect(&region, state, NULL) == HEAP_FOREIGN_POINTER,
        "a region pass accepted a pointer into another heap");
  work.pass_scanned = region.scanned_allocations;
  allocation_scope_end(&region, NULL, NULL);
  enter(previous);
  heap_destroy(heap_a);
  /* Under AddressSanitizer a pass that swept heap B's allocations fails here. */
  CHECK(list_sum(foreign) == foreign_sum, "the pass changed the foreign list");
  CHECK(strlen(foreign_text->as.text) == size, "the pass changed the foreign Text");
  heap_destroy(heap_b);
  return work;
}

/* CB06: a pointer into another heap is reported, not followed. */
static void cb06_foreign_pointer_not_followed(void) {
  BoundaryWork small = cross_into_foreign_values(1);
  BoundaryWork large = cross_into_foreign_values(4096);
  CHECK(large.copy_peak_bytes == small.copy_peak_bytes,
        "the copy followed a foreign pointer: peak %zu bytes for size 1, %zu for 4096",
        small.copy_peak_bytes, large.copy_peak_bytes);
  CHECK(large.pass_scanned == small.pass_scanned,
        "the pass followed a foreign pointer: %llu allocations for size 1, %llu for 4096",
        (unsigned long long)small.pass_scanned, (unsigned long long)large.pass_scanned);
}

int main(int argc, char **argv) {
  if (argc != 2) {
    fputs("usage: heap_boundary <scenario ID>\n", stderr);
    return 2;
  }
  if (strcmp(argv[1], "CB06") == 0) {
    cb06_foreign_pointer_not_followed();
  } else {
    fprintf(stderr, "heap_boundary: no scenario %s\n", argv[1]);
    return 2;
  }
  puts("ok");
  return 0;
}
