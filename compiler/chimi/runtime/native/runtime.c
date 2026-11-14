/* Chimi native runtime ABI version 18. */
#include "runtime.h"
#include "chimi_fiber.h"

#include <inttypes.h>
#include <math.h>
#include <stdarg.h>
#include <errno.h>
#include <limits.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

/* The stack of every scheduler fiber: the root task, child tasks, and incarnations. */
#define CHIMI_FIBER_STACK_BYTES (1024u * 1024u)

/* Process programs build this runtime with CHIMI_PROCESS_KERNEL=1 (see invoke_process). */
#ifndef CHIMI_PROCESS_KERNEL
#define CHIMI_PROCESS_KERNEL 0
#endif

// Ordinary binaries keep the native runtime but intentionally omit the fiber
// archive. Concurrency builds force-load the archive, whose strong definitions
// replace these unreachable fallbacks. A white-box harness that includes this
// file and answers requests itself defines CHIMI_HARNESS_FIBER_REQUEST.
#ifndef CHIMI_HARNESS_FIBER_REQUEST
__attribute__((weak)) ChimiFiberCommand chimi_fiber_request(
  void *request, const ChimiRootFrame *roots) {
  (void)request;
  (void)roots;
  abort();
}
#endif

__attribute__((weak)) ChimiSchedulerOutcome chimi_scheduler_run(
  const ChimiSchedulerConfig *config) {
  (void)config;
  return (ChimiSchedulerOutcome){.state = CHIMI_FIBER_DEFECTED};
}

typedef enum { VALUE_VOID, VALUE_INTEGER, VALUE_FLOAT, VALUE_TEXT, VALUE_BOOLEAN, VALUE_LIST, VALUE_RECORD,
               VALUE_CONSTRUCTED, VALUE_CALLABLE, VALUE_MAP, VALUE_JSON_NUMBER, VALUE_TASK,
               VALUE_CHANNEL_SENDER, VALUE_CHANNEL_RECEIVER,
               VALUE_CHANNEL_COMPLETION, VALUE_TUPLE, VALUE_RANGE,
               /* A `Duration`: nanoseconds in `as.integer`. */
               VALUE_DURATION,
               /* Process values (std/process, std/supervisor). Each is plain data that
                  copies as-is: identities the scheduler resolves, never pointers. */
               VALUE_PROCESS_ADDRESS, VALUE_PROCESS_INBOX, VALUE_SUPERVISOR_RUNNING,
               VALUE_MANAGED_PROCESS, VALUE_MANAGED_SUPERVISOR } ValueKind;
/* CALL_CONSTRUCTOR is a constructor with fields named as a value; calling it constructs. */
typedef enum { CALL_DECLARED, CALL_CLOSURE, CALL_DEPENDENCY, CALL_HOST, CALL_INTRINSIC,
               CALL_CONSTRUCTOR } CallableKind;
typedef enum { HOST_PROCESS_UTF8_LINES, HOST_WRITE_LINE, HOST_WRITE_ERROR } HostFunction;
/* CONTROL_BREAK carries the target loop's state symbol in `node` and its value in `value`.
   Verified programs keep it inside one function, so it never reaches a call or root. */
typedef enum { CONTROL_SUCCESS, CONTROL_FAILURE, CONTROL_DEFECT, CONTROL_CANCEL, CONTROL_BREAK } ControlKind;

typedef struct ChimiValue ChimiValue;
typedef struct ChimiTaskContext ChimiTaskContext;
typedef struct { const char *name; ChimiValue *value; } RuntimeField;
typedef struct { uint64_t channel; uint64_t owner; uint64_t token; } RuntimeCapability;
typedef struct MapNode MapNode;
typedef struct ListNode ListNode;

/* One node of a persistent vector: a 32-way radix trie whose leaves (height 0)
   hold values and whose branches hold child nodes. Trie leaves are always full;
   a list's tail leaf holds its last 1..32 values. Nodes are immutable once a
   list value refers to them, so every version shares all untouched nodes. */
struct ListNode {
  uint32_t height;
  uint32_t count;
  void *slots[];
};

/* A persistent vector of `length` values. `root` is NULL until it outgrows one
   leaf; `tail` is NULL only when empty. */
typedef struct { ListNode *root; ListNode *tail; uint32_t length; } ListVector;

/* Keys are any equatable value, kept in the internal structural order. */
struct MapNode {
  ChimiValue *key;
  ChimiValue *value;
  MapNode *left;
  MapNode *right;
  uint16_t height;
};

typedef struct ChimiCallable {
  CallableKind kind;
  uint32_t symbol;
  uint32_t body;
  uint32_t parameters_offset;
  uint32_t parameters_length;
  ChimiEnvironment *environment;
  struct ChimiCallable *inner;
  uint32_t requirement;
  uint32_t interface_symbol;
  uint32_t method;
  const char *name;
  uint32_t intrinsic;
  HostFunction host;
} ChimiCallable;

struct ChimiValue {
  ValueKind kind;
  union {
    int64_t integer;
    double floating; /* always finite */
    const char *text;
    bool boolean;
    /* A List is the window [start, start + length) of a shared vector, so
       slicing is O(1) amortized; see list_slice for when a window copies. */
    struct { ListVector vector; uint32_t start; uint32_t length; } list;
    /* A record, or a tuple (VALUE_TUPLE), whose fields have no names. */
    struct { RuntimeField *fields; uint32_t length; } record;
    struct { uint32_t constructor; const char *type_name; const char *variant_name;
             RuntimeField *fields; uint32_t length; } constructed;
    /* Normalized half-open bounds: `..=` already added one to the end. */
    struct { int64_t start; int64_t end; } range;
    ChimiCallable *callable;
    MapNode *map;
    const char *json_number;
    uint64_t task;
    RuntimeCapability capability;
    /* CHIMI_ADDRESS_NAMED with a name id, or CHIMI_ADDRESS_DIRECT with an
       incarnation id. */
    struct { uint32_t kind; uint64_t target; } address;
    /* Running: `supervisor`. A managed handle: its supervisor and membership. */
    struct { uint64_t supervisor; uint64_t membership; } managed;
  } as;
};

struct ChimiEnvironment {
  ChimiValue **locals;
  ChimiValue **requirements;
};

struct ChimiControl {
  ControlKind kind;
  ChimiValue *value;
  const char *message;
  int64_t node;
};

/* The root task runs `entry` in the main heap. A child task gets its own heap and
   adopts `captures`, a fragment holding its callable value and the requirements it
   captured; the context is malloc'd and the child frees it. */
struct ChimiTaskContext {
  ChimiRuntime *runtime;
  ChimiEnvironment *environment;
  ChimiNodeFn entry;
  struct RuntimeFragment *captures;
  uint64_t span;
};

typedef enum {
  ALLOCATION_BYTES,
  ALLOCATION_VALUE,
  ALLOCATION_CONTROL,
  ALLOCATION_ENVIRONMENT,
  ALLOCATION_VALUE_POINTERS,
  ALLOCATION_FIELDS,
  ALLOCATION_CALLABLE,
  ALLOCATION_MAP_NODE,
  ALLOCATION_LIST_NODE,
  /* An environment that nodes write after creating it; no value may point to one. */
  ALLOCATION_FRAME_ENVIRONMENT,
  ALLOCATION_KIND_COUNT,
} AllocationKind;

struct ChimiRuntime {
  const char *execution_id;
  bool full_trace;
  /* `--clock=virtual`: the scheduler's deterministic time source. */
  bool virtual_clock;
  /* The parked tasks of a virtual-time deadlock, from the scheduler's report. */
  char *deadlock_waiters;
  /* The defect message of each task the scheduler defected for a channel
     capability violation, from its report. */
  struct CapabilityViolationNote *capability_violations;
  bool cancel;
  bool silent;
  FILE *telemetry;
  FILE *outcome;
  uint32_t max_line_bytes;
  uint64_t next_sequence;
  uint64_t next_span;
  size_t scoped_total_allocated_bytes;
  size_t scoped_allocated_bytes_by_kind[ALLOCATION_KIND_COUNT];
  size_t scoped_peak_live_bytes;
  size_t scoped_retained_bytes;
  uint64_t scoped_collections;
  uint64_t scoped_full_collections;
  uint64_t scoped_scanned_allocations;
  /* Telemetry events built for the telemetry file: the runtime's own and the
     scheduler's. 0 under --trace=off (lab scenario OB01). */
  uint64_t trace_events_constructed;
};

/* Per-owner heaps (docs/architecture/supervised-process-runtime.md).

   Every allocation carries a header that names its area. An area belongs to one
   owner heap, to one detached fragment, or to the static constants:

   - A heap's base area holds what its owner allocates outside every region.
     Nothing reclaims it before the heap is destroyed.
   - A region (AllocationScope) is opened by each `loop` and by
     Stream.process_utf8_lines. It has a young area for what the current pass
     allocates, a survivor area for what earlier passes kept, and the areas of
     fragments adopted during the pass.
   - A fragment area holds a copy made for a transfer between owners. Adoption
     hands the whole area to the receiver's innermost region in O(1).
   - The static area holds module constants, which every owner shares.

   Invariants:
   - A heap value never points into another heap or into a fragment, so a heap
     can be destroyed, and a fragment freed, without looking at anything else.
   - Values never change after construction (the one exception, list_slice's
     compaction, allocates into the area of the value it updates). So a value
     never points to a value allocated after it, and a survivor never points to
     a young value. A minor pass therefore marks from the state through young
     allocations only, and survivors are walked only by a full pass.
   - Environments that nodes write after creating them are frame environments
     (ALLOCATION_FRAME_ENVIRONMENT). No value points to one; marking and copying
     check the kind of every allocation they reach, so a violation aborts.

   Collection does not move anything, so pointers held in C frames stay valid.
   The runtime switches the current heap around every scheduler request
   (scheduler_request), because a fiber resumes only by returning from one. */
typedef struct AllocationHeader AllocationHeader;
typedef struct AllocationArea AllocationArea;
typedef struct AllocationScope AllocationScope;
typedef struct RuntimeHeap RuntimeHeap;

typedef enum { AREA_YOUNG, AREA_SURVIVOR, AREA_BASE, AREA_FRAGMENT, AREA_STATIC } AreaRole;

struct AllocationHeader {
  AllocationHeader *next;
  AllocationArea *area;
  size_t bytes;
  uint64_t mark_epoch;
  AllocationKind kind;
};

struct AllocationArea {
  AreaRole role;
  /* The owner heap; NULL for a fragment and for static data. */
  RuntimeHeap *heap;
  /* The region of a young or survivor area; NULL otherwise. */
  AllocationScope *scope;
  AllocationHeader *allocations;
  size_t bytes;
  size_t count;
  /* The next adopted area of the same region (young areas only). */
  AllocationArea *next;
};

struct AllocationScope {
  AllocationScope *parent;
  RuntimeHeap *heap;
  /* `young` is the area this region allocates into; `adopted` lists the areas of
     fragments received since the last pass, which join the young generation. */
  AllocationArea young;
  AllocationArea *adopted;
  AllocationArea survivors;
  /* Survivor bytes right after the last full pass, or after the first pass that
     promoted anything. A full pass runs once survivors reach twice this and grew
     by at least REGION_FULL_MIN_GROWTH, so a small state that is replaced on every
     pass does not run a full pass on every other pass. */
  size_t full_baseline;
  bool has_baseline;
  size_t live_bytes;
  size_t peak_live_bytes;
  size_t total_allocated_bytes;
  size_t allocated_bytes_by_kind[ALLOCATION_KIND_COUNT];
  uint64_t collections;
  uint64_t full_collections;
  /* Allocations marked by passes, roots included, and bytes promoted to survivors:
     together they bound a pass's work, which must not grow with old survivors. */
  uint64_t scanned_allocations;
  size_t promoted_bytes;
};

struct RuntimeHeap {
  uint64_t owner;
  /* Payload bytes of the allocations this heap owns now, and their peak: the memory
     report's `process_heap_peak_value_bytes` is the largest peak of an incarnation. */
  size_t live_bytes;
  size_t peak_live_bytes;
  AllocationScope *innermost;
  AllocationArea base;
  /* Areas of fragments adopted outside every region; they belong to the base. */
  AllocationArea *adopted;
};

typedef enum { HEAP_OK = 0, HEAP_FOREIGN_POINTER = 1 } HeapStatus;

/* Process-wide accounting, so tests can prove that memory stays flat and that
   destroying heaps and fragments returns every allocation. Footprint bytes count
   headers and fragment records as well as payloads; static constants are excluded. */
typedef struct {
  size_t live_allocations;
  size_t footprint_bytes;
  size_t peak_footprint_bytes;
  size_t live_fragments;
  size_t fragment_footprint_bytes;
  size_t peak_fragment_footprint_bytes;
  size_t live_heaps;
  /* Freed small blocks kept for reuse (see small_block_take); not live. */
  size_t cached_bytes;
} HeapTotals;

enum { REGION_FULL_MIN_GROWTH = 4 * 1024 };

static RuntimeHeap main_heap = {.owner = 1, .base = {.role = AREA_BASE, .heap = &main_heap}};
static RuntimeHeap *current_heap = &main_heap;
static uint64_t next_heap_owner = 2;
static AllocationArea static_area = {.role = AREA_STATIC};
/* When set, allocate() uses this area instead of the current heap's innermost one:
   for static constants, fragment copies, and list_slice's in-place compaction. */
static AllocationArea *allocation_target;
static HeapTotals heap_totals;
/* Bytes allocated outside every region, static constants included. Nothing reclaims
   them before exit. */
static size_t unscoped_allocated_bytes;
static uint64_t next_mark_epoch = 1;

static void exhausted_memory(void) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }

/* Freed blocks of up to SMALL_BLOCK_LIMIT bytes are kept on per-size free lists and
   reused, because a message or a pass allocates and frees many small blocks and the
   system allocator dominated their cost. Each list keeps at most SMALL_BLOCK_CACHE
   blocks, so the cache is bounded. Sanitizer builds skip it so that a use after free
   is still reported. */
enum { SMALL_BLOCK_GRAIN = 16, SMALL_BLOCK_LIMIT = 256, SMALL_BLOCK_CACHE = 4096 };
#if defined(__SANITIZE_ADDRESS__)
#define CHIMI_SMALL_BLOCK_CACHE 0
#elif defined(__has_feature)
#if __has_feature(address_sanitizer)
#define CHIMI_SMALL_BLOCK_CACHE 0
#endif
#endif
#ifndef CHIMI_SMALL_BLOCK_CACHE
#define CHIMI_SMALL_BLOCK_CACHE 1
#endif
typedef struct SmallBlock { struct SmallBlock *next; } SmallBlock;
static SmallBlock *small_blocks[SMALL_BLOCK_LIMIT / SMALL_BLOCK_GRAIN];
static uint32_t small_block_counts[SMALL_BLOCK_LIMIT / SMALL_BLOCK_GRAIN];

/* A zeroed block of at least `bytes` bytes, to be returned with small_block_give. */
static void *small_block_take(size_t bytes) {
  size_t rounded = (bytes + SMALL_BLOCK_GRAIN - 1) / SMALL_BLOCK_GRAIN * SMALL_BLOCK_GRAIN;
  if (CHIMI_SMALL_BLOCK_CACHE && rounded <= SMALL_BLOCK_LIMIT) {
    size_t class = rounded / SMALL_BLOCK_GRAIN - 1;
    SmallBlock *block = small_blocks[class];
    if (block) {
      small_blocks[class] = block->next;
      --small_block_counts[class];
      heap_totals.cached_bytes -= rounded;
      memset(block, 0, rounded);
      return block;
    }
  }
  void *block = calloc(1, rounded);
  if (!block) exhausted_memory();
  return block;
}

static void small_block_give(void *pointer, size_t bytes) {
  size_t rounded = (bytes + SMALL_BLOCK_GRAIN - 1) / SMALL_BLOCK_GRAIN * SMALL_BLOCK_GRAIN;
  if (CHIMI_SMALL_BLOCK_CACHE && rounded <= SMALL_BLOCK_LIMIT) {
    size_t class = rounded / SMALL_BLOCK_GRAIN - 1;
    if (small_block_counts[class] < SMALL_BLOCK_CACHE) {
      SmallBlock *block = pointer;
      block->next = small_blocks[class];
      small_blocks[class] = block;
      ++small_block_counts[class];
      heap_totals.cached_bytes += rounded;
      return;
    }
  }
  free(pointer);
}

static AllocationArea *current_area(void) {
  if (allocation_target) return allocation_target;
  return current_heap->innermost ? &current_heap->innermost->young : &current_heap->base;
}

static void footprint_add(size_t bytes) {
  heap_totals.footprint_bytes += bytes;
  if (heap_totals.footprint_bytes > heap_totals.peak_footprint_bytes)
    heap_totals.peak_footprint_bytes = heap_totals.footprint_bytes;
}

static void fragment_footprint_add(size_t bytes) {
  heap_totals.fragment_footprint_bytes += bytes;
  if (heap_totals.fragment_footprint_bytes > heap_totals.peak_fragment_footprint_bytes)
    heap_totals.peak_fragment_footprint_bytes = heap_totals.fragment_footprint_bytes;
}

static void scope_add_live(AllocationScope *scope, size_t bytes) {
  scope->live_bytes += bytes;
  if (scope->live_bytes > scope->peak_live_bytes) scope->peak_live_bytes = scope->live_bytes;
}

static void heap_add_live(RuntimeHeap *heap, size_t bytes) {
  heap->live_bytes += bytes;
  if (heap->live_bytes > heap->peak_live_bytes) heap->peak_live_bytes = heap->live_bytes;
}

static void area_link(AllocationArea *area, AllocationHeader *header) {
  header->area = area;
  header->next = area->allocations;
  area->allocations = header;
  area->bytes += header->bytes;
  ++area->count;
}

static void *allocate(size_t count, size_t size) {
  if (count == 0) count = 1;
  if (size != 0 && count > SIZE_MAX / size) {
    fputs("Chimi runtime allocation overflow\n", stderr); exit(70);
  }
  size_t bytes = count * size;
  if (bytes > SIZE_MAX - sizeof(AllocationHeader) - SMALL_BLOCK_GRAIN) {
    fputs("Chimi runtime allocation overflow\n", stderr); exit(70);
  }
  AllocationHeader *header = small_block_take(sizeof(*header) + bytes);
  header->bytes = bytes;
  AllocationArea *area = current_area();
  area_link(area, header);
  if (area->heap) heap_add_live(area->heap, bytes);
  if (area->scope) {
    AllocationScope *scope = area->scope;
    scope_add_live(scope, bytes);
    scope->total_allocated_bytes += bytes;
    scope->allocated_bytes_by_kind[ALLOCATION_BYTES] += bytes;
  } else if (area->role != AREA_FRAGMENT) {
    unscoped_allocated_bytes += bytes;
  }
  if (area->role != AREA_STATIC) {
    ++heap_totals.live_allocations;
    footprint_add(sizeof(*header) + bytes);
    if (area->role == AREA_FRAGMENT) fragment_footprint_add(sizeof(*header) + bytes);
  }
  return header + 1;
}

static AllocationHeader *allocation_header(const void *value) {
  return value ? (AllocationHeader *)value - 1 : NULL;
}

static void allocation_kind(void *value, AllocationKind kind) {
  AllocationHeader *header = allocation_header(value);
  AllocationScope *scope = header->area->scope;
  if (scope && header->kind != kind) {
    scope->allocated_bytes_by_kind[header->kind] -= header->bytes;
    scope->allocated_bytes_by_kind[kind] += header->bytes;
  }
  header->kind = kind;
}

/* Channel release (docs/architecture/supervised-process-runtime.md, "Channel
   release"). Each live Sender, Receiver, or Completion value counts as one
   reference to its channel, in whichever heap or fragment it is allocated, so a
   copy in transit (a message, a task's captures or result) counts while the
   scheduler holds it. channel_capability_value creates endpoint values,
   copy_forward is the only thing that copies them, and release_allocation the only
   thing that frees them; those three keep the counts. At zero no value can name
   the channel again, so its id waits in `released_channels` for the next
   scheduler request of any task (flush_channel_releases). A count can reach zero
   inside a collection or a scheduler release hook, where no request may be issued.
   The table is an open-addressing map keyed by channel id; ids are never 0. */
typedef struct { uint64_t channel; size_t endpoints; } ChannelEndpoints;
static ChannelEndpoints *channel_endpoints;
static size_t channel_endpoint_capacity;
static size_t channel_endpoint_count;
static uint64_t *released_channels;
static size_t released_channel_count;
static size_t released_channel_capacity;

static bool is_channel_endpoint(const ChimiValue *value) {
  return value->kind == VALUE_CHANNEL_SENDER || value->kind == VALUE_CHANNEL_RECEIVER ||
         value->kind == VALUE_CHANNEL_COMPLETION;
}

static size_t channel_slot(uint64_t channel, size_t capacity) {
  return (size_t)((channel * UINT64_C(0x9E3779B97F4A7C15)) >> 32) & (capacity - 1);
}

static void channel_endpoints_grow(void) {
  size_t capacity = channel_endpoint_capacity ? channel_endpoint_capacity * 2 : 64;
  ChannelEndpoints *table = calloc(capacity, sizeof(*table));
  if (!table) exhausted_memory();
  for (size_t index = 0; index < channel_endpoint_capacity; ++index) {
    if (!channel_endpoints[index].channel) continue;
    size_t slot = channel_slot(channel_endpoints[index].channel, capacity);
    while (table[slot].channel) slot = (slot + 1) & (capacity - 1);
    table[slot] = channel_endpoints[index];
  }
  free(channel_endpoints);
  channel_endpoints = table;
  channel_endpoint_capacity = capacity;
}

static void channel_endpoint_retain(uint64_t channel) {
  if (2 * (channel_endpoint_count + 1) > channel_endpoint_capacity) channel_endpoints_grow();
  size_t mask = channel_endpoint_capacity - 1;
  size_t slot = channel_slot(channel, channel_endpoint_capacity);
  while (channel_endpoints[slot].channel && channel_endpoints[slot].channel != channel)
    slot = (slot + 1) & mask;
  if (!channel_endpoints[slot].channel) {
    channel_endpoints[slot].channel = channel;
    ++channel_endpoint_count;
  }
  ++channel_endpoints[slot].endpoints;
}

static void channel_endpoint_drop(uint64_t channel) {
  size_t mask = channel_endpoint_capacity - 1;
  size_t slot = channel_slot(channel, channel_endpoint_capacity);
  while (channel_endpoints[slot].channel != channel) {
    /* An endpoint of an earlier execution's scheduler (benchmark repeats). */
    if (!channel_endpoints[slot].channel) return;
    slot = (slot + 1) & mask;
  }
  if (--channel_endpoints[slot].endpoints != 0) return;
  /* Backward-shift deletion keeps every probe sequence unbroken. */
  size_t hole = slot;
  for (size_t next = (hole + 1) & mask; channel_endpoints[next].channel;
       next = (next + 1) & mask) {
    size_t home = channel_slot(channel_endpoints[next].channel, channel_endpoint_capacity);
    if (((next - home) & mask) >= ((next - hole) & mask)) {
      channel_endpoints[hole] = channel_endpoints[next];
      hole = next;
    }
  }
  channel_endpoints[hole] = (ChannelEndpoints){0};
  --channel_endpoint_count;
  if (released_channel_count == released_channel_capacity) {
    size_t capacity = released_channel_capacity ? released_channel_capacity * 2 : 16;
    uint64_t *released = realloc(released_channels, capacity * sizeof(*released));
    if (!released) exhausted_memory();
    released_channels = released;
    released_channel_capacity = capacity;
  }
  released_channels[released_channel_count++] = channel;
}

/* Each execution starts a new scheduler, whose channel ids start over. */
static void channel_endpoints_reset(void) {
  if (channel_endpoints) memset(channel_endpoints, 0, channel_endpoint_capacity * sizeof(*channel_endpoints));
  channel_endpoint_count = 0;
  released_channel_count = 0;
}

/* Frees one allocation that its area has already unlinked. */
static void release_allocation(AllocationHeader *header) {
  if (channel_endpoint_count && header->kind == ALLOCATION_VALUE) {
    ChimiValue *value = (ChimiValue *)(header + 1);
    if (is_channel_endpoint(value)) channel_endpoint_drop(value->as.capability.channel);
  }
  if (header->area->heap) header->area->heap->live_bytes -= header->bytes;
  if (header->area->role != AREA_STATIC) {
    --heap_totals.live_allocations;
    heap_totals.footprint_bytes -= sizeof(*header) + header->bytes;
  }
  small_block_give(header, sizeof(*header) + header->bytes);
}

/* Frees every allocation of `area`, leaving it empty. O(allocations). */
static void area_release_all(AllocationArea *area) {
  while (area->allocations) {
    AllocationHeader *header = area->allocations;
    area->allocations = header->next;
    release_allocation(header);
  }
  area->bytes = 0;
  area->count = 0;
}

/* Moves every allocation of `source` into `target`. O(allocations of `source`). */
static void area_merge(AllocationArea *target, AllocationArea *source) {
  if (!source->allocations) return;
  AllocationHeader *last = NULL;
  for (AllocationHeader *header = source->allocations; header; header = header->next) {
    header->area = target;
    last = header;
  }
  last->next = target->allocations;
  target->allocations = source->allocations;
  target->bytes += source->bytes;
  target->count += source->count;
  source->allocations = NULL;
  source->bytes = 0;
  source->count = 0;
}

/* A fragment's record. Its area is the first member, so once the area is adopted
   the record lives on as that area and is freed with it (area_free). */
enum { FRAGMENT_ROOTS = 2 };
typedef struct RuntimeFragment {
  AllocationArea area;
  uint32_t root_count;
  void *roots[FRAGMENT_ROOTS];
} RuntimeFragment;

/* Frees an emptied adopted area, which is the record of the fragment it came from. */
static void area_free(AllocationArea *area) {
  heap_totals.footprint_bytes -= sizeof(RuntimeFragment);
  small_block_give(area, sizeof(RuntimeFragment));
}

static RuntimeHeap *heap_create(void) {
  RuntimeHeap *heap = calloc(1, sizeof(*heap));
  if (!heap) exhausted_memory();
  heap->owner = next_heap_owner++;
  heap->base.role = AREA_BASE;
  heap->base.heap = heap;
  ++heap_totals.live_heaps;
  return heap;
}

/* Requires: no region of `heap` is open. Frees every allocation the heap owns,
   adopted fragments included, without touching any other heap. */
static void heap_destroy(RuntimeHeap *heap) {
  if (heap->innermost) { fputs("Chimi runtime destroyed a heap with an open region\n", stderr); exit(70); }
  area_release_all(&heap->base);
  while (heap->adopted) {
    AllocationArea *area = heap->adopted;
    heap->adopted = area->next;
    area_release_all(area);
    area_free(area);
  }
  free(heap);
  --heap_totals.live_heaps;
}

static void *reallocate_bytes(void *value, size_t old_bytes, size_t new_bytes) {
  void *replacement = allocate(new_bytes, 1);
  if (value) memcpy(replacement, value, old_bytes < new_bytes ? old_bytes : new_bytes);
  return replacement;
}

static ChimiValue *new_value(ValueKind kind) {
  ChimiValue *value = allocate(1, sizeof(*value));
  allocation_kind(value, ALLOCATION_VALUE); value->kind = kind; return value;
}
static ChimiControl *new_control(ControlKind kind, ChimiValue *value, const char *message, int64_t node) {
  ChimiControl *control = allocate(1, sizeof(*control));
  allocation_kind(control, ALLOCATION_CONTROL);
  control->kind = kind; control->value = value; control->message = message; control->node = node;
  return control;
}
static ChimiControl *success(ChimiValue *value) { return new_control(CONTROL_SUCCESS, value, NULL, -1); }
static ChimiControl *failure(ChimiValue *value) { return new_control(CONTROL_FAILURE, value, NULL, -1); }
static ChimiControl *defect(const char *message, int64_t node) { return new_control(CONTROL_DEFECT, NULL, message, node); }
static ChimiControl *cancelled(void) { return new_control(CONTROL_CANCEL, NULL, NULL, -1); }

static ChimiValue **new_value_slots(uint32_t count) {
  if (count == 0) return NULL;
  ChimiValue **slots = allocate(count, sizeof(*slots));
  allocation_kind(slots, ALLOCATION_VALUE_POINTERS);
  return slots;
}
static ChimiValue **clone_value_slots(ChimiValue **source, uint32_t count) {
  ChimiValue **slots = new_value_slots(count);
  if (count != 0) memcpy(slots, source, count * sizeof(*slots));
  return slots;
}
static ChimiEnvironment *new_environment(void) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_FRAME_ENVIRONMENT);
  environment->locals = new_value_slots(CHIMI_LOCAL_SLOT_COUNT);
  environment->requirements = new_value_slots(CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
static ChimiEnvironment *clone_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_FRAME_ENVIRONMENT);
  environment->locals = clone_value_slots(source->locals, CHIMI_LOCAL_SLOT_COUNT);
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
// Locals and requirements are independently immutable outside the boundaries
// that bind them. Copy only the table a boundary mutates; precise marking keeps
// the other table alive through the shared environment pointer.
static ChimiEnvironment *clone_local_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_FRAME_ENVIRONMENT);
  environment->locals = clone_value_slots(source->locals, CHIMI_LOCAL_SLOT_COUNT);
  environment->requirements = source->requirements;
  return environment;
}
static ChimiEnvironment *clone_requirement_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_FRAME_ENVIRONMENT);
  environment->locals = source->locals;
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
static ChimiEnvironment *capture_requirements(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->locals = NULL;
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
static ChimiEnvironment *capture_closure_environment(const ChimiEnvironment *source,
                                                     const ChimiNode *node) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->locals = new_value_slots(CHIMI_LOCAL_SLOT_COUNT);
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  for (uint32_t index = 0; index < node->c; ++index) {
    uint32_t symbol = CHIMI_IDS[node->b + index];
    uint32_t slot = CHIMI_SYMBOLS[symbol].environment_slot;
    environment->locals[slot] = source->locals[slot];
  }
  return environment;
}
static uint32_t environment_slot(uint32_t symbol) {
  return CHIMI_SYMBOLS[symbol].environment_slot;
}

enum { LIST_BITS = 5, LIST_WIDTH = 1 << LIST_BITS, LIST_MASK = LIST_WIDTH - 1 };

static ListNode *list_node(uint32_t height, uint32_t count) {
  ListNode *node = allocate(1, sizeof(*node) + (size_t)count * sizeof(node->slots[0]));
  allocation_kind(node, ALLOCATION_LIST_NODE);
  node->height = height; node->count = count;
  return node;
}
static ListNode *list_node_copy(const ListNode *source, uint32_t count) {
  ListNode *node = list_node(source->height, count);
  uint32_t kept = source->count < count ? source->count : count;
  memcpy(node->slots, source->slots, kept * sizeof(node->slots[0]));
  return node;
}
/* Index of the first element stored in the tail rather than the trie. */
static uint32_t vector_tail_offset(uint32_t length) {
  return length == 0 ? 0 : ((length - 1) >> LIST_BITS) << LIST_BITS;
}
/* The leaf holding element `index`; the caller reads slot `index & LIST_MASK`. */
static ListNode *vector_leaf(ListVector vector, uint32_t index) {
  if (index >= vector_tail_offset(vector.length)) return vector.tail;
  ListNode *node = vector.root;
  for (uint32_t height = node->height; height > 0; --height)
    node = node->slots[(index >> (height * LIST_BITS)) & LIST_MASK];
  return node;
}
static ListNode *vector_new_path(uint32_t height, ListNode *leaf) {
  if (height == 0) return leaf;
  ListNode *node = list_node(height, 1);
  node->slots[0] = vector_new_path(height - 1, leaf);
  return node;
}
/* Copies the path to the leaf that starts at element `first_index` and attaches `leaf`. */
static ListNode *vector_push_leaf(const ListNode *node, uint32_t first_index, ListNode *leaf) {
  uint32_t slot = (first_index >> (node->height * LIST_BITS)) & LIST_MASK;
  ListNode *copy = list_node_copy(node, slot + 1);
  if (node->height == 1) copy->slots[slot] = leaf;
  else if (slot < node->count) copy->slots[slot] = vector_push_leaf(node->slots[slot], first_index, leaf);
  else copy->slots[slot] = vector_new_path(node->height - 1, leaf);
  return copy;
}
/* Amortized O(1): copies at most one 32-slot tail plus one root-to-leaf path. */
static ListVector vector_push(ListVector vector, ChimiValue *item) {
  if (vector.length == UINT32_MAX) { fputs("Chimi runtime list length overflow\n", stderr); exit(70); }
  uint32_t tail_length = vector.length - vector_tail_offset(vector.length);
  if (vector.length == 0 || tail_length < LIST_WIDTH) {
    ListNode *next = vector.length == 0 ? list_node(0, 1) : list_node_copy(vector.tail, tail_length + 1);
    next->slots[tail_length] = item;
    return (ListVector){vector.root, next, vector.length + 1};
  }
  ListNode *root = vector.root;
  uint32_t first_index = vector.length - LIST_WIDTH;
  if (!root) {
    root = vector_new_path(1, vector.tail);
  } else if ((uint64_t)(first_index >> LIST_BITS) == (uint64_t)1 << (root->height * LIST_BITS)) {
    ListNode *grown = list_node(root->height + 1, 2);
    grown->slots[0] = root; grown->slots[1] = vector_new_path(root->height, vector.tail);
    root = grown;
  } else {
    root = vector_push_leaf(root, first_index, vector.tail);
  }
  ListNode *next = list_node(0, 1);
  next->slots[0] = item;
  return (ListVector){root, next, vector.length + 1};
}
static ListNode *vector_set_node(const ListNode *node, uint32_t index, ChimiValue *item) {
  ListNode *copy = list_node_copy(node, node->count);
  if (node->height == 0) copy->slots[index & LIST_MASK] = item;
  else {
    uint32_t slot = (index >> (node->height * LIST_BITS)) & LIST_MASK;
    copy->slots[slot] = vector_set_node(node->slots[slot], index, item);
  }
  return copy;
}
/* O(log32 n): copies the path to element `index`, which must exist. */
static ListVector vector_set(ListVector vector, uint32_t index, ChimiValue *item) {
  if (index >= vector_tail_offset(vector.length))
    return (ListVector){vector.root, vector_set_node(vector.tail, index, item), vector.length};
  return (ListVector){vector_set_node(vector.root, index, item), vector.tail, vector.length};
}
/* Builds a vector from `length` values in O(length) without intermediate versions. */
static ListVector vector_from_values(ChimiValue *const *values, uint32_t length) {
  if (length == 0) return (ListVector){NULL, NULL, 0};
  uint32_t tail_offset = vector_tail_offset(length);
  ListNode *tail = list_node(0, length - tail_offset);
  memcpy(tail->slots, values + tail_offset, tail->count * sizeof(tail->slots[0]));
  uint32_t level_count = tail_offset >> LIST_BITS;
  if (level_count == 0) return (ListVector){NULL, tail, length};
  ListNode **level = malloc(level_count * sizeof(*level));
  if (!level) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  for (uint32_t index = 0; index < level_count; ++index) {
    level[index] = list_node(0, LIST_WIDTH);
    memcpy(level[index]->slots, values + ((size_t)index << LIST_BITS), sizeof(level[index]->slots[0]) * LIST_WIDTH);
  }
  uint32_t height = 0;
  while (level_count > 1 || height == 0) {
    ++height;
    uint32_t parent_count = (level_count + LIST_MASK) >> LIST_BITS;
    for (uint32_t parent = 0; parent < parent_count; ++parent) {
      uint32_t first = parent << LIST_BITS;
      uint32_t count = level_count - first < LIST_WIDTH ? level_count - first : LIST_WIDTH;
      ListNode *node = list_node(height, count);
      memcpy(node->slots, level + first, count * sizeof(node->slots[0]));
      level[parent] = node;
    }
    level_count = parent_count;
  }
  ListNode *root = level[0];
  free(level);
  return (ListVector){root, tail, length};
}

