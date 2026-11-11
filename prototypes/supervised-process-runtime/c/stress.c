/* Heap stress: many passes mixing the shapes the unit tests cover one at a time.
 *
 * Each outer pass opens a nested region whose values point at the enclosing
 * region's young values and at a value from before both regions, builds large
 * values (thousands of items) and text, and runs inner passes. It then copies
 * the inner result into a fragment, adopts it into the outer region, and keeps
 * it in the outer state across an outer pass. The heap decides between minor
 * and full passes on its own; the run must see both.
 *
 * Checked on every pass: values from before a region are never moved or
 * changed, an adopted copy has the same contents as its source, the adopted
 * copy keeps its contents after later passes, and text keeps its length. At the
 * end every chunk is returned.
 *
 * usage: stress [PASSES]   Prints one stats line and "ok", or the first failure. */
#include "heap.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

#define CHECK(condition, ...)                                                      \
  do {                                                                             \
    if (!(condition)) {                                                            \
      fprintf(stderr, "%s:%d: check failed: %s: ", __FILE__, __LINE__, #condition); \
      fprintf(stderr, __VA_ARGS__);                                                \
      fputc('\n', stderr);                                                         \
      exit(1);                                                                     \
    }                                                                              \
  } while (0)

static uint64_t rng_state = 0x9E3779B97F4A7C15ull;
static uint32_t below(uint32_t bound) {
  rng_state ^= rng_state << 13;
  rng_state ^= rng_state >> 7;
  rng_state ^= rng_state << 17;
  return (uint32_t)(rng_state % bound);
}

/* A digest of the graph under `root` that counts each distinct value once, so
 * it is linear in distinct values even for heavily shared graphs. */
enum { SEEN_SLOTS = 1u << 20 };
static const ChimiValue **seen;
static const ChimiValue **pending;
static int64_t digest(const ChimiValue *root) {
  if (!seen) {
    seen = calloc(SEEN_SLOTS, sizeof(*seen));
    pending = malloc(SEEN_SLOTS * sizeof(*pending));
  }
  memset(seen, 0, SEEN_SLOTS * sizeof(*seen));
  size_t depth = 0, distinct = 0;
  int64_t sum = 0, edges = 0;
  pending[depth++] = root;
  while (depth) {
    const ChimiValue *value = pending[--depth];
    size_t slot = (size_t)(((uintptr_t)value >> 3) * 0x9E3779B97F4A7C15ull >> 44);
    while (seen[slot] && seen[slot] != value) slot = (slot + 1) & (SEEN_SLOTS - 1);
    if (seen[slot]) continue;
    seen[slot] = value;
    CHECK(++distinct < SEEN_SLOTS / 2, "graph too large for the digest table");
    switch (value->kind) {
      case CHIMI_VALUE_INT: sum += value->as.integer; break;
      case CHIMI_VALUE_TEXT:
        CHECK(strlen(value->as.text) == value->length, "text length %u", value->length);
        sum += 1000 * (int64_t)value->length;
        break;
      case CHIMI_VALUE_ITEMS:
        edges += value->length;
        sum += 1;
        for (uint32_t index = 0; index < value->length; ++index) {
          CHECK(depth < SEEN_SLOTS, "digest stack overflow");
          pending[depth++] = value->as.items[index];
        }
        break;
      default: CHECK(0, "value kind %d", (int)value->kind);
    }
  }
  return sum * 1000003 + edges;
}

static ChimiValue *large_value(uint32_t length, ChimiValue *fill) {
  ChimiValue **items = malloc(sizeof(*items) * length);
  for (uint32_t index = 0; index < length; ++index) items[index] = fill;
  ChimiValue *value = chimi_value_items(items, length);
  free(items);
  return value;
}

int main(int argc, char **argv) {
  int passes = argc > 1 ? atoi(argv[1]) : 300;
  ChimiChunkStats before = chimi_chunk_stats();
  ChimiHeap *heap = chimi_heap_create(1);
  chimi_heap_switch(heap);

  ChimiValue *outer = chimi_value_int(7);
  chimi_region_enter();
  ChimiValue *outer_young = chimi_value_int(11);
  ChimiValue *state = chimi_value_items((ChimiValue *[]){outer, outer_young}, 2);

  for (int pass = 0; pass < passes; ++pass) {
    chimi_region_enter();
    ChimiValue *inner = chimi_value_items((ChimiValue *[]){outer_young, outer}, 2);
    for (int step = 0; step < 5; ++step) {
      ChimiValue *text = chimi_value_text("hello", 5);
      ChimiValue *large = large_value(3000 + below(3000), below(2) ? text : inner);
      inner = chimi_value_items((ChimiValue *[]){inner, large, large, text}, 4);
      CHECK(chimi_region_pass(&inner) == CHIMI_HEAP_OK, "inner pass %d.%d", pass, step);
    }
    chimi_region_exit();
    CHECK(outer->as.integer == 7 && outer_young->as.integer == 11,
          "pass %d: a value from before the inner region changed", pass);

    ChimiFragment *fragment;
    CHECK(chimi_fragment_copy(inner, &fragment) == CHIMI_HEAP_OK, "pass %d: copy", pass);
    ChimiValue *adopted = chimi_fragment_adopt(fragment);
    int64_t want = digest(inner);
    CHECK(digest(adopted) == want, "pass %d: adopted copy differs from its source", pass);
    CHECK(chimi_value_owner(adopted) == 1, "pass %d: adopted owner %llu", pass,
          (unsigned long long)chimi_value_owner(adopted));

    ChimiValue *previous = state;
    state = chimi_value_items((ChimiValue *[]){adopted, outer_young, below(4) ? previous : state, outer}, 4);
    if (below(3) == 0) state = chimi_value_items((ChimiValue *[]){adopted, outer_young, outer}, 3);
    CHECK(chimi_region_pass(&state) == CHIMI_HEAP_OK, "outer pass %d", pass);
    CHECK(state->as.items[state->length - 1] == outer, "pass %d: pre-region value moved", pass);
    outer_young = state->as.items[1];
    CHECK(outer_young->as.integer == 11, "pass %d: outer young value lost", pass);
    CHECK(digest(state->as.items[0]) == want, "pass %d: adopted value changed by a pass", pass);
  }

  ChimiHeapStats stats = chimi_heap_stats(heap);
  ChimiChunkStats during = chimi_chunk_stats();
  printf("passes=%d live=%zu peak=%zu copied=%zu minor=%llu full=%llu chunks=%zu chunk_bytes=%zu\n",
         passes, stats.live_bytes, stats.peak_live_bytes, stats.copied_bytes,
         (unsigned long long)stats.minor_collections, (unsigned long long)stats.full_collections,
         during.live_chunks, during.live_chunk_bytes);
  if (passes >= 10) {
    CHECK(stats.minor_collections > 0 && stats.full_collections > 0,
          "expected both minor and full passes, got minor=%llu full=%llu",
          (unsigned long long)stats.minor_collections, (unsigned long long)stats.full_collections);
  }
  /* The outer state holds one adopted inner result; live memory must stay near
   * that, not grow with the number of passes. */
  CHECK(stats.live_bytes < 16u * 1024 * 1024, "live bytes %zu grew with passes", stats.live_bytes);

  chimi_region_exit();
  chimi_heap_destroy(heap);
  ChimiChunkStats after = chimi_chunk_stats();
  CHECK(after.live_chunks == before.live_chunks && after.live_chunk_bytes == before.live_chunk_bytes,
        "chunks leaked: %zu chunks / %zu bytes before, %zu / %zu after", before.live_chunks,
        before.live_chunk_bytes, after.live_chunks, after.live_chunk_bytes);
  puts("ok");
  return 0;
}
