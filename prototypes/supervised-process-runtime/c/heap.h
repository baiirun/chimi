/* Per-owner heaps for supervised processes.
 *
 * Global invariants:
 * - Every heap value belongs to exactly one heap or one detached fragment.
 *   A heap value never points into another heap or fragment.
 * - Values are immutable, so a value never points to a value allocated after
 *   it. Collection relies on this instead of write barriers or stack maps.
 * - A heap is used by one fiber at a time, through chimi_heap_switch.
 *
 * Values allocate into the innermost open region of the current heap. A region
 * is opened at loop entry; everything allocated before it is outside it and is
 * never moved or freed by its collections. */
#ifndef CHIMI_PROCESS_HEAP_H
#define CHIMI_PROCESS_HEAP_H

#include <stddef.h>
#include <stdint.h>

typedef enum {
  CHIMI_VALUE_INT = 1,
  CHIMI_VALUE_TEXT = 2,
  /* Lists, records, and closure environments share one shape: an item array. */
  CHIMI_VALUE_ITEMS = 3,
} ChimiValueKind;

typedef struct ChimiValue ChimiValue;
struct ChimiValue {
  ChimiValueKind kind;
  uint32_t length;
  union {
    int64_t integer;
    char *text;
    ChimiValue **items;
  } as;
};

typedef struct ChimiHeap ChimiHeap;
typedef struct ChimiFragment ChimiFragment;

typedef enum {
  CHIMI_HEAP_OK = 0,
  /* A traversed pointer belongs to another heap or fragment. */
  CHIMI_HEAP_FOREIGN_POINTER = 1,
} ChimiHeapStatus;

typedef struct {
  size_t live_bytes;
  size_t peak_live_bytes;
  size_t copied_bytes;
  /* Values a pass traversed: the root plus every value its Cheney scan visited.
   * With copied_bytes this bounds a pass's work, not only its copying. */
  uint64_t scanned_values;
  uint64_t minor_collections;
  uint64_t full_collections;
  uint32_t open_regions;
} ChimiHeapStats;

/* Chunk accounting for the calling thread, so tests can prove nothing leaks.
 * Per thread because heaps are used by one scheduler thread and the test
 * harness runs tests on parallel threads. Empty chunks held in the thread's
 * reuse cache are not live. */
typedef struct {
  size_t live_chunks;
  size_t live_chunk_bytes;
} ChimiChunkStats;

/* Ensures: an empty heap with no open region, tagged with `owner` (nonzero).
 * The heap is not current until chimi_heap_switch selects it. */
ChimiHeap *chimi_heap_create(uint64_t owner);

/* Ensures: every chunk of the heap is freed in O(chunks), not O(values).
 * Must not: touch adopted fragments' former owners or any other heap. */
void chimi_heap_destroy(ChimiHeap *heap);

/* Ensures: `heap` (may be NULL) becomes current; returns the previous one.
 * The runtime calls this on every fiber switch. */
ChimiHeap *chimi_heap_switch(ChimiHeap *heap);

/* Requires: a current heap. Allocate into its innermost open region, or its
 * base area when no region is open. `items` must belong to the current heap. */
ChimiValue *chimi_value_int(int64_t value);
ChimiValue *chimi_value_text(const char *bytes, size_t length);
ChimiValue *chimi_value_items(ChimiValue *const *items, uint32_t length);

/* Loop entry. Ensures: a new innermost region; values allocated earlier are
 * outside it and stay valid and unmoved until it closes. */
void chimi_region_enter(void);

/* Pass boundary. Requires: `*state` is the only value allocated inside the
 * innermost region that the program can still reach.
 * Ensures: unreachable values in the region are reclaimed and `*state` is
 * updated if it moved. Cost is O(values allocated this pass + young survivors),
 * independent of state that already survived earlier passes, except for a full
 * collection of the region when its survivors have doubled since the last one. */
ChimiHeapStatus chimi_region_pass(ChimiValue **state);

/* Loop exit. Ensures: the innermost region's values become part of the enclosing
 * region (or base area) without copying, so the loop result stays valid. */
void chimi_region_exit(void);

/* Transfer boundary. Requires: a current heap that owns every value reachable
 * from `root`. Ensures: a detached fragment holding a copy of the graph, with
 * shared subvalues copied once, so its size is linear in distinct values.
 * Returns FOREIGN_POINTER and no fragment if the graph leaves the current heap. */
ChimiHeapStatus chimi_fragment_copy(const ChimiValue *root, ChimiFragment **out);

/* Ensures: the fragment's chunks join the current heap's innermost region in
 * O(1) per chunk without copying; returns the copied root. Consumes `fragment`. */
ChimiValue *chimi_fragment_adopt(ChimiFragment *fragment);

/* Discards an unadopted fragment, for example an unread mailbox message. */
void chimi_fragment_free(ChimiFragment *fragment);

/* Owner tag of the heap holding `value`; 0 for a value in a detached fragment. */
uint64_t chimi_value_owner(const ChimiValue *value);

ChimiHeapStats chimi_heap_stats(const ChimiHeap *heap);
ChimiChunkStats chimi_chunk_stats(void);

#endif