static ChimiValue *list_value(ListVector vector, uint32_t start, uint32_t length) {
  ChimiValue *value = new_value(VALUE_LIST);
  value->as.list.vector = vector; value->as.list.start = start; value->as.list.length = length;
  return value;
}
static ChimiValue *list_from_values(ChimiValue *const *values, uint32_t length) {
  return list_value(vector_from_values(values, length), 0, length);
}
static uint32_t list_length(const ChimiValue *list) { return list->as.list.length; }
static ChimiValue *list_get(const ChimiValue *list, uint32_t index) {
  if (index >= list->as.list.length) return NULL;
  index += list->as.list.start;
  return vector_leaf(list->as.list.vector, index)->slots[index & LIST_MASK];
}
/* list_push is reached from Chimi once the standard-library mechanism (G1)
   exposes it; until then only tests call it. */

/* Returns a new list with `item` appended; `list` is unchanged. A window that
   ends before its vector does writes slot `end` into a path copy instead, so
   the shared vector every other window sees is untouched. */
__attribute__((unused)) static ChimiValue *list_push(const ChimiValue *list, ChimiValue *item) {
  ListVector vector = list->as.list.vector;
  uint32_t end = list->as.list.start + list->as.list.length;
  vector = end == vector.length ? vector_push(vector, item) : vector_set(vector, end, item);
  return list_value(vector, list->as.list.start, list->as.list.length + 1);
}
/* Copies elements [from, from + length) of `list` into a fresh vector allocated in
   `area`, or in the current area when `area` is NULL. */
static ListVector list_copy_window(const ChimiValue *list, uint32_t from, uint32_t length,
                                   AllocationArea *area) {
  ChimiValue **values = malloc((size_t)length * sizeof(*values));
  if (!values) exhausted_memory();
  for (uint32_t index = 0; index < length; ++index) values[index] = list_get(list, from + index);
  AllocationArea *previous = allocation_target;
  if (area) allocation_target = area;
  ListVector copy = vector_from_values(values, length);
  allocation_target = previous;
  free(values);
  return copy;
}
/* A window shorter than 1/LIST_RETAINED_FRACTION of its vector is copied. */
enum { LIST_RETAINED_FRACTION = 4 };
static bool list_retains_too_much(uint32_t window, uint32_t vector) {
  return (uint64_t)window * LIST_RETAINED_FRACTION < vector;
}
/* Window [from, to) of `list`, or NULL when the range is reversed or past the end.
   The window shares the vector in O(1) unless it is shorter than a quarter of it,
   so a small window does not keep a large vector alive. Then `list` itself is
   compacted first: it adopts a fresh vector holding exactly its elements, and the
   window shares that. Values are immutable, so swapping the representation is
   unobservable, and every later window of `list` shares the compact vector instead
   of copying again; a window shorter than a quarter of `list` itself is copied on
   its own. Draining n elements one at a time copies fewer than n/3 in total
   (n/4 + n/16 + ...), because each copy of m elements follows at least 3m dropped
   ones. The interpreter's PersistentList::slice applies the same rule.

   The compact vector is allocated in the area that holds `list`, so `list` never
   points into a younger area: a survivor's new vector is itself a survivor. The
   in-place update is safe because one carrier runs every fiber and none switches
   inside list_slice; parallel carriers (R4) would have to publish the vector and
   start together, for example through an atomic pointer to an immutable window,
   or leave shared values uncompacted. */
static ChimiValue *list_slice(ChimiValue *list, uint32_t from, uint32_t to) {
  if (from > to || to > list->as.list.length) return NULL;
  if (from == to) return list_value((ListVector){NULL, NULL, 0}, 0, 0);
  uint32_t length = to - from;
  if (list_retains_too_much(length, list->as.list.vector.length)) {
    if (list_retains_too_much(length, list->as.list.length))
      return list_value(list_copy_window(list, from, length, NULL), 0, length);
    list->as.list.vector =
      list_copy_window(list, 0, list->as.list.length, allocation_header(list)->area);
    list->as.list.start = 0;
  }
  return list_value(list->as.list.vector, list->as.list.start + from, length);
}

static void allocation_graph_inconsistent(void) {
  fputs("Chimi runtime allocation graph is inconsistent\n", stderr); exit(70);
}

/* A growable stack of allocations, used by marking and copying so neither recurses:
   both run on 64 KiB fiber stacks. */
typedef struct { void **items; size_t length; size_t capacity; } PointerStack;
static void pointer_stack_push(PointerStack *stack, void *pointer) {
  if (stack->length == stack->capacity) {
    size_t capacity = stack->capacity ? stack->capacity * 2 : 256;
    void **items = realloc(stack->items, capacity * sizeof(*items));
    if (!items) exhausted_memory();
    stack->items = items;
    stack->capacity = capacity;
  }
  stack->items[stack->length++] = pointer;
}

/* The pass in progress. Marking reaches only this region's young allocations, plus
   its survivors in a full pass: everything else is older than the region's values
   or than the pass, so it cannot reach them. */
static AllocationScope *collecting_scope;
static bool collecting_full;
static bool collecting_foreign;
static uint64_t collecting_epoch;
static PointerStack mark_stack;

static void mark_push(void *pointer, AllocationKind kind) {
  if (!pointer) return;
  AllocationHeader *header = allocation_header(pointer);
  if (header->kind != kind) allocation_graph_inconsistent();
  AllocationArea *area = header->area;
  if (area->scope != collecting_scope) {
    if (area->heap != collecting_scope->heap && area->role != AREA_STATIC) collecting_foreign = true;
    return;
  }
  if (area->role == AREA_SURVIVOR && !collecting_full) return;
  if (header->mark_epoch == collecting_epoch) return;
  header->mark_epoch = collecting_epoch;
  ++collecting_scope->scanned_allocations;
  pointer_stack_push(&mark_stack, pointer);
}

static void mark_children(void *pointer) {
  AllocationHeader *header = allocation_header(pointer);
  switch (header->kind) {
    case ALLOCATION_VALUE: {
      ChimiValue *value = pointer;
      switch (value->kind) {
        case VALUE_TEXT: mark_push((void *)value->as.text, ALLOCATION_BYTES); break;
        case VALUE_JSON_NUMBER: mark_push((void *)value->as.json_number, ALLOCATION_BYTES); break;
        case VALUE_LIST:
          mark_push(value->as.list.vector.root, ALLOCATION_LIST_NODE);
          mark_push(value->as.list.vector.tail, ALLOCATION_LIST_NODE);
          break;
        case VALUE_RECORD:
        case VALUE_TUPLE: mark_push(value->as.record.fields, ALLOCATION_FIELDS); break;
        case VALUE_CONSTRUCTED: mark_push(value->as.constructed.fields, ALLOCATION_FIELDS); break;
        case VALUE_CALLABLE: mark_push(value->as.callable, ALLOCATION_CALLABLE); break;
        case VALUE_MAP: mark_push(value->as.map, ALLOCATION_MAP_NODE); break;
        case VALUE_TASK:
        case VALUE_CHANNEL_SENDER:
        case VALUE_CHANNEL_RECEIVER:
        case VALUE_CHANNEL_COMPLETION:
        case VALUE_VOID:
        case VALUE_INTEGER:
        case VALUE_FLOAT:
        case VALUE_BOOLEAN:
        case VALUE_RANGE:
        case VALUE_DURATION:
        case VALUE_PROCESS_ADDRESS:
        case VALUE_PROCESS_INBOX:
        case VALUE_SUPERVISOR_RUNNING:
        case VALUE_MANAGED_PROCESS:
        case VALUE_MANAGED_SUPERVISOR: break;
      }
      break;
    }
    case ALLOCATION_LIST_NODE: {
      ListNode *node = pointer;
      AllocationKind kind = node->height == 0 ? ALLOCATION_VALUE : ALLOCATION_LIST_NODE;
      for (uint32_t index = 0; index < node->count; ++index) mark_push(node->slots[index], kind);
      break;
    }
    case ALLOCATION_FIELDS: {
      RuntimeField *fields = pointer;
      size_t length = header->bytes / sizeof(*fields);
      for (size_t index = 0; index < length; ++index) mark_push(fields[index].value, ALLOCATION_VALUE);
      break;
    }
    case ALLOCATION_VALUE_POINTERS: {
      ChimiValue **values = pointer;
      size_t length = header->bytes / sizeof(*values);
      for (size_t index = 0; index < length; ++index) mark_push(values[index], ALLOCATION_VALUE);
      break;
    }
    case ALLOCATION_ENVIRONMENT: {
      ChimiEnvironment *environment = pointer;
      mark_push(environment->locals, ALLOCATION_VALUE_POINTERS);
      mark_push(environment->requirements, ALLOCATION_VALUE_POINTERS);
      break;
    }
    case ALLOCATION_CALLABLE: {
      ChimiCallable *callable = pointer;
      mark_push(callable->environment, ALLOCATION_ENVIRONMENT);
      mark_push(callable->inner, ALLOCATION_CALLABLE);
      break;
    }
    case ALLOCATION_MAP_NODE: {
      MapNode *node = pointer;
      mark_push(node->key, ALLOCATION_VALUE);
      mark_push(node->value, ALLOCATION_VALUE);
      mark_push(node->left, ALLOCATION_MAP_NODE);
      mark_push(node->right, ALLOCATION_MAP_NODE);
      break;
    }
    case ALLOCATION_CONTROL: mark_push(((ChimiControl *)pointer)->value, ALLOCATION_VALUE); break;
    case ALLOCATION_BYTES: break;
    case ALLOCATION_FRAME_ENVIRONMENT:
    case ALLOCATION_KIND_COUNT: allocation_graph_inconsistent();
  }
}

static void allocation_scope_begin(AllocationScope *scope) {
  memset(scope, 0, sizeof(*scope));
  RuntimeHeap *heap = current_heap;
  scope->heap = heap;
  scope->parent = heap->innermost;
  scope->young = (AllocationArea){.role = AREA_YOUNG, .heap = heap, .scope = scope};
  scope->survivors = (AllocationArea){.role = AREA_SURVIVOR, .heap = heap, .scope = scope};
  heap->innermost = scope;
}

/* Moves the marked allocations of a young `area` to the survivors and frees the rest. */
static void promote_marked(AllocationScope *scope, AllocationArea *area) {
  AllocationHeader *header = area->allocations;
  while (header) {
    AllocationHeader *next = header->next;
    if (header->mark_epoch == collecting_epoch) {
      area_link(&scope->survivors, header);
      scope->promoted_bytes += header->bytes;
    } else {
      scope->live_bytes -= header->bytes;
      release_allocation(header);
    }
    header = next;
  }
  area->allocations = NULL;
  area->bytes = 0;
  area->count = 0;
}

/* Frees the survivors that this full pass did not mark. */
static void sweep_survivors(AllocationScope *scope) {
  AllocationArea *area = &scope->survivors;
  AllocationHeader **link = &area->allocations;
  while (*link) {
    AllocationHeader *header = *link;
    if (header->mark_epoch == collecting_epoch) {
      link = &header->next;
      continue;
    }
    *link = header->next;
    area->bytes -= header->bytes;
    --area->count;
    scope->live_bytes -= header->bytes;
    release_allocation(header);
  }
}

/* One pass over `scope`, keeping what `value` and `control` reach. A minor pass
   costs O(allocations since the last pass): it marks young allocations only and
   promotes the marked ones. A full pass also marks and sweeps the survivors.
   Returns HEAP_FOREIGN_POINTER, after collecting as usual, when a root reaches a
   value in another heap or in a fragment. */
static HeapStatus allocation_scope_sweep(AllocationScope *scope, ChimiValue *value,
                                         ChimiControl *control, bool full) {
  collecting_epoch = next_mark_epoch++;
  if (next_mark_epoch == 0) {
    fputs("Chimi runtime allocation mark epoch exhausted\n", stderr); exit(70);
  }
  collecting_scope = scope;
  collecting_full = full;
  collecting_foreign = false;
  mark_push(value, ALLOCATION_VALUE);
  mark_push(control, ALLOCATION_CONTROL);
  while (mark_stack.length) mark_children(mark_stack.items[--mark_stack.length]);
  bool foreign = collecting_foreign;
  promote_marked(scope, &scope->young);
  while (scope->adopted) {
    AllocationArea *area = scope->adopted;
    scope->adopted = area->next;
    promote_marked(scope, area);
    area_free(area);
  }
  if (full) {
    sweep_survivors(scope);
    ++scope->full_collections;
  }
  collecting_scope = NULL;
  ++scope->collections;
  return foreign ? HEAP_FOREIGN_POINTER : HEAP_OK;
}

/* A pass boundary: chooses a minor or a full pass (see full_baseline). */
static HeapStatus allocation_scope_collect(AllocationScope *scope, ChimiValue *value,
                                           ChimiControl *control) {
  size_t survivors = scope->survivors.bytes;
  bool full = scope->has_baseline && survivors >= 2 * scope->full_baseline &&
              survivors - scope->full_baseline >= REGION_FULL_MIN_GROWTH;
  HeapStatus status = allocation_scope_sweep(scope, value, control, full);
  if (full || (!scope->has_baseline && scope->survivors.bytes != 0)) {
    scope->has_baseline = true;
    scope->full_baseline = scope->survivors.bytes;
  }
  return status;
}

/* Region exit: a full pass keeps what `value` and `control` reach, and the survivors
   join the enclosing region's young generation, or the heap's base area. */
static HeapStatus allocation_scope_end(AllocationScope *scope, ChimiValue *value,
                                       ChimiControl *control) {
  HeapStatus status = allocation_scope_sweep(scope, value, control, true);
  RuntimeHeap *heap = scope->heap;
  AllocationScope *parent = scope->parent;
  heap->innermost = parent;
  area_merge(parent ? &parent->young : &heap->base, &scope->survivors);
  if (!parent) return status;
  size_t nested_peak = parent->live_bytes + scope->peak_live_bytes;
  if (nested_peak > parent->peak_live_bytes) parent->peak_live_bytes = nested_peak;
  parent->live_bytes += scope->live_bytes;
  parent->total_allocated_bytes += scope->total_allocated_bytes;
  for (uint32_t kind = 0; kind < ALLOCATION_KIND_COUNT; ++kind)
    parent->allocated_bytes_by_kind[kind] += scope->allocated_bytes_by_kind[kind];
  parent->collections += scope->collections;
  parent->full_collections += scope->full_collections;
  parent->scanned_allocations += scope->scanned_allocations;
  parent->promoted_bytes += scope->promoted_bytes;
  return status;
}

/* Fragments: copies for a transfer between owners. A copy keeps sharing through a
   forwarding table, so it is linear in distinct allocations, and it rebuilds each
   List window as a compact vector, so sending a short slice of a long list copies
   only the slice. Static constants are shared, not copied. */
enum { COPY_INLINE_SLOTS = 16 };
typedef struct {
  /* The copy's source: every allocation of `source_heap`, or of `source_area`. */
  RuntimeHeap *source_heap;
  const AllocationArea *source_area;
  AllocationArea *target;
  void **keys;
  void **copies;
  size_t capacity;
  size_t count;
  HeapStatus status;
  /* The owner of the first foreign allocation reached (0 for a fragment's). */
  uint64_t foreign_owner;
  /* Small messages fit the inline table, so copying one allocates nothing else. */
  void *inline_keys[COPY_INLINE_SLOTS];
  void *inline_copies[COPY_INLINE_SLOTS];
} FragmentCopy;

/* Copies that copy_scan still has to redirect. Copying never yields, so one stack,
   kept between copies, serves every heap. */
static PointerStack copy_pending;

/* Values and payload bytes that message copies (Process.send) duplicated: the memory
   report's `copied_values` and `copied_bytes`. Only a copy made with
   `copy_counting` set adds to them. */
static bool copy_counting;
static uint64_t copied_values;
static uint64_t copied_bytes;
/* The foreign owner the last failed fragment_copy reached. */
static uint64_t fragment_copy_foreign_owner;

static void copy_foreign(FragmentCopy *copy, const AllocationArea *area) {
  if (copy->status == HEAP_OK) copy->foreign_owner = area->heap ? area->heap->owner : 0;
  copy->status = HEAP_FOREIGN_POINTER;
}

static size_t pointer_slot(const void *pointer, size_t capacity) {
  uint64_t hash = (uint64_t)(uintptr_t)pointer * UINT64_C(0x9E3779B97F4A7C15);
  return (size_t)(hash >> 20) & (capacity - 1);
}

static void copy_table_insert(FragmentCopy *copy, void *key, void *value) {
  if (2 * (copy->count + 1) > copy->capacity) {
    size_t capacity = copy->capacity * 2;
    void **keys = calloc(capacity, sizeof(*keys));
    void **copies = calloc(capacity, sizeof(*copies));
    if (!keys || !copies) exhausted_memory();
    for (size_t index = 0; index < copy->capacity; ++index) {
      if (!copy->keys[index]) continue;
      size_t slot = pointer_slot(copy->keys[index], capacity);
      while (keys[slot]) slot = (slot + 1) & (capacity - 1);
      keys[slot] = copy->keys[index];
      copies[slot] = copy->copies[index];
    }
    if (copy->keys != copy->inline_keys) {
      free(copy->keys);
      free(copy->copies);
    }
    copy->keys = keys;
    copy->copies = copies;
    copy->capacity = capacity;
  }
  size_t slot = pointer_slot(key, copy->capacity);
  while (copy->keys[slot]) slot = (slot + 1) & (copy->capacity - 1);
  copy->keys[slot] = key;
  copy->copies[slot] = value;
  ++copy->count;
}

static void *copy_table_find(const FragmentCopy *copy, const void *key) {
  size_t slot = pointer_slot(key, copy->capacity);
  while (copy->keys[slot]) {
    if (copy->keys[slot] == key) return copy->copies[slot];
    slot = (slot + 1) & (copy->capacity - 1);
  }
  return NULL;
}

static bool copy_owns(const FragmentCopy *copy, const AllocationArea *area) {
  return copy->source_area ? area == copy->source_area : area->heap == copy->source_heap;
}

/* The copy of `pointer`, made on first sight and queued for copy_scan. */
static void *copy_forward(FragmentCopy *copy, void *pointer, AllocationKind kind) {
  if (!pointer) return NULL;
  AllocationHeader *header = allocation_header(pointer);
  if (header->kind != kind) allocation_graph_inconsistent();
  if (header->area->role == AREA_STATIC) return pointer;
  if (!copy_owns(copy, header->area) && copy->status == HEAP_OK)
    copy->foreign_owner = header->area->heap ? header->area->heap->owner : 0;
  if (!copy_owns(copy, header->area)) {
    copy->status = HEAP_FOREIGN_POINTER;
    return NULL;
  }
  void *existing = copy_table_find(copy, pointer);
  if (existing) return existing;
  AllocationArea *previous = allocation_target;
  allocation_target = copy->target;
  void *duplicate = allocate(header->bytes, 1);
  allocation_target = previous;
  allocation_kind(duplicate, kind);
  memcpy(duplicate, pointer, header->bytes);
  if (copy_counting) {
    copied_bytes += header->bytes;
    if (kind == ALLOCATION_VALUE) ++copied_values;
  }
  if (kind == ALLOCATION_VALUE && is_channel_endpoint(duplicate))
    channel_endpoint_retain(((ChimiValue *)duplicate)->as.capability.channel);
  copy_table_insert(copy, pointer, duplicate);
  pointer_stack_push(&copy_pending, duplicate);
  return duplicate;
}

static void copy_list_window(FragmentCopy *copy, ChimiValue *list) {
  ListVector source = list->as.list.vector;
  AllocationHeader *root = allocation_header(source.root), *tail = allocation_header(source.tail);
  if ((root && root->area->role != AREA_STATIC && !copy_owns(copy, root->area)) ||
      (tail && tail->area->role != AREA_STATIC && !copy_owns(copy, tail->area))) {
    copy_foreign(copy, root && !copy_owns(copy, root->area) ? root->area : tail->area);
    list->as.list.vector = (ListVector){NULL, NULL, 0};
    list->as.list.start = list->as.list.length = 0;
    return;
  }
  uint32_t length = list->as.list.length;
  ChimiValue **values = malloc(((size_t)length + 1) * sizeof(*values));
  if (!values) exhausted_memory();
  for (uint32_t index = 0; index < length; ++index)
    values[index] = copy_forward(copy, list_get(list, index), ALLOCATION_VALUE);
  AllocationArea *previous = allocation_target;
  allocation_target = copy->target;
  list->as.list.vector = vector_from_values(values, length);
  allocation_target = previous;
  list->as.list.start = 0;
  free(values);
}

/* Redirects the pointers of a fresh duplicate from its source's graph to copies. */
static void copy_scan(FragmentCopy *copy, void *duplicate) {
  AllocationHeader *header = allocation_header(duplicate);
  switch (header->kind) {
    case ALLOCATION_VALUE: {
      ChimiValue *value = duplicate;
      switch (value->kind) {
        case VALUE_TEXT:
          value->as.text = copy_forward(copy, (void *)value->as.text, ALLOCATION_BYTES); break;
        case VALUE_JSON_NUMBER:
          value->as.json_number = copy_forward(copy, (void *)value->as.json_number, ALLOCATION_BYTES);
          break;
        case VALUE_LIST: copy_list_window(copy, value); break;
        case VALUE_RECORD:
        case VALUE_TUPLE:
          value->as.record.fields = copy_forward(copy, value->as.record.fields, ALLOCATION_FIELDS);
          break;
        case VALUE_CONSTRUCTED:
          value->as.constructed.fields =
            copy_forward(copy, value->as.constructed.fields, ALLOCATION_FIELDS);
          break;
        case VALUE_CALLABLE:
          value->as.callable = copy_forward(copy, value->as.callable, ALLOCATION_CALLABLE); break;
        case VALUE_MAP: value->as.map = copy_forward(copy, value->as.map, ALLOCATION_MAP_NODE); break;
        case VALUE_TASK:
        case VALUE_CHANNEL_SENDER:
        case VALUE_CHANNEL_RECEIVER:
        case VALUE_CHANNEL_COMPLETION:
        case VALUE_VOID:
        case VALUE_INTEGER:
        case VALUE_FLOAT:
        case VALUE_BOOLEAN:
        case VALUE_RANGE:
        case VALUE_DURATION:
        case VALUE_PROCESS_ADDRESS:
        case VALUE_PROCESS_INBOX:
        case VALUE_SUPERVISOR_RUNNING:
        case VALUE_MANAGED_PROCESS:
        case VALUE_MANAGED_SUPERVISOR: break;
      }
      break;
    }
    case ALLOCATION_FIELDS: {
      RuntimeField *fields = duplicate;
      size_t length = header->bytes / sizeof(*fields);
      for (size_t index = 0; index < length; ++index)
        fields[index].value = copy_forward(copy, fields[index].value, ALLOCATION_VALUE);
      break;
    }
    case ALLOCATION_VALUE_POINTERS: {
      ChimiValue **values = duplicate;
      size_t length = header->bytes / sizeof(*values);
      for (size_t index = 0; index < length; ++index)
        values[index] = copy_forward(copy, values[index], ALLOCATION_VALUE);
      break;
    }
    case ALLOCATION_ENVIRONMENT: {
      ChimiEnvironment *environment = duplicate;
      environment->locals = copy_forward(copy, environment->locals, ALLOCATION_VALUE_POINTERS);
      environment->requirements =
        copy_forward(copy, environment->requirements, ALLOCATION_VALUE_POINTERS);
      break;
    }
    case ALLOCATION_CALLABLE: {
      ChimiCallable *callable = duplicate;
      callable->environment = copy_forward(copy, callable->environment, ALLOCATION_ENVIRONMENT);
      callable->inner = copy_forward(copy, callable->inner, ALLOCATION_CALLABLE);
      break;
    }
    case ALLOCATION_MAP_NODE: {
      MapNode *node = duplicate;
      node->key = copy_forward(copy, node->key, ALLOCATION_VALUE);
      node->value = copy_forward(copy, node->value, ALLOCATION_VALUE);
      node->left = copy_forward(copy, node->left, ALLOCATION_MAP_NODE);
      node->right = copy_forward(copy, node->right, ALLOCATION_MAP_NODE);
      break;
    }
    case ALLOCATION_CONTROL: {
      ChimiControl *control = duplicate;
      control->value = copy_forward(copy, control->value, ALLOCATION_VALUE);
      break;
    }
    case ALLOCATION_BYTES: break;
    case ALLOCATION_LIST_NODE:
    case ALLOCATION_FRAME_ENVIRONMENT:
    case ALLOCATION_KIND_COUNT: allocation_graph_inconsistent();
  }
}

/* Copies the graphs under `roots` into `target`, writing the copied roots to `out`. */
static HeapStatus copy_graph(FragmentCopy *copy, void *const *roots, const AllocationKind *kinds,
                             uint32_t count, void **out) {
  memset(copy->inline_keys, 0, sizeof(copy->inline_keys));
  copy->keys = copy->inline_keys;
  copy->copies = copy->inline_copies;
  copy->capacity = COPY_INLINE_SLOTS;
  for (uint32_t index = 0; index < count; ++index)
    out[index] = copy_forward(copy, roots[index], kinds[index]);
  while (copy_pending.length) copy_scan(copy, copy_pending.items[--copy_pending.length]);
  if (copy->keys != copy->inline_keys) {
    free(copy->keys);
    free(copy->copies);
  }
  return copy->status;
}

static size_t fragment_footprint(const RuntimeFragment *fragment) {
  return sizeof(*fragment) + fragment->area.bytes +
         fragment->area.count * sizeof(AllocationHeader);
}

/* Transfer boundary. Requires: the current heap owns every value under `roots`, or
   they are static. Ensures: a detached fragment holding one copy of the graph.
   Returns HEAP_FOREIGN_POINTER, and no fragment, if the graph leaves the heap. */
static HeapStatus fragment_copy(void *const *roots, const AllocationKind *kinds, uint32_t count,
                                RuntimeFragment **out) {
  if (count > FRAGMENT_ROOTS) abort();
  RuntimeFragment *fragment = small_block_take(sizeof(*fragment));
  fragment->area.role = AREA_FRAGMENT;
  fragment->root_count = count;
  ++heap_totals.live_fragments;
  footprint_add(sizeof(*fragment));
  fragment_footprint_add(sizeof(*fragment));
  FragmentCopy copy = {.source_heap = current_heap, .target = &fragment->area};
  HeapStatus status = copy_graph(&copy, roots, kinds, count, fragment->roots);
  if (status != HEAP_OK) {
    fragment_copy_foreign_owner = copy.foreign_owner;
    heap_totals.fragment_footprint_bytes -= fragment_footprint(fragment);
    area_release_all(&fragment->area);
    --heap_totals.live_fragments;
    area_free(&fragment->area);
    *out = NULL;
    return status;
  }
  *out = fragment;
  return HEAP_OK;
}

/* Hands the fragment's allocations to the current heap's innermost region (or its
   base area) in O(1), without copying, and writes its roots to `roots`. Consumes it. */
static void fragment_adopt(RuntimeFragment *fragment, void **roots) {
  for (uint32_t index = 0; index < fragment->root_count; ++index) roots[index] = fragment->roots[index];
  heap_totals.fragment_footprint_bytes -= fragment_footprint(fragment);
  --heap_totals.live_fragments;
  AllocationArea *area = &fragment->area;
  RuntimeHeap *heap = current_heap;
  AllocationScope *scope = heap->innermost;
  area->heap = heap;
  heap_add_live(heap, area->bytes);
  if (scope) {
    area->role = AREA_YOUNG;
    area->scope = scope;
    area->next = scope->adopted;
    scope->adopted = area;
    scope_add_live(scope, area->bytes);
  } else {
    area->role = AREA_BASE;
    area->next = heap->adopted;
    heap->adopted = area;
    unscoped_allocated_bytes += area->bytes;
  }
}

/* Copies the fragment's graph into the current area, keeping sharing, and leaves the
   fragment intact, so it can be copied again (Task.result). O(fragment). */
static void fragment_copy_into_current(const RuntimeFragment *fragment, void **roots) {
  AllocationKind kinds[FRAGMENT_ROOTS];
  for (uint32_t index = 0; index < fragment->root_count; ++index)
    kinds[index] = allocation_header(fragment->roots[index])->kind;
  FragmentCopy copy = {.source_area = &fragment->area, .target = current_area()};
  if (copy_graph(&copy, fragment->roots, kinds, fragment->root_count, roots) != HEAP_OK)
    allocation_graph_inconsistent();
}

/* Discards an unadopted fragment, for example a message handed back to its sender. */
static void fragment_free(RuntimeFragment *fragment) {
  heap_totals.fragment_footprint_bytes -= fragment_footprint(fragment);
  --heap_totals.live_fragments;
  area_release_all(&fragment->area);
  area_free(&fragment->area);
}

/* The trace label (`<module path>.<identifier>`) of a scheduler name id, which is the
   `process name` symbol's index plus 1; NULL for an id that names none. */
static const char *process_name_label(uint64_t name) {
  if (name == 0 || name > CHIMI_SYMBOL_COUNT ||
      CHIMI_SYMBOLS[name - 1].kind != CHIMI_SYMBOL_PROCESS_NAME)
    return NULL;
  return CHIMI_STRINGS[CHIMI_SYMBOLS[name - 1].name];
}

static const ChimiFunction *find_function(uint32_t symbol) {
  for (uint32_t index = 0; index < CHIMI_FUNCTION_COUNT; ++index)
    if (CHIMI_FUNCTIONS[index].symbol == symbol) return &CHIMI_FUNCTIONS[index];
  return NULL;
}

static ChimiControl *run_node(uint32_t node, ChimiRuntime *runtime,
                              ChimiEnvironment *environment, uint64_t parent_span) {
  if (runtime->cancel) return cancelled();
  return CHIMI_NODE_FUNCTIONS[node](runtime, environment, parent_span);
}

static const char *control_name(const ChimiControl *control) {
  switch (control->kind) {
    case CONTROL_SUCCESS: return "success";
    case CONTROL_FAILURE: return "expected_failure";
    case CONTROL_DEFECT: return "defect";
    case CONTROL_CANCEL: return "cancellation";
    case CONTROL_BREAK: return "break";
  }
  return "defect";
}

static void json_string(FILE *output, const char *value) {
  fputc('"', output);
  for (const unsigned char *cursor = (const unsigned char *)value; *cursor; ++cursor) {
    switch (*cursor) {
      case '"': fputs("\\\"", output); break;
      case '\\': fputs("\\\\", output); break;
      case '\n': fputs("\\n", output); break;
      case '\r': fputs("\\r", output); break;
      case '\t': fputs("\\t", output); break;
      default:
        if (*cursor < 0x20) fprintf(output, "\\u%04x", *cursor); else fputc(*cursor, output);
    }
  }
  fputc('"', output);
}

static void json_value(FILE *output, const ChimiValue *value);
static void float_text(double value, char *output);

static void emit_event(ChimiRuntime *runtime, const char *event, uint32_t node_id,
                       uint64_t span, int64_t parent, const char *operation,
                       const ChimiControl *outcome) {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  FILE *output = runtime->telemetry;
  ++runtime->trace_events_constructed;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":", output); json_string(output, event);
  fprintf(output, ",\"sequence\":%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, CHIMI_COMPUTATION); fputs(",\"execution_id\":", output);
  json_string(output, runtime->execution_id);
  fprintf(output, ",\"span_id\":%" PRIu64 ",\"parent_span_id\":", span);
  if (parent < 0) fputs("null", output); else fprintf(output, "%" PRId64, parent);
  fprintf(output, ",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":",
         node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  json_string(output, operation); fputs(",\"attributes\":{", output);
  if (outcome) { fputs("\"outcome\":", output); json_string(output, control_name(outcome)); }
  fputs("}}\n", output);
}

static void emit_dependency(ChimiRuntime *runtime, uint32_t node_id, uint64_t span,
                            uint64_t parent, const ChimiCallable *callable) {
  const ChimiNode *node = &CHIMI_NODES[node_id]; const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  FILE *output = runtime->telemetry;
  ++runtime->trace_events_constructed;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":\"dependency_called\",\"sequence\":", output);
  fprintf(output, "%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, CHIMI_COMPUTATION);
  fputs(",\"execution_id\":", output); json_string(output, runtime->execution_id);
  fprintf(output, ",\"span_id\":%" PRIu64 ",\"parent_span_id\":%" PRIu64
         ",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":\"dependency_call\",\"attributes\":{",
         span, parent, node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  fputs("\"requirement\":", output);
  json_string(output, CHIMI_STRINGS[CHIMI_SYMBOLS[callable->requirement].name]);
  fprintf(output, ",\"requirement_symbol_id\":%u,\"interface_symbol_id\":%u,\"method\":",
          callable->requirement, callable->interface_symbol); json_string(output, callable->name);
  fprintf(output, ",\"method_semantic_id\":%u}}\n", callable->method);
}

static uint64_t begin_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent, const char *operation) {
  if (runtime->silent) return parent;
  uint64_t span = runtime->next_span++;
  if (runtime->full_trace) emit_event(runtime, "operation_started", node, span, (int64_t)parent, operation, NULL);
  return span;
}
static ChimiControl *finish_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent,
                                 uint64_t span, const char *operation, ChimiControl *control) {
  if (runtime->full_trace) emit_event(runtime, "operation_completed", node, span, (int64_t)parent, operation, control);
  return control;
}

static ChimiValue *integer_value(int64_t raw) { ChimiValue *v = new_value(VALUE_INTEGER); v->as.integer = raw; return v; }
static ChimiValue *range_value(int64_t start, int64_t end) {
  ChimiValue *v = new_value(VALUE_RANGE); v->as.range.start = start; v->as.range.end = end; return v;
}
static ChimiValue *void_value(void) { return new_value(VALUE_VOID); }
static char *copy_text(const char *raw) {
  size_t length = strlen(raw);
  char *copy = allocate(length + 1, 1);
  memcpy(copy, raw, length);
  return copy;
}
static ChimiValue *text_value(const char *raw) { ChimiValue *v = new_value(VALUE_TEXT); v->as.text = copy_text(raw); return v; }
static ChimiValue *boolean_value(bool raw) { ChimiValue *v = new_value(VALUE_BOOLEAN); v->as.boolean = raw; return v; }
static ChimiValue *float_value(double raw) { ChimiValue *v = new_value(VALUE_FLOAT); v->as.floating = raw; return v; }

/* The numeric model. It mirrors compiler/chimi/src/numeric.rs value for value and
   message for message, because both execution paths must report identical outcomes. */

/* A defect message that outlives the collector: a defect ends its unit of work. */
static const char *numeric_message(const char *format, ...) {
  va_list arguments; va_start(arguments, format);
  char *message = malloc(256);
  if (!message) { fputs("Chimi runtime allocation failed\n", stderr); exit(70); }
  vsnprintf(message, 256, format, arguments); va_end(arguments);
  return message;
}

/* Canonical Float text: the shortest digits that parse back to the same value,
   positional for decimal exponents -5..15 and scientific otherwise, always with a
   `.` or an `e`. `output` holds at least 40 bytes. */
/* True when `significand` x 10^`exponent` (decimal) parses back to `magnitude`. */
static bool decimal_round_trips(uint64_t significand, int exponent, double magnitude) {
  char text[48]; snprintf(text, sizeof(text), "%" PRIu64 "e%d", significand, exponent);
  return strtod(text, NULL) == magnitude;
}

/* The shortest significand (and its decimal exponent) that parses back to
   `magnitude`, chosen like Rust's `{:e}`: at the shortest length, the candidate
   closest to the value, and the larger one on an exact tie. The correctly rounded
   candidate can miss the round-trip interval at a power of two, where the interval
   below the value is half as wide, so its neighbors are tried too. */
static void shortest_digits(double magnitude, char *digits, int *exponent) {
  for (int precision = 0; precision <= 16; ++precision) {
    char scientific[40]; snprintf(scientific, sizeof(scientific), "%.*e", precision, magnitude);
    uint64_t nearest = 0; const char *cursor = scientific;
    for (; *cursor != 'e'; ++cursor) if (*cursor != '.') nearest = nearest * 10 + (uint64_t)(*cursor - '0');
    int point = atoi(cursor + 1), scale = point - precision;
    uint64_t smallest = 1; for (int digit = 0; digit < precision; ++digit) smallest *= 10;
    uint64_t chosen = 0; bool found = false;
    if (decimal_round_trips(nearest, scale, magnitude)) {
      chosen = nearest; found = true;
      if (decimal_round_trips(nearest + 1, scale, magnitude)) {
        /* Both neighbors fit only on a tie: the exact value is `nearest`5000... */
        char exact[800]; snprintf(exact, sizeof(exact), "%.770e", magnitude);
        char wanted[24]; snprintf(wanted, sizeof(wanted), "%" PRIu64, nearest);
        char actual[800]; size_t length = 0;
        for (const char *c = exact; *c != 'e'; ++c) if (*c != '.') actual[length++] = *c;
        actual[length] = '\0';
        size_t prefix = strlen(wanted);
        bool tie = strncmp(actual, wanted, prefix) == 0 && actual[prefix] == '5' &&
                   strspn(actual + prefix + 1, "0") == length - prefix - 1;
        if (tie) chosen = nearest + 1;
      }
    } else if (decimal_round_trips(nearest + 1, scale, magnitude)) {
      chosen = nearest + 1; found = true;
    } else if (nearest > smallest && decimal_round_trips(nearest - 1, scale, magnitude)) {
      chosen = nearest - 1; found = true;
    }
    if (!found) continue;
    snprintf(digits, 24, "%" PRIu64, chosen);
    size_t count = strlen(digits);
    /* A carry such as 99 + 1 adds a digit; drop the zero it leaves. */
    *exponent = scale + (int)count - 1;
    while (count > 1 && digits[count - 1] == '0') digits[--count] = '\0';
    return;
  }
  /* 17 significant digits always round-trip. */
  char scientific[40]; snprintf(scientific, sizeof(scientific), "%.16e", magnitude);
  size_t count = 0; const char *cursor = scientific;
  for (; *cursor != 'e'; ++cursor) if (*cursor != '.') digits[count++] = *cursor;
  digits[count] = '\0'; *exponent = atoi(cursor + 1);
  while (count > 1 && digits[count - 1] == '0') digits[--count] = '\0';
}

static void float_text(double value, char *output) {
  const char *sign = signbit(value) ? "-" : "";
  if (value == 0) { snprintf(output, 40, "%s0.0", sign); return; }
  char digits[24]; int exponent = 0;
  shortest_digits(fabs(value), digits, &exponent);
  size_t count = strlen(digits);
  if (exponent >= -5 && exponent < 16) {
    if (exponent < 0) {
      snprintf(output, 40, "%s0.%.*s%s", sign, -exponent - 1, "0000", digits);
    } else if (count <= (size_t)exponent + 1) {
      snprintf(output, 40, "%s%s%.*s.0", sign, digits, (int)((size_t)exponent + 1 - count),
               "000000000000000");
    } else {
      snprintf(output, 40, "%s%.*s.%s", sign, exponent + 1, digits, digits + exponent + 1);
    }
  } else if (count == 1) {
    snprintf(output, 40, "%s%se%d", sign, digits, exponent);
  } else {
    snprintf(output, 40, "%s%c.%se%d", sign, digits[0], digits + 1, exponent);
  }
}

static const char *binary_symbol(uint32_t operator) {
  switch (operator) {
    case CHIMI_BINARY_ADD: return "+";
    case CHIMI_BINARY_SUBTRACT: return "-";
    case CHIMI_BINARY_MULTIPLY: return "*";
    case CHIMI_BINARY_DIVIDE: return "/";
    case CHIMI_BINARY_REMAINDER: return "%";
    default: return "?";
  }
}

static bool is_arithmetic(uint32_t operator) {
  return operator == CHIMI_BINARY_ADD || operator == CHIMI_BINARY_SUBTRACT ||
         operator == CHIMI_BINARY_MULTIPLY || operator == CHIMI_BINARY_DIVIDE ||
         operator == CHIMI_BINARY_REMAINDER;
}

/* Stores the exact result and returns true, or returns false on overflow or a zero divisor. */
static bool integer_result(uint32_t operator, int64_t l, int64_t r, int64_t *output) {
  switch (operator) {
    case CHIMI_BINARY_ADD: return !__builtin_add_overflow(l, r, output);
    case CHIMI_BINARY_SUBTRACT: return !__builtin_sub_overflow(l, r, output);
    case CHIMI_BINARY_MULTIPLY: return !__builtin_mul_overflow(l, r, output);
    case CHIMI_BINARY_DIVIDE:
      if (r == 0) return false;
      if (r == -1) return !__builtin_sub_overflow((int64_t)0, l, output);
      *output = l / r; return true;
    case CHIMI_BINARY_REMAINDER:
      if (r == 0) return false;
      /* INT64_MIN / -1 overflows, but INT64_MIN % -1 is exactly 0. */
      *output = r == -1 ? 0 : l % r; return true;
    default: return false;
  }
}

static ChimiControl *integer_arithmetic(uint32_t operator, int64_t l, int64_t r, int64_t node) {
  const char *symbol = binary_symbol(operator); int64_t output = 0;
  if (integer_result(operator, l, r, &output)) return success(integer_value(output));
  const char *format = r == 0 && (operator == CHIMI_BINARY_DIVIDE || operator == CHIMI_BINARY_REMAINDER)
    ? "integer division by zero in `%s`: %" PRId64 " %s %" PRId64
    : "integer overflow in `%s`: %" PRId64 " %s %" PRId64;
  return defect(numeric_message(format, symbol, l, symbol, r), node);
}

static ChimiControl *float_arithmetic(uint32_t operator, double l, double r, int64_t node) {
  const char *symbol = binary_symbol(operator); double output = 0;
  char left_text[40], right_text[40]; float_text(l, left_text); float_text(r, right_text);
  switch (operator) {
    case CHIMI_BINARY_ADD: output = l + r; break;
    case CHIMI_BINARY_SUBTRACT: output = l - r; break;
    case CHIMI_BINARY_MULTIPLY: output = l * r; break;
    case CHIMI_BINARY_DIVIDE:
    case CHIMI_BINARY_REMAINDER:
      if (r == 0) {
        return defect(numeric_message("float division by zero in `%s`: %s %s %s",
                                      symbol, left_text, symbol, right_text), node);
      }
      output = operator == CHIMI_BINARY_DIVIDE ? l / r : fmod(l, r);
      break;
  }
  if (!isfinite(output)) {
    return defect(numeric_message("float `%s` has no finite result: %s %s %s",
                                  symbol, left_text, symbol, right_text), node);
  }
  return success(float_value(output));
}
static ChimiValue *callable_value(ChimiCallable *raw) { ChimiValue *v = new_value(VALUE_CALLABLE); v->as.callable = raw; return v; }
static ChimiValue *map_value(MapNode *raw) { ChimiValue *v = new_value(VALUE_MAP); v->as.map = raw; return v; }
static ChimiValue *json_number_value(const char *raw) { ChimiValue *v = new_value(VALUE_JSON_NUMBER); v->as.json_number = copy_text(raw); return v; }
static ChimiValue *task_value(uint64_t raw) { ChimiValue *v = new_value(VALUE_TASK); v->as.task = raw; return v; }
static ChimiValue *channel_capability_value(ValueKind kind, uint64_t channel,
                                            uint64_t owner, uint64_t token) {
  ChimiValue *value = new_value(kind);
  value->as.capability = (RuntimeCapability){channel, owner, token};
  channel_endpoint_retain(channel);
  return value;
}
static ChimiValue *channel_bundle_value(uint64_t channel, uint64_t owner, uint64_t token) {
  ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = 3;
  value->as.record.fields = allocate(3, sizeof(*value->as.record.fields));
  allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
  value->as.record.fields[0] = (RuntimeField){
    "completion", channel_capability_value(VALUE_CHANNEL_COMPLETION, channel, owner, token)
  };
  value->as.record.fields[1] = (RuntimeField){
    "receiver", channel_capability_value(VALUE_CHANNEL_RECEIVER, channel, owner, token)
  };
  value->as.record.fields[2] = (RuntimeField){
    "sender", channel_capability_value(VALUE_CHANNEL_SENDER, channel, 0, 0)
  };
  return value;
}

/* Issues CHIMI_REQUEST_CHANNEL_RELEASE once for each channel whose last endpoint
   value was freed. The scheduler serves a request as soon as the task parks, so a
   release is done even when the task resumes with CHIMI_FIBER_CANCEL. */
static ChimiFiberCommand flush_channel_releases(void) {
  while (released_channel_count) {
    ChimiSchedulerRequest request = {
      .kind = CHIMI_REQUEST_CHANNEL_RELEASE,
      .channel = released_channels[--released_channel_count],
    };
    if (chimi_fiber_request(&request, NULL) == CHIMI_FIBER_CANCEL) return CHIMI_FIBER_CANCEL;
  }
  return CHIMI_FIBER_RESUME;
}

/* Every scheduler request goes through here. A fiber resumes only by returning from
   its request, so restoring the caller's heap here switches heaps on every resume.
   Pending channel releases go first; if the task is cancelled meanwhile, `request`
   is not issued and keeps status CHIMI_RESPONSE_UNSET. */
static ChimiFiberCommand scheduler_request(void *request) {
  RuntimeHeap *heap = current_heap;
  ChimiFiberCommand command = flush_channel_releases();
  if (command != CHIMI_FIBER_CANCEL) command = chimi_fiber_request(request, NULL);
  current_heap = heap;
  return command;
}

/* Capability transfer for Task.start: walks the child's copy of its captures and
   moves every Receiver and Completion it holds to the child. The walk keeps its own
   stack, so a capture of any nesting depth costs heap, not the parent fiber's
   stack. A part reachable twice (shared after the copy) is visited once, where a
   recursive pre-order walk would first reach it: `visited` is an open-addressing
   set of the parts already walked, so the walk is linear in the references it
   follows. Map keys are not walked: typing admits no capability in a
   key. */
typedef enum {
  TRANSFER_VALUE, TRANSFER_ENVIRONMENT, TRANSFER_CALLABLE, TRANSFER_MAP_NODE
} TransferPartKind;
typedef struct { TransferPartKind kind; void *pointer; } TransferPart;
typedef struct {
  const void **visited;
  size_t visited_count;
  size_t visited_capacity;
  TransferPart *pending;
  size_t pending_length;
  size_t pending_capacity;
  uint64_t child;
  bool cancelled;
} CapabilityTransfer;

/* Visited-set slots examined by every capture transfer so far; the memory report's
   `capture_transfer_probes`, so a test can bound transfer work without timing it. */
static uint64_t capture_transfer_probes;

/* Fibonacci hashing: the top bits of the product, for a power-of-two capacity. */
static size_t transfer_slot(const void *pointer, size_t capacity) {
  uint64_t hash = (uint64_t)(uintptr_t)pointer * UINT64_C(0x9e3779b97f4a7c15);
  return (size_t)(hash >> (64 - __builtin_ctzll((unsigned long long)capacity)));
}

/* Adds `pointer` to the visited set; false when it was already there. */
static bool transfer_visit(CapabilityTransfer *transfer, const void *pointer) {
  if (2 * (transfer->visited_count + 1) > transfer->visited_capacity) {
    size_t capacity = transfer->visited_capacity ? transfer->visited_capacity * 2 : 64;
    const void **visited = calloc(capacity, sizeof(*visited));
    if (!visited) exhausted_memory();
    for (size_t index = 0; index < transfer->visited_capacity; ++index) {
      const void *entry = transfer->visited[index];
      if (!entry) continue;
      size_t slot = transfer_slot(entry, capacity);
      while (visited[slot]) slot = (slot + 1) & (capacity - 1);
      visited[slot] = entry;
    }
    free(transfer->visited);
    transfer->visited = visited;
    transfer->visited_capacity = capacity;
  }
  size_t slot = transfer_slot(pointer, transfer->visited_capacity);
  for (;;) {
    ++capture_transfer_probes;
    if (transfer->visited[slot] == pointer) return false;
    if (!transfer->visited[slot]) break;
    slot = (slot + 1) & (transfer->visited_capacity - 1);
  }
  transfer->visited[slot] = pointer;
  ++transfer->visited_count;
  return true;
}

/* Queues a part. Parts pop last-pushed first, so a container pushes its parts last
   to first and the walk visits them in order. */
static void transfer_push(CapabilityTransfer *transfer, TransferPartKind kind, void *pointer) {
  if (!pointer) return;
  if (transfer->pending_length == transfer->pending_capacity) {
    size_t capacity = transfer->pending_capacity ? transfer->pending_capacity * 2 : 64;
    TransferPart *pending = realloc(transfer->pending, capacity * sizeof(*pending));
    if (!pending) exhausted_memory();
    transfer->pending = pending;
    transfer->pending_capacity = capacity;
  }
  transfer->pending[transfer->pending_length++] = (TransferPart){kind, pointer};
}

static bool transfer_captured_capability(CapabilityTransfer *transfer, ChimiValue *value,
                                         uint32_t request_kind) {
  ChimiSchedulerRequest request = {
    .kind = request_kind,
    .task = transfer->child,
    .channel = value->as.capability.channel,
    .owner = value->as.capability.owner,
    .token = value->as.capability.token,
  };
  ChimiFiberCommand command = scheduler_request(&request);
  if (command == CHIMI_FIBER_CANCEL) {
    transfer->cancelled = true;
    return false;
  }
  if (request.status != CHIMI_RESPONSE_OK) return false;
  value->as.capability.owner = request.owner;
  value->as.capability.token = request.token;
  return true;
}

/* Queues a value's parts, or transfers it when it is a capability the child takes. */
static bool transfer_captured_value(CapabilityTransfer *transfer, ChimiValue *value) {
  switch (value->kind) {
    case VALUE_CHANNEL_RECEIVER:
      return transfer_captured_capability(
        transfer, value, CHIMI_REQUEST_CHANNEL_TRANSFER_RECEIVER);
    case VALUE_CHANNEL_COMPLETION:
      return transfer_captured_capability(
        transfer, value, CHIMI_REQUEST_CHANNEL_TRANSFER_COMPLETION);
    case VALUE_LIST:
      for (uint32_t index = list_length(value); index-- > 0;)
        transfer_push(transfer, TRANSFER_VALUE, list_get(value, index));
      return true;
    case VALUE_RECORD:
    case VALUE_TUPLE:
      for (uint32_t index = value->as.record.length; index-- > 0;)
        transfer_push(transfer, TRANSFER_VALUE, value->as.record.fields[index].value);
      return true;
    case VALUE_CONSTRUCTED:
      for (uint32_t index = value->as.constructed.length; index-- > 0;)
        transfer_push(transfer, TRANSFER_VALUE, value->as.constructed.fields[index].value);
      return true;
    case VALUE_CALLABLE: transfer_push(transfer, TRANSFER_CALLABLE, value->as.callable); return true;
    case VALUE_MAP: transfer_push(transfer, TRANSFER_MAP_NODE, value->as.map); return true;
    case VALUE_VOID:
    case VALUE_INTEGER:
    case VALUE_FLOAT:
    case VALUE_TEXT:
    case VALUE_BOOLEAN:
    case VALUE_RANGE:
    case VALUE_DURATION:
    case VALUE_JSON_NUMBER:
    case VALUE_TASK:
    case VALUE_CHANNEL_SENDER:
    case VALUE_PROCESS_ADDRESS:
    case VALUE_PROCESS_INBOX:
    case VALUE_SUPERVISOR_RUNNING:
    case VALUE_MANAGED_PROCESS:
    case VALUE_MANAGED_SUPERVISOR: return true;
  }
  return false;
}

static void transfer_captured_part(CapabilityTransfer *transfer, TransferPart part) {
  switch (part.kind) {
    case TRANSFER_VALUE: break;
    case TRANSFER_ENVIRONMENT: {
      ChimiEnvironment *environment = part.pointer;
      if (environment->requirements)
        for (uint32_t slot = CHIMI_REQUIREMENT_SLOT_COUNT; slot-- > 0;)
          transfer_push(transfer, TRANSFER_VALUE, environment->requirements[slot]);
      if (environment->locals)
        for (uint32_t slot = CHIMI_LOCAL_SLOT_COUNT; slot-- > 0;)
          transfer_push(transfer, TRANSFER_VALUE, environment->locals[slot]);
      return;
    }
    case TRANSFER_CALLABLE: {
      ChimiCallable *callable = part.pointer;
      transfer_push(transfer, TRANSFER_CALLABLE, callable->inner);
      transfer_push(transfer, TRANSFER_ENVIRONMENT, callable->environment);
      return;
    }
    case TRANSFER_MAP_NODE: {
      MapNode *node = part.pointer;
      transfer_push(transfer, TRANSFER_MAP_NODE, node->right);
      transfer_push(transfer, TRANSFER_MAP_NODE, node->left);
      transfer_push(transfer, TRANSFER_VALUE, node->value);
      return;
    }
  }
}

/* Transfers every capability reachable from `root`. False when a transfer request
   fails, or when the parent is cancelled meanwhile (`cancelled` is then set). */
static bool transfer_captures(CapabilityTransfer *transfer, ChimiValue *root) {
  transfer_push(transfer, TRANSFER_VALUE, root);
  bool transferred = true;
  while (transferred && transfer->pending_length) {
    TransferPart part = transfer->pending[--transfer->pending_length];
    if (!transfer_visit(transfer, part.pointer)) continue;
    if (part.kind == TRANSFER_VALUE) transferred = transfer_captured_value(transfer, part.pointer);
    else transfer_captured_part(transfer, part);
  }
  free(transfer->visited);
  free(transfer->pending);
  return transferred;
}

static ChimiValue *application_arguments(int32_t count, const char **argv) {
  ChimiValue **items = new_value_slots((uint32_t)count);
  for (int32_t argument = 0; argument < count; ++argument) items[argument] = text_value(argv[argument]);
  return list_from_values(items, (uint32_t)count);
}

int32_t chimi_rt_full_trace(const ChimiRuntime *runtime) { return runtime->full_trace; }
ChimiControl *chimi_rt_direct_integer_success(ChimiRuntime *runtime, uint32_t node_count,
                                               int64_t raw) {
  if (!runtime->silent) runtime->next_span += node_count;
  return success(integer_value(raw));
}

static ChimiValue *select_value(ChimiValue *value, const char *name) {
  RuntimeField *fields = NULL; uint32_t length = 0;
  if (value->kind == VALUE_RECORD) { fields = value->as.record.fields; length = value->as.record.length; }
  else if (value->kind == VALUE_CONSTRUCTED) { fields = value->as.constructed.fields; length = value->as.constructed.length; }
  for (uint32_t index = 0; index < length; ++index)
    if (strcmp(fields[index].name, name) == 0) return fields[index].value;
  return NULL;
}

static uint16_t map_height(const MapNode *node) { return node ? node->height : 0; }
static uint16_t map_max(uint16_t left, uint16_t right) { return left > right ? left : right; }
static int chimi_value_compare(const ChimiValue *left_root, const ChimiValue *right_root);
static MapNode *map_node(ChimiValue *key, ChimiValue *value, MapNode *left, MapNode *right) {
  MapNode *node = allocate(1, sizeof(*node));
  allocation_kind(node, ALLOCATION_MAP_NODE);
  node->key = key; node->value = value; node->left = left; node->right = right;
  node->height = (uint16_t)(1 + map_max(map_height(left), map_height(right)));
  return node;
}
static MapNode *map_rotate_left(MapNode *node) {
  MapNode *right = node->right;
  MapNode *left = map_node(node->key, node->value, node->left, right->left);
  return map_node(right->key, right->value, left, right->right);
}
static MapNode *map_rotate_right(MapNode *node) {
  MapNode *left = node->left;
  MapNode *right = map_node(node->key, node->value, left->right, node->right);
  return map_node(left->key, left->value, left->left, right);
}
static MapNode *map_balance(MapNode *node) {
  int balance = (int)map_height(node->left) - (int)map_height(node->right);
  if (balance > 1) {
    if (map_height(node->left->right) > map_height(node->left->left))
      node = map_node(node->key, node->value, map_rotate_left(node->left), node->right);
    return map_rotate_right(node);
  }
  if (balance < -1) {
    if (map_height(node->right->left) > map_height(node->right->right))
      node = map_node(node->key, node->value, node->left, map_rotate_right(node->right));
    return map_rotate_left(node);
  }
  return node;
}
static MapNode *map_put_node(MapNode *node, ChimiValue *key, ChimiValue *value) {
  if (!node) return map_node(key, value, NULL, NULL);
  int order = chimi_value_compare(key, node->key);
  if (order == 0) return map_node(key, value, node->left, node->right);
  if (order < 0) return map_balance(map_node(node->key, node->value,
                                            map_put_node(node->left, key, value), node->right));
  return map_balance(map_node(node->key, node->value, node->left,
                              map_put_node(node->right, key, value)));
}
static ChimiValue *map_get_node(MapNode *node, const ChimiValue *key) {
  while (node) {
    int order = chimi_value_compare(key, node->key);
    if (order == 0) return node->value;
    node = order < 0 ? node->left : node->right;
  }
  return NULL;
}

/* Structural order: the one comparison behind `==`, `<`, `Order.compare`, and Map
   keys. For ordered types it is the source-level order; for types that are only
   equatable (records, maps, Json.Value) it is an internal total order consistent
   with equality. It mirrors execution/structural.rs. The walk keeps its own work
   stack on the heap, so nesting depth never costs host stack. */
typedef struct { const ChimiValue *left; const ChimiValue *right; int decided; } CompareStep;
typedef struct { CompareStep *steps; size_t length; size_t capacity; } CompareStack;

static void compare_push(CompareStack *stack, const ChimiValue *left, const ChimiValue *right, int decided) {
  if (stack->length == stack->capacity) {
    size_t capacity = stack->capacity ? stack->capacity * 2 : 64;
    CompareStep *steps = realloc(stack->steps, capacity * sizeof(*steps));
    if (steps == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
    stack->steps = steps; stack->capacity = capacity;
  }
  stack->steps[stack->length++] = (CompareStep){left, right, decided};
}
/* A step with no values decides the comparison when every step above it was equal,
   such as the length rule after the shared prefix of two lists. */
static void compare_decide(CompareStack *stack, int decided) { compare_push(stack, NULL, NULL, decided); }
static int order_of(int64_t left, int64_t right) { return (left > right) - (left < right); }

/* A signed decimal integer of any size, as in json.rs: a written exponent may have
   more digits than any machine integer holds and is never clamped or wrapped.
   `digits` are values 0-9, most significant first, with no leading zeros; zero has
   length 0 and is never negative. The buffer is owned. */
typedef struct { bool negative; unsigned char *digits; size_t length; } DecimalExponent;
static DecimalExponent decimal_exponent(bool negative, const unsigned char *digits, size_t length, unsigned char zero) {
  while (length > 0 && *digits == zero) { ++digits; --length; }
  DecimalExponent exponent = {negative && length > 0, malloc(length ? length : 1), length};
  if (exponent.digits == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  for (size_t index = 0; index < length; ++index) exponent.digits[index] = (unsigned char)(digits[index] - zero);
  return exponent;
}
static int magnitude_order(const DecimalExponent *left, const DecimalExponent *right) {
  if (left->length != right->length) return left->length < right->length ? -1 : 1;
  int order = left->length ? memcmp(left->digits, right->digits, left->length) : 0;
  return (order > 0) - (order < 0);
}
static int decimal_exponent_order(const DecimalExponent *left, const DecimalExponent *right) {
  int left_rank = left->negative ? -1 : left->length > 0, right_rank = right->negative ? -1 : right->length > 0;
  if (left_rank != right_rank) return left_rank < right_rank ? -1 : 1;
  return left->negative ? -magnitude_order(left, right) : magnitude_order(left, right);
}
/* left + right as a new value; neither argument is consumed. */
static DecimalExponent decimal_exponent_sum(const DecimalExponent *left, const DecimalExponent *right) {
  int order = magnitude_order(left, right);
  if (left->negative != right->negative && order == 0) return decimal_exponent(false, NULL, 0, 0);
  const DecimalExponent *larger = order >= 0 ? left : right, *smaller = order >= 0 ? right : left;
  bool subtract = left->negative != right->negative;
  size_t length = larger->length + 1;
  unsigned char *result = malloc(length);
  if (result == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  int carry = 0;
  for (size_t place = 0; place < length; ++place) {
    int big = place < larger->length ? larger->digits[larger->length - 1 - place] : 0;
    int small = place < smaller->length ? smaller->digits[smaller->length - 1 - place] : 0;
    int digit = subtract ? big - small - carry : big + small + carry;
    carry = subtract ? digit < 0 : digit > 9;
    result[length - 1 - place] = (unsigned char)(subtract ? digit + 10 * carry : digit - 10 * carry);
  }
  DecimalExponent sum = decimal_exponent(larger->negative, result, length, 0);
  free(result);
  return sum;
}
static DecimalExponent decimal_exponent_of(int64_t value) {
  char text[24]; snprintf(text, sizeof(text), "%" PRIu64, value < 0 ? -(uint64_t)value : (uint64_t)value);
  return decimal_exponent(value < 0, (const unsigned char *)text, strlen(text), '0');
}

/* A JSON number as sign x 0.digits x 10^exponent with no leading or trailing zeros
   in digits; zero has sign 0. */
typedef struct { int sign; const char *digits; size_t length; DecimalExponent exponent; char *owned; } CanonicalNumber;
static CanonicalNumber canonical_number(const char *text) {
  CanonicalNumber number = {0};
  bool negative = *text == '-'; if (negative) ++text;
  size_t mantissa = strcspn(text, "eE");
  DecimalExponent exponent = decimal_exponent(false, NULL, 0, 0);
  if (text[mantissa]) {
    const char *written = text + mantissa + 1; bool negative_exponent = *written == '-';
    if (*written == '-' || *written == '+') ++written;
    free(exponent.digits);
    exponent = decimal_exponent(negative_exponent, (const unsigned char *)written, strlen(written), '0');
  }
  size_t point = strcspn(text, ".eE"); if (point > mantissa) point = mantissa;
  number.owned = malloc(mantissa + 1);
  if (number.owned == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  size_t length = 0;
  for (size_t index = 0; index < mantissa; ++index)
    if (text[index] != '.') number.owned[length++] = text[index];
  size_t leading = 0; while (leading < length && number.owned[leading] == '0') ++leading;
  while (length > leading && number.owned[length - 1] == '0') --length;
  number.exponent = exponent;
  if (length == leading) return number;
  number.sign = negative ? -1 : 1; number.digits = number.owned + leading; number.length = length - leading;
  DecimalExponent offset = decimal_exponent_of((int64_t)point - (int64_t)leading);
  number.exponent = decimal_exponent_sum(&exponent, &offset);
  free(exponent.digits); free(offset.digits);
  return number;
}
static int json_number_compare(const char *left_text, const char *right_text) {
  CanonicalNumber left = canonical_number(left_text), right = canonical_number(right_text);
  int order = order_of(left.sign, right.sign);
  if (order == 0 && left.sign != 0) {
    order = decimal_exponent_order(&left.exponent, &right.exponent);
    if (order == 0) {
      size_t shared = left.length < right.length ? left.length : right.length;
      int digits = memcmp(left.digits, right.digits, shared);
      order = digits != 0 ? (digits > 0) - (digits < 0) : order_of((int64_t)left.length, (int64_t)right.length);
    }
    if (left.sign < 0) order = -order;
  }
  free(left.owned); free(right.owned); free(left.exponent.digits); free(right.exponent.digits);
  return order;
}

static int field_name_order(const void *left, const void *right) {
  return strcmp((*(const RuntimeField *const *)left)->name, (*(const RuntimeField *const *)right)->name);
}
/* Records have no declaration order; name order is a fixed order both sides share. */
static const RuntimeField **fields_by_name(const RuntimeField *fields, uint32_t length) {
  const RuntimeField **sorted = malloc((length ? length : 1) * sizeof(*sorted));
  if (sorted == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  for (uint32_t index = 0; index < length; ++index) sorted[index] = &fields[index];
  qsort(sorted, length, sizeof(*sorted), field_name_order);
  return sorted;
}

typedef struct { const MapNode **nodes; size_t length; } MapEntries;
/* In key order, so equal maps yield equal sequences whatever their insertion history.
   The tree is balanced, so the pending stack never exceeds its height. */
static MapEntries map_entries(const MapNode *root) {
  size_t capacity = 16, length = 0, pending_length = 0;
  const MapNode **nodes = malloc(capacity * sizeof(*nodes)); const MapNode *pending[64];
  if (nodes == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  const MapNode *node = root;
  for (;;) {
    while (node) { pending[pending_length++] = node; node = node->left; }
    if (pending_length == 0) break;
    node = pending[--pending_length];
    if (length == capacity) {
      capacity *= 2; const MapNode **grown = realloc(nodes, capacity * sizeof(*nodes));
      if (grown == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
      nodes = grown;
    }
    nodes[length++] = node; node = node->right;
  }
  return (MapEntries){nodes, length};
}

/* Writing an outcome value as JSON. The writer keeps its own stack of steps, so a
   value of any nesting depth costs heap, not C stack; a step pushed last runs first. */
typedef enum {
  JSON_STEP_VALUE, JSON_STEP_TEXT, JSON_STEP_NAME, JSON_STEP_KEY, JSON_STEP_KEY_END, JSON_STEP_FREE
} JsonStepKind;
typedef struct { JsonStepKind kind; const void *pointer; } JsonStep;
typedef struct { JsonStep *items; size_t length; size_t capacity; } JsonSteps;

static void json_step(JsonSteps *steps, JsonStepKind kind, const void *pointer) {
  if (steps->length == steps->capacity) {
    size_t capacity = steps->capacity ? steps->capacity * 2 : 64;
    JsonStep *items = realloc(steps->items, capacity * sizeof(*items));
    if (!items) exhausted_memory();
    steps->items = items;
    steps->capacity = capacity;
  }
  steps->items[steps->length++] = (JsonStep){kind, pointer};
}

/* `"name":value` pairs, comma-separated, then `close`. */
static void json_push_fields(JsonSteps *steps, const RuntimeField *fields, uint32_t length,
                             bool named, const char *close) {
  json_step(steps, JSON_STEP_TEXT, close);
  for (uint32_t index = length; index-- > 0;) {
    json_step(steps, JSON_STEP_VALUE, fields[index].value);
    if (named) json_step(steps, JSON_STEP_NAME, fields[index].name);
    if (index) json_step(steps, JSON_STEP_TEXT, ",");
  }
}

/* A map value in an outcome record is an object of its entries in key order, as the
   interpreter writes it: a Text key is the member name, and any other key's member
   name is that key's own JSON encoding. */
static void json_push_map(JsonSteps *steps, const MapNode *map) {
  MapEntries entries = map_entries(map);
  json_step(steps, JSON_STEP_FREE, entries.nodes);
  json_step(steps, JSON_STEP_TEXT, "}");
  for (size_t index = entries.length; index-- > 0;) {
    json_step(steps, JSON_STEP_VALUE, entries.nodes[index]->value);
    json_step(steps, JSON_STEP_KEY, entries.nodes[index]);
    if (index) json_step(steps, JSON_STEP_TEXT, ",");
  }
}

/* A non-Text map key's member name is its own JSON encoding, written into a buffer
   of its own and then quoted into the output it interrupted. */
typedef struct { FILE *outer; FILE *buffer; char *name; size_t name_length; } JsonKey;

static void json_value(FILE *output, const ChimiValue *root) {
  JsonSteps steps = {0};
  json_step(&steps, JSON_STEP_VALUE, root);
  while (steps.length) {
    JsonStep step = steps.items[--steps.length];
    switch (step.kind) {
      case JSON_STEP_TEXT: fputs(step.pointer, output); continue;
      case JSON_STEP_NAME: json_string(output, step.pointer); fputc(':', output); continue;
      /* A key's steps go on the same stack, so maps nested through keys of maps
         cost heap, not C stack. */
      case JSON_STEP_KEY: {
        const MapNode *entry = step.pointer;
        if (entry->key->kind == VALUE_TEXT) {
          json_string(output, entry->key->as.text); fputc(':', output); continue;
        }
        JsonKey *key = malloc(sizeof(*key));
        if (!key) exhausted_memory();
        *key = (JsonKey){.outer = output};
        key->buffer = open_memstream(&key->name, &key->name_length);
        if (!key->buffer) exhausted_memory();
        json_step(&steps, JSON_STEP_KEY_END, key);
        json_step(&steps, JSON_STEP_VALUE, entry->key);
        output = key->buffer;
        continue;
      }
      case JSON_STEP_KEY_END: {
        JsonKey *key = (JsonKey *)step.pointer;
        fclose(key->buffer);
        output = key->outer;
        json_string(output, key->name); fputc(':', output);
        free(key->name); free(key);
        continue;
      }
      case JSON_STEP_FREE: free((void *)step.pointer); continue;
      case JSON_STEP_VALUE: break;
    }
    const ChimiValue *value = step.pointer;
    switch (value->kind) {
      case VALUE_VOID: fputs("{\"type\":\"Void\"}", output); break;
      case VALUE_INTEGER: fprintf(output, "%" PRId64, value->as.integer); break;
      case VALUE_FLOAT: { char text[40]; float_text(value->as.floating, text); fputs(text, output); break; }
      case VALUE_TEXT: json_string(output, value->as.text); break;
      case VALUE_BOOLEAN: fputs(value->as.boolean ? "true" : "false", output); break;
      case VALUE_LIST:
        fputc('[', output);
        json_step(&steps, JSON_STEP_TEXT, "]");
        for (uint32_t index = list_length(value); index-- > 0;) {
          json_step(&steps, JSON_STEP_VALUE, list_get(value, index));
          if (index) json_step(&steps, JSON_STEP_TEXT, ",");
        }
        break;
      case VALUE_RECORD:
        fputc('{', output);
        json_push_fields(&steps, value->as.record.fields, value->as.record.length, true, "}");
        break;
      case VALUE_DURATION: fprintf(output, "{\"duration\":%" PRId64 "}", value->as.integer); break;
      case VALUE_RANGE:
        fprintf(output, "{\"range\":{\"start\":%" PRId64 ",\"end\":%" PRId64 "}}",
                value->as.range.start, value->as.range.end); break;
      case VALUE_TUPLE:
        fputs("{\"tuple\":[", output);
        json_push_fields(&steps, value->as.record.fields, value->as.record.length, false, "]}");
        break;
      case VALUE_CONSTRUCTED:
        fputs("{\"type\":", output); json_string(output, value->as.constructed.type_name);
        if (value->as.constructed.variant_name) {
          fputs(",\"variant\":", output); json_string(output, value->as.constructed.variant_name);
        }
        fputs(",\"fields\":{", output);
        json_push_fields(&steps, value->as.constructed.fields, value->as.constructed.length, true, "}}");
        break;
      case VALUE_CALLABLE:
        fputs("{\"callable\":", output); json_string(output, value->as.callable->name);
        fputc('}', output); break;
      case VALUE_MAP:
        fputc('{', output);
        json_push_map(&steps, value->as.map);
        break;
      case VALUE_JSON_NUMBER: json_string(output, value->as.json_number); break;
      case VALUE_TASK: fprintf(output, "{\"task\":%" PRIu64 "}", value->as.task); break;
      case VALUE_CHANNEL_SENDER:
      case VALUE_CHANNEL_RECEIVER:
      case VALUE_CHANNEL_COMPLETION:
        fprintf(output, "{\"channel\":%" PRIu64 "}", value->as.capability.channel); break;
      /* Opaque, as in the interpreter's public form: only what the value is. */
      case VALUE_PROCESS_ADDRESS: fputs("{\"type\":\"Address\",\"fields\":{}}", output); break;
      case VALUE_PROCESS_INBOX: fputs("{\"type\":\"Inbox\",\"fields\":{}}", output); break;
      case VALUE_SUPERVISOR_RUNNING: fputs("{\"type\":\"Running\",\"fields\":{}}", output); break;
      case VALUE_MANAGED_PROCESS: fputs("{\"type\":\"ManagedProcess\",\"fields\":{}}", output); break;
      case VALUE_MANAGED_SUPERVISOR:
        fputs("{\"type\":\"ManagedSupervisor\",\"fields\":{}}", output); break;
    }
  }
  free(steps.items);
}

static uint32_t constructor_ordinal(uint32_t symbol) {
  return CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[symbol].constructor].ordinal;
}

/* Requires: both values have the same equatable type. Returns -1, 0, or 1. */
static int chimi_value_compare(const ChimiValue *left_root, const ChimiValue *right_root) {
  CompareStack stack = {0}; int order = 0;
  compare_push(&stack, left_root, right_root, 0);
  while (order == 0 && stack.length != 0) {
    CompareStep step = stack.steps[--stack.length];
    if (step.left == NULL) { order = step.decided; continue; }
    const ChimiValue *left = step.left, *right = step.right;
    if (left == right) continue;
    if (left->kind != right->kind) { order = order_of(left->kind, right->kind); continue; }
    switch (left->kind) {
      case VALUE_VOID: break;
      case VALUE_INTEGER:
      case VALUE_DURATION: order = order_of(left->as.integer, right->as.integer); break;
      /* Finite, so total; -0.0 and 0.0 are equal. */
      case VALUE_FLOAT: order = (left->as.floating > right->as.floating) - (left->as.floating < right->as.floating); break;
      case VALUE_BOOLEAN: order = order_of(left->as.boolean, right->as.boolean); break;
      case VALUE_TEXT: { int bytes = strcmp(left->as.text, right->as.text); order = (bytes > 0) - (bytes < 0); break; }
      case VALUE_JSON_NUMBER: order = json_number_compare(left->as.json_number, right->as.json_number); break;
      case VALUE_LIST: {
        uint32_t left_length = list_length(left), right_length = list_length(right);
        uint32_t shared = left_length < right_length ? left_length : right_length;
        compare_decide(&stack, order_of(left_length, right_length));
        for (uint32_t index = shared; index-- > 0;)
          compare_push(&stack, list_get(left, index), list_get(right, index), 0);
        break;
      }
      case VALUE_RECORD: {
        uint32_t length = left->as.record.length < right->as.record.length ? left->as.record.length : right->as.record.length;
        const RuntimeField **left_fields = fields_by_name(left->as.record.fields, length);
        const RuntimeField **right_fields = fields_by_name(right->as.record.fields, length);
        for (uint32_t index = length; index-- > 0;)
          compare_push(&stack, left_fields[index]->value, right_fields[index]->value, 0);
        free(left_fields); free(right_fields);
        break;
      }
      /* Normalized bounds, so 0..3 == 0..=2; empty ranges keep their bounds. */
      case VALUE_RANGE:
        order = order_of(left->as.range.start, right->as.range.start);
        if (order == 0) order = order_of(left->as.range.end, right->as.range.end);
        break;
      /* Typing gives both tuples one arity; elements compare left to right. */
      case VALUE_TUPLE:
        for (uint32_t index = left->as.record.length; index-- > 0;)
          compare_push(&stack, left->as.record.fields[index].value, right->as.record.fields[index].value, 0);
        break;
      case VALUE_CONSTRUCTED: {
        order = order_of(constructor_ordinal(left->as.constructed.constructor),
                         constructor_ordinal(right->as.constructed.constructor));
        if (order != 0) break;
        uint32_t length = left->as.constructed.length;
        for (uint32_t index = length; index-- > 0;)
          compare_push(&stack, left->as.constructed.fields[index].value, right->as.constructed.fields[index].value, 0);
        break;
      }
      case VALUE_MAP: {
        MapEntries left_entries = map_entries(left->as.map), right_entries = map_entries(right->as.map);
        size_t shared = left_entries.length < right_entries.length ? left_entries.length : right_entries.length;
        compare_decide(&stack, order_of((int64_t)left_entries.length, (int64_t)right_entries.length));
        for (size_t index = shared; index-- > 0;) {
          compare_push(&stack, left_entries.nodes[index]->value, right_entries.nodes[index]->value, 0);
          compare_push(&stack, left_entries.nodes[index]->key, right_entries.nodes[index]->key, 0);
        }
        free(left_entries.nodes); free(right_entries.nodes);
        break;
      }
      /* Typing admits no comparison of these; identity keeps the order total. */
      case VALUE_CALLABLE: case VALUE_TASK: case VALUE_CHANNEL_SENDER:
      case VALUE_CHANNEL_RECEIVER: case VALUE_CHANNEL_COMPLETION:
      case VALUE_PROCESS_INBOX: case VALUE_SUPERVISOR_RUNNING: case VALUE_MANAGED_PROCESS:
      case VALUE_MANAGED_SUPERVISOR:
        order = order_of((int64_t)(uintptr_t)left, (int64_t)(uintptr_t)right); break;
      /* Routing kind, then the name or incarnation it routes to. */
      case VALUE_PROCESS_ADDRESS:
        order = order_of(left->as.address.kind, right->as.address.kind);
        if (order == 0)
          order = order_of((int64_t)left->as.address.target, (int64_t)right->as.address.target);
        break;
    }
  }
  free(stack.steps);
  return order;
}

static uint32_t standard_constructor_symbol(uint32_t standard_constructor) {
  for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
    if (CHIMI_SYMBOLS[symbol].standard_constructor == standard_constructor) return symbol;
  return UINT32_MAX;
}
static ChimiValue *standard_value(uint32_t standard_constructor, ChimiValue **values,
                                  uint32_t value_count) {
  uint32_t symbol = standard_constructor_symbol(standard_constructor);
  if (symbol == UINT32_MAX) return NULL;
  const ChimiConstructor *shape = &CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[symbol].constructor];
  if (shape->fields_length != value_count) return NULL;
  ChimiValue *value = new_value(VALUE_CONSTRUCTED);
  value->as.constructed.constructor = symbol;
  value->as.constructed.type_name = CHIMI_STRINGS[shape->type_name];
  value->as.constructed.variant_name = shape->variant_name == UINT32_MAX ? NULL : CHIMI_STRINGS[shape->variant_name];
  value->as.constructed.length = value_count;
  value->as.constructed.fields = allocate(value_count, sizeof(*value->as.constructed.fields));
  allocation_kind(value->as.constructed.fields, ALLOCATION_FIELDS);
  for (uint32_t field = 0; field < value_count; ++field)
    value->as.constructed.fields[field] = (RuntimeField){
      CHIMI_STRINGS[CHIMI_IDS[shape->fields_offset + field]], values[field]
    };
  return value;
}

typedef struct { char *data; size_t length; size_t capacity; } TextBuffer;
static void buffer_reserve(TextBuffer *buffer, size_t extra) {
  if (buffer->length + extra + 1 <= buffer->capacity) return;
  size_t capacity = buffer->capacity ? buffer->capacity : 64;
  while (capacity < buffer->length + extra + 1) capacity *= 2;
  char *data = reallocate_bytes(buffer->data, buffer->capacity, capacity);
  buffer->data = data; buffer->capacity = capacity;
}
static void buffer_bytes(TextBuffer *buffer, const char *value, size_t length) {
  buffer_reserve(buffer, length); memcpy(buffer->data + buffer->length, value, length);
  buffer->length += length; buffer->data[buffer->length] = '\0';
}
static void buffer_char(TextBuffer *buffer, char value) { buffer_bytes(buffer, &value, 1); }
static void buffer_json_string(TextBuffer *buffer, const char *value) {
  buffer_char(buffer, '"');
  for (const unsigned char *cursor = (const unsigned char *)value; *cursor; ++cursor) {
    switch (*cursor) {
      case '"': buffer_bytes(buffer, "\\\"", 2); break;
      case '\\': buffer_bytes(buffer, "\\\\", 2); break;
      case '\b': buffer_bytes(buffer, "\\b", 2); break;
      case '\f': buffer_bytes(buffer, "\\f", 2); break;
      case '\n': buffer_bytes(buffer, "\\n", 2); break;
      case '\r': buffer_bytes(buffer, "\\r", 2); break;
      case '\t': buffer_bytes(buffer, "\\t", 2); break;
      default:
        if (*cursor < 0x20) {
          char escaped[7]; snprintf(escaped, sizeof(escaped), "\\u%04x", *cursor);
          buffer_bytes(buffer, escaped, 6);
        } else buffer_char(buffer, (char)*cursor);
    }
  }
  buffer_char(buffer, '"');
}

/* `Json.encode`. It keeps its own stack of steps, as the outcome writer does, so a
   program-built Json.Value of any nesting depth costs heap, not C stack. A step
   pushed last runs first: TEXT is a literal, KEY is an Object entry's member name
   and `:`, and FREE releases an entry array once its entries are written. Returns
   false, having freed what it queued, when the value is not a well-formed
   Json.Value or an Object key is not Text. */
static bool encode_json_step(TextBuffer *buffer, JsonSteps *steps, const ChimiValue *value) {
  if (value->kind != VALUE_CONSTRUCTED) return false;
  uint32_t kind = CHIMI_SYMBOLS[value->as.constructed.constructor].standard_constructor;
  ChimiValue *field = value->as.constructed.length ? value->as.constructed.fields[0].value : NULL;
  switch (kind) {
    case CHIMI_CONSTRUCTOR_JSON_NULL: buffer_bytes(buffer, "null", 4); return true;
    case CHIMI_CONSTRUCTOR_JSON_BOOLEAN:
      if (!field || field->kind != VALUE_BOOLEAN) return false;
      buffer_bytes(buffer, field->as.boolean ? "true" : "false", field->as.boolean ? 4 : 5); return true;
    case CHIMI_CONSTRUCTOR_JSON_NUMBER:
      if (!field || field->kind != VALUE_JSON_NUMBER) return false;
      buffer_bytes(buffer, field->as.json_number, strlen(field->as.json_number)); return true;
    case CHIMI_CONSTRUCTOR_JSON_TEXT:
      if (!field || field->kind != VALUE_TEXT) return false;
      buffer_json_string(buffer, field->as.text); return true;
    case CHIMI_CONSTRUCTOR_JSON_ARRAY:
      if (!field || field->kind != VALUE_LIST) return false;
      buffer_char(buffer, '[');
      json_step(steps, JSON_STEP_TEXT, "]");
      for (uint32_t index = list_length(field); index-- > 0;) {
        json_step(steps, JSON_STEP_VALUE, list_get(field, index));
        if (index) json_step(steps, JSON_STEP_TEXT, ",");
      }
      return true;
    case CHIMI_CONSTRUCTOR_JSON_OBJECT: {
      if (!field || field->kind != VALUE_MAP) return false;
      buffer_char(buffer, '{');
      MapEntries entries = map_entries(field->as.map);
      json_step(steps, JSON_STEP_FREE, entries.nodes);
      json_step(steps, JSON_STEP_TEXT, "}");
      for (size_t index = entries.length; index-- > 0;) {
        json_step(steps, JSON_STEP_VALUE, entries.nodes[index]->value);
        json_step(steps, JSON_STEP_KEY, entries.nodes[index]);
        if (index) json_step(steps, JSON_STEP_TEXT, ",");
      }
      return true;
    }
    default: return false;
  }
}
static bool encode_json_value(TextBuffer *buffer, const ChimiValue *root) {
  JsonSteps steps = {0};
  json_step(&steps, JSON_STEP_VALUE, root);
  bool encoded = true;
  while (steps.length) {
    JsonStep step = steps.items[--steps.length];
    if (!encoded) {
      if (step.kind == JSON_STEP_FREE) free((void *)step.pointer);
      continue;
    }
    switch (step.kind) {
      case JSON_STEP_TEXT: buffer_bytes(buffer, step.pointer, strlen(step.pointer)); break;
      case JSON_STEP_KEY: {
        const MapNode *entry = step.pointer;
        if (entry->key->kind != VALUE_TEXT) { encoded = false; break; }
        buffer_json_string(buffer, entry->key->as.text); buffer_char(buffer, ':');
        break;
      }
      case JSON_STEP_FREE: free((void *)step.pointer); break;
      case JSON_STEP_VALUE: encoded = encode_json_step(buffer, &steps, step.pointer); break;
      /* The outcome writer's steps; never queued here. */
      case JSON_STEP_NAME:
      case JSON_STEP_KEY_END: encoded = false; break;
    }
  }
  free(steps.items);
  return encoded;
}

typedef struct { const char *source; size_t length; size_t offset; const char *message; } JsonParser;
static void json_skip_space(JsonParser *parser) {
  while (parser->offset < parser->length &&
         strchr(" \t\r\n", parser->source[parser->offset])) ++parser->offset;
}
static bool json_hex(char value, uint32_t *digit) {
  if (value >= '0' && value <= '9') *digit = (uint32_t)(value - '0');
  else if (value >= 'a' && value <= 'f') *digit = (uint32_t)(value - 'a' + 10);
  else if (value >= 'A' && value <= 'F') *digit = (uint32_t)(value - 'A' + 10);
  else return false;
  return true;
}
/* Each decode error carries the message and byte offset that the reference
   interpreter's parser (compiler/chimi/src/json.rs) reports for the same input,
   so DecodeError.InvalidJson is identical on both backends. */
static void *json_fail(JsonParser *parser, size_t offset, const char *message) {
  parser->offset = offset; parser->message = message; return NULL;
}
/* Reads four hex digits; on failure leaves the offset at the first digit. */
static bool json_code_unit(JsonParser *parser, uint32_t *value) {
  size_t start = parser->offset;
  if (start + 4 > parser->length) { json_fail(parser, start, "incomplete JSON Unicode escape"); return false; }
  *value = 0;
  for (uint32_t index = 0; index < 4; ++index) {
    uint32_t digit;
    if (!json_hex(parser->source[start + index], &digit)) {
      json_fail(parser, start, "invalid JSON Unicode escape"); return false;
    }
    *value = *value * 16 + digit;
  }
  parser->offset = start + 4;
  return true;
}
static void buffer_utf8(TextBuffer *buffer, uint32_t value) {
  if (value <= 0x7f) buffer_char(buffer, (char)value);
  else if (value <= 0x7ff) {
    buffer_char(buffer, (char)(0xc0 | value >> 6)); buffer_char(buffer, (char)(0x80 | (value & 0x3f)));
  } else if (value <= 0xffff) {
    buffer_char(buffer, (char)(0xe0 | value >> 12)); buffer_char(buffer, (char)(0x80 | ((value >> 6) & 0x3f)));
    buffer_char(buffer, (char)(0x80 | (value & 0x3f)));
  } else {
    buffer_char(buffer, (char)(0xf0 | value >> 18)); buffer_char(buffer, (char)(0x80 | ((value >> 12) & 0x3f)));
    buffer_char(buffer, (char)(0x80 | ((value >> 6) & 0x3f))); buffer_char(buffer, (char)(0x80 | (value & 0x3f)));
  }
}
static char *json_parse_string(JsonParser *parser) {
  if (parser->offset >= parser->length || parser->source[parser->offset++] != '"') return NULL;
  TextBuffer buffer = {0};
  while (parser->offset < parser->length) {
    unsigned char value = (unsigned char)parser->source[parser->offset];
    if (value < 0x20) return json_fail(parser, parser->offset, "unescaped control character in JSON string");
    ++parser->offset;
    if (value == '"') return buffer.data ? buffer.data : allocate(1, 1);
    if (value != '\\') { buffer_char(&buffer, (char)value); continue; }
    if (parser->offset >= parser->length) return json_fail(parser, parser->offset, "unterminated JSON escape");
    char escaped = parser->source[parser->offset++];
    switch (escaped) {
      case '"': case '\\': case '/': buffer_char(&buffer, escaped); break;
      case 'b': buffer_char(&buffer, '\b'); break; case 'f': buffer_char(&buffer, '\f'); break;
      case 'n': buffer_char(&buffer, '\n'); break; case 'r': buffer_char(&buffer, '\r'); break;
      case 't': buffer_char(&buffer, '\t'); break;
      case 'u': {
        static const char *const unpaired = "high surrogate must be followed by a low surrogate";
        uint32_t code;
        if (!json_code_unit(parser, &code)) return NULL;
        if (code >= 0xd800 && code <= 0xdbff) {
          if (parser->offset >= parser->length || parser->source[parser->offset] != '\\')
            return json_fail(parser, parser->offset, unpaired);
          if (parser->offset + 1 >= parser->length || parser->source[parser->offset + 1] != 'u')
            return json_fail(parser, parser->offset + 1, unpaired);
          parser->offset += 2; uint32_t low;
          if (!json_code_unit(parser, &low)) return NULL;
          if (low < 0xdc00 || low > 0xdfff) return json_fail(parser, parser->offset, unpaired);
          code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
        } else if (code >= 0xdc00 && code <= 0xdfff) {
          return json_fail(parser, parser->offset, "isolated low surrogate in JSON string");
        }
        buffer_utf8(&buffer, code); break;
      }
      default: return json_fail(parser, parser->offset, "invalid JSON escape");
    }
  }
  return json_fail(parser, parser->offset, "unterminated JSON string");
}

static ChimiValue *json_parse_value(JsonParser *parser, uint32_t depth);
static ChimiValue *json_parse_array(JsonParser *parser, uint32_t depth) {
  ++parser->offset; json_skip_space(parser);
  ChimiValue **items = NULL; uint32_t length = 0, capacity = 0;
  if (parser->offset < parser->length && parser->source[parser->offset] == ']') ++parser->offset;
  else for (;;) {
    ChimiValue *item = json_parse_value(parser, depth + 1); if (!item) return NULL;
    if (length == capacity) {
      uint32_t old_capacity = capacity;
      capacity = capacity ? capacity * 2 : 4;
      items = reallocate_bytes(items, old_capacity * sizeof(*items), capacity * sizeof(*items));
      allocation_kind(items, ALLOCATION_VALUE_POINTERS);
    }
    items[length++] = item; json_skip_space(parser);
    char separator = parser->offset < parser->length ? parser->source[parser->offset] : '\0';
    if (separator != ']' && separator != ',')
      return json_fail(parser, parser->offset, "expected `,` or `]` in JSON array");
    ++parser->offset; if (separator == ']') break;
    json_skip_space(parser);
  }
  ChimiValue *fields[] = {list_from_values(items, length)};
  return standard_value(CHIMI_CONSTRUCTOR_JSON_ARRAY, fields, 1);
}
static ChimiValue *json_parse_object(JsonParser *parser, uint32_t depth) {
  ++parser->offset; json_skip_space(parser); MapNode *map = NULL;
  if (parser->offset < parser->length && parser->source[parser->offset] == '}') ++parser->offset;
  else for (;;) {
    if (parser->offset >= parser->length || parser->source[parser->offset] != '"')
      return json_fail(parser, parser->offset, "expected a string key in JSON object");
    size_t key_offset = parser->offset;
    char *key_text = json_parse_string(parser); if (!key_text) return NULL;
    ChimiValue *key = text_value(key_text);
    if (map_get_node(map, key)) {
      TextBuffer message = {0};
      static const char prefix[] = "duplicate JSON object key `";
      buffer_bytes(&message, prefix, sizeof(prefix) - 1);
      buffer_bytes(&message, key_text, strlen(key_text)); buffer_char(&message, '`');
      return json_fail(parser, key_offset, message.data);
    }
    json_skip_space(parser);
    if (parser->offset >= parser->length || parser->source[parser->offset] != ':')
      return json_fail(parser, parser->offset, "expected `:` after JSON object key");
    ++parser->offset;
    ChimiValue *item = json_parse_value(parser, depth + 1); if (!item) return NULL;
    map = map_put_node(map, key, item); json_skip_space(parser);
    char separator = parser->offset < parser->length ? parser->source[parser->offset] : '\0';
    if (separator != '}' && separator != ',')
      return json_fail(parser, parser->offset, "expected `,` or `}` in JSON object");
    ++parser->offset; if (separator == '}') break;
    json_skip_space(parser);
  }
  ChimiValue *fields_value = map_value(map); ChimiValue *fields[] = {fields_value};
  return standard_value(CHIMI_CONSTRUCTOR_JSON_OBJECT, fields, 1);
}
static ChimiValue *json_parse_number(JsonParser *parser) {
  size_t start = parser->offset;
  if (parser->source[parser->offset] == '-') ++parser->offset;
  if (parser->offset >= parser->length) return json_fail(parser, parser->offset, "expected digits in JSON number");
  if (parser->source[parser->offset] == '0') {
    ++parser->offset;
    if (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9')
      return json_fail(parser, parser->offset, "JSON numbers cannot contain leading zeroes");
  } else if (parser->source[parser->offset] >= '1' && parser->source[parser->offset] <= '9')
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
  else return json_fail(parser, parser->offset, "expected digits in JSON number");
  if (parser->offset < parser->length && parser->source[parser->offset] == '.') {
    ++parser->offset; size_t digits = parser->offset;
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
    if (digits == parser->offset) return json_fail(parser, parser->offset, "expected digits after JSON decimal point");
  }
  if (parser->offset < parser->length && (parser->source[parser->offset] == 'e' || parser->source[parser->offset] == 'E')) {
    ++parser->offset; if (parser->offset < parser->length && (parser->source[parser->offset] == '+' || parser->source[parser->offset] == '-')) ++parser->offset;
    size_t digits = parser->offset;
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
    if (digits == parser->offset) return json_fail(parser, parser->offset, "expected digits in JSON exponent");
  }
  size_t length = parser->offset - start; char *number = allocate(length + 1, 1);
  memcpy(number, parser->source + start, length); ChimiValue *raw = json_number_value(number);
  ChimiValue *fields[] = {raw}; return standard_value(CHIMI_CONSTRUCTOR_JSON_NUMBER, fields, 1);
}
static ChimiValue *json_parse_value(JsonParser *parser, uint32_t depth) {
  json_skip_space(parser);
  if (depth > 128) return json_fail(parser, parser->offset, "JSON nesting exceeds the configured limit");
  if (parser->offset >= parser->length) return json_fail(parser, parser->offset, "expected a JSON value");
  char value = parser->source[parser->offset];
  if (value == 'n' && parser->length - parser->offset >= 4 && memcmp(parser->source + parser->offset, "null", 4) == 0) {
    parser->offset += 4; return standard_value(CHIMI_CONSTRUCTOR_JSON_NULL, NULL, 0);
  }
  if ((value == 't' && parser->length - parser->offset >= 4 && memcmp(parser->source + parser->offset, "true", 4) == 0) ||
      (value == 'f' && parser->length - parser->offset >= 5 && memcmp(parser->source + parser->offset, "false", 5) == 0)) {
    bool raw = value == 't'; parser->offset += raw ? 4 : 5; ChimiValue *boolean = boolean_value(raw);
    ChimiValue *fields[] = {boolean}; return standard_value(CHIMI_CONSTRUCTOR_JSON_BOOLEAN, fields, 1);
  }
  if (value == '"') { char *text = json_parse_string(parser); if (!text) return NULL;
    ChimiValue *raw = text_value(text); ChimiValue *fields[] = {raw}; return standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1); }
  if (value == '[') return json_parse_array(parser, depth);
  if (value == '{') return json_parse_object(parser, depth);
  if (value == '-' || (value >= '0' && value <= '9')) return json_parse_number(parser);
  if (value == 'n' || value == 't' || value == 'f') return json_fail(parser, parser->offset, "invalid JSON literal");
  return json_fail(parser, parser->offset, "expected a JSON value");
}

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment);

static ChimiControl *invoke_intrinsic(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment);

static char *copy_bytes(const unsigned char *source, size_t length) {
  char *copy = allocate(length + 1, 1);
  memcpy(copy, source, length);
  return copy;
}

static bool valid_utf8(const unsigned char *text, size_t length) {
  size_t offset = 0;
  while (offset < length) {
    unsigned char first = text[offset++];
    if (first <= 0x7f) {
      if (first == 0) return false;
      continue;
    }
    if (first >= 0xc2 && first <= 0xdf) {
      if (offset >= length || (text[offset++] & 0xc0) != 0x80) return false;
      continue;
    }
    if (first >= 0xe0 && first <= 0xef) {
      if (offset + 1 >= length) return false;
      unsigned char second = text[offset++], third = text[offset++];
      if ((third & 0xc0) != 0x80) return false;
      if (first == 0xe0) { if (second < 0xa0 || second > 0xbf) return false; }
      else if (first == 0xed) { if (second < 0x80 || second > 0x9f) return false; }
      else if ((second & 0xc0) != 0x80) return false;
      continue;
    }
    if (first >= 0xf0 && first <= 0xf4) {
      if (offset + 2 >= length) return false;
      unsigned char second = text[offset++], third = text[offset++], fourth = text[offset++];
      if ((third & 0xc0) != 0x80 || (fourth & 0xc0) != 0x80) return false;
      if (first == 0xf0) { if (second < 0x90 || second > 0xbf) return false; }
      else if (first == 0xf4) { if (second < 0x80 || second > 0x8f) return false; }
      else if ((second & 0xc0) != 0x80) return false;
      continue;
    }
    return false;
  }
  return true;
}

static ChimiControl *host_file_message(uint32_t constructor, const char *path,
                                       const char *message) {
  ChimiValue *fields[] = {
    text_value(path),
    text_value(message ? message : "unknown I/O error"),
  };
  return failure(standard_value(constructor, fields, 2));
}

static ChimiControl *invoke_line_callback(ChimiRuntime *runtime, ChimiCallable *callback,
                                          ChimiValue *state, unsigned char *bytes, size_t length,
                                          uint64_t line, const char *path, uint64_t span,
                                          uint32_t call_node, ChimiEnvironment *environment) {
  if (!valid_utf8(bytes, length)) {
    ChimiValue *fields[] = {text_value(path), integer_value((int64_t)line)};
    return failure(standard_value(CHIMI_CONSTRUCTOR_UTF8_INVALID, fields, 2));
  }
  ChimiValue *arguments[] = {state, text_value(copy_bytes(bytes, length))};
  return invoke(runtime, callback, arguments, 2, span, call_node, environment);
}

static const char *const FOREIGN_POINTER_DEFECT =
  "Chimi runtime heap invariant violated: a value points into another heap";

/* Ends a reclaiming scope, keeping only what `control` reaches, and returns the
   control the scope's owner should produce: `control`, or a defect when the scope
   reached into another heap. Only an outermost scope reports to the runtime,
   because a nested scope's statistics already merge into its parent. */
static ChimiControl *end_measured_allocation_scope(ChimiRuntime *runtime, AllocationScope *scope,
                                                   ChimiControl *control) {
  bool top_scope = scope->parent == NULL;
  HeapStatus status = allocation_scope_end(scope, NULL, control);
  if (top_scope) {
    runtime->scoped_total_allocated_bytes += scope->total_allocated_bytes;
    for (uint32_t kind = 0; kind < ALLOCATION_KIND_COUNT; ++kind)
      runtime->scoped_allocated_bytes_by_kind[kind] += scope->allocated_bytes_by_kind[kind];
    if (scope->peak_live_bytes > runtime->scoped_peak_live_bytes)
      runtime->scoped_peak_live_bytes = scope->peak_live_bytes;
    runtime->scoped_retained_bytes += scope->live_bytes;
    runtime->scoped_collections += scope->collections;
    runtime->scoped_full_collections += scope->full_collections;
    runtime->scoped_scanned_allocations += scope->scanned_allocations;
  }
  return status == HEAP_OK ? control : defect(FOREIGN_POINTER_DEFECT, -1);
}

static ChimiControl *process_utf8_lines(ChimiRuntime *runtime, ChimiValue **arguments,
                                        uint32_t argument_count, uint64_t span,
                                        uint32_t call_node, ChimiEnvironment *environment) {
  if (argument_count != 3 || arguments[0]->kind != VALUE_TEXT ||
      arguments[2]->kind != VALUE_CALLABLE)
    return defect("verified host function has invalid runtime arguments", call_node);
  if (runtime->cancel) return cancelled();
  const char *path = arguments[0]->as.text;
  bool owns_file = strcmp(path, "-") != 0;
  FILE *input = owns_file ? fopen(path, "rb") : stdin;
  if (!input) return host_file_message(CHIMI_CONSTRUCTOR_FILE_OPEN, path, strerror(errno));
  unsigned char *line = allocate((size_t)runtime->max_line_bytes + 1, 1);
  size_t length = 0;
  uint64_t line_number = 1;
  ChimiValue *state = arguments[1];
  ChimiControl *control = NULL;
  AllocationScope allocation_scope;
  allocation_scope_begin(&allocation_scope);
  for (;;) {
    if (runtime->cancel) {
      control = cancelled();
      allocation_scope_collect(&allocation_scope, NULL, control);
      break;
    }
    errno = 0;
    int byte = fgetc(input);
    if (byte == EOF) {
      if (ferror(input)) {
        control = runtime->cancel && errno == EINTR
          ? cancelled()
          : host_file_message(CHIMI_CONSTRUCTOR_FILE_READ, path, strerror(errno));
        allocation_scope_collect(&allocation_scope, NULL, control);
      } else if (length > 0) {
        control = invoke_line_callback(runtime, arguments[2]->as.callable, state, line, length,
                                       line_number, path, span, call_node, environment);
        allocation_scope_collect(&allocation_scope, NULL, control);
      } else {
        control = success(state);
        allocation_scope_collect(&allocation_scope, NULL, control);
      }
      break;
    }
    if (byte == '\n') {
      size_t content_length = length > 0 && line[length - 1] == '\r' ? length - 1 : length;
      control = invoke_line_callback(runtime, arguments[2]->as.callable, state, line,
                                     content_length, line_number, path, span, call_node,
                                     environment);
      if (control->kind != CONTROL_SUCCESS) {
        allocation_scope_collect(&allocation_scope, NULL, control);
        break;
      }
      state = control->value;
      allocation_scope_collect(&allocation_scope, state, NULL);
      length = 0;
      ++line_number;
      continue;
    }
    if (length == runtime->max_line_bytes) {
      ChimiValue *fields[] = {text_value(path), integer_value(runtime->max_line_bytes)};
      control = failure(standard_value(CHIMI_CONSTRUCTOR_FILE_LINE_TOO_LONG, fields, 2));
      allocation_scope_collect(&allocation_scope, NULL, control);
      break;
    }
    line[length++] = (unsigned char)byte;
  }
  if (owns_file && fclose(input) != 0 &&
      (control->kind == CONTROL_SUCCESS || control->kind == CONTROL_FAILURE)) {
    control = host_file_message(CHIMI_CONSTRUCTOR_FILE_CLOSE, path, strerror(errno));
    allocation_scope_collect(&allocation_scope, NULL, control);
  }
  return end_measured_allocation_scope(runtime, &allocation_scope, control);
}

static ChimiControl *invoke_host(ChimiRuntime *runtime, HostFunction function,
                                 ChimiValue **arguments, uint32_t argument_count,
                                 uint64_t span, uint32_t call_node,
                                 ChimiEnvironment *environment) {
  if (function == HOST_PROCESS_UTF8_LINES)
    return process_utf8_lines(runtime, arguments, argument_count, span, call_node, environment);
  if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT)
    return defect("verified host function has invalid runtime arguments", call_node);
  if (runtime->cancel) return cancelled();
  FILE *output = function == HOST_WRITE_LINE ? stdout : stderr;
  if (fputs(arguments[0]->as.text, output) < 0 || fputc('\n', output) == EOF || fflush(output) != 0) {
    const char *stream = function == HOST_WRITE_LINE ? "stdout" : "stderr";
    ChimiValue *fields[] = {text_value(stream), text_value(strerror(errno))};
    return failure(standard_value(CHIMI_CONSTRUCTOR_CONSOLE_WRITE_FAILED, fields, 2));
  }
  return runtime->cancel ? cancelled() : success(void_value());
}

static ChimiControl *construct_value(uint32_t symbol, ChimiValue **values);

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment) {
  if (callable->kind == CALL_CONSTRUCTOR) return construct_value(callable->symbol, arguments);
  if (callable->kind == CALL_DEPENDENCY) {
    if (!runtime->silent) {
      uint64_t dependency_span = runtime->next_span++;
      emit_dependency(runtime, call_node, dependency_span, span, callable);
    }
    return invoke(runtime, callable->inner, arguments, argument_count, span, call_node, call_environment);
  }
  if (callable->kind == CALL_INTRINSIC)
    return invoke_intrinsic(runtime, callable->intrinsic, arguments, argument_count,
                            span, call_node, call_environment);
  if (callable->kind == CALL_HOST)
    return invoke_host(runtime, callable->host, arguments, argument_count,
                       span, call_node, call_environment);
  ChimiEnvironment *environment;
  if (callable->kind == CALL_DECLARED) {
    // Declared functions capture provisioned requirements, but never local
    // bindings. Build a clean local table for parameters while retaining the
    // requirement snapshot taken when the function became a value.
    environment = new_environment();
    if (CHIMI_REQUIREMENT_SLOT_COUNT != 0)
      memcpy(environment->requirements, callable->environment->requirements,
             CHIMI_REQUIREMENT_SLOT_COUNT * sizeof(*environment->requirements));
  } else {
    environment = clone_environment(callable->environment);
  }
  for (uint32_t slot = 0; slot < CHIMI_REQUIREMENT_SLOT_COUNT; ++slot)
    if (call_environment->requirements[slot] != NULL)
      environment->requirements[slot] = call_environment->requirements[slot];
  uint32_t count = callable->parameters_length < argument_count ? callable->parameters_length : argument_count;
  for (uint32_t index = 0; index < count; ++index)
    environment->locals[environment_slot(CHIMI_IDS[callable->parameters_offset + index])] = arguments[index];
  return run_node(callable->body, runtime, environment, span);
}

static ChimiFiberResult task_fiber_result(ChimiControl *control) {
  if (control->kind == CONTROL_CANCEL)
    return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
  if (control->kind == CONTROL_DEFECT)
    return (ChimiFiberResult){
      .state = CHIMI_FIBER_DEFECTED,
      .defect_code = (int64_t)(intptr_t)control,
    };
  return (ChimiFiberResult){
    .state = CHIMI_FIBER_SUCCEEDED,
    .value = (int64_t)(intptr_t)control,
  };
}

/* A child's result outlives its heap as a fragment. The scheduler keeps it with the
   task record, and every Task.result copies it into the caller's heap. The scheduler
   can also hand a child's defect code to the root, so a child's result word carries
   CHILD_RESULT_TAG to tell it from the root's own ChimiControl pointer. */
enum { CHILD_RESULT_TAG = 1 };
static ChimiFiberResult child_task_result(ChimiControl *control) {
  if (control->kind == CONTROL_CANCEL) return task_fiber_result(control);
  void *roots[] = {control};
  AllocationKind kinds[] = {ALLOCATION_CONTROL};
  RuntimeFragment *result = NULL;
  if (fragment_copy(roots, kinds, 1, &result) != HEAP_OK) {
    roots[0] = control = defect(FOREIGN_POINTER_DEFECT, -1);
    if (fragment_copy(roots, kinds, 1, &result) != HEAP_OK) abort();
  }
  if (control->kind == CONTROL_DEFECT)
    return (ChimiFiberResult){
      .state = CHIMI_FIBER_DEFECTED,
      .defect_code = (int64_t)(intptr_t)result | CHILD_RESULT_TAG,
    };
  return (ChimiFiberResult){
    .state = CHIMI_FIBER_SUCCEEDED,
    .value = (int64_t)(intptr_t)result | CHILD_RESULT_TAG,
  };
}

static ChimiFiberResult chimi_task_entry(void *raw_context) {
  ChimiTaskContext *context = raw_context;
  if (context->entry) {
    current_heap = &main_heap;
    return task_fiber_result(context->entry(context->runtime, context->environment, context->span));
  }
  ChimiRuntime *runtime = context->runtime;
  uint64_t span = context->span;
  RuntimeHeap *heap = heap_create();
  current_heap = heap;
  void *captures[FRAGMENT_ROOTS];
  fragment_adopt(context->captures, captures);
  free(context);
  ChimiValue *callable = captures[0];
  ChimiControl *control = invoke(runtime, callable->as.callable, NULL, 0, span, UINT32_MAX,
                                 captures[1]);
  ChimiFiberResult result = child_task_result(control);
  heap_destroy(heap);
  current_heap = NULL;
  return result;
}

/* The control a task's result word names: the root's own control, or a copy of a
   child's result fragment in the current heap. */
static ChimiControl *task_reply(int64_t raw) {
  if (!(raw & CHILD_RESULT_TAG)) return (ChimiControl *)(intptr_t)raw;
  void *roots[FRAGMENT_ROOTS];
  fragment_copy_into_current((RuntimeFragment *)(intptr_t)(raw & ~(int64_t)CHILD_RESULT_TAG), roots);
  return roots[0];
}

/* The scheduler's release hooks. It calls them on the host stack with no task
   running, so they free fragments without touching current_heap. A task word is
   a child's tagged result fragment or the root's own control, which lives in the
   main heap and is not the scheduler's to free. */
static void release_task_word(void *context, int64_t word) {
  (void)context;
  if (word & CHILD_RESULT_TAG)
    fragment_free((RuntimeFragment *)(intptr_t)(word & ~(int64_t)CHILD_RESULT_TAG));
}

static void release_channel_word(void *context, int64_t word) {
  (void)context;
  fragment_free((RuntimeFragment *)(intptr_t)word);
}

/* A child the scheduler ends before its entry runs never adopts its captures. */
static void release_task_context(void *context, void *raw) {
  (void)context;
  ChimiTaskContext *task = raw;
  fragment_free(task->captures);
  free(task);
}

static ChimiControl *map_values_node(ChimiRuntime *runtime, MapNode *node,
                                     ChimiCallable *callback, MapNode **output,
                                     uint64_t span, uint32_t call_node,
                                     ChimiEnvironment *environment) {
  if (!node) return success(NULL);
  ChimiControl *control = map_values_node(runtime, node->left, callback, output,
                                         span, call_node, environment);
  if (control->kind != CONTROL_SUCCESS) return control;
  ChimiValue *arguments[] = {node->value};
  control = invoke(runtime, callback, arguments, 1, span, call_node, environment);
  if (control->kind != CONTROL_SUCCESS) return control;
  *output = map_put_node(*output, node->key, control->value);
  return map_values_node(runtime, node->right, callback, output,
                         span, call_node, environment);
}

/* `digits [. digits] [(e|E) [+|-] digits]`, as numeric.rs `is_decimal_number`. */
static bool is_decimal_number(const char *text) {
  const char *cursor = text;
  #define CHIMI_DIGITS() do { const char *start = cursor; \
    while (*cursor >= '0' && *cursor <= '9') ++cursor; if (cursor == start) return false; } while (0)
  CHIMI_DIGITS();
  if (*cursor == '.') { ++cursor; CHIMI_DIGITS(); }
  if (*cursor == 'e' || *cursor == 'E') {
    ++cursor; if (*cursor == '+' || *cursor == '-') ++cursor;
    CHIMI_DIGITS();
  }
  #undef CHIMI_DIGITS
  return *cursor == '\0';
}

/* `Int.parse`; returns 0 on success or the failure constructor. */
static uint32_t parse_int_text(const char *text, int64_t *output) {
  if (*text == '\0') return CHIMI_CONSTRUCTOR_INT_PARSE_EMPTY;
  const char *digits = *text == '-' ? text + 1 : text;
  if (*digits == '\0') return CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT;
  for (const char *cursor = digits; *cursor; ++cursor)
    if (*cursor < '0' || *cursor > '9') return CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT;
  errno = 0; *output = strtoll(text, NULL, 10);
  return errno == ERANGE ? CHIMI_CONSTRUCTOR_INT_PARSE_OUT_OF_RANGE : 0;
}

/* `Float.parse`; returns 0 on success or the failure constructor. */
static uint32_t parse_float_text(const char *text, double *output) {
  if (*text == '\0') return CHIMI_CONSTRUCTOR_FLOAT_PARSE_EMPTY;
  if (!is_decimal_number(*text == '-' ? text + 1 : text)) return CHIMI_CONSTRUCTOR_FLOAT_PARSE_INVALID;
  *output = strtod(text, NULL);
  return isfinite(*output) ? 0 : CHIMI_CONSTRUCTOR_FLOAT_PARSE_OUT_OF_RANGE;
}

/* `std/duration`. It mirrors compiler/chimi/src/duration.rs value for value, text for
   text, and message for message; the language tests run a generated set through both. */
static const int64_t DURATION_UNITS[6] = {
  3600000000000, 60000000000, 1000000000, 1000000, 1000, 1
};
static const char *const DURATION_SUFFIXES[6] = { "h", "m", "s", "ms", "us", "ns" };
static const char *const DURATION_CONSTRUCTORS[6] = {
  "hours", "minutes", "seconds", "milliseconds", "microseconds", "nanoseconds"
};
static ChimiValue *duration_value(int64_t nanoseconds) {
  ChimiValue *v = new_value(VALUE_DURATION); v->as.integer = nanoseconds; return v;
}

/* Appends `value / scale` as an exact decimal with no trailing fraction zeros. */
static char *duration_decimal(char *out, uint64_t value, uint64_t scale) {
  out += sprintf(out, "%" PRIu64, value / scale);
  uint64_t fraction = value % scale;
  if (fraction == 0) return out;
  int width = 0; for (uint64_t rest = scale; rest > 1; rest /= 10) ++width;
  char digits[24]; snprintf(digits, sizeof(digits), "%0*" PRIu64, width, fraction);
  size_t length = strlen(digits); while (length > 0 && digits[length - 1] == '0') --length;
  digits[length] = '\0';
  return out + sprintf(out, ".%s", digits);
}

/* The canonical text form into `text`, which holds at least 32 bytes. */
static void duration_text(int64_t duration, char *text) {
  if (duration == 0) { strcpy(text, "0s"); return; }
  char *out = text;
  if (duration < 0) *out++ = '-';
  /* The magnitude of the minimum value does not fit int64_t. */
  uint64_t magnitude = duration < 0 ? (uint64_t)0 - (uint64_t)duration : (uint64_t)duration;
  if (magnitude < (uint64_t)DURATION_UNITS[2]) {
    int unit = magnitude >= (uint64_t)DURATION_UNITS[3] ? 3 : magnitude >= (uint64_t)DURATION_UNITS[4] ? 4 : 5;
    out = duration_decimal(out, magnitude, (uint64_t)DURATION_UNITS[unit]);
    strcpy(out, DURATION_SUFFIXES[unit]); return;
  }
  uint64_t hour = (uint64_t)DURATION_UNITS[0], minute = (uint64_t)DURATION_UNITS[1];
  /* Seconds stay in nanoseconds, so the fraction is exact. */
  uint64_t parts[3] = { magnitude / hour, magnitude % hour / minute, magnitude % minute };
  int first = 0, last = 2;
  while (parts[first] == 0) ++first;
  while (parts[last] == 0) --last;
  for (int part = first; part <= last; ++part) {
    if (part == 2) out = duration_decimal(out, parts[part], (uint64_t)DURATION_UNITS[2]);
    else out += sprintf(out, "%" PRIu64, parts[part]);
    out += sprintf(out, "%s", DURATION_SUFFIXES[part]);
  }
}

/* `Duration.parse`; returns 0 on success or the failure constructor. A grammar error
   anywhere outranks a range error, and an inexact fraction is INVALID, never rounded. */
static uint32_t duration_parse(const char *text, int64_t *output) {
  if (*text == '\0') return CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY;
  if (strcmp(text, "0") == 0) { *output = 0; return 0; }
  bool negative = *text == '-', out_of_range = false;
  const char *cursor = negative ? text + 1 : text;
  if (*cursor == '\0') return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
  __int128 total = 0; int previous = -1;
  while (*cursor) {
    const char *integer = cursor;
    while (*cursor >= '0' && *cursor <= '9') ++cursor;
    size_t integer_length = (size_t)(cursor - integer);
    if (integer_length == 0) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    const char *fraction = cursor; size_t fraction_length = 0;
    if (*cursor == '.') {
      fraction = ++cursor;
      while (*cursor >= '0' && *cursor <= '9') ++cursor;
      fraction_length = (size_t)(cursor - fraction);
      if (fraction_length == 0) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    }
    int unit = -1;
    for (int candidate = 0; candidate < 6; ++candidate) {
      size_t length = strlen(DURATION_SUFFIXES[candidate]);
      /* `ms` before `m`: the longest suffix wins. */
      if (strncmp(cursor, DURATION_SUFFIXES[candidate], length) == 0 &&
          (unit < 0 || length > strlen(DURATION_SUFFIXES[unit]))) unit = candidate;
    }
    if (unit < 0 || unit <= previous) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    cursor += strlen(DURATION_SUFFIXES[unit]); previous = unit;
    while (fraction_length > 0 && fraction[fraction_length - 1] == '0') --fraction_length;
    /* An exact fraction has at most 13 significant digits: `h` is 2^13 * 5^11 * 9 ns. */
    if (fraction_length > 13) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    __int128 scale = DURATION_UNITS[unit], denominator = 1, numerator = 0;
    for (size_t i = 0; i < fraction_length; ++i) {
      denominator *= 10; numerator = numerator * 10 + (fraction[i] - '0');
    }
    numerator *= scale;
    if (numerator % denominator != 0) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    while (integer_length > 0 && *integer == '0') { ++integer; --integer_length; }
    if (integer_length > 22) { out_of_range = true; continue; }
    __int128 whole = 0;
    for (size_t i = 0; i < integer_length; ++i) whole = whole * 10 + (integer[i] - '0');
    total += whole * scale + numerator / denominator;
  }
  if (out_of_range) return CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE;
  if (negative) total = -total;
  if (total > INT64_MAX || total < INT64_MIN) return CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE;
  *output = (int64_t)total; return 0;
}

static ChimiControl *intrinsic_shape_defect(uint32_t call_node);

static ChimiControl *duration_overflow(const char *function, const char *operation, int64_t node) {
  return defect(numeric_message("`Duration.%s` overflows the Duration range: %s", function, operation), node);
}

static ChimiControl *invoke_duration(uint32_t intrinsic, ChimiValue **arguments,
                                     uint32_t argument_count, uint32_t call_node) {
  char left[32], right[32], operation[96];
  if (intrinsic <= CHIMI_INTRINSIC_DURATION_HOURS) {
    if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
    int unit = 5 - (int)(intrinsic - CHIMI_INTRINSIC_DURATION_NANOSECONDS);
    int64_t count = arguments[0]->as.integer, result;
    if (__builtin_mul_overflow(count, DURATION_UNITS[unit], &result))
      return defect(numeric_message("`Duration.%s(%" PRId64 ")` overflows the Duration range",
                                    DURATION_CONSTRUCTORS[unit], count), call_node);
    return success(duration_value(result));
  }
  if (intrinsic == CHIMI_INTRINSIC_DURATION_PARSE) {
    if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
    int64_t value = 0; uint32_t error = duration_parse(arguments[0]->as.text, &value);
    return error ? failure(standard_value(error, NULL, 0)) : success(duration_value(value));
  }
  if (argument_count < 1 || arguments[0]->kind != VALUE_DURATION) return intrinsic_shape_defect(call_node);
  int64_t value = arguments[0]->as.integer, result;
  duration_text(value, left);
  if (intrinsic <= CHIMI_INTRINSIC_DURATION_TO_SECONDS) {
    if (argument_count != 1) return intrinsic_shape_defect(call_node);
    int unit = 5 - (int)(intrinsic - CHIMI_INTRINSIC_DURATION_TO_NANOSECONDS);
    return success(integer_value(value / DURATION_UNITS[unit]));
  }
  switch (intrinsic) {
    case CHIMI_INTRINSIC_DURATION_ADD:
    case CHIMI_INTRINSIC_DURATION_SUBTRACT: {
      if (argument_count != 2 || arguments[1]->kind != VALUE_DURATION) return intrinsic_shape_defect(call_node);
      bool add = intrinsic == CHIMI_INTRINSIC_DURATION_ADD;
      int64_t other = arguments[1]->as.integer;
      bool overflow = add ? __builtin_add_overflow(value, other, &result)
                          : __builtin_sub_overflow(value, other, &result);
      if (!overflow) return success(duration_value(result));
      duration_text(other, right);
      snprintf(operation, sizeof(operation), "%s %s %s", left, add ? "+" : "-", right);
      return duration_overflow(add ? "add" : "subtract", operation, call_node);
    }
    case CHIMI_INTRINSIC_DURATION_MULTIPLY:
    case CHIMI_INTRINSIC_DURATION_DIVIDE: {
      if (argument_count != 2 || arguments[1]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      int64_t other = arguments[1]->as.integer;
      if (intrinsic == CHIMI_INTRINSIC_DURATION_MULTIPLY) {
        if (!__builtin_mul_overflow(value, other, &result)) return success(duration_value(result));
        snprintf(operation, sizeof(operation), "%s * %" PRId64, left, other);
        return duration_overflow("multiply", operation, call_node);
      }
      if (other == 0)
        return defect(numeric_message("`Duration.divide` by zero: %s / 0", left), call_node);
      if (value == INT64_MIN && other == -1) {
        snprintf(operation, sizeof(operation), "%s / %" PRId64, left, other);
        return duration_overflow("divide", operation, call_node);
      }
      return success(duration_value(value / other));
    }
    case CHIMI_INTRINSIC_DURATION_NEGATE:
    case CHIMI_INTRINSIC_DURATION_ABSOLUTE: {
      if (argument_count != 1) return intrinsic_shape_defect(call_node);
      bool negate = intrinsic == CHIMI_INTRINSIC_DURATION_NEGATE;
      if (value == INT64_MIN) {
        snprintf(operation, sizeof(operation), negate ? "-(%s)" : "|%s|", left);
        return duration_overflow(negate ? "negate" : "absolute", operation, call_node);
      }
      return success(duration_value(negate || value < 0 ? -value : value));
    }
    case CHIMI_INTRINSIC_DURATION_TO_TEXT:
      if (argument_count != 1) return intrinsic_shape_defect(call_node);
      return success(text_value(left));
    default: return intrinsic_shape_defect(call_node);
  }
}

/* The debug format of `std/debug` (G10). It MUST match the interpreter's renderer in
   execution/debug.rs byte for byte; the language tests compare both. Captures and
   capability contents are never shown. */
typedef struct { TextBuffer *buffer; bool indented; uint32_t depth; } DebugRenderer;

static void debug_text(TextBuffer *buffer, const char *value) { buffer_bytes(buffer, value, strlen(value)); }

static void debug_quote(TextBuffer *buffer, const char *text) {
  buffer_char(buffer, '"');
  for (const unsigned char *cursor = (const unsigned char *)text; *cursor; ++cursor) {
    switch (*cursor) {
      case '"': debug_text(buffer, "\\\""); break;
      case '\\': debug_text(buffer, "\\\\"); break;
      case '\n': debug_text(buffer, "\\n"); break;
      case '\r': debug_text(buffer, "\\r"); break;
      case '\t': debug_text(buffer, "\\t"); break;
      case '$':
        if (cursor[1] == '{') debug_text(buffer, "\\$"); else buffer_char(buffer, '$');
        break;
      default:
        if (*cursor < 0x20 || *cursor == 0x7f) {
          char escaped[16]; snprintf(escaped, sizeof(escaped), "\\u{%x}", *cursor);
          debug_text(buffer, escaped);
        } else {
          buffer_char(buffer, (char)*cursor);
        }
    }
  }
  buffer_char(buffer, '"');
}

/* An integral JSON number within 2^53 as an integer, any other as the shortest Float. */
static void debug_json_number(TextBuffer *buffer, const char *text) {
  char *end = NULL; errno = 0;
  long long integer = strtoll(text, &end, 10);
  char output[48];
  if (errno == 0 && end != text && *end == '\0') {
    snprintf(output, sizeof(output), "%lld", integer); debug_text(buffer, output); return;
  }
  errno = 0; double value = strtod(text, &end);
  if (end == text || *end != '\0' || !isfinite(value)) { debug_text(buffer, text); return; }
  if (value == trunc(value) && fabs(value) < 9007199254740992.0) {
    snprintf(output, sizeof(output), "%lld", (long long)value); debug_text(buffer, output); return;
  }
  float_text(value == 0.0 ? 0.0 : value, output); debug_text(buffer, output);
}

/* The renderer keeps its own stack of steps, so a value of any nesting depth costs
   heap, not C stack: `Debug.render` may run on a 64 KiB fiber stack. A step pushed
   last runs first. */
typedef enum {
  DEBUG_STEP_VALUE, DEBUG_STEP_LABEL, DEBUG_STEP_KEY, DEBUG_STEP_KEY_END, DEBUG_STEP_SEPARATOR,
  DEBUG_STEP_PART_END, DEBUG_STEP_CLOSE, DEBUG_STEP_FREE
} DebugStepKind;
typedef struct { DebugStepKind kind; bool spaced; uint32_t index; const void *pointer; } DebugStep;
typedef struct { DebugStep *items; size_t length; size_t capacity; } DebugSteps;

static void debug_step(DebugSteps *steps, DebugStepKind kind, const void *pointer, uint32_t index,
                       bool spaced) {
  if (steps->length == steps->capacity) {
    size_t capacity = steps->capacity ? steps->capacity * 2 : 64;
    DebugStep *items = realloc(steps->items, capacity * sizeof(*items));
    if (!items) exhausted_memory();
    steps->items = items;
    steps->capacity = capacity;
  }
  steps->items[steps->length++] = (DebugStep){kind, spaced, index, pointer};
}

static void debug_value(DebugRenderer *renderer, const ChimiValue *value);

static void debug_line(DebugRenderer *renderer) {
  buffer_char(renderer->buffer, '\n');
  for (uint32_t i = 0; i < renderer->depth; ++i) debug_text(renderer->buffer, "  ");
}

/* Between parts: `, ` on one line, or a line break and indentation. Before the first
   part on one line, a space when `spaced`. */
static void debug_separator(DebugRenderer *renderer, uint32_t index, bool spaced) {
  if (renderer->indented) debug_line(renderer);
  else if (index > 0) debug_text(renderer->buffer, ", ");
  else if (spaced) buffer_char(renderer->buffer, ' ');
}
static void debug_part_end(DebugRenderer *renderer) {
  if (renderer->indented) buffer_char(renderer->buffer, ',');
}
static void debug_close(DebugRenderer *renderer, const char *close, bool spaced) {
  if (renderer->indented) debug_line(renderer);
  else if (spaced) buffer_char(renderer->buffer, ' ');
  debug_text(renderer->buffer, close);
}

/* Opens a composite of `length` parts: each part is a separator, an optional label,
   its value, and a part end, and `close` follows the last. Returns false when there
   are no parts, after writing `close` itself. */
static bool debug_open(DebugRenderer *renderer, DebugSteps *steps, const char *open,
                       const char *close, bool spaced, uint32_t length) {
  debug_text(renderer->buffer, open);
  if (length == 0) { debug_text(renderer->buffer, close); return false; }
  ++renderer->depth;
  debug_step(steps, DEBUG_STEP_CLOSE, close, 0, spaced);
  return true;
}

/* Queues part `index`; parts are queued last to first. */
static void debug_part(DebugSteps *steps, uint32_t index, bool spaced, const char *label,
                       const ChimiValue *value) {
  debug_step(steps, DEBUG_STEP_PART_END, NULL, 0, false);
  debug_step(steps, DEBUG_STEP_VALUE, value, 0, false);
  if (label) debug_step(steps, DEBUG_STEP_LABEL, label, 0, false);
  debug_step(steps, DEBUG_STEP_SEPARATOR, NULL, index, spaced);
}

/* Named parts in the given order, as `name: value`; `labels` false drops the names. */
static void debug_fields(DebugRenderer *renderer, DebugSteps *steps, const char *open,
                         const char *close, bool spaced, const RuntimeField *fields,
                         const uint32_t *order, uint32_t length, bool labels) {
  if (!debug_open(renderer, steps, open, close, spaced, length)) return;
  for (uint32_t i = length; i-- > 0;) {
    const RuntimeField *field = &fields[order ? order[i] : i];
    debug_part(steps, i, spaced, labels ? field->name : NULL, field->value);
  }
}

/* Writes a scalar, or queues the parts of a composite. */
static void debug_expand(DebugRenderer *renderer, DebugSteps *steps, const ChimiValue *value) {
  TextBuffer *buffer = renderer->buffer;
  char scalar[48];
  switch (value->kind) {
    case VALUE_VOID: debug_text(buffer, "Void"); return;
    case VALUE_INTEGER:
      snprintf(scalar, sizeof(scalar), "%" PRId64, value->as.integer); debug_text(buffer, scalar); return;
    case VALUE_FLOAT:
      float_text(value->as.floating == 0.0 ? 0.0 : value->as.floating, scalar);
      debug_text(buffer, scalar); return;
    case VALUE_TEXT: debug_quote(buffer, value->as.text); return;
    case VALUE_BOOLEAN: debug_text(buffer, value->as.boolean ? "True" : "False"); return;
    case VALUE_LIST: {
      uint32_t length = value->as.list.length;
      if (!debug_open(renderer, steps, "[", "]", false, length)) return;
      for (uint32_t i = length; i-- > 0;) debug_part(steps, i, false, NULL, list_get(value, i));
      return;
    }
    case VALUE_RECORD: {
      uint32_t length = value->as.record.length;
      uint32_t *order = allocate(length, sizeof(*order));
      for (uint32_t i = 0; i < length; ++i) {
        uint32_t j = i;
        while (j > 0 && strcmp(value->as.record.fields[order[j - 1]].name,
                               value->as.record.fields[i].name) > 0) {
          order[j] = order[j - 1]; --j;
        }
        order[j] = i;
      }
      debug_fields(renderer, steps, "{", "}", true, value->as.record.fields, order, length, true);
      return;
    }
    /* The registry's render override: the canonical text form. */
    case VALUE_DURATION: duration_text(value->as.integer, scalar); debug_text(buffer, scalar); return;
    case VALUE_RANGE:
      snprintf(scalar, sizeof(scalar), "%" PRId64 "..%" PRId64, value->as.range.start, value->as.range.end);
      debug_text(buffer, scalar); return;
    case VALUE_TUPLE:
      debug_fields(renderer, steps, "(", ")", false, value->as.record.fields, NULL,
                   value->as.record.length, false);
      return;
    case VALUE_CONSTRUCTED: {
      uint32_t standard = CHIMI_SYMBOLS[value->as.constructed.constructor].standard_constructor;
      bool prelude = standard == CHIMI_CONSTRUCTOR_OPTION_SOME || standard == CHIMI_CONSTRUCTOR_OPTION_NONE;
      const char *variant = value->as.constructed.variant_name;
      uint32_t length = value->as.constructed.length;
      if (!prelude) debug_text(buffer, value->as.constructed.type_name);
      if (!variant) {
        debug_fields(renderer, steps, " {", "}", true, value->as.constructed.fields, NULL, length, true);
        return;
      }
      if (!prelude) buffer_char(buffer, '.');
      debug_text(buffer, variant);
      if (length > 0)
        debug_fields(renderer, steps, "(", ")", false, value->as.constructed.fields, NULL, length,
                     length > 1);
      return;
    }
    case VALUE_MAP: {
      MapEntries entries = map_entries(value->as.map);
      if (!debug_open(renderer, steps, "Map {", "}", true, (uint32_t)entries.length)) {
        free(entries.nodes);
        return;
      }
      /* Under the close, so it runs after every entry. */
      DebugStep close = steps->items[--steps->length];
      debug_step(steps, DEBUG_STEP_FREE, entries.nodes, 0, false);
      steps->items[steps->length++] = close;
      for (size_t i = entries.length; i-- > 0;) {
        debug_step(steps, DEBUG_STEP_PART_END, NULL, 0, false);
        debug_step(steps, DEBUG_STEP_VALUE, entries.nodes[i]->value, 0, false);
        debug_step(steps, DEBUG_STEP_KEY, entries.nodes[i]->key, 0, false);
        debug_step(steps, DEBUG_STEP_SEPARATOR, NULL, (uint32_t)i, true);
      }
      return;
    }
    case VALUE_JSON_NUMBER:
      debug_text(buffer, "Json.Number(");
      debug_json_number(buffer, value->as.json_number);
      debug_text(buffer, ")");
      return;
    case VALUE_CALLABLE: {
      const ChimiCallable *callable = value->as.callable;
      const char *name = NULL;
      switch (callable->kind) {
        case CALL_DECLARED: name = CHIMI_STRINGS[CHIMI_SYMBOLS[callable->symbol].name]; break;
        case CALL_DEPENDENCY: case CALL_INTRINSIC: case CALL_CONSTRUCTOR: name = callable->name; break;
        case CALL_CLOSURE: case CALL_HOST: break;
      }
      debug_text(buffer, "<fn");
      if (name) { buffer_char(buffer, ' '); debug_text(buffer, name); }
      debug_text(buffer, ">");
      return;
    }
    case VALUE_TASK: debug_text(buffer, "<Task>"); return;
    case VALUE_CHANNEL_SENDER: debug_text(buffer, "<Sender>"); return;
    case VALUE_CHANNEL_RECEIVER: debug_text(buffer, "<Receiver>"); return;
    case VALUE_CHANNEL_COMPLETION: debug_text(buffer, "<Completion>"); return;
    case VALUE_PROCESS_ADDRESS: {
      const char *label = value->as.address.kind == CHIMI_ADDRESS_NAMED
        ? process_name_label(value->as.address.target) : NULL;
      debug_text(buffer, "<address");
      if (label) { buffer_char(buffer, ' '); debug_text(buffer, label); }
      debug_text(buffer, ">");
      return;
    }
    case VALUE_PROCESS_INBOX: debug_text(buffer, "<Inbox>"); return;
    case VALUE_SUPERVISOR_RUNNING: debug_text(buffer, "<Running>"); return;
    case VALUE_MANAGED_PROCESS: debug_text(buffer, "<ManagedProcess>"); return;
    case VALUE_MANAGED_SUPERVISOR: debug_text(buffer, "<ManagedSupervisor>"); return;
  }
}

static void debug_value(DebugRenderer *renderer, const ChimiValue *root) {
  DebugSteps steps = {0};
  debug_step(&steps, DEBUG_STEP_VALUE, root, 0, false);
  while (steps.length) {
    DebugStep step = steps.items[--steps.length];
    switch (step.kind) {
      case DEBUG_STEP_VALUE: debug_expand(renderer, &steps, step.pointer); break;
      case DEBUG_STEP_LABEL:
        debug_text(renderer->buffer, step.pointer); debug_text(renderer->buffer, ": "); break;
      /* A key renders on one line even in the indented format, as the interpreter
         does. Its steps go on the same stack, so maps nested through keys of maps
         cost heap, not C stack; KEY_END restores the format the key interrupted. */
      case DEBUG_STEP_KEY:
        debug_step(&steps, DEBUG_STEP_KEY_END, NULL, 0, renderer->indented);
        debug_step(&steps, DEBUG_STEP_VALUE, step.pointer, 0, false);
        renderer->indented = false;
        break;
      case DEBUG_STEP_KEY_END:
        renderer->indented = step.spaced;
        debug_text(renderer->buffer, ": ");
        break;
      case DEBUG_STEP_SEPARATOR: debug_separator(renderer, step.index, step.spaced); break;
      case DEBUG_STEP_PART_END: debug_part_end(renderer); break;
      case DEBUG_STEP_CLOSE:
        --renderer->depth;
        debug_close(renderer, step.pointer, step.spaced);
        break;
      case DEBUG_STEP_FREE: free((void *)step.pointer); break;
    }
  }
  free(steps.items);
}

static ChimiValue *debug_render(const ChimiValue *value, bool indented) {
  TextBuffer buffer = {0};
  DebugRenderer renderer = {&buffer, indented, 0};
  debug_value(&renderer, value);
  return text_value(buffer.data);
}

static ChimiControl *intrinsic_shape_defect(uint32_t call_node) {
  return defect("verified standard function has invalid runtime arguments", call_node);
}

/* std/process and std/supervisor (docs/architecture/supervised-process-runtime.md,
   "Mapping onto the native runtime"; the scheduler's process ABI in chimi_fiber.h).

   Specs, members, policies, and restart limits are plain records with the field
   names the interpreter uses, so module constants built at compile time and values
   built here are the same. Supervisor.start and start_process marshal them into the
   scheduler's C structs:
   - each process spec gets a ProcessSpecContext: a detached fragment holding a copy
     of its body (the callable, which carries the providers it captured), because a
     supervisor has no heap. The scheduler owns the context from the request that
     hands it over and frees it through release_spec_context; every incarnation runs
     on its own copy (fragment_copy_into_current), in its own heap.
   - a message crosses Process.send as a fragment copied in the sender's heap. An
     accepted one belongs to the scheduler, which hands it to Process.receive (the
     receiver adopts it) or frees it through release_message; a refused one stays
     with the sender, which frees it and returns its own original value.

   The data constructors are pure and always compiled. The operations that talk to
   the scheduler need the process kernel, which only process programs link: their
   runtime is built with CHIMI_PROCESS_KERNEL (pay-for-play, lab scenario MF06). */

static ChimiValue *process_address_value(uint32_t kind, uint64_t target) {
  ChimiValue *value = new_value(VALUE_PROCESS_ADDRESS);
  value->as.address.kind = kind;
  value->as.address.target = target;
  return value;
}

/* A record with `count` fields, whose names are given in ascending order. */
static ChimiValue *process_record(const char *const *names, ChimiValue **values, uint32_t count) {
  ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = count;
  value->as.record.fields = allocate(count, sizeof(*value->as.record.fields));
  allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
  for (uint32_t index = 0; index < count; ++index)
    value->as.record.fields[index] = (RuntimeField){names[index], values[index]};
  return value;
}

/* The field `name` of a process record, or NULL. */
static ChimiValue *process_field(const ChimiValue *record, const char *name) {
  if (!record || record->kind != VALUE_RECORD) return NULL;
  for (uint32_t index = 0; index < record->as.record.length; ++index)
    if (strcmp(record->as.record.fields[index].name, name) == 0) return record->as.record.fields[index].value;
  return NULL;
}

static ChimiControl *process_range_defect(const char *format, int64_t value, int64_t node) {
  return defect(numeric_message(format, value), node);
}

static ChimiControl *process_duration_defect(const char *format, int64_t nanoseconds, int64_t node) {
  char text[32];
  duration_text(nanoseconds, text);
  return defect(numeric_message(format, text), node);
}

/* Process.spec, register, Supervisor.process, supervisor, policy, and restarts. The
   range defects match the interpreter's (process-and-supervisor-static-contracts.md). */
static ChimiControl *invoke_process_data(uint32_t intrinsic, ChimiValue **arguments,
                                         uint32_t argument_count, uint32_t call_node) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_PROCESS_SPEC: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_CALLABLE ||
          arguments[1]->kind != VALUE_INTEGER || arguments[2]->kind != VALUE_DURATION)
        return intrinsic_shape_defect(call_node);
      if (arguments[1]->as.integer < 1)
        return process_range_defect("Process.spec: mailbox capacity must be at least 1, got %" PRId64,
                                    arguments[1]->as.integer, call_node);
      if (arguments[2]->as.integer < 0)
        return process_duration_defect("Process.spec: shutdown_within must be at least zero, got %s",
                                       arguments[2]->as.integer, call_node);
      static const char *const names[] = {"body", "capacity", "name", "shutdown_within"};
      ChimiValue *values[] = {arguments[0], arguments[1],
                              standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0), arguments[2]};
      if (!values[2]) return intrinsic_shape_defect(call_node);
      return success(process_record(names, values, 4));
    }
    case CHIMI_INTRINSIC_PROCESS_REGISTER: {
      if (argument_count != 2 || arguments[1]->kind != VALUE_PROCESS_ADDRESS)
        return intrinsic_shape_defect(call_node);
      ChimiValue *name = process_field(arguments[0], "name");
      if (!name || name->kind != VALUE_CONSTRUCTED) return intrinsic_shape_defect(call_node);
      if (name->as.constructed.length != 0)
        return defect("Process.register: spec already has a name", call_node);
      static const char *const names[] = {"body", "capacity", "name", "shutdown_within"};
      ChimiValue *some[] = {arguments[1]};
      ChimiValue *values[] = {process_field(arguments[0], "body"), process_field(arguments[0], "capacity"),
                              standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, some, 1),
                              process_field(arguments[0], "shutdown_within")};
      for (uint32_t index = 0; index < 4; ++index)
        if (!values[index]) return intrinsic_shape_defect(call_node);
      return success(process_record(names, values, 4));
    }
    case CHIMI_INTRINSIC_SUPERVISOR_PROCESS: {
      if (argument_count != 2) return intrinsic_shape_defect(call_node);
      static const char *const names[] = {"restart", "spec"};
      ChimiValue *values[] = {arguments[1], arguments[0]};
      return success(process_record(names, values, 2));
    }
    case CHIMI_INTRINSIC_SUPERVISOR_SUPERVISOR: {
      if (argument_count != 2) return intrinsic_shape_defect(call_node);
      static const char *const names[] = {"policy", "restart"};
      return success(process_record(names, arguments, 2));
    }
    case CHIMI_INTRINSIC_SUPERVISOR_POLICY: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_LIST || arguments[1]->kind != VALUE_INTEGER)
        return intrinsic_shape_defect(call_node);
      int64_t capacity = arguments[1]->as.integer;
      if (capacity < 1)
        return process_range_defect("Supervisor.policy: capacity must be at least 1, got %" PRId64,
                                    capacity, call_node);
      if ((int64_t)list_length(arguments[0]) > capacity)
        return defect(numeric_message("Supervisor.policy: members must not exceed capacity %" PRId64
                                      ", got %" PRIu32, capacity, list_length(arguments[0])),
                      call_node);
      static const char *const names[] = {"capacity", "members", "restarts"};
      ChimiValue *values[] = {arguments[1], arguments[0], arguments[2]};
      return success(process_record(names, values, 3));
    }
    case CHIMI_INTRINSIC_SUPERVISOR_RESTARTS: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_INTEGER || arguments[1]->kind != VALUE_DURATION)
        return intrinsic_shape_defect(call_node);
      if (arguments[0]->as.integer < 0)
        return process_range_defect("Supervisor.restarts: restart limit must be at least 0, got %" PRId64,
                                    arguments[0]->as.integer, call_node);
      if (arguments[1]->as.integer <= 0)
        return process_duration_defect("Supervisor.restarts: restart window must be positive, got %s",
                                       arguments[1]->as.integer, call_node);
      static const char *const names[] = {"limit", "window"};
      return success(process_record(names, arguments, 2));
    }
  }
  return intrinsic_shape_defect(call_node);
}

#if CHIMI_PROCESS_KERNEL

static ChimiValue *managed_value(ValueKind kind, uint64_t supervisor, uint64_t membership) {
  ChimiValue *value = new_value(kind);
  value->as.managed.supervisor = supervisor;
  value->as.managed.membership = membership;
  return value;
}

/* What every incarnation of one spec runs: its body, copied into each new heap. */
typedef struct {
  ChimiRuntime *runtime;
  RuntimeFragment *body;
} ProcessSpecContext;

/* The largest peak of live payload bytes of any incarnation's heap. */
static size_t process_heap_peak_value_bytes;

/* The defect messages of incarnations whose body defected, kept while tracing so
   the `process_terminated` event that follows can carry them (`defect`). */
typedef struct { uint64_t incarnation; char *message; } ProcessDefect;
static ProcessDefect *process_defects;
static size_t process_defect_count;

static void remember_process_defect(const char *message) {
  ChimiProcessRequest self = {.kind = CHIMI_REQUEST_PROCESS_SELF};
  if (scheduler_request(&self) == CHIMI_FIBER_CANCEL || self.status != CHIMI_RESPONSE_OK) return;
  ProcessDefect *grown = realloc(process_defects, (process_defect_count + 1) * sizeof(*grown));
  char *copy = strdup(message ? message : "defect");
  if (!grown || !copy) exhausted_memory();
  process_defects = grown;
  process_defects[process_defect_count++] = (ProcessDefect){self.address, copy};
}

/* The remembered message of `incarnation`, handed to the caller to free, or NULL. */
static char *take_process_defect(uint64_t incarnation) {
  for (size_t index = 0; index < process_defect_count; ++index)
    if (process_defects[index].incarnation == incarnation) {
      char *message = process_defects[index].message;
      process_defects[index] = process_defects[--process_defect_count];
      return message;
    }
  return NULL;
}

/* The entry of every incarnation (CHIMI_PROCESS_END_* for a body that ends). The name
   is external so a built executable shows that it links the process kernel (MF06). */
ChimiFiberResult chimi_rt_process_entry(void *raw_context) {
  ProcessSpecContext *context = raw_context;
  ChimiRuntime *runtime = context->runtime;
  RuntimeHeap *heap = heap_create();
  current_heap = heap;
  void *roots[FRAGMENT_ROOTS];
  fragment_copy_into_current(context->body, roots);
  ChimiValue *body = roots[0];
  ChimiValue *arguments[] = {new_value(VALUE_PROCESS_INBOX)};
  uint64_t span = runtime->next_span++;
  ChimiControl *control = invoke(runtime, body->as.callable, arguments, 1, span, UINT32_MAX,
                                 new_environment());
  ChimiFiberResult result = {.state = CHIMI_FIBER_SUCCEEDED, .value = CHIMI_PROCESS_END_RETURNED};
  switch (control->kind) {
    case CONTROL_SUCCESS: break;
    case CONTROL_FAILURE: result.value = CHIMI_PROCESS_END_FAILED; break;
    case CONTROL_CANCEL: result = (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED}; break;
    case CONTROL_DEFECT:
    case CONTROL_BREAK:
      if (!runtime->silent) remember_process_defect(control->message);
      result = (ChimiFiberResult){.state = CHIMI_FIBER_DEFECTED, .defect_code = 1};
      break;
  }
  if (heap->peak_live_bytes > process_heap_peak_value_bytes)
    process_heap_peak_value_bytes = heap->peak_live_bytes;
  heap_destroy(heap);
  current_heap = NULL;
  return result;
}

static void release_spec_context_word(void *context, void *raw) {
  (void)context;
  ProcessSpecContext *spec = raw;
  fragment_free(spec->body);
  free(spec);
}

static void release_message_word(void *context, uint64_t message) {
  (void)context;
  fragment_free((RuntimeFragment *)(uintptr_t)message);
}

/* The C structs one Supervisor.start or start_process request reads. The arrays are
   freed after the request; the contexts belong to the scheduler once it is sent. */
typedef struct {
  void **arrays;
  size_t array_count;
  void **contexts;
  size_t context_count;
} ProcessMarshal;

static void *marshal_keep(void ***items, size_t *count, void *item) {
  void **grown = realloc(*items, (*count + 1) * sizeof(**items));
  if (!grown) exhausted_memory();
  *items = grown;
  grown[(*count)++] = item;
  return item;
}

/* Frees the arrays, and the contexts too unless the request reached the scheduler. */
static void marshal_finish(ProcessMarshal *marshal, bool handed_over) {
  for (size_t index = 0; index < marshal->array_count; ++index) free(marshal->arrays[index]);
  if (!handed_over)
    for (size_t index = 0; index < marshal->context_count; ++index)
      release_spec_context_word(NULL, marshal->contexts[index]);
  free(marshal->arrays);
  free(marshal->contexts);
}

static uint32_t marshal_restart(const ChimiValue *restart) {
  return restart && restart->kind == VALUE_CONSTRUCTED &&
             restart->as.constructed.constructor ==
               standard_constructor_symbol(CHIMI_CONSTRUCTOR_RESTART_ON_DEFECT)
           ? CHIMI_RESTART_ON_DEFECT : CHIMI_RESTART_NEVER;
}

/* NULL, or the defect that stops the operation `function`. */
static ChimiControl *marshal_spec(ChimiRuntime *runtime, const ChimiValue *spec, ProcessMarshal *marshal,
                                  ChimiProcessSpec *out, const char *function, uint32_t call_node) {
  ChimiValue *body = process_field(spec, "body"), *capacity = process_field(spec, "capacity");
  ChimiValue *name = process_field(spec, "name"), *within = process_field(spec, "shutdown_within");
  if (!body || body->kind != VALUE_CALLABLE || !capacity || capacity->kind != VALUE_INTEGER ||
      !name || name->kind != VALUE_CONSTRUCTED || !within || within->kind != VALUE_DURATION)
    return intrinsic_shape_defect(call_node);
  if (capacity->as.integer > UINT32_MAX)
    return defect(numeric_message("%s: mailbox capacity must be at most 4294967295, got %" PRId64,
                                  function, capacity->as.integer), call_node);
  void *roots[] = {body};
  AllocationKind kinds[] = {ALLOCATION_VALUE};
  RuntimeFragment *copy = NULL;
  if (fragment_copy(roots, kinds, 1, &copy) != HEAP_OK) return defect(FOREIGN_POINTER_DEFECT, call_node);
  ProcessSpecContext *context = malloc(sizeof(*context));
  if (!context) exhausted_memory();
  *context = (ProcessSpecContext){.runtime = runtime, .body = copy};
  marshal_keep(&marshal->contexts, &marshal->context_count, context);
  uint64_t registered = 0;
  if (name->as.constructed.length == 1 && name->as.constructed.fields[0].value->kind == VALUE_PROCESS_ADDRESS)
    registered = name->as.constructed.fields[0].value->as.address.target;
  *out = (ChimiProcessSpec){
    .entry = chimi_rt_process_entry,
    .context = context,
    .mailbox_capacity = (uint32_t)capacity->as.integer,
    .shutdown_within = within->as.integer,
    .name = registered,
  };
  return NULL;
}

static ChimiControl *marshal_policy(ChimiRuntime *runtime, const ChimiValue *policy, ProcessMarshal *marshal,
                                    ChimiSupervisorPolicy *out, const char *function, uint32_t call_node) {
  ChimiValue *members = process_field(policy, "members"), *capacity = process_field(policy, "capacity");
  ChimiValue *restarts = process_field(policy, "restarts");
  ChimiValue *limit = process_field(restarts, "limit"), *window = process_field(restarts, "window");
  if (!members || members->kind != VALUE_LIST || !capacity || capacity->kind != VALUE_INTEGER ||
      !limit || limit->kind != VALUE_INTEGER || !window || window->kind != VALUE_DURATION)
    return intrinsic_shape_defect(call_node);
  if (capacity->as.integer > UINT32_MAX)
    return defect(numeric_message("%s: capacity must be at most 4294967295, got %" PRId64,
                                  function, capacity->as.integer), call_node);
  if (limit->as.integer > UINT32_MAX)
    return defect(numeric_message("%s: restart limit must be at most 4294967295, got %" PRId64,
                                  function, limit->as.integer), call_node);
  uint32_t count = list_length(members);
  ChimiSupervisorMember *array = calloc(count ? count : 1, sizeof(*array));
  if (!array) exhausted_memory();
  marshal_keep(&marshal->arrays, &marshal->array_count, array);
  for (uint32_t index = 0; index < count; ++index) {
    ChimiValue *member = list_get(members, index);
    ChimiValue *spec = process_field(member, "spec"), *nested = process_field(member, "policy");
    array[index].restart = marshal_restart(process_field(member, "restart"));
    ChimiControl *failed;
    if (spec) {
      array[index].kind = CHIMI_MEMBER_PROCESS;
      failed = marshal_spec(runtime, spec, marshal, &array[index].process, function, call_node);
    } else {
      ChimiSupervisorPolicy *child = calloc(1, sizeof(*child));
      if (!child) exhausted_memory();
      marshal_keep(&marshal->arrays, &marshal->array_count, child);
      array[index].kind = CHIMI_MEMBER_SUPERVISOR;
      array[index].supervisor = child;
      failed = marshal_policy(runtime, nested, marshal, child, function, call_node);
    }
    if (failed) return failed;
  }
  *out = (ChimiSupervisorPolicy){
    .members = array,
    .member_count = count,
    .capacity = (uint32_t)capacity->as.integer,
    .restart_limit = (uint32_t)limit->as.integer,
    .restart_within = window->as.integer,
  };
  return NULL;
}

/* Issues `request`, repeating it while the scheduler answers PENDING. */
static ChimiFiberCommand process_request(ChimiProcessRequest *request) {
  for (;;) {
    request->status = CHIMI_RESPONSE_UNSET;
    ChimiFiberCommand command = scheduler_request(request);
    if (command == CHIMI_FIBER_CANCEL || request->status != CHIMI_RESPONSE_PENDING) return command;
  }
}

static ChimiControl *start_error(uint32_t status, uint64_t name, const char *function, uint32_t call_node) {
  switch (status) {
    case CHIMI_RESPONSE_CAPACITY_REACHED:
      return failure(standard_value(CHIMI_CONSTRUCTOR_START_CAPACITY_REACHED, NULL, 0));
    case CHIMI_RESPONSE_STOPPED: return failure(standard_value(CHIMI_CONSTRUCTOR_START_STOPPED, NULL, 0));
    case CHIMI_RESPONSE_NAME_TAKEN: {
      const char *label = process_name_label(name);
      ChimiValue *fields[] = {text_value(label ? label : "")};
      return failure(standard_value(CHIMI_CONSTRUCTOR_START_NAME_TAKEN, fields, 1));
    }
  }
  return defect(numeric_message("%s scheduler request failed", function), call_node);
}

static bool is_kind(ChimiValue **arguments, uint32_t index, ValueKind kind) {
  return arguments[index]->kind == kind;
}

/* The process and supervisor operations that run on the scheduler. */
static ChimiControl *invoke_process_kernel(ChimiRuntime *runtime, uint32_t intrinsic, ChimiValue **arguments,
                                           uint32_t argument_count, uint32_t call_node) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_PROCESS_SEND: {
      if (argument_count != 2 || !is_kind(arguments, 0, VALUE_PROCESS_ADDRESS))
        return intrinsic_shape_defect(call_node);
      void *roots[] = {arguments[1]};
      AllocationKind kinds[] = {ALLOCATION_VALUE};
      RuntimeFragment *message = NULL;
      copy_counting = true;
      HeapStatus status = fragment_copy(roots, kinds, 1, &message);
      copy_counting = false;
      if (status != HEAP_OK)
        return defect(numeric_message("Process.send: the message points into owner %" PRIu64
                                      ", outside the sender's heap (owner %" PRIu64 ")",
                                      fragment_copy_foreign_owner, current_heap->owner),
                      call_node);
      ChimiProcessRequest request = {
        .kind = CHIMI_REQUEST_PROCESS_SEND,
        .address_kind = arguments[0]->as.address.kind,
        .address = arguments[0]->as.address.target,
        .message = (uint64_t)(uintptr_t)message,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      /* Only an accepted message is the scheduler's; any other reply leaves it here. */
      if (request.status != CHIMI_RESPONSE_OK) fragment_free(message);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      ChimiValue *fields[] = {arguments[1]};
      switch (request.status) {
        case CHIMI_RESPONSE_OK: return success(void_value());
        case CHIMI_RESPONSE_FULL: return failure(standard_value(CHIMI_CONSTRUCTOR_PROCESS_SEND_FULL, fields, 1));
        case CHIMI_RESPONSE_UNAVAILABLE:
          return failure(standard_value(CHIMI_CONSTRUCTOR_PROCESS_SEND_UNAVAILABLE, fields, 1));
      }
      return defect("Process.send scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_PROCESS_RECEIVE: {
      if (argument_count != 1 || !is_kind(arguments, 0, VALUE_PROCESS_INBOX))
        return intrinsic_shape_defect(call_node);
      ChimiProcessRequest request = {.kind = CHIMI_REQUEST_PROCESS_RECEIVE};
      ChimiFiberCommand command = process_request(&request);
      if (command == CHIMI_FIBER_CANCEL) {
        if (request.status == CHIMI_RESPONSE_SUCCEEDED)
          fragment_free((RuntimeFragment *)(uintptr_t)request.message);
        return cancelled();
      }
      if (request.status == CHIMI_RESPONSE_SUCCEEDED) {
        void *roots[FRAGMENT_ROOTS];
        fragment_adopt((RuntimeFragment *)(uintptr_t)request.message, roots);
        return success(roots[0]);
      }
      if (request.status == CHIMI_RESPONSE_CANCELLED || request.status == CHIMI_RESPONSE_FORCED)
        return cancelled();
      return defect("Process.receive scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_SUPERVISOR_START:
    case CHIMI_INTRINSIC_SUPERVISOR_START_SUPERVISOR: {
      bool nested = intrinsic == CHIMI_INTRINSIC_SUPERVISOR_START_SUPERVISOR;
      const char *function = nested ? "Supervisor.start_supervisor" : "Supervisor.start";
      if (argument_count != (nested ? 3u : 1u) || (nested && !is_kind(arguments, 0, VALUE_SUPERVISOR_RUNNING)))
        return intrinsic_shape_defect(call_node);
      ProcessMarshal marshal = {0};
      ChimiSupervisorPolicy policy;
      ChimiControl *failed = marshal_policy(runtime, arguments[nested ? 1 : 0], &marshal, &policy,
                                            function, call_node);
      if (failed) {
        marshal_finish(&marshal, false);
        return failed;
      }
      ChimiProcessRequest request = {
        .kind = nested ? CHIMI_REQUEST_SUPERVISOR_START_SUPERVISOR : CHIMI_REQUEST_SUPERVISOR_START,
        .supervisor = nested ? arguments[0]->as.managed.supervisor : 0,
        .restart = nested ? marshal_restart(arguments[2]) : 0,
        .policy = &policy,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      marshal_finish(&marshal, request.status != CHIMI_RESPONSE_UNSET);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status == CHIMI_RESPONSE_OK)
        return success(nested ? managed_value(VALUE_MANAGED_SUPERVISOR, arguments[0]->as.managed.supervisor,
                                              request.membership)
                              : managed_value(VALUE_SUPERVISOR_RUNNING, request.supervisor, 0));
      return start_error(request.status, request.name, function, call_node);
    }
    case CHIMI_INTRINSIC_SUPERVISOR_START_PROCESS: {
      if (argument_count != 3 || !is_kind(arguments, 0, VALUE_SUPERVISOR_RUNNING))
        return intrinsic_shape_defect(call_node);
      ProcessMarshal marshal = {0};
      ChimiProcessSpec spec;
      ChimiControl *failed = marshal_spec(runtime, arguments[1], &marshal, &spec,
                                          "Supervisor.start_process", call_node);
      if (failed) {
        marshal_finish(&marshal, false);
        return failed;
      }
      ChimiProcessRequest request = {
        .kind = CHIMI_REQUEST_SUPERVISOR_START_PROCESS,
        .supervisor = arguments[0]->as.managed.supervisor,
        .restart = marshal_restart(arguments[2]),
        .spec = &spec,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      marshal_finish(&marshal, request.status != CHIMI_RESPONSE_UNSET);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status == CHIMI_RESPONSE_OK)
        return success(managed_value(VALUE_MANAGED_PROCESS, arguments[0]->as.managed.supervisor,
                                     request.membership));
      return start_error(request.status, request.name, "Supervisor.start_process", call_node);
    }
    case CHIMI_INTRINSIC_SUPERVISOR_ADDRESS: {
      if (argument_count != 1 || !is_kind(arguments, 0, VALUE_MANAGED_PROCESS))
        return intrinsic_shape_defect(call_node);
      ChimiProcessRequest request = {
        .kind = CHIMI_REQUEST_SUPERVISOR_ADDRESS,
        .supervisor = arguments[0]->as.managed.supervisor,
        .membership = arguments[0]->as.managed.membership,
      };
      if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status != CHIMI_RESPONSE_OK)
        return defect("Supervisor.address scheduler request failed", call_node);
      return success(process_address_value(CHIMI_ADDRESS_DIRECT, request.address));
    }
    case CHIMI_INTRINSIC_SUPERVISOR_REMOVE_PROCESS:
    case CHIMI_INTRINSIC_SUPERVISOR_REMOVE_SUPERVISOR: {
      bool process = intrinsic == CHIMI_INTRINSIC_SUPERVISOR_REMOVE_PROCESS;
      const char *function = process ? "Supervisor.remove_process" : "Supervisor.remove_supervisor";
      if (argument_count != 2 || !is_kind(arguments, 0, VALUE_SUPERVISOR_RUNNING) ||
          !is_kind(arguments, 1, process ? VALUE_MANAGED_PROCESS : VALUE_MANAGED_SUPERVISOR))
        return intrinsic_shape_defect(call_node);
      ChimiProcessRequest request = {
        .kind = process ? CHIMI_REQUEST_SUPERVISOR_REMOVE_PROCESS : CHIMI_REQUEST_SUPERVISOR_REMOVE_SUPERVISOR,
        .supervisor = arguments[0]->as.managed.supervisor,
        .membership = arguments[1]->as.managed.membership,
      };
      if (process_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status == CHIMI_RESPONSE_OK) return success(void_value());
      if (request.owner != 0)
        return defect(numeric_message("%s: the handle belongs to supervisor %" PRIu64 ", not supervisor %" PRIu64,
                                      function, request.owner, arguments[0]->as.managed.supervisor),
                      call_node);
      return defect(numeric_message("%s scheduler request failed", function), call_node);
    }
    case CHIMI_INTRINSIC_SUPERVISOR_STOP:
    case CHIMI_INTRINSIC_SUPERVISOR_WAIT: {
      bool stop = intrinsic == CHIMI_INTRINSIC_SUPERVISOR_STOP;
      if (argument_count != 1 || !is_kind(arguments, 0, VALUE_SUPERVISOR_RUNNING))
        return intrinsic_shape_defect(call_node);
      ChimiProcessRequest request = {
        .kind = stop ? CHIMI_REQUEST_SUPERVISOR_STOP : CHIMI_REQUEST_SUPERVISOR_WAIT,
        .supervisor = arguments[0]->as.managed.supervisor,
      };
      if (process_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status == CHIMI_RESPONSE_OK) return success(void_value());
      /* The supervisor escalated: the program ends with the escalation's defect. */
      if (!stop && request.status == CHIMI_RESPONSE_DEFECTED)
        return defect("Supervisor.RestartLimitExceeded", call_node);
      return defect(stop ? "Supervisor.stop scheduler request failed" : "Supervisor.wait scheduler request failed",
                    call_node);
    }
  }
  return intrinsic_shape_defect(call_node);
}

#endif

static ChimiControl *invoke_process(ChimiRuntime *runtime, uint32_t intrinsic, ChimiValue **arguments,
                                    uint32_t argument_count, uint32_t call_node) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_PROCESS_SPEC:
    case CHIMI_INTRINSIC_PROCESS_REGISTER:
    case CHIMI_INTRINSIC_SUPERVISOR_PROCESS:
    case CHIMI_INTRINSIC_SUPERVISOR_SUPERVISOR:
    case CHIMI_INTRINSIC_SUPERVISOR_POLICY:
    case CHIMI_INTRINSIC_SUPERVISOR_RESTARTS:
      return invoke_process_data(intrinsic, arguments, argument_count, call_node);
  }
#if CHIMI_PROCESS_KERNEL
  return invoke_process_kernel(runtime, intrinsic, arguments, argument_count, call_node);
#else
  (void)runtime;
  /* The compiler links the process runtime into every program that reaches one. */
  return defect("this runtime was built without the supervised process runtime", call_node);
#endif
}

static ChimiControl *invoke_intrinsic(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  if (intrinsic >= CHIMI_INTRINSIC_DURATION_NANOSECONDS && intrinsic <= CHIMI_INTRINSIC_DURATION_PARSE)
    return invoke_duration(intrinsic, arguments, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_PROCESS_SPEC && intrinsic <= CHIMI_INTRINSIC_SUPERVISOR_WAIT)
    return invoke_process(runtime, intrinsic, arguments, argument_count, call_node);
  switch (intrinsic) {
    case CHIMI_INTRINSIC_IO_PATH:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT)
        return intrinsic_shape_defect(call_node);
      return success(arguments[0]);
    case CHIMI_INTRINSIC_LIST_FIRST: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST)
        return intrinsic_shape_defect(call_node);
      if (list_length(arguments[0]) == 0)
        return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
      ChimiValue *fields[] = {list_get(arguments[0], 0)};
      return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
    }
    case CHIMI_INTRINSIC_LIST_LENGTH:
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST)
        return intrinsic_shape_defect(call_node);
      return success(integer_value((int64_t)list_length(arguments[0])));
    case CHIMI_INTRINSIC_ORDER_COMPARE: {
      if (argument_count != 2) return intrinsic_shape_defect(call_node);
      int order = chimi_value_compare(arguments[0], arguments[1]);
      uint32_t constructor = order < 0 ? CHIMI_CONSTRUCTOR_ORDERING_LESS
                           : order > 0 ? CHIMI_CONSTRUCTOR_ORDERING_GREATER : CHIMI_CONSTRUCTOR_ORDERING_EQUAL;
      return success(standard_value(constructor, NULL, 0));
    }
    /* Both return the left operand when the two are equal. */
    case CHIMI_INTRINSIC_ORDER_MIN:
      if (argument_count != 2) return intrinsic_shape_defect(call_node);
      return success(chimi_value_compare(arguments[0], arguments[1]) > 0 ? arguments[1] : arguments[0]);
    case CHIMI_INTRINSIC_ORDER_MAX:
      if (argument_count != 2) return intrinsic_shape_defect(call_node);
      return success(chimi_value_compare(arguments[0], arguments[1]) < 0 ? arguments[1] : arguments[0]);
    case CHIMI_INTRINSIC_MAP_EMPTY:
      return argument_count == 0 ? success(map_value(NULL)) : intrinsic_shape_defect(call_node);
    case CHIMI_INTRINSIC_MAP_GET: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_MAP)
        return intrinsic_shape_defect(call_node);
      ChimiValue *found = map_get_node(arguments[0]->as.map, arguments[1]);
      if (!found) return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
      ChimiValue *fields[] = {found};
      return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
    }
    case CHIMI_INTRINSIC_MAP_PUT: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_MAP)
        return intrinsic_shape_defect(call_node);
      return success(map_value(map_put_node(arguments[0]->as.map, arguments[1], arguments[2])));
    }
    case CHIMI_INTRINSIC_MAP_FROM: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST) return intrinsic_shape_defect(call_node);
      MapNode *map = NULL;
      for (uint32_t i = 0; i < list_length(arguments[0]); ++i) {
        ChimiValue *pair = list_get(arguments[0], i);
        if (pair->kind != VALUE_TUPLE || pair->as.record.length != 2) return intrinsic_shape_defect(call_node);
        map = map_put_node(map, pair->as.record.fields[0].value, pair->as.record.fields[1].value);
      }
      return success(map_value(map));
    }
    case CHIMI_INTRINSIC_MAP_UPDATE: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_MAP ||
          arguments[2]->kind != VALUE_CALLABLE) return intrinsic_shape_defect(call_node);
      ChimiValue *found = map_get_node(arguments[0]->as.map, arguments[1]);
      ChimiValue *current;
      if (found) { ChimiValue *fields[] = {found}; current = standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1); }
      else current = standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0);
      ChimiValue *callback_arguments[] = {current};
      ChimiControl *control = invoke(runtime, arguments[2]->as.callable, callback_arguments, 1,
                                     span, call_node, environment);
      if (control->kind != CONTROL_SUCCESS) return control;
      return success(map_value(map_put_node(arguments[0]->as.map, arguments[1], control->value)));
    }
    case CHIMI_INTRINSIC_MAP_MAP_VALUES: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_MAP || arguments[1]->kind != VALUE_CALLABLE)
        return intrinsic_shape_defect(call_node);
      MapNode *output = NULL;
      ChimiControl *control = map_values_node(runtime, arguments[0]->as.map, arguments[1]->as.callable,
                                              &output, span, call_node, environment);
      return control->kind == CONTROL_SUCCESS ? success(map_value(output)) : control;
    }
    case CHIMI_INTRINSIC_JSON_DECODE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      JsonParser parser = {arguments[0]->as.text, strlen(arguments[0]->as.text), 0, NULL};
      ChimiValue *value = json_parse_value(&parser, 0); if (value) json_skip_space(&parser);
      if (value && parser.offset == parser.length) return success(value);
      if (!parser.message) parser.message = "unexpected characters after JSON value";
      ChimiValue *fields[] = {text_value(parser.message), integer_value((int64_t)parser.offset)};
      return failure(standard_value(CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON, fields, 2));
    }
    case CHIMI_INTRINSIC_JSON_ENCODE: {
      if (argument_count != 1) return intrinsic_shape_defect(call_node);
      TextBuffer buffer = {0};
      if (!encode_json_value(&buffer, arguments[0])) return intrinsic_shape_defect(call_node);
      if (!buffer.data) buffer.data = allocate(1, 1);
      return success(text_value(buffer.data));
    }
    case CHIMI_INTRINSIC_JSON_BOOLEAN: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_BOOLEAN) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_BOOLEAN, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      char *number = allocate(32, 1); snprintf(number, 32, "%" PRId64, arguments[0]->as.integer);
      ChimiValue *raw = json_number_value(number); ChimiValue *fields[] = {raw};
      return success(standard_value(CHIMI_CONSTRUCTOR_JSON_NUMBER, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_TEXT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_ARRAY: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_ARRAY, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_OBJECT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_MAP) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_OBJECT, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_NUMBER_TO_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_JSON_NUMBER) return intrinsic_shape_defect(call_node);
      const char *number = arguments[0]->as.json_number;
      if (strpbrk(number, ".eE")) {
        ChimiValue *fields[] = {arguments[0]};
        return failure(standard_value(CHIMI_CONSTRUCTOR_NUMBER_NOT_INTEGER, fields, 1));
      }
      errno = 0; char *end = NULL; int64_t value = strtoll(number, &end, 10);
      if (errno == ERANGE || !end || *end) {
        ChimiValue *fields[] = {arguments[0]};
        return failure(standard_value(CHIMI_CONSTRUCTOR_NUMBER_OUT_OF_RANGE, fields, 1));
      }
      return success(integer_value(value));
    }
    case CHIMI_INTRINSIC_TEXT_APPEND: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT ||
          arguments[1]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      size_t left = strlen(arguments[0]->as.text), right = strlen(arguments[1]->as.text);
      char *output = allocate(left + right + 1, 1);
      memcpy(output, arguments[0]->as.text, left);
      memcpy(output + left, arguments[1]->as.text, right);
      return success(text_value(output));
    }
    case CHIMI_INTRINSIC_TEXT_EQUAL:
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT ||
          arguments[1]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      return success(boolean_value(strcmp(arguments[0]->as.text, arguments[1]->as.text) == 0));
    case CHIMI_INTRINSIC_INT_PARSE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      int64_t value = 0; uint32_t error = parse_int_text(arguments[0]->as.text, &value);
      return error ? failure(standard_value(error, NULL, 0)) : success(integer_value(value));
    }
    case CHIMI_INTRINSIC_INT_TO_TEXT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      char text[24]; snprintf(text, sizeof(text), "%" PRId64, arguments[0]->as.integer);
      return success(text_value(text));
    }
    case CHIMI_INTRINSIC_INT_ADD_CHECKED:
    case CHIMI_INTRINSIC_INT_SUBTRACT_CHECKED:
    case CHIMI_INTRINSIC_INT_MULTIPLY_CHECKED:
    case CHIMI_INTRINSIC_INT_DIVIDE_CHECKED:
    case CHIMI_INTRINSIC_INT_REMAINDER_CHECKED: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_INTEGER ||
          arguments[1]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      static const uint32_t operators[] = {CHIMI_BINARY_ADD, CHIMI_BINARY_SUBTRACT, CHIMI_BINARY_MULTIPLY,
                                           CHIMI_BINARY_DIVIDE, CHIMI_BINARY_REMAINDER};
      int64_t value = 0;
      if (!integer_result(operators[intrinsic - CHIMI_INTRINSIC_INT_ADD_CHECKED],
                          arguments[0]->as.integer, arguments[1]->as.integer, &value))
        return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
      ChimiValue *fields[] = {integer_value(value)};
      return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
    }
    case CHIMI_INTRINSIC_INT_MODULO: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_INTEGER ||
          arguments[1]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      int64_t value = arguments[0]->as.integer, modulus = arguments[1]->as.integer;
      if (modulus == 0) {
        return defect(numeric_message("integer division by zero in `Int.modulo`: Int.modulo(%" PRId64
                                      ", %" PRId64 ")", value, modulus), call_node);
      }
      int64_t remainder = modulus == -1 ? 0 : value % modulus;
      if (remainder != 0 && (remainder < 0) != (modulus < 0)) remainder += modulus;
      return success(integer_value(remainder));
    }
    case CHIMI_INTRINSIC_RANGE_START:
    case CHIMI_INTRINSIC_RANGE_END:
    case CHIMI_INTRINSIC_RANGE_LENGTH: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_RANGE) return intrinsic_shape_defect(call_node);
      int64_t start = arguments[0]->as.range.start, end = arguments[0]->as.range.end;
      if (intrinsic == CHIMI_INTRINSIC_RANGE_START) return success(integer_value(start));
      if (intrinsic == CHIMI_INTRINSIC_RANGE_END) return success(integer_value(end));
      if (end <= start) return success(integer_value(0));
      int64_t length;
      if (__builtin_sub_overflow(end, start, &length))
        return defect("Range.length overflows Int: the range holds more than the largest Int elements", call_node);
      return success(integer_value(length));
    }
    case CHIMI_INTRINSIC_RANGE_CONTAINS:
      if (argument_count != 2 || arguments[0]->kind != VALUE_RANGE || arguments[1]->kind != VALUE_INTEGER)
        return intrinsic_shape_defect(call_node);
      return success(boolean_value(arguments[0]->as.range.start <= arguments[1]->as.integer &&
                                   arguments[1]->as.integer < arguments[0]->as.range.end));
    case CHIMI_INTRINSIC_FLOAT_FROM_INT:
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      return success(float_value((double)arguments[0]->as.integer));
    case CHIMI_INTRINSIC_FLOAT_TRUNCATE:
    case CHIMI_INTRINSIC_FLOAT_FLOOR:
    case CHIMI_INTRINSIC_FLOAT_CEILING:
    case CHIMI_INTRINSIC_FLOAT_ROUND: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_FLOAT) return intrinsic_shape_defect(call_node);
      double value = arguments[0]->as.floating;
      switch (intrinsic) {
        case CHIMI_INTRINSIC_FLOAT_TRUNCATE: value = trunc(value); break;
        case CHIMI_INTRINSIC_FLOAT_FLOOR: value = floor(value); break;
        case CHIMI_INTRINSIC_FLOAT_CEILING: value = ceil(value); break;
        default: value = round(value); break; /* half away from zero */
      }
      return success(float_value(value));
    }
    case CHIMI_INTRINSIC_FLOAT_TO_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_FLOAT) return intrinsic_shape_defect(call_node);
      double value = arguments[0]->as.floating;
      if (value != trunc(value)) return failure(standard_value(CHIMI_CONSTRUCTOR_FLOAT_NOT_INTEGER, NULL, 0));
      /* -2^63 is exactly representable; 2^63 is the first value above Int. */
      if (!(value >= -9223372036854775808.0 && value < 9223372036854775808.0))
        return failure(standard_value(CHIMI_CONSTRUCTOR_FLOAT_OUT_OF_RANGE, NULL, 0));
      return success(integer_value((int64_t)value));
    }
    case CHIMI_INTRINSIC_DEBUG_RENDER:
    case CHIMI_INTRINSIC_DEBUG_RENDER_INDENTED:
      if (argument_count != 1) return intrinsic_shape_defect(call_node);
      return success(debug_render(arguments[0], intrinsic == CHIMI_INTRINSIC_DEBUG_RENDER_INDENTED));
    case CHIMI_INTRINSIC_FLOAT_TO_TEXT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_FLOAT) return intrinsic_shape_defect(call_node);
      char text[40]; float_text(arguments[0]->as.floating, text);
      return success(text_value(text));
    }
    case CHIMI_INTRINSIC_FLOAT_PARSE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      double value = 0; uint32_t error = parse_float_text(arguments[0]->as.text, &value);
      return error ? failure(standard_value(error, NULL, 0)) : success(float_value(value));
    }
    case CHIMI_INTRINSIC_TASK_START: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CALLABLE)
        return intrinsic_shape_defect(call_node);
      /* The child runs in its own heap, so it gets a copy of what it captured. */
      void *roots[] = {arguments[0], capture_requirements(environment)};
      AllocationKind kinds[] = {ALLOCATION_VALUE, ALLOCATION_ENVIRONMENT};
      RuntimeFragment *captures = NULL;
      if (fragment_copy(roots, kinds, 2, &captures) != HEAP_OK)
        return defect(FOREIGN_POINTER_DEFECT, call_node);
      ChimiTaskContext *context = malloc(sizeof(*context));
      if (!context) exhausted_memory();
      *context = (ChimiTaskContext){.runtime = runtime, .captures = captures, .span = span};
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_TASK_START,
        .entry = chimi_task_entry,
        .context = context,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      if (request.status != CHIMI_RESPONSE_OK) {
        fragment_free(captures);
        free(context);
        if (command == CHIMI_FIBER_CANCEL) return cancelled();
        return defect("Task.start scheduler request failed", call_node);
      }
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      /* The child has not run yet: it adopts the fragment when it first runs, so the
         transfer below rewrites the capabilities in the child's copy. */
      CapabilityTransfer transfer = {.child = request.task};
      bool transferred = transfer_captures(&transfer, captures->roots[0]);
      if (!transferred) {
        if (transfer.cancelled) return cancelled();
        return defect("Task.start could not transfer an exclusive capture", call_node);
      }
      return success(task_value(request.task));
    }
    case CHIMI_INTRINSIC_TASK_RESULT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TASK)
        return intrinsic_shape_defect(call_node);
      for (;;) {
        ChimiSchedulerRequest request = {
          .kind = CHIMI_REQUEST_TASK_RESULT,
          .task = arguments[0]->as.task,
        };
        ChimiFiberCommand command = scheduler_request(&request);
        if (command == CHIMI_FIBER_CANCEL) return cancelled();
        if (request.status == CHIMI_RESPONSE_PENDING) continue;
        if (request.status == CHIMI_RESPONSE_SUCCEEDED) return task_reply(request.value);
        if (request.status == CHIMI_RESPONSE_CANCELLED) return cancelled();
        if (request.status == CHIMI_RESPONSE_DEFECTED && request.value > 0)
          return task_reply(request.value);
        if (request.status == CHIMI_RESPONSE_STACK_OVERFLOW)
          return defect("Task stack overflow", call_node);
        return defect("Task.result scheduler request failed", call_node);
      }
    }
    case CHIMI_INTRINSIC_TASK_CANCEL: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TASK)
        return intrinsic_shape_defect(call_node);
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_TASK_CANCEL,
        .task = arguments[0]->as.task,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(void_value())
        : defect("Task.cancel scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_CHANNEL_BOUNDED: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER ||
          arguments[0]->as.integer < 0)
        return intrinsic_shape_defect(call_node);
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_BOUNDED,
        .value = arguments[0]->as.integer,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(channel_bundle_value(request.channel, request.owner, request.token))
        : defect("Channel.bounded scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_CHANNEL_SEND: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_CHANNEL_SENDER)
        return intrinsic_shape_defect(call_node);
      /* The message travels as a fragment. Once the channel accepts it, the
         scheduler owns it and releases it if it is discarded. One the channel
         refuses comes back, and the sender frees it and recovers its own original
         value instead. */
      void *roots[] = {arguments[1]};
      AllocationKind kinds[] = {ALLOCATION_VALUE};
      RuntimeFragment *message = NULL;
      if (fragment_copy(roots, kinds, 1, &message) != HEAP_OK)
        return defect(FOREIGN_POINTER_DEFECT, call_node);
      for (;;) {
        ChimiSchedulerRequest request = {
          .kind = CHIMI_REQUEST_CHANNEL_SEND,
          .channel = arguments[0]->as.capability.channel,
          .value = (int64_t)(intptr_t)message,
        };
        ChimiFiberCommand command = scheduler_request(&request);
        /* The scheduler takes the message on OK or PENDING and hands it back on
           CLOSED or ABORTED; any other reply leaves it with the sender. Either way
           the sender frees it, even when it is being cancelled. */
        bool taken = request.status == CHIMI_RESPONSE_OK ||
                     request.status == CHIMI_RESPONSE_PENDING;
        bool returned = request.status == CHIMI_RESPONSE_CLOSED ||
                        request.status == CHIMI_RESPONSE_ABORTED;
        if (returned) fragment_free((RuntimeFragment *)(intptr_t)request.value);
        else if (!taken) fragment_free(message);
        if (command == CHIMI_FIBER_CANCEL) return cancelled();
        if (request.status == CHIMI_RESPONSE_PENDING) continue;
        if (request.status == CHIMI_RESPONSE_OK) return success(void_value());
        ChimiValue *fields[] = {arguments[1]};
        if (request.status == CHIMI_RESPONSE_CLOSED)
          return failure(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_SEND_CLOSED, fields, 1));
        if (request.status == CHIMI_RESPONSE_ABORTED)
          return failure(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_SEND_ABORTED, fields, 1));
        return defect("Channel.send scheduler request failed", call_node);
      }
    }
    case CHIMI_INTRINSIC_CHANNEL_RECEIVE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CHANNEL_RECEIVER)
        return intrinsic_shape_defect(call_node);
      RuntimeCapability capability = arguments[0]->as.capability;
      for (;;) {
        ChimiSchedulerRequest request = {
          .kind = CHIMI_REQUEST_CHANNEL_RECEIVE,
          .channel = capability.channel,
          .owner = capability.owner,
          .token = capability.token,
        };
        ChimiFiberCommand command = scheduler_request(&request);
        if (command == CHIMI_FIBER_CANCEL) {
          if (request.status == CHIMI_RESPONSE_SUCCEEDED)
            fragment_free((RuntimeFragment *)(intptr_t)request.value);
          return cancelled();
        }
        if (request.status == CHIMI_RESPONSE_PENDING) continue;
        if (request.status == CHIMI_RESPONSE_SUCCEEDED) {
          void *roots[FRAGMENT_ROOTS];
          fragment_adopt((RuntimeFragment *)(intptr_t)request.value, roots);
          ChimiValue *fields[] = {roots[0]};
          return success(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_RECEIVED, fields, 1));
        }
        if (request.status == CHIMI_RESPONSE_DRAINED)
          return success(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_DRAINED, NULL, 0));
        if (request.status == CHIMI_RESPONSE_ABORTED)
          return failure(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_ABORTED, NULL, 0));
        return defect("Channel.receive scheduler request failed", call_node);
      }
    }
    case CHIMI_INTRINSIC_CHANNEL_CLOSE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CHANNEL_COMPLETION)
        return intrinsic_shape_defect(call_node);
      RuntimeCapability capability = arguments[0]->as.capability;
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_CLOSE,
        .channel = capability.channel,
        .owner = capability.owner,
        .token = capability.token,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(void_value())
        : defect("Channel.close scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_CLOCK_SLEEP: {
      /* `Clock.sleep(d)` is `sleep_until(now + d)`. Reading now checks
         cancellation; a deadline at or before now then returns without
         parking (time-and-outcomes.md, "The Clock capability"). */
      if (argument_count != 1 || arguments[0]->kind != VALUE_DURATION)
        return intrinsic_shape_defect(call_node);
      ChimiSchedulerRequest now = {.kind = CHIMI_REQUEST_TIMER_NOW};
      if (scheduler_request(&now) == CHIMI_FIBER_CANCEL) return cancelled();
      if (now.status != CHIMI_RESPONSE_OK)
        return defect("Clock.sleep scheduler request failed", call_node);
      int64_t deadline;
      if (__builtin_add_overflow(now.value, arguments[0]->as.integer, &deadline)) {
        char duration[32];
        duration_text(arguments[0]->as.integer, duration);
        return defect(numeric_message("`Clock.sleep(%s)` overflows the Instant range", duration),
                      call_node);
      }
      for (;;) {
        ChimiSchedulerRequest request = {
          .kind = CHIMI_REQUEST_TIMER_SLEEP_UNTIL,
          .value = deadline,
        };
        if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
        if (request.status == CHIMI_RESPONSE_OK) return success(void_value());
        if (request.status != CHIMI_RESPONSE_PENDING)
          return defect("Clock.sleep scheduler request failed", call_node);
      }
    }
    case CHIMI_INTRINSIC_CHANNEL_ABORT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CHANNEL_RECEIVER)
        return intrinsic_shape_defect(call_node);
      RuntimeCapability capability = arguments[0]->as.capability;
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_ABORT,
        .channel = capability.channel,
        .owner = capability.owner,
        .token = capability.token,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(void_value())
        : defect("Channel.abort scheduler request failed", call_node);
    }
  }
  return defect("unknown standard intrinsic", call_node);
}

/* Module constants, decoded from CHIMI_CONSTANT_VALUES before the entry runs. They are
   allocated in the static area, so no collection frees them, and values are
   immutable, so every task shares them. Decoding cannot fail and emits no events. */
static ChimiValue **constant_values;

/* Binds each (requirement name, value id) pair to every requirement of that name, as a
   provision does. */
static void bind_constant_requirements(ChimiEnvironment *environment, const uint32_t *pairs,
                                       int64_t count) {
  for (int64_t i = 0; i < count; ++i) {
    const char *name = CHIMI_STRINGS[pairs[2 * i]];
    for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
      if (CHIMI_SYMBOLS[symbol].kind == CHIMI_SYMBOL_REQUIREMENT &&
          strcmp(CHIMI_STRINGS[CHIMI_SYMBOLS[symbol].name], name) == 0)
        environment->requirements[CHIMI_SYMBOLS[symbol].environment_slot] = constant_values[pairs[2 * i + 1]];
  }
}

static ChimiValue *decode_constant(const ChimiConstantValue *raw) {
  const uint32_t *ids = &CHIMI_IDS[raw->offset];
  switch (raw->kind) {
    case CHIMI_CONSTANT_VOID: return void_value();
    case CHIMI_CONSTANT_INTEGER: return integer_value(raw->integer);
    case CHIMI_CONSTANT_FLOAT: {
      uint64_t bits = (uint64_t)raw->integer; double value; memcpy(&value, &bits, sizeof(value));
      return float_value(value);
    }
    case CHIMI_CONSTANT_TEXT: return text_value(CHIMI_STRINGS[raw->a]);
    case CHIMI_CONSTANT_BOOLEAN: return boolean_value(raw->integer != 0);
    case CHIMI_CONSTANT_JSON_NUMBER: return json_number_value(CHIMI_STRINGS[raw->a]);
    case CHIMI_CONSTANT_LIST: {
      ChimiValue **items = allocate(raw->length, sizeof(*items));
      for (uint32_t i = 0; i < raw->length; ++i) items[i] = constant_values[ids[i]];
      return list_from_values(items, raw->length);
    }
    case CHIMI_CONSTANT_DURATION: return duration_value(raw->integer);
    case CHIMI_CONSTANT_PROCESS_ADDRESS: return process_address_value(CHIMI_ADDRESS_NAMED, (uint64_t)raw->a + 1);
    case CHIMI_CONSTANT_RANGE:
      return range_value(constant_values[ids[0]]->as.integer, constant_values[ids[1]]->as.integer);
    case CHIMI_CONSTANT_TUPLE: {
      ChimiValue *value = new_value(VALUE_TUPLE);
      value->as.record.length = raw->length;
      value->as.record.fields = allocate(raw->length, sizeof(*value->as.record.fields));
      allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
      for (uint32_t i = 0; i < raw->length; ++i)
        value->as.record.fields[i] = (RuntimeField){NULL, constant_values[ids[i]]};
      return value;
    }
    case CHIMI_CONSTANT_RECORD: {
      ChimiValue *value = new_value(VALUE_RECORD);
      value->as.record.length = raw->length;
      value->as.record.fields = allocate(raw->length, sizeof(*value->as.record.fields));
      allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
      for (uint32_t i = 0; i < raw->length; ++i)
        value->as.record.fields[i] = (RuntimeField){CHIMI_STRINGS[ids[2 * i]], constant_values[ids[2 * i + 1]]};
      return value;
    }
    case CHIMI_CONSTANT_CONSTRUCTED: {
      ChimiValue **fields = allocate(raw->length, sizeof(*fields));
      for (uint32_t i = 0; i < raw->length; ++i) fields[i] = constant_values[ids[i]];
      return construct_value(raw->a, fields)->value;
    }
    case CHIMI_CONSTANT_MAP: {
      MapNode *map = NULL;
      for (uint32_t i = 0; i < raw->length; ++i)
        map = map_put_node(map, constant_values[ids[2 * i]], constant_values[ids[2 * i + 1]]);
      return map_value(map);
    }
    default: break;
  }
  ChimiCallable *callable = allocate(1, sizeof(*callable));
  allocation_kind(callable, ALLOCATION_CALLABLE);
  const ChimiSymbol *shape = &CHIMI_SYMBOLS[raw->a];
  switch (raw->kind) {
    case CHIMI_CONSTANT_DECLARED: {
      const ChimiFunction *function = find_function(raw->a);
      callable->kind = CALL_DECLARED; callable->symbol = raw->a; callable->body = function->body;
      callable->parameters_offset = function->parameters_offset;
      callable->parameters_length = function->parameters_length;
      callable->environment = capture_requirements(new_environment()); callable->name = "function";
      bind_constant_requirements(callable->environment, ids, raw->integer);
      break;
    }
    case CHIMI_CONSTANT_INTRINSIC:
      callable->kind = CALL_INTRINSIC; callable->intrinsic = shape->intrinsic;
      callable->name = CHIMI_STRINGS[shape->name];
      break;
    case CHIMI_CONSTANT_CONSTRUCTOR:
      callable->kind = CALL_CONSTRUCTOR; callable->symbol = raw->a; callable->name = CHIMI_STRINGS[shape->name];
      break;
    case CHIMI_CONSTANT_CLOSURE: {
      const ChimiNode *node = &CHIMI_NODES[raw->a];
      ChimiEnvironment *environment = new_environment();
      allocation_kind(environment, ALLOCATION_ENVIRONMENT);
      for (uint32_t i = 0; i < raw->length; ++i)
        environment->locals[environment_slot(ids[2 * i])] = constant_values[ids[2 * i + 1]];
      bind_constant_requirements(environment, &ids[2 * raw->length], raw->integer);
      callable->kind = CALL_CLOSURE; callable->body = node->a;
      callable->parameters_offset = node->offset; callable->parameters_length = node->length;
      callable->environment = environment; callable->name = "closure";
      break;
    }
    default: abort();
  }
  return callable_value(callable);
}

static void decode_constants(void) {
  AllocationArea *previous = allocation_target;
  allocation_target = &static_area;
  constant_values = allocate(CHIMI_CONSTANT_VALUE_COUNT, sizeof(*constant_values));
  for (uint32_t i = 0; i < CHIMI_CONSTANT_VALUE_COUNT; ++i)
    constant_values[i] = decode_constant(&CHIMI_CONSTANT_VALUES[i]);
  allocation_target = previous;
}

static ChimiControl *eval_reference(ChimiRuntime *runtime, uint32_t node_id,
                                    ChimiEnvironment *environment, uint64_t span) {
  uint32_t symbol = CHIMI_NODES[node_id].a; const ChimiSymbol *shape = &CHIMI_SYMBOLS[symbol];
  switch (shape->kind) {
    case CHIMI_SYMBOL_FUNCTION: {
      const ChimiFunction *function = find_function(symbol);
      ChimiCallable *callable = allocate(1, sizeof(*callable));
      allocation_kind(callable, ALLOCATION_CALLABLE); callable->kind = CALL_DECLARED;
      callable->symbol = symbol; callable->body = function->body;
      callable->parameters_offset = function->parameters_offset; callable->parameters_length = function->parameters_length;
      callable->environment = capture_requirements(environment); callable->name = "function";
      return success(callable_value(callable));
    }
    case CHIMI_SYMBOL_CONSTANT:
      return success(constant_values[shape->constant]);
    case CHIMI_SYMBOL_REQUIREMENT:
      if (environment->requirements[shape->environment_slot])
        return success(environment->requirements[shape->environment_slot]);
      return defect("closed core has no provider", -1);
    case CHIMI_SYMBOL_LOCAL:
      if (environment->locals[shape->environment_slot])
        return success(environment->locals[shape->environment_slot]);
      return defect("verified local is not bound", -1);
    case CHIMI_SYMBOL_CONSTRUCTOR: {
      ChimiCallable *callable = allocate(1, sizeof(*callable));
      allocation_kind(callable, ALLOCATION_CALLABLE); callable->kind = CALL_CONSTRUCTOR;
      callable->symbol = symbol; callable->name = CHIMI_STRINGS[shape->name];
      return success(callable_value(callable));
    }
    case CHIMI_SYMBOL_INTRINSIC: {
      ChimiCallable *callable = allocate(1, sizeof(*callable));
      allocation_kind(callable, ALLOCATION_CALLABLE); callable->kind = CALL_INTRINSIC;
      callable->intrinsic = shape->intrinsic; callable->name = CHIMI_STRINGS[shape->name];
      return success(callable_value(callable));
    }
    case CHIMI_SYMBOL_PROCESS_NAME:
      return success(process_address_value(CHIMI_ADDRESS_NAMED, (uint64_t)symbol + 1));
    default: return defect("symbol is not a runtime value", -1);
  }
}

/* Core nodes share their incoming environment because ordinary evaluation only reads it.
   Sequence and provision clone before binding locals or requirements, and match arms clone
   before binding patterns. Any new node that mutates either table must establish the same
   isolation boundary instead of writing through this shared pointer. */
#define NODE_WRAPPER(NAME, OPERATION, ...) \
  ChimiControl *chimi_rt_##NAME(ChimiRuntime *runtime, uint32_t node_id, ChimiEnvironment *incoming, uint64_t parent) { \
    uint64_t span = begin_node(runtime, node_id, parent, OPERATION); ChimiEnvironment *environment = incoming; \
    (void)environment; \
    ChimiControl *result = NULL; do { __VA_ARGS__ } while (0); return finish_node(runtime, node_id, parent, span, OPERATION, result); \
  }
#define RUN(CHILD) run_node((CHILD), runtime, environment, span)
#define REQUIRE_SUCCESS(CONTROL, TARGET) do { ChimiControl *_control = (CONTROL); \
  if (_control->kind != CONTROL_SUCCESS) { result = _control; break; } (TARGET) = _control->value; } while (0)

NODE_WRAPPER(constant, "constant", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value;
  if (node->a == CHIMI_LITERAL_INTEGER) value = integer_value(node->integer);
  else if (node->a == CHIMI_LITERAL_FLOAT) {
    double raw; memcpy(&raw, &node->integer, sizeof(raw)); value = float_value(raw);
  } else if (node->a == CHIMI_LITERAL_TEXT) value = text_value(CHIMI_STRINGS[node->b]);
  else if (node->a == CHIMI_LITERAL_VOID) value = void_value();
  else value = boolean_value(node->integer != 0); result = success(value);
})
NODE_WRAPPER(reference, "reference", { result = eval_reference(runtime, node_id, environment, span); })
NODE_WRAPPER(select, "select", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *control = RUN(node->a);
  if (control->kind != CONTROL_SUCCESS) { result = control; break; }
  ChimiValue *value = select_value(control->value, CHIMI_STRINGS[node->b]);
  result = value ? success(value) : defect("verified value has no field", node_id);
})
NODE_WRAPPER(dependency_method, "dependency_method", {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  ChimiValue *provider = environment->requirements[environment_slot(node->a)];
  ChimiCallable *inner = NULL;
  if (!provider) {
    uint32_t standard_type = CHIMI_SYMBOLS[node->b].standard_type;
    inner = allocate(1, sizeof(*inner)); allocation_kind(inner, ALLOCATION_CALLABLE);
    inner->kind = CALL_HOST;
    if (standard_type == CHIMI_TYPE_FILE_SYSTEM && node->c == 0)
      inner->host = HOST_PROCESS_UTF8_LINES;
    else if (standard_type == CHIMI_TYPE_CONSOLE && node->c == 0)
      inner->host = HOST_WRITE_LINE;
    else if (standard_type == CHIMI_TYPE_CONSOLE && node->c == 1)
      inner->host = HOST_WRITE_ERROR;
    else { result = defect("closed core has no provider", node_id); break; }
  } else {
    ChimiValue *method = select_value(provider, CHIMI_STRINGS[node->d]);
    if (!method || method->kind != VALUE_CALLABLE) {
      result = defect("provider has no callable field", node_id); break;
    }
    inner = method->as.callable;
  }
  ChimiCallable *callable = allocate(1, sizeof(*callable));
  allocation_kind(callable, ALLOCATION_CALLABLE); callable->kind = CALL_DEPENDENCY;
  callable->inner = inner; callable->requirement = node->a; callable->interface_symbol = node->b;
  callable->method = node->c; callable->name = CHIMI_STRINGS[node->d]; result = success(callable_value(callable));
})

static ChimiControl *construct_value(uint32_t symbol, ChimiValue **values) {
  const ChimiConstructor *shape = &CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[symbol].constructor];
  if (shape->outcome_success >= 0) return shape->outcome_success ? success(values[0]) : failure(values[0]);
  ChimiValue *value = new_value(VALUE_CONSTRUCTED); value->as.constructed.constructor = symbol;
  value->as.constructed.type_name = CHIMI_STRINGS[shape->type_name];
  value->as.constructed.variant_name = shape->variant_name == UINT32_MAX ? NULL : CHIMI_STRINGS[shape->variant_name];
  value->as.constructed.length = shape->fields_length;
  value->as.constructed.fields = allocate(shape->fields_length, sizeof(*value->as.constructed.fields));
  allocation_kind(value->as.constructed.fields, ALLOCATION_FIELDS);
  for (uint32_t field = 0; field < shape->fields_length; ++field) {
    const char *name = CHIMI_STRINGS[CHIMI_IDS[shape->fields_offset + field]];
    value->as.constructed.fields[field] = (RuntimeField){name, values[field]};
  }
  return success(value);
}

NODE_WRAPPER(construct, "construct", {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  ChimiValue **values = allocate(node->length, sizeof(*values));
  bool stopped = false;
  for (uint32_t index = 0; index < node->length; ++index) {
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + index]);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    values[index] = control->value;
  }
  if (stopped) break;
  result = construct_value(node->a, values);
})

NODE_WRAPPER(call, "call", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *callee = RUN(node->a);
  if (callee->kind != CONTROL_SUCCESS) { result = callee; break; }
  if (callee->value->kind != VALUE_CALLABLE) { result = defect("verified call target is not callable", node_id); break; }
  ChimiValue **arguments = allocate(node->length, sizeof(*arguments)); bool stopped = false;
  for (uint32_t index = 0; index < node->length; ++index) {
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + index]);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    arguments[index] = control->value;
  }
  if (!stopped) result = invoke(runtime, callee->value->as.callable, arguments, node->length, span, node_id, environment);
})

NODE_WRAPPER(list, "list", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue **items = new_value_slots(node->length);
  bool stopped = false; for (uint32_t i = 0; i < node->length; ++i) {
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + i]);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    items[i] = control->value;
  } if (!stopped) result = success(list_from_values(items, node->length));
})

NODE_WRAPPER(record, "record", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = node->length; value->as.record.fields = allocate(node->length, sizeof(*value->as.record.fields));
  allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
  bool stopped = false; for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiNamedNode *field = &CHIMI_NAMED_NODES[node->offset + i]; ChimiControl *control = RUN(field->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    value->as.record.fields[i] = (RuntimeField){CHIMI_STRINGS[field->name], control->value};
  } if (!stopped) result = success(value);
})

NODE_WRAPPER(tuple, "tuple", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value = new_value(VALUE_TUPLE);
  value->as.record.length = node->length; value->as.record.fields = allocate(node->length, sizeof(*value->as.record.fields));
  allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
  bool stopped = false; for (uint32_t i = 0; i < node->length; ++i) {
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + i]);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    value->as.record.fields[i] = (RuntimeField){NULL, control->value};
  } if (!stopped) result = success(value);
})

NODE_WRAPPER(unary, "unary", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *control = RUN(node->a);
  if (control->kind != CONTROL_SUCCESS) { result = control; break; }
  if (control->value->kind == VALUE_FLOAT) { result = success(float_value(-control->value->as.floating)); break; }
  if (control->value->kind != VALUE_INTEGER) { result = defect("verified unary operand is not an integer", node_id); break; }
  if (control->value->as.integer == INT64_MIN) {
    result = defect(numeric_message("integer overflow in unary `-`: -(%" PRId64 ")", INT64_MIN), node_id); break;
  }
  result = success(integer_value(-control->value->as.integer));
})

NODE_WRAPPER(binary, "binary", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *left = RUN(node->b);
  if (left->kind != CONTROL_SUCCESS) { result = left; break; }
  ChimiControl *right = RUN(node->c); if (right->kind != CONTROL_SUCCESS) { result = right; break; }
  if (is_arithmetic(node->a)) {
    if (left->value->kind == VALUE_INTEGER && right->value->kind == VALUE_INTEGER)
      result = integer_arithmetic(node->a, left->value->as.integer, right->value->as.integer, node_id);
    else if (left->value->kind == VALUE_FLOAT && right->value->kind == VALUE_FLOAT)
      result = float_arithmetic(node->a, left->value->as.floating, right->value->as.floating, node_id);
    else
      result = defect("verified binary operands are not numbers", node_id);
    break;
  }
  if (node->a == CHIMI_BINARY_RANGE || node->a == CHIMI_BINARY_RANGE_INCLUSIVE) {
    if (left->value->kind != VALUE_INTEGER || right->value->kind != VALUE_INTEGER) {
      result = defect("verified range bounds are not integers", node_id); break;
    }
    int64_t end = right->value->as.integer;
    if (node->a == CHIMI_BINARY_RANGE_INCLUSIVE) {
      if (end == INT64_MAX) {
        result = defect("the inclusive range `..=` ends at the largest Int, so its exclusive end overflows", node_id);
        break;
      }
      ++end;
    }
    result = success(range_value(left->value->as.integer, end)); break;
  }
  int order = chimi_value_compare(left->value, right->value);
  switch (node->a) {
    case CHIMI_BINARY_EQUAL: result = success(boolean_value(order == 0)); break;
    case CHIMI_BINARY_LESS: result = success(boolean_value(order < 0)); break;
    case CHIMI_BINARY_LESS_EQUAL: result = success(boolean_value(order <= 0)); break;
    case CHIMI_BINARY_GREATER: result = success(boolean_value(order > 0)); break;
    case CHIMI_BINARY_GREATER_EQUAL: result = success(boolean_value(order >= 0)); break;
    default: result = defect("verified binary operator is unknown", node_id); break;
  }
})

NODE_WRAPPER(identity, "identity", { result = RUN(CHIMI_NODES[node_id].a); })

NODE_WRAPPER(provision, "provision", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; bool stopped = false;
  environment = clone_requirement_environment(incoming);
  for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiNamedNode *provider = &CHIMI_NAMED_NODES[node->offset + i]; ChimiControl *control = RUN(provider->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    const char *name = CHIMI_STRINGS[provider->name];
    for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
      if (CHIMI_SYMBOLS[symbol].kind == CHIMI_SYMBOL_REQUIREMENT &&
          strcmp(CHIMI_STRINGS[CHIMI_SYMBOLS[symbol].name], name) == 0)
        environment->requirements[CHIMI_SYMBOLS[symbol].environment_slot] = control->value;
  } if (!stopped) result = RUN(node->a);
})

NODE_WRAPPER(sequence, "sequence", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *last = NULL; bool stopped = false;
  environment = clone_local_environment(incoming);
  for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiStep *step = &CHIMI_STEPS[node->offset + i]; ChimiControl *control = RUN(step->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    last = control->value;
    if (step->kind == 0) environment->locals[environment_slot(step->symbol)] = last;
  } if (!stopped) result = success(last);
})

NODE_WRAPPER(closure, "closure", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiCallable *callable = allocate(1, sizeof(*callable));
  allocation_kind(callable, ALLOCATION_CALLABLE);
  callable->kind = CALL_CLOSURE; callable->body = node->a; callable->parameters_offset = node->offset;
  callable->parameters_length = node->length;
  callable->environment = capture_closure_environment(environment, node);
  callable->name = "closure"; result = success(callable_value(callable));
})

typedef enum { MATCH_PLAIN, MATCH_SUCCESS, MATCH_FAILURE } MatchKind;
typedef struct { MatchKind kind; ChimiValue *value; } MatchValue;

static bool literal_matches(const ChimiPattern *pattern, const ChimiValue *value) {
  if (pattern->a == CHIMI_LITERAL_INTEGER) return value->kind == VALUE_INTEGER && value->as.integer == pattern->integer;
  if (pattern->a == CHIMI_LITERAL_TEXT) return value->kind == VALUE_TEXT && strcmp(value->as.text, CHIMI_STRINGS[pattern->offset]) == 0;
  return value->kind == VALUE_BOOLEAN && value->as.boolean == (pattern->integer != 0);
}
/* The binding pass of a match. `view_failed` records a list view that could not be
   taken, which the match reports as a defect. */
typedef struct { ChimiEnvironment *environment; bool view_failed; } PatternBinder;

static bool match_plain(uint32_t pattern_id, ChimiValue *value, PatternBinder *binder);
/* Checks the length first, then reads each fixed element by index. In the binding
   pass a named rest receives the view between the prefix and the suffix. */
static bool match_list(const ChimiPattern *pattern, ChimiValue *value, PatternBinder *binder) {
  if (value->kind != VALUE_LIST) return false;
  uint32_t length = list_length(value), prefix = pattern->a, has_rest = pattern->integer != 0;
  uint32_t suffix = pattern->length - prefix - has_rest;
  if (has_rest ? length < prefix + suffix : length != prefix) return false;
  uint32_t suffix_start = length - suffix;
  for (uint32_t index = 0; index < prefix; ++index)
    if (!match_plain(CHIMI_IDS[pattern->offset + index], list_get(value, index), binder)) return false;
  for (uint32_t index = 0; index < suffix; ++index)
    if (!match_plain(CHIMI_IDS[pattern->offset + prefix + has_rest + index],
                     list_get(value, suffix_start + index), binder))
      return false;
  if (!has_rest || !binder) return true;
  uint32_t rest_id = CHIMI_IDS[pattern->offset + prefix];
  if (CHIMI_PATTERNS[rest_id].kind == CHIMI_PATTERN_WILDCARD) return true;
  ChimiValue *view = list_slice(value, prefix, suffix_start);
  if (!view) { binder->view_failed = true; return false; }
  return match_plain(rest_id, view, binder);
}
static bool match_plain(uint32_t pattern_id, ChimiValue *value, PatternBinder *binder) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  switch (pattern->kind) {
    case CHIMI_PATTERN_WILDCARD: return true;
    case CHIMI_PATTERN_BINDING:
      if (binder) binder->environment->locals[environment_slot(pattern->a)] = value;
      return true;
    case CHIMI_PATTERN_LITERAL: return literal_matches(pattern, value);
    case CHIMI_PATTERN_CONSTRUCTOR:
      if (value->kind != VALUE_CONSTRUCTED || value->as.constructed.constructor != pattern->a ||
          value->as.constructed.length != pattern->length) return false;
      for (uint32_t i = 0; i < pattern->length; ++i)
        if (!match_plain(CHIMI_IDS[pattern->offset + i], value->as.constructed.fields[i].value, binder)) return false;
      return true;
    case CHIMI_PATTERN_OUTCOME: return false;
    case CHIMI_PATTERN_LIST: return match_list(pattern, value, binder);
    case CHIMI_PATTERN_TUPLE:
      if (value->kind != VALUE_TUPLE || value->as.record.length != pattern->length) return false;
      for (uint32_t i = 0; i < pattern->length; ++i)
        if (!match_plain(CHIMI_IDS[pattern->offset + i], value->as.record.fields[i].value, binder)) return false;
      return true;
    case CHIMI_PATTERN_ALIAS:
      if (!match_plain(pattern->offset, value, binder)) return false;
      if (binder) binder->environment->locals[environment_slot(pattern->a)] = value;
      return true;
  }
  return false;
}
static bool match_value(uint32_t pattern_id, MatchValue value, PatternBinder *binder) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  if (pattern->kind == CHIMI_PATTERN_OUTCOME) {
    MatchKind expected = pattern->a ? MATCH_SUCCESS : MATCH_FAILURE;
    return value.kind == expected && match_plain(pattern->offset, value.value, binder);
  }
  return value.kind == MATCH_PLAIN && match_plain(pattern_id, value.value, binder);
}
static bool pattern_binds(uint32_t pattern_id) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  if (pattern->kind == CHIMI_PATTERN_BINDING || pattern->kind == CHIMI_PATTERN_ALIAS) return true;
  if (pattern->kind == CHIMI_PATTERN_OUTCOME) return pattern_binds(pattern->offset);
  if (pattern->kind != CHIMI_PATTERN_CONSTRUCTOR && pattern->kind != CHIMI_PATTERN_LIST &&
      pattern->kind != CHIMI_PATTERN_TUPLE) return false;
  for (uint32_t index = 0; index < pattern->length; ++index)
    if (pattern_binds(CHIMI_IDS[pattern->offset + index])) return true;
  return false;
}

NODE_WRAPPER(match, "match", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; MatchValue *values = allocate(node->length, sizeof(*values)); bool stopped = false;
  for (uint32_t subject = 0; subject < node->length; ++subject) {
    bool contextual = false;
    for (uint32_t arm_index = 0; arm_index < node->b; ++arm_index) {
      const ChimiArm *arm = &CHIMI_ARMS[node->a + arm_index];
      if (CHIMI_PATTERNS[CHIMI_IDS[arm->patterns_offset + subject]].kind == CHIMI_PATTERN_OUTCOME) contextual = true;
    }
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + subject]);
    if (contextual && control->kind == CONTROL_SUCCESS) values[subject] = (MatchValue){MATCH_SUCCESS, control->value};
    else if (contextual && control->kind == CONTROL_FAILURE) values[subject] = (MatchValue){MATCH_FAILURE, control->value};
    else if (!contextual && control->kind == CONTROL_SUCCESS) values[subject] = (MatchValue){MATCH_PLAIN, control->value};
    else { result = control; stopped = true; break; }
  }
  if (stopped) break;
  for (uint32_t arm_index = 0; arm_index < node->b; ++arm_index) {
    const ChimiArm *arm = &CHIMI_ARMS[node->a + arm_index]; bool matched = true;
    for (uint32_t subject = 0; subject < node->length; ++subject)
      if (!match_value(CHIMI_IDS[arm->patterns_offset + subject], values[subject], NULL)) { matched = false; break; }
    if (!matched) continue;
    bool binds = false;
    for (uint32_t subject = 0; subject < node->length; ++subject)
      if (pattern_binds(CHIMI_IDS[arm->patterns_offset + subject])) { binds = true; break; }
    ChimiEnvironment *arm_environment = binds ? clone_local_environment(environment) : environment;
    PatternBinder binder = {arm_environment, false};
    for (uint32_t subject = 0; binds && subject < node->length; ++subject)
      if (!match_value(CHIMI_IDS[arm->patterns_offset + subject], values[subject], &binder)) {
        result = defect(binder.view_failed ? "verified list pattern took a view outside its list"
                                           : "verified pattern changed while binding", node_id);
        stopped = true; break;
      }
    if (stopped) break;
    if (arm->guard != UINT32_MAX) {
      ChimiControl *guard = run_node(arm->guard, runtime, arm_environment, span);
      if (guard->kind != CONTROL_SUCCESS) { result = guard; stopped = true; break; }
      if (guard->value->kind != VALUE_BOOLEAN) { result = defect("verified case guard is not Boolean", node_id); stopped = true; break; }
      if (!guard->value->as.boolean) continue;
    }
    result = run_node(arm->body, runtime, arm_environment, span); stopped = true; break;
  }
  if (!stopped) {
    for (uint32_t i = 0; i < node->length; ++i) if (values[i].kind == MATCH_FAILURE) { result = failure(values[i].value); stopped = true; break; }
    if (!stopped) result = defect("verified exhaustive match had no matching arm", node_id);
  }
})

/* Runs passes in a C loop so stack use is independent of the pass count. The loop owns
   one cloned environment and rebinds only its state slot between passes; that is safe
   because closures copy captured slots and binding nodes clone before writing. Each
   pass checks cancellation first and runs under this node's span, so pass operations
   are siblings.

   A pass's allocations are reclaimed once it ends, keeping only what the next state
   reaches. Values are immutable and the loop environment lives in the enclosing area,
   so the state is the only path from one pass to the next. Under the scheduler each
   task has its own heap, and values cross between tasks only as fragment copies, so a
   pass never frees what another task can reach. */
NODE_WRAPPER(loop, "loop", {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  ChimiControl *initial = RUN(node->b);
  if (initial->kind != CONTROL_SUCCESS) { result = initial; break; }
  environment = clone_local_environment(incoming);
  uint32_t state_slot = environment_slot(node->a);
  ChimiValue *state = initial->value;
  AllocationScope passes;
  allocation_scope_begin(&passes);
  for (;;) {
    if (runtime->cancel) { result = cancelled(); break; }
    environment->locals[state_slot] = state;
    ChimiControl *pass = RUN(node->c);
    if (pass->kind == CONTROL_SUCCESS) {
      state = pass->value;
      if (allocation_scope_collect(&passes, state, NULL) != HEAP_OK) {
        result = defect(FOREIGN_POINTER_DEFECT, (int64_t)node_id);
        break;
      }
      continue;
    }
    if (pass->kind == CONTROL_BREAK && pass->node == (int64_t)node->a) {
      result = success(pass->value);
    } else {
      result = pass;
    }
    break;
  }
  result = end_measured_allocation_scope(runtime, &passes, result);
})
NODE_WRAPPER(break, "break", {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  ChimiValue *value = void_value();
  if (node->b != UINT32_MAX) {
    ChimiControl *control = RUN(node->b);
    if (control->kind != CONTROL_SUCCESS) { result = control; break; }
    value = control->value;
  }
  result = new_control(CONTROL_BREAK, value, NULL, node->a);
})

static void print_outcome(FILE *output, const ChimiControl *control) {
  fputs("{\"schema\":\"chimi.runtime.v1\",", output);
  switch (control->kind) {
    case CONTROL_SUCCESS:
      fputs("\"outcome\":\"success\",\"value\":", output); json_value(output, control->value); break;
    case CONTROL_FAILURE:
      fputs("\"outcome\":\"expected_failure\",\"error\":", output); json_value(output, control->value); break;
    case CONTROL_DEFECT:
      fputs("\"outcome\":\"defect\",\"message\":", output); json_string(output, control->message);
      if (control->node < 0) fputs(",\"core_node_id\":null,\"origin_id\":null", output);
      else fprintf(output, ",\"core_node_id\":%" PRId64 ",\"origin_id\":%u",
                   control->node, CHIMI_NODES[control->node].origin);
      break;
    case CONTROL_CANCEL:
      fputs("\"outcome\":\"cancellation\",\"reason\":\"requested before the next semantic operation\"", output); break;
  }
  fputs("}\n", output);
}

static bool close_output(FILE *output) {
  if (output == NULL) return false;
  bool failed = fflush(output) != 0;
  if (fclose(output) != 0) failed = true;
  return failed;
}

static bool write_memory_report(const char *path, const ChimiRuntime *runtime) {
  if (!path) return false;
  FILE *output = fopen(path, "wx");
  if (!output) {
    fprintf(stderr, "cannot open memory report file %s: %s\n", path, strerror(errno));
    return true;
  }
  fprintf(output,
          "{\"schema\":\"chimi.runtime-memory.v1\",\"scoped_total_allocated_bytes\":%zu,"
          "\"scoped_allocated_bytes_by_kind\":{\"bytes\":%zu,\"values\":%zu,"
          "\"controls\":%zu,\"environments\":%zu,\"value_pointers\":%zu,"
          "\"fields\":%zu,\"callables\":%zu,\"map_nodes\":%zu,\"list_nodes\":%zu},"
          "\"scoped_peak_live_bytes\":%zu,\"scoped_retained_bytes\":%zu,"
          "\"scoped_collections\":%" PRIu64 ",\"scoped_full_collections\":%" PRIu64
          ",\"scoped_scanned_allocations\":%" PRIu64 ",\"unscoped_allocated_bytes\":%zu,\"peak_footprint_bytes\":%zu,"
          "\"peak_fragment_footprint_bytes\":%zu,\"capture_transfer_probes\":%" PRIu64
          ",\"heap_chunks_live\":%zu,\"heap_chunk_bytes_peak\":%zu,\"fragments_live\":%zu"
          ",\"fragment_bytes_peak\":%zu,\"process_heap_peak_value_bytes\":%zu"
          ",\"copied_values\":%" PRIu64 ",\"copied_bytes\":%" PRIu64
          ",\"trace_events_constructed\":%" PRIu64 "}\n",
          runtime->scoped_total_allocated_bytes,
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_BYTES],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_VALUE],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_CONTROL],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_ENVIRONMENT] +
            runtime->scoped_allocated_bytes_by_kind[ALLOCATION_FRAME_ENVIRONMENT],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_VALUE_POINTERS],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_FIELDS],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_CALLABLE],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_MAP_NODE],
          runtime->scoped_allocated_bytes_by_kind[ALLOCATION_LIST_NODE],
          runtime->scoped_peak_live_bytes,
          runtime->scoped_retained_bytes,
          runtime->scoped_collections,
          runtime->scoped_full_collections,
          runtime->scoped_scanned_allocations,
          unscoped_allocated_bytes,
          heap_totals.peak_footprint_bytes,
          heap_totals.peak_fragment_footprint_bytes,
          capture_transfer_probes,
          /* runtime.c has no chunks: an allocation is its unit of heap memory. */
          heap_totals.live_allocations,
          heap_totals.peak_footprint_bytes,
          heap_totals.live_fragments,
          heap_totals.peak_fragment_footprint_bytes,
#if CHIMI_PROCESS_KERNEL
          process_heap_peak_value_bytes,
#else
          (size_t)0,
#endif
          copied_values,
          copied_bytes,
          runtime->trace_events_constructed);
  return close_output(output);
}

static const char *scheduler_event_name(const ChimiSchedulerTraceEvent *event) {
  static const char *task_names[] = {
    "task_started", "task_parked", "task_woken", "task_cancellation_requested",
    "task_cleanup_completed", "task_terminal",
  };
  static const char *channel_names[] = {
    "channel_created", "channel_send_accepted", "channel_send_parked",
    "channel_send_rejected_closed", "channel_send_rejected_aborted",
    "channel_send_cancelled", "channel_receive_delivered", "channel_receive_parked",
    "channel_receive_cancelled", "channel_closed", "channel_drained",
    "channel_aborted", "channel_receiver_transferred", "channel_completion_transferred",
    "channel_released",
  };
  if (event->family == CHIMI_SCHEDULER_TRACE_TASK &&
      event->kind > 0 && event->kind <= sizeof(task_names) / sizeof(*task_names))
    return task_names[event->kind - 1];
  static const char *clock_names[] = {
    "clock_advanced", "clock_timer_started", "clock_timer_fired",
    "clock_timer_cancelled", "clock_deadlock_waiter",
  };
  if (event->family == CHIMI_SCHEDULER_TRACE_CHANNEL &&
      event->kind > 0 && event->kind <= sizeof(channel_names) / sizeof(*channel_names))
    return channel_names[event->kind - 1];
  if (event->family == CHIMI_SCHEDULER_TRACE_CLOCK &&
      event->kind > 0 && event->kind <= sizeof(clock_names) / sizeof(*clock_names))
    return clock_names[event->kind - 1];
  /* The transcript table's names, in trace-kind order. */
  static const char *process_names[] = {
    "process_started", "admission_rejected", "process_terminated", "restart_attempted",
    "restart_limit_exceeded", "supervisor_escalated", "membership_removed",
    "supervisor_stopped", "process_send", "supervisor_started",
  };
  if (event->family == CHIMI_SCHEDULER_TRACE_PROCESS &&
      event->kind > 0 && event->kind <= sizeof(process_names) / sizeof(*process_names))
    return process_names[event->kind - 1];
  return "scheduler_event";
}

static void json_identity(FILE *output, const char *key, uint64_t identity) {
  fprintf(output, ",\"%s\":", key);
  if (identity == 0) fputs("null", output); else fprintf(output, "%" PRIu64, identity);
}

static void json_word(FILE *output, const char *key, const char *word) {
  fprintf(output, ",\"%s\":", key);
  if (word) json_string(output, word); else fputs("null", output);
}

/* The word of a process event's `cause`: a terminal cause, or an admission reason. */
static const char *process_cause_word(const ChimiSchedulerTraceEvent *event) {
  static const char *terminal[] = {"returned", "failed", "defected", "cancelled", "forced"};
  static const char *admission[] = {"capacity_reached", "name_taken", "stopped", "start_failed"};
  if (event->kind == CHIMI_PROCESS_TRACE_TERMINATED && event->cause >= 1 && event->cause <= 5)
    return terminal[event->cause - 1];
  if (event->kind == CHIMI_PROCESS_TRACE_ADMISSION_REJECTED && event->cause >= 1 && event->cause <= 4)
    return admission[event->cause - 1];
  return NULL;
}

/* The process family's fields (supervised-process-conformance.md, "Runner telemetry
   for process and clock events"): identities at the top level, the rest in
   `attributes`, a field the kind does not set as null. */
static void emit_process_fields(FILE *output, const ChimiSchedulerTraceEvent *event, bool attributes) {
  if (!attributes) {
    json_identity(output, "supervisor_id", event->supervisor);
    json_identity(output, "membership_id", event->membership);
    json_identity(output, "incarnation_id", event->incarnation);
    json_identity(output, "parent_supervisor_id", event->parent_supervisor);
    return;
  }
  bool send = event->kind == CHIMI_PROCESS_TRACE_SEND;
  static const char *outcomes[] = {"accepted", "full", "unavailable"};
  fprintf(output, ",\"time_ns\":%" PRId64, event->time);
  json_word(output, "cause", process_cause_word(event));
  json_word(output, "name", process_name_label(event->name));
  json_word(output, "address_kind",
            !send ? NULL : event->address_kind == CHIMI_ADDRESS_NAMED ? "named"
            : event->address_kind == CHIMI_ADDRESS_DIRECT ? "direct" : NULL);
  json_word(output, "outcome", send && event->outcome >= 1 && event->outcome <= 3 ? outcomes[event->outcome - 1] : NULL);
  fprintf(output, ",\"attempts_in_window\":%" PRIu64, event->attempts_in_window);
  /* `defect`: why a defected incarnation ended. A stack overflow ends the body
     without returning, so the scheduler reports it (CHIMI_PROCESS_DEFECT_*). */
  char *defect = NULL;
  const char *named = NULL;
  if (event->kind == CHIMI_PROCESS_TRACE_TERMINATED && event->cause == CHIMI_PROCESS_CAUSE_DEFECTED) {
    if (event->outcome == CHIMI_PROCESS_DEFECT_STACK_OVERFLOW) named = "Process stack overflow";
#if CHIMI_PROCESS_KERNEL
    defect = take_process_defect(event->incarnation);
    if (!named) named = defect;
#endif
  }
  json_word(output, "defect", named);
  free(defect);
}

static void emit_scheduler_event(void *context, const ChimiSchedulerTraceEvent *event) {
  ChimiRuntime *runtime = context;
  FILE *output = runtime->telemetry;
  ++runtime->trace_events_constructed;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":", output);
  json_string(output, scheduler_event_name(event));
  fprintf(output, ",\"sequence\":%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, CHIMI_COMPUTATION);
  fputs(",\"execution_id\":", output);
  json_string(output, runtime->execution_id);
  fputs(",\"task_id\":", output);
  if (event->task == 0) fputs("null", output); else fprintf(output, "%" PRIu64, event->task);
  fputs(",\"parent_task_id\":", output);
  if (event->parent == 0) fputs("null", output); else fprintf(output, "%" PRIu64, event->parent);
  /* A clock event's `cause` is the waiter's request kind, and a process event's is
     a cause code, not a task (supervised-process-conformance.md, "Runner telemetry"). */
  bool clock = event->family == CHIMI_SCHEDULER_TRACE_CLOCK;
  bool process = event->family == CHIMI_SCHEDULER_TRACE_PROCESS;
  fputs(",\"cause_task_id\":", output);
  if (clock || process || event->cause == 0) fputs("null", output);
  else fprintf(output, "%" PRIu64, event->cause);
  fputs(",\"channel_id\":", output);
  if (event->channel == 0) fputs("null", output); else fprintf(output, "%" PRIu64, event->channel);
  if (process) emit_process_fields(output, event, false);
  fprintf(output,
          ",\"attributes\":{\"scheduler_sequence\":%" PRIu64
          ",\"capacity\":%" PRIu64 ",\"queue_depth\":%" PRIu64
          ",\"waiter_depth\":%" PRIu64 ",\"discard_count\":%" PRIu64,
          event->sequence, event->capacity, event->queue_depth,
          event->waiter_depth, event->discard_count);
  if (clock)
    fprintf(output, ",\"time_ns\":%" PRId64 ",\"request_kind\":%" PRIu64,
            event->time, event->cause);
  if (process) emit_process_fields(output, event, true);
  fputs("}}\n", output);
}

/* The operation a parked task waits on, by its scheduler request kind. */
static const char *waited_operation(uint32_t request_kind) {
  switch (request_kind) {
    case CHIMI_REQUEST_TASK_RESULT: return "Task.result";
    case CHIMI_REQUEST_CHANNEL_SEND: return "Channel.send";
    case CHIMI_REQUEST_CHANNEL_RECEIVE: return "Channel.receive";
    default: return "a scheduler request";
  }
}

/* A task the scheduler defected for a channel capability violation and the defect
   message that names the capability, the task, and the capability's owner. */
typedef struct CapabilityViolationNote {
  uint64_t task;
  char *message;
  struct CapabilityViolationNote *next;
} CapabilityViolationNote;

static const char *capability_name(uint32_t capability) {
  switch (capability) {
    case CHIMI_CAPABILITY_RECEIVER: return "Receiver";
    case CHIMI_CAPABILITY_COMPLETION: return "Completion";
    default: return "capability";
  }
}

/* The surface operation behind a channel request kind. */
static const char *channel_operation(uint32_t request_kind) {
  switch (request_kind) {
    case CHIMI_REQUEST_CHANNEL_SEND: return "Channel.send";
    case CHIMI_REQUEST_CHANNEL_RECEIVE: return "Channel.receive";
    case CHIMI_REQUEST_CHANNEL_CLOSE: return "Channel.close";
    case CHIMI_REQUEST_CHANNEL_ABORT: return "aborting the channel";
    case CHIMI_REQUEST_CHANNEL_RELEASE: return "releasing the channel";
    default: return "a channel request";
  }
}

/* Records the defect message of a channel capability violation
   (task-and-channel-capabilities.md, "Checked at run time"). The exclusive-move
   rule does not follow a capability across function boundaries, so this is the
   check that turns two owners of one Receiver or Completion into a defect. */
static void record_capability_violation(void *context, const ChimiCapabilityViolation *violation) {
  ChimiRuntime *runtime = context;
  char holder[96];
  if (violation->owner == 0)
    snprintf(holder, sizeof(holder), "the task it was handed to has ended");
  /* The task owns the capability, so the copy it used failed the token check:
     another copy has replaced it. */
  else if (violation->owner == violation->task)
    snprintf(holder, sizeof(holder), "this copy is stale, and task %" PRIu64
             " holds the current one", violation->task);
  else
    snprintf(holder, sizeof(holder), "task %" PRIu64 " owns it", violation->owner);
  char text[320];
  bool transfer = violation->request_kind == CHIMI_REQUEST_CHANNEL_TRANSFER_RECEIVER ||
                  violation->request_kind == CHIMI_REQUEST_CHANNEL_TRANSFER_COMPLETION;
  switch (violation->reason) {
    case CHIMI_VIOLATION_WRONG_OWNER:
      if (transfer)
        snprintf(text, sizeof(text),
                 "channel capability violation: task %" PRIu64 " cannot hand the %s of channel %"
                 PRIu64 " to task %" PRIu64 ": %s", violation->task,
                 capability_name(violation->capability), violation->channel,
                 violation->destination, holder);
      else
        snprintf(text, sizeof(text),
                 "channel capability violation: task %" PRIu64 " cannot use the %s of channel %"
                 PRIu64 " in %s: %s", violation->task, capability_name(violation->capability),
                 violation->channel, channel_operation(violation->request_kind), holder);
      break;
    case CHIMI_VIOLATION_OVERLAPPING_OPERATION:
      snprintf(text, sizeof(text),
               "channel capability violation: task %" PRIu64
               " started a channel operation while its previous one was still pending",
               violation->task);
      break;
    case CHIMI_VIOLATION_PENDING_OPERATION:
      snprintf(text, sizeof(text),
               "channel capability violation: task %" PRIu64 " cannot hand a channel capability to task %"
               PRIu64 " while an operation on it is pending", violation->task, violation->destination);
      break;
    case CHIMI_VIOLATION_UNKNOWN_CHANNEL:
      snprintf(text, sizeof(text),
               "channel capability violation: task %" PRIu64 " used channel %" PRIu64
               " after it was released", violation->task, violation->channel);
      break;
    default:
      snprintf(text, sizeof(text),
               "channel capability violation: task %" PRIu64 " released channel %" PRIu64
               " while a task still holds it", violation->task, violation->channel);
      break;
  }
  CapabilityViolationNote *note = malloc(sizeof(*note));
  char *message = strdup(text);
  if (!note || !message) { fputs("Chimi runtime allocation failed\n", stderr); exit(70); }
  *note = (CapabilityViolationNote){
    .task = violation->task, .message = message, .next = runtime->capability_violations};
  runtime->capability_violations = note;
}

/* The capability-violation message of the task the scheduler defected, if any. */
static const char *capability_violation_of(ChimiRuntime *runtime, uint64_t task) {
  for (CapabilityViolationNote *note = runtime->capability_violations; note; note = note->next)
    if (note->task == task) return note->message;
  return NULL;
}

/* Builds the root's virtual-time deadlock defect: each parked task and the
   operation it waits on (time-and-outcomes.md, "Virtual time"). */
static void record_deadlock(void *context, const ChimiDeadlockWaiter *waiters, size_t count) {
  ChimiRuntime *runtime = context;
  static const char prefix[] =
    "virtual-time deadlock: every task is parked and no timer is pending:";
  size_t capacity = sizeof(prefix) + count * 64;
  char *message = malloc(capacity);
  if (!message) { fputs("Chimi runtime allocation failed\n", stderr); exit(70); }
  size_t length = (size_t)snprintf(message, capacity, "%s", prefix);
  for (size_t index = 0; index < count; ++index)
    length += (size_t)snprintf(message + length, capacity - length, "%s task %" PRIu64 " waits on %s",
                               index == 0 ? "" : ";", waiters[index].task,
                               waited_operation(waiters[index].request_kind));
  runtime->deadlock_waiters = message;
}

static ChimiControl *execute_program_entry(ChimiRuntime *runtime, ChimiNodeFn entry,
                                           ChimiEnvironment *environment) {
  if (!CHIMI_REQUIRES_SCHEDULER) return entry(runtime, environment, 1);
  ChimiTaskContext context = {
    .runtime = runtime,
    .environment = environment,
    .entry = entry,
    .span = 1,
  };
  ChimiSchedulerConfig config = {
    .entry = chimi_task_entry,
    .context = &context,
    /* Every task and incarnation gets a guarded 1 MiB stack. Pages are touched on
       use, so a parked fiber's resident cost follows its deepest call. */
    .stack_bytes = CHIMI_FIBER_STACK_BYTES,
    .trace_mode = runtime->silent ? 0 : (runtime->full_trace ? 2 : 1),
    .clock_mode = runtime->virtual_clock ? CHIMI_SCHEDULER_CLOCK_VIRTUAL : CHIMI_SCHEDULER_CLOCK_REAL,
    .trace_sink = emit_scheduler_event,
    .trace_context = runtime,
    .release_context = runtime,
    .deadlock_report = record_deadlock,
    .deadlock_context = runtime,
    .capability_violation_report = record_capability_violation,
    .capability_violation_context = runtime,
    .release_task_value = release_task_word,
    .release_channel_value = release_channel_word,
    .release_task_context = release_task_context,
#if CHIMI_PROCESS_KERNEL
    .release_message = release_message_word,
    .release_spec_context = release_spec_context_word,
#endif
  };
  channel_endpoints_reset();
  ChimiSchedulerOutcome outcome = chimi_scheduler_run(&config);
  current_heap = &main_heap;
  /* The outcome's word is ours: copy a child's result fragment, then free it. */
  if (outcome.state == CHIMI_FIBER_SUCCEEDED || outcome.state == CHIMI_FIBER_DEFECTED) {
    int64_t word = outcome.state == CHIMI_FIBER_SUCCEEDED ? outcome.value : outcome.defect_code;
    if (word > 0 && (word & CHILD_RESULT_TAG)) {
      ChimiControl *control = task_reply(word);
      release_task_word(runtime, word);
      return control;
    }
  }
  if (outcome.state == CHIMI_FIBER_SUCCEEDED)
    return task_reply(outcome.value);
  if (outcome.state == CHIMI_FIBER_CANCELLED) return cancelled();
  /* A positive code is a task's result word. A negative code is one the scheduler
     made, for example when nothing can run and the root is still parked. */
  if (outcome.state == CHIMI_FIBER_DEFECTED && outcome.defect_code > 0)
    return task_reply(outcome.defect_code);
  if (outcome.state == CHIMI_FIBER_DEFECTED && outcome.defect_code == CHIMI_SCHEDULER_DEFECT_DEADLOCK &&
      runtime->deadlock_waiters)
    return defect(runtime->deadlock_waiters, -1);
  const char *violation = outcome.state == CHIMI_FIBER_DEFECTED && outcome.defect_code < 0
    ? capability_violation_of(runtime, outcome.origin) : NULL;
  if (violation) return defect(violation, -1);
  /* A root supervisor escalated: the scheduler ends every root task with this code,
     parked or runnable, and the program ends with the escalation's defect. */
  if (outcome.state == CHIMI_FIBER_DEFECTED &&
      outcome.defect_code == CHIMI_SCHEDULER_DEFECT_RESTART_LIMIT_EXCEEDED)
    return defect("Supervisor.RestartLimitExceeded", -1);
  /* Under the real clock a root parked with nothing pending stays suspended,
     because a host event might wake it. The native runtime has no
     asynchronous host operations, so nothing ever will: report the same
     deadlock as before, leaving the suspended tasks to process exit. */
  if (outcome.state == CHIMI_FIBER_PARKED ||
      (outcome.state == CHIMI_FIBER_DEFECTED && outcome.defect_code < 0))
    return defect("Task scheduler defected: the root task could not finish, "
                  "because every task is parked (deadlock) or the scheduler failed", -1);
  if (outcome.state == CHIMI_FIBER_STACK_OVERFLOW)
    return defect("Task stack overflow", -1);
  return defect("Task scheduler failed to produce a terminal root", -1);
}

int32_t chimi_rt_main(ChimiNodeFn entry, int32_t argc, const char **argv) {
  struct timespec identity_time;
  clock_gettime(CLOCK_REALTIME, &identity_time);
  char generated_execution_id[96];
  snprintf(generated_execution_id, sizeof(generated_execution_id),
           "execution-%ld-%" PRId64 "-%ld", (long)getpid(),
           (int64_t)identity_time.tv_sec, identity_time.tv_nsec);
  ChimiRuntime runtime = {
    .execution_id = generated_execution_id,
    .silent = true,
    .max_line_bytes = 1024 * 1024,
    .next_span = 2,
  };
  uint32_t benchmark_repeats = 0;
  const char *telemetry_path = NULL;
  const char *outcome_path = NULL;
  const char *memory_report_path = NULL;
  decode_constants();
  int32_t index = 1;
  if (index < argc && strcmp(argv[index], "--chimi-runner") == 0) {
    ++index;
    bool found_boundary = false;
    for (; index < argc; ++index) {
      if (strcmp(argv[index], "--") == 0) {
        found_boundary = true;
        ++index;
        break;
      }
    if (strcmp(argv[index], "--trace=full") == 0) { runtime.full_trace = true; runtime.silent = false; }
    else if (strcmp(argv[index], "--trace=boundaries") == 0) { runtime.full_trace = false; runtime.silent = false; }
    else if (strcmp(argv[index], "--trace=off") == 0) { runtime.full_trace = false; runtime.silent = true; }
    else if (strncmp(argv[index], "--telemetry-file=", 17) == 0) telemetry_path = argv[index] + 17;
    else if (strncmp(argv[index], "--outcome-file=", 15) == 0) outcome_path = argv[index] + 15;
    else if (strncmp(argv[index], "--memory-report-file=", 21) == 0) memory_report_path = argv[index] + 21;
    else if (strncmp(argv[index], "--execution-id=", 15) == 0) runtime.execution_id = argv[index] + 15;
    else if (strcmp(argv[index], "--cancel") == 0) runtime.cancel = true;
    else if (strcmp(argv[index], "--clock=real") == 0) runtime.virtual_clock = false;
    else if (strcmp(argv[index], "--clock=virtual") == 0) runtime.virtual_clock = true;
    else if (strncmp(argv[index], "--max-line-bytes=", 17) == 0) {
      errno = 0;
      char *end = NULL;
      unsigned long value = strtoul(argv[index] + 17, &end, 10);
      if (errno == ERANGE || end == argv[index] + 17 || *end != '\0' ||
          value == 0 || value > UINT32_MAX) {
        fputs("--max-line-bytes requires an integer from 1 through 4294967295\n", stderr);
        return 64;
      }
      runtime.max_line_bytes = (uint32_t)value;
    }
    else if (strncmp(argv[index], "--benchmark-repeats=", 20) == 0)
      benchmark_repeats = (uint32_t)strtoul(argv[index] + 20, NULL, 10);
    else { fprintf(stderr, "unknown Chimi runtime argument: %s\n", argv[index]); return 64; }
    }
    if (!found_boundary) {
      fputs("Chimi runner configuration must end with --\n", stderr);
      return 64;
    }
  }
  if (!runtime.silent && telemetry_path == NULL) {
    fputs("boundary or full tracing requires --telemetry-file\n", stderr);
    return 64;
  }
  if (telemetry_path && outcome_path && strcmp(telemetry_path, outcome_path) == 0) {
    fputs("telemetry and runner outcome require distinct files\n", stderr);
    return 64;
  }
  if ((memory_report_path && telemetry_path && strcmp(memory_report_path, telemetry_path) == 0) ||
      (memory_report_path && outcome_path && strcmp(memory_report_path, outcome_path) == 0)) {
    fputs("memory report, telemetry, and runner outcome require distinct files\n", stderr);
    return 64;
  }
  if (telemetry_path) {
    runtime.telemetry = fopen(telemetry_path, strcmp(telemetry_path, "/dev/null") == 0 ? "w" : "wx");
    if (!runtime.telemetry) {
      fprintf(stderr, "cannot open telemetry file %s: %s\n", telemetry_path, strerror(errno));
      return 74;
    }
  }
  if (outcome_path) {
    runtime.outcome = fopen(outcome_path, "wx");
    if (!runtime.outcome) {
      fprintf(stderr, "cannot open outcome file %s: %s\n", outcome_path, strerror(errno));
      if (runtime.telemetry) fclose(runtime.telemetry);
      return 74;
    }
  }
  uint32_t entry_node = 0;
  const ChimiFunction *entry_function = NULL;
  for (uint32_t i = 0; i < CHIMI_FUNCTION_COUNT; ++i)
    if (CHIMI_NODE_FUNCTIONS[CHIMI_FUNCTIONS[i].body] == entry) {
      entry_node = CHIMI_FUNCTIONS[i].body;
      entry_function = &CHIMI_FUNCTIONS[i];
      break;
    }
  if (entry_function == NULL) return 70;
  if (entry_function->parameters_length == 0 && index < argc) {
    fputs("program entry does not accept application arguments\n", stderr);
    return 64;
  }
  if (benchmark_repeats > 0) {
    struct timespec started, finished;
    clock_gettime(CLOCK_MONOTONIC, &started);
    for (uint32_t repeat = 0; repeat < benchmark_repeats; ++repeat) {
      runtime.next_sequence = 0;
      runtime.next_span = 2;
      if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
      ChimiEnvironment *environment = new_environment();
      if (entry_function->parameters_length == 1) {
        ChimiValue *arguments = application_arguments(argc - index, argv + index);
        environment->locals[environment_slot(CHIMI_IDS[entry_function->parameters_offset])] = arguments;
      }
      ChimiControl *control = runtime.cancel
        ? cancelled()
        : execute_program_entry(&runtime, entry, environment);
      if (control == NULL) return 70;
      if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
    }
    bool write_failed = write_memory_report(memory_report_path, &runtime);
    write_failed = close_output(runtime.telemetry) || write_failed;
    runtime.telemetry = NULL;
    write_failed = close_output(runtime.outcome) || write_failed;
    runtime.outcome = NULL;
    clock_gettime(CLOCK_MONOTONIC, &finished);
    int64_t elapsed = (int64_t)(finished.tv_sec - started.tv_sec) * INT64_C(1000000000)
                    + (int64_t)finished.tv_nsec - (int64_t)started.tv_nsec;
    fprintf(stderr, "%" PRIu64 "\n", elapsed / benchmark_repeats);
    return write_failed ? 74 : 0;
  }
  if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
  ChimiEnvironment *environment = new_environment();
  if (entry_function->parameters_length == 1) {
    ChimiValue *arguments = application_arguments(argc - index, argv + index);
    environment->locals[environment_slot(CHIMI_IDS[entry_function->parameters_offset])] = arguments;
  }
  ChimiControl *control = runtime.cancel
    ? cancelled()
    : execute_program_entry(&runtime, entry, environment);
  if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
  if (runtime.outcome) print_outcome(runtime.outcome, control);
  bool write_failed = write_memory_report(memory_report_path, &runtime);
  write_failed = close_output(runtime.telemetry) || write_failed;
  write_failed = close_output(runtime.outcome) || write_failed;
  if (write_failed) return 74;
  switch (control->kind) {
    case CONTROL_SUCCESS: return 0;
    case CONTROL_FAILURE: return 1;
    case CONTROL_DEFECT: return 70;
    case CONTROL_CANCEL: return 130;
  }
  return 70;
}
