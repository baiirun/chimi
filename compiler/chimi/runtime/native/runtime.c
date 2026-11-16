/* Chimi native runtime ABI version 22. */
/* glibc hides strdup and clock_gettime under -std=c11; macOS
   exposes them by default, and a POSIX feature macro there would hide others. */
#if defined(__linux__) && !defined(_GNU_SOURCE)
#define _GNU_SOURCE
#endif
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
#include <fcntl.h>
#include <pthread.h>
#include <signal.h>

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

__attribute__((weak)) size_t chimi_scheduler_live_memberships(void) { return 0; }
/* Without the fiber archive nothing is ever cancelled and no scope fires. */
__attribute__((weak)) uint32_t chimi_fiber_cancel_is_terminal(void) { return 1; }
__attribute__((weak)) uint32_t chimi_fiber_scope_cancel_pending(void) { return 0; }
__attribute__((weak)) uint32_t chimi_fiber_scope_polls(void) { return 0; }

typedef enum { VALUE_VOID, VALUE_INTEGER, VALUE_FLOAT, VALUE_TEXT, VALUE_BOOLEAN, VALUE_LIST, VALUE_RECORD,
               VALUE_CONSTRUCTED, VALUE_CALLABLE, VALUE_MAP, VALUE_JSON_NUMBER, VALUE_TASK,
               VALUE_CHANNEL_SENDER, VALUE_CHANNEL_RECEIVER,
               VALUE_CHANNEL_COMPLETION, VALUE_TUPLE, VALUE_RANGE,
               /* A `Duration`: nanoseconds in `as.integer`. */
               VALUE_DURATION,
               /* A `std/clock.Instant`: nanoseconds since the execution's origin, and a
                  `Timestamp`: nanoseconds since the Unix epoch; both in `as.integer`. */
               VALUE_INSTANT, VALUE_TIMESTAMP,
               /* A `Decimal`: plain data in `as.decimal`, copied as-is. */
               VALUE_DECIMAL,
               /* Process values (std/process, std/supervisor). Each is plain data that
                  copies as-is: identities the scheduler resolves, never pointers. */
               VALUE_PROCESS_ADDRESS, VALUE_PROCESS_INBOX, VALUE_SUPERVISOR_RUNNING,
               VALUE_MANAGED_PROCESS, VALUE_MANAGED_SUPERVISOR,
               /* A `Process.Monitor`: its id in `as.address.target`. */
               VALUE_PROCESS_MONITOR,
               /* A `std/set.Set` in `as.map`: each item is a key bound to Void. */
               VALUE_SET,
               /* A `std/bytes.Bytes` in `as.text`, which holds any bytes. */
               VALUE_BYTES,
               /* A `std/cause.Cause` in `as.record`: the CAUSE_* fields, unnamed. */
               VALUE_CAUSE } ValueKind;
/* CALL_CONSTRUCTOR is a constructor with fields named as a value; calling it constructs. */
typedef enum { CALL_DECLARED, CALL_CLOSURE, CALL_DEPENDENCY, CALL_HOST, CALL_INTRINSIC,
               CALL_CONSTRUCTOR } CallableKind;
/* The HOST_CLOCK_* functions are the execution clock provider's methods. A shifted
   timestamp is `Clock.starting_at`'s: its `inner` reads now, plus `shift`. */
typedef enum { HOST_PROCESS_UTF8_LINES, HOST_WRITE_LINE, HOST_WRITE_ERROR,
               HOST_CLOCK_NOW, HOST_CLOCK_TIMESTAMP, HOST_CLOCK_SLEEP_UNTIL,
               HOST_CLOCK_SHIFTED_TIMESTAMP,
               /* A Select timer on a user Clock: its `inner` is the provider's
                  sleep_until, called with the Instant in `shift`. */
               HOST_SELECT_SLEEP } HostFunction;
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
  /* A declared function's local count, which sizes each call's environment. */
  uint32_t local_count;
  ChimiEnvironment *environment;
  struct ChimiCallable *inner;
  uint32_t requirement;
  uint32_t interface_symbol;
  uint32_t method;
  const char *name;
  uint32_t intrinsic;
  HostFunction host;
  int64_t shift;
} ChimiCallable;

struct ChimiValue {
  ValueKind kind;
  union {
    int64_t integer;
    double floating; /* always finite */
    /* UTF-8 bytes with an explicit length, since Text may hold U+0000. The bytes are
       also NUL-terminated, so Text without U+0000 can reach a C API directly. */
    struct { const char *bytes; size_t length; } text;
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
    /* The magnitude's halves (an `unsigned __int128` member would raise the
       value's alignment), at most 38 digits; zero is never negative. */
    struct { uint64_t high; uint64_t low; uint32_t scale; bool negative; } decimal;
  } as;
};

/* `locals` has one slot per local of the body this environment runs: a
   function's, or a constant initializer's, with the closures written in it. */
struct ChimiEnvironment {
  ChimiValue **locals;
  ChimiValue **requirements;
  uint32_t local_count;
};

/* `node` is where a failure or defect started (-1 when unknown), or a break's
   target. `cause` is a failure's, defect's or cancellation's cause once built;
   NULL means the fresh cause of the outcome itself (see control_cause). */
struct ChimiControl {
  ControlKind kind;
  ChimiValue *value;
  const char *message;
  int64_t node;
  ChimiValue *cause;
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
  /* The task that started this one: a child's result hands its exclusive
     capabilities to it before the child ends. */
  uint64_t parent;
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
  /* `--clock-start`: the virtual source's Timestamp at Instant zero, in Unix ns. */
  int64_t clock_start;
  /* The parked tasks of a virtual-time deadlock, from the scheduler's report. */
  char *deadlock_waiters;
  /* The latest process incarnation that defected, from the scheduler's
     termination report under any trace mode, and how many have defected. A
     root that cannot finish names it, so a process defect is never only in
     the trace. */
  uint64_t last_defect_incarnation;
  uint64_t last_defect_membership;
  char *last_defect_message;
  uint64_t process_defect_count;
  /* The defect message of each task the scheduler defected for a channel
     capability violation, from its report. */
  struct CapabilityViolationNote *capability_violations;
  /* Set by --cancel, or asynchronously by a stop signal in a program
     without the scheduler. */
  volatile sig_atomic_t cancel;
  /* `--signal`: a stop signal the virtual clock delivers at
     `injected_signal_at` nanoseconds; 0 for none. */
  int32_t injected_signal;
  int64_t injected_signal_at;
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

   A pass boundary moves the young chunk allocations that the roots reach into
   survivor blocks (forward_survivors) and rewrites the roots, which are passed by
   address. So across allocation_scope_collect or allocation_scope_end, a C frame may
   keep a pointer to a value of that region's current pass only through those roots;
   everything older never moves. The runtime switches the current heap around every scheduler request
   (scheduler_request), because a fiber resumes only by returning from one. */
typedef struct AllocationHeader AllocationHeader;
typedef struct AllocationArea AllocationArea;
typedef struct AllocationScope AllocationScope;
typedef struct RuntimeHeap RuntimeHeap;

typedef enum { AREA_YOUNG, AREA_SURVIVOR, AREA_BASE, AREA_FRAGMENT, AREA_STATIC } AreaRole;

struct AllocationHeader {
  /* The area's list link; for a chunk allocation (see YoungChunk), which is in no
     list, the copy it was moved to at its pass boundary, or NULL. */
  AllocationHeader *next;
  AllocationArea *area;
  size_t bytes;
  uint64_t mark_epoch;
  AllocationKind kind;
  bool in_chunk;
};

/* A region's young allocations are bumped out of chunks it owns (CHI-219,
   docs/performance/native-bump-allocation-design.md). A chunk allocation is in no
   area list: a walk of the chunk finds it, by its header's size. */
enum { YOUNG_CHUNK_BYTES = 16 * 1024, YOUNG_CHUNK_CACHE = 16 };
typedef struct YoungChunk {
  struct YoungChunk *next;
  /* Where the allocations of a filled chunk end; the region's `bump` for the last. */
  unsigned char *used;
  _Alignas(16) unsigned char data[];
} YoungChunk;
enum { YOUNG_CHUNK_CAPACITY = YOUNG_CHUNK_BYTES - offsetof(YoungChunk, data),
       YOUNG_CHUNK_LARGEST = YOUNG_CHUNK_CAPACITY / 4 };

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
  /* The young area's chunks, oldest first, and the free space of the last one. */
  YoungChunk *first_chunk;
  YoungChunk *last_chunk;
  unsigned char *bump;
  unsigned char *bump_end;
  /* Payload bytes and count of the chunk allocations not yet added to the memory
     counters (see flush_young_counts). */
  size_t pending_bytes;
  size_t pending_count;
  /* Most of the last pass's young bytes survived it. Moving survivors out of the
     chunks would then cost more than bumping saves, so this pass links its
     allocations to the young list instead, as they would have to be anyway. */
  bool links_young;
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
  /* Under --profile-file, the id of the node this heap's owner is evaluating, plus 1,
     or 0 before its first node. Each heap keeps its own, so a task's allocations are
     charged to its own nodes. */
  uint32_t profiled_node_plus_one;
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

/* The operation profile (--profile-file): per Core node, how often it was evaluated
   and what it allocated itself, not counting the nodes it ran. NULL unless profiling,
   so an unprofiled run pays a branch at each node's entry and exit and one per
   allocation. */
typedef struct {
  const char *operation;
  uint64_t evaluations;
  uint64_t allocations;
  uint64_t allocated_bytes;
} ProfileRow;
static ProfileRow *profile_rows;

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

/* Footprint of the young chunk allocations whose counters are still deferred to
   their scope's next flush. Bumping one only adds to this, so the true footprint
   (footprint_bytes plus this) peaks just before it falls: footprint_sub and
   footprint_add record the peak there, and the report reads footprint_peak(). */
static size_t pending_footprint_bytes;

static void footprint_record_peak(void) {
  size_t footprint = heap_totals.footprint_bytes + pending_footprint_bytes;
  if (footprint > heap_totals.peak_footprint_bytes) heap_totals.peak_footprint_bytes = footprint;
}

static void footprint_add(size_t bytes) {
  heap_totals.footprint_bytes += bytes;
  footprint_record_peak();
}

static void footprint_sub(size_t bytes) {
  footprint_record_peak();
  heap_totals.footprint_bytes -= bytes;
}

static size_t footprint_peak(void) {
  footprint_record_peak();
  return heap_totals.peak_footprint_bytes;
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

static YoungChunk *young_chunk_cache;
static uint32_t young_chunk_cache_count;

static YoungChunk *young_chunk_take(void) {
  YoungChunk *chunk = young_chunk_cache;
  if (chunk) {
    young_chunk_cache = chunk->next;
    --young_chunk_cache_count;
  } else {
    chunk = calloc(1, YOUNG_CHUNK_BYTES);
    if (!chunk) exhausted_memory();
  }
  chunk->next = NULL;
  chunk->used = chunk->data;
  return chunk;
}

/* CHIMI_POISON_YOUNG_CHUNKS is a checking build: every pass boundary frees its
   chunks after filling them with a pattern, so a pointer kept across the boundary
   reads garbage, or fails as a use after free under AddressSanitizer. */
/* Requires: `chunk->used` marks the end of its allocations. Cached chunks are
   zero, so a bump needs no zeroing of its own. */
static void young_chunk_give(YoungChunk *chunk) {
#ifdef CHIMI_POISON_YOUNG_CHUNKS
  memset(chunk->data, 0xdb, YOUNG_CHUNK_CAPACITY);
  free(chunk);
  return;
#endif
  if (young_chunk_cache_count < YOUNG_CHUNK_CACHE) {
    memset(chunk->data, 0, (size_t)(chunk->used - chunk->data));
    chunk->next = young_chunk_cache;
    young_chunk_cache = chunk;
    ++young_chunk_cache_count;
    return;
  }
  free(chunk);
}

/* Adds the chunk allocations made since the last call to the memory counters. A
   pass only adds to its scope's and heap's counters until its boundary, so
   deferring the additions to the boundary (or to a nested region's start) leaves
   those peaks the same. The global footprint can fall mid-pass, so its peak
   counts the deferred bytes through pending_footprint_bytes. */
static void flush_young_counts(AllocationScope *scope) {
  size_t bytes = scope->pending_bytes, count = scope->pending_count;
  if (count == 0) return;
  scope->pending_bytes = 0;
  scope->pending_count = 0;
  scope->young.bytes += bytes;
  scope->young.count += count;
  heap_add_live(scope->heap, bytes);
  scope_add_live(scope, bytes);
  scope->total_allocated_bytes += bytes;
  scope->allocated_bytes_by_kind[ALLOCATION_BYTES] += bytes;
  heap_totals.live_allocations += count;
  pending_footprint_bytes -= count * sizeof(AllocationHeader) + bytes;
  footprint_add(count * sizeof(AllocationHeader) + bytes);
}

/* A zeroed block of `rounded` bytes (a multiple of SMALL_BLOCK_GRAIN, at most
   YOUNG_CHUNK_LARGEST) from `scope`'s chunks. */
static AllocationHeader *young_bump(AllocationScope *scope, size_t rounded) {
  if ((size_t)(scope->bump_end - scope->bump) < rounded) {
    YoungChunk *chunk = young_chunk_take();
    if (scope->last_chunk) {
      scope->last_chunk->used = scope->bump;
      scope->last_chunk->next = chunk;
    } else {
      scope->first_chunk = chunk;
    }
    scope->last_chunk = chunk;
    scope->bump = chunk->data;
    scope->bump_end = chunk->data + YOUNG_CHUNK_CAPACITY;
  }
  AllocationHeader *header = (AllocationHeader *)scope->bump;
  scope->bump += rounded;
  return header;
}

static size_t young_rounded(size_t bytes) {
  return (sizeof(AllocationHeader) + bytes + SMALL_BLOCK_GRAIN - 1) / SMALL_BLOCK_GRAIN * SMALL_BLOCK_GRAIN;
}

static void *allocate(size_t count, size_t size) {
  if (count == 0) count = 1;
  size_t bytes;
  if (__builtin_mul_overflow(count, size, &bytes) ||
      bytes > SIZE_MAX - sizeof(AllocationHeader) - SMALL_BLOCK_GRAIN) {
    fputs("Chimi runtime allocation overflow\n", stderr); exit(70);
  }
  AllocationScope *innermost = allocation_target ? NULL : current_heap->innermost;
  if (innermost && !innermost->links_young && bytes <= YOUNG_CHUNK_LARGEST) {
    size_t rounded = young_rounded(bytes);
    AllocationHeader *header = young_bump(innermost, rounded);
    header->bytes = bytes;
    header->area = &innermost->young;
    header->in_chunk = true;
    innermost->pending_bytes += bytes;
    ++innermost->pending_count;
    pending_footprint_bytes += sizeof(AllocationHeader) + bytes;
    if (profile_rows && current_heap->profiled_node_plus_one) {
      ProfileRow *row = &profile_rows[current_heap->profiled_node_plus_one - 1];
      ++row->allocations;
      row->allocated_bytes += bytes;
    }
    return header + 1;
  }
  AllocationArea *area = current_area();
  AllocationHeader *header = small_block_take(sizeof(*header) + bytes);
  header->bytes = bytes;
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
  if (profile_rows && current_heap->profiled_node_plus_one) {
    ProfileRow *row = &profile_rows[current_heap->profiled_node_plus_one - 1];
    ++row->allocations;
    row->allocated_bytes += bytes;
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
    footprint_sub(sizeof(*header) + header->bytes);
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
  footprint_sub(sizeof(RuntimeFragment));
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
  control->cause = NULL;
  return control;
}
static ChimiControl *success(ChimiValue *value) { return new_control(CONTROL_SUCCESS, value, NULL, -1); }
static ChimiControl *failure(ChimiValue *value) { return new_control(CONTROL_FAILURE, value, NULL, -1); }
static ChimiControl *defect(const char *message, int64_t node) { return new_control(CONTROL_DEFECT, NULL, message, node); }

/* A program without tasks runs main on the host thread, which has no overflow guard
   of its own: past its stack the program would die with SIGSEGV. Instead each call
   checks the stack against this address and defects while STACK_HEADROOM_BYTES
   remain, enough for the runtime frames between two calls and for reporting the
   defect. An unlimited stack (glibc then reports the end of the previous mapping)
   is held to STACK_UNLIMITED_BYTES, so the program defects before memory runs
   out. Finding the bottom reads /proc/self/maps on Linux, so it runs once at
   startup, not per benchmark repeat. Zero disables the check: on a fiber the guard page catches overflow, and
   on a host whose stack bounds are unknown nothing better is available. */
static uintptr_t host_stack_limit;
enum { STACK_HEADROOM_BYTES = 256 * 1024 };
#define STACK_UNLIMITED_BYTES ((uintptr_t)1 << 30)

/* The lowest address of the calling thread's stack, or 0 when it is unknown. */
static uintptr_t host_stack_bottom(void) {
#if defined(__APPLE__)
  pthread_t self = pthread_self();
  uintptr_t top = (uintptr_t)pthread_get_stackaddr_np(self);
  size_t size = pthread_get_stacksize_np(self);
  return size < top ? top - size : 0;
#elif defined(__linux__)
  pthread_attr_t attributes;
  if (pthread_getattr_np(pthread_self(), &attributes) != 0) return 0;
  void *bottom = NULL;
  size_t size = 0;
  int status = pthread_attr_getstack(&attributes, &bottom, &size);
  pthread_attr_destroy(&attributes);
  return status == 0 ? (uintptr_t)bottom : 0;
#else
  return 0;
#endif
}

static void guard_host_stack(void) {
  uintptr_t bottom = host_stack_bottom();
  uintptr_t here = (uintptr_t)__builtin_frame_address(0);
  if (bottom == 0 || here <= bottom || here - bottom <= 2 * (uintptr_t)STACK_HEADROOM_BYTES) return;
  host_stack_limit = here - bottom > STACK_UNLIMITED_BYTES ? here - STACK_UNLIMITED_BYTES
                                                         : bottom + STACK_HEADROOM_BYTES;
}

static bool host_stack_exhausted(void) {
  return (uintptr_t)__builtin_frame_address(0) < host_stack_limit;
}
static ChimiControl *cancelled(void) { return new_control(CONTROL_CANCEL, NULL, NULL, -1); }

/* Timestamps (time-and-outcomes.md, "Timestamp"): signed nanoseconds since
   1970-01-01T00:00:00Z ignoring leap seconds, written in RFC 3339 UTC. Civil dates
   use the proleptic Gregorian calendar (days_from_civil and civil_from_days). */
static int64_t days_from_civil(int64_t year, int64_t month, int64_t day) {
  year -= month <= 2;
  int64_t era = (year >= 0 ? year : year - 399) / 400;
  int64_t year_of_era = year - era * 400;
  int64_t day_of_year = (153 * (month + (month > 2 ? -3 : 9)) + 2) / 5 + day - 1;
  int64_t day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
  return era * 146097 + day_of_era - 719468;
}
static void civil_from_days(int64_t days, int64_t *year, int64_t *month, int64_t *day) {
  days += 719468;
  int64_t era = (days >= 0 ? days : days - 146096) / 146097;
  int64_t day_of_era = days - era * 146097;
  int64_t year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
  int64_t day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
  int64_t shifted = (5 * day_of_year + 2) / 153;
  *day = day_of_year - (153 * shifted + 2) / 5 + 1;
  *month = shifted < 10 ? shifted + 3 : shifted - 9;
  *year = year_of_era + era * 400 + (*month <= 2);
}
/* Writes `nanoseconds` as `2026-11-10T09:30:00.25Z`: the fraction loses its trailing
   zeros and is left out when it is zero. `out` holds at least 40 bytes. */
static void timestamp_text(int64_t nanoseconds, char *out) {
  int64_t seconds = nanoseconds / 1000000000, fraction = nanoseconds % 1000000000;
  if (fraction < 0) { fraction += 1000000000; seconds -= 1; }
  int64_t days = seconds / 86400, of_day = seconds % 86400;
  if (of_day < 0) { of_day += 86400; days -= 1; }
  int64_t year, month, day;
  civil_from_days(days, &year, &month, &day);
  int length = sprintf(out, "%04" PRId64 "-%02" PRId64 "-%02" PRId64 "T%02" PRId64 ":%02" PRId64 ":%02" PRId64,
                       year, month, day, of_day / 3600, of_day / 60 % 60, of_day % 60);
  if (fraction) {
    char digits[10]; sprintf(digits, "%09" PRId64, fraction);
    size_t used = 9; while (digits[used - 1] == '0') --used;
    digits[used] = '\0'; length += sprintf(out + length, ".%s", digits);
  }
  strcpy(out + length, "Z");
}
static bool parse_digits(const char **cursor, int count, int64_t *value) {
  *value = 0;
  for (int i = 0; i < count; ++i, ++*cursor) {
    if (**cursor < '0' || **cursor > '9') return false;
    *value = *value * 10 + (**cursor - '0');
  }
  return true;
}
/* Parses an RFC 3339 date-time (`T` and `Z` in either case, a fraction of up to nine
   digits, `Z` or a `+HH:MM`/`-HH:MM` offset) into Unix nanoseconds. Leap second 60 and
   values outside the Timestamp range are rejected. */
static bool parse_timestamp(const char *text, int64_t *out) {
  const char *cursor = text;
  int64_t year, month, day, hour, minute, second, fraction = 0, offset = 0;
  if (!parse_digits(&cursor, 4, &year) || *cursor++ != '-' || !parse_digits(&cursor, 2, &month) ||
      *cursor++ != '-' || !parse_digits(&cursor, 2, &day) || (*cursor != 'T' && *cursor != 't'))
    return false;
  ++cursor;
  if (!parse_digits(&cursor, 2, &hour) || *cursor++ != ':' || !parse_digits(&cursor, 2, &minute) ||
      *cursor++ != ':' || !parse_digits(&cursor, 2, &second))
    return false;
  if (*cursor == '.') {
    ++cursor; int digits = 0;
    while (*cursor >= '0' && *cursor <= '9') {
      if (++digits > 9) return false;
      fraction = fraction * 10 + (*cursor++ - '0');
    }
    if (digits == 0) return false;
    for (; digits < 9; ++digits) fraction *= 10;
  }
  if (*cursor == 'Z' || *cursor == 'z') ++cursor;
  else if (*cursor == '+' || *cursor == '-') {
    int64_t sign = *cursor++ == '-' ? -1 : 1, offset_hours, offset_minutes;
    if (!parse_digits(&cursor, 2, &offset_hours) || *cursor++ != ':' ||
        !parse_digits(&cursor, 2, &offset_minutes) || offset_hours > 23 || offset_minutes > 59)
      return false;
    offset = sign * (offset_hours * 3600 + offset_minutes * 60);
  } else return false;
  if (*cursor != '\0') return false;
  static const int month_days[] = {31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
  bool leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
  if (year < 1678 || year > 2262 || month < 1 || month > 12 || day < 1 || day > month_days[month - 1] ||
      (month == 2 && day == 29 && !leap) || hour > 23 || minute > 59 || second > 59)
    return false;
  int64_t seconds = days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second - offset;
  int64_t nanoseconds;
  if (__builtin_mul_overflow(seconds, INT64_C(1000000000), &nanoseconds) ||
      __builtin_add_overflow(nanoseconds, fraction, &nanoseconds))
    return false;
  *out = nanoseconds;
  return true;
}

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
static ChimiEnvironment *new_environment(uint32_t local_count) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_FRAME_ENVIRONMENT);
  environment->local_count = local_count;
  environment->locals = new_value_slots(local_count);
  environment->requirements = new_value_slots(CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
static ChimiEnvironment *clone_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_FRAME_ENVIRONMENT);
  environment->local_count = source->local_count;
  environment->locals = clone_value_slots(source->locals, source->local_count);
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
// Locals and requirements are independently immutable outside the boundaries
// that bind them. Copy only the table a boundary mutates; precise marking keeps
// the other table alive through the shared environment pointer.
static ChimiEnvironment *clone_local_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_FRAME_ENVIRONMENT);
  environment->local_count = source->local_count;
  environment->locals = clone_value_slots(source->locals, source->local_count);
  environment->requirements = source->requirements;
  return environment;
}
static ChimiEnvironment *clone_requirement_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_FRAME_ENVIRONMENT);
  environment->local_count = source->local_count;
  environment->locals = source->locals;
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
static ChimiEnvironment *capture_requirements(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->local_count = 0;
  environment->locals = NULL;
  environment->requirements = clone_value_slots(source->requirements, CHIMI_REQUIREMENT_SLOT_COUNT);
  return environment;
}
static ChimiEnvironment *capture_closure_environment(const ChimiEnvironment *source,
                                                     const ChimiNode *node) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  allocation_kind(environment, ALLOCATION_ENVIRONMENT);
  environment->local_count = node->d;
  environment->locals = new_value_slots(node->d);
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
/* Returns a new list with `item` appended; `list` is unchanged. A window that
   ends before its vector does writes slot `end` into a path copy instead, so
   the shared vector every other window sees is untouched. */
static ChimiValue *list_push(const ChimiValue *list, ChimiValue *item) {
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
        case VALUE_TEXT:
        case VALUE_BYTES: mark_push((void *)value->as.text.bytes, ALLOCATION_BYTES); break;
        case VALUE_JSON_NUMBER: mark_push((void *)value->as.json_number, ALLOCATION_BYTES); break;
        case VALUE_LIST:
          mark_push(value->as.list.vector.root, ALLOCATION_LIST_NODE);
          mark_push(value->as.list.vector.tail, ALLOCATION_LIST_NODE);
          break;
        case VALUE_RECORD:
        case VALUE_TUPLE:
        case VALUE_CAUSE: mark_push(value->as.record.fields, ALLOCATION_FIELDS); break;
        case VALUE_CONSTRUCTED: mark_push(value->as.constructed.fields, ALLOCATION_FIELDS); break;
        case VALUE_CALLABLE: mark_push(value->as.callable, ALLOCATION_CALLABLE); break;
        case VALUE_MAP:
        case VALUE_SET: mark_push(value->as.map, ALLOCATION_MAP_NODE); break;
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
        case VALUE_INSTANT:
        case VALUE_TIMESTAMP:
        case VALUE_DECIMAL:
        case VALUE_PROCESS_ADDRESS:
        case VALUE_PROCESS_INBOX:
        case VALUE_SUPERVISOR_RUNNING:
        case VALUE_MANAGED_PROCESS:
        case VALUE_MANAGED_SUPERVISOR:
        case VALUE_PROCESS_MONITOR: break;
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
    case ALLOCATION_CONTROL:
      mark_push(((ChimiControl *)pointer)->value, ALLOCATION_VALUE);
      mark_push(((ChimiControl *)pointer)->cause, ALLOCATION_VALUE);
      break;
    case ALLOCATION_BYTES: break;
    case ALLOCATION_FRAME_ENVIRONMENT:
    case ALLOCATION_KIND_COUNT: allocation_graph_inconsistent();
  }
}

/* Moving the survivors of a pass out of its chunks (see YoungChunk). After marking,
   the collector copies each marked chunk allocation it reaches into a survivor
   block and leaves the copy's address in the old header, then rewrites every
   pointer the copies and the marked list allocations hold. Only this region's
   chunk allocations move: everything else a pass reaches is older than the pass
   or in a list, and nothing older points into a chunk. */
static PointerStack copy_stack;

static void *forward_young(void *pointer) {
  if (!pointer) return NULL;
  AllocationHeader *header = allocation_header(pointer);
  if (!header->in_chunk || header->area->scope != collecting_scope) return pointer;
  if (header->next) return header->next + 1;
  if (header->mark_epoch != collecting_epoch) allocation_graph_inconsistent();
  size_t total = sizeof(*header) + header->bytes;
  AllocationHeader *copy = small_block_take(total);
  memcpy(copy, header, total);
  copy->in_chunk = false;
  area_link(&collecting_scope->survivors, copy);
  collecting_scope->promoted_bytes += copy->bytes;
  header->next = copy;
  pointer_stack_push(&copy_stack, copy + 1);
  return copy + 1;
}

#define FORWARD(slot) ((slot) = (void *)forward_young((void *)(slot)))

/* Rewrites the pointers `pointer` holds to the copies of moved allocations. It
   follows the same pointers as mark_children. */
static void forward_children(void *pointer) {
  AllocationHeader *header = allocation_header(pointer);
  switch (header->kind) {
    case ALLOCATION_VALUE: {
      ChimiValue *value = pointer;
      switch (value->kind) {
        case VALUE_TEXT:
        case VALUE_BYTES: FORWARD(value->as.text.bytes); break;
        case VALUE_JSON_NUMBER: FORWARD(value->as.json_number); break;
        case VALUE_LIST:
          FORWARD(value->as.list.vector.root);
          FORWARD(value->as.list.vector.tail);
          break;
        case VALUE_RECORD:
        case VALUE_TUPLE:
        case VALUE_CAUSE: FORWARD(value->as.record.fields); break;
        case VALUE_CONSTRUCTED: FORWARD(value->as.constructed.fields); break;
        case VALUE_CALLABLE: FORWARD(value->as.callable); break;
        case VALUE_MAP:
        case VALUE_SET: FORWARD(value->as.map); break;
        default: break;
      }
      break;
    }
    case ALLOCATION_LIST_NODE: {
      ListNode *node = pointer;
      for (uint32_t index = 0; index < node->count; ++index) FORWARD(node->slots[index]);
      break;
    }
    case ALLOCATION_FIELDS: {
      RuntimeField *fields = pointer;
      size_t length = header->bytes / sizeof(*fields);
      for (size_t index = 0; index < length; ++index) FORWARD(fields[index].value);
      break;
    }
    case ALLOCATION_VALUE_POINTERS: {
      ChimiValue **values = pointer;
      size_t length = header->bytes / sizeof(*values);
      for (size_t index = 0; index < length; ++index) FORWARD(values[index]);
      break;
    }
    case ALLOCATION_ENVIRONMENT: {
      ChimiEnvironment *environment = pointer;
      FORWARD(environment->locals);
      FORWARD(environment->requirements);
      break;
    }
    case ALLOCATION_CALLABLE: {
      ChimiCallable *callable = pointer;
      FORWARD(callable->environment);
      FORWARD(callable->inner);
      break;
    }
    case ALLOCATION_MAP_NODE: {
      MapNode *node = pointer;
      FORWARD(node->key);
      FORWARD(node->value);
      FORWARD(node->left);
      FORWARD(node->right);
      break;
    }
    case ALLOCATION_CONTROL:
      FORWARD(((ChimiControl *)pointer)->value);
      FORWARD(((ChimiControl *)pointer)->cause);
      break;
    case ALLOCATION_BYTES: break;
    case ALLOCATION_FRAME_ENVIRONMENT:
    case ALLOCATION_KIND_COUNT: allocation_graph_inconsistent();
  }
}
#undef FORWARD

static void forward_marked_list(AllocationArea *area) {
  for (AllocationHeader *header = area->allocations; header; header = header->next)
    if (header->mark_epoch == collecting_epoch) forward_children(header + 1);
}

/* Whether this pass of `scope` has bumped an allocation into its chunks. */
static bool young_chunks_used(const AllocationScope *scope) {
  return scope->first_chunk && (scope->first_chunk->next || scope->bump != scope->first_chunk->data);
}

/* Moves what the roots reach out of `scope`'s chunks and updates the roots. */
static void forward_survivors(AllocationScope *scope, ChimiValue **value, ChimiControl **control) {
  if (!young_chunks_used(scope)) return;
  if (value) *value = forward_young(*value);
  if (control) *control = forward_young(*control);
  forward_marked_list(&scope->young);
  for (AllocationArea *area = scope->adopted; area; area = area->next) forward_marked_list(area);
  while (copy_stack.length) forward_children(copy_stack.items[--copy_stack.length]);
}

/* Releases the chunk allocations of this pass that were not moved, and empties the
   chunks for the next pass, keeping the first one. Requires flush_young_counts. */
static void reset_young_chunks(AllocationScope *scope, bool keep_first) {
  YoungChunk *first = scope->first_chunk;
  if (!first || (keep_first && !young_chunks_used(scope))) return;
  scope->last_chunk->used = scope->bump;
  size_t dead_bytes = 0, dead_count = 0;
  for (YoungChunk *chunk = first; chunk; chunk = chunk->next) {
    for (unsigned char *cursor = chunk->data; cursor < chunk->used;) {
      AllocationHeader *header = (AllocationHeader *)cursor;
      cursor += young_rounded(header->bytes);
      if (header->next) continue;
      dead_bytes += header->bytes;
      ++dead_count;
      if (channel_endpoint_count && header->kind == ALLOCATION_VALUE) {
        ChimiValue *value = (ChimiValue *)(header + 1);
        if (is_channel_endpoint(value)) channel_endpoint_drop(value->as.capability.channel);
      }
    }
  }
  scope->live_bytes -= dead_bytes;
  scope->heap->live_bytes -= dead_bytes;
  heap_totals.live_allocations -= dead_count;
  footprint_sub(dead_count * sizeof(AllocationHeader) + dead_bytes);
  YoungChunk *rest = first->next;
#ifdef CHIMI_POISON_YOUNG_CHUNKS
  keep_first = false;
#endif
  if (keep_first) {
    memset(first->data, 0, (size_t)(first->used - first->data));
    first->next = NULL;
    first->used = first->data;
    scope->last_chunk = first;
    scope->bump = first->data;
    scope->bump_end = first->data + YOUNG_CHUNK_CAPACITY;
  } else {
    young_chunk_give(first);
    scope->first_chunk = scope->last_chunk = NULL;
    scope->bump = scope->bump_end = NULL;
  }
  while (rest) {
    YoungChunk *next = rest->next;
    young_chunk_give(rest);
    rest = next;
  }
}

static void allocation_scope_begin(AllocationScope *scope) {
  memset(scope, 0, sizeof(*scope));
  RuntimeHeap *heap = current_heap;
  /* The enclosing region's peak must count what its pass has allocated so far. */
  if (heap->innermost) flush_young_counts(heap->innermost);
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
static HeapStatus allocation_scope_sweep(AllocationScope *scope, ChimiValue **value,
                                         ChimiControl **control, bool full, bool ending) {
  collecting_epoch = next_mark_epoch++;
  if (next_mark_epoch == 0) {
    fputs("Chimi runtime allocation mark epoch exhausted\n", stderr); exit(70);
  }
  flush_young_counts(scope);
  collecting_scope = scope;
  collecting_full = full;
  collecting_foreign = false;
  mark_push(value ? *value : NULL, ALLOCATION_VALUE);
  mark_push(control ? *control : NULL, ALLOCATION_CONTROL);
  while (mark_stack.length) mark_children(mark_stack.items[--mark_stack.length]);
  bool foreign = collecting_foreign;
  size_t young_bytes = scope->young.bytes, promoted_before = scope->promoted_bytes;
  forward_survivors(scope, value, control);
  reset_young_chunks(scope, !ending);
  promote_marked(scope, &scope->young);
  scope->links_young = 4 * (scope->promoted_bytes - promoted_before) > young_bytes;
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
static HeapStatus allocation_scope_collect(AllocationScope *scope, ChimiValue **value,
                                           ChimiControl **control) {
  size_t survivors = scope->survivors.bytes;
  bool full = scope->has_baseline && survivors >= 2 * scope->full_baseline &&
              survivors - scope->full_baseline >= REGION_FULL_MIN_GROWTH;
  HeapStatus status = allocation_scope_sweep(scope, value, control, full, false);
  if (full || (!scope->has_baseline && scope->survivors.bytes != 0)) {
    scope->has_baseline = true;
    scope->full_baseline = scope->survivors.bytes;
  }
  return status;
}

/* Region exit: a full pass keeps what `value` and `control` reach, and the survivors
   join the enclosing region's young generation, or the heap's base area. */
static HeapStatus allocation_scope_end(AllocationScope *scope, ChimiValue **value,
                                       ChimiControl **control) {
  HeapStatus status = allocation_scope_sweep(scope, value, control, true, true);
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
        case VALUE_BYTES:
          value->as.text.bytes = copy_forward(copy, (void *)value->as.text.bytes, ALLOCATION_BYTES); break;
        case VALUE_JSON_NUMBER:
          value->as.json_number = copy_forward(copy, (void *)value->as.json_number, ALLOCATION_BYTES);
          break;
        case VALUE_LIST: copy_list_window(copy, value); break;
        case VALUE_RECORD:
        case VALUE_CAUSE:
        case VALUE_TUPLE:
          value->as.record.fields = copy_forward(copy, value->as.record.fields, ALLOCATION_FIELDS);
          break;
        case VALUE_CONSTRUCTED:
          value->as.constructed.fields =
            copy_forward(copy, value->as.constructed.fields, ALLOCATION_FIELDS);
          break;
        case VALUE_CALLABLE:
          value->as.callable = copy_forward(copy, value->as.callable, ALLOCATION_CALLABLE); break;
        case VALUE_MAP:
        case VALUE_SET: value->as.map = copy_forward(copy, value->as.map, ALLOCATION_MAP_NODE); break;
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
        case VALUE_INSTANT:
        case VALUE_TIMESTAMP:
        case VALUE_DECIMAL:
        case VALUE_PROCESS_ADDRESS:
        case VALUE_PROCESS_INBOX:
        case VALUE_SUPERVISOR_RUNNING:
        case VALUE_MANAGED_PROCESS:
        case VALUE_MANAGED_SUPERVISOR:
        case VALUE_PROCESS_MONITOR: break;
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
      control->cause = copy_forward(copy, control->cause, ALLOCATION_VALUE);
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

/* A second detached copy of an unadopted fragment, for another broadcast or watch
   subscription. O(fragment). */
static RuntimeFragment *fragment_duplicate(const RuntimeFragment *source) {
  RuntimeFragment *fragment = small_block_take(sizeof(*fragment));
  fragment->area.role = AREA_FRAGMENT;
  fragment->root_count = source->root_count;
  ++heap_totals.live_fragments;
  footprint_add(sizeof(*fragment));
  fragment_footprint_add(sizeof(*fragment));
  AllocationKind kinds[FRAGMENT_ROOTS];
  for (uint32_t index = 0; index < source->root_count; ++index)
    kinds[index] = allocation_header(source->roots[index])->kind;
  FragmentCopy copy = {.source_area = &source->area, .target = &fragment->area};
  if (copy_graph(&copy, source->roots, kinds, source->root_count, fragment->roots) != HEAP_OK)
    allocation_graph_inconsistent();
  return fragment;
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

static void json_bytes(FILE *output, const char *value, size_t length) {
  fputc('"', output);
  const unsigned char *end = (const unsigned char *)value + length;
  for (const unsigned char *cursor = (const unsigned char *)value; cursor < end; ++cursor) {
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
static void json_string(FILE *output, const char *value) { json_bytes(output, value, strlen(value)); }

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

/* Charges the current heap's allocations to `node` until profile_leave. */
static uint32_t profile_enter(uint32_t node, const char *operation) {
  if (!profile_rows) return 0;
  ProfileRow *row = &profile_rows[node];
  row->operation = operation;
  ++row->evaluations;
  uint32_t outer = current_heap->profiled_node_plus_one;
  current_heap->profiled_node_plus_one = node + 1;
  return outer;
}
static void profile_leave(uint32_t outer) {
  if (profile_rows) current_heap->profiled_node_plus_one = outer;
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
/* Void, True and False are each one shared value in the static area, because values
   never change after construction and conditions and effects produce them on almost
   every pass. Static values are never collected, copied or freed. */
static ChimiValue *shared_values[3];
static ChimiValue *shared_value(uint32_t index) {
  if (!shared_values[index]) {
    AllocationArea *previous = allocation_target;
    allocation_target = &static_area;
    ChimiValue *value = new_value(index == 0 ? VALUE_VOID : VALUE_BOOLEAN);
    if (index == 2) value->as.boolean = true;
    allocation_target = previous;
    shared_values[index] = value;
  }
  return shared_values[index];
}
static ChimiValue *void_value(void) { return shared_value(0); }
static char *copy_text(const char *raw) {
  size_t length = strlen(raw);
  char *copy = allocate(length + 1, 1);
  memcpy(copy, raw, length);
  return copy;
}
static ChimiValue *text_from_bytes(const char *bytes, size_t length) {
  char *raw = allocate(length + 1, 1);
  memcpy(raw, bytes, length); raw[length] = '\0';
  ChimiValue *v = new_value(VALUE_TEXT); v->as.text.bytes = raw; v->as.text.length = length; return v;
}
static char hex_digit(unsigned value) { return "0123456789abcdef"[value & 0xf]; }
/* Text from a C string, which cannot hold U+0000: a path, a message, a number. */
static ChimiValue *text_value(const char *raw) { return text_from_bytes(raw, strlen(raw)); }
/* A Text literal or constant from the string table, which may hold U+0000. */
static ChimiValue *string_text(uint32_t id) { return text_from_bytes(CHIMI_STRINGS[id], CHIMI_STRING_LENGTHS[id]); }
static bool text_equals(const ChimiValue *text, const char *bytes, size_t length) {
  return text->as.text.length == length && memcmp(text->as.text.bytes, bytes, length) == 0;
}
static bool text_holds_nul(const ChimiValue *text) {
  return memchr(text->as.text.bytes, '\0', text->as.text.length) != NULL;
}
/* The first `needle` in `text`, or NULL. `needle` is not empty. */
static const char *text_find(const char *text, size_t length, const char *needle, size_t width) {
  return memmem(text, length, needle, width);
}
static ChimiValue *boolean_value(bool raw) { return shared_value(raw ? 2 : 1); }
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

/* A defect message copied outside every reclaimable heap, like
   numeric_message's: a control's message is not traced, and a loop region or a
   child task's heap may be freed while the defect it ended with is still read. */
static const char *lasting_message(const char *raw) {
  size_t length = strlen(raw);
  char *message = malloc(length + 1);
  if (!message) { fputs("Chimi runtime allocation failed\n", stderr); exit(70); }
  memcpy(message, raw, length + 1);
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

/* std/decimal. It mirrors compiler/chimi/src/decimal.rs value for value and message
   for message: a sign, a magnitude of at most 38 digits, and a scale from 0 to 38.
   No operation rounds unless it takes a rounding mode, and none wraps or saturates;
   a result outside the range makes the function return false. */
typedef unsigned __int128 DecimalMagnitude;
typedef struct { bool negative; DecimalMagnitude magnitude; uint32_t scale; } Decimal;
typedef enum { DECIMAL_REMAINDER_ZERO, DECIMAL_BELOW_HALF, DECIMAL_HALF, DECIMAL_ABOVE_HALF } DecimalRemainder;
/* The codes `std/decimal.rounding_code` passes, in `Rounding` declaration order. */
typedef enum { ROUND_TOWARD_ZERO, ROUND_AWAY_FROM_ZERO, ROUND_FLOOR, ROUND_CEILING,
               ROUND_HALF_TOWARD_ZERO, ROUND_HALF_AWAY_FROM_ZERO, ROUND_HALF_EVEN } DecimalRounding;
#define DECIMAL_MAX_SCALE 38u
/* Large enough for a sign, 39 digits, and a point. */
#define DECIMAL_TEXT_BYTES 48

/* 10^exponent, or false when it exceeds the magnitude type (from 10^39). */
static bool decimal_power(uint32_t exponent, DecimalMagnitude *output) {
  if (exponent > DECIMAL_MAX_SCALE) return false;
  DecimalMagnitude power = 1;
  for (uint32_t i = 0; i < exponent; ++i) power *= 10;
  *output = power;
  return true;
}

static DecimalMagnitude decimal_max_magnitude(void) {
  DecimalMagnitude power = 0; decimal_power(DECIMAL_MAX_SCALE, &power); return power - 1;
}

static bool decimal_make(bool negative, DecimalMagnitude magnitude, uint32_t scale, Decimal *output) {
  if (magnitude > decimal_max_magnitude() || scale > DECIMAL_MAX_SCALE) return false;
  *output = (Decimal){negative && magnitude != 0, magnitude, scale};
  return true;
}

static Decimal decimal_of(const ChimiValue *value) {
  return (Decimal){value->as.decimal.negative,
                   ((DecimalMagnitude)value->as.decimal.high << 64) | value->as.decimal.low,
                   value->as.decimal.scale};
}

static ChimiValue *decimal_value(Decimal decimal) {
  ChimiValue *v = new_value(VALUE_DECIMAL);
  v->as.decimal.high = (uint64_t)(decimal.magnitude >> 64);
  v->as.decimal.low = (uint64_t)decimal.magnitude;
  v->as.decimal.scale = decimal.scale;
  v->as.decimal.negative = decimal.negative;
  return v;
}

static Decimal decimal_negate(Decimal value) {
  value.negative = !value.negative && value.magnitude != 0;
  return value;
}

static bool decimal_add(Decimal left, Decimal right, Decimal *output) {
  uint32_t scale = left.scale > right.scale ? left.scale : right.scale;
  DecimalMagnitude left_power = 0, right_power = 0, l = 0, r = 0;
  decimal_power(scale - left.scale, &left_power); decimal_power(scale - right.scale, &right_power);
  /* Either aligned magnitude may exceed the range while the sum does not; past the
     magnitude type it cannot, because the other is at most 38 digits. */
  if (__builtin_mul_overflow(left.magnitude, left_power, &l) ||
      __builtin_mul_overflow(right.magnitude, right_power, &r)) return false;
  if (left.negative == right.negative) {
    DecimalMagnitude sum = 0;
    if (__builtin_add_overflow(l, r, &sum)) return false;
    return decimal_make(left.negative, sum, scale, output);
  }
  if (l >= r) return decimal_make(left.negative, l - r, scale, output);
  return decimal_make(right.negative, r - l, scale, output);
}

static bool decimal_multiply(Decimal left, Decimal right, Decimal *output) {
  DecimalMagnitude product = 0;
  if (__builtin_mul_overflow(left.magnitude, right.magnitude, &product)) return false;
  return decimal_make(left.negative != right.negative, product, left.scale + right.scale, output);
}

/* Requires: remainder < denominator. */
static DecimalRemainder decimal_remainder(DecimalMagnitude remainder, DecimalMagnitude denominator) {
  if (remainder == 0) return DECIMAL_REMAINDER_ZERO;
  DecimalMagnitude rest = denominator - remainder;
  return remainder < rest ? DECIMAL_BELOW_HALF : remainder == rest ? DECIMAL_HALF : DECIMAL_ABOVE_HALF;
}

/* numerator x 10^shift / denominator, or false once the quotient leaves the
   magnitude type. Requires: 0 < denominator < 10^38, so the remainder doubled fits. */
static bool decimal_long_division(DecimalMagnitude numerator, DecimalMagnitude denominator, uint32_t shift,
                                  DecimalMagnitude *quotient, DecimalRemainder *remainder) {
  DecimalMagnitude q = numerator / denominator, r = numerator % denominator;
  for (uint32_t step = 0; step < shift; ++step) {
    /* r x 10 = digit x denominator + next, without leaving the magnitude type. */
    DecimalMagnitude digit = 0, next = 0;
    for (int i = 0; i < 10; ++i) {
      next += r;
      if (next >= denominator) { next -= denominator; ++digit; }
    }
    if (__builtin_mul_overflow(q, (DecimalMagnitude)10, &q) || __builtin_add_overflow(q, digit, &q)) return false;
    r = next;
  }
  if (q > decimal_max_magnitude()) return false;
  *quotient = q; *remainder = decimal_remainder(r, denominator);
  return true;
}

/* dividend / divisor with `scale` fraction digits, or false when the divisor is
   zero or the rounded quotient is out of range. Requires: scale <= 38. */
static bool decimal_divide(Decimal dividend, Decimal divisor, uint32_t scale, DecimalRounding rounding,
                           Decimal *output) {
  if (divisor.magnitude == 0) return false;
  int64_t shift = (int64_t)scale + divisor.scale - dividend.scale;
  DecimalMagnitude quotient = 0; DecimalRemainder remainder = DECIMAL_REMAINDER_ZERO;
  if (shift >= 0) {
    if (!decimal_long_division(dividend.magnitude, divisor.magnitude, (uint32_t)shift, &quotient, &remainder))
      return false;
  } else {
    DecimalMagnitude power = 0, denominator = 0;
    if (decimal_power((uint32_t)-shift, &power) && !__builtin_mul_overflow(divisor.magnitude, power, &denominator)) {
      quotient = dividend.magnitude / denominator;
      remainder = decimal_remainder(dividend.magnitude % denominator, denominator);
    } else {
      /* The divisor exceeds the magnitude type, so it is more than twice the dividend. */
      remainder = dividend.magnitude == 0 ? DECIMAL_REMAINDER_ZERO : DECIMAL_BELOW_HALF;
    }
  }
  bool negative = dividend.negative != divisor.negative, inexact = remainder != DECIMAL_REMAINDER_ZERO, away = false;
  switch (rounding) {
    case ROUND_TOWARD_ZERO: away = false; break;
    case ROUND_AWAY_FROM_ZERO: away = inexact; break;
    case ROUND_FLOOR: away = negative && inexact; break;
    case ROUND_CEILING: away = !negative && inexact; break;
    case ROUND_HALF_TOWARD_ZERO: away = remainder == DECIMAL_ABOVE_HALF; break;
    case ROUND_HALF_AWAY_FROM_ZERO: away = remainder >= DECIMAL_HALF; break;
    case ROUND_HALF_EVEN:
      away = remainder == DECIMAL_ABOVE_HALF || (remainder == DECIMAL_HALF && quotient % 2 == 1); break;
  }
  return decimal_make(negative, quotient + (away ? 1 : 0), scale, output);
}

/* Orders magnitudes, aligning the one with the smaller scale; past the magnitude
   type the aligned side exceeds any 38-digit magnitude. */
static int decimal_magnitude_compare(Decimal left, Decimal right) {
  DecimalMagnitude power = 0, aligned = 0;
  if (left.scale >= right.scale) {
    decimal_power(left.scale - right.scale, &power);
    if (__builtin_mul_overflow(right.magnitude, power, &aligned)) return -1;
    return (left.magnitude > aligned) - (left.magnitude < aligned);
  }
  decimal_power(right.scale - left.scale, &power);
  if (__builtin_mul_overflow(left.magnitude, power, &aligned)) return 1;
  return (aligned > right.magnitude) - (aligned < right.magnitude);
}

/* Numeric order, so 1.50 and 1.5 are equal. Returns -1, 0, or 1. */
static int decimal_compare(Decimal left, Decimal right) {
  int left_sign = left.magnitude == 0 ? 0 : left.negative ? -1 : 1;
  int right_sign = right.magnitude == 0 ? 0 : right.negative ? -1 : 1;
  if (left_sign != right_sign) return (left_sign > right_sign) - (left_sign < right_sign);
  return left.negative ? decimal_magnitude_compare(right, left) : decimal_magnitude_compare(left, right);
}

/* `Decimal.to_text`: exactly `scale` fraction digits. `output` holds DECIMAL_TEXT_BYTES. */
static void decimal_text(Decimal value, char *output) {
  char digits[DECIMAL_TEXT_BYTES]; size_t count = 0;
  DecimalMagnitude magnitude = value.magnitude;
  do { digits[count++] = (char)('0' + (int)(magnitude % 10)); magnitude /= 10; } while (magnitude != 0);
  while (count < (size_t)value.scale + 1) digits[count++] = '0';
  size_t length = 0;
  if (value.negative) output[length++] = '-';
  for (size_t index = count; index-- > 0;) {
    output[length++] = digits[index];
    if (index == value.scale && index != 0) output[length++] = '.';
  }
  output[length] = '\0';
}

/* The debug form: trailing fraction zeros removed, so equal values render the same. */
static void decimal_normalized_text(Decimal value, char *output) {
  while (value.scale > 0 && value.magnitude % 10 == 0) { value.magnitude /= 10; --value.scale; }
  decimal_text(value, output);
}

/* Reads -?digits(.digits)?; leading zeros are allowed. False for any other text,
   and for more than 38 significant or fraction digits. */
static bool decimal_parse(const char *text, Decimal *output) {
  bool negative = *text == '-';
  const char *cursor = negative ? text + 1 : text;
  DecimalMagnitude magnitude = 0, largest = decimal_max_magnitude();
  size_t whole = 0, fraction = 0; bool point = false;
  for (; *cursor; ++cursor) {
    if (*cursor == '.' && !point) { point = true; continue; }
    if (*cursor < '0' || *cursor > '9') return false;
    /* Checked before multiplying: 38 nines times 10 leaves the magnitude type. */
    DecimalMagnitude digit = (DecimalMagnitude)(*cursor - '0');
    if (magnitude > (largest - digit) / 10) return false;
    magnitude = magnitude * 10 + digit;
    if (point) ++fraction; else ++whole;
  }
  if (whole == 0 || (point && fraction == 0) || fraction > DECIMAL_MAX_SCALE) return false;
  return decimal_make(negative, magnitude, (uint32_t)fraction, output);
}

/* Applies `+`, `-`, or `*`; typing rejects `/` and `%` on Decimal (T4306). */
static ChimiControl *decimal_arithmetic(uint32_t operator, Decimal l, Decimal r, int64_t node) {
  const char *symbol = binary_symbol(operator); Decimal output; bool fits = false;
  switch (operator) {
    case CHIMI_BINARY_ADD: fits = decimal_add(l, r, &output); break;
    case CHIMI_BINARY_SUBTRACT: fits = decimal_add(l, decimal_negate(r), &output); break;
    case CHIMI_BINARY_MULTIPLY: fits = decimal_multiply(l, r, &output); break;
    default: return defect(numeric_message("`%s` is not a `Decimal` operator", symbol), node);
  }
  if (fits) return success(decimal_value(output));
  char left_text[DECIMAL_TEXT_BYTES], right_text[DECIMAL_TEXT_BYTES];
  decimal_text(l, left_text); decimal_text(r, right_text);
  return defect(numeric_message("decimal overflow in `%s`: %s %s %s needs more than 38 digits or scale 38",
                                symbol, left_text, symbol, right_text), node);
}
static ChimiValue *callable_value(ChimiCallable *raw) { ChimiValue *v = new_value(VALUE_CALLABLE); v->as.callable = raw; return v; }
static ChimiValue *map_value(MapNode *raw) { ChimiValue *v = new_value(VALUE_MAP); v->as.map = raw; return v; }
static ChimiValue *set_value(MapNode *raw) { ChimiValue *v = new_value(VALUE_SET); v->as.map = raw; return v; }
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

/* The `{ completion, publisher }` record of a broadcast or watch: its publisher is a
   Sender of the hub's id. */
static ChimiValue *publisher_bundle_value(uint64_t channel, uint64_t owner, uint64_t token) {
  ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = 2;
  value->as.record.fields = allocate(2, sizeof(*value->as.record.fields));
  allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
  value->as.record.fields[0] = (RuntimeField){
    "completion", channel_capability_value(VALUE_CHANNEL_COMPLETION, channel, owner, token)
  };
  value->as.record.fields[1] = (RuntimeField){
    "publisher", channel_capability_value(VALUE_CHANNEL_SENDER, channel, 0, 0)
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
  /* Collecting for a send instead of transferring: each capability found is
     recorded here with the sender's token, and its copy in the message takes
     the in-flight token (ChimiCarriedCapability). */
  bool collect;
  ChimiCarriedCapability *carried;
  size_t carried_count;
  size_t carried_capacity;
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

static void collect_carried_capability(CapabilityTransfer *transfer, ChimiValue *value,
                                       uint32_t capability) {
  if (transfer->carried_count == transfer->carried_capacity) {
    size_t capacity = transfer->carried_capacity ? transfer->carried_capacity * 2 : 8;
    ChimiCarriedCapability *carried = realloc(transfer->carried, capacity * sizeof(*carried));
    if (!carried) exhausted_memory();
    transfer->carried = carried;
    transfer->carried_capacity = capacity;
  }
  transfer->carried[transfer->carried_count++] = (ChimiCarriedCapability){
    .channel = value->as.capability.channel,
    .token = value->as.capability.token,
    .capability = capability,
  };
  ++value->as.capability.token;
}

static bool transfer_captured_capability(CapabilityTransfer *transfer, ChimiValue *value,
                                         uint32_t request_kind) {
  if (transfer->collect) {
    collect_carried_capability(transfer, value,
                               request_kind == CHIMI_REQUEST_CHANNEL_TRANSFER_RECEIVER
                                 ? CHIMI_CAPABILITY_RECEIVER : CHIMI_CAPABILITY_COMPLETION);
    return true;
  }
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
    case VALUE_CAUSE:
    case VALUE_TUPLE:
      for (uint32_t index = value->as.record.length; index-- > 0;)
        transfer_push(transfer, TRANSFER_VALUE, value->as.record.fields[index].value);
      return true;
    case VALUE_CONSTRUCTED:
      for (uint32_t index = value->as.constructed.length; index-- > 0;)
        transfer_push(transfer, TRANSFER_VALUE, value->as.constructed.fields[index].value);
      return true;
    case VALUE_CALLABLE: transfer_push(transfer, TRANSFER_CALLABLE, value->as.callable); return true;
    case VALUE_MAP:
    case VALUE_SET: transfer_push(transfer, TRANSFER_MAP_NODE, value->as.map); return true;
    case VALUE_VOID:
    case VALUE_INTEGER:
    case VALUE_FLOAT:
    case VALUE_TEXT:
    case VALUE_BYTES:
    case VALUE_BOOLEAN:
    case VALUE_RANGE:
    case VALUE_DURATION:
    case VALUE_INSTANT:
    case VALUE_TIMESTAMP:
    case VALUE_DECIMAL:
    case VALUE_JSON_NUMBER:
    case VALUE_TASK:
    case VALUE_CHANNEL_SENDER:
    case VALUE_PROCESS_ADDRESS:
    case VALUE_PROCESS_INBOX:
    case VALUE_SUPERVISOR_RUNNING:
    case VALUE_MANAGED_PROCESS:
    case VALUE_MANAGED_SUPERVISOR:
    case VALUE_PROCESS_MONITOR: return true;
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
        for (uint32_t slot = environment->local_count; slot-- > 0;)
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

/* Whether a direct integer root must run node by node: full tracing and profiling
   both observe every node. */
int32_t chimi_rt_full_trace(const ChimiRuntime *runtime) { return runtime->full_trace || profile_rows; }
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
  /* An equal key already held stays; only the value changes. */
  if (order == 0) return map_node(node->key, value, node->left, node->right);
  if (order < 0) return map_balance(map_node(node->key, node->value,
                                            map_put_node(node->left, key, value), node->right));
  return map_balance(map_node(node->key, node->value, node->left,
                              map_put_node(node->right, key, value)));
}
/* The least node of `node` in `*least`, and the tree without it. */
static MapNode *map_remove_least(MapNode *node, MapNode **least) {
  if (!node->left) { *least = node; return node->right; }
  MapNode *left = map_remove_least(node->left, least);
  return map_balance(map_node(node->key, node->value, left, node->right));
}
/* `node` without `key`, rebalanced; `node` itself when it has no such key. The
   recursion is bounded by the tree height. */
static MapNode *map_remove_node(MapNode *node, const ChimiValue *key) {
  if (!node) return NULL;
  int order = chimi_value_compare(key, node->key);
  if (order < 0) {
    MapNode *left = map_remove_node(node->left, key);
    return left == node->left ? node : map_balance(map_node(node->key, node->value, left, node->right));
  }
  if (order > 0) {
    MapNode *right = map_remove_node(node->right, key);
    return right == node->right ? node : map_balance(map_node(node->key, node->value, node->left, right));
  }
  if (!node->left) return node->right;
  if (!node->right) return node->left;
  MapNode *least = NULL;
  MapNode *right = map_remove_least(node->right, &least);
  return map_balance(map_node(least->key, least->value, node->left, right));
}
static int64_t map_size(const MapNode *node) {
  return node ? 1 + map_size(node->left) + map_size(node->right) : 0;
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
   stack, so nesting depth never costs host stack: the first COMPARE_INLINE_STEPS
   steps live in the caller's frame, which covers comparing scalars and small
   values without a heap allocation, and deeper walks move to the heap. */
enum { COMPARE_INLINE_STEPS = 32 };
typedef struct { const ChimiValue *left; const ChimiValue *right; int decided; } CompareStep;
typedef struct {
  CompareStep *steps; size_t length; size_t capacity;
  CompareStep inline_steps[COMPARE_INLINE_STEPS];
} CompareStack;

static void compare_push(CompareStack *stack, const ChimiValue *left, const ChimiValue *right, int decided) {
  if (stack->length == stack->capacity) {
    size_t capacity = stack->capacity * 2;
    bool on_heap = stack->steps != stack->inline_steps;
    CompareStep *steps = realloc(on_heap ? stack->steps : NULL, capacity * sizeof(*steps));
    if (steps == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
    if (!on_heap) memcpy(steps, stack->inline_steps, stack->length * sizeof(*steps));
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
  JSON_STEP_VALUE, JSON_STEP_TEXT, JSON_STEP_NAME, JSON_STEP_KEY, JSON_STEP_FREE
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

/* A map value in an outcome record, as the interpreter writes it: a Text-keyed or
   empty map is an object of its entries in key order, and any other map is
   `{"map":[[key,value],...]}` in key order, so each key is written once. */
static void json_push_map(FILE *output, JsonSteps *steps, const MapNode *map) {
  MapEntries entries = map_entries(map);
  bool text_keyed = !entries.length || entries.nodes[0]->key->kind == VALUE_TEXT;
  fputs(text_keyed ? "{" : "{\"map\":[", output);
  json_step(steps, JSON_STEP_FREE, entries.nodes);
  json_step(steps, JSON_STEP_TEXT, text_keyed ? "}" : "]}");
  for (size_t index = entries.length; index-- > 0;) {
    if (text_keyed) {
      json_step(steps, JSON_STEP_VALUE, entries.nodes[index]->value);
      json_step(steps, JSON_STEP_KEY, entries.nodes[index]);
      if (index) json_step(steps, JSON_STEP_TEXT, ",");
    } else {
      json_step(steps, JSON_STEP_TEXT, "]");
      json_step(steps, JSON_STEP_VALUE, entries.nodes[index]->value);
      json_step(steps, JSON_STEP_TEXT, ",");
      json_step(steps, JSON_STEP_VALUE, entries.nodes[index]->key);
      json_step(steps, JSON_STEP_TEXT, index ? ",[" : "[");
    }
  }
}

static void json_value(FILE *output, const ChimiValue *root) {
  JsonSteps steps = {0};
  json_step(&steps, JSON_STEP_VALUE, root);
  while (steps.length) {
    JsonStep step = steps.items[--steps.length];
    switch (step.kind) {
      case JSON_STEP_TEXT: fputs(step.pointer, output); continue;
      case JSON_STEP_NAME: json_string(output, step.pointer); fputc(':', output); continue;
      /* A Text key's member name and `:`; json_push_map queues no other key. */
      case JSON_STEP_KEY: {
        const MapNode *entry = step.pointer;
        json_bytes(output, entry->key->as.text.bytes, entry->key->as.text.length); fputc(':', output);
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
      case VALUE_TEXT: json_bytes(output, value->as.text.bytes, value->as.text.length); break;
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
      case VALUE_INSTANT: fprintf(output, "{\"instant\":%" PRId64 "}", value->as.integer); break;
      case VALUE_TIMESTAMP: fprintf(output, "{\"timestamp\":%" PRId64 "}", value->as.integer); break;
      case VALUE_DECIMAL: {
        char text[DECIMAL_TEXT_BYTES]; decimal_normalized_text(decimal_of(value), text);
        fputs("{\"decimal\":", output); json_string(output, text); fputc(0x7d, output); break;
      }
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
      case VALUE_MAP: json_push_map(output, &steps, value->as.map); break;
      /* A set is a list of its items in ascending order, as the interpreter writes it. */
      case VALUE_SET: {
        MapEntries entries = map_entries(value->as.map);
        fputc('[', output);
        json_step(&steps, JSON_STEP_FREE, entries.nodes);
        json_step(&steps, JSON_STEP_TEXT, "]");
        for (size_t index = entries.length; index-- > 0;) {
          json_step(&steps, JSON_STEP_VALUE, entries.nodes[index]->key);
          if (index) json_step(&steps, JSON_STEP_TEXT, ",");
        }
        break;
      }
      case VALUE_JSON_NUMBER: json_string(output, value->as.json_number); break;
      /* Lowercase hex, two digits per byte, as the interpreter writes it. */
      case VALUE_BYTES:
        fputs("{\"bytes\":\"", output);
        for (size_t i = 0; i < value->as.text.length; ++i) {
          unsigned char byte = (unsigned char)value->as.text.bytes[i];
          fputc(hex_digit(byte >> 4), output); fputc(hex_digit(byte), output);
        }
        fputs("\"}", output); break;
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
      case VALUE_PROCESS_MONITOR: fputs("{\"type\":\"Monitor\",\"fields\":{}}", output); break;
      case VALUE_CAUSE: fputs("{\"type\":\"Cause\",\"fields\":{}}", output); break;
    }
  }
  free(steps.items);
}

static uint32_t constructor_ordinal(uint32_t symbol) {
  return CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[symbol].constructor].ordinal;
}

/* Requires: both values have the same equatable type. Returns -1, 0, or 1. */
static int chimi_value_compare(const ChimiValue *left_root, const ChimiValue *right_root) {
  CompareStack stack; int order = 0;
  stack.steps = stack.inline_steps; stack.length = 0; stack.capacity = COMPARE_INLINE_STEPS;
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
      case VALUE_DURATION:
      case VALUE_INSTANT:
      case VALUE_TIMESTAMP: order = order_of(left->as.integer, right->as.integer); break;
      case VALUE_DECIMAL: order = decimal_compare(decimal_of(left), decimal_of(right)); break;
      /* Finite, so total; -0.0 and 0.0 are equal. */
      case VALUE_FLOAT: order = (left->as.floating > right->as.floating) - (left->as.floating < right->as.floating); break;
      case VALUE_BOOLEAN: order = order_of(left->as.boolean, right->as.boolean); break;
      /* Lexicographic by unsigned byte: memcmp compares as unsigned char. */
      case VALUE_TEXT:
      case VALUE_BYTES: {
        size_t left_length = left->as.text.length, right_length = right->as.text.length;
        int bytes = memcmp(left->as.text.bytes, right->as.text.bytes,
                           left_length < right_length ? left_length : right_length);
        order = bytes ? (bytes > 0) - (bytes < 0) : order_of(left_length, right_length); break;
      }
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
      /* A set's values are all Void, so the walk compares its items. */
      case VALUE_MAP:
      case VALUE_SET: {
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
      case VALUE_MANAGED_SUPERVISOR: case VALUE_CAUSE:
        order = order_of((int64_t)(uintptr_t)left, (int64_t)(uintptr_t)right); break;
      /* Routing kind, then the name or incarnation it routes to. */
      case VALUE_PROCESS_MONITOR:
        order = order_of((int64_t)left->as.address.target, (int64_t)right->as.address.target); break;
      case VALUE_PROCESS_ADDRESS:
        order = order_of(left->as.address.kind, right->as.address.kind);
        if (order == 0)
          order = order_of((int64_t)left->as.address.target, (int64_t)right->as.address.target);
        break;
    }
  }
  if (stack.steps != stack.inline_steps) free(stack.steps);
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
static void buffer_json_string(TextBuffer *buffer, const ChimiValue *text) {
  buffer_char(buffer, '"');
  const unsigned char *cursor = (const unsigned char *)text->as.text.bytes;
  for (const unsigned char *end = cursor + text->as.text.length; cursor < end; ++cursor) {
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
      buffer_json_string(buffer, field); return true;
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
        buffer_json_string(buffer, entry->key); buffer_char(buffer, ':');
        break;
      }
      case JSON_STEP_FREE: free((void *)step.pointer); break;
      case JSON_STEP_VALUE: encoded = encode_json_step(buffer, &steps, step.pointer); break;
      /* The outcome writer's step; never queued here. */
      case JSON_STEP_NAME: encoded = false; break;
    }
  }
  free(steps.items);
  return encoded;
}

typedef struct { const char *source; size_t length; size_t offset; const char *message; } JsonParser;
static void json_skip_space(JsonParser *parser) {
  /* Not strchr: it also finds the terminator, so it would skip a U+0000 byte. */
  while (parser->offset < parser->length) {
    char next = parser->source[parser->offset];
    if (next != ' ' && next != '\t' && next != '\r' && next != '\n') break;
    ++parser->offset;
  }
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
/* A JSON string as Text; `\u0000` decodes to U+0000. NULL on failure. */
static ChimiValue *json_parse_string(JsonParser *parser) {
  if (parser->offset >= parser->length || parser->source[parser->offset++] != '"') return NULL;
  TextBuffer buffer = {0};
  while (parser->offset < parser->length) {
    unsigned char value = (unsigned char)parser->source[parser->offset];
    if (value < 0x20) return json_fail(parser, parser->offset, "unescaped control character in JSON string");
    ++parser->offset;
    if (value == '"') return text_from_bytes(buffer.data ? buffer.data : "", buffer.length);
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
    ChimiValue *key = json_parse_string(parser); if (!key) return NULL;
    if (map_get_node(map, key)) {
      TextBuffer message = {0};
      static const char prefix[] = "duplicate JSON object key `";
      buffer_bytes(&message, prefix, sizeof(prefix) - 1);
      buffer_bytes(&message, key->as.text.bytes, key->as.text.length); buffer_char(&message, '`');
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
  if (value == '"') { ChimiValue *raw = json_parse_string(parser); if (!raw) return NULL;
    ChimiValue *fields[] = {raw}; return standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1); }
  if (value == '[') return json_parse_array(parser, depth);
  if (value == '{') return json_parse_object(parser, depth);
  if (value == '-' || (value >= '0' && value <= '9')) return json_parse_number(parser);
  if (value == 'n' || value == 't' || value == 'f') return json_fail(parser, parser->offset, "invalid JSON literal");
  return json_fail(parser, parser->offset, "expected a JSON value");
}

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment);

static ChimiControl *intrinsic_shape_defect(uint32_t call_node);

/* Unicode White_Space, the characters Text.trim_start and Text.trim_end remove; the
   interpreter's core_operations::is_white_space keeps the same table. */
static bool is_white_space(uint32_t code) {
  return (code >= 0x9 && code <= 0xd) || code == 0x20 || code == 0x85 || code == 0xa0 ||
         code == 0x1680 || (code >= 0x2000 && code <= 0x200a) || code == 0x2028 ||
         code == 0x2029 || code == 0x202f || code == 0x205f || code == 0x3000;
}
/* Decodes the valid UTF-8 sequence at `text`, storing its length in `length`. */
static uint32_t utf8_decode(const unsigned char *text, size_t *length) {
  if (text[0] < 0x80) { *length = 1; return text[0]; }
  if (text[0] < 0xe0) { *length = 2; return ((uint32_t)(text[0] & 0x1f) << 6) | (text[1] & 0x3f); }
  if (text[0] < 0xf0) {
    *length = 3;
    return ((uint32_t)(text[0] & 0x0f) << 12) | ((uint32_t)(text[1] & 0x3f) << 6) | (text[2] & 0x3f);
  }
  *length = 4;
  return ((uint32_t)(text[0] & 0x07) << 18) | ((uint32_t)(text[1] & 0x3f) << 12) |
         ((uint32_t)(text[2] & 0x3f) << 6) | (text[3] & 0x3f);
}
static bool utf8_boundary(const char *text, size_t length, size_t offset) {
  return offset == length || ((unsigned char)text[offset] & 0xc0) != 0x80;
}
/* Clamps [start, end) to [0, length]; a reversed range clamps to empty. */
static void clamp_range(int64_t start, int64_t end, size_t length, size_t *from, size_t *to) {
  size_t low = start <= 0 ? 0 : (uint64_t)start > length ? length : (size_t)start;
  size_t high = end <= 0 ? 0 : (uint64_t)end > length ? length : (size_t)end;
  *from = low; *to = high < low ? low : high;
}

/* Text.lowercase and Text.uppercase: the Unicode default case conversion, as the
   interpreter's str::to_lowercase and str::to_uppercase do it, from tables generated
   from those functions. Mappings change length (U+00DF ß uppercases to "SS"). */
typedef struct { uint32_t code; uint32_t mapped[3]; } CaseMapping;
enum { SIGMA_CONTEXT_OTHER, SIGMA_CONTEXT_CASED, SIGMA_CONTEXT_IGNORABLE };
typedef struct { uint32_t first, last; uint8_t context; } SigmaContextRange;
/* BEGIN GENERATED unicode case tables (tests/unicode_tables.rs, Unicode 17.0.0) */
static const CaseMapping lowercase_mappings[] = {
  {0x41, {0x61}}, {0x42, {0x62}}, {0x43, {0x63}}, {0x44, {0x64}}, {0x45, {0x65}}, {0x46, {0x66}},
  {0x47, {0x67}}, {0x48, {0x68}}, {0x49, {0x69}}, {0x4a, {0x6a}}, {0x4b, {0x6b}}, {0x4c, {0x6c}},
  {0x4d, {0x6d}}, {0x4e, {0x6e}}, {0x4f, {0x6f}}, {0x50, {0x70}}, {0x51, {0x71}}, {0x52, {0x72}},
  {0x53, {0x73}}, {0x54, {0x74}}, {0x55, {0x75}}, {0x56, {0x76}}, {0x57, {0x77}}, {0x58, {0x78}},
  {0x59, {0x79}}, {0x5a, {0x7a}}, {0xc0, {0xe0}}, {0xc1, {0xe1}}, {0xc2, {0xe2}}, {0xc3, {0xe3}},
  {0xc4, {0xe4}}, {0xc5, {0xe5}}, {0xc6, {0xe6}}, {0xc7, {0xe7}}, {0xc8, {0xe8}}, {0xc9, {0xe9}},
  {0xca, {0xea}}, {0xcb, {0xeb}}, {0xcc, {0xec}}, {0xcd, {0xed}}, {0xce, {0xee}}, {0xcf, {0xef}},
  {0xd0, {0xf0}}, {0xd1, {0xf1}}, {0xd2, {0xf2}}, {0xd3, {0xf3}}, {0xd4, {0xf4}}, {0xd5, {0xf5}},
  {0xd6, {0xf6}}, {0xd8, {0xf8}}, {0xd9, {0xf9}}, {0xda, {0xfa}}, {0xdb, {0xfb}}, {0xdc, {0xfc}},
  {0xdd, {0xfd}}, {0xde, {0xfe}}, {0x100, {0x101}}, {0x102, {0x103}}, {0x104, {0x105}}, {0x106, {0x107}},
  {0x108, {0x109}}, {0x10a, {0x10b}}, {0x10c, {0x10d}}, {0x10e, {0x10f}}, {0x110, {0x111}}, {0x112, {0x113}},
  {0x114, {0x115}}, {0x116, {0x117}}, {0x118, {0x119}}, {0x11a, {0x11b}}, {0x11c, {0x11d}}, {0x11e, {0x11f}},
  {0x120, {0x121}}, {0x122, {0x123}}, {0x124, {0x125}}, {0x126, {0x127}}, {0x128, {0x129}}, {0x12a, {0x12b}},
  {0x12c, {0x12d}}, {0x12e, {0x12f}}, {0x130, {0x69, 0x307}}, {0x132, {0x133}}, {0x134, {0x135}}, {0x136, {0x137}},
  {0x139, {0x13a}}, {0x13b, {0x13c}}, {0x13d, {0x13e}}, {0x13f, {0x140}}, {0x141, {0x142}}, {0x143, {0x144}},
  {0x145, {0x146}}, {0x147, {0x148}}, {0x14a, {0x14b}}, {0x14c, {0x14d}}, {0x14e, {0x14f}}, {0x150, {0x151}},
  {0x152, {0x153}}, {0x154, {0x155}}, {0x156, {0x157}}, {0x158, {0x159}}, {0x15a, {0x15b}}, {0x15c, {0x15d}},
  {0x15e, {0x15f}}, {0x160, {0x161}}, {0x162, {0x163}}, {0x164, {0x165}}, {0x166, {0x167}}, {0x168, {0x169}},
  {0x16a, {0x16b}}, {0x16c, {0x16d}}, {0x16e, {0x16f}}, {0x170, {0x171}}, {0x172, {0x173}}, {0x174, {0x175}},
  {0x176, {0x177}}, {0x178, {0xff}}, {0x179, {0x17a}}, {0x17b, {0x17c}}, {0x17d, {0x17e}}, {0x181, {0x253}},
  {0x182, {0x183}}, {0x184, {0x185}}, {0x186, {0x254}}, {0x187, {0x188}}, {0x189, {0x256}}, {0x18a, {0x257}},
  {0x18b, {0x18c}}, {0x18e, {0x1dd}}, {0x18f, {0x259}}, {0x190, {0x25b}}, {0x191, {0x192}}, {0x193, {0x260}},
  {0x194, {0x263}}, {0x196, {0x269}}, {0x197, {0x268}}, {0x198, {0x199}}, {0x19c, {0x26f}}, {0x19d, {0x272}},
  {0x19f, {0x275}}, {0x1a0, {0x1a1}}, {0x1a2, {0x1a3}}, {0x1a4, {0x1a5}}, {0x1a6, {0x280}}, {0x1a7, {0x1a8}},
  {0x1a9, {0x283}}, {0x1ac, {0x1ad}}, {0x1ae, {0x288}}, {0x1af, {0x1b0}}, {0x1b1, {0x28a}}, {0x1b2, {0x28b}},
  {0x1b3, {0x1b4}}, {0x1b5, {0x1b6}}, {0x1b7, {0x292}}, {0x1b8, {0x1b9}}, {0x1bc, {0x1bd}}, {0x1c4, {0x1c6}},
  {0x1c5, {0x1c6}}, {0x1c7, {0x1c9}}, {0x1c8, {0x1c9}}, {0x1ca, {0x1cc}}, {0x1cb, {0x1cc}}, {0x1cd, {0x1ce}},
  {0x1cf, {0x1d0}}, {0x1d1, {0x1d2}}, {0x1d3, {0x1d4}}, {0x1d5, {0x1d6}}, {0x1d7, {0x1d8}}, {0x1d9, {0x1da}},
  {0x1db, {0x1dc}}, {0x1de, {0x1df}}, {0x1e0, {0x1e1}}, {0x1e2, {0x1e3}}, {0x1e4, {0x1e5}}, {0x1e6, {0x1e7}},
  {0x1e8, {0x1e9}}, {0x1ea, {0x1eb}}, {0x1ec, {0x1ed}}, {0x1ee, {0x1ef}}, {0x1f1, {0x1f3}}, {0x1f2, {0x1f3}},
  {0x1f4, {0x1f5}}, {0x1f6, {0x195}}, {0x1f7, {0x1bf}}, {0x1f8, {0x1f9}}, {0x1fa, {0x1fb}}, {0x1fc, {0x1fd}},
  {0x1fe, {0x1ff}}, {0x200, {0x201}}, {0x202, {0x203}}, {0x204, {0x205}}, {0x206, {0x207}}, {0x208, {0x209}},
  {0x20a, {0x20b}}, {0x20c, {0x20d}}, {0x20e, {0x20f}}, {0x210, {0x211}}, {0x212, {0x213}}, {0x214, {0x215}},
  {0x216, {0x217}}, {0x218, {0x219}}, {0x21a, {0x21b}}, {0x21c, {0x21d}}, {0x21e, {0x21f}}, {0x220, {0x19e}},
  {0x222, {0x223}}, {0x224, {0x225}}, {0x226, {0x227}}, {0x228, {0x229}}, {0x22a, {0x22b}}, {0x22c, {0x22d}},
  {0x22e, {0x22f}}, {0x230, {0x231}}, {0x232, {0x233}}, {0x23a, {0x2c65}}, {0x23b, {0x23c}}, {0x23d, {0x19a}},
  {0x23e, {0x2c66}}, {0x241, {0x242}}, {0x243, {0x180}}, {0x244, {0x289}}, {0x245, {0x28c}}, {0x246, {0x247}},
  {0x248, {0x249}}, {0x24a, {0x24b}}, {0x24c, {0x24d}}, {0x24e, {0x24f}}, {0x370, {0x371}}, {0x372, {0x373}},
  {0x376, {0x377}}, {0x37f, {0x3f3}}, {0x386, {0x3ac}}, {0x388, {0x3ad}}, {0x389, {0x3ae}}, {0x38a, {0x3af}},
  {0x38c, {0x3cc}}, {0x38e, {0x3cd}}, {0x38f, {0x3ce}}, {0x391, {0x3b1}}, {0x392, {0x3b2}}, {0x393, {0x3b3}},
  {0x394, {0x3b4}}, {0x395, {0x3b5}}, {0x396, {0x3b6}}, {0x397, {0x3b7}}, {0x398, {0x3b8}}, {0x399, {0x3b9}},
  {0x39a, {0x3ba}}, {0x39b, {0x3bb}}, {0x39c, {0x3bc}}, {0x39d, {0x3bd}}, {0x39e, {0x3be}}, {0x39f, {0x3bf}},
  {0x3a0, {0x3c0}}, {0x3a1, {0x3c1}}, {0x3a3, {0x3c3}}, {0x3a4, {0x3c4}}, {0x3a5, {0x3c5}}, {0x3a6, {0x3c6}},
  {0x3a7, {0x3c7}}, {0x3a8, {0x3c8}}, {0x3a9, {0x3c9}}, {0x3aa, {0x3ca}}, {0x3ab, {0x3cb}}, {0x3cf, {0x3d7}},
  {0x3d8, {0x3d9}}, {0x3da, {0x3db}}, {0x3dc, {0x3dd}}, {0x3de, {0x3df}}, {0x3e0, {0x3e1}}, {0x3e2, {0x3e3}},
  {0x3e4, {0x3e5}}, {0x3e6, {0x3e7}}, {0x3e8, {0x3e9}}, {0x3ea, {0x3eb}}, {0x3ec, {0x3ed}}, {0x3ee, {0x3ef}},
  {0x3f4, {0x3b8}}, {0x3f7, {0x3f8}}, {0x3f9, {0x3f2}}, {0x3fa, {0x3fb}}, {0x3fd, {0x37b}}, {0x3fe, {0x37c}},
  {0x3ff, {0x37d}}, {0x400, {0x450}}, {0x401, {0x451}}, {0x402, {0x452}}, {0x403, {0x453}}, {0x404, {0x454}},
  {0x405, {0x455}}, {0x406, {0x456}}, {0x407, {0x457}}, {0x408, {0x458}}, {0x409, {0x459}}, {0x40a, {0x45a}},
  {0x40b, {0x45b}}, {0x40c, {0x45c}}, {0x40d, {0x45d}}, {0x40e, {0x45e}}, {0x40f, {0x45f}}, {0x410, {0x430}},
  {0x411, {0x431}}, {0x412, {0x432}}, {0x413, {0x433}}, {0x414, {0x434}}, {0x415, {0x435}}, {0x416, {0x436}},
  {0x417, {0x437}}, {0x418, {0x438}}, {0x419, {0x439}}, {0x41a, {0x43a}}, {0x41b, {0x43b}}, {0x41c, {0x43c}},
  {0x41d, {0x43d}}, {0x41e, {0x43e}}, {0x41f, {0x43f}}, {0x420, {0x440}}, {0x421, {0x441}}, {0x422, {0x442}},
  {0x423, {0x443}}, {0x424, {0x444}}, {0x425, {0x445}}, {0x426, {0x446}}, {0x427, {0x447}}, {0x428, {0x448}},
  {0x429, {0x449}}, {0x42a, {0x44a}}, {0x42b, {0x44b}}, {0x42c, {0x44c}}, {0x42d, {0x44d}}, {0x42e, {0x44e}},
  {0x42f, {0x44f}}, {0x460, {0x461}}, {0x462, {0x463}}, {0x464, {0x465}}, {0x466, {0x467}}, {0x468, {0x469}},
  {0x46a, {0x46b}}, {0x46c, {0x46d}}, {0x46e, {0x46f}}, {0x470, {0x471}}, {0x472, {0x473}}, {0x474, {0x475}},
  {0x476, {0x477}}, {0x478, {0x479}}, {0x47a, {0x47b}}, {0x47c, {0x47d}}, {0x47e, {0x47f}}, {0x480, {0x481}},
  {0x48a, {0x48b}}, {0x48c, {0x48d}}, {0x48e, {0x48f}}, {0x490, {0x491}}, {0x492, {0x493}}, {0x494, {0x495}},
  {0x496, {0x497}}, {0x498, {0x499}}, {0x49a, {0x49b}}, {0x49c, {0x49d}}, {0x49e, {0x49f}}, {0x4a0, {0x4a1}},
  {0x4a2, {0x4a3}}, {0x4a4, {0x4a5}}, {0x4a6, {0x4a7}}, {0x4a8, {0x4a9}}, {0x4aa, {0x4ab}}, {0x4ac, {0x4ad}},
  {0x4ae, {0x4af}}, {0x4b0, {0x4b1}}, {0x4b2, {0x4b3}}, {0x4b4, {0x4b5}}, {0x4b6, {0x4b7}}, {0x4b8, {0x4b9}},
  {0x4ba, {0x4bb}}, {0x4bc, {0x4bd}}, {0x4be, {0x4bf}}, {0x4c0, {0x4cf}}, {0x4c1, {0x4c2}}, {0x4c3, {0x4c4}},
  {0x4c5, {0x4c6}}, {0x4c7, {0x4c8}}, {0x4c9, {0x4ca}}, {0x4cb, {0x4cc}}, {0x4cd, {0x4ce}}, {0x4d0, {0x4d1}},
  {0x4d2, {0x4d3}}, {0x4d4, {0x4d5}}, {0x4d6, {0x4d7}}, {0x4d8, {0x4d9}}, {0x4da, {0x4db}}, {0x4dc, {0x4dd}},
  {0x4de, {0x4df}}, {0x4e0, {0x4e1}}, {0x4e2, {0x4e3}}, {0x4e4, {0x4e5}}, {0x4e6, {0x4e7}}, {0x4e8, {0x4e9}},
  {0x4ea, {0x4eb}}, {0x4ec, {0x4ed}}, {0x4ee, {0x4ef}}, {0x4f0, {0x4f1}}, {0x4f2, {0x4f3}}, {0x4f4, {0x4f5}},
  {0x4f6, {0x4f7}}, {0x4f8, {0x4f9}}, {0x4fa, {0x4fb}}, {0x4fc, {0x4fd}}, {0x4fe, {0x4ff}}, {0x500, {0x501}},
  {0x502, {0x503}}, {0x504, {0x505}}, {0x506, {0x507}}, {0x508, {0x509}}, {0x50a, {0x50b}}, {0x50c, {0x50d}},
  {0x50e, {0x50f}}, {0x510, {0x511}}, {0x512, {0x513}}, {0x514, {0x515}}, {0x516, {0x517}}, {0x518, {0x519}},
  {0x51a, {0x51b}}, {0x51c, {0x51d}}, {0x51e, {0x51f}}, {0x520, {0x521}}, {0x522, {0x523}}, {0x524, {0x525}},
  {0x526, {0x527}}, {0x528, {0x529}}, {0x52a, {0x52b}}, {0x52c, {0x52d}}, {0x52e, {0x52f}}, {0x531, {0x561}},
  {0x532, {0x562}}, {0x533, {0x563}}, {0x534, {0x564}}, {0x535, {0x565}}, {0x536, {0x566}}, {0x537, {0x567}},
  {0x538, {0x568}}, {0x539, {0x569}}, {0x53a, {0x56a}}, {0x53b, {0x56b}}, {0x53c, {0x56c}}, {0x53d, {0x56d}},
  {0x53e, {0x56e}}, {0x53f, {0x56f}}, {0x540, {0x570}}, {0x541, {0x571}}, {0x542, {0x572}}, {0x543, {0x573}},
  {0x544, {0x574}}, {0x545, {0x575}}, {0x546, {0x576}}, {0x547, {0x577}}, {0x548, {0x578}}, {0x549, {0x579}},
  {0x54a, {0x57a}}, {0x54b, {0x57b}}, {0x54c, {0x57c}}, {0x54d, {0x57d}}, {0x54e, {0x57e}}, {0x54f, {0x57f}},
  {0x550, {0x580}}, {0x551, {0x581}}, {0x552, {0x582}}, {0x553, {0x583}}, {0x554, {0x584}}, {0x555, {0x585}},
  {0x556, {0x586}}, {0x10a0, {0x2d00}}, {0x10a1, {0x2d01}}, {0x10a2, {0x2d02}}, {0x10a3, {0x2d03}}, {0x10a4, {0x2d04}},
  {0x10a5, {0x2d05}}, {0x10a6, {0x2d06}}, {0x10a7, {0x2d07}}, {0x10a8, {0x2d08}}, {0x10a9, {0x2d09}}, {0x10aa, {0x2d0a}},
  {0x10ab, {0x2d0b}}, {0x10ac, {0x2d0c}}, {0x10ad, {0x2d0d}}, {0x10ae, {0x2d0e}}, {0x10af, {0x2d0f}}, {0x10b0, {0x2d10}},
  {0x10b1, {0x2d11}}, {0x10b2, {0x2d12}}, {0x10b3, {0x2d13}}, {0x10b4, {0x2d14}}, {0x10b5, {0x2d15}}, {0x10b6, {0x2d16}},
  {0x10b7, {0x2d17}}, {0x10b8, {0x2d18}}, {0x10b9, {0x2d19}}, {0x10ba, {0x2d1a}}, {0x10bb, {0x2d1b}}, {0x10bc, {0x2d1c}},
  {0x10bd, {0x2d1d}}, {0x10be, {0x2d1e}}, {0x10bf, {0x2d1f}}, {0x10c0, {0x2d20}}, {0x10c1, {0x2d21}}, {0x10c2, {0x2d22}},
  {0x10c3, {0x2d23}}, {0x10c4, {0x2d24}}, {0x10c5, {0x2d25}}, {0x10c7, {0x2d27}}, {0x10cd, {0x2d2d}}, {0x13a0, {0xab70}},
  {0x13a1, {0xab71}}, {0x13a2, {0xab72}}, {0x13a3, {0xab73}}, {0x13a4, {0xab74}}, {0x13a5, {0xab75}}, {0x13a6, {0xab76}},
  {0x13a7, {0xab77}}, {0x13a8, {0xab78}}, {0x13a9, {0xab79}}, {0x13aa, {0xab7a}}, {0x13ab, {0xab7b}}, {0x13ac, {0xab7c}},
  {0x13ad, {0xab7d}}, {0x13ae, {0xab7e}}, {0x13af, {0xab7f}}, {0x13b0, {0xab80}}, {0x13b1, {0xab81}}, {0x13b2, {0xab82}},
  {0x13b3, {0xab83}}, {0x13b4, {0xab84}}, {0x13b5, {0xab85}}, {0x13b6, {0xab86}}, {0x13b7, {0xab87}}, {0x13b8, {0xab88}},
  {0x13b9, {0xab89}}, {0x13ba, {0xab8a}}, {0x13bb, {0xab8b}}, {0x13bc, {0xab8c}}, {0x13bd, {0xab8d}}, {0x13be, {0xab8e}},
  {0x13bf, {0xab8f}}, {0x13c0, {0xab90}}, {0x13c1, {0xab91}}, {0x13c2, {0xab92}}, {0x13c3, {0xab93}}, {0x13c4, {0xab94}},
  {0x13c5, {0xab95}}, {0x13c6, {0xab96}}, {0x13c7, {0xab97}}, {0x13c8, {0xab98}}, {0x13c9, {0xab99}}, {0x13ca, {0xab9a}},
  {0x13cb, {0xab9b}}, {0x13cc, {0xab9c}}, {0x13cd, {0xab9d}}, {0x13ce, {0xab9e}}, {0x13cf, {0xab9f}}, {0x13d0, {0xaba0}},
  {0x13d1, {0xaba1}}, {0x13d2, {0xaba2}}, {0x13d3, {0xaba3}}, {0x13d4, {0xaba4}}, {0x13d5, {0xaba5}}, {0x13d6, {0xaba6}},
  {0x13d7, {0xaba7}}, {0x13d8, {0xaba8}}, {0x13d9, {0xaba9}}, {0x13da, {0xabaa}}, {0x13db, {0xabab}}, {0x13dc, {0xabac}},
  {0x13dd, {0xabad}}, {0x13de, {0xabae}}, {0x13df, {0xabaf}}, {0x13e0, {0xabb0}}, {0x13e1, {0xabb1}}, {0x13e2, {0xabb2}},
  {0x13e3, {0xabb3}}, {0x13e4, {0xabb4}}, {0x13e5, {0xabb5}}, {0x13e6, {0xabb6}}, {0x13e7, {0xabb7}}, {0x13e8, {0xabb8}},
  {0x13e9, {0xabb9}}, {0x13ea, {0xabba}}, {0x13eb, {0xabbb}}, {0x13ec, {0xabbc}}, {0x13ed, {0xabbd}}, {0x13ee, {0xabbe}},
  {0x13ef, {0xabbf}}, {0x13f0, {0x13f8}}, {0x13f1, {0x13f9}}, {0x13f2, {0x13fa}}, {0x13f3, {0x13fb}}, {0x13f4, {0x13fc}},
  {0x13f5, {0x13fd}}, {0x1c89, {0x1c8a}}, {0x1c90, {0x10d0}}, {0x1c91, {0x10d1}}, {0x1c92, {0x10d2}}, {0x1c93, {0x10d3}},
  {0x1c94, {0x10d4}}, {0x1c95, {0x10d5}}, {0x1c96, {0x10d6}}, {0x1c97, {0x10d7}}, {0x1c98, {0x10d8}}, {0x1c99, {0x10d9}},
  {0x1c9a, {0x10da}}, {0x1c9b, {0x10db}}, {0x1c9c, {0x10dc}}, {0x1c9d, {0x10dd}}, {0x1c9e, {0x10de}}, {0x1c9f, {0x10df}},
  {0x1ca0, {0x10e0}}, {0x1ca1, {0x10e1}}, {0x1ca2, {0x10e2}}, {0x1ca3, {0x10e3}}, {0x1ca4, {0x10e4}}, {0x1ca5, {0x10e5}},
  {0x1ca6, {0x10e6}}, {0x1ca7, {0x10e7}}, {0x1ca8, {0x10e8}}, {0x1ca9, {0x10e9}}, {0x1caa, {0x10ea}}, {0x1cab, {0x10eb}},
  {0x1cac, {0x10ec}}, {0x1cad, {0x10ed}}, {0x1cae, {0x10ee}}, {0x1caf, {0x10ef}}, {0x1cb0, {0x10f0}}, {0x1cb1, {0x10f1}},
  {0x1cb2, {0x10f2}}, {0x1cb3, {0x10f3}}, {0x1cb4, {0x10f4}}, {0x1cb5, {0x10f5}}, {0x1cb6, {0x10f6}}, {0x1cb7, {0x10f7}},
  {0x1cb8, {0x10f8}}, {0x1cb9, {0x10f9}}, {0x1cba, {0x10fa}}, {0x1cbd, {0x10fd}}, {0x1cbe, {0x10fe}}, {0x1cbf, {0x10ff}},
  {0x1e00, {0x1e01}}, {0x1e02, {0x1e03}}, {0x1e04, {0x1e05}}, {0x1e06, {0x1e07}}, {0x1e08, {0x1e09}}, {0x1e0a, {0x1e0b}},
  {0x1e0c, {0x1e0d}}, {0x1e0e, {0x1e0f}}, {0x1e10, {0x1e11}}, {0x1e12, {0x1e13}}, {0x1e14, {0x1e15}}, {0x1e16, {0x1e17}},
  {0x1e18, {0x1e19}}, {0x1e1a, {0x1e1b}}, {0x1e1c, {0x1e1d}}, {0x1e1e, {0x1e1f}}, {0x1e20, {0x1e21}}, {0x1e22, {0x1e23}},
  {0x1e24, {0x1e25}}, {0x1e26, {0x1e27}}, {0x1e28, {0x1e29}}, {0x1e2a, {0x1e2b}}, {0x1e2c, {0x1e2d}}, {0x1e2e, {0x1e2f}},
  {0x1e30, {0x1e31}}, {0x1e32, {0x1e33}}, {0x1e34, {0x1e35}}, {0x1e36, {0x1e37}}, {0x1e38, {0x1e39}}, {0x1e3a, {0x1e3b}},
  {0x1e3c, {0x1e3d}}, {0x1e3e, {0x1e3f}}, {0x1e40, {0x1e41}}, {0x1e42, {0x1e43}}, {0x1e44, {0x1e45}}, {0x1e46, {0x1e47}},
  {0x1e48, {0x1e49}}, {0x1e4a, {0x1e4b}}, {0x1e4c, {0x1e4d}}, {0x1e4e, {0x1e4f}}, {0x1e50, {0x1e51}}, {0x1e52, {0x1e53}},
  {0x1e54, {0x1e55}}, {0x1e56, {0x1e57}}, {0x1e58, {0x1e59}}, {0x1e5a, {0x1e5b}}, {0x1e5c, {0x1e5d}}, {0x1e5e, {0x1e5f}},
  {0x1e60, {0x1e61}}, {0x1e62, {0x1e63}}, {0x1e64, {0x1e65}}, {0x1e66, {0x1e67}}, {0x1e68, {0x1e69}}, {0x1e6a, {0x1e6b}},
  {0x1e6c, {0x1e6d}}, {0x1e6e, {0x1e6f}}, {0x1e70, {0x1e71}}, {0x1e72, {0x1e73}}, {0x1e74, {0x1e75}}, {0x1e76, {0x1e77}},
  {0x1e78, {0x1e79}}, {0x1e7a, {0x1e7b}}, {0x1e7c, {0x1e7d}}, {0x1e7e, {0x1e7f}}, {0x1e80, {0x1e81}}, {0x1e82, {0x1e83}},
  {0x1e84, {0x1e85}}, {0x1e86, {0x1e87}}, {0x1e88, {0x1e89}}, {0x1e8a, {0x1e8b}}, {0x1e8c, {0x1e8d}}, {0x1e8e, {0x1e8f}},
  {0x1e90, {0x1e91}}, {0x1e92, {0x1e93}}, {0x1e94, {0x1e95}}, {0x1e9e, {0xdf}}, {0x1ea0, {0x1ea1}}, {0x1ea2, {0x1ea3}},
  {0x1ea4, {0x1ea5}}, {0x1ea6, {0x1ea7}}, {0x1ea8, {0x1ea9}}, {0x1eaa, {0x1eab}}, {0x1eac, {0x1ead}}, {0x1eae, {0x1eaf}},
  {0x1eb0, {0x1eb1}}, {0x1eb2, {0x1eb3}}, {0x1eb4, {0x1eb5}}, {0x1eb6, {0x1eb7}}, {0x1eb8, {0x1eb9}}, {0x1eba, {0x1ebb}},
  {0x1ebc, {0x1ebd}}, {0x1ebe, {0x1ebf}}, {0x1ec0, {0x1ec1}}, {0x1ec2, {0x1ec3}}, {0x1ec4, {0x1ec5}}, {0x1ec6, {0x1ec7}},
  {0x1ec8, {0x1ec9}}, {0x1eca, {0x1ecb}}, {0x1ecc, {0x1ecd}}, {0x1ece, {0x1ecf}}, {0x1ed0, {0x1ed1}}, {0x1ed2, {0x1ed3}},
  {0x1ed4, {0x1ed5}}, {0x1ed6, {0x1ed7}}, {0x1ed8, {0x1ed9}}, {0x1eda, {0x1edb}}, {0x1edc, {0x1edd}}, {0x1ede, {0x1edf}},
  {0x1ee0, {0x1ee1}}, {0x1ee2, {0x1ee3}}, {0x1ee4, {0x1ee5}}, {0x1ee6, {0x1ee7}}, {0x1ee8, {0x1ee9}}, {0x1eea, {0x1eeb}},
  {0x1eec, {0x1eed}}, {0x1eee, {0x1eef}}, {0x1ef0, {0x1ef1}}, {0x1ef2, {0x1ef3}}, {0x1ef4, {0x1ef5}}, {0x1ef6, {0x1ef7}},
  {0x1ef8, {0x1ef9}}, {0x1efa, {0x1efb}}, {0x1efc, {0x1efd}}, {0x1efe, {0x1eff}}, {0x1f08, {0x1f00}}, {0x1f09, {0x1f01}},
  {0x1f0a, {0x1f02}}, {0x1f0b, {0x1f03}}, {0x1f0c, {0x1f04}}, {0x1f0d, {0x1f05}}, {0x1f0e, {0x1f06}}, {0x1f0f, {0x1f07}},
  {0x1f18, {0x1f10}}, {0x1f19, {0x1f11}}, {0x1f1a, {0x1f12}}, {0x1f1b, {0x1f13}}, {0x1f1c, {0x1f14}}, {0x1f1d, {0x1f15}},
  {0x1f28, {0x1f20}}, {0x1f29, {0x1f21}}, {0x1f2a, {0x1f22}}, {0x1f2b, {0x1f23}}, {0x1f2c, {0x1f24}}, {0x1f2d, {0x1f25}},
  {0x1f2e, {0x1f26}}, {0x1f2f, {0x1f27}}, {0x1f38, {0x1f30}}, {0x1f39, {0x1f31}}, {0x1f3a, {0x1f32}}, {0x1f3b, {0x1f33}},
  {0x1f3c, {0x1f34}}, {0x1f3d, {0x1f35}}, {0x1f3e, {0x1f36}}, {0x1f3f, {0x1f37}}, {0x1f48, {0x1f40}}, {0x1f49, {0x1f41}},
  {0x1f4a, {0x1f42}}, {0x1f4b, {0x1f43}}, {0x1f4c, {0x1f44}}, {0x1f4d, {0x1f45}}, {0x1f59, {0x1f51}}, {0x1f5b, {0x1f53}},
  {0x1f5d, {0x1f55}}, {0x1f5f, {0x1f57}}, {0x1f68, {0x1f60}}, {0x1f69, {0x1f61}}, {0x1f6a, {0x1f62}}, {0x1f6b, {0x1f63}},
  {0x1f6c, {0x1f64}}, {0x1f6d, {0x1f65}}, {0x1f6e, {0x1f66}}, {0x1f6f, {0x1f67}}, {0x1f88, {0x1f80}}, {0x1f89, {0x1f81}},
  {0x1f8a, {0x1f82}}, {0x1f8b, {0x1f83}}, {0x1f8c, {0x1f84}}, {0x1f8d, {0x1f85}}, {0x1f8e, {0x1f86}}, {0x1f8f, {0x1f87}},
  {0x1f98, {0x1f90}}, {0x1f99, {0x1f91}}, {0x1f9a, {0x1f92}}, {0x1f9b, {0x1f93}}, {0x1f9c, {0x1f94}}, {0x1f9d, {0x1f95}},
  {0x1f9e, {0x1f96}}, {0x1f9f, {0x1f97}}, {0x1fa8, {0x1fa0}}, {0x1fa9, {0x1fa1}}, {0x1faa, {0x1fa2}}, {0x1fab, {0x1fa3}},
  {0x1fac, {0x1fa4}}, {0x1fad, {0x1fa5}}, {0x1fae, {0x1fa6}}, {0x1faf, {0x1fa7}}, {0x1fb8, {0x1fb0}}, {0x1fb9, {0x1fb1}},
  {0x1fba, {0x1f70}}, {0x1fbb, {0x1f71}}, {0x1fbc, {0x1fb3}}, {0x1fc8, {0x1f72}}, {0x1fc9, {0x1f73}}, {0x1fca, {0x1f74}},
  {0x1fcb, {0x1f75}}, {0x1fcc, {0x1fc3}}, {0x1fd8, {0x1fd0}}, {0x1fd9, {0x1fd1}}, {0x1fda, {0x1f76}}, {0x1fdb, {0x1f77}},
  {0x1fe8, {0x1fe0}}, {0x1fe9, {0x1fe1}}, {0x1fea, {0x1f7a}}, {0x1feb, {0x1f7b}}, {0x1fec, {0x1fe5}}, {0x1ff8, {0x1f78}},
  {0x1ff9, {0x1f79}}, {0x1ffa, {0x1f7c}}, {0x1ffb, {0x1f7d}}, {0x1ffc, {0x1ff3}}, {0x2126, {0x3c9}}, {0x212a, {0x6b}},
  {0x212b, {0xe5}}, {0x2132, {0x214e}}, {0x2160, {0x2170}}, {0x2161, {0x2171}}, {0x2162, {0x2172}}, {0x2163, {0x2173}},
  {0x2164, {0x2174}}, {0x2165, {0x2175}}, {0x2166, {0x2176}}, {0x2167, {0x2177}}, {0x2168, {0x2178}}, {0x2169, {0x2179}},
  {0x216a, {0x217a}}, {0x216b, {0x217b}}, {0x216c, {0x217c}}, {0x216d, {0x217d}}, {0x216e, {0x217e}}, {0x216f, {0x217f}},
  {0x2183, {0x2184}}, {0x24b6, {0x24d0}}, {0x24b7, {0x24d1}}, {0x24b8, {0x24d2}}, {0x24b9, {0x24d3}}, {0x24ba, {0x24d4}},
  {0x24bb, {0x24d5}}, {0x24bc, {0x24d6}}, {0x24bd, {0x24d7}}, {0x24be, {0x24d8}}, {0x24bf, {0x24d9}}, {0x24c0, {0x24da}},
  {0x24c1, {0x24db}}, {0x24c2, {0x24dc}}, {0x24c3, {0x24dd}}, {0x24c4, {0x24de}}, {0x24c5, {0x24df}}, {0x24c6, {0x24e0}},
  {0x24c7, {0x24e1}}, {0x24c8, {0x24e2}}, {0x24c9, {0x24e3}}, {0x24ca, {0x24e4}}, {0x24cb, {0x24e5}}, {0x24cc, {0x24e6}},
  {0x24cd, {0x24e7}}, {0x24ce, {0x24e8}}, {0x24cf, {0x24e9}}, {0x2c00, {0x2c30}}, {0x2c01, {0x2c31}}, {0x2c02, {0x2c32}},
  {0x2c03, {0x2c33}}, {0x2c04, {0x2c34}}, {0x2c05, {0x2c35}}, {0x2c06, {0x2c36}}, {0x2c07, {0x2c37}}, {0x2c08, {0x2c38}},
  {0x2c09, {0x2c39}}, {0x2c0a, {0x2c3a}}, {0x2c0b, {0x2c3b}}, {0x2c0c, {0x2c3c}}, {0x2c0d, {0x2c3d}}, {0x2c0e, {0x2c3e}},
  {0x2c0f, {0x2c3f}}, {0x2c10, {0x2c40}}, {0x2c11, {0x2c41}}, {0x2c12, {0x2c42}}, {0x2c13, {0x2c43}}, {0x2c14, {0x2c44}},
  {0x2c15, {0x2c45}}, {0x2c16, {0x2c46}}, {0x2c17, {0x2c47}}, {0x2c18, {0x2c48}}, {0x2c19, {0x2c49}}, {0x2c1a, {0x2c4a}},
  {0x2c1b, {0x2c4b}}, {0x2c1c, {0x2c4c}}, {0x2c1d, {0x2c4d}}, {0x2c1e, {0x2c4e}}, {0x2c1f, {0x2c4f}}, {0x2c20, {0x2c50}},
  {0x2c21, {0x2c51}}, {0x2c22, {0x2c52}}, {0x2c23, {0x2c53}}, {0x2c24, {0x2c54}}, {0x2c25, {0x2c55}}, {0x2c26, {0x2c56}},
  {0x2c27, {0x2c57}}, {0x2c28, {0x2c58}}, {0x2c29, {0x2c59}}, {0x2c2a, {0x2c5a}}, {0x2c2b, {0x2c5b}}, {0x2c2c, {0x2c5c}},
  {0x2c2d, {0x2c5d}}, {0x2c2e, {0x2c5e}}, {0x2c2f, {0x2c5f}}, {0x2c60, {0x2c61}}, {0x2c62, {0x26b}}, {0x2c63, {0x1d7d}},
  {0x2c64, {0x27d}}, {0x2c67, {0x2c68}}, {0x2c69, {0x2c6a}}, {0x2c6b, {0x2c6c}}, {0x2c6d, {0x251}}, {0x2c6e, {0x271}},
  {0x2c6f, {0x250}}, {0x2c70, {0x252}}, {0x2c72, {0x2c73}}, {0x2c75, {0x2c76}}, {0x2c7e, {0x23f}}, {0x2c7f, {0x240}},
  {0x2c80, {0x2c81}}, {0x2c82, {0x2c83}}, {0x2c84, {0x2c85}}, {0x2c86, {0x2c87}}, {0x2c88, {0x2c89}}, {0x2c8a, {0x2c8b}},
  {0x2c8c, {0x2c8d}}, {0x2c8e, {0x2c8f}}, {0x2c90, {0x2c91}}, {0x2c92, {0x2c93}}, {0x2c94, {0x2c95}}, {0x2c96, {0x2c97}},
  {0x2c98, {0x2c99}}, {0x2c9a, {0x2c9b}}, {0x2c9c, {0x2c9d}}, {0x2c9e, {0x2c9f}}, {0x2ca0, {0x2ca1}}, {0x2ca2, {0x2ca3}},
  {0x2ca4, {0x2ca5}}, {0x2ca6, {0x2ca7}}, {0x2ca8, {0x2ca9}}, {0x2caa, {0x2cab}}, {0x2cac, {0x2cad}}, {0x2cae, {0x2caf}},
  {0x2cb0, {0x2cb1}}, {0x2cb2, {0x2cb3}}, {0x2cb4, {0x2cb5}}, {0x2cb6, {0x2cb7}}, {0x2cb8, {0x2cb9}}, {0x2cba, {0x2cbb}},
  {0x2cbc, {0x2cbd}}, {0x2cbe, {0x2cbf}}, {0x2cc0, {0x2cc1}}, {0x2cc2, {0x2cc3}}, {0x2cc4, {0x2cc5}}, {0x2cc6, {0x2cc7}},
  {0x2cc8, {0x2cc9}}, {0x2cca, {0x2ccb}}, {0x2ccc, {0x2ccd}}, {0x2cce, {0x2ccf}}, {0x2cd0, {0x2cd1}}, {0x2cd2, {0x2cd3}},
  {0x2cd4, {0x2cd5}}, {0x2cd6, {0x2cd7}}, {0x2cd8, {0x2cd9}}, {0x2cda, {0x2cdb}}, {0x2cdc, {0x2cdd}}, {0x2cde, {0x2cdf}},
  {0x2ce0, {0x2ce1}}, {0x2ce2, {0x2ce3}}, {0x2ceb, {0x2cec}}, {0x2ced, {0x2cee}}, {0x2cf2, {0x2cf3}}, {0xa640, {0xa641}},
  {0xa642, {0xa643}}, {0xa644, {0xa645}}, {0xa646, {0xa647}}, {0xa648, {0xa649}}, {0xa64a, {0xa64b}}, {0xa64c, {0xa64d}},
  {0xa64e, {0xa64f}}, {0xa650, {0xa651}}, {0xa652, {0xa653}}, {0xa654, {0xa655}}, {0xa656, {0xa657}}, {0xa658, {0xa659}},
  {0xa65a, {0xa65b}}, {0xa65c, {0xa65d}}, {0xa65e, {0xa65f}}, {0xa660, {0xa661}}, {0xa662, {0xa663}}, {0xa664, {0xa665}},
  {0xa666, {0xa667}}, {0xa668, {0xa669}}, {0xa66a, {0xa66b}}, {0xa66c, {0xa66d}}, {0xa680, {0xa681}}, {0xa682, {0xa683}},
  {0xa684, {0xa685}}, {0xa686, {0xa687}}, {0xa688, {0xa689}}, {0xa68a, {0xa68b}}, {0xa68c, {0xa68d}}, {0xa68e, {0xa68f}},
  {0xa690, {0xa691}}, {0xa692, {0xa693}}, {0xa694, {0xa695}}, {0xa696, {0xa697}}, {0xa698, {0xa699}}, {0xa69a, {0xa69b}},
  {0xa722, {0xa723}}, {0xa724, {0xa725}}, {0xa726, {0xa727}}, {0xa728, {0xa729}}, {0xa72a, {0xa72b}}, {0xa72c, {0xa72d}},
  {0xa72e, {0xa72f}}, {0xa732, {0xa733}}, {0xa734, {0xa735}}, {0xa736, {0xa737}}, {0xa738, {0xa739}}, {0xa73a, {0xa73b}},
  {0xa73c, {0xa73d}}, {0xa73e, {0xa73f}}, {0xa740, {0xa741}}, {0xa742, {0xa743}}, {0xa744, {0xa745}}, {0xa746, {0xa747}},
  {0xa748, {0xa749}}, {0xa74a, {0xa74b}}, {0xa74c, {0xa74d}}, {0xa74e, {0xa74f}}, {0xa750, {0xa751}}, {0xa752, {0xa753}},
  {0xa754, {0xa755}}, {0xa756, {0xa757}}, {0xa758, {0xa759}}, {0xa75a, {0xa75b}}, {0xa75c, {0xa75d}}, {0xa75e, {0xa75f}},
  {0xa760, {0xa761}}, {0xa762, {0xa763}}, {0xa764, {0xa765}}, {0xa766, {0xa767}}, {0xa768, {0xa769}}, {0xa76a, {0xa76b}},
  {0xa76c, {0xa76d}}, {0xa76e, {0xa76f}}, {0xa779, {0xa77a}}, {0xa77b, {0xa77c}}, {0xa77d, {0x1d79}}, {0xa77e, {0xa77f}},
  {0xa780, {0xa781}}, {0xa782, {0xa783}}, {0xa784, {0xa785}}, {0xa786, {0xa787}}, {0xa78b, {0xa78c}}, {0xa78d, {0x265}},
  {0xa790, {0xa791}}, {0xa792, {0xa793}}, {0xa796, {0xa797}}, {0xa798, {0xa799}}, {0xa79a, {0xa79b}}, {0xa79c, {0xa79d}},
  {0xa79e, {0xa79f}}, {0xa7a0, {0xa7a1}}, {0xa7a2, {0xa7a3}}, {0xa7a4, {0xa7a5}}, {0xa7a6, {0xa7a7}}, {0xa7a8, {0xa7a9}},
  {0xa7aa, {0x266}}, {0xa7ab, {0x25c}}, {0xa7ac, {0x261}}, {0xa7ad, {0x26c}}, {0xa7ae, {0x26a}}, {0xa7b0, {0x29e}},
  {0xa7b1, {0x287}}, {0xa7b2, {0x29d}}, {0xa7b3, {0xab53}}, {0xa7b4, {0xa7b5}}, {0xa7b6, {0xa7b7}}, {0xa7b8, {0xa7b9}},
  {0xa7ba, {0xa7bb}}, {0xa7bc, {0xa7bd}}, {0xa7be, {0xa7bf}}, {0xa7c0, {0xa7c1}}, {0xa7c2, {0xa7c3}}, {0xa7c4, {0xa794}},
  {0xa7c5, {0x282}}, {0xa7c6, {0x1d8e}}, {0xa7c7, {0xa7c8}}, {0xa7c9, {0xa7ca}}, {0xa7cb, {0x264}}, {0xa7cc, {0xa7cd}},
  {0xa7ce, {0xa7cf}}, {0xa7d0, {0xa7d1}}, {0xa7d2, {0xa7d3}}, {0xa7d4, {0xa7d5}}, {0xa7d6, {0xa7d7}}, {0xa7d8, {0xa7d9}},
  {0xa7da, {0xa7db}}, {0xa7dc, {0x19b}}, {0xa7f5, {0xa7f6}}, {0xff21, {0xff41}}, {0xff22, {0xff42}}, {0xff23, {0xff43}},
  {0xff24, {0xff44}}, {0xff25, {0xff45}}, {0xff26, {0xff46}}, {0xff27, {0xff47}}, {0xff28, {0xff48}}, {0xff29, {0xff49}},
  {0xff2a, {0xff4a}}, {0xff2b, {0xff4b}}, {0xff2c, {0xff4c}}, {0xff2d, {0xff4d}}, {0xff2e, {0xff4e}}, {0xff2f, {0xff4f}},
  {0xff30, {0xff50}}, {0xff31, {0xff51}}, {0xff32, {0xff52}}, {0xff33, {0xff53}}, {0xff34, {0xff54}}, {0xff35, {0xff55}},
  {0xff36, {0xff56}}, {0xff37, {0xff57}}, {0xff38, {0xff58}}, {0xff39, {0xff59}}, {0xff3a, {0xff5a}}, {0x10400, {0x10428}},
  {0x10401, {0x10429}}, {0x10402, {0x1042a}}, {0x10403, {0x1042b}}, {0x10404, {0x1042c}}, {0x10405, {0x1042d}}, {0x10406, {0x1042e}},
  {0x10407, {0x1042f}}, {0x10408, {0x10430}}, {0x10409, {0x10431}}, {0x1040a, {0x10432}}, {0x1040b, {0x10433}}, {0x1040c, {0x10434}},
  {0x1040d, {0x10435}}, {0x1040e, {0x10436}}, {0x1040f, {0x10437}}, {0x10410, {0x10438}}, {0x10411, {0x10439}}, {0x10412, {0x1043a}},
  {0x10413, {0x1043b}}, {0x10414, {0x1043c}}, {0x10415, {0x1043d}}, {0x10416, {0x1043e}}, {0x10417, {0x1043f}}, {0x10418, {0x10440}},
  {0x10419, {0x10441}}, {0x1041a, {0x10442}}, {0x1041b, {0x10443}}, {0x1041c, {0x10444}}, {0x1041d, {0x10445}}, {0x1041e, {0x10446}},
  {0x1041f, {0x10447}}, {0x10420, {0x10448}}, {0x10421, {0x10449}}, {0x10422, {0x1044a}}, {0x10423, {0x1044b}}, {0x10424, {0x1044c}},
  {0x10425, {0x1044d}}, {0x10426, {0x1044e}}, {0x10427, {0x1044f}}, {0x104b0, {0x104d8}}, {0x104b1, {0x104d9}}, {0x104b2, {0x104da}},
  {0x104b3, {0x104db}}, {0x104b4, {0x104dc}}, {0x104b5, {0x104dd}}, {0x104b6, {0x104de}}, {0x104b7, {0x104df}}, {0x104b8, {0x104e0}},
  {0x104b9, {0x104e1}}, {0x104ba, {0x104e2}}, {0x104bb, {0x104e3}}, {0x104bc, {0x104e4}}, {0x104bd, {0x104e5}}, {0x104be, {0x104e6}},
  {0x104bf, {0x104e7}}, {0x104c0, {0x104e8}}, {0x104c1, {0x104e9}}, {0x104c2, {0x104ea}}, {0x104c3, {0x104eb}}, {0x104c4, {0x104ec}},
  {0x104c5, {0x104ed}}, {0x104c6, {0x104ee}}, {0x104c7, {0x104ef}}, {0x104c8, {0x104f0}}, {0x104c9, {0x104f1}}, {0x104ca, {0x104f2}},
  {0x104cb, {0x104f3}}, {0x104cc, {0x104f4}}, {0x104cd, {0x104f5}}, {0x104ce, {0x104f6}}, {0x104cf, {0x104f7}}, {0x104d0, {0x104f8}},
  {0x104d1, {0x104f9}}, {0x104d2, {0x104fa}}, {0x104d3, {0x104fb}}, {0x10570, {0x10597}}, {0x10571, {0x10598}}, {0x10572, {0x10599}},
  {0x10573, {0x1059a}}, {0x10574, {0x1059b}}, {0x10575, {0x1059c}}, {0x10576, {0x1059d}}, {0x10577, {0x1059e}}, {0x10578, {0x1059f}},
  {0x10579, {0x105a0}}, {0x1057a, {0x105a1}}, {0x1057c, {0x105a3}}, {0x1057d, {0x105a4}}, {0x1057e, {0x105a5}}, {0x1057f, {0x105a6}},
  {0x10580, {0x105a7}}, {0x10581, {0x105a8}}, {0x10582, {0x105a9}}, {0x10583, {0x105aa}}, {0x10584, {0x105ab}}, {0x10585, {0x105ac}},
  {0x10586, {0x105ad}}, {0x10587, {0x105ae}}, {0x10588, {0x105af}}, {0x10589, {0x105b0}}, {0x1058a, {0x105b1}}, {0x1058c, {0x105b3}},
  {0x1058d, {0x105b4}}, {0x1058e, {0x105b5}}, {0x1058f, {0x105b6}}, {0x10590, {0x105b7}}, {0x10591, {0x105b8}}, {0x10592, {0x105b9}},
  {0x10594, {0x105bb}}, {0x10595, {0x105bc}}, {0x10c80, {0x10cc0}}, {0x10c81, {0x10cc1}}, {0x10c82, {0x10cc2}}, {0x10c83, {0x10cc3}},
  {0x10c84, {0x10cc4}}, {0x10c85, {0x10cc5}}, {0x10c86, {0x10cc6}}, {0x10c87, {0x10cc7}}, {0x10c88, {0x10cc8}}, {0x10c89, {0x10cc9}},
  {0x10c8a, {0x10cca}}, {0x10c8b, {0x10ccb}}, {0x10c8c, {0x10ccc}}, {0x10c8d, {0x10ccd}}, {0x10c8e, {0x10cce}}, {0x10c8f, {0x10ccf}},
  {0x10c90, {0x10cd0}}, {0x10c91, {0x10cd1}}, {0x10c92, {0x10cd2}}, {0x10c93, {0x10cd3}}, {0x10c94, {0x10cd4}}, {0x10c95, {0x10cd5}},
  {0x10c96, {0x10cd6}}, {0x10c97, {0x10cd7}}, {0x10c98, {0x10cd8}}, {0x10c99, {0x10cd9}}, {0x10c9a, {0x10cda}}, {0x10c9b, {0x10cdb}},
  {0x10c9c, {0x10cdc}}, {0x10c9d, {0x10cdd}}, {0x10c9e, {0x10cde}}, {0x10c9f, {0x10cdf}}, {0x10ca0, {0x10ce0}}, {0x10ca1, {0x10ce1}},
  {0x10ca2, {0x10ce2}}, {0x10ca3, {0x10ce3}}, {0x10ca4, {0x10ce4}}, {0x10ca5, {0x10ce5}}, {0x10ca6, {0x10ce6}}, {0x10ca7, {0x10ce7}},
  {0x10ca8, {0x10ce8}}, {0x10ca9, {0x10ce9}}, {0x10caa, {0x10cea}}, {0x10cab, {0x10ceb}}, {0x10cac, {0x10cec}}, {0x10cad, {0x10ced}},
  {0x10cae, {0x10cee}}, {0x10caf, {0x10cef}}, {0x10cb0, {0x10cf0}}, {0x10cb1, {0x10cf1}}, {0x10cb2, {0x10cf2}}, {0x10d50, {0x10d70}},
  {0x10d51, {0x10d71}}, {0x10d52, {0x10d72}}, {0x10d53, {0x10d73}}, {0x10d54, {0x10d74}}, {0x10d55, {0x10d75}}, {0x10d56, {0x10d76}},
  {0x10d57, {0x10d77}}, {0x10d58, {0x10d78}}, {0x10d59, {0x10d79}}, {0x10d5a, {0x10d7a}}, {0x10d5b, {0x10d7b}}, {0x10d5c, {0x10d7c}},
  {0x10d5d, {0x10d7d}}, {0x10d5e, {0x10d7e}}, {0x10d5f, {0x10d7f}}, {0x10d60, {0x10d80}}, {0x10d61, {0x10d81}}, {0x10d62, {0x10d82}},
  {0x10d63, {0x10d83}}, {0x10d64, {0x10d84}}, {0x10d65, {0x10d85}}, {0x118a0, {0x118c0}}, {0x118a1, {0x118c1}}, {0x118a2, {0x118c2}},
  {0x118a3, {0x118c3}}, {0x118a4, {0x118c4}}, {0x118a5, {0x118c5}}, {0x118a6, {0x118c6}}, {0x118a7, {0x118c7}}, {0x118a8, {0x118c8}},
  {0x118a9, {0x118c9}}, {0x118aa, {0x118ca}}, {0x118ab, {0x118cb}}, {0x118ac, {0x118cc}}, {0x118ad, {0x118cd}}, {0x118ae, {0x118ce}},
  {0x118af, {0x118cf}}, {0x118b0, {0x118d0}}, {0x118b1, {0x118d1}}, {0x118b2, {0x118d2}}, {0x118b3, {0x118d3}}, {0x118b4, {0x118d4}},
  {0x118b5, {0x118d5}}, {0x118b6, {0x118d6}}, {0x118b7, {0x118d7}}, {0x118b8, {0x118d8}}, {0x118b9, {0x118d9}}, {0x118ba, {0x118da}},
  {0x118bb, {0x118db}}, {0x118bc, {0x118dc}}, {0x118bd, {0x118dd}}, {0x118be, {0x118de}}, {0x118bf, {0x118df}}, {0x16e40, {0x16e60}},
  {0x16e41, {0x16e61}}, {0x16e42, {0x16e62}}, {0x16e43, {0x16e63}}, {0x16e44, {0x16e64}}, {0x16e45, {0x16e65}}, {0x16e46, {0x16e66}},
  {0x16e47, {0x16e67}}, {0x16e48, {0x16e68}}, {0x16e49, {0x16e69}}, {0x16e4a, {0x16e6a}}, {0x16e4b, {0x16e6b}}, {0x16e4c, {0x16e6c}},
  {0x16e4d, {0x16e6d}}, {0x16e4e, {0x16e6e}}, {0x16e4f, {0x16e6f}}, {0x16e50, {0x16e70}}, {0x16e51, {0x16e71}}, {0x16e52, {0x16e72}},
  {0x16e53, {0x16e73}}, {0x16e54, {0x16e74}}, {0x16e55, {0x16e75}}, {0x16e56, {0x16e76}}, {0x16e57, {0x16e77}}, {0x16e58, {0x16e78}},
  {0x16e59, {0x16e79}}, {0x16e5a, {0x16e7a}}, {0x16e5b, {0x16e7b}}, {0x16e5c, {0x16e7c}}, {0x16e5d, {0x16e7d}}, {0x16e5e, {0x16e7e}},
  {0x16e5f, {0x16e7f}}, {0x16ea0, {0x16ebb}}, {0x16ea1, {0x16ebc}}, {0x16ea2, {0x16ebd}}, {0x16ea3, {0x16ebe}}, {0x16ea4, {0x16ebf}},
  {0x16ea5, {0x16ec0}}, {0x16ea6, {0x16ec1}}, {0x16ea7, {0x16ec2}}, {0x16ea8, {0x16ec3}}, {0x16ea9, {0x16ec4}}, {0x16eaa, {0x16ec5}},
  {0x16eab, {0x16ec6}}, {0x16eac, {0x16ec7}}, {0x16ead, {0x16ec8}}, {0x16eae, {0x16ec9}}, {0x16eaf, {0x16eca}}, {0x16eb0, {0x16ecb}},
  {0x16eb1, {0x16ecc}}, {0x16eb2, {0x16ecd}}, {0x16eb3, {0x16ece}}, {0x16eb4, {0x16ecf}}, {0x16eb5, {0x16ed0}}, {0x16eb6, {0x16ed1}},
  {0x16eb7, {0x16ed2}}, {0x16eb8, {0x16ed3}}, {0x1e900, {0x1e922}}, {0x1e901, {0x1e923}}, {0x1e902, {0x1e924}}, {0x1e903, {0x1e925}},
  {0x1e904, {0x1e926}}, {0x1e905, {0x1e927}}, {0x1e906, {0x1e928}}, {0x1e907, {0x1e929}}, {0x1e908, {0x1e92a}}, {0x1e909, {0x1e92b}},
  {0x1e90a, {0x1e92c}}, {0x1e90b, {0x1e92d}}, {0x1e90c, {0x1e92e}}, {0x1e90d, {0x1e92f}}, {0x1e90e, {0x1e930}}, {0x1e90f, {0x1e931}},
  {0x1e910, {0x1e932}}, {0x1e911, {0x1e933}}, {0x1e912, {0x1e934}}, {0x1e913, {0x1e935}}, {0x1e914, {0x1e936}}, {0x1e915, {0x1e937}},
  {0x1e916, {0x1e938}}, {0x1e917, {0x1e939}}, {0x1e918, {0x1e93a}}, {0x1e919, {0x1e93b}}, {0x1e91a, {0x1e93c}}, {0x1e91b, {0x1e93d}},
  {0x1e91c, {0x1e93e}}, {0x1e91d, {0x1e93f}}, {0x1e91e, {0x1e940}}, {0x1e91f, {0x1e941}}, {0x1e920, {0x1e942}}, {0x1e921, {0x1e943}},
};
static const CaseMapping uppercase_mappings[] = {
  {0x61, {0x41}}, {0x62, {0x42}}, {0x63, {0x43}}, {0x64, {0x44}}, {0x65, {0x45}}, {0x66, {0x46}},
  {0x67, {0x47}}, {0x68, {0x48}}, {0x69, {0x49}}, {0x6a, {0x4a}}, {0x6b, {0x4b}}, {0x6c, {0x4c}},
  {0x6d, {0x4d}}, {0x6e, {0x4e}}, {0x6f, {0x4f}}, {0x70, {0x50}}, {0x71, {0x51}}, {0x72, {0x52}},
  {0x73, {0x53}}, {0x74, {0x54}}, {0x75, {0x55}}, {0x76, {0x56}}, {0x77, {0x57}}, {0x78, {0x58}},
  {0x79, {0x59}}, {0x7a, {0x5a}}, {0xb5, {0x39c}}, {0xdf, {0x53, 0x53}}, {0xe0, {0xc0}}, {0xe1, {0xc1}},
  {0xe2, {0xc2}}, {0xe3, {0xc3}}, {0xe4, {0xc4}}, {0xe5, {0xc5}}, {0xe6, {0xc6}}, {0xe7, {0xc7}},
  {0xe8, {0xc8}}, {0xe9, {0xc9}}, {0xea, {0xca}}, {0xeb, {0xcb}}, {0xec, {0xcc}}, {0xed, {0xcd}},
  {0xee, {0xce}}, {0xef, {0xcf}}, {0xf0, {0xd0}}, {0xf1, {0xd1}}, {0xf2, {0xd2}}, {0xf3, {0xd3}},
  {0xf4, {0xd4}}, {0xf5, {0xd5}}, {0xf6, {0xd6}}, {0xf8, {0xd8}}, {0xf9, {0xd9}}, {0xfa, {0xda}},
  {0xfb, {0xdb}}, {0xfc, {0xdc}}, {0xfd, {0xdd}}, {0xfe, {0xde}}, {0xff, {0x178}}, {0x101, {0x100}},
  {0x103, {0x102}}, {0x105, {0x104}}, {0x107, {0x106}}, {0x109, {0x108}}, {0x10b, {0x10a}}, {0x10d, {0x10c}},
  {0x10f, {0x10e}}, {0x111, {0x110}}, {0x113, {0x112}}, {0x115, {0x114}}, {0x117, {0x116}}, {0x119, {0x118}},
  {0x11b, {0x11a}}, {0x11d, {0x11c}}, {0x11f, {0x11e}}, {0x121, {0x120}}, {0x123, {0x122}}, {0x125, {0x124}},
  {0x127, {0x126}}, {0x129, {0x128}}, {0x12b, {0x12a}}, {0x12d, {0x12c}}, {0x12f, {0x12e}}, {0x131, {0x49}},
  {0x133, {0x132}}, {0x135, {0x134}}, {0x137, {0x136}}, {0x13a, {0x139}}, {0x13c, {0x13b}}, {0x13e, {0x13d}},
  {0x140, {0x13f}}, {0x142, {0x141}}, {0x144, {0x143}}, {0x146, {0x145}}, {0x148, {0x147}}, {0x149, {0x2bc, 0x4e}},
  {0x14b, {0x14a}}, {0x14d, {0x14c}}, {0x14f, {0x14e}}, {0x151, {0x150}}, {0x153, {0x152}}, {0x155, {0x154}},
  {0x157, {0x156}}, {0x159, {0x158}}, {0x15b, {0x15a}}, {0x15d, {0x15c}}, {0x15f, {0x15e}}, {0x161, {0x160}},
  {0x163, {0x162}}, {0x165, {0x164}}, {0x167, {0x166}}, {0x169, {0x168}}, {0x16b, {0x16a}}, {0x16d, {0x16c}},
  {0x16f, {0x16e}}, {0x171, {0x170}}, {0x173, {0x172}}, {0x175, {0x174}}, {0x177, {0x176}}, {0x17a, {0x179}},
  {0x17c, {0x17b}}, {0x17e, {0x17d}}, {0x17f, {0x53}}, {0x180, {0x243}}, {0x183, {0x182}}, {0x185, {0x184}},
  {0x188, {0x187}}, {0x18c, {0x18b}}, {0x192, {0x191}}, {0x195, {0x1f6}}, {0x199, {0x198}}, {0x19a, {0x23d}},
  {0x19b, {0xa7dc}}, {0x19e, {0x220}}, {0x1a1, {0x1a0}}, {0x1a3, {0x1a2}}, {0x1a5, {0x1a4}}, {0x1a8, {0x1a7}},
  {0x1ad, {0x1ac}}, {0x1b0, {0x1af}}, {0x1b4, {0x1b3}}, {0x1b6, {0x1b5}}, {0x1b9, {0x1b8}}, {0x1bd, {0x1bc}},
  {0x1bf, {0x1f7}}, {0x1c5, {0x1c4}}, {0x1c6, {0x1c4}}, {0x1c8, {0x1c7}}, {0x1c9, {0x1c7}}, {0x1cb, {0x1ca}},
  {0x1cc, {0x1ca}}, {0x1ce, {0x1cd}}, {0x1d0, {0x1cf}}, {0x1d2, {0x1d1}}, {0x1d4, {0x1d3}}, {0x1d6, {0x1d5}},
  {0x1d8, {0x1d7}}, {0x1da, {0x1d9}}, {0x1dc, {0x1db}}, {0x1dd, {0x18e}}, {0x1df, {0x1de}}, {0x1e1, {0x1e0}},
  {0x1e3, {0x1e2}}, {0x1e5, {0x1e4}}, {0x1e7, {0x1e6}}, {0x1e9, {0x1e8}}, {0x1eb, {0x1ea}}, {0x1ed, {0x1ec}},
  {0x1ef, {0x1ee}}, {0x1f0, {0x4a, 0x30c}}, {0x1f2, {0x1f1}}, {0x1f3, {0x1f1}}, {0x1f5, {0x1f4}}, {0x1f9, {0x1f8}},
  {0x1fb, {0x1fa}}, {0x1fd, {0x1fc}}, {0x1ff, {0x1fe}}, {0x201, {0x200}}, {0x203, {0x202}}, {0x205, {0x204}},
  {0x207, {0x206}}, {0x209, {0x208}}, {0x20b, {0x20a}}, {0x20d, {0x20c}}, {0x20f, {0x20e}}, {0x211, {0x210}},
  {0x213, {0x212}}, {0x215, {0x214}}, {0x217, {0x216}}, {0x219, {0x218}}, {0x21b, {0x21a}}, {0x21d, {0x21c}},
  {0x21f, {0x21e}}, {0x223, {0x222}}, {0x225, {0x224}}, {0x227, {0x226}}, {0x229, {0x228}}, {0x22b, {0x22a}},
  {0x22d, {0x22c}}, {0x22f, {0x22e}}, {0x231, {0x230}}, {0x233, {0x232}}, {0x23c, {0x23b}}, {0x23f, {0x2c7e}},
  {0x240, {0x2c7f}}, {0x242, {0x241}}, {0x247, {0x246}}, {0x249, {0x248}}, {0x24b, {0x24a}}, {0x24d, {0x24c}},
  {0x24f, {0x24e}}, {0x250, {0x2c6f}}, {0x251, {0x2c6d}}, {0x252, {0x2c70}}, {0x253, {0x181}}, {0x254, {0x186}},
  {0x256, {0x189}}, {0x257, {0x18a}}, {0x259, {0x18f}}, {0x25b, {0x190}}, {0x25c, {0xa7ab}}, {0x260, {0x193}},
  {0x261, {0xa7ac}}, {0x263, {0x194}}, {0x264, {0xa7cb}}, {0x265, {0xa78d}}, {0x266, {0xa7aa}}, {0x268, {0x197}},
  {0x269, {0x196}}, {0x26a, {0xa7ae}}, {0x26b, {0x2c62}}, {0x26c, {0xa7ad}}, {0x26f, {0x19c}}, {0x271, {0x2c6e}},
  {0x272, {0x19d}}, {0x275, {0x19f}}, {0x27d, {0x2c64}}, {0x280, {0x1a6}}, {0x282, {0xa7c5}}, {0x283, {0x1a9}},
  {0x287, {0xa7b1}}, {0x288, {0x1ae}}, {0x289, {0x244}}, {0x28a, {0x1b1}}, {0x28b, {0x1b2}}, {0x28c, {0x245}},
  {0x292, {0x1b7}}, {0x29d, {0xa7b2}}, {0x29e, {0xa7b0}}, {0x345, {0x399}}, {0x371, {0x370}}, {0x373, {0x372}},
  {0x377, {0x376}}, {0x37b, {0x3fd}}, {0x37c, {0x3fe}}, {0x37d, {0x3ff}}, {0x390, {0x399, 0x308, 0x301}}, {0x3ac, {0x386}},
  {0x3ad, {0x388}}, {0x3ae, {0x389}}, {0x3af, {0x38a}}, {0x3b0, {0x3a5, 0x308, 0x301}}, {0x3b1, {0x391}}, {0x3b2, {0x392}},
  {0x3b3, {0x393}}, {0x3b4, {0x394}}, {0x3b5, {0x395}}, {0x3b6, {0x396}}, {0x3b7, {0x397}}, {0x3b8, {0x398}},
  {0x3b9, {0x399}}, {0x3ba, {0x39a}}, {0x3bb, {0x39b}}, {0x3bc, {0x39c}}, {0x3bd, {0x39d}}, {0x3be, {0x39e}},
  {0x3bf, {0x39f}}, {0x3c0, {0x3a0}}, {0x3c1, {0x3a1}}, {0x3c2, {0x3a3}}, {0x3c3, {0x3a3}}, {0x3c4, {0x3a4}},
  {0x3c5, {0x3a5}}, {0x3c6, {0x3a6}}, {0x3c7, {0x3a7}}, {0x3c8, {0x3a8}}, {0x3c9, {0x3a9}}, {0x3ca, {0x3aa}},
  {0x3cb, {0x3ab}}, {0x3cc, {0x38c}}, {0x3cd, {0x38e}}, {0x3ce, {0x38f}}, {0x3d0, {0x392}}, {0x3d1, {0x398}},
  {0x3d5, {0x3a6}}, {0x3d6, {0x3a0}}, {0x3d7, {0x3cf}}, {0x3d9, {0x3d8}}, {0x3db, {0x3da}}, {0x3dd, {0x3dc}},
  {0x3df, {0x3de}}, {0x3e1, {0x3e0}}, {0x3e3, {0x3e2}}, {0x3e5, {0x3e4}}, {0x3e7, {0x3e6}}, {0x3e9, {0x3e8}},
  {0x3eb, {0x3ea}}, {0x3ed, {0x3ec}}, {0x3ef, {0x3ee}}, {0x3f0, {0x39a}}, {0x3f1, {0x3a1}}, {0x3f2, {0x3f9}},
  {0x3f3, {0x37f}}, {0x3f5, {0x395}}, {0x3f8, {0x3f7}}, {0x3fb, {0x3fa}}, {0x430, {0x410}}, {0x431, {0x411}},
  {0x432, {0x412}}, {0x433, {0x413}}, {0x434, {0x414}}, {0x435, {0x415}}, {0x436, {0x416}}, {0x437, {0x417}},
  {0x438, {0x418}}, {0x439, {0x419}}, {0x43a, {0x41a}}, {0x43b, {0x41b}}, {0x43c, {0x41c}}, {0x43d, {0x41d}},
  {0x43e, {0x41e}}, {0x43f, {0x41f}}, {0x440, {0x420}}, {0x441, {0x421}}, {0x442, {0x422}}, {0x443, {0x423}},
  {0x444, {0x424}}, {0x445, {0x425}}, {0x446, {0x426}}, {0x447, {0x427}}, {0x448, {0x428}}, {0x449, {0x429}},
  {0x44a, {0x42a}}, {0x44b, {0x42b}}, {0x44c, {0x42c}}, {0x44d, {0x42d}}, {0x44e, {0x42e}}, {0x44f, {0x42f}},
  {0x450, {0x400}}, {0x451, {0x401}}, {0x452, {0x402}}, {0x453, {0x403}}, {0x454, {0x404}}, {0x455, {0x405}},
  {0x456, {0x406}}, {0x457, {0x407}}, {0x458, {0x408}}, {0x459, {0x409}}, {0x45a, {0x40a}}, {0x45b, {0x40b}},
  {0x45c, {0x40c}}, {0x45d, {0x40d}}, {0x45e, {0x40e}}, {0x45f, {0x40f}}, {0x461, {0x460}}, {0x463, {0x462}},
  {0x465, {0x464}}, {0x467, {0x466}}, {0x469, {0x468}}, {0x46b, {0x46a}}, {0x46d, {0x46c}}, {0x46f, {0x46e}},
  {0x471, {0x470}}, {0x473, {0x472}}, {0x475, {0x474}}, {0x477, {0x476}}, {0x479, {0x478}}, {0x47b, {0x47a}},
  {0x47d, {0x47c}}, {0x47f, {0x47e}}, {0x481, {0x480}}, {0x48b, {0x48a}}, {0x48d, {0x48c}}, {0x48f, {0x48e}},
  {0x491, {0x490}}, {0x493, {0x492}}, {0x495, {0x494}}, {0x497, {0x496}}, {0x499, {0x498}}, {0x49b, {0x49a}},
  {0x49d, {0x49c}}, {0x49f, {0x49e}}, {0x4a1, {0x4a0}}, {0x4a3, {0x4a2}}, {0x4a5, {0x4a4}}, {0x4a7, {0x4a6}},
  {0x4a9, {0x4a8}}, {0x4ab, {0x4aa}}, {0x4ad, {0x4ac}}, {0x4af, {0x4ae}}, {0x4b1, {0x4b0}}, {0x4b3, {0x4b2}},
  {0x4b5, {0x4b4}}, {0x4b7, {0x4b6}}, {0x4b9, {0x4b8}}, {0x4bb, {0x4ba}}, {0x4bd, {0x4bc}}, {0x4bf, {0x4be}},
  {0x4c2, {0x4c1}}, {0x4c4, {0x4c3}}, {0x4c6, {0x4c5}}, {0x4c8, {0x4c7}}, {0x4ca, {0x4c9}}, {0x4cc, {0x4cb}},
  {0x4ce, {0x4cd}}, {0x4cf, {0x4c0}}, {0x4d1, {0x4d0}}, {0x4d3, {0x4d2}}, {0x4d5, {0x4d4}}, {0x4d7, {0x4d6}},
  {0x4d9, {0x4d8}}, {0x4db, {0x4da}}, {0x4dd, {0x4dc}}, {0x4df, {0x4de}}, {0x4e1, {0x4e0}}, {0x4e3, {0x4e2}},
  {0x4e5, {0x4e4}}, {0x4e7, {0x4e6}}, {0x4e9, {0x4e8}}, {0x4eb, {0x4ea}}, {0x4ed, {0x4ec}}, {0x4ef, {0x4ee}},
  {0x4f1, {0x4f0}}, {0x4f3, {0x4f2}}, {0x4f5, {0x4f4}}, {0x4f7, {0x4f6}}, {0x4f9, {0x4f8}}, {0x4fb, {0x4fa}},
  {0x4fd, {0x4fc}}, {0x4ff, {0x4fe}}, {0x501, {0x500}}, {0x503, {0x502}}, {0x505, {0x504}}, {0x507, {0x506}},
  {0x509, {0x508}}, {0x50b, {0x50a}}, {0x50d, {0x50c}}, {0x50f, {0x50e}}, {0x511, {0x510}}, {0x513, {0x512}},
  {0x515, {0x514}}, {0x517, {0x516}}, {0x519, {0x518}}, {0x51b, {0x51a}}, {0x51d, {0x51c}}, {0x51f, {0x51e}},
  {0x521, {0x520}}, {0x523, {0x522}}, {0x525, {0x524}}, {0x527, {0x526}}, {0x529, {0x528}}, {0x52b, {0x52a}},
  {0x52d, {0x52c}}, {0x52f, {0x52e}}, {0x561, {0x531}}, {0x562, {0x532}}, {0x563, {0x533}}, {0x564, {0x534}},
  {0x565, {0x535}}, {0x566, {0x536}}, {0x567, {0x537}}, {0x568, {0x538}}, {0x569, {0x539}}, {0x56a, {0x53a}},
  {0x56b, {0x53b}}, {0x56c, {0x53c}}, {0x56d, {0x53d}}, {0x56e, {0x53e}}, {0x56f, {0x53f}}, {0x570, {0x540}},
  {0x571, {0x541}}, {0x572, {0x542}}, {0x573, {0x543}}, {0x574, {0x544}}, {0x575, {0x545}}, {0x576, {0x546}},
  {0x577, {0x547}}, {0x578, {0x548}}, {0x579, {0x549}}, {0x57a, {0x54a}}, {0x57b, {0x54b}}, {0x57c, {0x54c}},
  {0x57d, {0x54d}}, {0x57e, {0x54e}}, {0x57f, {0x54f}}, {0x580, {0x550}}, {0x581, {0x551}}, {0x582, {0x552}},
  {0x583, {0x553}}, {0x584, {0x554}}, {0x585, {0x555}}, {0x586, {0x556}}, {0x587, {0x535, 0x552}}, {0x10d0, {0x1c90}},
  {0x10d1, {0x1c91}}, {0x10d2, {0x1c92}}, {0x10d3, {0x1c93}}, {0x10d4, {0x1c94}}, {0x10d5, {0x1c95}}, {0x10d6, {0x1c96}},
  {0x10d7, {0x1c97}}, {0x10d8, {0x1c98}}, {0x10d9, {0x1c99}}, {0x10da, {0x1c9a}}, {0x10db, {0x1c9b}}, {0x10dc, {0x1c9c}},
  {0x10dd, {0x1c9d}}, {0x10de, {0x1c9e}}, {0x10df, {0x1c9f}}, {0x10e0, {0x1ca0}}, {0x10e1, {0x1ca1}}, {0x10e2, {0x1ca2}},
  {0x10e3, {0x1ca3}}, {0x10e4, {0x1ca4}}, {0x10e5, {0x1ca5}}, {0x10e6, {0x1ca6}}, {0x10e7, {0x1ca7}}, {0x10e8, {0x1ca8}},
  {0x10e9, {0x1ca9}}, {0x10ea, {0x1caa}}, {0x10eb, {0x1cab}}, {0x10ec, {0x1cac}}, {0x10ed, {0x1cad}}, {0x10ee, {0x1cae}},
  {0x10ef, {0x1caf}}, {0x10f0, {0x1cb0}}, {0x10f1, {0x1cb1}}, {0x10f2, {0x1cb2}}, {0x10f3, {0x1cb3}}, {0x10f4, {0x1cb4}},
  {0x10f5, {0x1cb5}}, {0x10f6, {0x1cb6}}, {0x10f7, {0x1cb7}}, {0x10f8, {0x1cb8}}, {0x10f9, {0x1cb9}}, {0x10fa, {0x1cba}},
  {0x10fd, {0x1cbd}}, {0x10fe, {0x1cbe}}, {0x10ff, {0x1cbf}}, {0x13f8, {0x13f0}}, {0x13f9, {0x13f1}}, {0x13fa, {0x13f2}},
  {0x13fb, {0x13f3}}, {0x13fc, {0x13f4}}, {0x13fd, {0x13f5}}, {0x1c80, {0x412}}, {0x1c81, {0x414}}, {0x1c82, {0x41e}},
  {0x1c83, {0x421}}, {0x1c84, {0x422}}, {0x1c85, {0x422}}, {0x1c86, {0x42a}}, {0x1c87, {0x462}}, {0x1c88, {0xa64a}},
  {0x1c8a, {0x1c89}}, {0x1d79, {0xa77d}}, {0x1d7d, {0x2c63}}, {0x1d8e, {0xa7c6}}, {0x1e01, {0x1e00}}, {0x1e03, {0x1e02}},
  {0x1e05, {0x1e04}}, {0x1e07, {0x1e06}}, {0x1e09, {0x1e08}}, {0x1e0b, {0x1e0a}}, {0x1e0d, {0x1e0c}}, {0x1e0f, {0x1e0e}},
  {0x1e11, {0x1e10}}, {0x1e13, {0x1e12}}, {0x1e15, {0x1e14}}, {0x1e17, {0x1e16}}, {0x1e19, {0x1e18}}, {0x1e1b, {0x1e1a}},
  {0x1e1d, {0x1e1c}}, {0x1e1f, {0x1e1e}}, {0x1e21, {0x1e20}}, {0x1e23, {0x1e22}}, {0x1e25, {0x1e24}}, {0x1e27, {0x1e26}},
  {0x1e29, {0x1e28}}, {0x1e2b, {0x1e2a}}, {0x1e2d, {0x1e2c}}, {0x1e2f, {0x1e2e}}, {0x1e31, {0x1e30}}, {0x1e33, {0x1e32}},
  {0x1e35, {0x1e34}}, {0x1e37, {0x1e36}}, {0x1e39, {0x1e38}}, {0x1e3b, {0x1e3a}}, {0x1e3d, {0x1e3c}}, {0x1e3f, {0x1e3e}},
  {0x1e41, {0x1e40}}, {0x1e43, {0x1e42}}, {0x1e45, {0x1e44}}, {0x1e47, {0x1e46}}, {0x1e49, {0x1e48}}, {0x1e4b, {0x1e4a}},
  {0x1e4d, {0x1e4c}}, {0x1e4f, {0x1e4e}}, {0x1e51, {0x1e50}}, {0x1e53, {0x1e52}}, {0x1e55, {0x1e54}}, {0x1e57, {0x1e56}},
  {0x1e59, {0x1e58}}, {0x1e5b, {0x1e5a}}, {0x1e5d, {0x1e5c}}, {0x1e5f, {0x1e5e}}, {0x1e61, {0x1e60}}, {0x1e63, {0x1e62}},
  {0x1e65, {0x1e64}}, {0x1e67, {0x1e66}}, {0x1e69, {0x1e68}}, {0x1e6b, {0x1e6a}}, {0x1e6d, {0x1e6c}}, {0x1e6f, {0x1e6e}},
  {0x1e71, {0x1e70}}, {0x1e73, {0x1e72}}, {0x1e75, {0x1e74}}, {0x1e77, {0x1e76}}, {0x1e79, {0x1e78}}, {0x1e7b, {0x1e7a}},
  {0x1e7d, {0x1e7c}}, {0x1e7f, {0x1e7e}}, {0x1e81, {0x1e80}}, {0x1e83, {0x1e82}}, {0x1e85, {0x1e84}}, {0x1e87, {0x1e86}},
  {0x1e89, {0x1e88}}, {0x1e8b, {0x1e8a}}, {0x1e8d, {0x1e8c}}, {0x1e8f, {0x1e8e}}, {0x1e91, {0x1e90}}, {0x1e93, {0x1e92}},
  {0x1e95, {0x1e94}}, {0x1e96, {0x48, 0x331}}, {0x1e97, {0x54, 0x308}}, {0x1e98, {0x57, 0x30a}}, {0x1e99, {0x59, 0x30a}}, {0x1e9a, {0x41, 0x2be}},
  {0x1e9b, {0x1e60}}, {0x1ea1, {0x1ea0}}, {0x1ea3, {0x1ea2}}, {0x1ea5, {0x1ea4}}, {0x1ea7, {0x1ea6}}, {0x1ea9, {0x1ea8}},
  {0x1eab, {0x1eaa}}, {0x1ead, {0x1eac}}, {0x1eaf, {0x1eae}}, {0x1eb1, {0x1eb0}}, {0x1eb3, {0x1eb2}}, {0x1eb5, {0x1eb4}},
  {0x1eb7, {0x1eb6}}, {0x1eb9, {0x1eb8}}, {0x1ebb, {0x1eba}}, {0x1ebd, {0x1ebc}}, {0x1ebf, {0x1ebe}}, {0x1ec1, {0x1ec0}},
  {0x1ec3, {0x1ec2}}, {0x1ec5, {0x1ec4}}, {0x1ec7, {0x1ec6}}, {0x1ec9, {0x1ec8}}, {0x1ecb, {0x1eca}}, {0x1ecd, {0x1ecc}},
  {0x1ecf, {0x1ece}}, {0x1ed1, {0x1ed0}}, {0x1ed3, {0x1ed2}}, {0x1ed5, {0x1ed4}}, {0x1ed7, {0x1ed6}}, {0x1ed9, {0x1ed8}},
  {0x1edb, {0x1eda}}, {0x1edd, {0x1edc}}, {0x1edf, {0x1ede}}, {0x1ee1, {0x1ee0}}, {0x1ee3, {0x1ee2}}, {0x1ee5, {0x1ee4}},
  {0x1ee7, {0x1ee6}}, {0x1ee9, {0x1ee8}}, {0x1eeb, {0x1eea}}, {0x1eed, {0x1eec}}, {0x1eef, {0x1eee}}, {0x1ef1, {0x1ef0}},
  {0x1ef3, {0x1ef2}}, {0x1ef5, {0x1ef4}}, {0x1ef7, {0x1ef6}}, {0x1ef9, {0x1ef8}}, {0x1efb, {0x1efa}}, {0x1efd, {0x1efc}},
  {0x1eff, {0x1efe}}, {0x1f00, {0x1f08}}, {0x1f01, {0x1f09}}, {0x1f02, {0x1f0a}}, {0x1f03, {0x1f0b}}, {0x1f04, {0x1f0c}},
  {0x1f05, {0x1f0d}}, {0x1f06, {0x1f0e}}, {0x1f07, {0x1f0f}}, {0x1f10, {0x1f18}}, {0x1f11, {0x1f19}}, {0x1f12, {0x1f1a}},
  {0x1f13, {0x1f1b}}, {0x1f14, {0x1f1c}}, {0x1f15, {0x1f1d}}, {0x1f20, {0x1f28}}, {0x1f21, {0x1f29}}, {0x1f22, {0x1f2a}},
  {0x1f23, {0x1f2b}}, {0x1f24, {0x1f2c}}, {0x1f25, {0x1f2d}}, {0x1f26, {0x1f2e}}, {0x1f27, {0x1f2f}}, {0x1f30, {0x1f38}},
  {0x1f31, {0x1f39}}, {0x1f32, {0x1f3a}}, {0x1f33, {0x1f3b}}, {0x1f34, {0x1f3c}}, {0x1f35, {0x1f3d}}, {0x1f36, {0x1f3e}},
  {0x1f37, {0x1f3f}}, {0x1f40, {0x1f48}}, {0x1f41, {0x1f49}}, {0x1f42, {0x1f4a}}, {0x1f43, {0x1f4b}}, {0x1f44, {0x1f4c}},
  {0x1f45, {0x1f4d}}, {0x1f50, {0x3a5, 0x313}}, {0x1f51, {0x1f59}}, {0x1f52, {0x3a5, 0x313, 0x300}}, {0x1f53, {0x1f5b}}, {0x1f54, {0x3a5, 0x313, 0x301}},
  {0x1f55, {0x1f5d}}, {0x1f56, {0x3a5, 0x313, 0x342}}, {0x1f57, {0x1f5f}}, {0x1f60, {0x1f68}}, {0x1f61, {0x1f69}}, {0x1f62, {0x1f6a}},
  {0x1f63, {0x1f6b}}, {0x1f64, {0x1f6c}}, {0x1f65, {0x1f6d}}, {0x1f66, {0x1f6e}}, {0x1f67, {0x1f6f}}, {0x1f70, {0x1fba}},
  {0x1f71, {0x1fbb}}, {0x1f72, {0x1fc8}}, {0x1f73, {0x1fc9}}, {0x1f74, {0x1fca}}, {0x1f75, {0x1fcb}}, {0x1f76, {0x1fda}},
  {0x1f77, {0x1fdb}}, {0x1f78, {0x1ff8}}, {0x1f79, {0x1ff9}}, {0x1f7a, {0x1fea}}, {0x1f7b, {0x1feb}}, {0x1f7c, {0x1ffa}},
  {0x1f7d, {0x1ffb}}, {0x1f80, {0x1f08, 0x399}}, {0x1f81, {0x1f09, 0x399}}, {0x1f82, {0x1f0a, 0x399}}, {0x1f83, {0x1f0b, 0x399}}, {0x1f84, {0x1f0c, 0x399}},
  {0x1f85, {0x1f0d, 0x399}}, {0x1f86, {0x1f0e, 0x399}}, {0x1f87, {0x1f0f, 0x399}}, {0x1f88, {0x1f08, 0x399}}, {0x1f89, {0x1f09, 0x399}}, {0x1f8a, {0x1f0a, 0x399}},
  {0x1f8b, {0x1f0b, 0x399}}, {0x1f8c, {0x1f0c, 0x399}}, {0x1f8d, {0x1f0d, 0x399}}, {0x1f8e, {0x1f0e, 0x399}}, {0x1f8f, {0x1f0f, 0x399}}, {0x1f90, {0x1f28, 0x399}},
  {0x1f91, {0x1f29, 0x399}}, {0x1f92, {0x1f2a, 0x399}}, {0x1f93, {0x1f2b, 0x399}}, {0x1f94, {0x1f2c, 0x399}}, {0x1f95, {0x1f2d, 0x399}}, {0x1f96, {0x1f2e, 0x399}},
  {0x1f97, {0x1f2f, 0x399}}, {0x1f98, {0x1f28, 0x399}}, {0x1f99, {0x1f29, 0x399}}, {0x1f9a, {0x1f2a, 0x399}}, {0x1f9b, {0x1f2b, 0x399}}, {0x1f9c, {0x1f2c, 0x399}},
  {0x1f9d, {0x1f2d, 0x399}}, {0x1f9e, {0x1f2e, 0x399}}, {0x1f9f, {0x1f2f, 0x399}}, {0x1fa0, {0x1f68, 0x399}}, {0x1fa1, {0x1f69, 0x399}}, {0x1fa2, {0x1f6a, 0x399}},
  {0x1fa3, {0x1f6b, 0x399}}, {0x1fa4, {0x1f6c, 0x399}}, {0x1fa5, {0x1f6d, 0x399}}, {0x1fa6, {0x1f6e, 0x399}}, {0x1fa7, {0x1f6f, 0x399}}, {0x1fa8, {0x1f68, 0x399}},
  {0x1fa9, {0x1f69, 0x399}}, {0x1faa, {0x1f6a, 0x399}}, {0x1fab, {0x1f6b, 0x399}}, {0x1fac, {0x1f6c, 0x399}}, {0x1fad, {0x1f6d, 0x399}}, {0x1fae, {0x1f6e, 0x399}},
  {0x1faf, {0x1f6f, 0x399}}, {0x1fb0, {0x1fb8}}, {0x1fb1, {0x1fb9}}, {0x1fb2, {0x1fba, 0x399}}, {0x1fb3, {0x391, 0x399}}, {0x1fb4, {0x386, 0x399}},
  {0x1fb6, {0x391, 0x342}}, {0x1fb7, {0x391, 0x342, 0x399}}, {0x1fbc, {0x391, 0x399}}, {0x1fbe, {0x399}}, {0x1fc2, {0x1fca, 0x399}}, {0x1fc3, {0x397, 0x399}},
  {0x1fc4, {0x389, 0x399}}, {0x1fc6, {0x397, 0x342}}, {0x1fc7, {0x397, 0x342, 0x399}}, {0x1fcc, {0x397, 0x399}}, {0x1fd0, {0x1fd8}}, {0x1fd1, {0x1fd9}},
  {0x1fd2, {0x399, 0x308, 0x300}}, {0x1fd3, {0x399, 0x308, 0x301}}, {0x1fd6, {0x399, 0x342}}, {0x1fd7, {0x399, 0x308, 0x342}}, {0x1fe0, {0x1fe8}}, {0x1fe1, {0x1fe9}},
  {0x1fe2, {0x3a5, 0x308, 0x300}}, {0x1fe3, {0x3a5, 0x308, 0x301}}, {0x1fe4, {0x3a1, 0x313}}, {0x1fe5, {0x1fec}}, {0x1fe6, {0x3a5, 0x342}}, {0x1fe7, {0x3a5, 0x308, 0x342}},
  {0x1ff2, {0x1ffa, 0x399}}, {0x1ff3, {0x3a9, 0x399}}, {0x1ff4, {0x38f, 0x399}}, {0x1ff6, {0x3a9, 0x342}}, {0x1ff7, {0x3a9, 0x342, 0x399}}, {0x1ffc, {0x3a9, 0x399}},
  {0x214e, {0x2132}}, {0x2170, {0x2160}}, {0x2171, {0x2161}}, {0x2172, {0x2162}}, {0x2173, {0x2163}}, {0x2174, {0x2164}},
  {0x2175, {0x2165}}, {0x2176, {0x2166}}, {0x2177, {0x2167}}, {0x2178, {0x2168}}, {0x2179, {0x2169}}, {0x217a, {0x216a}},
  {0x217b, {0x216b}}, {0x217c, {0x216c}}, {0x217d, {0x216d}}, {0x217e, {0x216e}}, {0x217f, {0x216f}}, {0x2184, {0x2183}},
  {0x24d0, {0x24b6}}, {0x24d1, {0x24b7}}, {0x24d2, {0x24b8}}, {0x24d3, {0x24b9}}, {0x24d4, {0x24ba}}, {0x24d5, {0x24bb}},
  {0x24d6, {0x24bc}}, {0x24d7, {0x24bd}}, {0x24d8, {0x24be}}, {0x24d9, {0x24bf}}, {0x24da, {0x24c0}}, {0x24db, {0x24c1}},
  {0x24dc, {0x24c2}}, {0x24dd, {0x24c3}}, {0x24de, {0x24c4}}, {0x24df, {0x24c5}}, {0x24e0, {0x24c6}}, {0x24e1, {0x24c7}},
  {0x24e2, {0x24c8}}, {0x24e3, {0x24c9}}, {0x24e4, {0x24ca}}, {0x24e5, {0x24cb}}, {0x24e6, {0x24cc}}, {0x24e7, {0x24cd}},
  {0x24e8, {0x24ce}}, {0x24e9, {0x24cf}}, {0x2c30, {0x2c00}}, {0x2c31, {0x2c01}}, {0x2c32, {0x2c02}}, {0x2c33, {0x2c03}},
  {0x2c34, {0x2c04}}, {0x2c35, {0x2c05}}, {0x2c36, {0x2c06}}, {0x2c37, {0x2c07}}, {0x2c38, {0x2c08}}, {0x2c39, {0x2c09}},
  {0x2c3a, {0x2c0a}}, {0x2c3b, {0x2c0b}}, {0x2c3c, {0x2c0c}}, {0x2c3d, {0x2c0d}}, {0x2c3e, {0x2c0e}}, {0x2c3f, {0x2c0f}},
  {0x2c40, {0x2c10}}, {0x2c41, {0x2c11}}, {0x2c42, {0x2c12}}, {0x2c43, {0x2c13}}, {0x2c44, {0x2c14}}, {0x2c45, {0x2c15}},
  {0x2c46, {0x2c16}}, {0x2c47, {0x2c17}}, {0x2c48, {0x2c18}}, {0x2c49, {0x2c19}}, {0x2c4a, {0x2c1a}}, {0x2c4b, {0x2c1b}},
  {0x2c4c, {0x2c1c}}, {0x2c4d, {0x2c1d}}, {0x2c4e, {0x2c1e}}, {0x2c4f, {0x2c1f}}, {0x2c50, {0x2c20}}, {0x2c51, {0x2c21}},
  {0x2c52, {0x2c22}}, {0x2c53, {0x2c23}}, {0x2c54, {0x2c24}}, {0x2c55, {0x2c25}}, {0x2c56, {0x2c26}}, {0x2c57, {0x2c27}},
  {0x2c58, {0x2c28}}, {0x2c59, {0x2c29}}, {0x2c5a, {0x2c2a}}, {0x2c5b, {0x2c2b}}, {0x2c5c, {0x2c2c}}, {0x2c5d, {0x2c2d}},
  {0x2c5e, {0x2c2e}}, {0x2c5f, {0x2c2f}}, {0x2c61, {0x2c60}}, {0x2c65, {0x23a}}, {0x2c66, {0x23e}}, {0x2c68, {0x2c67}},
  {0x2c6a, {0x2c69}}, {0x2c6c, {0x2c6b}}, {0x2c73, {0x2c72}}, {0x2c76, {0x2c75}}, {0x2c81, {0x2c80}}, {0x2c83, {0x2c82}},
  {0x2c85, {0x2c84}}, {0x2c87, {0x2c86}}, {0x2c89, {0x2c88}}, {0x2c8b, {0x2c8a}}, {0x2c8d, {0x2c8c}}, {0x2c8f, {0x2c8e}},
  {0x2c91, {0x2c90}}, {0x2c93, {0x2c92}}, {0x2c95, {0x2c94}}, {0x2c97, {0x2c96}}, {0x2c99, {0x2c98}}, {0x2c9b, {0x2c9a}},
  {0x2c9d, {0x2c9c}}, {0x2c9f, {0x2c9e}}, {0x2ca1, {0x2ca0}}, {0x2ca3, {0x2ca2}}, {0x2ca5, {0x2ca4}}, {0x2ca7, {0x2ca6}},
  {0x2ca9, {0x2ca8}}, {0x2cab, {0x2caa}}, {0x2cad, {0x2cac}}, {0x2caf, {0x2cae}}, {0x2cb1, {0x2cb0}}, {0x2cb3, {0x2cb2}},
  {0x2cb5, {0x2cb4}}, {0x2cb7, {0x2cb6}}, {0x2cb9, {0x2cb8}}, {0x2cbb, {0x2cba}}, {0x2cbd, {0x2cbc}}, {0x2cbf, {0x2cbe}},
  {0x2cc1, {0x2cc0}}, {0x2cc3, {0x2cc2}}, {0x2cc5, {0x2cc4}}, {0x2cc7, {0x2cc6}}, {0x2cc9, {0x2cc8}}, {0x2ccb, {0x2cca}},
  {0x2ccd, {0x2ccc}}, {0x2ccf, {0x2cce}}, {0x2cd1, {0x2cd0}}, {0x2cd3, {0x2cd2}}, {0x2cd5, {0x2cd4}}, {0x2cd7, {0x2cd6}},
  {0x2cd9, {0x2cd8}}, {0x2cdb, {0x2cda}}, {0x2cdd, {0x2cdc}}, {0x2cdf, {0x2cde}}, {0x2ce1, {0x2ce0}}, {0x2ce3, {0x2ce2}},
  {0x2cec, {0x2ceb}}, {0x2cee, {0x2ced}}, {0x2cf3, {0x2cf2}}, {0x2d00, {0x10a0}}, {0x2d01, {0x10a1}}, {0x2d02, {0x10a2}},
  {0x2d03, {0x10a3}}, {0x2d04, {0x10a4}}, {0x2d05, {0x10a5}}, {0x2d06, {0x10a6}}, {0x2d07, {0x10a7}}, {0x2d08, {0x10a8}},
  {0x2d09, {0x10a9}}, {0x2d0a, {0x10aa}}, {0x2d0b, {0x10ab}}, {0x2d0c, {0x10ac}}, {0x2d0d, {0x10ad}}, {0x2d0e, {0x10ae}},
  {0x2d0f, {0x10af}}, {0x2d10, {0x10b0}}, {0x2d11, {0x10b1}}, {0x2d12, {0x10b2}}, {0x2d13, {0x10b3}}, {0x2d14, {0x10b4}},
  {0x2d15, {0x10b5}}, {0x2d16, {0x10b6}}, {0x2d17, {0x10b7}}, {0x2d18, {0x10b8}}, {0x2d19, {0x10b9}}, {0x2d1a, {0x10ba}},
  {0x2d1b, {0x10bb}}, {0x2d1c, {0x10bc}}, {0x2d1d, {0x10bd}}, {0x2d1e, {0x10be}}, {0x2d1f, {0x10bf}}, {0x2d20, {0x10c0}},
  {0x2d21, {0x10c1}}, {0x2d22, {0x10c2}}, {0x2d23, {0x10c3}}, {0x2d24, {0x10c4}}, {0x2d25, {0x10c5}}, {0x2d27, {0x10c7}},
  {0x2d2d, {0x10cd}}, {0xa641, {0xa640}}, {0xa643, {0xa642}}, {0xa645, {0xa644}}, {0xa647, {0xa646}}, {0xa649, {0xa648}},
  {0xa64b, {0xa64a}}, {0xa64d, {0xa64c}}, {0xa64f, {0xa64e}}, {0xa651, {0xa650}}, {0xa653, {0xa652}}, {0xa655, {0xa654}},
  {0xa657, {0xa656}}, {0xa659, {0xa658}}, {0xa65b, {0xa65a}}, {0xa65d, {0xa65c}}, {0xa65f, {0xa65e}}, {0xa661, {0xa660}},
  {0xa663, {0xa662}}, {0xa665, {0xa664}}, {0xa667, {0xa666}}, {0xa669, {0xa668}}, {0xa66b, {0xa66a}}, {0xa66d, {0xa66c}},
  {0xa681, {0xa680}}, {0xa683, {0xa682}}, {0xa685, {0xa684}}, {0xa687, {0xa686}}, {0xa689, {0xa688}}, {0xa68b, {0xa68a}},
  {0xa68d, {0xa68c}}, {0xa68f, {0xa68e}}, {0xa691, {0xa690}}, {0xa693, {0xa692}}, {0xa695, {0xa694}}, {0xa697, {0xa696}},
  {0xa699, {0xa698}}, {0xa69b, {0xa69a}}, {0xa723, {0xa722}}, {0xa725, {0xa724}}, {0xa727, {0xa726}}, {0xa729, {0xa728}},
  {0xa72b, {0xa72a}}, {0xa72d, {0xa72c}}, {0xa72f, {0xa72e}}, {0xa733, {0xa732}}, {0xa735, {0xa734}}, {0xa737, {0xa736}},
  {0xa739, {0xa738}}, {0xa73b, {0xa73a}}, {0xa73d, {0xa73c}}, {0xa73f, {0xa73e}}, {0xa741, {0xa740}}, {0xa743, {0xa742}},
  {0xa745, {0xa744}}, {0xa747, {0xa746}}, {0xa749, {0xa748}}, {0xa74b, {0xa74a}}, {0xa74d, {0xa74c}}, {0xa74f, {0xa74e}},
  {0xa751, {0xa750}}, {0xa753, {0xa752}}, {0xa755, {0xa754}}, {0xa757, {0xa756}}, {0xa759, {0xa758}}, {0xa75b, {0xa75a}},
  {0xa75d, {0xa75c}}, {0xa75f, {0xa75e}}, {0xa761, {0xa760}}, {0xa763, {0xa762}}, {0xa765, {0xa764}}, {0xa767, {0xa766}},
  {0xa769, {0xa768}}, {0xa76b, {0xa76a}}, {0xa76d, {0xa76c}}, {0xa76f, {0xa76e}}, {0xa77a, {0xa779}}, {0xa77c, {0xa77b}},
  {0xa77f, {0xa77e}}, {0xa781, {0xa780}}, {0xa783, {0xa782}}, {0xa785, {0xa784}}, {0xa787, {0xa786}}, {0xa78c, {0xa78b}},
  {0xa791, {0xa790}}, {0xa793, {0xa792}}, {0xa794, {0xa7c4}}, {0xa797, {0xa796}}, {0xa799, {0xa798}}, {0xa79b, {0xa79a}},
  {0xa79d, {0xa79c}}, {0xa79f, {0xa79e}}, {0xa7a1, {0xa7a0}}, {0xa7a3, {0xa7a2}}, {0xa7a5, {0xa7a4}}, {0xa7a7, {0xa7a6}},
  {0xa7a9, {0xa7a8}}, {0xa7b5, {0xa7b4}}, {0xa7b7, {0xa7b6}}, {0xa7b9, {0xa7b8}}, {0xa7bb, {0xa7ba}}, {0xa7bd, {0xa7bc}},
  {0xa7bf, {0xa7be}}, {0xa7c1, {0xa7c0}}, {0xa7c3, {0xa7c2}}, {0xa7c8, {0xa7c7}}, {0xa7ca, {0xa7c9}}, {0xa7cd, {0xa7cc}},
  {0xa7cf, {0xa7ce}}, {0xa7d1, {0xa7d0}}, {0xa7d3, {0xa7d2}}, {0xa7d5, {0xa7d4}}, {0xa7d7, {0xa7d6}}, {0xa7d9, {0xa7d8}},
  {0xa7db, {0xa7da}}, {0xa7f6, {0xa7f5}}, {0xab53, {0xa7b3}}, {0xab70, {0x13a0}}, {0xab71, {0x13a1}}, {0xab72, {0x13a2}},
  {0xab73, {0x13a3}}, {0xab74, {0x13a4}}, {0xab75, {0x13a5}}, {0xab76, {0x13a6}}, {0xab77, {0x13a7}}, {0xab78, {0x13a8}},
  {0xab79, {0x13a9}}, {0xab7a, {0x13aa}}, {0xab7b, {0x13ab}}, {0xab7c, {0x13ac}}, {0xab7d, {0x13ad}}, {0xab7e, {0x13ae}},
  {0xab7f, {0x13af}}, {0xab80, {0x13b0}}, {0xab81, {0x13b1}}, {0xab82, {0x13b2}}, {0xab83, {0x13b3}}, {0xab84, {0x13b4}},
  {0xab85, {0x13b5}}, {0xab86, {0x13b6}}, {0xab87, {0x13b7}}, {0xab88, {0x13b8}}, {0xab89, {0x13b9}}, {0xab8a, {0x13ba}},
  {0xab8b, {0x13bb}}, {0xab8c, {0x13bc}}, {0xab8d, {0x13bd}}, {0xab8e, {0x13be}}, {0xab8f, {0x13bf}}, {0xab90, {0x13c0}},
  {0xab91, {0x13c1}}, {0xab92, {0x13c2}}, {0xab93, {0x13c3}}, {0xab94, {0x13c4}}, {0xab95, {0x13c5}}, {0xab96, {0x13c6}},
  {0xab97, {0x13c7}}, {0xab98, {0x13c8}}, {0xab99, {0x13c9}}, {0xab9a, {0x13ca}}, {0xab9b, {0x13cb}}, {0xab9c, {0x13cc}},
  {0xab9d, {0x13cd}}, {0xab9e, {0x13ce}}, {0xab9f, {0x13cf}}, {0xaba0, {0x13d0}}, {0xaba1, {0x13d1}}, {0xaba2, {0x13d2}},
  {0xaba3, {0x13d3}}, {0xaba4, {0x13d4}}, {0xaba5, {0x13d5}}, {0xaba6, {0x13d6}}, {0xaba7, {0x13d7}}, {0xaba8, {0x13d8}},
  {0xaba9, {0x13d9}}, {0xabaa, {0x13da}}, {0xabab, {0x13db}}, {0xabac, {0x13dc}}, {0xabad, {0x13dd}}, {0xabae, {0x13de}},
  {0xabaf, {0x13df}}, {0xabb0, {0x13e0}}, {0xabb1, {0x13e1}}, {0xabb2, {0x13e2}}, {0xabb3, {0x13e3}}, {0xabb4, {0x13e4}},
  {0xabb5, {0x13e5}}, {0xabb6, {0x13e6}}, {0xabb7, {0x13e7}}, {0xabb8, {0x13e8}}, {0xabb9, {0x13e9}}, {0xabba, {0x13ea}},
  {0xabbb, {0x13eb}}, {0xabbc, {0x13ec}}, {0xabbd, {0x13ed}}, {0xabbe, {0x13ee}}, {0xabbf, {0x13ef}}, {0xfb00, {0x46, 0x46}},
  {0xfb01, {0x46, 0x49}}, {0xfb02, {0x46, 0x4c}}, {0xfb03, {0x46, 0x46, 0x49}}, {0xfb04, {0x46, 0x46, 0x4c}}, {0xfb05, {0x53, 0x54}}, {0xfb06, {0x53, 0x54}},
  {0xfb13, {0x544, 0x546}}, {0xfb14, {0x544, 0x535}}, {0xfb15, {0x544, 0x53b}}, {0xfb16, {0x54e, 0x546}}, {0xfb17, {0x544, 0x53d}}, {0xff41, {0xff21}},
  {0xff42, {0xff22}}, {0xff43, {0xff23}}, {0xff44, {0xff24}}, {0xff45, {0xff25}}, {0xff46, {0xff26}}, {0xff47, {0xff27}},
  {0xff48, {0xff28}}, {0xff49, {0xff29}}, {0xff4a, {0xff2a}}, {0xff4b, {0xff2b}}, {0xff4c, {0xff2c}}, {0xff4d, {0xff2d}},
  {0xff4e, {0xff2e}}, {0xff4f, {0xff2f}}, {0xff50, {0xff30}}, {0xff51, {0xff31}}, {0xff52, {0xff32}}, {0xff53, {0xff33}},
  {0xff54, {0xff34}}, {0xff55, {0xff35}}, {0xff56, {0xff36}}, {0xff57, {0xff37}}, {0xff58, {0xff38}}, {0xff59, {0xff39}},
  {0xff5a, {0xff3a}}, {0x10428, {0x10400}}, {0x10429, {0x10401}}, {0x1042a, {0x10402}}, {0x1042b, {0x10403}}, {0x1042c, {0x10404}},
  {0x1042d, {0x10405}}, {0x1042e, {0x10406}}, {0x1042f, {0x10407}}, {0x10430, {0x10408}}, {0x10431, {0x10409}}, {0x10432, {0x1040a}},
  {0x10433, {0x1040b}}, {0x10434, {0x1040c}}, {0x10435, {0x1040d}}, {0x10436, {0x1040e}}, {0x10437, {0x1040f}}, {0x10438, {0x10410}},
  {0x10439, {0x10411}}, {0x1043a, {0x10412}}, {0x1043b, {0x10413}}, {0x1043c, {0x10414}}, {0x1043d, {0x10415}}, {0x1043e, {0x10416}},
  {0x1043f, {0x10417}}, {0x10440, {0x10418}}, {0x10441, {0x10419}}, {0x10442, {0x1041a}}, {0x10443, {0x1041b}}, {0x10444, {0x1041c}},
  {0x10445, {0x1041d}}, {0x10446, {0x1041e}}, {0x10447, {0x1041f}}, {0x10448, {0x10420}}, {0x10449, {0x10421}}, {0x1044a, {0x10422}},
  {0x1044b, {0x10423}}, {0x1044c, {0x10424}}, {0x1044d, {0x10425}}, {0x1044e, {0x10426}}, {0x1044f, {0x10427}}, {0x104d8, {0x104b0}},
  {0x104d9, {0x104b1}}, {0x104da, {0x104b2}}, {0x104db, {0x104b3}}, {0x104dc, {0x104b4}}, {0x104dd, {0x104b5}}, {0x104de, {0x104b6}},
  {0x104df, {0x104b7}}, {0x104e0, {0x104b8}}, {0x104e1, {0x104b9}}, {0x104e2, {0x104ba}}, {0x104e3, {0x104bb}}, {0x104e4, {0x104bc}},
  {0x104e5, {0x104bd}}, {0x104e6, {0x104be}}, {0x104e7, {0x104bf}}, {0x104e8, {0x104c0}}, {0x104e9, {0x104c1}}, {0x104ea, {0x104c2}},
  {0x104eb, {0x104c3}}, {0x104ec, {0x104c4}}, {0x104ed, {0x104c5}}, {0x104ee, {0x104c6}}, {0x104ef, {0x104c7}}, {0x104f0, {0x104c8}},
  {0x104f1, {0x104c9}}, {0x104f2, {0x104ca}}, {0x104f3, {0x104cb}}, {0x104f4, {0x104cc}}, {0x104f5, {0x104cd}}, {0x104f6, {0x104ce}},
  {0x104f7, {0x104cf}}, {0x104f8, {0x104d0}}, {0x104f9, {0x104d1}}, {0x104fa, {0x104d2}}, {0x104fb, {0x104d3}}, {0x10597, {0x10570}},
  {0x10598, {0x10571}}, {0x10599, {0x10572}}, {0x1059a, {0x10573}}, {0x1059b, {0x10574}}, {0x1059c, {0x10575}}, {0x1059d, {0x10576}},
  {0x1059e, {0x10577}}, {0x1059f, {0x10578}}, {0x105a0, {0x10579}}, {0x105a1, {0x1057a}}, {0x105a3, {0x1057c}}, {0x105a4, {0x1057d}},
  {0x105a5, {0x1057e}}, {0x105a6, {0x1057f}}, {0x105a7, {0x10580}}, {0x105a8, {0x10581}}, {0x105a9, {0x10582}}, {0x105aa, {0x10583}},
  {0x105ab, {0x10584}}, {0x105ac, {0x10585}}, {0x105ad, {0x10586}}, {0x105ae, {0x10587}}, {0x105af, {0x10588}}, {0x105b0, {0x10589}},
  {0x105b1, {0x1058a}}, {0x105b3, {0x1058c}}, {0x105b4, {0x1058d}}, {0x105b5, {0x1058e}}, {0x105b6, {0x1058f}}, {0x105b7, {0x10590}},
  {0x105b8, {0x10591}}, {0x105b9, {0x10592}}, {0x105bb, {0x10594}}, {0x105bc, {0x10595}}, {0x10cc0, {0x10c80}}, {0x10cc1, {0x10c81}},
  {0x10cc2, {0x10c82}}, {0x10cc3, {0x10c83}}, {0x10cc4, {0x10c84}}, {0x10cc5, {0x10c85}}, {0x10cc6, {0x10c86}}, {0x10cc7, {0x10c87}},
  {0x10cc8, {0x10c88}}, {0x10cc9, {0x10c89}}, {0x10cca, {0x10c8a}}, {0x10ccb, {0x10c8b}}, {0x10ccc, {0x10c8c}}, {0x10ccd, {0x10c8d}},
  {0x10cce, {0x10c8e}}, {0x10ccf, {0x10c8f}}, {0x10cd0, {0x10c90}}, {0x10cd1, {0x10c91}}, {0x10cd2, {0x10c92}}, {0x10cd3, {0x10c93}},
  {0x10cd4, {0x10c94}}, {0x10cd5, {0x10c95}}, {0x10cd6, {0x10c96}}, {0x10cd7, {0x10c97}}, {0x10cd8, {0x10c98}}, {0x10cd9, {0x10c99}},
  {0x10cda, {0x10c9a}}, {0x10cdb, {0x10c9b}}, {0x10cdc, {0x10c9c}}, {0x10cdd, {0x10c9d}}, {0x10cde, {0x10c9e}}, {0x10cdf, {0x10c9f}},
  {0x10ce0, {0x10ca0}}, {0x10ce1, {0x10ca1}}, {0x10ce2, {0x10ca2}}, {0x10ce3, {0x10ca3}}, {0x10ce4, {0x10ca4}}, {0x10ce5, {0x10ca5}},
  {0x10ce6, {0x10ca6}}, {0x10ce7, {0x10ca7}}, {0x10ce8, {0x10ca8}}, {0x10ce9, {0x10ca9}}, {0x10cea, {0x10caa}}, {0x10ceb, {0x10cab}},
  {0x10cec, {0x10cac}}, {0x10ced, {0x10cad}}, {0x10cee, {0x10cae}}, {0x10cef, {0x10caf}}, {0x10cf0, {0x10cb0}}, {0x10cf1, {0x10cb1}},
  {0x10cf2, {0x10cb2}}, {0x10d70, {0x10d50}}, {0x10d71, {0x10d51}}, {0x10d72, {0x10d52}}, {0x10d73, {0x10d53}}, {0x10d74, {0x10d54}},
  {0x10d75, {0x10d55}}, {0x10d76, {0x10d56}}, {0x10d77, {0x10d57}}, {0x10d78, {0x10d58}}, {0x10d79, {0x10d59}}, {0x10d7a, {0x10d5a}},
  {0x10d7b, {0x10d5b}}, {0x10d7c, {0x10d5c}}, {0x10d7d, {0x10d5d}}, {0x10d7e, {0x10d5e}}, {0x10d7f, {0x10d5f}}, {0x10d80, {0x10d60}},
  {0x10d81, {0x10d61}}, {0x10d82, {0x10d62}}, {0x10d83, {0x10d63}}, {0x10d84, {0x10d64}}, {0x10d85, {0x10d65}}, {0x118c0, {0x118a0}},
  {0x118c1, {0x118a1}}, {0x118c2, {0x118a2}}, {0x118c3, {0x118a3}}, {0x118c4, {0x118a4}}, {0x118c5, {0x118a5}}, {0x118c6, {0x118a6}},
  {0x118c7, {0x118a7}}, {0x118c8, {0x118a8}}, {0x118c9, {0x118a9}}, {0x118ca, {0x118aa}}, {0x118cb, {0x118ab}}, {0x118cc, {0x118ac}},
  {0x118cd, {0x118ad}}, {0x118ce, {0x118ae}}, {0x118cf, {0x118af}}, {0x118d0, {0x118b0}}, {0x118d1, {0x118b1}}, {0x118d2, {0x118b2}},
  {0x118d3, {0x118b3}}, {0x118d4, {0x118b4}}, {0x118d5, {0x118b5}}, {0x118d6, {0x118b6}}, {0x118d7, {0x118b7}}, {0x118d8, {0x118b8}},
  {0x118d9, {0x118b9}}, {0x118da, {0x118ba}}, {0x118db, {0x118bb}}, {0x118dc, {0x118bc}}, {0x118dd, {0x118bd}}, {0x118de, {0x118be}},
  {0x118df, {0x118bf}}, {0x16e60, {0x16e40}}, {0x16e61, {0x16e41}}, {0x16e62, {0x16e42}}, {0x16e63, {0x16e43}}, {0x16e64, {0x16e44}},
  {0x16e65, {0x16e45}}, {0x16e66, {0x16e46}}, {0x16e67, {0x16e47}}, {0x16e68, {0x16e48}}, {0x16e69, {0x16e49}}, {0x16e6a, {0x16e4a}},
  {0x16e6b, {0x16e4b}}, {0x16e6c, {0x16e4c}}, {0x16e6d, {0x16e4d}}, {0x16e6e, {0x16e4e}}, {0x16e6f, {0x16e4f}}, {0x16e70, {0x16e50}},
  {0x16e71, {0x16e51}}, {0x16e72, {0x16e52}}, {0x16e73, {0x16e53}}, {0x16e74, {0x16e54}}, {0x16e75, {0x16e55}}, {0x16e76, {0x16e56}},
  {0x16e77, {0x16e57}}, {0x16e78, {0x16e58}}, {0x16e79, {0x16e59}}, {0x16e7a, {0x16e5a}}, {0x16e7b, {0x16e5b}}, {0x16e7c, {0x16e5c}},
  {0x16e7d, {0x16e5d}}, {0x16e7e, {0x16e5e}}, {0x16e7f, {0x16e5f}}, {0x16ebb, {0x16ea0}}, {0x16ebc, {0x16ea1}}, {0x16ebd, {0x16ea2}},
  {0x16ebe, {0x16ea3}}, {0x16ebf, {0x16ea4}}, {0x16ec0, {0x16ea5}}, {0x16ec1, {0x16ea6}}, {0x16ec2, {0x16ea7}}, {0x16ec3, {0x16ea8}},
  {0x16ec4, {0x16ea9}}, {0x16ec5, {0x16eaa}}, {0x16ec6, {0x16eab}}, {0x16ec7, {0x16eac}}, {0x16ec8, {0x16ead}}, {0x16ec9, {0x16eae}},
  {0x16eca, {0x16eaf}}, {0x16ecb, {0x16eb0}}, {0x16ecc, {0x16eb1}}, {0x16ecd, {0x16eb2}}, {0x16ece, {0x16eb3}}, {0x16ecf, {0x16eb4}},
  {0x16ed0, {0x16eb5}}, {0x16ed1, {0x16eb6}}, {0x16ed2, {0x16eb7}}, {0x16ed3, {0x16eb8}}, {0x1e922, {0x1e900}}, {0x1e923, {0x1e901}},
  {0x1e924, {0x1e902}}, {0x1e925, {0x1e903}}, {0x1e926, {0x1e904}}, {0x1e927, {0x1e905}}, {0x1e928, {0x1e906}}, {0x1e929, {0x1e907}},
  {0x1e92a, {0x1e908}}, {0x1e92b, {0x1e909}}, {0x1e92c, {0x1e90a}}, {0x1e92d, {0x1e90b}}, {0x1e92e, {0x1e90c}}, {0x1e92f, {0x1e90d}},
  {0x1e930, {0x1e90e}}, {0x1e931, {0x1e90f}}, {0x1e932, {0x1e910}}, {0x1e933, {0x1e911}}, {0x1e934, {0x1e912}}, {0x1e935, {0x1e913}},
  {0x1e936, {0x1e914}}, {0x1e937, {0x1e915}}, {0x1e938, {0x1e916}}, {0x1e939, {0x1e917}}, {0x1e93a, {0x1e918}}, {0x1e93b, {0x1e919}},
  {0x1e93c, {0x1e91a}}, {0x1e93d, {0x1e91b}}, {0x1e93e, {0x1e91c}}, {0x1e93f, {0x1e91d}}, {0x1e940, {0x1e91e}}, {0x1e941, {0x1e91f}},
  {0x1e942, {0x1e920}}, {0x1e943, {0x1e921}},
};
static const SigmaContextRange sigma_context_ranges[] = {
  {0x27, 0x27, SIGMA_CONTEXT_IGNORABLE}, {0x2e, 0x2e, SIGMA_CONTEXT_IGNORABLE}, {0x3a, 0x3a, SIGMA_CONTEXT_IGNORABLE},
  {0x41, 0x5a, SIGMA_CONTEXT_CASED}, {0x5e, 0x5e, SIGMA_CONTEXT_IGNORABLE}, {0x60, 0x60, SIGMA_CONTEXT_IGNORABLE},
  {0x61, 0x7a, SIGMA_CONTEXT_CASED}, {0xa8, 0xa8, SIGMA_CONTEXT_IGNORABLE}, {0xaa, 0xaa, SIGMA_CONTEXT_CASED},
  {0xad, 0xad, SIGMA_CONTEXT_IGNORABLE}, {0xaf, 0xaf, SIGMA_CONTEXT_IGNORABLE}, {0xb4, 0xb4, SIGMA_CONTEXT_IGNORABLE},
  {0xb5, 0xb5, SIGMA_CONTEXT_CASED}, {0xb7, 0xb8, SIGMA_CONTEXT_IGNORABLE}, {0xba, 0xba, SIGMA_CONTEXT_CASED},
  {0xc0, 0xd6, SIGMA_CONTEXT_CASED}, {0xd8, 0xf6, SIGMA_CONTEXT_CASED}, {0xf8, 0x1ba, SIGMA_CONTEXT_CASED},
  {0x1bc, 0x1bf, SIGMA_CONTEXT_CASED}, {0x1c4, 0x293, SIGMA_CONTEXT_CASED}, {0x296, 0x2af, SIGMA_CONTEXT_CASED},
  {0x2b0, 0x36f, SIGMA_CONTEXT_IGNORABLE}, {0x370, 0x373, SIGMA_CONTEXT_CASED}, {0x374, 0x375, SIGMA_CONTEXT_IGNORABLE},
  {0x376, 0x377, SIGMA_CONTEXT_CASED}, {0x37a, 0x37a, SIGMA_CONTEXT_IGNORABLE}, {0x37b, 0x37d, SIGMA_CONTEXT_CASED},
  {0x37f, 0x37f, SIGMA_CONTEXT_CASED}, {0x384, 0x385, SIGMA_CONTEXT_IGNORABLE}, {0x386, 0x386, SIGMA_CONTEXT_CASED},
  {0x387, 0x387, SIGMA_CONTEXT_IGNORABLE}, {0x388, 0x38a, SIGMA_CONTEXT_CASED}, {0x38c, 0x38c, SIGMA_CONTEXT_CASED},
  {0x38e, 0x3a1, SIGMA_CONTEXT_CASED}, {0x3a3, 0x3f5, SIGMA_CONTEXT_CASED}, {0x3f7, 0x481, SIGMA_CONTEXT_CASED},
  {0x483, 0x489, SIGMA_CONTEXT_IGNORABLE}, {0x48a, 0x52f, SIGMA_CONTEXT_CASED}, {0x531, 0x556, SIGMA_CONTEXT_CASED},
  {0x559, 0x559, SIGMA_CONTEXT_IGNORABLE}, {0x55f, 0x55f, SIGMA_CONTEXT_IGNORABLE}, {0x560, 0x588, SIGMA_CONTEXT_CASED},
  {0x591, 0x5bd, SIGMA_CONTEXT_IGNORABLE}, {0x5bf, 0x5bf, SIGMA_CONTEXT_IGNORABLE}, {0x5c1, 0x5c2, SIGMA_CONTEXT_IGNORABLE},
  {0x5c4, 0x5c5, SIGMA_CONTEXT_IGNORABLE}, {0x5c7, 0x5c7, SIGMA_CONTEXT_IGNORABLE}, {0x5f4, 0x5f4, SIGMA_CONTEXT_IGNORABLE},
  {0x600, 0x605, SIGMA_CONTEXT_IGNORABLE}, {0x610, 0x61a, SIGMA_CONTEXT_IGNORABLE}, {0x61c, 0x61c, SIGMA_CONTEXT_IGNORABLE},
  {0x640, 0x640, SIGMA_CONTEXT_IGNORABLE}, {0x64b, 0x65f, SIGMA_CONTEXT_IGNORABLE}, {0x670, 0x670, SIGMA_CONTEXT_IGNORABLE},
  {0x6d6, 0x6dd, SIGMA_CONTEXT_IGNORABLE}, {0x6df, 0x6e8, SIGMA_CONTEXT_IGNORABLE}, {0x6ea, 0x6ed, SIGMA_CONTEXT_IGNORABLE},
  {0x70f, 0x70f, SIGMA_CONTEXT_IGNORABLE}, {0x711, 0x711, SIGMA_CONTEXT_IGNORABLE}, {0x730, 0x74a, SIGMA_CONTEXT_IGNORABLE},
  {0x7a6, 0x7b0, SIGMA_CONTEXT_IGNORABLE}, {0x7eb, 0x7f5, SIGMA_CONTEXT_IGNORABLE}, {0x7fa, 0x7fa, SIGMA_CONTEXT_IGNORABLE},
  {0x7fd, 0x7fd, SIGMA_CONTEXT_IGNORABLE}, {0x816, 0x82d, SIGMA_CONTEXT_IGNORABLE}, {0x859, 0x85b, SIGMA_CONTEXT_IGNORABLE},
  {0x888, 0x888, SIGMA_CONTEXT_IGNORABLE}, {0x890, 0x891, SIGMA_CONTEXT_IGNORABLE}, {0x897, 0x89f, SIGMA_CONTEXT_IGNORABLE},
  {0x8c9, 0x902, SIGMA_CONTEXT_IGNORABLE}, {0x93a, 0x93a, SIGMA_CONTEXT_IGNORABLE}, {0x93c, 0x93c, SIGMA_CONTEXT_IGNORABLE},
  {0x941, 0x948, SIGMA_CONTEXT_IGNORABLE}, {0x94d, 0x94d, SIGMA_CONTEXT_IGNORABLE}, {0x951, 0x957, SIGMA_CONTEXT_IGNORABLE},
  {0x962, 0x963, SIGMA_CONTEXT_IGNORABLE}, {0x971, 0x971, SIGMA_CONTEXT_IGNORABLE}, {0x981, 0x981, SIGMA_CONTEXT_IGNORABLE},
  {0x9bc, 0x9bc, SIGMA_CONTEXT_IGNORABLE}, {0x9c1, 0x9c4, SIGMA_CONTEXT_IGNORABLE}, {0x9cd, 0x9cd, SIGMA_CONTEXT_IGNORABLE},
  {0x9e2, 0x9e3, SIGMA_CONTEXT_IGNORABLE}, {0x9fe, 0x9fe, SIGMA_CONTEXT_IGNORABLE}, {0xa01, 0xa02, SIGMA_CONTEXT_IGNORABLE},
  {0xa3c, 0xa3c, SIGMA_CONTEXT_IGNORABLE}, {0xa41, 0xa42, SIGMA_CONTEXT_IGNORABLE}, {0xa47, 0xa48, SIGMA_CONTEXT_IGNORABLE},
  {0xa4b, 0xa4d, SIGMA_CONTEXT_IGNORABLE}, {0xa51, 0xa51, SIGMA_CONTEXT_IGNORABLE}, {0xa70, 0xa71, SIGMA_CONTEXT_IGNORABLE},
  {0xa75, 0xa75, SIGMA_CONTEXT_IGNORABLE}, {0xa81, 0xa82, SIGMA_CONTEXT_IGNORABLE}, {0xabc, 0xabc, SIGMA_CONTEXT_IGNORABLE},
  {0xac1, 0xac5, SIGMA_CONTEXT_IGNORABLE}, {0xac7, 0xac8, SIGMA_CONTEXT_IGNORABLE}, {0xacd, 0xacd, SIGMA_CONTEXT_IGNORABLE},
  {0xae2, 0xae3, SIGMA_CONTEXT_IGNORABLE}, {0xafa, 0xaff, SIGMA_CONTEXT_IGNORABLE}, {0xb01, 0xb01, SIGMA_CONTEXT_IGNORABLE},
  {0xb3c, 0xb3c, SIGMA_CONTEXT_IGNORABLE}, {0xb3f, 0xb3f, SIGMA_CONTEXT_IGNORABLE}, {0xb41, 0xb44, SIGMA_CONTEXT_IGNORABLE},
  {0xb4d, 0xb4d, SIGMA_CONTEXT_IGNORABLE}, {0xb55, 0xb56, SIGMA_CONTEXT_IGNORABLE}, {0xb62, 0xb63, SIGMA_CONTEXT_IGNORABLE},
  {0xb82, 0xb82, SIGMA_CONTEXT_IGNORABLE}, {0xbc0, 0xbc0, SIGMA_CONTEXT_IGNORABLE}, {0xbcd, 0xbcd, SIGMA_CONTEXT_IGNORABLE},
  {0xc00, 0xc00, SIGMA_CONTEXT_IGNORABLE}, {0xc04, 0xc04, SIGMA_CONTEXT_IGNORABLE}, {0xc3c, 0xc3c, SIGMA_CONTEXT_IGNORABLE},
  {0xc3e, 0xc40, SIGMA_CONTEXT_IGNORABLE}, {0xc46, 0xc48, SIGMA_CONTEXT_IGNORABLE}, {0xc4a, 0xc4d, SIGMA_CONTEXT_IGNORABLE},
  {0xc55, 0xc56, SIGMA_CONTEXT_IGNORABLE}, {0xc62, 0xc63, SIGMA_CONTEXT_IGNORABLE}, {0xc81, 0xc81, SIGMA_CONTEXT_IGNORABLE},
  {0xcbc, 0xcbc, SIGMA_CONTEXT_IGNORABLE}, {0xcbf, 0xcbf, SIGMA_CONTEXT_IGNORABLE}, {0xcc6, 0xcc6, SIGMA_CONTEXT_IGNORABLE},
  {0xccc, 0xccd, SIGMA_CONTEXT_IGNORABLE}, {0xce2, 0xce3, SIGMA_CONTEXT_IGNORABLE}, {0xd00, 0xd01, SIGMA_CONTEXT_IGNORABLE},
  {0xd3b, 0xd3c, SIGMA_CONTEXT_IGNORABLE}, {0xd41, 0xd44, SIGMA_CONTEXT_IGNORABLE}, {0xd4d, 0xd4d, SIGMA_CONTEXT_IGNORABLE},
  {0xd62, 0xd63, SIGMA_CONTEXT_IGNORABLE}, {0xd81, 0xd81, SIGMA_CONTEXT_IGNORABLE}, {0xdca, 0xdca, SIGMA_CONTEXT_IGNORABLE},
  {0xdd2, 0xdd4, SIGMA_CONTEXT_IGNORABLE}, {0xdd6, 0xdd6, SIGMA_CONTEXT_IGNORABLE}, {0xe31, 0xe31, SIGMA_CONTEXT_IGNORABLE},
  {0xe34, 0xe3a, SIGMA_CONTEXT_IGNORABLE}, {0xe46, 0xe4e, SIGMA_CONTEXT_IGNORABLE}, {0xeb1, 0xeb1, SIGMA_CONTEXT_IGNORABLE},
  {0xeb4, 0xebc, SIGMA_CONTEXT_IGNORABLE}, {0xec6, 0xec6, SIGMA_CONTEXT_IGNORABLE}, {0xec8, 0xece, SIGMA_CONTEXT_IGNORABLE},
  {0xf18, 0xf19, SIGMA_CONTEXT_IGNORABLE}, {0xf35, 0xf35, SIGMA_CONTEXT_IGNORABLE}, {0xf37, 0xf37, SIGMA_CONTEXT_IGNORABLE},
  {0xf39, 0xf39, SIGMA_CONTEXT_IGNORABLE}, {0xf71, 0xf7e, SIGMA_CONTEXT_IGNORABLE}, {0xf80, 0xf84, SIGMA_CONTEXT_IGNORABLE},
  {0xf86, 0xf87, SIGMA_CONTEXT_IGNORABLE}, {0xf8d, 0xf97, SIGMA_CONTEXT_IGNORABLE}, {0xf99, 0xfbc, SIGMA_CONTEXT_IGNORABLE},
  {0xfc6, 0xfc6, SIGMA_CONTEXT_IGNORABLE}, {0x102d, 0x1030, SIGMA_CONTEXT_IGNORABLE}, {0x1032, 0x1037, SIGMA_CONTEXT_IGNORABLE},
  {0x1039, 0x103a, SIGMA_CONTEXT_IGNORABLE}, {0x103d, 0x103e, SIGMA_CONTEXT_IGNORABLE}, {0x1058, 0x1059, SIGMA_CONTEXT_IGNORABLE},
  {0x105e, 0x1060, SIGMA_CONTEXT_IGNORABLE}, {0x1071, 0x1074, SIGMA_CONTEXT_IGNORABLE}, {0x1082, 0x1082, SIGMA_CONTEXT_IGNORABLE},
  {0x1085, 0x1086, SIGMA_CONTEXT_IGNORABLE}, {0x108d, 0x108d, SIGMA_CONTEXT_IGNORABLE}, {0x109d, 0x109d, SIGMA_CONTEXT_IGNORABLE},
  {0x10a0, 0x10c5, SIGMA_CONTEXT_CASED}, {0x10c7, 0x10c7, SIGMA_CONTEXT_CASED}, {0x10cd, 0x10cd, SIGMA_CONTEXT_CASED},
  {0x10d0, 0x10fa, SIGMA_CONTEXT_CASED}, {0x10fc, 0x10fc, SIGMA_CONTEXT_IGNORABLE}, {0x10fd, 0x10ff, SIGMA_CONTEXT_CASED},
  {0x135d, 0x135f, SIGMA_CONTEXT_IGNORABLE}, {0x13a0, 0x13f5, SIGMA_CONTEXT_CASED}, {0x13f8, 0x13fd, SIGMA_CONTEXT_CASED},
  {0x1712, 0x1714, SIGMA_CONTEXT_IGNORABLE}, {0x1732, 0x1733, SIGMA_CONTEXT_IGNORABLE}, {0x1752, 0x1753, SIGMA_CONTEXT_IGNORABLE},
  {0x1772, 0x1773, SIGMA_CONTEXT_IGNORABLE}, {0x17b4, 0x17b5, SIGMA_CONTEXT_IGNORABLE}, {0x17b7, 0x17bd, SIGMA_CONTEXT_IGNORABLE},
  {0x17c6, 0x17c6, SIGMA_CONTEXT_IGNORABLE}, {0x17c9, 0x17d3, SIGMA_CONTEXT_IGNORABLE}, {0x17d7, 0x17d7, SIGMA_CONTEXT_IGNORABLE},
  {0x17dd, 0x17dd, SIGMA_CONTEXT_IGNORABLE}, {0x180b, 0x180f, SIGMA_CONTEXT_IGNORABLE}, {0x1843, 0x1843, SIGMA_CONTEXT_IGNORABLE},
  {0x1885, 0x1886, SIGMA_CONTEXT_IGNORABLE}, {0x18a9, 0x18a9, SIGMA_CONTEXT_IGNORABLE}, {0x1920, 0x1922, SIGMA_CONTEXT_IGNORABLE},
  {0x1927, 0x1928, SIGMA_CONTEXT_IGNORABLE}, {0x1932, 0x1932, SIGMA_CONTEXT_IGNORABLE}, {0x1939, 0x193b, SIGMA_CONTEXT_IGNORABLE},
  {0x1a17, 0x1a18, SIGMA_CONTEXT_IGNORABLE}, {0x1a1b, 0x1a1b, SIGMA_CONTEXT_IGNORABLE}, {0x1a56, 0x1a56, SIGMA_CONTEXT_IGNORABLE},
  {0x1a58, 0x1a5e, SIGMA_CONTEXT_IGNORABLE}, {0x1a60, 0x1a60, SIGMA_CONTEXT_IGNORABLE}, {0x1a62, 0x1a62, SIGMA_CONTEXT_IGNORABLE},
  {0x1a65, 0x1a6c, SIGMA_CONTEXT_IGNORABLE}, {0x1a73, 0x1a7c, SIGMA_CONTEXT_IGNORABLE}, {0x1a7f, 0x1a7f, SIGMA_CONTEXT_IGNORABLE},
  {0x1aa7, 0x1aa7, SIGMA_CONTEXT_IGNORABLE}, {0x1ab0, 0x1add, SIGMA_CONTEXT_IGNORABLE}, {0x1ae0, 0x1aeb, SIGMA_CONTEXT_IGNORABLE},
  {0x1b00, 0x1b03, SIGMA_CONTEXT_IGNORABLE}, {0x1b34, 0x1b34, SIGMA_CONTEXT_IGNORABLE}, {0x1b36, 0x1b3a, SIGMA_CONTEXT_IGNORABLE},
  {0x1b3c, 0x1b3c, SIGMA_CONTEXT_IGNORABLE}, {0x1b42, 0x1b42, SIGMA_CONTEXT_IGNORABLE}, {0x1b6b, 0x1b73, SIGMA_CONTEXT_IGNORABLE},
  {0x1b80, 0x1b81, SIGMA_CONTEXT_IGNORABLE}, {0x1ba2, 0x1ba5, SIGMA_CONTEXT_IGNORABLE}, {0x1ba8, 0x1ba9, SIGMA_CONTEXT_IGNORABLE},
  {0x1bab, 0x1bad, SIGMA_CONTEXT_IGNORABLE}, {0x1be6, 0x1be6, SIGMA_CONTEXT_IGNORABLE}, {0x1be8, 0x1be9, SIGMA_CONTEXT_IGNORABLE},
  {0x1bed, 0x1bed, SIGMA_CONTEXT_IGNORABLE}, {0x1bef, 0x1bf1, SIGMA_CONTEXT_IGNORABLE}, {0x1c2c, 0x1c33, SIGMA_CONTEXT_IGNORABLE},
  {0x1c36, 0x1c37, SIGMA_CONTEXT_IGNORABLE}, {0x1c78, 0x1c7d, SIGMA_CONTEXT_IGNORABLE}, {0x1c80, 0x1c8a, SIGMA_CONTEXT_CASED},
  {0x1c90, 0x1cba, SIGMA_CONTEXT_CASED}, {0x1cbd, 0x1cbf, SIGMA_CONTEXT_CASED}, {0x1cd0, 0x1cd2, SIGMA_CONTEXT_IGNORABLE},
  {0x1cd4, 0x1ce0, SIGMA_CONTEXT_IGNORABLE}, {0x1ce2, 0x1ce8, SIGMA_CONTEXT_IGNORABLE}, {0x1ced, 0x1ced, SIGMA_CONTEXT_IGNORABLE},
  {0x1cf4, 0x1cf4, SIGMA_CONTEXT_IGNORABLE}, {0x1cf8, 0x1cf9, SIGMA_CONTEXT_IGNORABLE}, {0x1d00, 0x1d2b, SIGMA_CONTEXT_CASED},
  {0x1d2c, 0x1d6a, SIGMA_CONTEXT_IGNORABLE}, {0x1d6b, 0x1d77, SIGMA_CONTEXT_CASED}, {0x1d78, 0x1d78, SIGMA_CONTEXT_IGNORABLE},
  {0x1d79, 0x1d9a, SIGMA_CONTEXT_CASED}, {0x1d9b, 0x1dff, SIGMA_CONTEXT_IGNORABLE}, {0x1e00, 0x1f15, SIGMA_CONTEXT_CASED},
  {0x1f18, 0x1f1d, SIGMA_CONTEXT_CASED}, {0x1f20, 0x1f45, SIGMA_CONTEXT_CASED}, {0x1f48, 0x1f4d, SIGMA_CONTEXT_CASED},
  {0x1f50, 0x1f57, SIGMA_CONTEXT_CASED}, {0x1f59, 0x1f59, SIGMA_CONTEXT_CASED}, {0x1f5b, 0x1f5b, SIGMA_CONTEXT_CASED},
  {0x1f5d, 0x1f5d, SIGMA_CONTEXT_CASED}, {0x1f5f, 0x1f7d, SIGMA_CONTEXT_CASED}, {0x1f80, 0x1fb4, SIGMA_CONTEXT_CASED},
  {0x1fb6, 0x1fbc, SIGMA_CONTEXT_CASED}, {0x1fbd, 0x1fbd, SIGMA_CONTEXT_IGNORABLE}, {0x1fbe, 0x1fbe, SIGMA_CONTEXT_CASED},
  {0x1fbf, 0x1fc1, SIGMA_CONTEXT_IGNORABLE}, {0x1fc2, 0x1fc4, SIGMA_CONTEXT_CASED}, {0x1fc6, 0x1fcc, SIGMA_CONTEXT_CASED},
  {0x1fcd, 0x1fcf, SIGMA_CONTEXT_IGNORABLE}, {0x1fd0, 0x1fd3, SIGMA_CONTEXT_CASED}, {0x1fd6, 0x1fdb, SIGMA_CONTEXT_CASED},
  {0x1fdd, 0x1fdf, SIGMA_CONTEXT_IGNORABLE}, {0x1fe0, 0x1fec, SIGMA_CONTEXT_CASED}, {0x1fed, 0x1fef, SIGMA_CONTEXT_IGNORABLE},
  {0x1ff2, 0x1ff4, SIGMA_CONTEXT_CASED}, {0x1ff6, 0x1ffc, SIGMA_CONTEXT_CASED}, {0x1ffd, 0x1ffe, SIGMA_CONTEXT_IGNORABLE},
  {0x200b, 0x200f, SIGMA_CONTEXT_IGNORABLE}, {0x2018, 0x2019, SIGMA_CONTEXT_IGNORABLE}, {0x2024, 0x2024, SIGMA_CONTEXT_IGNORABLE},
  {0x2027, 0x2027, SIGMA_CONTEXT_IGNORABLE}, {0x202a, 0x202e, SIGMA_CONTEXT_IGNORABLE}, {0x2060, 0x2064, SIGMA_CONTEXT_IGNORABLE},
  {0x2066, 0x206f, SIGMA_CONTEXT_IGNORABLE}, {0x2071, 0x2071, SIGMA_CONTEXT_IGNORABLE}, {0x207f, 0x207f, SIGMA_CONTEXT_IGNORABLE},
  {0x2090, 0x209c, SIGMA_CONTEXT_IGNORABLE}, {0x20d0, 0x20f0, SIGMA_CONTEXT_IGNORABLE}, {0x2102, 0x2102, SIGMA_CONTEXT_CASED},
  {0x2107, 0x2107, SIGMA_CONTEXT_CASED}, {0x210a, 0x2113, SIGMA_CONTEXT_CASED}, {0x2115, 0x2115, SIGMA_CONTEXT_CASED},
  {0x2119, 0x211d, SIGMA_CONTEXT_CASED}, {0x2124, 0x2124, SIGMA_CONTEXT_CASED}, {0x2126, 0x2126, SIGMA_CONTEXT_CASED},
  {0x2128, 0x2128, SIGMA_CONTEXT_CASED}, {0x212a, 0x212d, SIGMA_CONTEXT_CASED}, {0x212f, 0x2134, SIGMA_CONTEXT_CASED},
  {0x2139, 0x2139, SIGMA_CONTEXT_CASED}, {0x213c, 0x213f, SIGMA_CONTEXT_CASED}, {0x2145, 0x2149, SIGMA_CONTEXT_CASED},
  {0x214e, 0x214e, SIGMA_CONTEXT_CASED}, {0x2160, 0x217f, SIGMA_CONTEXT_CASED}, {0x2183, 0x2184, SIGMA_CONTEXT_CASED},
  {0x24b6, 0x24e9, SIGMA_CONTEXT_CASED}, {0x2c00, 0x2c7b, SIGMA_CONTEXT_CASED}, {0x2c7c, 0x2c7d, SIGMA_CONTEXT_IGNORABLE},
  {0x2c7e, 0x2ce4, SIGMA_CONTEXT_CASED}, {0x2ceb, 0x2cee, SIGMA_CONTEXT_CASED}, {0x2cef, 0x2cf1, SIGMA_CONTEXT_IGNORABLE},
  {0x2cf2, 0x2cf3, SIGMA_CONTEXT_CASED}, {0x2d00, 0x2d25, SIGMA_CONTEXT_CASED}, {0x2d27, 0x2d27, SIGMA_CONTEXT_CASED},
  {0x2d2d, 0x2d2d, SIGMA_CONTEXT_CASED}, {0x2d6f, 0x2d6f, SIGMA_CONTEXT_IGNORABLE}, {0x2d7f, 0x2d7f, SIGMA_CONTEXT_IGNORABLE},
  {0x2de0, 0x2dff, SIGMA_CONTEXT_IGNORABLE}, {0x2e2f, 0x2e2f, SIGMA_CONTEXT_IGNORABLE}, {0x3005, 0x3005, SIGMA_CONTEXT_IGNORABLE},
  {0x302a, 0x302d, SIGMA_CONTEXT_IGNORABLE}, {0x3031, 0x3035, SIGMA_CONTEXT_IGNORABLE}, {0x303b, 0x303b, SIGMA_CONTEXT_IGNORABLE},
  {0x3099, 0x309e, SIGMA_CONTEXT_IGNORABLE}, {0x30fc, 0x30fe, SIGMA_CONTEXT_IGNORABLE}, {0xa015, 0xa015, SIGMA_CONTEXT_IGNORABLE},
  {0xa4f8, 0xa4fd, SIGMA_CONTEXT_IGNORABLE}, {0xa60c, 0xa60c, SIGMA_CONTEXT_IGNORABLE}, {0xa640, 0xa66d, SIGMA_CONTEXT_CASED},
  {0xa66f, 0xa672, SIGMA_CONTEXT_IGNORABLE}, {0xa674, 0xa67d, SIGMA_CONTEXT_IGNORABLE}, {0xa67f, 0xa67f, SIGMA_CONTEXT_IGNORABLE},
  {0xa680, 0xa69b, SIGMA_CONTEXT_CASED}, {0xa69c, 0xa69f, SIGMA_CONTEXT_IGNORABLE}, {0xa6f0, 0xa6f1, SIGMA_CONTEXT_IGNORABLE},
  {0xa700, 0xa721, SIGMA_CONTEXT_IGNORABLE}, {0xa722, 0xa76f, SIGMA_CONTEXT_CASED}, {0xa770, 0xa770, SIGMA_CONTEXT_IGNORABLE},
  {0xa771, 0xa787, SIGMA_CONTEXT_CASED}, {0xa788, 0xa78a, SIGMA_CONTEXT_IGNORABLE}, {0xa78b, 0xa78e, SIGMA_CONTEXT_CASED},
  {0xa790, 0xa7dc, SIGMA_CONTEXT_CASED}, {0xa7f1, 0xa7f4, SIGMA_CONTEXT_IGNORABLE}, {0xa7f5, 0xa7f6, SIGMA_CONTEXT_CASED},
  {0xa7f8, 0xa7f9, SIGMA_CONTEXT_IGNORABLE}, {0xa7fa, 0xa7fa, SIGMA_CONTEXT_CASED}, {0xa802, 0xa802, SIGMA_CONTEXT_IGNORABLE},
  {0xa806, 0xa806, SIGMA_CONTEXT_IGNORABLE}, {0xa80b, 0xa80b, SIGMA_CONTEXT_IGNORABLE}, {0xa825, 0xa826, SIGMA_CONTEXT_IGNORABLE},
  {0xa82c, 0xa82c, SIGMA_CONTEXT_IGNORABLE}, {0xa8c4, 0xa8c5, SIGMA_CONTEXT_IGNORABLE}, {0xa8e0, 0xa8f1, SIGMA_CONTEXT_IGNORABLE},
  {0xa8ff, 0xa8ff, SIGMA_CONTEXT_IGNORABLE}, {0xa926, 0xa92d, SIGMA_CONTEXT_IGNORABLE}, {0xa947, 0xa951, SIGMA_CONTEXT_IGNORABLE},
  {0xa980, 0xa982, SIGMA_CONTEXT_IGNORABLE}, {0xa9b3, 0xa9b3, SIGMA_CONTEXT_IGNORABLE}, {0xa9b6, 0xa9b9, SIGMA_CONTEXT_IGNORABLE},
  {0xa9bc, 0xa9bd, SIGMA_CONTEXT_IGNORABLE}, {0xa9cf, 0xa9cf, SIGMA_CONTEXT_IGNORABLE}, {0xa9e5, 0xa9e6, SIGMA_CONTEXT_IGNORABLE},
  {0xaa29, 0xaa2e, SIGMA_CONTEXT_IGNORABLE}, {0xaa31, 0xaa32, SIGMA_CONTEXT_IGNORABLE}, {0xaa35, 0xaa36, SIGMA_CONTEXT_IGNORABLE},
  {0xaa43, 0xaa43, SIGMA_CONTEXT_IGNORABLE}, {0xaa4c, 0xaa4c, SIGMA_CONTEXT_IGNORABLE}, {0xaa70, 0xaa70, SIGMA_CONTEXT_IGNORABLE},
  {0xaa7c, 0xaa7c, SIGMA_CONTEXT_IGNORABLE}, {0xaab0, 0xaab0, SIGMA_CONTEXT_IGNORABLE}, {0xaab2, 0xaab4, SIGMA_CONTEXT_IGNORABLE},
  {0xaab7, 0xaab8, SIGMA_CONTEXT_IGNORABLE}, {0xaabe, 0xaabf, SIGMA_CONTEXT_IGNORABLE}, {0xaac1, 0xaac1, SIGMA_CONTEXT_IGNORABLE},
  {0xaadd, 0xaadd, SIGMA_CONTEXT_IGNORABLE}, {0xaaec, 0xaaed, SIGMA_CONTEXT_IGNORABLE}, {0xaaf3, 0xaaf4, SIGMA_CONTEXT_IGNORABLE},
  {0xaaf6, 0xaaf6, SIGMA_CONTEXT_IGNORABLE}, {0xab30, 0xab5a, SIGMA_CONTEXT_CASED}, {0xab5b, 0xab5f, SIGMA_CONTEXT_IGNORABLE},
  {0xab60, 0xab68, SIGMA_CONTEXT_CASED}, {0xab69, 0xab6b, SIGMA_CONTEXT_IGNORABLE}, {0xab70, 0xabbf, SIGMA_CONTEXT_CASED},
  {0xabe5, 0xabe5, SIGMA_CONTEXT_IGNORABLE}, {0xabe8, 0xabe8, SIGMA_CONTEXT_IGNORABLE}, {0xabed, 0xabed, SIGMA_CONTEXT_IGNORABLE},
  {0xfb00, 0xfb06, SIGMA_CONTEXT_CASED}, {0xfb13, 0xfb17, SIGMA_CONTEXT_CASED}, {0xfb1e, 0xfb1e, SIGMA_CONTEXT_IGNORABLE},
  {0xfbb2, 0xfbc2, SIGMA_CONTEXT_IGNORABLE}, {0xfe00, 0xfe0f, SIGMA_CONTEXT_IGNORABLE}, {0xfe13, 0xfe13, SIGMA_CONTEXT_IGNORABLE},
  {0xfe20, 0xfe2f, SIGMA_CONTEXT_IGNORABLE}, {0xfe52, 0xfe52, SIGMA_CONTEXT_IGNORABLE}, {0xfe55, 0xfe55, SIGMA_CONTEXT_IGNORABLE},
  {0xfeff, 0xfeff, SIGMA_CONTEXT_IGNORABLE}, {0xff07, 0xff07, SIGMA_CONTEXT_IGNORABLE}, {0xff0e, 0xff0e, SIGMA_CONTEXT_IGNORABLE},
  {0xff1a, 0xff1a, SIGMA_CONTEXT_IGNORABLE}, {0xff21, 0xff3a, SIGMA_CONTEXT_CASED}, {0xff3e, 0xff3e, SIGMA_CONTEXT_IGNORABLE},
  {0xff40, 0xff40, SIGMA_CONTEXT_IGNORABLE}, {0xff41, 0xff5a, SIGMA_CONTEXT_CASED}, {0xff70, 0xff70, SIGMA_CONTEXT_IGNORABLE},
  {0xff9e, 0xff9f, SIGMA_CONTEXT_IGNORABLE}, {0xffe3, 0xffe3, SIGMA_CONTEXT_IGNORABLE}, {0xfff9, 0xfffb, SIGMA_CONTEXT_IGNORABLE},
  {0x101fd, 0x101fd, SIGMA_CONTEXT_IGNORABLE}, {0x102e0, 0x102e0, SIGMA_CONTEXT_IGNORABLE}, {0x10376, 0x1037a, SIGMA_CONTEXT_IGNORABLE},
  {0x10400, 0x1044f, SIGMA_CONTEXT_CASED}, {0x104b0, 0x104d3, SIGMA_CONTEXT_CASED}, {0x104d8, 0x104fb, SIGMA_CONTEXT_CASED},
  {0x10570, 0x1057a, SIGMA_CONTEXT_CASED}, {0x1057c, 0x1058a, SIGMA_CONTEXT_CASED}, {0x1058c, 0x10592, SIGMA_CONTEXT_CASED},
  {0x10594, 0x10595, SIGMA_CONTEXT_CASED}, {0x10597, 0x105a1, SIGMA_CONTEXT_CASED}, {0x105a3, 0x105b1, SIGMA_CONTEXT_CASED},
  {0x105b3, 0x105b9, SIGMA_CONTEXT_CASED}, {0x105bb, 0x105bc, SIGMA_CONTEXT_CASED}, {0x10780, 0x10785, SIGMA_CONTEXT_IGNORABLE},
  {0x10787, 0x107b0, SIGMA_CONTEXT_IGNORABLE}, {0x107b2, 0x107ba, SIGMA_CONTEXT_IGNORABLE}, {0x10a01, 0x10a03, SIGMA_CONTEXT_IGNORABLE},
  {0x10a05, 0x10a06, SIGMA_CONTEXT_IGNORABLE}, {0x10a0c, 0x10a0f, SIGMA_CONTEXT_IGNORABLE}, {0x10a38, 0x10a3a, SIGMA_CONTEXT_IGNORABLE},
  {0x10a3f, 0x10a3f, SIGMA_CONTEXT_IGNORABLE}, {0x10ae5, 0x10ae6, SIGMA_CONTEXT_IGNORABLE}, {0x10c80, 0x10cb2, SIGMA_CONTEXT_CASED},
  {0x10cc0, 0x10cf2, SIGMA_CONTEXT_CASED}, {0x10d24, 0x10d27, SIGMA_CONTEXT_IGNORABLE}, {0x10d4e, 0x10d4e, SIGMA_CONTEXT_IGNORABLE},
  {0x10d50, 0x10d65, SIGMA_CONTEXT_CASED}, {0x10d69, 0x10d6d, SIGMA_CONTEXT_IGNORABLE}, {0x10d6f, 0x10d6f, SIGMA_CONTEXT_IGNORABLE},
  {0x10d70, 0x10d85, SIGMA_CONTEXT_CASED}, {0x10eab, 0x10eac, SIGMA_CONTEXT_IGNORABLE}, {0x10ec5, 0x10ec5, SIGMA_CONTEXT_IGNORABLE},
  {0x10efa, 0x10eff, SIGMA_CONTEXT_IGNORABLE}, {0x10f46, 0x10f50, SIGMA_CONTEXT_IGNORABLE}, {0x10f82, 0x10f85, SIGMA_CONTEXT_IGNORABLE},
  {0x11001, 0x11001, SIGMA_CONTEXT_IGNORABLE}, {0x11038, 0x11046, SIGMA_CONTEXT_IGNORABLE}, {0x11070, 0x11070, SIGMA_CONTEXT_IGNORABLE},
  {0x11073, 0x11074, SIGMA_CONTEXT_IGNORABLE}, {0x1107f, 0x11081, SIGMA_CONTEXT_IGNORABLE}, {0x110b3, 0x110b6, SIGMA_CONTEXT_IGNORABLE},
  {0x110b9, 0x110ba, SIGMA_CONTEXT_IGNORABLE}, {0x110bd, 0x110bd, SIGMA_CONTEXT_IGNORABLE}, {0x110c2, 0x110c2, SIGMA_CONTEXT_IGNORABLE},
  {0x110cd, 0x110cd, SIGMA_CONTEXT_IGNORABLE}, {0x11100, 0x11102, SIGMA_CONTEXT_IGNORABLE}, {0x11127, 0x1112b, SIGMA_CONTEXT_IGNORABLE},
  {0x1112d, 0x11134, SIGMA_CONTEXT_IGNORABLE}, {0x11173, 0x11173, SIGMA_CONTEXT_IGNORABLE}, {0x11180, 0x11181, SIGMA_CONTEXT_IGNORABLE},
  {0x111b6, 0x111be, SIGMA_CONTEXT_IGNORABLE}, {0x111c9, 0x111cc, SIGMA_CONTEXT_IGNORABLE}, {0x111cf, 0x111cf, SIGMA_CONTEXT_IGNORABLE},
  {0x1122f, 0x11231, SIGMA_CONTEXT_IGNORABLE}, {0x11234, 0x11234, SIGMA_CONTEXT_IGNORABLE}, {0x11236, 0x11237, SIGMA_CONTEXT_IGNORABLE},
  {0x1123e, 0x1123e, SIGMA_CONTEXT_IGNORABLE}, {0x11241, 0x11241, SIGMA_CONTEXT_IGNORABLE}, {0x112df, 0x112df, SIGMA_CONTEXT_IGNORABLE},
  {0x112e3, 0x112ea, SIGMA_CONTEXT_IGNORABLE}, {0x11300, 0x11301, SIGMA_CONTEXT_IGNORABLE}, {0x1133b, 0x1133c, SIGMA_CONTEXT_IGNORABLE},
  {0x11340, 0x11340, SIGMA_CONTEXT_IGNORABLE}, {0x11366, 0x1136c, SIGMA_CONTEXT_IGNORABLE}, {0x11370, 0x11374, SIGMA_CONTEXT_IGNORABLE},
  {0x113bb, 0x113c0, SIGMA_CONTEXT_IGNORABLE}, {0x113ce, 0x113ce, SIGMA_CONTEXT_IGNORABLE}, {0x113d0, 0x113d0, SIGMA_CONTEXT_IGNORABLE},
  {0x113d2, 0x113d2, SIGMA_CONTEXT_IGNORABLE}, {0x113e1, 0x113e2, SIGMA_CONTEXT_IGNORABLE}, {0x11438, 0x1143f, SIGMA_CONTEXT_IGNORABLE},
  {0x11442, 0x11444, SIGMA_CONTEXT_IGNORABLE}, {0x11446, 0x11446, SIGMA_CONTEXT_IGNORABLE}, {0x1145e, 0x1145e, SIGMA_CONTEXT_IGNORABLE},
  {0x114b3, 0x114b8, SIGMA_CONTEXT_IGNORABLE}, {0x114ba, 0x114ba, SIGMA_CONTEXT_IGNORABLE}, {0x114bf, 0x114c0, SIGMA_CONTEXT_IGNORABLE},
  {0x114c2, 0x114c3, SIGMA_CONTEXT_IGNORABLE}, {0x115b2, 0x115b5, SIGMA_CONTEXT_IGNORABLE}, {0x115bc, 0x115bd, SIGMA_CONTEXT_IGNORABLE},
  {0x115bf, 0x115c0, SIGMA_CONTEXT_IGNORABLE}, {0x115dc, 0x115dd, SIGMA_CONTEXT_IGNORABLE}, {0x11633, 0x1163a, SIGMA_CONTEXT_IGNORABLE},
  {0x1163d, 0x1163d, SIGMA_CONTEXT_IGNORABLE}, {0x1163f, 0x11640, SIGMA_CONTEXT_IGNORABLE}, {0x116ab, 0x116ab, SIGMA_CONTEXT_IGNORABLE},
  {0x116ad, 0x116ad, SIGMA_CONTEXT_IGNORABLE}, {0x116b0, 0x116b5, SIGMA_CONTEXT_IGNORABLE}, {0x116b7, 0x116b7, SIGMA_CONTEXT_IGNORABLE},
  {0x1171d, 0x1171d, SIGMA_CONTEXT_IGNORABLE}, {0x1171f, 0x1171f, SIGMA_CONTEXT_IGNORABLE}, {0x11722, 0x11725, SIGMA_CONTEXT_IGNORABLE},
  {0x11727, 0x1172b, SIGMA_CONTEXT_IGNORABLE}, {0x1182f, 0x11837, SIGMA_CONTEXT_IGNORABLE}, {0x11839, 0x1183a, SIGMA_CONTEXT_IGNORABLE},
  {0x118a0, 0x118df, SIGMA_CONTEXT_CASED}, {0x1193b, 0x1193c, SIGMA_CONTEXT_IGNORABLE}, {0x1193e, 0x1193e, SIGMA_CONTEXT_IGNORABLE},
  {0x11943, 0x11943, SIGMA_CONTEXT_IGNORABLE}, {0x119d4, 0x119d7, SIGMA_CONTEXT_IGNORABLE}, {0x119da, 0x119db, SIGMA_CONTEXT_IGNORABLE},
  {0x119e0, 0x119e0, SIGMA_CONTEXT_IGNORABLE}, {0x11a01, 0x11a0a, SIGMA_CONTEXT_IGNORABLE}, {0x11a33, 0x11a38, SIGMA_CONTEXT_IGNORABLE},
  {0x11a3b, 0x11a3e, SIGMA_CONTEXT_IGNORABLE}, {0x11a47, 0x11a47, SIGMA_CONTEXT_IGNORABLE}, {0x11a51, 0x11a56, SIGMA_CONTEXT_IGNORABLE},
  {0x11a59, 0x11a5b, SIGMA_CONTEXT_IGNORABLE}, {0x11a8a, 0x11a96, SIGMA_CONTEXT_IGNORABLE}, {0x11a98, 0x11a99, SIGMA_CONTEXT_IGNORABLE},
  {0x11b60, 0x11b60, SIGMA_CONTEXT_IGNORABLE}, {0x11b62, 0x11b64, SIGMA_CONTEXT_IGNORABLE}, {0x11b66, 0x11b66, SIGMA_CONTEXT_IGNORABLE},
  {0x11c30, 0x11c36, SIGMA_CONTEXT_IGNORABLE}, {0x11c38, 0x11c3d, SIGMA_CONTEXT_IGNORABLE}, {0x11c3f, 0x11c3f, SIGMA_CONTEXT_IGNORABLE},
  {0x11c92, 0x11ca7, SIGMA_CONTEXT_IGNORABLE}, {0x11caa, 0x11cb0, SIGMA_CONTEXT_IGNORABLE}, {0x11cb2, 0x11cb3, SIGMA_CONTEXT_IGNORABLE},
  {0x11cb5, 0x11cb6, SIGMA_CONTEXT_IGNORABLE}, {0x11d31, 0x11d36, SIGMA_CONTEXT_IGNORABLE}, {0x11d3a, 0x11d3a, SIGMA_CONTEXT_IGNORABLE},
  {0x11d3c, 0x11d3d, SIGMA_CONTEXT_IGNORABLE}, {0x11d3f, 0x11d45, SIGMA_CONTEXT_IGNORABLE}, {0x11d47, 0x11d47, SIGMA_CONTEXT_IGNORABLE},
  {0x11d90, 0x11d91, SIGMA_CONTEXT_IGNORABLE}, {0x11d95, 0x11d95, SIGMA_CONTEXT_IGNORABLE}, {0x11d97, 0x11d97, SIGMA_CONTEXT_IGNORABLE},
  {0x11dd9, 0x11dd9, SIGMA_CONTEXT_IGNORABLE}, {0x11ef3, 0x11ef4, SIGMA_CONTEXT_IGNORABLE}, {0x11f00, 0x11f01, SIGMA_CONTEXT_IGNORABLE},
  {0x11f36, 0x11f3a, SIGMA_CONTEXT_IGNORABLE}, {0x11f40, 0x11f40, SIGMA_CONTEXT_IGNORABLE}, {0x11f42, 0x11f42, SIGMA_CONTEXT_IGNORABLE},
  {0x11f5a, 0x11f5a, SIGMA_CONTEXT_IGNORABLE}, {0x13430, 0x13440, SIGMA_CONTEXT_IGNORABLE}, {0x13447, 0x13455, SIGMA_CONTEXT_IGNORABLE},
  {0x1611e, 0x16129, SIGMA_CONTEXT_IGNORABLE}, {0x1612d, 0x1612f, SIGMA_CONTEXT_IGNORABLE}, {0x16af0, 0x16af4, SIGMA_CONTEXT_IGNORABLE},
  {0x16b30, 0x16b36, SIGMA_CONTEXT_IGNORABLE}, {0x16b40, 0x16b43, SIGMA_CONTEXT_IGNORABLE}, {0x16d40, 0x16d42, SIGMA_CONTEXT_IGNORABLE},
  {0x16d6b, 0x16d6c, SIGMA_CONTEXT_IGNORABLE}, {0x16e40, 0x16e7f, SIGMA_CONTEXT_CASED}, {0x16ea0, 0x16eb8, SIGMA_CONTEXT_CASED},
  {0x16ebb, 0x16ed3, SIGMA_CONTEXT_CASED}, {0x16f4f, 0x16f4f, SIGMA_CONTEXT_IGNORABLE}, {0x16f8f, 0x16f9f, SIGMA_CONTEXT_IGNORABLE},
  {0x16fe0, 0x16fe1, SIGMA_CONTEXT_IGNORABLE}, {0x16fe3, 0x16fe4, SIGMA_CONTEXT_IGNORABLE}, {0x16ff2, 0x16ff3, SIGMA_CONTEXT_IGNORABLE},
  {0x1aff0, 0x1aff3, SIGMA_CONTEXT_IGNORABLE}, {0x1aff5, 0x1affb, SIGMA_CONTEXT_IGNORABLE}, {0x1affd, 0x1affe, SIGMA_CONTEXT_IGNORABLE},
  {0x1bc9d, 0x1bc9e, SIGMA_CONTEXT_IGNORABLE}, {0x1bca0, 0x1bca3, SIGMA_CONTEXT_IGNORABLE}, {0x1cf00, 0x1cf2d, SIGMA_CONTEXT_IGNORABLE},
  {0x1cf30, 0x1cf46, SIGMA_CONTEXT_IGNORABLE}, {0x1d167, 0x1d169, SIGMA_CONTEXT_IGNORABLE}, {0x1d173, 0x1d182, SIGMA_CONTEXT_IGNORABLE},
  {0x1d185, 0x1d18b, SIGMA_CONTEXT_IGNORABLE}, {0x1d1aa, 0x1d1ad, SIGMA_CONTEXT_IGNORABLE}, {0x1d242, 0x1d244, SIGMA_CONTEXT_IGNORABLE},
  {0x1d400, 0x1d454, SIGMA_CONTEXT_CASED}, {0x1d456, 0x1d49c, SIGMA_CONTEXT_CASED}, {0x1d49e, 0x1d49f, SIGMA_CONTEXT_CASED},
  {0x1d4a2, 0x1d4a2, SIGMA_CONTEXT_CASED}, {0x1d4a5, 0x1d4a6, SIGMA_CONTEXT_CASED}, {0x1d4a9, 0x1d4ac, SIGMA_CONTEXT_CASED},
  {0x1d4ae, 0x1d4b9, SIGMA_CONTEXT_CASED}, {0x1d4bb, 0x1d4bb, SIGMA_CONTEXT_CASED}, {0x1d4bd, 0x1d4c3, SIGMA_CONTEXT_CASED},
  {0x1d4c5, 0x1d505, SIGMA_CONTEXT_CASED}, {0x1d507, 0x1d50a, SIGMA_CONTEXT_CASED}, {0x1d50d, 0x1d514, SIGMA_CONTEXT_CASED},
  {0x1d516, 0x1d51c, SIGMA_CONTEXT_CASED}, {0x1d51e, 0x1d539, SIGMA_CONTEXT_CASED}, {0x1d53b, 0x1d53e, SIGMA_CONTEXT_CASED},
  {0x1d540, 0x1d544, SIGMA_CONTEXT_CASED}, {0x1d546, 0x1d546, SIGMA_CONTEXT_CASED}, {0x1d54a, 0x1d550, SIGMA_CONTEXT_CASED},
  {0x1d552, 0x1d6a5, SIGMA_CONTEXT_CASED}, {0x1d6a8, 0x1d6c0, SIGMA_CONTEXT_CASED}, {0x1d6c2, 0x1d6da, SIGMA_CONTEXT_CASED},
  {0x1d6dc, 0x1d6fa, SIGMA_CONTEXT_CASED}, {0x1d6fc, 0x1d714, SIGMA_CONTEXT_CASED}, {0x1d716, 0x1d734, SIGMA_CONTEXT_CASED},
  {0x1d736, 0x1d74e, SIGMA_CONTEXT_CASED}, {0x1d750, 0x1d76e, SIGMA_CONTEXT_CASED}, {0x1d770, 0x1d788, SIGMA_CONTEXT_CASED},
  {0x1d78a, 0x1d7a8, SIGMA_CONTEXT_CASED}, {0x1d7aa, 0x1d7c2, SIGMA_CONTEXT_CASED}, {0x1d7c4, 0x1d7cb, SIGMA_CONTEXT_CASED},
  {0x1da00, 0x1da36, SIGMA_CONTEXT_IGNORABLE}, {0x1da3b, 0x1da6c, SIGMA_CONTEXT_IGNORABLE}, {0x1da75, 0x1da75, SIGMA_CONTEXT_IGNORABLE},
  {0x1da84, 0x1da84, SIGMA_CONTEXT_IGNORABLE}, {0x1da9b, 0x1da9f, SIGMA_CONTEXT_IGNORABLE}, {0x1daa1, 0x1daaf, SIGMA_CONTEXT_IGNORABLE},
  {0x1df00, 0x1df09, SIGMA_CONTEXT_CASED}, {0x1df0b, 0x1df1e, SIGMA_CONTEXT_CASED}, {0x1df25, 0x1df2a, SIGMA_CONTEXT_CASED},
  {0x1e000, 0x1e006, SIGMA_CONTEXT_IGNORABLE}, {0x1e008, 0x1e018, SIGMA_CONTEXT_IGNORABLE}, {0x1e01b, 0x1e021, SIGMA_CONTEXT_IGNORABLE},
  {0x1e023, 0x1e024, SIGMA_CONTEXT_IGNORABLE}, {0x1e026, 0x1e02a, SIGMA_CONTEXT_IGNORABLE}, {0x1e030, 0x1e06d, SIGMA_CONTEXT_IGNORABLE},
  {0x1e08f, 0x1e08f, SIGMA_CONTEXT_IGNORABLE}, {0x1e130, 0x1e13d, SIGMA_CONTEXT_IGNORABLE}, {0x1e2ae, 0x1e2ae, SIGMA_CONTEXT_IGNORABLE},
  {0x1e2ec, 0x1e2ef, SIGMA_CONTEXT_IGNORABLE}, {0x1e4eb, 0x1e4ef, SIGMA_CONTEXT_IGNORABLE}, {0x1e5ee, 0x1e5ef, SIGMA_CONTEXT_IGNORABLE},
  {0x1e6e3, 0x1e6e3, SIGMA_CONTEXT_IGNORABLE}, {0x1e6e6, 0x1e6e6, SIGMA_CONTEXT_IGNORABLE}, {0x1e6ee, 0x1e6ef, SIGMA_CONTEXT_IGNORABLE},
  {0x1e6f5, 0x1e6f5, SIGMA_CONTEXT_IGNORABLE}, {0x1e6ff, 0x1e6ff, SIGMA_CONTEXT_IGNORABLE}, {0x1e8d0, 0x1e8d6, SIGMA_CONTEXT_IGNORABLE},
  {0x1e900, 0x1e943, SIGMA_CONTEXT_CASED}, {0x1e944, 0x1e94b, SIGMA_CONTEXT_IGNORABLE}, {0x1f130, 0x1f149, SIGMA_CONTEXT_CASED},
  {0x1f150, 0x1f169, SIGMA_CONTEXT_CASED}, {0x1f170, 0x1f189, SIGMA_CONTEXT_CASED}, {0x1f3fb, 0x1f3ff, SIGMA_CONTEXT_IGNORABLE},
  {0xe0001, 0xe0001, SIGMA_CONTEXT_IGNORABLE}, {0xe0020, 0xe007f, SIGMA_CONTEXT_IGNORABLE}, {0xe0100, 0xe01ef, SIGMA_CONTEXT_IGNORABLE},
};
/* END GENERATED unicode case tables */
#define CAPITAL_SIGMA 0x3a3
#define SMALL_SIGMA 0x3c3
#define SMALL_FINAL_SIGMA 0x3c2
#define COUNT_OF(table) (sizeof (table) / sizeof (table)[0])
static const CaseMapping *find_case_mapping(const CaseMapping *table, size_t count, uint32_t code) {
  size_t low = 0, high = count;
  while (low < high) {
    size_t middle = low + (high - low) / 2;
    if (table[middle].code < code) low = middle + 1; else high = middle;
  }
  return low < count && table[low].code == code ? &table[low] : NULL;
}
static uint8_t sigma_context(uint32_t code) {
  size_t low = 0, high = COUNT_OF(sigma_context_ranges);
  while (low < high) {
    size_t middle = low + (high - low) / 2;
    if (sigma_context_ranges[middle].last < code) low = middle + 1; else high = middle;
  }
  if (low < COUNT_OF(sigma_context_ranges) && sigma_context_ranges[low].first <= code)
    return sigma_context_ranges[low].context;
  return SIGMA_CONTEXT_OTHER;
}
/* Final_Sigma as Rust tests it: skip Case_Ignorable characters, then ask whether the
   next character is Cased. `before` scans backwards from `offset`, otherwise forwards. */
static bool cased_beyond_ignorables(const char *text, size_t length, size_t offset, bool before) {
  size_t width;
  while (before ? offset > 0 : offset < length) {
    size_t start = offset;
    if (before) { start = offset - 1; while (start > 0 && !utf8_boundary(text, length, start)) --start; }
    uint8_t context = sigma_context(utf8_decode((const unsigned char *)text + start, &width));
    if (context != SIGMA_CONTEXT_IGNORABLE) return context == SIGMA_CONTEXT_CASED;
    offset = before ? start : start + width;
  }
  return false;
}
static ChimiValue *text_case_map(const ChimiValue *value, const CaseMapping *table, size_t count,
                                 bool final_sigma) {
  const char *text = value->as.text.bytes;
  size_t length = value->as.text.length;
  TextBuffer buffer = {0};
  for (size_t offset = 0, width; offset < length; offset += width) {
    uint32_t code = utf8_decode((const unsigned char *)text + offset, &width);
    if (final_sigma && code == CAPITAL_SIGMA) {
      bool is_final = cased_beyond_ignorables(text, length, offset, true) &&
                      !cased_beyond_ignorables(text, length, offset + width, false);
      buffer_utf8(&buffer, is_final ? SMALL_FINAL_SIGMA : SMALL_SIGMA);
      continue;
    }
    const CaseMapping *mapping = find_case_mapping(table, count, code);
    if (!mapping) { buffer_bytes(&buffer, text + offset, width); continue; }
    for (int i = 0; i < 3 && mapping->mapped[i]; ++i) buffer_utf8(&buffer, mapping->mapped[i]);
  }
  return buffer.data ? text_from_bytes(buffer.data, buffer.length) : text_value("");
}

/* Extended grapheme clusters (Unicode UAX #29) for Text.graphemes and the functions
   that count in graphemes. The interpreter's src/graphemes.rs implements the same
   rules over the same generated table. */
enum {
  GB_OTHER, GB_CR, GB_LF, GB_CONTROL, GB_EXTEND, GB_ZWJ, GB_REGIONAL_INDICATOR, GB_PREPEND,
  GB_SPACING_MARK, GB_L, GB_V, GB_T, GB_LV, GB_LVT
};
enum { INCB_NONE, INCB_LINKER, INCB_CONSONANT, INCB_EXTEND };
typedef struct { uint32_t first, last; uint8_t grapheme_break, conjunct, pictographic; } GraphemeRange;
/* BEGIN GENERATED grapheme table (tests/unicode_tables.rs, Unicode 17.0.0) */
static const GraphemeRange grapheme_ranges[] = {
  {0x0, 0x9, GB_CONTROL, INCB_NONE, 0}, {0xa, 0xa, GB_LF, INCB_NONE, 0}, {0xb, 0xc, GB_CONTROL, INCB_NONE, 0},
  {0xd, 0xd, GB_CR, INCB_NONE, 0}, {0xe, 0x1f, GB_CONTROL, INCB_NONE, 0}, {0x7f, 0x9f, GB_CONTROL, INCB_NONE, 0},
  {0xa9, 0xa9, GB_OTHER, INCB_NONE, 1}, {0xad, 0xad, GB_CONTROL, INCB_NONE, 0}, {0xae, 0xae, GB_OTHER, INCB_NONE, 1},
  {0x300, 0x36f, GB_EXTEND, INCB_EXTEND, 0}, {0x483, 0x489, GB_EXTEND, INCB_EXTEND, 0}, {0x591, 0x5bd, GB_EXTEND, INCB_EXTEND, 0},
  {0x5bf, 0x5bf, GB_EXTEND, INCB_EXTEND, 0}, {0x5c1, 0x5c2, GB_EXTEND, INCB_EXTEND, 0}, {0x5c4, 0x5c5, GB_EXTEND, INCB_EXTEND, 0},
  {0x5c7, 0x5c7, GB_EXTEND, INCB_EXTEND, 0}, {0x600, 0x605, GB_PREPEND, INCB_NONE, 0}, {0x610, 0x61a, GB_EXTEND, INCB_EXTEND, 0},
  {0x61c, 0x61c, GB_CONTROL, INCB_NONE, 0}, {0x64b, 0x65f, GB_EXTEND, INCB_EXTEND, 0}, {0x670, 0x670, GB_EXTEND, INCB_EXTEND, 0},
  {0x6d6, 0x6dc, GB_EXTEND, INCB_EXTEND, 0}, {0x6dd, 0x6dd, GB_PREPEND, INCB_NONE, 0}, {0x6df, 0x6e4, GB_EXTEND, INCB_EXTEND, 0},
  {0x6e7, 0x6e8, GB_EXTEND, INCB_EXTEND, 0}, {0x6ea, 0x6ed, GB_EXTEND, INCB_EXTEND, 0}, {0x70f, 0x70f, GB_PREPEND, INCB_NONE, 0},
  {0x711, 0x711, GB_EXTEND, INCB_EXTEND, 0}, {0x730, 0x74a, GB_EXTEND, INCB_EXTEND, 0}, {0x7a6, 0x7b0, GB_EXTEND, INCB_EXTEND, 0},
  {0x7eb, 0x7f3, GB_EXTEND, INCB_EXTEND, 0}, {0x7fd, 0x7fd, GB_EXTEND, INCB_EXTEND, 0}, {0x816, 0x819, GB_EXTEND, INCB_EXTEND, 0},
  {0x81b, 0x823, GB_EXTEND, INCB_EXTEND, 0}, {0x825, 0x827, GB_EXTEND, INCB_EXTEND, 0}, {0x829, 0x82d, GB_EXTEND, INCB_EXTEND, 0},
  {0x859, 0x85b, GB_EXTEND, INCB_EXTEND, 0}, {0x890, 0x891, GB_PREPEND, INCB_NONE, 0}, {0x897, 0x89f, GB_EXTEND, INCB_EXTEND, 0},
  {0x8ca, 0x8e1, GB_EXTEND, INCB_EXTEND, 0}, {0x8e2, 0x8e2, GB_PREPEND, INCB_NONE, 0}, {0x8e3, 0x902, GB_EXTEND, INCB_EXTEND, 0},
  {0x903, 0x903, GB_SPACING_MARK, INCB_NONE, 0}, {0x915, 0x939, GB_OTHER, INCB_CONSONANT, 0}, {0x93a, 0x93a, GB_EXTEND, INCB_EXTEND, 0},
  {0x93b, 0x93b, GB_SPACING_MARK, INCB_NONE, 0}, {0x93c, 0x93c, GB_EXTEND, INCB_EXTEND, 0}, {0x93e, 0x940, GB_SPACING_MARK, INCB_NONE, 0},
  {0x941, 0x948, GB_EXTEND, INCB_EXTEND, 0}, {0x949, 0x94c, GB_SPACING_MARK, INCB_NONE, 0}, {0x94d, 0x94d, GB_EXTEND, INCB_LINKER, 0},
  {0x94e, 0x94f, GB_SPACING_MARK, INCB_NONE, 0}, {0x951, 0x957, GB_EXTEND, INCB_EXTEND, 0}, {0x958, 0x95f, GB_OTHER, INCB_CONSONANT, 0},
  {0x962, 0x963, GB_EXTEND, INCB_EXTEND, 0}, {0x978, 0x97f, GB_OTHER, INCB_CONSONANT, 0}, {0x981, 0x981, GB_EXTEND, INCB_EXTEND, 0},
  {0x982, 0x983, GB_SPACING_MARK, INCB_NONE, 0}, {0x995, 0x9a8, GB_OTHER, INCB_CONSONANT, 0}, {0x9aa, 0x9b0, GB_OTHER, INCB_CONSONANT, 0},
  {0x9b2, 0x9b2, GB_OTHER, INCB_CONSONANT, 0}, {0x9b6, 0x9b9, GB_OTHER, INCB_CONSONANT, 0}, {0x9bc, 0x9bc, GB_EXTEND, INCB_EXTEND, 0},
  {0x9be, 0x9be, GB_EXTEND, INCB_EXTEND, 0}, {0x9bf, 0x9c0, GB_SPACING_MARK, INCB_NONE, 0}, {0x9c1, 0x9c4, GB_EXTEND, INCB_EXTEND, 0},
  {0x9c7, 0x9c8, GB_SPACING_MARK, INCB_NONE, 0}, {0x9cb, 0x9cc, GB_SPACING_MARK, INCB_NONE, 0}, {0x9cd, 0x9cd, GB_EXTEND, INCB_LINKER, 0},
  {0x9d7, 0x9d7, GB_EXTEND, INCB_EXTEND, 0}, {0x9dc, 0x9dd, GB_OTHER, INCB_CONSONANT, 0}, {0x9df, 0x9df, GB_OTHER, INCB_CONSONANT, 0},
  {0x9e2, 0x9e3, GB_EXTEND, INCB_EXTEND, 0}, {0x9f0, 0x9f1, GB_OTHER, INCB_CONSONANT, 0}, {0x9fe, 0x9fe, GB_EXTEND, INCB_EXTEND, 0},
  {0xa01, 0xa02, GB_EXTEND, INCB_EXTEND, 0}, {0xa03, 0xa03, GB_SPACING_MARK, INCB_NONE, 0}, {0xa3c, 0xa3c, GB_EXTEND, INCB_EXTEND, 0},
  {0xa3e, 0xa40, GB_SPACING_MARK, INCB_NONE, 0}, {0xa41, 0xa42, GB_EXTEND, INCB_EXTEND, 0}, {0xa47, 0xa48, GB_EXTEND, INCB_EXTEND, 0},
  {0xa4b, 0xa4d, GB_EXTEND, INCB_EXTEND, 0}, {0xa51, 0xa51, GB_EXTEND, INCB_EXTEND, 0}, {0xa70, 0xa71, GB_EXTEND, INCB_EXTEND, 0},
  {0xa75, 0xa75, GB_EXTEND, INCB_EXTEND, 0}, {0xa81, 0xa82, GB_EXTEND, INCB_EXTEND, 0}, {0xa83, 0xa83, GB_SPACING_MARK, INCB_NONE, 0},
  {0xa95, 0xaa8, GB_OTHER, INCB_CONSONANT, 0}, {0xaaa, 0xab0, GB_OTHER, INCB_CONSONANT, 0}, {0xab2, 0xab3, GB_OTHER, INCB_CONSONANT, 0},
  {0xab5, 0xab9, GB_OTHER, INCB_CONSONANT, 0}, {0xabc, 0xabc, GB_EXTEND, INCB_EXTEND, 0}, {0xabe, 0xac0, GB_SPACING_MARK, INCB_NONE, 0},
  {0xac1, 0xac5, GB_EXTEND, INCB_EXTEND, 0}, {0xac7, 0xac8, GB_EXTEND, INCB_EXTEND, 0}, {0xac9, 0xac9, GB_SPACING_MARK, INCB_NONE, 0},
  {0xacb, 0xacc, GB_SPACING_MARK, INCB_NONE, 0}, {0xacd, 0xacd, GB_EXTEND, INCB_LINKER, 0}, {0xae2, 0xae3, GB_EXTEND, INCB_EXTEND, 0},
  {0xaf9, 0xaf9, GB_OTHER, INCB_CONSONANT, 0}, {0xafa, 0xaff, GB_EXTEND, INCB_EXTEND, 0}, {0xb01, 0xb01, GB_EXTEND, INCB_EXTEND, 0},
  {0xb02, 0xb03, GB_SPACING_MARK, INCB_NONE, 0}, {0xb15, 0xb28, GB_OTHER, INCB_CONSONANT, 0}, {0xb2a, 0xb30, GB_OTHER, INCB_CONSONANT, 0},
  {0xb32, 0xb33, GB_OTHER, INCB_CONSONANT, 0}, {0xb35, 0xb39, GB_OTHER, INCB_CONSONANT, 0}, {0xb3c, 0xb3c, GB_EXTEND, INCB_EXTEND, 0},
  {0xb3e, 0xb3f, GB_EXTEND, INCB_EXTEND, 0}, {0xb40, 0xb40, GB_SPACING_MARK, INCB_NONE, 0}, {0xb41, 0xb44, GB_EXTEND, INCB_EXTEND, 0},
  {0xb47, 0xb48, GB_SPACING_MARK, INCB_NONE, 0}, {0xb4b, 0xb4c, GB_SPACING_MARK, INCB_NONE, 0}, {0xb4d, 0xb4d, GB_EXTEND, INCB_LINKER, 0},
  {0xb55, 0xb57, GB_EXTEND, INCB_EXTEND, 0}, {0xb5c, 0xb5d, GB_OTHER, INCB_CONSONANT, 0}, {0xb5f, 0xb5f, GB_OTHER, INCB_CONSONANT, 0},
  {0xb62, 0xb63, GB_EXTEND, INCB_EXTEND, 0}, {0xb71, 0xb71, GB_OTHER, INCB_CONSONANT, 0}, {0xb82, 0xb82, GB_EXTEND, INCB_EXTEND, 0},
  {0xbbe, 0xbbe, GB_EXTEND, INCB_EXTEND, 0}, {0xbbf, 0xbbf, GB_SPACING_MARK, INCB_NONE, 0}, {0xbc0, 0xbc0, GB_EXTEND, INCB_EXTEND, 0},
  {0xbc1, 0xbc2, GB_SPACING_MARK, INCB_NONE, 0}, {0xbc6, 0xbc8, GB_SPACING_MARK, INCB_NONE, 0}, {0xbca, 0xbcc, GB_SPACING_MARK, INCB_NONE, 0},
  {0xbcd, 0xbcd, GB_EXTEND, INCB_EXTEND, 0}, {0xbd7, 0xbd7, GB_EXTEND, INCB_EXTEND, 0}, {0xc00, 0xc00, GB_EXTEND, INCB_EXTEND, 0},
  {0xc01, 0xc03, GB_SPACING_MARK, INCB_NONE, 0}, {0xc04, 0xc04, GB_EXTEND, INCB_EXTEND, 0}, {0xc15, 0xc28, GB_OTHER, INCB_CONSONANT, 0},
  {0xc2a, 0xc39, GB_OTHER, INCB_CONSONANT, 0}, {0xc3c, 0xc3c, GB_EXTEND, INCB_EXTEND, 0}, {0xc3e, 0xc40, GB_EXTEND, INCB_EXTEND, 0},
  {0xc41, 0xc44, GB_SPACING_MARK, INCB_NONE, 0}, {0xc46, 0xc48, GB_EXTEND, INCB_EXTEND, 0}, {0xc4a, 0xc4c, GB_EXTEND, INCB_EXTEND, 0},
  {0xc4d, 0xc4d, GB_EXTEND, INCB_LINKER, 0}, {0xc55, 0xc56, GB_EXTEND, INCB_EXTEND, 0}, {0xc58, 0xc5a, GB_OTHER, INCB_CONSONANT, 0},
  {0xc62, 0xc63, GB_EXTEND, INCB_EXTEND, 0}, {0xc81, 0xc81, GB_EXTEND, INCB_EXTEND, 0}, {0xc82, 0xc83, GB_SPACING_MARK, INCB_NONE, 0},
  {0xcbc, 0xcbc, GB_EXTEND, INCB_EXTEND, 0}, {0xcbe, 0xcbe, GB_SPACING_MARK, INCB_NONE, 0}, {0xcbf, 0xcc0, GB_EXTEND, INCB_EXTEND, 0},
  {0xcc1, 0xcc1, GB_SPACING_MARK, INCB_NONE, 0}, {0xcc2, 0xcc2, GB_EXTEND, INCB_EXTEND, 0}, {0xcc3, 0xcc4, GB_SPACING_MARK, INCB_NONE, 0},
  {0xcc6, 0xcc8, GB_EXTEND, INCB_EXTEND, 0}, {0xcca, 0xccd, GB_EXTEND, INCB_EXTEND, 0}, {0xcd5, 0xcd6, GB_EXTEND, INCB_EXTEND, 0},
  {0xce2, 0xce3, GB_EXTEND, INCB_EXTEND, 0}, {0xcf3, 0xcf3, GB_SPACING_MARK, INCB_NONE, 0}, {0xd00, 0xd01, GB_EXTEND, INCB_EXTEND, 0},
  {0xd02, 0xd03, GB_SPACING_MARK, INCB_NONE, 0}, {0xd15, 0xd3a, GB_OTHER, INCB_CONSONANT, 0}, {0xd3b, 0xd3c, GB_EXTEND, INCB_EXTEND, 0},
  {0xd3e, 0xd3e, GB_EXTEND, INCB_EXTEND, 0}, {0xd3f, 0xd40, GB_SPACING_MARK, INCB_NONE, 0}, {0xd41, 0xd44, GB_EXTEND, INCB_EXTEND, 0},
  {0xd46, 0xd48, GB_SPACING_MARK, INCB_NONE, 0}, {0xd4a, 0xd4c, GB_SPACING_MARK, INCB_NONE, 0}, {0xd4d, 0xd4d, GB_EXTEND, INCB_LINKER, 0},
  {0xd4e, 0xd4e, GB_PREPEND, INCB_NONE, 0}, {0xd57, 0xd57, GB_EXTEND, INCB_EXTEND, 0}, {0xd62, 0xd63, GB_EXTEND, INCB_EXTEND, 0},
  {0xd81, 0xd81, GB_EXTEND, INCB_EXTEND, 0}, {0xd82, 0xd83, GB_SPACING_MARK, INCB_NONE, 0}, {0xdca, 0xdca, GB_EXTEND, INCB_EXTEND, 0},
  {0xdcf, 0xdcf, GB_EXTEND, INCB_EXTEND, 0}, {0xdd0, 0xdd1, GB_SPACING_MARK, INCB_NONE, 0}, {0xdd2, 0xdd4, GB_EXTEND, INCB_EXTEND, 0},
  {0xdd6, 0xdd6, GB_EXTEND, INCB_EXTEND, 0}, {0xdd8, 0xdde, GB_SPACING_MARK, INCB_NONE, 0}, {0xddf, 0xddf, GB_EXTEND, INCB_EXTEND, 0},
  {0xdf2, 0xdf3, GB_SPACING_MARK, INCB_NONE, 0}, {0xe31, 0xe31, GB_EXTEND, INCB_EXTEND, 0}, {0xe33, 0xe33, GB_SPACING_MARK, INCB_NONE, 0},
  {0xe34, 0xe3a, GB_EXTEND, INCB_EXTEND, 0}, {0xe47, 0xe4e, GB_EXTEND, INCB_EXTEND, 0}, {0xeb1, 0xeb1, GB_EXTEND, INCB_EXTEND, 0},
  {0xeb3, 0xeb3, GB_SPACING_MARK, INCB_NONE, 0}, {0xeb4, 0xebc, GB_EXTEND, INCB_EXTEND, 0}, {0xec8, 0xece, GB_EXTEND, INCB_EXTEND, 0},
  {0xf18, 0xf19, GB_EXTEND, INCB_EXTEND, 0}, {0xf35, 0xf35, GB_EXTEND, INCB_EXTEND, 0}, {0xf37, 0xf37, GB_EXTEND, INCB_EXTEND, 0},
  {0xf39, 0xf39, GB_EXTEND, INCB_EXTEND, 0}, {0xf3e, 0xf3f, GB_SPACING_MARK, INCB_NONE, 0}, {0xf71, 0xf7e, GB_EXTEND, INCB_EXTEND, 0},
  {0xf7f, 0xf7f, GB_SPACING_MARK, INCB_NONE, 0}, {0xf80, 0xf84, GB_EXTEND, INCB_EXTEND, 0}, {0xf86, 0xf87, GB_EXTEND, INCB_EXTEND, 0},
  {0xf8d, 0xf97, GB_EXTEND, INCB_EXTEND, 0}, {0xf99, 0xfbc, GB_EXTEND, INCB_EXTEND, 0}, {0xfc6, 0xfc6, GB_EXTEND, INCB_EXTEND, 0},
  {0x1000, 0x102a, GB_OTHER, INCB_CONSONANT, 0}, {0x102d, 0x1030, GB_EXTEND, INCB_EXTEND, 0}, {0x1031, 0x1031, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1032, 0x1037, GB_EXTEND, INCB_EXTEND, 0}, {0x1039, 0x1039, GB_EXTEND, INCB_LINKER, 0}, {0x103a, 0x103a, GB_EXTEND, INCB_EXTEND, 0},
  {0x103b, 0x103c, GB_SPACING_MARK, INCB_NONE, 0}, {0x103d, 0x103e, GB_EXTEND, INCB_EXTEND, 0}, {0x103f, 0x103f, GB_OTHER, INCB_CONSONANT, 0},
  {0x1050, 0x1055, GB_OTHER, INCB_CONSONANT, 0}, {0x1056, 0x1057, GB_SPACING_MARK, INCB_NONE, 0}, {0x1058, 0x1059, GB_EXTEND, INCB_EXTEND, 0},
  {0x105a, 0x105d, GB_OTHER, INCB_CONSONANT, 0}, {0x105e, 0x1060, GB_EXTEND, INCB_EXTEND, 0}, {0x1061, 0x1061, GB_OTHER, INCB_CONSONANT, 0},
  {0x1065, 0x1066, GB_OTHER, INCB_CONSONANT, 0}, {0x106e, 0x1070, GB_OTHER, INCB_CONSONANT, 0}, {0x1071, 0x1074, GB_EXTEND, INCB_EXTEND, 0},
  {0x1075, 0x1081, GB_OTHER, INCB_CONSONANT, 0}, {0x1082, 0x1082, GB_EXTEND, INCB_EXTEND, 0}, {0x1084, 0x1084, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1085, 0x1086, GB_EXTEND, INCB_EXTEND, 0}, {0x108d, 0x108d, GB_EXTEND, INCB_EXTEND, 0}, {0x108e, 0x108e, GB_OTHER, INCB_CONSONANT, 0},
  {0x109d, 0x109d, GB_EXTEND, INCB_EXTEND, 0}, {0x1100, 0x115f, GB_L, INCB_NONE, 0}, {0x1160, 0x11a7, GB_V, INCB_NONE, 0},
  {0x11a8, 0x11ff, GB_T, INCB_NONE, 0}, {0x135d, 0x135f, GB_EXTEND, INCB_EXTEND, 0}, {0x1712, 0x1715, GB_EXTEND, INCB_EXTEND, 0},
  {0x1732, 0x1734, GB_EXTEND, INCB_EXTEND, 0}, {0x1752, 0x1753, GB_EXTEND, INCB_EXTEND, 0}, {0x1772, 0x1773, GB_EXTEND, INCB_EXTEND, 0},
  {0x1780, 0x17b3, GB_OTHER, INCB_CONSONANT, 0}, {0x17b4, 0x17b5, GB_EXTEND, INCB_EXTEND, 0}, {0x17b6, 0x17b6, GB_SPACING_MARK, INCB_NONE, 0},
  {0x17b7, 0x17bd, GB_EXTEND, INCB_EXTEND, 0}, {0x17be, 0x17c5, GB_SPACING_MARK, INCB_NONE, 0}, {0x17c6, 0x17c6, GB_EXTEND, INCB_EXTEND, 0},
  {0x17c7, 0x17c8, GB_SPACING_MARK, INCB_NONE, 0}, {0x17c9, 0x17d1, GB_EXTEND, INCB_EXTEND, 0}, {0x17d2, 0x17d2, GB_EXTEND, INCB_LINKER, 0},
  {0x17d3, 0x17d3, GB_EXTEND, INCB_EXTEND, 0}, {0x17dd, 0x17dd, GB_EXTEND, INCB_EXTEND, 0}, {0x180b, 0x180d, GB_EXTEND, INCB_EXTEND, 0},
  {0x180e, 0x180e, GB_CONTROL, INCB_NONE, 0}, {0x180f, 0x180f, GB_EXTEND, INCB_EXTEND, 0}, {0x1885, 0x1886, GB_EXTEND, INCB_EXTEND, 0},
  {0x18a9, 0x18a9, GB_EXTEND, INCB_EXTEND, 0}, {0x1920, 0x1922, GB_EXTEND, INCB_EXTEND, 0}, {0x1923, 0x1926, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1927, 0x1928, GB_EXTEND, INCB_EXTEND, 0}, {0x1929, 0x192b, GB_SPACING_MARK, INCB_NONE, 0}, {0x1930, 0x1931, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1932, 0x1932, GB_EXTEND, INCB_EXTEND, 0}, {0x1933, 0x1938, GB_SPACING_MARK, INCB_NONE, 0}, {0x1939, 0x193b, GB_EXTEND, INCB_EXTEND, 0},
  {0x1a17, 0x1a18, GB_EXTEND, INCB_EXTEND, 0}, {0x1a19, 0x1a1a, GB_SPACING_MARK, INCB_NONE, 0}, {0x1a1b, 0x1a1b, GB_EXTEND, INCB_EXTEND, 0},
  {0x1a20, 0x1a54, GB_OTHER, INCB_CONSONANT, 0}, {0x1a55, 0x1a55, GB_SPACING_MARK, INCB_NONE, 0}, {0x1a56, 0x1a56, GB_EXTEND, INCB_EXTEND, 0},
  {0x1a57, 0x1a57, GB_SPACING_MARK, INCB_NONE, 0}, {0x1a58, 0x1a5e, GB_EXTEND, INCB_EXTEND, 0}, {0x1a60, 0x1a60, GB_EXTEND, INCB_LINKER, 0},
  {0x1a62, 0x1a62, GB_EXTEND, INCB_EXTEND, 0}, {0x1a65, 0x1a6c, GB_EXTEND, INCB_EXTEND, 0}, {0x1a6d, 0x1a72, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1a73, 0x1a7c, GB_EXTEND, INCB_EXTEND, 0}, {0x1a7f, 0x1a7f, GB_EXTEND, INCB_EXTEND, 0}, {0x1ab0, 0x1add, GB_EXTEND, INCB_EXTEND, 0},
  {0x1ae0, 0x1aeb, GB_EXTEND, INCB_EXTEND, 0}, {0x1b00, 0x1b03, GB_EXTEND, INCB_EXTEND, 0}, {0x1b04, 0x1b04, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1b0b, 0x1b0c, GB_OTHER, INCB_CONSONANT, 0}, {0x1b13, 0x1b33, GB_OTHER, INCB_CONSONANT, 0}, {0x1b34, 0x1b3d, GB_EXTEND, INCB_EXTEND, 0},
  {0x1b3e, 0x1b41, GB_SPACING_MARK, INCB_NONE, 0}, {0x1b42, 0x1b43, GB_EXTEND, INCB_EXTEND, 0}, {0x1b44, 0x1b44, GB_EXTEND, INCB_LINKER, 0},
  {0x1b45, 0x1b4c, GB_OTHER, INCB_CONSONANT, 0}, {0x1b6b, 0x1b73, GB_EXTEND, INCB_EXTEND, 0}, {0x1b80, 0x1b81, GB_EXTEND, INCB_EXTEND, 0},
  {0x1b82, 0x1b82, GB_SPACING_MARK, INCB_NONE, 0}, {0x1b83, 0x1ba0, GB_OTHER, INCB_CONSONANT, 0}, {0x1ba1, 0x1ba1, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1ba2, 0x1ba5, GB_EXTEND, INCB_EXTEND, 0}, {0x1ba6, 0x1ba7, GB_SPACING_MARK, INCB_NONE, 0}, {0x1ba8, 0x1baa, GB_EXTEND, INCB_EXTEND, 0},
  {0x1bab, 0x1bab, GB_EXTEND, INCB_LINKER, 0}, {0x1bac, 0x1bad, GB_EXTEND, INCB_EXTEND, 0}, {0x1bae, 0x1baf, GB_OTHER, INCB_CONSONANT, 0},
  {0x1bbb, 0x1bbd, GB_OTHER, INCB_CONSONANT, 0}, {0x1be6, 0x1be6, GB_EXTEND, INCB_EXTEND, 0}, {0x1be7, 0x1be7, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1be8, 0x1be9, GB_EXTEND, INCB_EXTEND, 0}, {0x1bea, 0x1bec, GB_SPACING_MARK, INCB_NONE, 0}, {0x1bed, 0x1bed, GB_EXTEND, INCB_EXTEND, 0},
  {0x1bee, 0x1bee, GB_SPACING_MARK, INCB_NONE, 0}, {0x1bef, 0x1bf3, GB_EXTEND, INCB_EXTEND, 0}, {0x1c24, 0x1c2b, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1c2c, 0x1c33, GB_EXTEND, INCB_EXTEND, 0}, {0x1c34, 0x1c35, GB_SPACING_MARK, INCB_NONE, 0}, {0x1c36, 0x1c37, GB_EXTEND, INCB_EXTEND, 0},
  {0x1cd0, 0x1cd2, GB_EXTEND, INCB_EXTEND, 0}, {0x1cd4, 0x1ce0, GB_EXTEND, INCB_EXTEND, 0}, {0x1ce1, 0x1ce1, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1ce2, 0x1ce8, GB_EXTEND, INCB_EXTEND, 0}, {0x1ced, 0x1ced, GB_EXTEND, INCB_EXTEND, 0}, {0x1cf4, 0x1cf4, GB_EXTEND, INCB_EXTEND, 0},
  {0x1cf7, 0x1cf7, GB_SPACING_MARK, INCB_NONE, 0}, {0x1cf8, 0x1cf9, GB_EXTEND, INCB_EXTEND, 0}, {0x1dc0, 0x1dff, GB_EXTEND, INCB_EXTEND, 0},
  {0x200b, 0x200b, GB_CONTROL, INCB_NONE, 0}, {0x200c, 0x200c, GB_EXTEND, INCB_NONE, 0}, {0x200d, 0x200d, GB_ZWJ, INCB_EXTEND, 0},
  {0x200e, 0x200f, GB_CONTROL, INCB_NONE, 0}, {0x2028, 0x202e, GB_CONTROL, INCB_NONE, 0}, {0x203c, 0x203c, GB_OTHER, INCB_NONE, 1},
  {0x2049, 0x2049, GB_OTHER, INCB_NONE, 1}, {0x2060, 0x206f, GB_CONTROL, INCB_NONE, 0}, {0x20d0, 0x20f0, GB_EXTEND, INCB_EXTEND, 0},
  {0x2122, 0x2122, GB_OTHER, INCB_NONE, 1}, {0x2139, 0x2139, GB_OTHER, INCB_NONE, 1}, {0x2194, 0x2199, GB_OTHER, INCB_NONE, 1},
  {0x21a9, 0x21aa, GB_OTHER, INCB_NONE, 1}, {0x231a, 0x231b, GB_OTHER, INCB_NONE, 1}, {0x2328, 0x2328, GB_OTHER, INCB_NONE, 1},
  {0x23cf, 0x23cf, GB_OTHER, INCB_NONE, 1}, {0x23e9, 0x23f3, GB_OTHER, INCB_NONE, 1}, {0x23f8, 0x23fa, GB_OTHER, INCB_NONE, 1},
  {0x24c2, 0x24c2, GB_OTHER, INCB_NONE, 1}, {0x25aa, 0x25ab, GB_OTHER, INCB_NONE, 1}, {0x25b6, 0x25b6, GB_OTHER, INCB_NONE, 1},
  {0x25c0, 0x25c0, GB_OTHER, INCB_NONE, 1}, {0x25fb, 0x25fe, GB_OTHER, INCB_NONE, 1}, {0x2600, 0x2604, GB_OTHER, INCB_NONE, 1},
  {0x260e, 0x260e, GB_OTHER, INCB_NONE, 1}, {0x2611, 0x2611, GB_OTHER, INCB_NONE, 1}, {0x2614, 0x2615, GB_OTHER, INCB_NONE, 1},
  {0x2618, 0x2618, GB_OTHER, INCB_NONE, 1}, {0x261d, 0x261d, GB_OTHER, INCB_NONE, 1}, {0x2620, 0x2620, GB_OTHER, INCB_NONE, 1},
  {0x2622, 0x2623, GB_OTHER, INCB_NONE, 1}, {0x2626, 0x2626, GB_OTHER, INCB_NONE, 1}, {0x262a, 0x262a, GB_OTHER, INCB_NONE, 1},
  {0x262e, 0x262f, GB_OTHER, INCB_NONE, 1}, {0x2638, 0x263a, GB_OTHER, INCB_NONE, 1}, {0x2640, 0x2640, GB_OTHER, INCB_NONE, 1},
  {0x2642, 0x2642, GB_OTHER, INCB_NONE, 1}, {0x2648, 0x2653, GB_OTHER, INCB_NONE, 1}, {0x265f, 0x2660, GB_OTHER, INCB_NONE, 1},
  {0x2663, 0x2663, GB_OTHER, INCB_NONE, 1}, {0x2665, 0x2666, GB_OTHER, INCB_NONE, 1}, {0x2668, 0x2668, GB_OTHER, INCB_NONE, 1},
  {0x267b, 0x267b, GB_OTHER, INCB_NONE, 1}, {0x267e, 0x267f, GB_OTHER, INCB_NONE, 1}, {0x2692, 0x2697, GB_OTHER, INCB_NONE, 1},
  {0x2699, 0x2699, GB_OTHER, INCB_NONE, 1}, {0x269b, 0x269c, GB_OTHER, INCB_NONE, 1}, {0x26a0, 0x26a1, GB_OTHER, INCB_NONE, 1},
  {0x26a7, 0x26a7, GB_OTHER, INCB_NONE, 1}, {0x26aa, 0x26ab, GB_OTHER, INCB_NONE, 1}, {0x26b0, 0x26b1, GB_OTHER, INCB_NONE, 1},
  {0x26bd, 0x26be, GB_OTHER, INCB_NONE, 1}, {0x26c4, 0x26c5, GB_OTHER, INCB_NONE, 1}, {0x26c8, 0x26c8, GB_OTHER, INCB_NONE, 1},
  {0x26ce, 0x26cf, GB_OTHER, INCB_NONE, 1}, {0x26d1, 0x26d1, GB_OTHER, INCB_NONE, 1}, {0x26d3, 0x26d4, GB_OTHER, INCB_NONE, 1},
  {0x26e9, 0x26ea, GB_OTHER, INCB_NONE, 1}, {0x26f0, 0x26f5, GB_OTHER, INCB_NONE, 1}, {0x26f7, 0x26fa, GB_OTHER, INCB_NONE, 1},
  {0x26fd, 0x26fd, GB_OTHER, INCB_NONE, 1}, {0x2702, 0x2702, GB_OTHER, INCB_NONE, 1}, {0x2705, 0x2705, GB_OTHER, INCB_NONE, 1},
  {0x2708, 0x270d, GB_OTHER, INCB_NONE, 1}, {0x270f, 0x270f, GB_OTHER, INCB_NONE, 1}, {0x2712, 0x2712, GB_OTHER, INCB_NONE, 1},
  {0x2714, 0x2714, GB_OTHER, INCB_NONE, 1}, {0x2716, 0x2716, GB_OTHER, INCB_NONE, 1}, {0x271d, 0x271d, GB_OTHER, INCB_NONE, 1},
  {0x2721, 0x2721, GB_OTHER, INCB_NONE, 1}, {0x2728, 0x2728, GB_OTHER, INCB_NONE, 1}, {0x2733, 0x2734, GB_OTHER, INCB_NONE, 1},
  {0x2744, 0x2744, GB_OTHER, INCB_NONE, 1}, {0x2747, 0x2747, GB_OTHER, INCB_NONE, 1}, {0x274c, 0x274c, GB_OTHER, INCB_NONE, 1},
  {0x274e, 0x274e, GB_OTHER, INCB_NONE, 1}, {0x2753, 0x2755, GB_OTHER, INCB_NONE, 1}, {0x2757, 0x2757, GB_OTHER, INCB_NONE, 1},
  {0x2763, 0x2764, GB_OTHER, INCB_NONE, 1}, {0x2795, 0x2797, GB_OTHER, INCB_NONE, 1}, {0x27a1, 0x27a1, GB_OTHER, INCB_NONE, 1},
  {0x27b0, 0x27b0, GB_OTHER, INCB_NONE, 1}, {0x27bf, 0x27bf, GB_OTHER, INCB_NONE, 1}, {0x2934, 0x2935, GB_OTHER, INCB_NONE, 1},
  {0x2b05, 0x2b07, GB_OTHER, INCB_NONE, 1}, {0x2b1b, 0x2b1c, GB_OTHER, INCB_NONE, 1}, {0x2b50, 0x2b50, GB_OTHER, INCB_NONE, 1},
  {0x2b55, 0x2b55, GB_OTHER, INCB_NONE, 1}, {0x2cef, 0x2cf1, GB_EXTEND, INCB_EXTEND, 0}, {0x2d7f, 0x2d7f, GB_EXTEND, INCB_EXTEND, 0},
  {0x2de0, 0x2dff, GB_EXTEND, INCB_EXTEND, 0}, {0x302a, 0x302f, GB_EXTEND, INCB_EXTEND, 0}, {0x3030, 0x3030, GB_OTHER, INCB_NONE, 1},
  {0x303d, 0x303d, GB_OTHER, INCB_NONE, 1}, {0x3099, 0x309a, GB_EXTEND, INCB_EXTEND, 0}, {0x3297, 0x3297, GB_OTHER, INCB_NONE, 1},
  {0x3299, 0x3299, GB_OTHER, INCB_NONE, 1}, {0xa66f, 0xa672, GB_EXTEND, INCB_EXTEND, 0}, {0xa674, 0xa67d, GB_EXTEND, INCB_EXTEND, 0},
  {0xa69e, 0xa69f, GB_EXTEND, INCB_EXTEND, 0}, {0xa6f0, 0xa6f1, GB_EXTEND, INCB_EXTEND, 0}, {0xa802, 0xa802, GB_EXTEND, INCB_EXTEND, 0},
  {0xa806, 0xa806, GB_EXTEND, INCB_EXTEND, 0}, {0xa80b, 0xa80b, GB_EXTEND, INCB_EXTEND, 0}, {0xa823, 0xa824, GB_SPACING_MARK, INCB_NONE, 0},
  {0xa825, 0xa826, GB_EXTEND, INCB_EXTEND, 0}, {0xa827, 0xa827, GB_SPACING_MARK, INCB_NONE, 0}, {0xa82c, 0xa82c, GB_EXTEND, INCB_EXTEND, 0},
  {0xa880, 0xa881, GB_SPACING_MARK, INCB_NONE, 0}, {0xa8b4, 0xa8c3, GB_SPACING_MARK, INCB_NONE, 0}, {0xa8c4, 0xa8c5, GB_EXTEND, INCB_EXTEND, 0},
  {0xa8e0, 0xa8f1, GB_EXTEND, INCB_EXTEND, 0}, {0xa8ff, 0xa8ff, GB_EXTEND, INCB_EXTEND, 0}, {0xa926, 0xa92d, GB_EXTEND, INCB_EXTEND, 0},
  {0xa947, 0xa951, GB_EXTEND, INCB_EXTEND, 0}, {0xa952, 0xa952, GB_SPACING_MARK, INCB_NONE, 0}, {0xa953, 0xa953, GB_EXTEND, INCB_EXTEND, 0},
  {0xa960, 0xa97c, GB_L, INCB_NONE, 0}, {0xa980, 0xa982, GB_EXTEND, INCB_EXTEND, 0}, {0xa983, 0xa983, GB_SPACING_MARK, INCB_NONE, 0},
  {0xa989, 0xa98b, GB_OTHER, INCB_CONSONANT, 0}, {0xa98f, 0xa9b2, GB_OTHER, INCB_CONSONANT, 0}, {0xa9b3, 0xa9b3, GB_EXTEND, INCB_EXTEND, 0},
  {0xa9b4, 0xa9b5, GB_SPACING_MARK, INCB_NONE, 0}, {0xa9b6, 0xa9b9, GB_EXTEND, INCB_EXTEND, 0}, {0xa9ba, 0xa9bb, GB_SPACING_MARK, INCB_NONE, 0},
  {0xa9bc, 0xa9bd, GB_EXTEND, INCB_EXTEND, 0}, {0xa9be, 0xa9bf, GB_SPACING_MARK, INCB_NONE, 0}, {0xa9c0, 0xa9c0, GB_EXTEND, INCB_LINKER, 0},
  {0xa9e0, 0xa9e4, GB_OTHER, INCB_CONSONANT, 0}, {0xa9e5, 0xa9e5, GB_EXTEND, INCB_EXTEND, 0}, {0xa9e7, 0xa9ef, GB_OTHER, INCB_CONSONANT, 0},
  {0xa9fa, 0xa9fe, GB_OTHER, INCB_CONSONANT, 0}, {0xaa29, 0xaa2e, GB_EXTEND, INCB_EXTEND, 0}, {0xaa2f, 0xaa30, GB_SPACING_MARK, INCB_NONE, 0},
  {0xaa31, 0xaa32, GB_EXTEND, INCB_EXTEND, 0}, {0xaa33, 0xaa34, GB_SPACING_MARK, INCB_NONE, 0}, {0xaa35, 0xaa36, GB_EXTEND, INCB_EXTEND, 0},
  {0xaa43, 0xaa43, GB_EXTEND, INCB_EXTEND, 0}, {0xaa4c, 0xaa4c, GB_EXTEND, INCB_EXTEND, 0}, {0xaa4d, 0xaa4d, GB_SPACING_MARK, INCB_NONE, 0},
  {0xaa60, 0xaa6f, GB_OTHER, INCB_CONSONANT, 0}, {0xaa71, 0xaa73, GB_OTHER, INCB_CONSONANT, 0}, {0xaa7a, 0xaa7a, GB_OTHER, INCB_CONSONANT, 0},
  {0xaa7c, 0xaa7c, GB_EXTEND, INCB_EXTEND, 0}, {0xaa7e, 0xaa7f, GB_OTHER, INCB_CONSONANT, 0}, {0xaab0, 0xaab0, GB_EXTEND, INCB_EXTEND, 0},
  {0xaab2, 0xaab4, GB_EXTEND, INCB_EXTEND, 0}, {0xaab7, 0xaab8, GB_EXTEND, INCB_EXTEND, 0}, {0xaabe, 0xaabf, GB_EXTEND, INCB_EXTEND, 0},
  {0xaac1, 0xaac1, GB_EXTEND, INCB_EXTEND, 0}, {0xaae0, 0xaaea, GB_OTHER, INCB_CONSONANT, 0}, {0xaaeb, 0xaaeb, GB_SPACING_MARK, INCB_NONE, 0},
  {0xaaec, 0xaaed, GB_EXTEND, INCB_EXTEND, 0}, {0xaaee, 0xaaef, GB_SPACING_MARK, INCB_NONE, 0}, {0xaaf5, 0xaaf5, GB_SPACING_MARK, INCB_NONE, 0},
  {0xaaf6, 0xaaf6, GB_EXTEND, INCB_LINKER, 0}, {0xabc0, 0xabda, GB_OTHER, INCB_CONSONANT, 0}, {0xabe3, 0xabe4, GB_SPACING_MARK, INCB_NONE, 0},
  {0xabe5, 0xabe5, GB_EXTEND, INCB_EXTEND, 0}, {0xabe6, 0xabe7, GB_SPACING_MARK, INCB_NONE, 0}, {0xabe8, 0xabe8, GB_EXTEND, INCB_EXTEND, 0},
  {0xabe9, 0xabea, GB_SPACING_MARK, INCB_NONE, 0}, {0xabec, 0xabec, GB_SPACING_MARK, INCB_NONE, 0}, {0xabed, 0xabed, GB_EXTEND, INCB_EXTEND, 0},
  {0xac00, 0xac00, GB_LV, INCB_NONE, 0}, {0xac01, 0xac1b, GB_LVT, INCB_NONE, 0}, {0xac1c, 0xac1c, GB_LV, INCB_NONE, 0},
  {0xac1d, 0xac37, GB_LVT, INCB_NONE, 0}, {0xac38, 0xac38, GB_LV, INCB_NONE, 0}, {0xac39, 0xac53, GB_LVT, INCB_NONE, 0},
  {0xac54, 0xac54, GB_LV, INCB_NONE, 0}, {0xac55, 0xac6f, GB_LVT, INCB_NONE, 0}, {0xac70, 0xac70, GB_LV, INCB_NONE, 0},
  {0xac71, 0xac8b, GB_LVT, INCB_NONE, 0}, {0xac8c, 0xac8c, GB_LV, INCB_NONE, 0}, {0xac8d, 0xaca7, GB_LVT, INCB_NONE, 0},
  {0xaca8, 0xaca8, GB_LV, INCB_NONE, 0}, {0xaca9, 0xacc3, GB_LVT, INCB_NONE, 0}, {0xacc4, 0xacc4, GB_LV, INCB_NONE, 0},
  {0xacc5, 0xacdf, GB_LVT, INCB_NONE, 0}, {0xace0, 0xace0, GB_LV, INCB_NONE, 0}, {0xace1, 0xacfb, GB_LVT, INCB_NONE, 0},
  {0xacfc, 0xacfc, GB_LV, INCB_NONE, 0}, {0xacfd, 0xad17, GB_LVT, INCB_NONE, 0}, {0xad18, 0xad18, GB_LV, INCB_NONE, 0},
  {0xad19, 0xad33, GB_LVT, INCB_NONE, 0}, {0xad34, 0xad34, GB_LV, INCB_NONE, 0}, {0xad35, 0xad4f, GB_LVT, INCB_NONE, 0},
  {0xad50, 0xad50, GB_LV, INCB_NONE, 0}, {0xad51, 0xad6b, GB_LVT, INCB_NONE, 0}, {0xad6c, 0xad6c, GB_LV, INCB_NONE, 0},
  {0xad6d, 0xad87, GB_LVT, INCB_NONE, 0}, {0xad88, 0xad88, GB_LV, INCB_NONE, 0}, {0xad89, 0xada3, GB_LVT, INCB_NONE, 0},
  {0xada4, 0xada4, GB_LV, INCB_NONE, 0}, {0xada5, 0xadbf, GB_LVT, INCB_NONE, 0}, {0xadc0, 0xadc0, GB_LV, INCB_NONE, 0},
  {0xadc1, 0xaddb, GB_LVT, INCB_NONE, 0}, {0xaddc, 0xaddc, GB_LV, INCB_NONE, 0}, {0xaddd, 0xadf7, GB_LVT, INCB_NONE, 0},
  {0xadf8, 0xadf8, GB_LV, INCB_NONE, 0}, {0xadf9, 0xae13, GB_LVT, INCB_NONE, 0}, {0xae14, 0xae14, GB_LV, INCB_NONE, 0},
  {0xae15, 0xae2f, GB_LVT, INCB_NONE, 0}, {0xae30, 0xae30, GB_LV, INCB_NONE, 0}, {0xae31, 0xae4b, GB_LVT, INCB_NONE, 0},
  {0xae4c, 0xae4c, GB_LV, INCB_NONE, 0}, {0xae4d, 0xae67, GB_LVT, INCB_NONE, 0}, {0xae68, 0xae68, GB_LV, INCB_NONE, 0},
  {0xae69, 0xae83, GB_LVT, INCB_NONE, 0}, {0xae84, 0xae84, GB_LV, INCB_NONE, 0}, {0xae85, 0xae9f, GB_LVT, INCB_NONE, 0},
  {0xaea0, 0xaea0, GB_LV, INCB_NONE, 0}, {0xaea1, 0xaebb, GB_LVT, INCB_NONE, 0}, {0xaebc, 0xaebc, GB_LV, INCB_NONE, 0},
  {0xaebd, 0xaed7, GB_LVT, INCB_NONE, 0}, {0xaed8, 0xaed8, GB_LV, INCB_NONE, 0}, {0xaed9, 0xaef3, GB_LVT, INCB_NONE, 0},
  {0xaef4, 0xaef4, GB_LV, INCB_NONE, 0}, {0xaef5, 0xaf0f, GB_LVT, INCB_NONE, 0}, {0xaf10, 0xaf10, GB_LV, INCB_NONE, 0},
  {0xaf11, 0xaf2b, GB_LVT, INCB_NONE, 0}, {0xaf2c, 0xaf2c, GB_LV, INCB_NONE, 0}, {0xaf2d, 0xaf47, GB_LVT, INCB_NONE, 0},
  {0xaf48, 0xaf48, GB_LV, INCB_NONE, 0}, {0xaf49, 0xaf63, GB_LVT, INCB_NONE, 0}, {0xaf64, 0xaf64, GB_LV, INCB_NONE, 0},
  {0xaf65, 0xaf7f, GB_LVT, INCB_NONE, 0}, {0xaf80, 0xaf80, GB_LV, INCB_NONE, 0}, {0xaf81, 0xaf9b, GB_LVT, INCB_NONE, 0},
  {0xaf9c, 0xaf9c, GB_LV, INCB_NONE, 0}, {0xaf9d, 0xafb7, GB_LVT, INCB_NONE, 0}, {0xafb8, 0xafb8, GB_LV, INCB_NONE, 0},
  {0xafb9, 0xafd3, GB_LVT, INCB_NONE, 0}, {0xafd4, 0xafd4, GB_LV, INCB_NONE, 0}, {0xafd5, 0xafef, GB_LVT, INCB_NONE, 0},
  {0xaff0, 0xaff0, GB_LV, INCB_NONE, 0}, {0xaff1, 0xb00b, GB_LVT, INCB_NONE, 0}, {0xb00c, 0xb00c, GB_LV, INCB_NONE, 0},
  {0xb00d, 0xb027, GB_LVT, INCB_NONE, 0}, {0xb028, 0xb028, GB_LV, INCB_NONE, 0}, {0xb029, 0xb043, GB_LVT, INCB_NONE, 0},
  {0xb044, 0xb044, GB_LV, INCB_NONE, 0}, {0xb045, 0xb05f, GB_LVT, INCB_NONE, 0}, {0xb060, 0xb060, GB_LV, INCB_NONE, 0},
  {0xb061, 0xb07b, GB_LVT, INCB_NONE, 0}, {0xb07c, 0xb07c, GB_LV, INCB_NONE, 0}, {0xb07d, 0xb097, GB_LVT, INCB_NONE, 0},
  {0xb098, 0xb098, GB_LV, INCB_NONE, 0}, {0xb099, 0xb0b3, GB_LVT, INCB_NONE, 0}, {0xb0b4, 0xb0b4, GB_LV, INCB_NONE, 0},
  {0xb0b5, 0xb0cf, GB_LVT, INCB_NONE, 0}, {0xb0d0, 0xb0d0, GB_LV, INCB_NONE, 0}, {0xb0d1, 0xb0eb, GB_LVT, INCB_NONE, 0},
  {0xb0ec, 0xb0ec, GB_LV, INCB_NONE, 0}, {0xb0ed, 0xb107, GB_LVT, INCB_NONE, 0}, {0xb108, 0xb108, GB_LV, INCB_NONE, 0},
  {0xb109, 0xb123, GB_LVT, INCB_NONE, 0}, {0xb124, 0xb124, GB_LV, INCB_NONE, 0}, {0xb125, 0xb13f, GB_LVT, INCB_NONE, 0},
  {0xb140, 0xb140, GB_LV, INCB_NONE, 0}, {0xb141, 0xb15b, GB_LVT, INCB_NONE, 0}, {0xb15c, 0xb15c, GB_LV, INCB_NONE, 0},
  {0xb15d, 0xb177, GB_LVT, INCB_NONE, 0}, {0xb178, 0xb178, GB_LV, INCB_NONE, 0}, {0xb179, 0xb193, GB_LVT, INCB_NONE, 0},
  {0xb194, 0xb194, GB_LV, INCB_NONE, 0}, {0xb195, 0xb1af, GB_LVT, INCB_NONE, 0}, {0xb1b0, 0xb1b0, GB_LV, INCB_NONE, 0},
  {0xb1b1, 0xb1cb, GB_LVT, INCB_NONE, 0}, {0xb1cc, 0xb1cc, GB_LV, INCB_NONE, 0}, {0xb1cd, 0xb1e7, GB_LVT, INCB_NONE, 0},
  {0xb1e8, 0xb1e8, GB_LV, INCB_NONE, 0}, {0xb1e9, 0xb203, GB_LVT, INCB_NONE, 0}, {0xb204, 0xb204, GB_LV, INCB_NONE, 0},
  {0xb205, 0xb21f, GB_LVT, INCB_NONE, 0}, {0xb220, 0xb220, GB_LV, INCB_NONE, 0}, {0xb221, 0xb23b, GB_LVT, INCB_NONE, 0},
  {0xb23c, 0xb23c, GB_LV, INCB_NONE, 0}, {0xb23d, 0xb257, GB_LVT, INCB_NONE, 0}, {0xb258, 0xb258, GB_LV, INCB_NONE, 0},
  {0xb259, 0xb273, GB_LVT, INCB_NONE, 0}, {0xb274, 0xb274, GB_LV, INCB_NONE, 0}, {0xb275, 0xb28f, GB_LVT, INCB_NONE, 0},
  {0xb290, 0xb290, GB_LV, INCB_NONE, 0}, {0xb291, 0xb2ab, GB_LVT, INCB_NONE, 0}, {0xb2ac, 0xb2ac, GB_LV, INCB_NONE, 0},
  {0xb2ad, 0xb2c7, GB_LVT, INCB_NONE, 0}, {0xb2c8, 0xb2c8, GB_LV, INCB_NONE, 0}, {0xb2c9, 0xb2e3, GB_LVT, INCB_NONE, 0},
  {0xb2e4, 0xb2e4, GB_LV, INCB_NONE, 0}, {0xb2e5, 0xb2ff, GB_LVT, INCB_NONE, 0}, {0xb300, 0xb300, GB_LV, INCB_NONE, 0},
  {0xb301, 0xb31b, GB_LVT, INCB_NONE, 0}, {0xb31c, 0xb31c, GB_LV, INCB_NONE, 0}, {0xb31d, 0xb337, GB_LVT, INCB_NONE, 0},
  {0xb338, 0xb338, GB_LV, INCB_NONE, 0}, {0xb339, 0xb353, GB_LVT, INCB_NONE, 0}, {0xb354, 0xb354, GB_LV, INCB_NONE, 0},
  {0xb355, 0xb36f, GB_LVT, INCB_NONE, 0}, {0xb370, 0xb370, GB_LV, INCB_NONE, 0}, {0xb371, 0xb38b, GB_LVT, INCB_NONE, 0},
  {0xb38c, 0xb38c, GB_LV, INCB_NONE, 0}, {0xb38d, 0xb3a7, GB_LVT, INCB_NONE, 0}, {0xb3a8, 0xb3a8, GB_LV, INCB_NONE, 0},
  {0xb3a9, 0xb3c3, GB_LVT, INCB_NONE, 0}, {0xb3c4, 0xb3c4, GB_LV, INCB_NONE, 0}, {0xb3c5, 0xb3df, GB_LVT, INCB_NONE, 0},
  {0xb3e0, 0xb3e0, GB_LV, INCB_NONE, 0}, {0xb3e1, 0xb3fb, GB_LVT, INCB_NONE, 0}, {0xb3fc, 0xb3fc, GB_LV, INCB_NONE, 0},
  {0xb3fd, 0xb417, GB_LVT, INCB_NONE, 0}, {0xb418, 0xb418, GB_LV, INCB_NONE, 0}, {0xb419, 0xb433, GB_LVT, INCB_NONE, 0},
  {0xb434, 0xb434, GB_LV, INCB_NONE, 0}, {0xb435, 0xb44f, GB_LVT, INCB_NONE, 0}, {0xb450, 0xb450, GB_LV, INCB_NONE, 0},
  {0xb451, 0xb46b, GB_LVT, INCB_NONE, 0}, {0xb46c, 0xb46c, GB_LV, INCB_NONE, 0}, {0xb46d, 0xb487, GB_LVT, INCB_NONE, 0},
  {0xb488, 0xb488, GB_LV, INCB_NONE, 0}, {0xb489, 0xb4a3, GB_LVT, INCB_NONE, 0}, {0xb4a4, 0xb4a4, GB_LV, INCB_NONE, 0},
  {0xb4a5, 0xb4bf, GB_LVT, INCB_NONE, 0}, {0xb4c0, 0xb4c0, GB_LV, INCB_NONE, 0}, {0xb4c1, 0xb4db, GB_LVT, INCB_NONE, 0},
  {0xb4dc, 0xb4dc, GB_LV, INCB_NONE, 0}, {0xb4dd, 0xb4f7, GB_LVT, INCB_NONE, 0}, {0xb4f8, 0xb4f8, GB_LV, INCB_NONE, 0},
  {0xb4f9, 0xb513, GB_LVT, INCB_NONE, 0}, {0xb514, 0xb514, GB_LV, INCB_NONE, 0}, {0xb515, 0xb52f, GB_LVT, INCB_NONE, 0},
  {0xb530, 0xb530, GB_LV, INCB_NONE, 0}, {0xb531, 0xb54b, GB_LVT, INCB_NONE, 0}, {0xb54c, 0xb54c, GB_LV, INCB_NONE, 0},
  {0xb54d, 0xb567, GB_LVT, INCB_NONE, 0}, {0xb568, 0xb568, GB_LV, INCB_NONE, 0}, {0xb569, 0xb583, GB_LVT, INCB_NONE, 0},
  {0xb584, 0xb584, GB_LV, INCB_NONE, 0}, {0xb585, 0xb59f, GB_LVT, INCB_NONE, 0}, {0xb5a0, 0xb5a0, GB_LV, INCB_NONE, 0},
  {0xb5a1, 0xb5bb, GB_LVT, INCB_NONE, 0}, {0xb5bc, 0xb5bc, GB_LV, INCB_NONE, 0}, {0xb5bd, 0xb5d7, GB_LVT, INCB_NONE, 0},
  {0xb5d8, 0xb5d8, GB_LV, INCB_NONE, 0}, {0xb5d9, 0xb5f3, GB_LVT, INCB_NONE, 0}, {0xb5f4, 0xb5f4, GB_LV, INCB_NONE, 0},
  {0xb5f5, 0xb60f, GB_LVT, INCB_NONE, 0}, {0xb610, 0xb610, GB_LV, INCB_NONE, 0}, {0xb611, 0xb62b, GB_LVT, INCB_NONE, 0},
  {0xb62c, 0xb62c, GB_LV, INCB_NONE, 0}, {0xb62d, 0xb647, GB_LVT, INCB_NONE, 0}, {0xb648, 0xb648, GB_LV, INCB_NONE, 0},
  {0xb649, 0xb663, GB_LVT, INCB_NONE, 0}, {0xb664, 0xb664, GB_LV, INCB_NONE, 0}, {0xb665, 0xb67f, GB_LVT, INCB_NONE, 0},
  {0xb680, 0xb680, GB_LV, INCB_NONE, 0}, {0xb681, 0xb69b, GB_LVT, INCB_NONE, 0}, {0xb69c, 0xb69c, GB_LV, INCB_NONE, 0},
  {0xb69d, 0xb6b7, GB_LVT, INCB_NONE, 0}, {0xb6b8, 0xb6b8, GB_LV, INCB_NONE, 0}, {0xb6b9, 0xb6d3, GB_LVT, INCB_NONE, 0},
  {0xb6d4, 0xb6d4, GB_LV, INCB_NONE, 0}, {0xb6d5, 0xb6ef, GB_LVT, INCB_NONE, 0}, {0xb6f0, 0xb6f0, GB_LV, INCB_NONE, 0},
  {0xb6f1, 0xb70b, GB_LVT, INCB_NONE, 0}, {0xb70c, 0xb70c, GB_LV, INCB_NONE, 0}, {0xb70d, 0xb727, GB_LVT, INCB_NONE, 0},
  {0xb728, 0xb728, GB_LV, INCB_NONE, 0}, {0xb729, 0xb743, GB_LVT, INCB_NONE, 0}, {0xb744, 0xb744, GB_LV, INCB_NONE, 0},
  {0xb745, 0xb75f, GB_LVT, INCB_NONE, 0}, {0xb760, 0xb760, GB_LV, INCB_NONE, 0}, {0xb761, 0xb77b, GB_LVT, INCB_NONE, 0},
  {0xb77c, 0xb77c, GB_LV, INCB_NONE, 0}, {0xb77d, 0xb797, GB_LVT, INCB_NONE, 0}, {0xb798, 0xb798, GB_LV, INCB_NONE, 0},
  {0xb799, 0xb7b3, GB_LVT, INCB_NONE, 0}, {0xb7b4, 0xb7b4, GB_LV, INCB_NONE, 0}, {0xb7b5, 0xb7cf, GB_LVT, INCB_NONE, 0},
  {0xb7d0, 0xb7d0, GB_LV, INCB_NONE, 0}, {0xb7d1, 0xb7eb, GB_LVT, INCB_NONE, 0}, {0xb7ec, 0xb7ec, GB_LV, INCB_NONE, 0},
  {0xb7ed, 0xb807, GB_LVT, INCB_NONE, 0}, {0xb808, 0xb808, GB_LV, INCB_NONE, 0}, {0xb809, 0xb823, GB_LVT, INCB_NONE, 0},
  {0xb824, 0xb824, GB_LV, INCB_NONE, 0}, {0xb825, 0xb83f, GB_LVT, INCB_NONE, 0}, {0xb840, 0xb840, GB_LV, INCB_NONE, 0},
  {0xb841, 0xb85b, GB_LVT, INCB_NONE, 0}, {0xb85c, 0xb85c, GB_LV, INCB_NONE, 0}, {0xb85d, 0xb877, GB_LVT, INCB_NONE, 0},
  {0xb878, 0xb878, GB_LV, INCB_NONE, 0}, {0xb879, 0xb893, GB_LVT, INCB_NONE, 0}, {0xb894, 0xb894, GB_LV, INCB_NONE, 0},
  {0xb895, 0xb8af, GB_LVT, INCB_NONE, 0}, {0xb8b0, 0xb8b0, GB_LV, INCB_NONE, 0}, {0xb8b1, 0xb8cb, GB_LVT, INCB_NONE, 0},
  {0xb8cc, 0xb8cc, GB_LV, INCB_NONE, 0}, {0xb8cd, 0xb8e7, GB_LVT, INCB_NONE, 0}, {0xb8e8, 0xb8e8, GB_LV, INCB_NONE, 0},
  {0xb8e9, 0xb903, GB_LVT, INCB_NONE, 0}, {0xb904, 0xb904, GB_LV, INCB_NONE, 0}, {0xb905, 0xb91f, GB_LVT, INCB_NONE, 0},
  {0xb920, 0xb920, GB_LV, INCB_NONE, 0}, {0xb921, 0xb93b, GB_LVT, INCB_NONE, 0}, {0xb93c, 0xb93c, GB_LV, INCB_NONE, 0},
  {0xb93d, 0xb957, GB_LVT, INCB_NONE, 0}, {0xb958, 0xb958, GB_LV, INCB_NONE, 0}, {0xb959, 0xb973, GB_LVT, INCB_NONE, 0},
  {0xb974, 0xb974, GB_LV, INCB_NONE, 0}, {0xb975, 0xb98f, GB_LVT, INCB_NONE, 0}, {0xb990, 0xb990, GB_LV, INCB_NONE, 0},
  {0xb991, 0xb9ab, GB_LVT, INCB_NONE, 0}, {0xb9ac, 0xb9ac, GB_LV, INCB_NONE, 0}, {0xb9ad, 0xb9c7, GB_LVT, INCB_NONE, 0},
  {0xb9c8, 0xb9c8, GB_LV, INCB_NONE, 0}, {0xb9c9, 0xb9e3, GB_LVT, INCB_NONE, 0}, {0xb9e4, 0xb9e4, GB_LV, INCB_NONE, 0},
  {0xb9e5, 0xb9ff, GB_LVT, INCB_NONE, 0}, {0xba00, 0xba00, GB_LV, INCB_NONE, 0}, {0xba01, 0xba1b, GB_LVT, INCB_NONE, 0},
  {0xba1c, 0xba1c, GB_LV, INCB_NONE, 0}, {0xba1d, 0xba37, GB_LVT, INCB_NONE, 0}, {0xba38, 0xba38, GB_LV, INCB_NONE, 0},
  {0xba39, 0xba53, GB_LVT, INCB_NONE, 0}, {0xba54, 0xba54, GB_LV, INCB_NONE, 0}, {0xba55, 0xba6f, GB_LVT, INCB_NONE, 0},
  {0xba70, 0xba70, GB_LV, INCB_NONE, 0}, {0xba71, 0xba8b, GB_LVT, INCB_NONE, 0}, {0xba8c, 0xba8c, GB_LV, INCB_NONE, 0},
  {0xba8d, 0xbaa7, GB_LVT, INCB_NONE, 0}, {0xbaa8, 0xbaa8, GB_LV, INCB_NONE, 0}, {0xbaa9, 0xbac3, GB_LVT, INCB_NONE, 0},
  {0xbac4, 0xbac4, GB_LV, INCB_NONE, 0}, {0xbac5, 0xbadf, GB_LVT, INCB_NONE, 0}, {0xbae0, 0xbae0, GB_LV, INCB_NONE, 0},
  {0xbae1, 0xbafb, GB_LVT, INCB_NONE, 0}, {0xbafc, 0xbafc, GB_LV, INCB_NONE, 0}, {0xbafd, 0xbb17, GB_LVT, INCB_NONE, 0},
  {0xbb18, 0xbb18, GB_LV, INCB_NONE, 0}, {0xbb19, 0xbb33, GB_LVT, INCB_NONE, 0}, {0xbb34, 0xbb34, GB_LV, INCB_NONE, 0},
  {0xbb35, 0xbb4f, GB_LVT, INCB_NONE, 0}, {0xbb50, 0xbb50, GB_LV, INCB_NONE, 0}, {0xbb51, 0xbb6b, GB_LVT, INCB_NONE, 0},
  {0xbb6c, 0xbb6c, GB_LV, INCB_NONE, 0}, {0xbb6d, 0xbb87, GB_LVT, INCB_NONE, 0}, {0xbb88, 0xbb88, GB_LV, INCB_NONE, 0},
  {0xbb89, 0xbba3, GB_LVT, INCB_NONE, 0}, {0xbba4, 0xbba4, GB_LV, INCB_NONE, 0}, {0xbba5, 0xbbbf, GB_LVT, INCB_NONE, 0},
  {0xbbc0, 0xbbc0, GB_LV, INCB_NONE, 0}, {0xbbc1, 0xbbdb, GB_LVT, INCB_NONE, 0}, {0xbbdc, 0xbbdc, GB_LV, INCB_NONE, 0},
  {0xbbdd, 0xbbf7, GB_LVT, INCB_NONE, 0}, {0xbbf8, 0xbbf8, GB_LV, INCB_NONE, 0}, {0xbbf9, 0xbc13, GB_LVT, INCB_NONE, 0},
  {0xbc14, 0xbc14, GB_LV, INCB_NONE, 0}, {0xbc15, 0xbc2f, GB_LVT, INCB_NONE, 0}, {0xbc30, 0xbc30, GB_LV, INCB_NONE, 0},
  {0xbc31, 0xbc4b, GB_LVT, INCB_NONE, 0}, {0xbc4c, 0xbc4c, GB_LV, INCB_NONE, 0}, {0xbc4d, 0xbc67, GB_LVT, INCB_NONE, 0},
  {0xbc68, 0xbc68, GB_LV, INCB_NONE, 0}, {0xbc69, 0xbc83, GB_LVT, INCB_NONE, 0}, {0xbc84, 0xbc84, GB_LV, INCB_NONE, 0},
  {0xbc85, 0xbc9f, GB_LVT, INCB_NONE, 0}, {0xbca0, 0xbca0, GB_LV, INCB_NONE, 0}, {0xbca1, 0xbcbb, GB_LVT, INCB_NONE, 0},
  {0xbcbc, 0xbcbc, GB_LV, INCB_NONE, 0}, {0xbcbd, 0xbcd7, GB_LVT, INCB_NONE, 0}, {0xbcd8, 0xbcd8, GB_LV, INCB_NONE, 0},
  {0xbcd9, 0xbcf3, GB_LVT, INCB_NONE, 0}, {0xbcf4, 0xbcf4, GB_LV, INCB_NONE, 0}, {0xbcf5, 0xbd0f, GB_LVT, INCB_NONE, 0},
  {0xbd10, 0xbd10, GB_LV, INCB_NONE, 0}, {0xbd11, 0xbd2b, GB_LVT, INCB_NONE, 0}, {0xbd2c, 0xbd2c, GB_LV, INCB_NONE, 0},
  {0xbd2d, 0xbd47, GB_LVT, INCB_NONE, 0}, {0xbd48, 0xbd48, GB_LV, INCB_NONE, 0}, {0xbd49, 0xbd63, GB_LVT, INCB_NONE, 0},
  {0xbd64, 0xbd64, GB_LV, INCB_NONE, 0}, {0xbd65, 0xbd7f, GB_LVT, INCB_NONE, 0}, {0xbd80, 0xbd80, GB_LV, INCB_NONE, 0},
  {0xbd81, 0xbd9b, GB_LVT, INCB_NONE, 0}, {0xbd9c, 0xbd9c, GB_LV, INCB_NONE, 0}, {0xbd9d, 0xbdb7, GB_LVT, INCB_NONE, 0},
  {0xbdb8, 0xbdb8, GB_LV, INCB_NONE, 0}, {0xbdb9, 0xbdd3, GB_LVT, INCB_NONE, 0}, {0xbdd4, 0xbdd4, GB_LV, INCB_NONE, 0},
  {0xbdd5, 0xbdef, GB_LVT, INCB_NONE, 0}, {0xbdf0, 0xbdf0, GB_LV, INCB_NONE, 0}, {0xbdf1, 0xbe0b, GB_LVT, INCB_NONE, 0},
  {0xbe0c, 0xbe0c, GB_LV, INCB_NONE, 0}, {0xbe0d, 0xbe27, GB_LVT, INCB_NONE, 0}, {0xbe28, 0xbe28, GB_LV, INCB_NONE, 0},
  {0xbe29, 0xbe43, GB_LVT, INCB_NONE, 0}, {0xbe44, 0xbe44, GB_LV, INCB_NONE, 0}, {0xbe45, 0xbe5f, GB_LVT, INCB_NONE, 0},
  {0xbe60, 0xbe60, GB_LV, INCB_NONE, 0}, {0xbe61, 0xbe7b, GB_LVT, INCB_NONE, 0}, {0xbe7c, 0xbe7c, GB_LV, INCB_NONE, 0},
  {0xbe7d, 0xbe97, GB_LVT, INCB_NONE, 0}, {0xbe98, 0xbe98, GB_LV, INCB_NONE, 0}, {0xbe99, 0xbeb3, GB_LVT, INCB_NONE, 0},
  {0xbeb4, 0xbeb4, GB_LV, INCB_NONE, 0}, {0xbeb5, 0xbecf, GB_LVT, INCB_NONE, 0}, {0xbed0, 0xbed0, GB_LV, INCB_NONE, 0},
  {0xbed1, 0xbeeb, GB_LVT, INCB_NONE, 0}, {0xbeec, 0xbeec, GB_LV, INCB_NONE, 0}, {0xbeed, 0xbf07, GB_LVT, INCB_NONE, 0},
  {0xbf08, 0xbf08, GB_LV, INCB_NONE, 0}, {0xbf09, 0xbf23, GB_LVT, INCB_NONE, 0}, {0xbf24, 0xbf24, GB_LV, INCB_NONE, 0},
  {0xbf25, 0xbf3f, GB_LVT, INCB_NONE, 0}, {0xbf40, 0xbf40, GB_LV, INCB_NONE, 0}, {0xbf41, 0xbf5b, GB_LVT, INCB_NONE, 0},
  {0xbf5c, 0xbf5c, GB_LV, INCB_NONE, 0}, {0xbf5d, 0xbf77, GB_LVT, INCB_NONE, 0}, {0xbf78, 0xbf78, GB_LV, INCB_NONE, 0},
  {0xbf79, 0xbf93, GB_LVT, INCB_NONE, 0}, {0xbf94, 0xbf94, GB_LV, INCB_NONE, 0}, {0xbf95, 0xbfaf, GB_LVT, INCB_NONE, 0},
  {0xbfb0, 0xbfb0, GB_LV, INCB_NONE, 0}, {0xbfb1, 0xbfcb, GB_LVT, INCB_NONE, 0}, {0xbfcc, 0xbfcc, GB_LV, INCB_NONE, 0},
  {0xbfcd, 0xbfe7, GB_LVT, INCB_NONE, 0}, {0xbfe8, 0xbfe8, GB_LV, INCB_NONE, 0}, {0xbfe9, 0xc003, GB_LVT, INCB_NONE, 0},
  {0xc004, 0xc004, GB_LV, INCB_NONE, 0}, {0xc005, 0xc01f, GB_LVT, INCB_NONE, 0}, {0xc020, 0xc020, GB_LV, INCB_NONE, 0},
  {0xc021, 0xc03b, GB_LVT, INCB_NONE, 0}, {0xc03c, 0xc03c, GB_LV, INCB_NONE, 0}, {0xc03d, 0xc057, GB_LVT, INCB_NONE, 0},
  {0xc058, 0xc058, GB_LV, INCB_NONE, 0}, {0xc059, 0xc073, GB_LVT, INCB_NONE, 0}, {0xc074, 0xc074, GB_LV, INCB_NONE, 0},
  {0xc075, 0xc08f, GB_LVT, INCB_NONE, 0}, {0xc090, 0xc090, GB_LV, INCB_NONE, 0}, {0xc091, 0xc0ab, GB_LVT, INCB_NONE, 0},
  {0xc0ac, 0xc0ac, GB_LV, INCB_NONE, 0}, {0xc0ad, 0xc0c7, GB_LVT, INCB_NONE, 0}, {0xc0c8, 0xc0c8, GB_LV, INCB_NONE, 0},
  {0xc0c9, 0xc0e3, GB_LVT, INCB_NONE, 0}, {0xc0e4, 0xc0e4, GB_LV, INCB_NONE, 0}, {0xc0e5, 0xc0ff, GB_LVT, INCB_NONE, 0},
  {0xc100, 0xc100, GB_LV, INCB_NONE, 0}, {0xc101, 0xc11b, GB_LVT, INCB_NONE, 0}, {0xc11c, 0xc11c, GB_LV, INCB_NONE, 0},
  {0xc11d, 0xc137, GB_LVT, INCB_NONE, 0}, {0xc138, 0xc138, GB_LV, INCB_NONE, 0}, {0xc139, 0xc153, GB_LVT, INCB_NONE, 0},
  {0xc154, 0xc154, GB_LV, INCB_NONE, 0}, {0xc155, 0xc16f, GB_LVT, INCB_NONE, 0}, {0xc170, 0xc170, GB_LV, INCB_NONE, 0},
  {0xc171, 0xc18b, GB_LVT, INCB_NONE, 0}, {0xc18c, 0xc18c, GB_LV, INCB_NONE, 0}, {0xc18d, 0xc1a7, GB_LVT, INCB_NONE, 0},
  {0xc1a8, 0xc1a8, GB_LV, INCB_NONE, 0}, {0xc1a9, 0xc1c3, GB_LVT, INCB_NONE, 0}, {0xc1c4, 0xc1c4, GB_LV, INCB_NONE, 0},
  {0xc1c5, 0xc1df, GB_LVT, INCB_NONE, 0}, {0xc1e0, 0xc1e0, GB_LV, INCB_NONE, 0}, {0xc1e1, 0xc1fb, GB_LVT, INCB_NONE, 0},
  {0xc1fc, 0xc1fc, GB_LV, INCB_NONE, 0}, {0xc1fd, 0xc217, GB_LVT, INCB_NONE, 0}, {0xc218, 0xc218, GB_LV, INCB_NONE, 0},
  {0xc219, 0xc233, GB_LVT, INCB_NONE, 0}, {0xc234, 0xc234, GB_LV, INCB_NONE, 0}, {0xc235, 0xc24f, GB_LVT, INCB_NONE, 0},
  {0xc250, 0xc250, GB_LV, INCB_NONE, 0}, {0xc251, 0xc26b, GB_LVT, INCB_NONE, 0}, {0xc26c, 0xc26c, GB_LV, INCB_NONE, 0},
  {0xc26d, 0xc287, GB_LVT, INCB_NONE, 0}, {0xc288, 0xc288, GB_LV, INCB_NONE, 0}, {0xc289, 0xc2a3, GB_LVT, INCB_NONE, 0},
  {0xc2a4, 0xc2a4, GB_LV, INCB_NONE, 0}, {0xc2a5, 0xc2bf, GB_LVT, INCB_NONE, 0}, {0xc2c0, 0xc2c0, GB_LV, INCB_NONE, 0},
  {0xc2c1, 0xc2db, GB_LVT, INCB_NONE, 0}, {0xc2dc, 0xc2dc, GB_LV, INCB_NONE, 0}, {0xc2dd, 0xc2f7, GB_LVT, INCB_NONE, 0},
  {0xc2f8, 0xc2f8, GB_LV, INCB_NONE, 0}, {0xc2f9, 0xc313, GB_LVT, INCB_NONE, 0}, {0xc314, 0xc314, GB_LV, INCB_NONE, 0},
  {0xc315, 0xc32f, GB_LVT, INCB_NONE, 0}, {0xc330, 0xc330, GB_LV, INCB_NONE, 0}, {0xc331, 0xc34b, GB_LVT, INCB_NONE, 0},
  {0xc34c, 0xc34c, GB_LV, INCB_NONE, 0}, {0xc34d, 0xc367, GB_LVT, INCB_NONE, 0}, {0xc368, 0xc368, GB_LV, INCB_NONE, 0},
  {0xc369, 0xc383, GB_LVT, INCB_NONE, 0}, {0xc384, 0xc384, GB_LV, INCB_NONE, 0}, {0xc385, 0xc39f, GB_LVT, INCB_NONE, 0},
  {0xc3a0, 0xc3a0, GB_LV, INCB_NONE, 0}, {0xc3a1, 0xc3bb, GB_LVT, INCB_NONE, 0}, {0xc3bc, 0xc3bc, GB_LV, INCB_NONE, 0},
  {0xc3bd, 0xc3d7, GB_LVT, INCB_NONE, 0}, {0xc3d8, 0xc3d8, GB_LV, INCB_NONE, 0}, {0xc3d9, 0xc3f3, GB_LVT, INCB_NONE, 0},
  {0xc3f4, 0xc3f4, GB_LV, INCB_NONE, 0}, {0xc3f5, 0xc40f, GB_LVT, INCB_NONE, 0}, {0xc410, 0xc410, GB_LV, INCB_NONE, 0},
  {0xc411, 0xc42b, GB_LVT, INCB_NONE, 0}, {0xc42c, 0xc42c, GB_LV, INCB_NONE, 0}, {0xc42d, 0xc447, GB_LVT, INCB_NONE, 0},
  {0xc448, 0xc448, GB_LV, INCB_NONE, 0}, {0xc449, 0xc463, GB_LVT, INCB_NONE, 0}, {0xc464, 0xc464, GB_LV, INCB_NONE, 0},
  {0xc465, 0xc47f, GB_LVT, INCB_NONE, 0}, {0xc480, 0xc480, GB_LV, INCB_NONE, 0}, {0xc481, 0xc49b, GB_LVT, INCB_NONE, 0},
  {0xc49c, 0xc49c, GB_LV, INCB_NONE, 0}, {0xc49d, 0xc4b7, GB_LVT, INCB_NONE, 0}, {0xc4b8, 0xc4b8, GB_LV, INCB_NONE, 0},
  {0xc4b9, 0xc4d3, GB_LVT, INCB_NONE, 0}, {0xc4d4, 0xc4d4, GB_LV, INCB_NONE, 0}, {0xc4d5, 0xc4ef, GB_LVT, INCB_NONE, 0},
  {0xc4f0, 0xc4f0, GB_LV, INCB_NONE, 0}, {0xc4f1, 0xc50b, GB_LVT, INCB_NONE, 0}, {0xc50c, 0xc50c, GB_LV, INCB_NONE, 0},
  {0xc50d, 0xc527, GB_LVT, INCB_NONE, 0}, {0xc528, 0xc528, GB_LV, INCB_NONE, 0}, {0xc529, 0xc543, GB_LVT, INCB_NONE, 0},
  {0xc544, 0xc544, GB_LV, INCB_NONE, 0}, {0xc545, 0xc55f, GB_LVT, INCB_NONE, 0}, {0xc560, 0xc560, GB_LV, INCB_NONE, 0},
  {0xc561, 0xc57b, GB_LVT, INCB_NONE, 0}, {0xc57c, 0xc57c, GB_LV, INCB_NONE, 0}, {0xc57d, 0xc597, GB_LVT, INCB_NONE, 0},
  {0xc598, 0xc598, GB_LV, INCB_NONE, 0}, {0xc599, 0xc5b3, GB_LVT, INCB_NONE, 0}, {0xc5b4, 0xc5b4, GB_LV, INCB_NONE, 0},
  {0xc5b5, 0xc5cf, GB_LVT, INCB_NONE, 0}, {0xc5d0, 0xc5d0, GB_LV, INCB_NONE, 0}, {0xc5d1, 0xc5eb, GB_LVT, INCB_NONE, 0},
  {0xc5ec, 0xc5ec, GB_LV, INCB_NONE, 0}, {0xc5ed, 0xc607, GB_LVT, INCB_NONE, 0}, {0xc608, 0xc608, GB_LV, INCB_NONE, 0},
  {0xc609, 0xc623, GB_LVT, INCB_NONE, 0}, {0xc624, 0xc624, GB_LV, INCB_NONE, 0}, {0xc625, 0xc63f, GB_LVT, INCB_NONE, 0},
  {0xc640, 0xc640, GB_LV, INCB_NONE, 0}, {0xc641, 0xc65b, GB_LVT, INCB_NONE, 0}, {0xc65c, 0xc65c, GB_LV, INCB_NONE, 0},
  {0xc65d, 0xc677, GB_LVT, INCB_NONE, 0}, {0xc678, 0xc678, GB_LV, INCB_NONE, 0}, {0xc679, 0xc693, GB_LVT, INCB_NONE, 0},
  {0xc694, 0xc694, GB_LV, INCB_NONE, 0}, {0xc695, 0xc6af, GB_LVT, INCB_NONE, 0}, {0xc6b0, 0xc6b0, GB_LV, INCB_NONE, 0},
  {0xc6b1, 0xc6cb, GB_LVT, INCB_NONE, 0}, {0xc6cc, 0xc6cc, GB_LV, INCB_NONE, 0}, {0xc6cd, 0xc6e7, GB_LVT, INCB_NONE, 0},
  {0xc6e8, 0xc6e8, GB_LV, INCB_NONE, 0}, {0xc6e9, 0xc703, GB_LVT, INCB_NONE, 0}, {0xc704, 0xc704, GB_LV, INCB_NONE, 0},
  {0xc705, 0xc71f, GB_LVT, INCB_NONE, 0}, {0xc720, 0xc720, GB_LV, INCB_NONE, 0}, {0xc721, 0xc73b, GB_LVT, INCB_NONE, 0},
  {0xc73c, 0xc73c, GB_LV, INCB_NONE, 0}, {0xc73d, 0xc757, GB_LVT, INCB_NONE, 0}, {0xc758, 0xc758, GB_LV, INCB_NONE, 0},
  {0xc759, 0xc773, GB_LVT, INCB_NONE, 0}, {0xc774, 0xc774, GB_LV, INCB_NONE, 0}, {0xc775, 0xc78f, GB_LVT, INCB_NONE, 0},
  {0xc790, 0xc790, GB_LV, INCB_NONE, 0}, {0xc791, 0xc7ab, GB_LVT, INCB_NONE, 0}, {0xc7ac, 0xc7ac, GB_LV, INCB_NONE, 0},
  {0xc7ad, 0xc7c7, GB_LVT, INCB_NONE, 0}, {0xc7c8, 0xc7c8, GB_LV, INCB_NONE, 0}, {0xc7c9, 0xc7e3, GB_LVT, INCB_NONE, 0},
  {0xc7e4, 0xc7e4, GB_LV, INCB_NONE, 0}, {0xc7e5, 0xc7ff, GB_LVT, INCB_NONE, 0}, {0xc800, 0xc800, GB_LV, INCB_NONE, 0},
  {0xc801, 0xc81b, GB_LVT, INCB_NONE, 0}, {0xc81c, 0xc81c, GB_LV, INCB_NONE, 0}, {0xc81d, 0xc837, GB_LVT, INCB_NONE, 0},
  {0xc838, 0xc838, GB_LV, INCB_NONE, 0}, {0xc839, 0xc853, GB_LVT, INCB_NONE, 0}, {0xc854, 0xc854, GB_LV, INCB_NONE, 0},
  {0xc855, 0xc86f, GB_LVT, INCB_NONE, 0}, {0xc870, 0xc870, GB_LV, INCB_NONE, 0}, {0xc871, 0xc88b, GB_LVT, INCB_NONE, 0},
  {0xc88c, 0xc88c, GB_LV, INCB_NONE, 0}, {0xc88d, 0xc8a7, GB_LVT, INCB_NONE, 0}, {0xc8a8, 0xc8a8, GB_LV, INCB_NONE, 0},
  {0xc8a9, 0xc8c3, GB_LVT, INCB_NONE, 0}, {0xc8c4, 0xc8c4, GB_LV, INCB_NONE, 0}, {0xc8c5, 0xc8df, GB_LVT, INCB_NONE, 0},
  {0xc8e0, 0xc8e0, GB_LV, INCB_NONE, 0}, {0xc8e1, 0xc8fb, GB_LVT, INCB_NONE, 0}, {0xc8fc, 0xc8fc, GB_LV, INCB_NONE, 0},
  {0xc8fd, 0xc917, GB_LVT, INCB_NONE, 0}, {0xc918, 0xc918, GB_LV, INCB_NONE, 0}, {0xc919, 0xc933, GB_LVT, INCB_NONE, 0},
  {0xc934, 0xc934, GB_LV, INCB_NONE, 0}, {0xc935, 0xc94f, GB_LVT, INCB_NONE, 0}, {0xc950, 0xc950, GB_LV, INCB_NONE, 0},
  {0xc951, 0xc96b, GB_LVT, INCB_NONE, 0}, {0xc96c, 0xc96c, GB_LV, INCB_NONE, 0}, {0xc96d, 0xc987, GB_LVT, INCB_NONE, 0},
  {0xc988, 0xc988, GB_LV, INCB_NONE, 0}, {0xc989, 0xc9a3, GB_LVT, INCB_NONE, 0}, {0xc9a4, 0xc9a4, GB_LV, INCB_NONE, 0},
  {0xc9a5, 0xc9bf, GB_LVT, INCB_NONE, 0}, {0xc9c0, 0xc9c0, GB_LV, INCB_NONE, 0}, {0xc9c1, 0xc9db, GB_LVT, INCB_NONE, 0},
  {0xc9dc, 0xc9dc, GB_LV, INCB_NONE, 0}, {0xc9dd, 0xc9f7, GB_LVT, INCB_NONE, 0}, {0xc9f8, 0xc9f8, GB_LV, INCB_NONE, 0},
  {0xc9f9, 0xca13, GB_LVT, INCB_NONE, 0}, {0xca14, 0xca14, GB_LV, INCB_NONE, 0}, {0xca15, 0xca2f, GB_LVT, INCB_NONE, 0},
  {0xca30, 0xca30, GB_LV, INCB_NONE, 0}, {0xca31, 0xca4b, GB_LVT, INCB_NONE, 0}, {0xca4c, 0xca4c, GB_LV, INCB_NONE, 0},
  {0xca4d, 0xca67, GB_LVT, INCB_NONE, 0}, {0xca68, 0xca68, GB_LV, INCB_NONE, 0}, {0xca69, 0xca83, GB_LVT, INCB_NONE, 0},
  {0xca84, 0xca84, GB_LV, INCB_NONE, 0}, {0xca85, 0xca9f, GB_LVT, INCB_NONE, 0}, {0xcaa0, 0xcaa0, GB_LV, INCB_NONE, 0},
  {0xcaa1, 0xcabb, GB_LVT, INCB_NONE, 0}, {0xcabc, 0xcabc, GB_LV, INCB_NONE, 0}, {0xcabd, 0xcad7, GB_LVT, INCB_NONE, 0},
  {0xcad8, 0xcad8, GB_LV, INCB_NONE, 0}, {0xcad9, 0xcaf3, GB_LVT, INCB_NONE, 0}, {0xcaf4, 0xcaf4, GB_LV, INCB_NONE, 0},
  {0xcaf5, 0xcb0f, GB_LVT, INCB_NONE, 0}, {0xcb10, 0xcb10, GB_LV, INCB_NONE, 0}, {0xcb11, 0xcb2b, GB_LVT, INCB_NONE, 0},
  {0xcb2c, 0xcb2c, GB_LV, INCB_NONE, 0}, {0xcb2d, 0xcb47, GB_LVT, INCB_NONE, 0}, {0xcb48, 0xcb48, GB_LV, INCB_NONE, 0},
  {0xcb49, 0xcb63, GB_LVT, INCB_NONE, 0}, {0xcb64, 0xcb64, GB_LV, INCB_NONE, 0}, {0xcb65, 0xcb7f, GB_LVT, INCB_NONE, 0},
  {0xcb80, 0xcb80, GB_LV, INCB_NONE, 0}, {0xcb81, 0xcb9b, GB_LVT, INCB_NONE, 0}, {0xcb9c, 0xcb9c, GB_LV, INCB_NONE, 0},
  {0xcb9d, 0xcbb7, GB_LVT, INCB_NONE, 0}, {0xcbb8, 0xcbb8, GB_LV, INCB_NONE, 0}, {0xcbb9, 0xcbd3, GB_LVT, INCB_NONE, 0},
  {0xcbd4, 0xcbd4, GB_LV, INCB_NONE, 0}, {0xcbd5, 0xcbef, GB_LVT, INCB_NONE, 0}, {0xcbf0, 0xcbf0, GB_LV, INCB_NONE, 0},
  {0xcbf1, 0xcc0b, GB_LVT, INCB_NONE, 0}, {0xcc0c, 0xcc0c, GB_LV, INCB_NONE, 0}, {0xcc0d, 0xcc27, GB_LVT, INCB_NONE, 0},
  {0xcc28, 0xcc28, GB_LV, INCB_NONE, 0}, {0xcc29, 0xcc43, GB_LVT, INCB_NONE, 0}, {0xcc44, 0xcc44, GB_LV, INCB_NONE, 0},
  {0xcc45, 0xcc5f, GB_LVT, INCB_NONE, 0}, {0xcc60, 0xcc60, GB_LV, INCB_NONE, 0}, {0xcc61, 0xcc7b, GB_LVT, INCB_NONE, 0},
  {0xcc7c, 0xcc7c, GB_LV, INCB_NONE, 0}, {0xcc7d, 0xcc97, GB_LVT, INCB_NONE, 0}, {0xcc98, 0xcc98, GB_LV, INCB_NONE, 0},
  {0xcc99, 0xccb3, GB_LVT, INCB_NONE, 0}, {0xccb4, 0xccb4, GB_LV, INCB_NONE, 0}, {0xccb5, 0xcccf, GB_LVT, INCB_NONE, 0},
  {0xccd0, 0xccd0, GB_LV, INCB_NONE, 0}, {0xccd1, 0xcceb, GB_LVT, INCB_NONE, 0}, {0xccec, 0xccec, GB_LV, INCB_NONE, 0},
  {0xcced, 0xcd07, GB_LVT, INCB_NONE, 0}, {0xcd08, 0xcd08, GB_LV, INCB_NONE, 0}, {0xcd09, 0xcd23, GB_LVT, INCB_NONE, 0},
  {0xcd24, 0xcd24, GB_LV, INCB_NONE, 0}, {0xcd25, 0xcd3f, GB_LVT, INCB_NONE, 0}, {0xcd40, 0xcd40, GB_LV, INCB_NONE, 0},
  {0xcd41, 0xcd5b, GB_LVT, INCB_NONE, 0}, {0xcd5c, 0xcd5c, GB_LV, INCB_NONE, 0}, {0xcd5d, 0xcd77, GB_LVT, INCB_NONE, 0},
  {0xcd78, 0xcd78, GB_LV, INCB_NONE, 0}, {0xcd79, 0xcd93, GB_LVT, INCB_NONE, 0}, {0xcd94, 0xcd94, GB_LV, INCB_NONE, 0},
  {0xcd95, 0xcdaf, GB_LVT, INCB_NONE, 0}, {0xcdb0, 0xcdb0, GB_LV, INCB_NONE, 0}, {0xcdb1, 0xcdcb, GB_LVT, INCB_NONE, 0},
  {0xcdcc, 0xcdcc, GB_LV, INCB_NONE, 0}, {0xcdcd, 0xcde7, GB_LVT, INCB_NONE, 0}, {0xcde8, 0xcde8, GB_LV, INCB_NONE, 0},
  {0xcde9, 0xce03, GB_LVT, INCB_NONE, 0}, {0xce04, 0xce04, GB_LV, INCB_NONE, 0}, {0xce05, 0xce1f, GB_LVT, INCB_NONE, 0},
  {0xce20, 0xce20, GB_LV, INCB_NONE, 0}, {0xce21, 0xce3b, GB_LVT, INCB_NONE, 0}, {0xce3c, 0xce3c, GB_LV, INCB_NONE, 0},
  {0xce3d, 0xce57, GB_LVT, INCB_NONE, 0}, {0xce58, 0xce58, GB_LV, INCB_NONE, 0}, {0xce59, 0xce73, GB_LVT, INCB_NONE, 0},
  {0xce74, 0xce74, GB_LV, INCB_NONE, 0}, {0xce75, 0xce8f, GB_LVT, INCB_NONE, 0}, {0xce90, 0xce90, GB_LV, INCB_NONE, 0},
  {0xce91, 0xceab, GB_LVT, INCB_NONE, 0}, {0xceac, 0xceac, GB_LV, INCB_NONE, 0}, {0xcead, 0xcec7, GB_LVT, INCB_NONE, 0},
  {0xcec8, 0xcec8, GB_LV, INCB_NONE, 0}, {0xcec9, 0xcee3, GB_LVT, INCB_NONE, 0}, {0xcee4, 0xcee4, GB_LV, INCB_NONE, 0},
  {0xcee5, 0xceff, GB_LVT, INCB_NONE, 0}, {0xcf00, 0xcf00, GB_LV, INCB_NONE, 0}, {0xcf01, 0xcf1b, GB_LVT, INCB_NONE, 0},
  {0xcf1c, 0xcf1c, GB_LV, INCB_NONE, 0}, {0xcf1d, 0xcf37, GB_LVT, INCB_NONE, 0}, {0xcf38, 0xcf38, GB_LV, INCB_NONE, 0},
  {0xcf39, 0xcf53, GB_LVT, INCB_NONE, 0}, {0xcf54, 0xcf54, GB_LV, INCB_NONE, 0}, {0xcf55, 0xcf6f, GB_LVT, INCB_NONE, 0},
  {0xcf70, 0xcf70, GB_LV, INCB_NONE, 0}, {0xcf71, 0xcf8b, GB_LVT, INCB_NONE, 0}, {0xcf8c, 0xcf8c, GB_LV, INCB_NONE, 0},
  {0xcf8d, 0xcfa7, GB_LVT, INCB_NONE, 0}, {0xcfa8, 0xcfa8, GB_LV, INCB_NONE, 0}, {0xcfa9, 0xcfc3, GB_LVT, INCB_NONE, 0},
  {0xcfc4, 0xcfc4, GB_LV, INCB_NONE, 0}, {0xcfc5, 0xcfdf, GB_LVT, INCB_NONE, 0}, {0xcfe0, 0xcfe0, GB_LV, INCB_NONE, 0},
  {0xcfe1, 0xcffb, GB_LVT, INCB_NONE, 0}, {0xcffc, 0xcffc, GB_LV, INCB_NONE, 0}, {0xcffd, 0xd017, GB_LVT, INCB_NONE, 0},
  {0xd018, 0xd018, GB_LV, INCB_NONE, 0}, {0xd019, 0xd033, GB_LVT, INCB_NONE, 0}, {0xd034, 0xd034, GB_LV, INCB_NONE, 0},
  {0xd035, 0xd04f, GB_LVT, INCB_NONE, 0}, {0xd050, 0xd050, GB_LV, INCB_NONE, 0}, {0xd051, 0xd06b, GB_LVT, INCB_NONE, 0},
  {0xd06c, 0xd06c, GB_LV, INCB_NONE, 0}, {0xd06d, 0xd087, GB_LVT, INCB_NONE, 0}, {0xd088, 0xd088, GB_LV, INCB_NONE, 0},
  {0xd089, 0xd0a3, GB_LVT, INCB_NONE, 0}, {0xd0a4, 0xd0a4, GB_LV, INCB_NONE, 0}, {0xd0a5, 0xd0bf, GB_LVT, INCB_NONE, 0},
  {0xd0c0, 0xd0c0, GB_LV, INCB_NONE, 0}, {0xd0c1, 0xd0db, GB_LVT, INCB_NONE, 0}, {0xd0dc, 0xd0dc, GB_LV, INCB_NONE, 0},
  {0xd0dd, 0xd0f7, GB_LVT, INCB_NONE, 0}, {0xd0f8, 0xd0f8, GB_LV, INCB_NONE, 0}, {0xd0f9, 0xd113, GB_LVT, INCB_NONE, 0},
  {0xd114, 0xd114, GB_LV, INCB_NONE, 0}, {0xd115, 0xd12f, GB_LVT, INCB_NONE, 0}, {0xd130, 0xd130, GB_LV, INCB_NONE, 0},
  {0xd131, 0xd14b, GB_LVT, INCB_NONE, 0}, {0xd14c, 0xd14c, GB_LV, INCB_NONE, 0}, {0xd14d, 0xd167, GB_LVT, INCB_NONE, 0},
  {0xd168, 0xd168, GB_LV, INCB_NONE, 0}, {0xd169, 0xd183, GB_LVT, INCB_NONE, 0}, {0xd184, 0xd184, GB_LV, INCB_NONE, 0},
  {0xd185, 0xd19f, GB_LVT, INCB_NONE, 0}, {0xd1a0, 0xd1a0, GB_LV, INCB_NONE, 0}, {0xd1a1, 0xd1bb, GB_LVT, INCB_NONE, 0},
  {0xd1bc, 0xd1bc, GB_LV, INCB_NONE, 0}, {0xd1bd, 0xd1d7, GB_LVT, INCB_NONE, 0}, {0xd1d8, 0xd1d8, GB_LV, INCB_NONE, 0},
  {0xd1d9, 0xd1f3, GB_LVT, INCB_NONE, 0}, {0xd1f4, 0xd1f4, GB_LV, INCB_NONE, 0}, {0xd1f5, 0xd20f, GB_LVT, INCB_NONE, 0},
  {0xd210, 0xd210, GB_LV, INCB_NONE, 0}, {0xd211, 0xd22b, GB_LVT, INCB_NONE, 0}, {0xd22c, 0xd22c, GB_LV, INCB_NONE, 0},
  {0xd22d, 0xd247, GB_LVT, INCB_NONE, 0}, {0xd248, 0xd248, GB_LV, INCB_NONE, 0}, {0xd249, 0xd263, GB_LVT, INCB_NONE, 0},
  {0xd264, 0xd264, GB_LV, INCB_NONE, 0}, {0xd265, 0xd27f, GB_LVT, INCB_NONE, 0}, {0xd280, 0xd280, GB_LV, INCB_NONE, 0},
  {0xd281, 0xd29b, GB_LVT, INCB_NONE, 0}, {0xd29c, 0xd29c, GB_LV, INCB_NONE, 0}, {0xd29d, 0xd2b7, GB_LVT, INCB_NONE, 0},
  {0xd2b8, 0xd2b8, GB_LV, INCB_NONE, 0}, {0xd2b9, 0xd2d3, GB_LVT, INCB_NONE, 0}, {0xd2d4, 0xd2d4, GB_LV, INCB_NONE, 0},
  {0xd2d5, 0xd2ef, GB_LVT, INCB_NONE, 0}, {0xd2f0, 0xd2f0, GB_LV, INCB_NONE, 0}, {0xd2f1, 0xd30b, GB_LVT, INCB_NONE, 0},
  {0xd30c, 0xd30c, GB_LV, INCB_NONE, 0}, {0xd30d, 0xd327, GB_LVT, INCB_NONE, 0}, {0xd328, 0xd328, GB_LV, INCB_NONE, 0},
  {0xd329, 0xd343, GB_LVT, INCB_NONE, 0}, {0xd344, 0xd344, GB_LV, INCB_NONE, 0}, {0xd345, 0xd35f, GB_LVT, INCB_NONE, 0},
  {0xd360, 0xd360, GB_LV, INCB_NONE, 0}, {0xd361, 0xd37b, GB_LVT, INCB_NONE, 0}, {0xd37c, 0xd37c, GB_LV, INCB_NONE, 0},
  {0xd37d, 0xd397, GB_LVT, INCB_NONE, 0}, {0xd398, 0xd398, GB_LV, INCB_NONE, 0}, {0xd399, 0xd3b3, GB_LVT, INCB_NONE, 0},
  {0xd3b4, 0xd3b4, GB_LV, INCB_NONE, 0}, {0xd3b5, 0xd3cf, GB_LVT, INCB_NONE, 0}, {0xd3d0, 0xd3d0, GB_LV, INCB_NONE, 0},
  {0xd3d1, 0xd3eb, GB_LVT, INCB_NONE, 0}, {0xd3ec, 0xd3ec, GB_LV, INCB_NONE, 0}, {0xd3ed, 0xd407, GB_LVT, INCB_NONE, 0},
  {0xd408, 0xd408, GB_LV, INCB_NONE, 0}, {0xd409, 0xd423, GB_LVT, INCB_NONE, 0}, {0xd424, 0xd424, GB_LV, INCB_NONE, 0},
  {0xd425, 0xd43f, GB_LVT, INCB_NONE, 0}, {0xd440, 0xd440, GB_LV, INCB_NONE, 0}, {0xd441, 0xd45b, GB_LVT, INCB_NONE, 0},
  {0xd45c, 0xd45c, GB_LV, INCB_NONE, 0}, {0xd45d, 0xd477, GB_LVT, INCB_NONE, 0}, {0xd478, 0xd478, GB_LV, INCB_NONE, 0},
  {0xd479, 0xd493, GB_LVT, INCB_NONE, 0}, {0xd494, 0xd494, GB_LV, INCB_NONE, 0}, {0xd495, 0xd4af, GB_LVT, INCB_NONE, 0},
  {0xd4b0, 0xd4b0, GB_LV, INCB_NONE, 0}, {0xd4b1, 0xd4cb, GB_LVT, INCB_NONE, 0}, {0xd4cc, 0xd4cc, GB_LV, INCB_NONE, 0},
  {0xd4cd, 0xd4e7, GB_LVT, INCB_NONE, 0}, {0xd4e8, 0xd4e8, GB_LV, INCB_NONE, 0}, {0xd4e9, 0xd503, GB_LVT, INCB_NONE, 0},
  {0xd504, 0xd504, GB_LV, INCB_NONE, 0}, {0xd505, 0xd51f, GB_LVT, INCB_NONE, 0}, {0xd520, 0xd520, GB_LV, INCB_NONE, 0},
  {0xd521, 0xd53b, GB_LVT, INCB_NONE, 0}, {0xd53c, 0xd53c, GB_LV, INCB_NONE, 0}, {0xd53d, 0xd557, GB_LVT, INCB_NONE, 0},
  {0xd558, 0xd558, GB_LV, INCB_NONE, 0}, {0xd559, 0xd573, GB_LVT, INCB_NONE, 0}, {0xd574, 0xd574, GB_LV, INCB_NONE, 0},
  {0xd575, 0xd58f, GB_LVT, INCB_NONE, 0}, {0xd590, 0xd590, GB_LV, INCB_NONE, 0}, {0xd591, 0xd5ab, GB_LVT, INCB_NONE, 0},
  {0xd5ac, 0xd5ac, GB_LV, INCB_NONE, 0}, {0xd5ad, 0xd5c7, GB_LVT, INCB_NONE, 0}, {0xd5c8, 0xd5c8, GB_LV, INCB_NONE, 0},
  {0xd5c9, 0xd5e3, GB_LVT, INCB_NONE, 0}, {0xd5e4, 0xd5e4, GB_LV, INCB_NONE, 0}, {0xd5e5, 0xd5ff, GB_LVT, INCB_NONE, 0},
  {0xd600, 0xd600, GB_LV, INCB_NONE, 0}, {0xd601, 0xd61b, GB_LVT, INCB_NONE, 0}, {0xd61c, 0xd61c, GB_LV, INCB_NONE, 0},
  {0xd61d, 0xd637, GB_LVT, INCB_NONE, 0}, {0xd638, 0xd638, GB_LV, INCB_NONE, 0}, {0xd639, 0xd653, GB_LVT, INCB_NONE, 0},
  {0xd654, 0xd654, GB_LV, INCB_NONE, 0}, {0xd655, 0xd66f, GB_LVT, INCB_NONE, 0}, {0xd670, 0xd670, GB_LV, INCB_NONE, 0},
  {0xd671, 0xd68b, GB_LVT, INCB_NONE, 0}, {0xd68c, 0xd68c, GB_LV, INCB_NONE, 0}, {0xd68d, 0xd6a7, GB_LVT, INCB_NONE, 0},
  {0xd6a8, 0xd6a8, GB_LV, INCB_NONE, 0}, {0xd6a9, 0xd6c3, GB_LVT, INCB_NONE, 0}, {0xd6c4, 0xd6c4, GB_LV, INCB_NONE, 0},
  {0xd6c5, 0xd6df, GB_LVT, INCB_NONE, 0}, {0xd6e0, 0xd6e0, GB_LV, INCB_NONE, 0}, {0xd6e1, 0xd6fb, GB_LVT, INCB_NONE, 0},
  {0xd6fc, 0xd6fc, GB_LV, INCB_NONE, 0}, {0xd6fd, 0xd717, GB_LVT, INCB_NONE, 0}, {0xd718, 0xd718, GB_LV, INCB_NONE, 0},
  {0xd719, 0xd733, GB_LVT, INCB_NONE, 0}, {0xd734, 0xd734, GB_LV, INCB_NONE, 0}, {0xd735, 0xd74f, GB_LVT, INCB_NONE, 0},
  {0xd750, 0xd750, GB_LV, INCB_NONE, 0}, {0xd751, 0xd76b, GB_LVT, INCB_NONE, 0}, {0xd76c, 0xd76c, GB_LV, INCB_NONE, 0},
  {0xd76d, 0xd787, GB_LVT, INCB_NONE, 0}, {0xd788, 0xd788, GB_LV, INCB_NONE, 0}, {0xd789, 0xd7a3, GB_LVT, INCB_NONE, 0},
  {0xd7b0, 0xd7c6, GB_V, INCB_NONE, 0}, {0xd7cb, 0xd7fb, GB_T, INCB_NONE, 0}, {0xfb1e, 0xfb1e, GB_EXTEND, INCB_EXTEND, 0},
  {0xfe00, 0xfe0f, GB_EXTEND, INCB_EXTEND, 0}, {0xfe20, 0xfe2f, GB_EXTEND, INCB_EXTEND, 0}, {0xfeff, 0xfeff, GB_CONTROL, INCB_NONE, 0},
  {0xff9e, 0xff9f, GB_EXTEND, INCB_EXTEND, 0}, {0xfff0, 0xfffb, GB_CONTROL, INCB_NONE, 0}, {0x101fd, 0x101fd, GB_EXTEND, INCB_EXTEND, 0},
  {0x102e0, 0x102e0, GB_EXTEND, INCB_EXTEND, 0}, {0x10376, 0x1037a, GB_EXTEND, INCB_EXTEND, 0}, {0x10a00, 0x10a00, GB_OTHER, INCB_CONSONANT, 0},
  {0x10a01, 0x10a03, GB_EXTEND, INCB_EXTEND, 0}, {0x10a05, 0x10a06, GB_EXTEND, INCB_EXTEND, 0}, {0x10a0c, 0x10a0f, GB_EXTEND, INCB_EXTEND, 0},
  {0x10a10, 0x10a13, GB_OTHER, INCB_CONSONANT, 0}, {0x10a15, 0x10a17, GB_OTHER, INCB_CONSONANT, 0}, {0x10a19, 0x10a35, GB_OTHER, INCB_CONSONANT, 0},
  {0x10a38, 0x10a3a, GB_EXTEND, INCB_EXTEND, 0}, {0x10a3f, 0x10a3f, GB_EXTEND, INCB_LINKER, 0}, {0x10ae5, 0x10ae6, GB_EXTEND, INCB_EXTEND, 0},
  {0x10d24, 0x10d27, GB_EXTEND, INCB_EXTEND, 0}, {0x10d69, 0x10d6d, GB_EXTEND, INCB_EXTEND, 0}, {0x10eab, 0x10eac, GB_EXTEND, INCB_EXTEND, 0},
  {0x10efa, 0x10eff, GB_EXTEND, INCB_EXTEND, 0}, {0x10f46, 0x10f50, GB_EXTEND, INCB_EXTEND, 0}, {0x10f82, 0x10f85, GB_EXTEND, INCB_EXTEND, 0},
  {0x11000, 0x11000, GB_SPACING_MARK, INCB_NONE, 0}, {0x11001, 0x11001, GB_EXTEND, INCB_EXTEND, 0}, {0x11002, 0x11002, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11038, 0x11046, GB_EXTEND, INCB_EXTEND, 0}, {0x11070, 0x11070, GB_EXTEND, INCB_EXTEND, 0}, {0x11073, 0x11074, GB_EXTEND, INCB_EXTEND, 0},
  {0x1107f, 0x11081, GB_EXTEND, INCB_EXTEND, 0}, {0x11082, 0x11082, GB_SPACING_MARK, INCB_NONE, 0}, {0x110b0, 0x110b2, GB_SPACING_MARK, INCB_NONE, 0},
  {0x110b3, 0x110b6, GB_EXTEND, INCB_EXTEND, 0}, {0x110b7, 0x110b8, GB_SPACING_MARK, INCB_NONE, 0}, {0x110b9, 0x110ba, GB_EXTEND, INCB_EXTEND, 0},
  {0x110bd, 0x110bd, GB_PREPEND, INCB_NONE, 0}, {0x110c2, 0x110c2, GB_EXTEND, INCB_EXTEND, 0}, {0x110cd, 0x110cd, GB_PREPEND, INCB_NONE, 0},
  {0x11100, 0x11102, GB_EXTEND, INCB_EXTEND, 0}, {0x11103, 0x11126, GB_OTHER, INCB_CONSONANT, 0}, {0x11127, 0x1112b, GB_EXTEND, INCB_EXTEND, 0},
  {0x1112c, 0x1112c, GB_SPACING_MARK, INCB_NONE, 0}, {0x1112d, 0x11132, GB_EXTEND, INCB_EXTEND, 0}, {0x11133, 0x11133, GB_EXTEND, INCB_LINKER, 0},
  {0x11134, 0x11134, GB_EXTEND, INCB_EXTEND, 0}, {0x11144, 0x11144, GB_OTHER, INCB_CONSONANT, 0}, {0x11145, 0x11146, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11147, 0x11147, GB_OTHER, INCB_CONSONANT, 0}, {0x11173, 0x11173, GB_EXTEND, INCB_EXTEND, 0}, {0x11180, 0x11181, GB_EXTEND, INCB_EXTEND, 0},
  {0x11182, 0x11182, GB_SPACING_MARK, INCB_NONE, 0}, {0x111b3, 0x111b5, GB_SPACING_MARK, INCB_NONE, 0}, {0x111b6, 0x111be, GB_EXTEND, INCB_EXTEND, 0},
  {0x111bf, 0x111bf, GB_SPACING_MARK, INCB_NONE, 0}, {0x111c0, 0x111c0, GB_EXTEND, INCB_EXTEND, 0}, {0x111c2, 0x111c3, GB_PREPEND, INCB_NONE, 0},
  {0x111c9, 0x111cc, GB_EXTEND, INCB_EXTEND, 0}, {0x111ce, 0x111ce, GB_SPACING_MARK, INCB_NONE, 0}, {0x111cf, 0x111cf, GB_EXTEND, INCB_EXTEND, 0},
  {0x1122c, 0x1122e, GB_SPACING_MARK, INCB_NONE, 0}, {0x1122f, 0x11231, GB_EXTEND, INCB_EXTEND, 0}, {0x11232, 0x11233, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11234, 0x11237, GB_EXTEND, INCB_EXTEND, 0}, {0x1123e, 0x1123e, GB_EXTEND, INCB_EXTEND, 0}, {0x11241, 0x11241, GB_EXTEND, INCB_EXTEND, 0},
  {0x112df, 0x112df, GB_EXTEND, INCB_EXTEND, 0}, {0x112e0, 0x112e2, GB_SPACING_MARK, INCB_NONE, 0}, {0x112e3, 0x112ea, GB_EXTEND, INCB_EXTEND, 0},
  {0x11300, 0x11301, GB_EXTEND, INCB_EXTEND, 0}, {0x11302, 0x11303, GB_SPACING_MARK, INCB_NONE, 0}, {0x1133b, 0x1133c, GB_EXTEND, INCB_EXTEND, 0},
  {0x1133e, 0x1133e, GB_EXTEND, INCB_EXTEND, 0}, {0x1133f, 0x1133f, GB_SPACING_MARK, INCB_NONE, 0}, {0x11340, 0x11340, GB_EXTEND, INCB_EXTEND, 0},
  {0x11341, 0x11344, GB_SPACING_MARK, INCB_NONE, 0}, {0x11347, 0x11348, GB_SPACING_MARK, INCB_NONE, 0}, {0x1134b, 0x1134c, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1134d, 0x1134d, GB_EXTEND, INCB_EXTEND, 0}, {0x11357, 0x11357, GB_EXTEND, INCB_EXTEND, 0}, {0x11362, 0x11363, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11366, 0x1136c, GB_EXTEND, INCB_EXTEND, 0}, {0x11370, 0x11374, GB_EXTEND, INCB_EXTEND, 0}, {0x11380, 0x11389, GB_OTHER, INCB_CONSONANT, 0},
  {0x1138b, 0x1138b, GB_OTHER, INCB_CONSONANT, 0}, {0x1138e, 0x1138e, GB_OTHER, INCB_CONSONANT, 0}, {0x11390, 0x113b5, GB_OTHER, INCB_CONSONANT, 0},
  {0x113b8, 0x113b8, GB_EXTEND, INCB_EXTEND, 0}, {0x113b9, 0x113ba, GB_SPACING_MARK, INCB_NONE, 0}, {0x113bb, 0x113c0, GB_EXTEND, INCB_EXTEND, 0},
  {0x113c2, 0x113c2, GB_EXTEND, INCB_EXTEND, 0}, {0x113c5, 0x113c5, GB_EXTEND, INCB_EXTEND, 0}, {0x113c7, 0x113c9, GB_EXTEND, INCB_EXTEND, 0},
  {0x113ca, 0x113ca, GB_SPACING_MARK, INCB_NONE, 0}, {0x113cc, 0x113cd, GB_SPACING_MARK, INCB_NONE, 0}, {0x113ce, 0x113cf, GB_EXTEND, INCB_EXTEND, 0},
  {0x113d0, 0x113d0, GB_EXTEND, INCB_LINKER, 0}, {0x113d1, 0x113d1, GB_PREPEND, INCB_NONE, 0}, {0x113d2, 0x113d2, GB_EXTEND, INCB_EXTEND, 0},
  {0x113e1, 0x113e2, GB_EXTEND, INCB_EXTEND, 0}, {0x11435, 0x11437, GB_SPACING_MARK, INCB_NONE, 0}, {0x11438, 0x1143f, GB_EXTEND, INCB_EXTEND, 0},
  {0x11440, 0x11441, GB_SPACING_MARK, INCB_NONE, 0}, {0x11442, 0x11444, GB_EXTEND, INCB_EXTEND, 0}, {0x11445, 0x11445, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11446, 0x11446, GB_EXTEND, INCB_EXTEND, 0}, {0x1145e, 0x1145e, GB_EXTEND, INCB_EXTEND, 0}, {0x114b0, 0x114b0, GB_EXTEND, INCB_EXTEND, 0},
  {0x114b1, 0x114b2, GB_SPACING_MARK, INCB_NONE, 0}, {0x114b3, 0x114b8, GB_EXTEND, INCB_EXTEND, 0}, {0x114b9, 0x114b9, GB_SPACING_MARK, INCB_NONE, 0},
  {0x114ba, 0x114ba, GB_EXTEND, INCB_EXTEND, 0}, {0x114bb, 0x114bc, GB_SPACING_MARK, INCB_NONE, 0}, {0x114bd, 0x114bd, GB_EXTEND, INCB_EXTEND, 0},
  {0x114be, 0x114be, GB_SPACING_MARK, INCB_NONE, 0}, {0x114bf, 0x114c0, GB_EXTEND, INCB_EXTEND, 0}, {0x114c1, 0x114c1, GB_SPACING_MARK, INCB_NONE, 0},
  {0x114c2, 0x114c3, GB_EXTEND, INCB_EXTEND, 0}, {0x115af, 0x115af, GB_EXTEND, INCB_EXTEND, 0}, {0x115b0, 0x115b1, GB_SPACING_MARK, INCB_NONE, 0},
  {0x115b2, 0x115b5, GB_EXTEND, INCB_EXTEND, 0}, {0x115b8, 0x115bb, GB_SPACING_MARK, INCB_NONE, 0}, {0x115bc, 0x115bd, GB_EXTEND, INCB_EXTEND, 0},
  {0x115be, 0x115be, GB_SPACING_MARK, INCB_NONE, 0}, {0x115bf, 0x115c0, GB_EXTEND, INCB_EXTEND, 0}, {0x115dc, 0x115dd, GB_EXTEND, INCB_EXTEND, 0},
  {0x11630, 0x11632, GB_SPACING_MARK, INCB_NONE, 0}, {0x11633, 0x1163a, GB_EXTEND, INCB_EXTEND, 0}, {0x1163b, 0x1163c, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1163d, 0x1163d, GB_EXTEND, INCB_EXTEND, 0}, {0x1163e, 0x1163e, GB_SPACING_MARK, INCB_NONE, 0}, {0x1163f, 0x11640, GB_EXTEND, INCB_EXTEND, 0},
  {0x116ab, 0x116ab, GB_EXTEND, INCB_EXTEND, 0}, {0x116ac, 0x116ac, GB_SPACING_MARK, INCB_NONE, 0}, {0x116ad, 0x116ad, GB_EXTEND, INCB_EXTEND, 0},
  {0x116ae, 0x116af, GB_SPACING_MARK, INCB_NONE, 0}, {0x116b0, 0x116b7, GB_EXTEND, INCB_EXTEND, 0}, {0x1171d, 0x1171d, GB_EXTEND, INCB_EXTEND, 0},
  {0x1171e, 0x1171e, GB_SPACING_MARK, INCB_NONE, 0}, {0x1171f, 0x1171f, GB_EXTEND, INCB_EXTEND, 0}, {0x11722, 0x11725, GB_EXTEND, INCB_EXTEND, 0},
  {0x11726, 0x11726, GB_SPACING_MARK, INCB_NONE, 0}, {0x11727, 0x1172b, GB_EXTEND, INCB_EXTEND, 0}, {0x1182c, 0x1182e, GB_SPACING_MARK, INCB_NONE, 0},
  {0x1182f, 0x11837, GB_EXTEND, INCB_EXTEND, 0}, {0x11838, 0x11838, GB_SPACING_MARK, INCB_NONE, 0}, {0x11839, 0x1183a, GB_EXTEND, INCB_EXTEND, 0},
  {0x11900, 0x11906, GB_OTHER, INCB_CONSONANT, 0}, {0x11909, 0x11909, GB_OTHER, INCB_CONSONANT, 0}, {0x1190c, 0x11913, GB_OTHER, INCB_CONSONANT, 0},
  {0x11915, 0x11916, GB_OTHER, INCB_CONSONANT, 0}, {0x11918, 0x1192f, GB_OTHER, INCB_CONSONANT, 0}, {0x11930, 0x11930, GB_EXTEND, INCB_EXTEND, 0},
  {0x11931, 0x11935, GB_SPACING_MARK, INCB_NONE, 0}, {0x11937, 0x11938, GB_SPACING_MARK, INCB_NONE, 0}, {0x1193b, 0x1193d, GB_EXTEND, INCB_EXTEND, 0},
  {0x1193e, 0x1193e, GB_EXTEND, INCB_LINKER, 0}, {0x1193f, 0x1193f, GB_PREPEND, INCB_NONE, 0}, {0x11940, 0x11940, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11941, 0x11941, GB_PREPEND, INCB_NONE, 0}, {0x11942, 0x11942, GB_SPACING_MARK, INCB_NONE, 0}, {0x11943, 0x11943, GB_EXTEND, INCB_EXTEND, 0},
  {0x119d1, 0x119d3, GB_SPACING_MARK, INCB_NONE, 0}, {0x119d4, 0x119d7, GB_EXTEND, INCB_EXTEND, 0}, {0x119da, 0x119db, GB_EXTEND, INCB_EXTEND, 0},
  {0x119dc, 0x119df, GB_SPACING_MARK, INCB_NONE, 0}, {0x119e0, 0x119e0, GB_EXTEND, INCB_EXTEND, 0}, {0x119e4, 0x119e4, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11a00, 0x11a00, GB_OTHER, INCB_CONSONANT, 0}, {0x11a01, 0x11a0a, GB_EXTEND, INCB_EXTEND, 0}, {0x11a0b, 0x11a32, GB_OTHER, INCB_CONSONANT, 0},
  {0x11a33, 0x11a38, GB_EXTEND, INCB_EXTEND, 0}, {0x11a39, 0x11a39, GB_SPACING_MARK, INCB_NONE, 0}, {0x11a3b, 0x11a3e, GB_EXTEND, INCB_EXTEND, 0},
  {0x11a47, 0x11a47, GB_EXTEND, INCB_LINKER, 0}, {0x11a50, 0x11a50, GB_OTHER, INCB_CONSONANT, 0}, {0x11a51, 0x11a56, GB_EXTEND, INCB_EXTEND, 0},
  {0x11a57, 0x11a58, GB_SPACING_MARK, INCB_NONE, 0}, {0x11a59, 0x11a5b, GB_EXTEND, INCB_EXTEND, 0}, {0x11a5c, 0x11a83, GB_OTHER, INCB_CONSONANT, 0},
  {0x11a84, 0x11a89, GB_PREPEND, INCB_NONE, 0}, {0x11a8a, 0x11a96, GB_EXTEND, INCB_EXTEND, 0}, {0x11a97, 0x11a97, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11a98, 0x11a98, GB_EXTEND, INCB_EXTEND, 0}, {0x11a99, 0x11a99, GB_EXTEND, INCB_LINKER, 0}, {0x11b60, 0x11b60, GB_EXTEND, INCB_EXTEND, 0},
  {0x11b61, 0x11b61, GB_SPACING_MARK, INCB_NONE, 0}, {0x11b62, 0x11b64, GB_EXTEND, INCB_EXTEND, 0}, {0x11b65, 0x11b65, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11b66, 0x11b66, GB_EXTEND, INCB_EXTEND, 0}, {0x11b67, 0x11b67, GB_SPACING_MARK, INCB_NONE, 0}, {0x11c2f, 0x11c2f, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11c30, 0x11c36, GB_EXTEND, INCB_EXTEND, 0}, {0x11c38, 0x11c3d, GB_EXTEND, INCB_EXTEND, 0}, {0x11c3e, 0x11c3e, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11c3f, 0x11c3f, GB_EXTEND, INCB_EXTEND, 0}, {0x11c92, 0x11ca7, GB_EXTEND, INCB_EXTEND, 0}, {0x11ca9, 0x11ca9, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11caa, 0x11cb0, GB_EXTEND, INCB_EXTEND, 0}, {0x11cb1, 0x11cb1, GB_SPACING_MARK, INCB_NONE, 0}, {0x11cb2, 0x11cb3, GB_EXTEND, INCB_EXTEND, 0},
  {0x11cb4, 0x11cb4, GB_SPACING_MARK, INCB_NONE, 0}, {0x11cb5, 0x11cb6, GB_EXTEND, INCB_EXTEND, 0}, {0x11d31, 0x11d36, GB_EXTEND, INCB_EXTEND, 0},
  {0x11d3a, 0x11d3a, GB_EXTEND, INCB_EXTEND, 0}, {0x11d3c, 0x11d3d, GB_EXTEND, INCB_EXTEND, 0}, {0x11d3f, 0x11d45, GB_EXTEND, INCB_EXTEND, 0},
  {0x11d46, 0x11d46, GB_PREPEND, INCB_NONE, 0}, {0x11d47, 0x11d47, GB_EXTEND, INCB_EXTEND, 0}, {0x11d8a, 0x11d8e, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11d90, 0x11d91, GB_EXTEND, INCB_EXTEND, 0}, {0x11d93, 0x11d94, GB_SPACING_MARK, INCB_NONE, 0}, {0x11d95, 0x11d95, GB_EXTEND, INCB_EXTEND, 0},
  {0x11d96, 0x11d96, GB_SPACING_MARK, INCB_NONE, 0}, {0x11d97, 0x11d97, GB_EXTEND, INCB_EXTEND, 0}, {0x11ef3, 0x11ef4, GB_EXTEND, INCB_EXTEND, 0},
  {0x11ef5, 0x11ef6, GB_SPACING_MARK, INCB_NONE, 0}, {0x11f00, 0x11f01, GB_EXTEND, INCB_EXTEND, 0}, {0x11f02, 0x11f02, GB_PREPEND, INCB_NONE, 0},
  {0x11f03, 0x11f03, GB_SPACING_MARK, INCB_NONE, 0}, {0x11f04, 0x11f10, GB_OTHER, INCB_CONSONANT, 0}, {0x11f12, 0x11f33, GB_OTHER, INCB_CONSONANT, 0},
  {0x11f34, 0x11f35, GB_SPACING_MARK, INCB_NONE, 0}, {0x11f36, 0x11f3a, GB_EXTEND, INCB_EXTEND, 0}, {0x11f3e, 0x11f3f, GB_SPACING_MARK, INCB_NONE, 0},
  {0x11f40, 0x11f41, GB_EXTEND, INCB_EXTEND, 0}, {0x11f42, 0x11f42, GB_EXTEND, INCB_LINKER, 0}, {0x11f5a, 0x11f5a, GB_EXTEND, INCB_EXTEND, 0},
  {0x13430, 0x1343f, GB_CONTROL, INCB_NONE, 0}, {0x13440, 0x13440, GB_EXTEND, INCB_EXTEND, 0}, {0x13447, 0x13455, GB_EXTEND, INCB_EXTEND, 0},
  {0x1611e, 0x16129, GB_EXTEND, INCB_EXTEND, 0}, {0x1612a, 0x1612c, GB_SPACING_MARK, INCB_NONE, 0}, {0x1612d, 0x1612f, GB_EXTEND, INCB_EXTEND, 0},
  {0x16af0, 0x16af4, GB_EXTEND, INCB_EXTEND, 0}, {0x16b30, 0x16b36, GB_EXTEND, INCB_EXTEND, 0}, {0x16d63, 0x16d63, GB_V, INCB_NONE, 0},
  {0x16d67, 0x16d6a, GB_V, INCB_NONE, 0}, {0x16f4f, 0x16f4f, GB_EXTEND, INCB_EXTEND, 0}, {0x16f51, 0x16f87, GB_SPACING_MARK, INCB_NONE, 0},
  {0x16f8f, 0x16f92, GB_EXTEND, INCB_EXTEND, 0}, {0x16fe4, 0x16fe4, GB_EXTEND, INCB_EXTEND, 0}, {0x16ff0, 0x16ff1, GB_EXTEND, INCB_EXTEND, 0},
  {0x1bc9d, 0x1bc9e, GB_EXTEND, INCB_EXTEND, 0}, {0x1bca0, 0x1bca3, GB_CONTROL, INCB_NONE, 0}, {0x1cf00, 0x1cf2d, GB_EXTEND, INCB_EXTEND, 0},
  {0x1cf30, 0x1cf46, GB_EXTEND, INCB_EXTEND, 0}, {0x1d165, 0x1d169, GB_EXTEND, INCB_EXTEND, 0}, {0x1d16d, 0x1d172, GB_EXTEND, INCB_EXTEND, 0},
  {0x1d173, 0x1d17a, GB_CONTROL, INCB_NONE, 0}, {0x1d17b, 0x1d182, GB_EXTEND, INCB_EXTEND, 0}, {0x1d185, 0x1d18b, GB_EXTEND, INCB_EXTEND, 0},
  {0x1d1aa, 0x1d1ad, GB_EXTEND, INCB_EXTEND, 0}, {0x1d242, 0x1d244, GB_EXTEND, INCB_EXTEND, 0}, {0x1da00, 0x1da36, GB_EXTEND, INCB_EXTEND, 0},
  {0x1da3b, 0x1da6c, GB_EXTEND, INCB_EXTEND, 0}, {0x1da75, 0x1da75, GB_EXTEND, INCB_EXTEND, 0}, {0x1da84, 0x1da84, GB_EXTEND, INCB_EXTEND, 0},
  {0x1da9b, 0x1da9f, GB_EXTEND, INCB_EXTEND, 0}, {0x1daa1, 0x1daaf, GB_EXTEND, INCB_EXTEND, 0}, {0x1e000, 0x1e006, GB_EXTEND, INCB_EXTEND, 0},
  {0x1e008, 0x1e018, GB_EXTEND, INCB_EXTEND, 0}, {0x1e01b, 0x1e021, GB_EXTEND, INCB_EXTEND, 0}, {0x1e023, 0x1e024, GB_EXTEND, INCB_EXTEND, 0},
  {0x1e026, 0x1e02a, GB_EXTEND, INCB_EXTEND, 0}, {0x1e08f, 0x1e08f, GB_EXTEND, INCB_EXTEND, 0}, {0x1e130, 0x1e136, GB_EXTEND, INCB_EXTEND, 0},
  {0x1e2ae, 0x1e2ae, GB_EXTEND, INCB_EXTEND, 0}, {0x1e2ec, 0x1e2ef, GB_EXTEND, INCB_EXTEND, 0}, {0x1e4ec, 0x1e4ef, GB_EXTEND, INCB_EXTEND, 0},
  {0x1e5ee, 0x1e5ef, GB_EXTEND, INCB_EXTEND, 0}, {0x1e6e3, 0x1e6e3, GB_EXTEND, INCB_EXTEND, 0}, {0x1e6e6, 0x1e6e6, GB_EXTEND, INCB_EXTEND, 0},
  {0x1e6ee, 0x1e6ef, GB_EXTEND, INCB_EXTEND, 0}, {0x1e6f5, 0x1e6f5, GB_EXTEND, INCB_EXTEND, 0}, {0x1e8d0, 0x1e8d6, GB_EXTEND, INCB_EXTEND, 0},
  {0x1e944, 0x1e94a, GB_EXTEND, INCB_EXTEND, 0}, {0x1f004, 0x1f004, GB_OTHER, INCB_NONE, 1}, {0x1f02c, 0x1f02f, GB_OTHER, INCB_NONE, 1},
  {0x1f094, 0x1f09f, GB_OTHER, INCB_NONE, 1}, {0x1f0af, 0x1f0b0, GB_OTHER, INCB_NONE, 1}, {0x1f0c0, 0x1f0c0, GB_OTHER, INCB_NONE, 1},
  {0x1f0cf, 0x1f0d0, GB_OTHER, INCB_NONE, 1}, {0x1f0f6, 0x1f0ff, GB_OTHER, INCB_NONE, 1}, {0x1f170, 0x1f171, GB_OTHER, INCB_NONE, 1},
  {0x1f17e, 0x1f17f, GB_OTHER, INCB_NONE, 1}, {0x1f18e, 0x1f18e, GB_OTHER, INCB_NONE, 1}, {0x1f191, 0x1f19a, GB_OTHER, INCB_NONE, 1},
  {0x1f1ae, 0x1f1e5, GB_OTHER, INCB_NONE, 1}, {0x1f1e6, 0x1f1ff, GB_REGIONAL_INDICATOR, INCB_NONE, 0}, {0x1f201, 0x1f20f, GB_OTHER, INCB_NONE, 1},
  {0x1f21a, 0x1f21a, GB_OTHER, INCB_NONE, 1}, {0x1f22f, 0x1f22f, GB_OTHER, INCB_NONE, 1}, {0x1f232, 0x1f23a, GB_OTHER, INCB_NONE, 1},
  {0x1f23c, 0x1f23f, GB_OTHER, INCB_NONE, 1}, {0x1f249, 0x1f25f, GB_OTHER, INCB_NONE, 1}, {0x1f266, 0x1f321, GB_OTHER, INCB_NONE, 1},
  {0x1f324, 0x1f393, GB_OTHER, INCB_NONE, 1}, {0x1f396, 0x1f397, GB_OTHER, INCB_NONE, 1}, {0x1f399, 0x1f39b, GB_OTHER, INCB_NONE, 1},
  {0x1f39e, 0x1f3f0, GB_OTHER, INCB_NONE, 1}, {0x1f3f3, 0x1f3f5, GB_OTHER, INCB_NONE, 1}, {0x1f3f7, 0x1f3fa, GB_OTHER, INCB_NONE, 1},
  {0x1f3fb, 0x1f3ff, GB_EXTEND, INCB_EXTEND, 0}, {0x1f400, 0x1f4fd, GB_OTHER, INCB_NONE, 1}, {0x1f4ff, 0x1f53d, GB_OTHER, INCB_NONE, 1},
  {0x1f549, 0x1f54e, GB_OTHER, INCB_NONE, 1}, {0x1f550, 0x1f567, GB_OTHER, INCB_NONE, 1}, {0x1f56f, 0x1f570, GB_OTHER, INCB_NONE, 1},
  {0x1f573, 0x1f57a, GB_OTHER, INCB_NONE, 1}, {0x1f587, 0x1f587, GB_OTHER, INCB_NONE, 1}, {0x1f58a, 0x1f58d, GB_OTHER, INCB_NONE, 1},
  {0x1f590, 0x1f590, GB_OTHER, INCB_NONE, 1}, {0x1f595, 0x1f596, GB_OTHER, INCB_NONE, 1}, {0x1f5a4, 0x1f5a5, GB_OTHER, INCB_NONE, 1},
  {0x1f5a8, 0x1f5a8, GB_OTHER, INCB_NONE, 1}, {0x1f5b1, 0x1f5b2, GB_OTHER, INCB_NONE, 1}, {0x1f5bc, 0x1f5bc, GB_OTHER, INCB_NONE, 1},
  {0x1f5c2, 0x1f5c4, GB_OTHER, INCB_NONE, 1}, {0x1f5d1, 0x1f5d3, GB_OTHER, INCB_NONE, 1}, {0x1f5dc, 0x1f5de, GB_OTHER, INCB_NONE, 1},
  {0x1f5e1, 0x1f5e1, GB_OTHER, INCB_NONE, 1}, {0x1f5e3, 0x1f5e3, GB_OTHER, INCB_NONE, 1}, {0x1f5e8, 0x1f5e8, GB_OTHER, INCB_NONE, 1},
  {0x1f5ef, 0x1f5ef, GB_OTHER, INCB_NONE, 1}, {0x1f5f3, 0x1f5f3, GB_OTHER, INCB_NONE, 1}, {0x1f5fa, 0x1f64f, GB_OTHER, INCB_NONE, 1},
  {0x1f680, 0x1f6c5, GB_OTHER, INCB_NONE, 1}, {0x1f6cb, 0x1f6d2, GB_OTHER, INCB_NONE, 1}, {0x1f6d5, 0x1f6e5, GB_OTHER, INCB_NONE, 1},
  {0x1f6e9, 0x1f6e9, GB_OTHER, INCB_NONE, 1}, {0x1f6eb, 0x1f6f0, GB_OTHER, INCB_NONE, 1}, {0x1f6f3, 0x1f6ff, GB_OTHER, INCB_NONE, 1},
  {0x1f7da, 0x1f7ff, GB_OTHER, INCB_NONE, 1}, {0x1f80c, 0x1f80f, GB_OTHER, INCB_NONE, 1}, {0x1f848, 0x1f84f, GB_OTHER, INCB_NONE, 1},
  {0x1f85a, 0x1f85f, GB_OTHER, INCB_NONE, 1}, {0x1f888, 0x1f88f, GB_OTHER, INCB_NONE, 1}, {0x1f8ae, 0x1f8af, GB_OTHER, INCB_NONE, 1},
  {0x1f8bc, 0x1f8bf, GB_OTHER, INCB_NONE, 1}, {0x1f8c2, 0x1f8cf, GB_OTHER, INCB_NONE, 1}, {0x1f8d9, 0x1f8ff, GB_OTHER, INCB_NONE, 1},
  {0x1f90c, 0x1f93a, GB_OTHER, INCB_NONE, 1}, {0x1f93c, 0x1f945, GB_OTHER, INCB_NONE, 1}, {0x1f947, 0x1f9ff, GB_OTHER, INCB_NONE, 1},
  {0x1fa58, 0x1fa5f, GB_OTHER, INCB_NONE, 1}, {0x1fa6e, 0x1faff, GB_OTHER, INCB_NONE, 1}, {0x1fc00, 0x1fffd, GB_OTHER, INCB_NONE, 1},
  {0xe0000, 0xe001f, GB_CONTROL, INCB_NONE, 0}, {0xe0020, 0xe007f, GB_EXTEND, INCB_EXTEND, 0}, {0xe0080, 0xe00ff, GB_CONTROL, INCB_NONE, 0},
  {0xe0100, 0xe01ef, GB_EXTEND, INCB_EXTEND, 0}, {0xe01f0, 0xe0fff, GB_CONTROL, INCB_NONE, 0},
};
/* END GENERATED grapheme table */
static GraphemeRange grapheme_properties(uint32_t code) {
  size_t low = 0, high = COUNT_OF(grapheme_ranges);
  while (low < high) {
    size_t middle = low + (high - low) / 2;
    if (grapheme_ranges[middle].last < code) low = middle + 1; else high = middle;
  }
  if (low < COUNT_OF(grapheme_ranges) && grapheme_ranges[low].first <= code) return grapheme_ranges[low];
  return (GraphemeRange){code, code, GB_OTHER, INCB_NONE, 0};
}
/* How far a cluster has matched the GB9c sequence Consonant [Extend Linker]* Linker
   [Extend Linker]*. */
enum { CONJUNCT_RUN_NONE, CONJUNCT_RUN_CONSONANT, CONJUNCT_RUN_LINKED };
/* What the rules need to know about the cluster read so far. */
typedef struct {
  uint8_t previous, conjunct_run;
  bool pictographic_run;        /* ends in Extended_Pictographic Extend* (GB11) */
  bool pictographic_joiner;     /* ends in Extended_Pictographic Extend* ZWJ (GB11) */
  bool odd_regional_indicators; /* ends in an odd number of regional indicators (GB12, GB13) */
} GraphemeCluster;
/* The rules GB3 to GB999, in their order of precedence. */
static bool grapheme_breaks_before(const GraphemeCluster *cluster, GraphemeRange next) {
  uint8_t previous = cluster->previous, current = next.grapheme_break;
  if (previous == GB_CR && current == GB_LF) return false;
  if (previous == GB_CR || previous == GB_LF || previous == GB_CONTROL) return true;
  if (current == GB_CR || current == GB_LF || current == GB_CONTROL) return true;
  if (previous == GB_L && (current == GB_L || current == GB_V || current == GB_LV || current == GB_LVT)) return false;
  if ((previous == GB_LV || previous == GB_V) && (current == GB_V || current == GB_T)) return false;
  if ((previous == GB_LVT || previous == GB_T) && current == GB_T) return false;
  if (current == GB_EXTEND || current == GB_ZWJ || current == GB_SPACING_MARK || previous == GB_PREPEND) return false;
  if (cluster->conjunct_run == CONJUNCT_RUN_LINKED && next.conjunct == INCB_CONSONANT) return false;
  if (previous == GB_ZWJ && cluster->pictographic_joiner && next.pictographic) return false;
  if (previous == GB_REGIONAL_INDICATOR && current == GB_REGIONAL_INDICATOR) return !cluster->odd_regional_indicators;
  return true;
}
static void grapheme_extend(GraphemeCluster *cluster, GraphemeRange next) {
  cluster->pictographic_joiner = next.grapheme_break == GB_ZWJ && cluster->pictographic_run;
  cluster->pictographic_run = next.pictographic || (cluster->pictographic_run && next.grapheme_break == GB_EXTEND);
  cluster->odd_regional_indicators =
      next.grapheme_break == GB_REGIONAL_INDICATOR && !cluster->odd_regional_indicators;
  if (next.conjunct == INCB_CONSONANT) cluster->conjunct_run = CONJUNCT_RUN_CONSONANT;
  else if (cluster->conjunct_run == CONJUNCT_RUN_NONE) cluster->conjunct_run = CONJUNCT_RUN_NONE;
  else if (next.conjunct == INCB_LINKER) cluster->conjunct_run = CONJUNCT_RUN_LINKED;
  else if (next.conjunct != INCB_EXTEND) cluster->conjunct_run = CONJUNCT_RUN_NONE;
  cluster->previous = next.grapheme_break;
}
/* Returns the byte offset where the cluster that starts at the character boundary
   `start` ends, or `length` when `start` is the end. */
static size_t grapheme_end(const char *text, size_t length, size_t start) {
  if (start == length) return start;
  size_t width;
  GraphemeRange first = grapheme_properties(utf8_decode((const unsigned char *)text + start, &width));
  GraphemeCluster cluster = {
    first.grapheme_break,
    first.conjunct == INCB_CONSONANT ? CONJUNCT_RUN_CONSONANT : CONJUNCT_RUN_NONE,
    first.pictographic, false, first.grapheme_break == GB_REGIONAL_INDICATOR,
  };
  for (size_t offset = start + width; offset < length; offset += width) {
    GraphemeRange next = grapheme_properties(utf8_decode((const unsigned char *)text + offset, &width));
    if (grapheme_breaks_before(&cluster, next)) return offset;
    grapheme_extend(&cluster, next);
  }
  return length;
}

/* Debug.crash, Option.unwrap_or_fail, and the native cores of std/text and std/list.
   Defect messages match the interpreter's (execution/core_operations.rs). */
static ChimiControl *invoke_core_operation(uint32_t intrinsic, ChimiValue **arguments,
                                           uint32_t argument_count, uint32_t call_node) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_DEBUG_CRASH:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      return defect(lasting_message(arguments[0]->as.text.bytes), call_node);
    case CHIMI_INTRINSIC_OPTION_UNWRAP_OR_FAIL:
      if (argument_count != 2 || arguments[0]->kind != VALUE_CONSTRUCTED) break;
      if (arguments[0]->as.constructed.length == 1) return success(arguments[0]->as.constructed.fields[0].value);
      return failure(arguments[1]);
    case CHIMI_INTRINSIC_TEXT_CONCAT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST) break;
      TextBuffer buffer = {0};
      for (uint32_t i = 0; i < list_length(arguments[0]); ++i) {
        ChimiValue *part = list_get(arguments[0], i);
        if (part->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
        buffer_bytes(&buffer, part->as.text.bytes, part->as.text.length);
      }
      return success(buffer.data ? text_from_bytes(buffer.data, buffer.length) : text_value(""));
    }
    case CHIMI_INTRINSIC_TEXT_BYTE_LENGTH:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      return success(integer_value((int64_t)arguments[0]->as.text.length));
    case CHIMI_INTRINSIC_TEXT_BYTE_SLICE: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT || arguments[1]->kind != VALUE_RANGE) break;
      const char *text = arguments[0]->as.text.bytes; size_t length = arguments[0]->as.text.length, from, to;
      clamp_range(arguments[1]->as.range.start, arguments[1]->as.range.end, length, &from, &to);
      if (!utf8_boundary(text, length, from))
        return defect(numeric_message("Text.byte_slice: bound %zu falls inside a UTF-8 sequence", from), call_node);
      if (!utf8_boundary(text, length, to))
        return defect(numeric_message("Text.byte_slice: bound %zu falls inside a UTF-8 sequence", to), call_node);
      return success(text_from_bytes(text + from, to - from));
    }
    case CHIMI_INTRINSIC_TEXT_CONTAINS:
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT || arguments[1]->kind != VALUE_TEXT) break;
      if (!arguments[1]->as.text.length) return defect("Text.contains: needle must not be empty", call_node);
      return success(boolean_value(text_find(arguments[0]->as.text.bytes, arguments[0]->as.text.length,
                                             arguments[1]->as.text.bytes, arguments[1]->as.text.length) != NULL));
    case CHIMI_INTRINSIC_TEXT_STARTS_WITH: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT || arguments[1]->kind != VALUE_TEXT) break;
      size_t prefix = arguments[1]->as.text.length;
      return success(boolean_value(prefix <= arguments[0]->as.text.length &&
                                   memcmp(arguments[0]->as.text.bytes, arguments[1]->as.text.bytes, prefix) == 0));
    }
    case CHIMI_INTRINSIC_TEXT_ENDS_WITH: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT || arguments[1]->kind != VALUE_TEXT) break;
      size_t length = arguments[0]->as.text.length, suffix = arguments[1]->as.text.length;
      return success(boolean_value(suffix <= length &&
                                   memcmp(arguments[0]->as.text.bytes + length - suffix, arguments[1]->as.text.bytes, suffix) == 0));
    }
    case CHIMI_INTRINSIC_TEXT_SPLIT: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT || arguments[1]->kind != VALUE_TEXT) break;
      const char *text = arguments[0]->as.text.bytes, *separator = arguments[1]->as.text.bytes;
      const char *end = text + arguments[0]->as.text.length;
      size_t width = arguments[1]->as.text.length;
      if (!width) return defect("Text.split: separator must not be empty", call_node);
      ChimiValue *parts = list_value((ListVector){NULL, NULL, 0}, 0, 0);
      for (const char *found; (found = text_find(text, (size_t)(end - text), separator, width)); text = found + width)
        parts = list_push(parts, text_from_bytes(text, (size_t)(found - text)));
      return success(list_push(parts, text_from_bytes(text, (size_t)(end - text))));
    }
    case CHIMI_INTRINSIC_TEXT_REPLACE: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_TEXT || arguments[1]->kind != VALUE_TEXT ||
          arguments[2]->kind != VALUE_TEXT) break;
      const char *text = arguments[0]->as.text.bytes, *old = arguments[1]->as.text.bytes;
      const char *new_text = arguments[2]->as.text.bytes, *end = text + arguments[0]->as.text.length;
      size_t width = arguments[1]->as.text.length, replacement = arguments[2]->as.text.length;
      if (!width) return defect("Text.replace: pattern must not be empty", call_node);
      TextBuffer buffer = {0};
      for (const char *found; (found = text_find(text, (size_t)(end - text), old, width)); text = found + width) {
        buffer_bytes(&buffer, text, (size_t)(found - text));
        buffer_bytes(&buffer, new_text, replacement);
      }
      buffer_bytes(&buffer, text, (size_t)(end - text));
      return success(text_from_bytes(buffer.data ? buffer.data : "", buffer.length));
    }
    case CHIMI_INTRINSIC_TEXT_TRIM_START: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      const unsigned char *text = (const unsigned char *)arguments[0]->as.text.bytes;
      const unsigned char *end = text + arguments[0]->as.text.length;
      size_t width;
      while (text < end && is_white_space(utf8_decode(text, &width))) text += width;
      return success(text_from_bytes((const char *)text, (size_t)(end - text)));
    }
    case CHIMI_INTRINSIC_TEXT_TRIM_END: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      const char *text = arguments[0]->as.text.bytes;
      size_t end = arguments[0]->as.text.length, width;
      while (end > 0) {
        size_t start = end - 1;
        while (start > 0 && !utf8_boundary(text, end, start)) --start;
        if (!is_white_space(utf8_decode((const unsigned char *)text + start, &width))) break;
        end = start;
      }
      return success(text_from_bytes(text, end));
    }
    case CHIMI_INTRINSIC_TEXT_LOWERCASE:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      return success(text_case_map(arguments[0], lowercase_mappings,
                                   COUNT_OF(lowercase_mappings), true));
    case CHIMI_INTRINSIC_TEXT_UPPERCASE:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      return success(text_case_map(arguments[0], uppercase_mappings,
                                   COUNT_OF(uppercase_mappings), false));
    case CHIMI_INTRINSIC_TEXT_GRAPHEMES: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      const char *text = arguments[0]->as.text.bytes;
      size_t length = arguments[0]->as.text.length;
      ChimiValue *graphemes = list_value((ListVector){NULL, NULL, 0}, 0, 0);
      for (size_t start = 0, end; start < length; start = end) {
        end = grapheme_end(text, length, start);
        graphemes = list_push(graphemes, text_from_bytes(text + start, end - start));
      }
      return success(graphemes);
    }
    case CHIMI_INTRINSIC_LIST_GET: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_LIST || arguments[1]->kind != VALUE_INTEGER) break;
      int64_t index = arguments[1]->as.integer;
      if (index < 0 || (uint64_t)index >= list_length(arguments[0]))
        return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
      ChimiValue *fields[] = {list_get(arguments[0], (uint32_t)index)};
      return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
    }
    case CHIMI_INTRINSIC_LIST_PUSH:
      if (argument_count != 2 || arguments[0]->kind != VALUE_LIST) break;
      return success(list_push(arguments[0], arguments[1]));
    case CHIMI_INTRINSIC_LIST_SLICE: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_LIST || arguments[1]->kind != VALUE_RANGE) break;
      size_t from, to;
      clamp_range(arguments[1]->as.range.start, arguments[1]->as.range.end, list_length(arguments[0]), &from, &to);
      return success(list_slice(arguments[0], (uint32_t)from, (uint32_t)to));
    }
    case CHIMI_INTRINSIC_LIST_SET: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_LIST || arguments[1]->kind != VALUE_INTEGER) break;
      ChimiValue *list = arguments[0];
      int64_t index = arguments[1]->as.integer;
      if (index < 0 || (uint64_t)index >= list_length(list))
        return defect(numeric_message("List.set: index must be in 0..%" PRIu32 ", got %" PRId64,
                                      list_length(list), index), call_node);
      ListVector vector = vector_set(list->as.list.vector, list->as.list.start + (uint32_t)index, arguments[2]);
      return success(list_value(vector, list->as.list.start, list_length(list)));
    }
  }
  return intrinsic_shape_defect(call_node);
}

/* std/bytes. A `Bytes` shares Text's representation (`as.text`) with its own kind,
   so the collector, heap copies, and transfers treat its buffer like a Text's. */
static ChimiValue *bytes_from(const char *raw, size_t length) {
  ChimiValue *value = text_from_bytes(raw, length);
  value->kind = VALUE_BYTES;
  return value;
}
/* The bytes a constant's hex text spells, two lowercase digits per byte. */
static unsigned hex_value(char digit) { return digit <= '9' ? (unsigned)(digit - '0') : (unsigned)(digit - 'a' + 10); }
static ChimiValue *bytes_from_hex(const char *hex, size_t length) {
  char *raw = allocate(length / 2 + 1, 1);
  for (size_t i = 0; i < length / 2; ++i) raw[i] = (char)(hex_value(hex[2 * i]) << 4 | hex_value(hex[2 * i + 1]));
  raw[length / 2] = '\0';
  ChimiValue *value = new_value(VALUE_BYTES); value->as.text.bytes = raw; value->as.text.length = length / 2;
  return value;
}

/* The UTF-8 sequence at `text`: how many bytes it spans, and in `complete`
   whether they form a whole scalar. An incomplete result is the maximal
   subpart Unicode recommends replacing with one U+FFFD (at least one byte),
   which is what Rust's `from_utf8_lossy` replaces. Ranges follow Unicode
   Table 3-7, so overlong forms, surrogates, and values past U+10FFFF fail. */
static size_t utf8_sequence(const unsigned char *text, size_t remaining, bool *complete) {
  unsigned char first = text[0];
  size_t width; unsigned char low = 0x80, high = 0xbf;
  if (first <= 0x7f) { *complete = true; return 1; }
  if (first >= 0xc2 && first <= 0xdf) width = 2;
  else if (first >= 0xe0 && first <= 0xef) {
    width = 3;
    if (first == 0xe0) low = 0xa0;
    if (first == 0xed) high = 0x9f;
  } else if (first >= 0xf0 && first <= 0xf4) {
    width = 4;
    if (first == 0xf0) low = 0x90;
    if (first == 0xf4) high = 0x8f;
  } else { *complete = false; return 1; }
  size_t taken = 1;
  for (; taken < width; ++taken) {
    if (taken >= remaining || text[taken] < low || text[taken] > high) { *complete = false; return taken; }
    low = 0x80; high = 0xbf;
  }
  *complete = true;
  return width;
}

/* `layout_code` in std/bytes: U8, I8, then U16Be, U16Le, I16Be, I16Le and the
   same four for 32 bits, then I64Be, I64Le. False for any other code. */
static bool int_layout(int64_t code, size_t *width, bool *is_signed, bool *big_endian) {
  if (code < 0 || code > 11) return false;
  if (code <= 1) { *width = 1; *is_signed = code == 1; *big_endian = true; return true; }
  if (code >= 10) { *width = 8; *is_signed = true; *big_endian = code == 10; return true; }
  int64_t within = code - 2;
  *width = within < 4 ? 2 : 4; *is_signed = within % 4 >= 2; *big_endian = within % 2 == 0;
  return true;
}

static ChimiControl *optional_value(ChimiValue *value) {
  if (!value) return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
  ChimiValue *fields[] = {value};
  return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
}

/* The native core of std/bytes. Defect messages match the interpreter's
   (execution/bytes_operations.rs). */
static ChimiControl *invoke_bytes_operation(uint32_t intrinsic, ChimiValue **arguments,
                                            uint32_t argument_count, uint32_t call_node) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_BYTES_FROM: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST) break;
      uint32_t length = list_length(arguments[0]);
      char *raw = allocate((size_t)length + 1, 1);
      for (uint32_t i = 0; i < length; ++i) {
        ChimiValue *element = list_get(arguments[0], i);
        if (element->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
        if (element->as.integer < 0 || element->as.integer > 255)
          return defect(numeric_message("Bytes.from: element %" PRIu32 " is %" PRId64 ", outside 0..=255",
                                        i, element->as.integer), call_node);
        raw[i] = (char)element->as.integer;
      }
      raw[length] = '\0';
      ChimiValue *value = new_value(VALUE_BYTES); value->as.text.bytes = raw; value->as.text.length = length;
      return success(value);
    }
    case CHIMI_INTRINSIC_BYTES_LENGTH:
      if (argument_count != 1 || arguments[0]->kind != VALUE_BYTES) break;
      return success(integer_value((int64_t)arguments[0]->as.text.length));
    case CHIMI_INTRINSIC_BYTES_GET: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_BYTES || arguments[1]->kind != VALUE_INTEGER) break;
      int64_t index = arguments[1]->as.integer;
      if (index < 0 || (uint64_t)index >= arguments[0]->as.text.length) return optional_value(NULL);
      return optional_value(integer_value((unsigned char)arguments[0]->as.text.bytes[index]));
    }
    case CHIMI_INTRINSIC_BYTES_INDEX_OF: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_BYTES || arguments[1]->kind != VALUE_BYTES) break;
      if (!arguments[1]->as.text.length) return defect("Bytes.index_of: needle must not be empty", call_node);
      const char *found = text_find(arguments[0]->as.text.bytes, arguments[0]->as.text.length,
                                    arguments[1]->as.text.bytes, arguments[1]->as.text.length);
      return optional_value(found ? integer_value((int64_t)(found - arguments[0]->as.text.bytes)) : NULL);
    }
    case CHIMI_INTRINSIC_BYTES_CONCAT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST) break;
      TextBuffer buffer = {0};
      for (uint32_t i = 0; i < list_length(arguments[0]); ++i) {
        ChimiValue *part = list_get(arguments[0], i);
        if (part->kind != VALUE_BYTES) return intrinsic_shape_defect(call_node);
        buffer_bytes(&buffer, part->as.text.bytes, part->as.text.length);
      }
      return success(bytes_from(buffer.data ? buffer.data : "", buffer.length));
    }
    case CHIMI_INTRINSIC_BYTES_SLICE: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_BYTES || arguments[1]->kind != VALUE_RANGE) break;
      size_t from, to;
      clamp_range(arguments[1]->as.range.start, arguments[1]->as.range.end, arguments[0]->as.text.length, &from, &to);
      return success(bytes_from(arguments[0]->as.text.bytes + from, to - from));
    }
    case CHIMI_INTRINSIC_BYTES_TO_LIST: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_BYTES) break;
      size_t length = arguments[0]->as.text.length;
      ChimiValue **items = allocate(length ? length : 1, sizeof(*items));
      for (size_t i = 0; i < length; ++i) items[i] = integer_value((unsigned char)arguments[0]->as.text.bytes[i]);
      return success(list_from_values(items, (uint32_t)length));
    }
    case CHIMI_INTRINSIC_BYTES_FROM_TEXT:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      return success(bytes_from(arguments[0]->as.text.bytes, arguments[0]->as.text.length));
    case CHIMI_INTRINSIC_BYTES_TO_TEXT_LOSSY: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_BYTES) break;
      const unsigned char *bytes = (const unsigned char *)arguments[0]->as.text.bytes;
      size_t length = arguments[0]->as.text.length;
      TextBuffer buffer = {0};
      for (size_t at = 0; at < length;) {
        bool complete;
        size_t width = utf8_sequence(bytes + at, length - at, &complete);
        if (complete) buffer_bytes(&buffer, (const char *)bytes + at, width);
        else buffer_bytes(&buffer, "\xef\xbf\xbd", 3);
        at += width;
      }
      return success(text_from_bytes(buffer.data ? buffer.data : "", buffer.length));
    }
    case CHIMI_INTRINSIC_BYTES_VALID_UTF8_LENGTH: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_BYTES) break;
      const unsigned char *bytes = (const unsigned char *)arguments[0]->as.text.bytes;
      size_t length = arguments[0]->as.text.length, at = 0;
      while (at < length) {
        bool complete;
        size_t width = utf8_sequence(bytes + at, length - at, &complete);
        if (!complete) break;
        at += width;
      }
      return success(integer_value((int64_t)at));
    }
    case CHIMI_INTRINSIC_BYTES_READ_INT_CODE: {
      size_t width; bool is_signed, big_endian;
      if (argument_count != 3 || arguments[0]->kind != VALUE_BYTES || arguments[1]->kind != VALUE_INTEGER ||
          arguments[2]->kind != VALUE_INTEGER || !int_layout(arguments[2]->as.integer, &width, &is_signed, &big_endian))
        break;
      int64_t at = arguments[1]->as.integer;
      size_t length = arguments[0]->as.text.length;
      if (at < 0 || (uint64_t)at > length || length - (size_t)at < width) return optional_value(NULL);
      const unsigned char *field = (const unsigned char *)arguments[0]->as.text.bytes + at;
      uint64_t raw = 0;
      for (size_t i = 0; i < width; ++i) raw = raw << 8 | field[big_endian ? i : width - 1 - i];
      int64_t value = (int64_t)raw;
      /* Sign-extend from the layout's width; 64 bits is already two's complement. */
      if (is_signed && width < 8 && (raw >> (8 * width - 1)) & 1) value = (int64_t)(raw | ~(uint64_t)0 << (8 * width));
      return optional_value(integer_value(value));
    }
    case CHIMI_INTRINSIC_BYTES_INT_BYTES: {
      size_t width; bool is_signed, big_endian;
      if (argument_count != 2 || arguments[0]->kind != VALUE_INTEGER || arguments[1]->kind != VALUE_INTEGER ||
          !int_layout(arguments[1]->as.integer, &width, &is_signed, &big_endian))
        break;
      int64_t value = arguments[0]->as.integer;
      if (width < 8) {
        int64_t bits = (int64_t)(8 * width);
        int64_t low = is_signed ? -((int64_t)1 << (bits - 1)) : 0;
        int64_t high = is_signed ? ((int64_t)1 << (bits - 1)) - 1 : ((int64_t)1 << bits) - 1;
        if (value < low || value > high) return optional_value(NULL);
      }
      char raw[8];
      for (size_t i = 0; i < width; ++i) {
        char byte = (char)(((uint64_t)value >> (8 * (width - 1 - i))) & 0xff);
        raw[big_endian ? i : width - 1 - i] = byte;
      }
      return optional_value(bytes_from(raw, width));
    }
    /* CHIMI_INTRINSIC_EMBED_TEXT and _BYTES never run here: lowering admits them
       only in module constants, which the build evaluates before emitting program
       data, so a native program receives their values as constant rows. */
  }
  return intrinsic_shape_defect(call_node);
}

/* std/test. A comparison's message is "<heading>\nactual:\n<render>\nexpected:\n<render>",
   byte for byte as `comparison_message` in execution/core_operations.rs, because
   `chimi test` reads the two renderings back from it. */
static ChimiValue *debug_render(const ChimiValue *value, bool indented);

static ChimiControl *invoke_assertion(uint32_t intrinsic, ChimiValue **arguments,
                                      uint32_t argument_count, uint32_t call_node) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_TEST_ASSERT_EQUAL:
    case CHIMI_INTRINSIC_TEST_ASSERT_NOT_EQUAL: {
      if (argument_count != 2) break;
      bool wanted = intrinsic == CHIMI_INTRINSIC_TEST_ASSERT_EQUAL;
      if ((chimi_value_compare(arguments[0], arguments[1]) == 0) == wanted)
        return success(void_value());
      TextBuffer buffer = {0};
      const char *heading = wanted ? "Test.assert_equal: the values differ"
                                    : "Test.assert_not_equal: the values are equal";
      buffer_bytes(&buffer, heading, strlen(heading));
      buffer_bytes(&buffer, "\nactual:\n", 9);
      ChimiValue *actual = debug_render(arguments[0], true);
      buffer_bytes(&buffer, actual->as.text.bytes, actual->as.text.length);
      buffer_bytes(&buffer, "\nexpected:\n", 11);
      ChimiValue *expected = debug_render(arguments[1], true);
      buffer_bytes(&buffer, expected->as.text.bytes, expected->as.text.length);
      buffer_char(&buffer, '\0');
      return defect(buffer.data, call_node);
    }
    case CHIMI_INTRINSIC_TEST_ASSERT:
      if (argument_count != 2 || arguments[0]->kind != VALUE_BOOLEAN || arguments[1]->kind != VALUE_TEXT) break;
      if (arguments[0]->as.boolean) return success(void_value());
      return defect(copy_text(arguments[1]->as.text.bytes), call_node);
    case CHIMI_INTRINSIC_TEST_FAIL:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      return defect(copy_text(arguments[0]->as.text.bytes), call_node);
  }
  return intrinsic_shape_defect(call_node);
}

static bool is_assertion_call(int64_t node) {
  uint32_t low = 0, high = CHIMI_ASSERTION_NODE_COUNT;
  while (low < high) {
    uint32_t middle = low + (high - low) / 2;
    if ((int64_t)CHIMI_ASSERTION_NODES[middle] < node) low = middle + 1;
    else high = middle;
  }
  return low < CHIMI_ASSERTION_NODE_COUNT && (int64_t)CHIMI_ASSERTION_NODES[low] == node;
}

static ChimiControl *capture_outcome_defect(const char *heading, const ChimiValue *value,
                                            uint32_t call_node) {
  TextBuffer buffer = {0};
  buffer_bytes(&buffer, heading, strlen(heading));
  buffer_char(&buffer, '\n');
  ChimiValue *rendering = debug_render(value, true);
  buffer_bytes(&buffer, rendering->as.text.bytes, rendering->as.text.length);
  buffer_char(&buffer, '\0');
  return defect(buffer.data, call_node);
}

/* Test.capture_defect, as invoke_capture_defect in execution/core_operations.rs.
   Under the scheduler the operation runs in a scope without a deadline; after a
   captured defect, the scope's children are cancelled and leaving it drains
   them. A cancellation from outside passes through, as through Task.timeout. */
static ChimiControl *capture_defect(ChimiRuntime *runtime, ChimiValue **arguments,
                                    uint32_t argument_count, uint64_t span, uint32_t call_node,
                                    ChimiEnvironment *environment) {
  if (argument_count != 1 || arguments[0]->kind != VALUE_CALLABLE)
    return intrinsic_shape_defect(call_node);
  ChimiSchedulerRequest enter = {.kind = CHIMI_REQUEST_SCOPE_ENTER, .value = INT64_MAX};
  if (CHIMI_REQUIRES_SCHEDULER) {
    if (scheduler_request(&enter) == CHIMI_FIBER_CANCEL) return cancelled();
    if (enter.status != CHIMI_RESPONSE_OK)
      return defect("Test.capture_defect scheduler request failed", call_node);
  }
  ChimiControl *control = invoke(runtime, arguments[0]->as.callable, NULL, 0, span, call_node, environment);
  if (control->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal()) return control;
  bool captured = control->kind == CONTROL_DEFECT && !is_assertion_call(control->node);
  if (CHIMI_REQUIRES_SCHEDULER) {
    if (captured) {
      ChimiSchedulerRequest cancel = {.kind = CHIMI_REQUEST_SCOPE_CANCEL, .token = enter.token};
      if (scheduler_request(&cancel) == CHIMI_FIBER_CANCEL) return cancelled();
      if (cancel.status != CHIMI_RESPONSE_OK)
        return defect("Test.capture_defect scheduler request failed", call_node);
    }
    for (;;) {
      /* A captured defect discards the operation's values, so the capabilities
         it gained in the scope are aborted, as after a cancellation. */
      ChimiSchedulerRequest exit = {.kind = CHIMI_REQUEST_SCOPE_EXIT, .token = enter.token,
                                    .value = captured};
      if (scheduler_request(&exit) == CHIMI_FIBER_CANCEL) return cancelled();
      if (exit.status == CHIMI_RESPONSE_PENDING) continue;
      if (exit.status != CHIMI_RESPONSE_OK)
        return defect("Test.capture_defect scheduler request failed", call_node);
      break;
    }
  }
  if (captured) return success(text_value(control->message));
  if (control->kind == CONTROL_SUCCESS)
    return capture_outcome_defect("Test.capture_defect: the operation returned instead of defecting:",
                                  control->value, call_node);
  if (control->kind == CONTROL_FAILURE)
    return capture_outcome_defect("Test.capture_defect: the operation failed with an expected "
                                  "failure instead of defecting:", control->value, call_node);
  return control;
}

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
  HeapStatus status = allocation_scope_end(scope, NULL, &control);
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
  const char *path = arguments[0]->as.text.bytes;
  /* A C path stops at U+0000, so such a path names no file; the message is the one
     the interpreter's file system reports. */
  if (text_holds_nul(arguments[0]))
    return failure(standard_value(CHIMI_CONSTRUCTOR_FILE_OPEN, (ChimiValue *[]){
      arguments[0], text_value("file name contained an unexpected NUL byte")}, 2));
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
      allocation_scope_collect(&allocation_scope, NULL, &control);
      break;
    }
    errno = 0;
    int byte = fgetc(input);
    if (byte == EOF) {
      if (ferror(input)) {
        control = runtime->cancel && errno == EINTR
          ? cancelled()
          : host_file_message(CHIMI_CONSTRUCTOR_FILE_READ, path, strerror(errno));
        allocation_scope_collect(&allocation_scope, NULL, &control);
      } else if (length > 0) {
        control = invoke_line_callback(runtime, arguments[2]->as.callable, state, line, length,
                                       line_number, path, span, call_node, environment);
        allocation_scope_collect(&allocation_scope, NULL, &control);
      } else {
        control = success(state);
        allocation_scope_collect(&allocation_scope, NULL, &control);
      }
      break;
    }
    if (byte == '\n') {
      size_t content_length = length > 0 && line[length - 1] == '\r' ? length - 1 : length;
      control = invoke_line_callback(runtime, arguments[2]->as.callable, state, line,
                                     content_length, line_number, path, span, call_node,
                                     environment);
      if (control->kind != CONTROL_SUCCESS) {
        allocation_scope_collect(&allocation_scope, NULL, &control);
        break;
      }
      state = control->value;
      allocation_scope_collect(&allocation_scope, &state, NULL);
      length = 0;
      ++line_number;
      continue;
    }
    if (length == runtime->max_line_bytes) {
      ChimiValue *fields[] = {text_value(path), integer_value(runtime->max_line_bytes)};
      control = failure(standard_value(CHIMI_CONSTRUCTOR_FILE_LINE_TOO_LONG, fields, 2));
      allocation_scope_collect(&allocation_scope, NULL, &control);
      break;
    }
    line[length++] = (unsigned char)byte;
  }
  if (owns_file && fclose(input) != 0 &&
      (control->kind == CONTROL_SUCCESS || control->kind == CONTROL_FAILURE)) {
    control = host_file_message(CHIMI_CONSTRUCTOR_FILE_CLOSE, path, strerror(errno));
    allocation_scope_collect(&allocation_scope, NULL, &control);
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
  size_t length = arguments[0]->as.text.length;
  if (fwrite(arguments[0]->as.text.bytes, 1, length, output) != length || fputc('\n', output) == EOF ||
      fflush(output) != 0) {
    const char *stream = function == HOST_WRITE_LINE ? "stdout" : "stderr";
    ChimiValue *fields[] = {text_value(stream), text_value(strerror(errno))};
    return failure(standard_value(CHIMI_CONSTRUCTOR_CONSOLE_WRITE_FAILED, fields, 2));
  }
  return runtime->cancel ? cancelled() : success(void_value());
}

static ChimiControl *construct_value(uint32_t symbol, ChimiValue **values);
static ChimiControl *invoke_clock_host(ChimiRuntime *runtime, ChimiCallable *callable,
                                       ChimiValue **arguments, uint32_t argument_count,
                                       uint64_t span, uint32_t call_node,
                                       ChimiEnvironment *environment);

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
  if (callable->kind == CALL_HOST && callable->host >= HOST_CLOCK_NOW)
    return invoke_clock_host(runtime, callable, arguments, argument_count, span, call_node,
                             call_environment);
  if (callable->kind == CALL_HOST)
    return invoke_host(runtime, callable->host, arguments, argument_count,
                       span, call_node, call_environment);
  if (host_stack_exhausted()) return defect("Stack overflow", -1);
  ChimiEnvironment *environment;
  if (callable->kind == CALL_DECLARED) {
    // Declared functions capture provisioned requirements, but never local
    // bindings. Build a clean local table for parameters while retaining the
    // requirement snapshot taken when the function became a value.
    environment = new_environment(callable->local_count);
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
  uint64_t parent = context->parent;
  void *captures[FRAGMENT_ROOTS];
  fragment_adopt(context->captures, captures);
  free(context);
  ChimiValue *callable = captures[0];
  ChimiControl *control = invoke(runtime, callable->as.callable, NULL, 0, span, UINT32_MAX,
                                 captures[1]);
  /* Only the parent can read the result (a task handle is parent-local), so the
     result's exclusive capabilities become the parent's before this task ends
     and its cleanup would abort them. */
  if ((control->kind == CONTROL_SUCCESS || control->kind == CONTROL_FAILURE) && control->value) {
    CapabilityTransfer transfer = {.child = parent};
    if (!transfer_captures(&transfer, control->value))
      control = transfer.cancelled
        ? cancelled()
        : defect("Task could not hand an exclusive capability in its result to its parent", -1);
  }
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

static int64_t copy_channel_word(void *context, int64_t word) {
  (void)context;
  return (int64_t)(intptr_t)fragment_duplicate((RuntimeFragment *)(intptr_t)word);
}

/* A child the scheduler ends before its entry runs never adopts its captures. */
static void release_task_context(void *context, void *raw) {
  (void)context;
  ChimiTaskContext *task = raw;
  fragment_free(task->captures);
  free(task);
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
    int64_t value = 0; uint32_t error = text_holds_nul(arguments[0]) ? CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID
                                                                     : duration_parse(arguments[0]->as.text.bytes, &value);
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

static void debug_quote(TextBuffer *buffer, const ChimiValue *text) {
  buffer_char(buffer, '"');
  const unsigned char *cursor = (const unsigned char *)text->as.text.bytes;
  for (const unsigned char *end = cursor + text->as.text.length; cursor < end; ++cursor) {
    switch (*cursor) {
      case '"': debug_text(buffer, "\\\""); break;
      case '\\': debug_text(buffer, "\\\\"); break;
      case '\n': debug_text(buffer, "\\n"); break;
      case '\r': debug_text(buffer, "\\r"); break;
      case '\t': debug_text(buffer, "\\t"); break;
      case '$':
        if (cursor + 1 < end && cursor[1] == '{') debug_text(buffer, "\\$"); else buffer_char(buffer, '$');
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

/* json.rs `canonical_number_text`: the exact value, so equal numbers render alike.
   Positional when the decimal exponent of the first significant digit is from -6
   to 20, otherwise d.ddde<exponent>, as in 1e21 and 1e400. */
static void debug_json_number(TextBuffer *buffer, const char *text) {
  CanonicalNumber number = canonical_number(text);
  if (number.sign == 0) {
    debug_text(buffer, "0");
  } else {
    if (number.sign < 0) buffer_char(buffer, '-');
    /* 0.digits x 10^exponent: `exponent` digits come before the point. */
    DecimalExponent low = decimal_exponent_of(-5), high = decimal_exponent_of(21);
    bool positional = decimal_exponent_order(&number.exponent, &low) >= 0 &&
                      decimal_exponent_order(&number.exponent, &high) <= 0;
    free(low.digits); free(high.digits);
    int64_t length = (int64_t)number.length;
    if (positional) {
      int64_t before = 0;
      for (size_t index = 0; index < number.exponent.length; ++index) before = before * 10 + number.exponent.digits[index];
      if (number.exponent.negative) before = -before;
      if (before >= length) {
        buffer_bytes(buffer, number.digits, number.length);
        for (int64_t zero = length; zero < before; ++zero) buffer_char(buffer, '0');
      } else if (before > 0) {
        buffer_bytes(buffer, number.digits, (size_t)before); buffer_char(buffer, '.');
        buffer_bytes(buffer, number.digits + before, (size_t)(length - before));
      } else {
        buffer_bytes(buffer, "0.", 2);
        for (int64_t zero = 0; zero < -before; ++zero) buffer_char(buffer, '0');
        buffer_bytes(buffer, number.digits, number.length);
      }
    } else {
      buffer_char(buffer, number.digits[0]);
      if (length > 1) { buffer_char(buffer, '.'); buffer_bytes(buffer, number.digits + 1, number.length - 1); }
      DecimalExponent one_less = decimal_exponent_of(-1);
      DecimalExponent scientific = decimal_exponent_sum(&number.exponent, &one_less);
      buffer_char(buffer, 'e');
      if (scientific.negative) buffer_char(buffer, '-');
      for (size_t index = 0; index < scientific.length; ++index) buffer_char(buffer, (char)('0' + scientific.digits[index]));
      free(one_less.digits); free(scientific.digits);
    }
  }
  free(number.owned); free(number.exponent.digits);
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
    case VALUE_TEXT: debug_quote(buffer, value); return;
    /* Two hex digits per byte, as a hex dump shows them: `Bytes(68 69)`. */
    case VALUE_BYTES:
      debug_text(buffer, "Bytes(");
      for (size_t i = 0; i < value->as.text.length; ++i) {
        unsigned char byte = (unsigned char)value->as.text.bytes[i];
        char digits[4] = {' ', hex_digit(byte >> 4), hex_digit(byte), '\0'};
        debug_text(buffer, i ? digits : digits + 1);
      }
      debug_text(buffer, ")"); return;
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
    /* An Instant has no text form, so debug shows its offset from the origin. */
    case VALUE_INSTANT: {
      char offset[48]; duration_text(value->as.integer, offset);
      snprintf(scalar, sizeof(scalar), "Instant(%s)", offset); debug_text(buffer, scalar); return;
    }
    case VALUE_TIMESTAMP: timestamp_text(value->as.integer, scalar); debug_text(buffer, scalar); return;
    /* Not the `Decimal.to_text` form, which keeps the scale: equal values render the same. */
    case VALUE_DECIMAL: {
      char text[DECIMAL_TEXT_BYTES]; decimal_normalized_text(decimal_of(value), text); debug_text(buffer, text); return;
    }
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
    case VALUE_SET: {
      MapEntries entries = map_entries(value->as.map);
      if (!debug_open(renderer, steps, "Set {", "}", true, (uint32_t)entries.length)) {
        free(entries.nodes);
        return;
      }
      DebugStep close = steps->items[--steps->length];
      debug_step(steps, DEBUG_STEP_FREE, entries.nodes, 0, false);
      steps->items[steps->length++] = close;
      for (size_t i = entries.length; i-- > 0;) {
        debug_step(steps, DEBUG_STEP_PART_END, NULL, 0, false);
        debug_step(steps, DEBUG_STEP_VALUE, entries.nodes[i]->key, 0, false);
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
    /* Diagnostic data, read through `Cause.to_text`, like a function's body. */
    case VALUE_CAUSE: debug_text(buffer, "<Cause>"); return;
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
    case VALUE_PROCESS_MONITOR: debug_text(buffer, "<Monitor>"); return;
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

/* Failure causes (time-and-outcomes.md, "Failure context"). The interpreter's
   execution/causes.rs is the reference; the language tests run every cause
   program on both backends.

   A cause is a VALUE_CAUSE, immutable once built: adding a frame or a link
   builds a new one, so a cause shared by a stored Err and a live failure never
   changes under either. Its unnamed fields are the CAUSE_* slots below; a frame
   is a tuple of its text and its node (-1 when unknown), and `caused_by` is a
   cause or Void. A live outcome holds its cause in `ChimiControl.cause`; NULL
   means the fresh cause of the outcome itself, built only when read. */
enum {
  CAUSE_KIND, CAUSE_PAYLOAD, CAUSE_ORIGIN, CAUSE_FRAMES, CAUSE_DROPPED_FRAMES,
  CAUSE_CAUSED_BY, CAUSE_LINKS, CAUSE_DROPPED_LINKS, CAUSE_SUPPRESSED,
  CAUSE_DROPPED_SUPPRESSED, CAUSE_REASON, CAUSE_FIELD_COUNT
};
enum { CAUSE_EXPECTED_FAILURE, CAUSE_DEFECT, CAUSE_CANCELLATION };
enum { CAUSE_FRAME_LIMIT = 32, CAUSE_SUPPRESSED_LIMIT = 16, CAUSE_LINK_LIMIT = 64 };
static const char CANCELLATION_REASON[] = "requested before the next semantic operation";
/* The name of the hidden field after a stored Err's own fields that holds its
   cause. Equality, order, rendering and patterns read only `length` fields. */
static const char STORED_CAUSE_FIELD[] = "cause";

static ChimiValue *cause_field(const ChimiValue *cause, uint32_t field) {
  return cause->as.record.fields[field].value;
}
static int64_t cause_integer(const ChimiValue *cause, uint32_t field) {
  return cause_field(cause, field)->as.integer;
}

static ChimiValue *cause_new(int64_t kind, ChimiValue *payload, int64_t origin) {
  ChimiValue *cause = new_value(VALUE_CAUSE);
  cause->as.record.length = CAUSE_FIELD_COUNT;
  cause->as.record.fields = allocate(CAUSE_FIELD_COUNT, sizeof(*cause->as.record.fields));
  allocation_kind(cause->as.record.fields, ALLOCATION_FIELDS);
  ChimiValue *zero = integer_value(0), *empty = list_from_values(NULL, 0);
  ChimiValue *values[CAUSE_FIELD_COUNT] = {
    integer_value(kind), payload, integer_value(origin), empty, zero,
    void_value(), zero, zero, empty, zero, integer_value(-1),
  };
  for (uint32_t field = 0; field < CAUSE_FIELD_COUNT; ++field)
    cause->as.record.fields[field] = (RuntimeField){NULL, values[field]};
  return cause;
}

static ChimiValue *cause_copy(const ChimiValue *cause) {
  ChimiValue *copy = cause_new(0, NULL, 0);
  for (uint32_t field = 0; field < CAUSE_FIELD_COUNT; ++field)
    copy->as.record.fields[field].value = cause_field(cause, field);
  return copy;
}

static void cause_set(ChimiValue *cause, uint32_t field, ChimiValue *value) {
  cause->as.record.fields[field].value = value;
}

/* The outcome's cause, built from the outcome when it is still implicit. */
static ChimiValue *control_cause(const ChimiControl *control) {
  if (control->cause) return control->cause;
  switch (control->kind) {
    case CONTROL_FAILURE: return cause_new(CAUSE_EXPECTED_FAILURE, control->value, control->node);
    case CONTROL_DEFECT: return cause_new(CAUSE_DEFECT, text_value(control->message), control->node);
    case CONTROL_CANCEL: return cause_new(CAUSE_CANCELLATION, text_value(CANCELLATION_REASON), -1);
    case CONTROL_SUCCESS:
    case CONTROL_BREAK: break;
  }
  return NULL;
}

/* A copy of the outcome with `cause` as its cause. */
static ChimiControl *control_with_cause(const ChimiControl *control, ChimiValue *cause) {
  ChimiControl *copy = new_control(control->kind, control->value, control->message, control->node);
  copy->cause = cause;
  if (control->kind == CONTROL_FAILURE) copy->node = cause_integer(cause, CAUSE_ORIGIN);
  return copy;
}

/* `items` with `item` pushed; at `limit` it first drops the entry just past the
   kept oldest half. Sets `*dropped` when it drops one. */
static ChimiValue *cause_push_bounded(const ChimiValue *items, ChimiValue *item, uint32_t limit,
                                      bool *dropped) {
  uint32_t length = list_length(items);
  *dropped = length == limit;
  if (!*dropped) return list_push(items, item);
  ChimiValue **kept = new_value_slots(limit);
  uint32_t count = 0;
  for (uint32_t index = 0; index < length; ++index)
    if (index != limit / 2) kept[count++] = list_get(items, index);
  kept[count++] = item;
  return list_from_values(kept, count);
}

static ChimiValue *cause_with_frame(const ChimiValue *cause, ChimiValue *text, int64_t node) {
  ChimiValue *frame = new_value(VALUE_TUPLE);
  frame->as.record.length = 2;
  frame->as.record.fields = allocate(2, sizeof(*frame->as.record.fields));
  allocation_kind(frame->as.record.fields, ALLOCATION_FIELDS);
  frame->as.record.fields[0] = (RuntimeField){NULL, text};
  frame->as.record.fields[1] = (RuntimeField){NULL, integer_value(node)};
  ChimiValue *copy = cause_copy(cause);
  bool dropped = false;
  cause_set(copy, CAUSE_FRAMES, cause_push_bounded(cause_field(cause, CAUSE_FRAMES), frame,
                                                   CAUSE_FRAME_LIMIT, &dropped));
  if (dropped)
    cause_set(copy, CAUSE_DROPPED_FRAMES, integer_value(cause_integer(cause, CAUSE_DROPPED_FRAMES) + 1));
  return copy;
}

static ChimiValue *cause_below(const ChimiValue *cause) {
  ChimiValue *below = cause_field(cause, CAUSE_CAUSED_BY);
  return below->kind == VALUE_CAUSE ? below : NULL;
}

static void cause_link(ChimiValue *cause, ChimiValue *below) {
  cause_set(cause, CAUSE_CAUSED_BY, below ? below : void_value());
  cause_set(cause, CAUSE_LINKS, integer_value(below ? cause_integer(below, CAUSE_LINKS) + 1 : 0));
}

/* `cause` (fresh, unshared) linked to `below`. A chain longer than the limit
   keeps its newest and oldest halves: the link at depth LIMIT / 2 is dropped and
   the newer links above it are copied onto the rest of the chain. */
static ChimiValue *cause_linked(ChimiValue *cause, ChimiValue *below) {
  cause_link(cause, below);
  if (cause_integer(cause, CAUSE_LINKS) <= CAUSE_LINK_LIMIT) return cause;
  enum { KEEP_NEWEST = CAUSE_LINK_LIMIT / 2 };
  ChimiValue *newer[KEEP_NEWEST];
  uint32_t count = 0;
  ChimiValue *link = below;
  for (uint32_t depth = 1; depth < KEEP_NEWEST; ++depth) {
    newer[count++] = link;
    link = cause_below(link);
  }
  int64_t dropped = cause_integer(link, CAUSE_DROPPED_LINKS) + 1;
  ChimiValue *resumed = cause_below(link);
  ChimiValue *top = cause;
  if (count == 0) {
    cause_set(cause, CAUSE_DROPPED_LINKS, integer_value(cause_integer(cause, CAUSE_DROPPED_LINKS) + dropped));
  }
  for (uint32_t index = count; index-- > 0;) {
    ChimiValue *copy = cause_copy(newer[index]);
    if (index == count - 1)
      cause_set(copy, CAUSE_DROPPED_LINKS, integer_value(cause_integer(copy, CAUSE_DROPPED_LINKS) + dropped));
    cause_link(copy, resumed);
    resumed = copy;
  }
  cause_link(top, resumed);
  return top;
}

/* The cause a `Failure(...)` or `Result.Err(...)` built in a cause arm gets from
   `matched` (a cause, or NULL): the same one when it keeps it, otherwise a new
   cause of `error` started at `node`, linked to it. */
static ChimiValue *cause_derived(ChimiValue *matched, bool keep, ChimiValue *error, int64_t node) {
  if (keep) return matched;
  ChimiValue *fresh = cause_new(CAUSE_EXPECTED_FAILURE, error, node);
  return matched ? cause_linked(fresh, matched) : fresh;
}

/* The cause a stored Err keeps in its hidden field, or NULL. */
static ChimiValue *stored_cause(const ChimiValue *value) {
  if (value->kind != VALUE_CONSTRUCTED) return NULL;
  uint32_t length = value->as.constructed.length;
  size_t slots = allocation_header(value->as.constructed.fields)->bytes / sizeof(RuntimeField);
  if (slots <= length || value->as.constructed.fields[length].name != STORED_CAUSE_FIELD) return NULL;
  return value->as.constructed.fields[length].value;
}

/* A copy of the constructed `value` whose hidden field holds `cause`. */
static ChimiValue *with_stored_cause(const ChimiValue *value, ChimiValue *cause) {
  uint32_t length = value->as.constructed.length;
  ChimiValue *copy = new_value(VALUE_CONSTRUCTED);
  copy->as.constructed = value->as.constructed;
  copy->as.constructed.fields = allocate(length + 1, sizeof(*copy->as.constructed.fields));
  allocation_kind(copy->as.constructed.fields, ALLOCATION_FIELDS);
  memcpy(copy->as.constructed.fields, value->as.constructed.fields, length * sizeof(RuntimeField));
  copy->as.constructed.fields[length] = (RuntimeField){STORED_CAUSE_FIELD, cause};
  return copy;
}

/* The cause a cause arm binds for a matched subject: the live failure's, or the
   one a stored Err keeps, or Void. */
static ChimiValue *matched_cause_value(const ChimiControl *subject) {
  ChimiValue *cause = NULL;
  if (subject->kind == CONTROL_FAILURE) cause = control_cause(subject);
  else if (subject->kind == CONTROL_SUCCESS) cause = stored_cause(subject->value);
  return cause ? cause : void_value();
}

static void cause_location(TextBuffer *buffer, int64_t node) {
  if (node < 0) return;
  debug_text(buffer, " (");
  debug_text(buffer, CHIMI_STRINGS[CHIMI_ORIGINS[CHIMI_NODES[node].origin].location]);
  debug_text(buffer, ")");
}

static void cause_indent(TextBuffer *buffer, uint32_t depth) {
  buffer_char(buffer, '\n');
  for (uint32_t level = 0; level <= depth; ++level) debug_text(buffer, "  ");
}

static void cause_dropped(TextBuffer *buffer, uint32_t depth, int64_t count, const char *what) {
  char text[96];
  snprintf(text, sizeof(text), "(%" PRId64 " %s%s dropped)", count, what, count == 1 ? "" : "s");
  cause_indent(buffer, depth);
  debug_text(buffer, text);
}

static const char *const SUPPRESSION_LABELS[] = {"cleanup", "sibling", "displaced"};

/* `Cause.to_text`, as causes.rs `render` writes it. Bounded: at most 64 links,
   each with at most 32 frames and 16 suppressed causes. */
static void cause_render(TextBuffer *buffer, const ChimiValue *cause, uint32_t depth) {
  static const char *const KINDS[] = {"expected failure", "defect", "cancellation"};
  debug_text(buffer, KINDS[cause_integer(cause, CAUSE_KIND)]);
  buffer_char(buffer, ' ');
  ChimiValue *payload = cause_field(cause, CAUSE_PAYLOAD);
  if (cause_integer(cause, CAUSE_KIND) == CAUSE_EXPECTED_FAILURE) {
    ChimiValue *summary = debug_render(payload, false);
    buffer_bytes(buffer, summary->as.text.bytes, summary->as.text.length);
  } else {
    buffer_bytes(buffer, payload->as.text.bytes, payload->as.text.length);
  }
  cause_location(buffer, cause_integer(cause, CAUSE_ORIGIN));
  ChimiValue *frames = cause_field(cause, CAUSE_FRAMES);
  int64_t dropped_frames = cause_integer(cause, CAUSE_DROPPED_FRAMES);
  for (uint32_t index = 0; index < list_length(frames); ++index) {
    if (index == CAUSE_FRAME_LIMIT / 2 && dropped_frames > 0)
      cause_dropped(buffer, depth, dropped_frames, "context frame");
    ChimiValue *frame = list_get(frames, index);
    ChimiValue *text = frame->as.record.fields[0].value;
    cause_indent(buffer, depth);
    debug_text(buffer, "context: ");
    buffer_bytes(buffer, text->as.text.bytes, text->as.text.length);
    cause_location(buffer, frame->as.record.fields[1].value->as.integer);
  }
  ChimiValue *suppressed = cause_field(cause, CAUSE_SUPPRESSED);
  int64_t dropped_suppressed = cause_integer(cause, CAUSE_DROPPED_SUPPRESSED);
  for (uint32_t index = 0; index < list_length(suppressed); ++index) {
    if (index == CAUSE_SUPPRESSED_LIMIT / 2 && dropped_suppressed > 0)
      cause_dropped(buffer, depth, dropped_suppressed, "suppressed cause");
    ChimiValue *child = list_get(suppressed, index);
    int64_t reason = cause_integer(child, CAUSE_REASON);
    cause_indent(buffer, depth);
    debug_text(buffer, "suppressed (");
    debug_text(buffer, reason >= 0 && reason <= 2 ? SUPPRESSION_LABELS[reason] : "unknown");
    debug_text(buffer, "): ");
    cause_render(buffer, child, depth + 1);
  }
  ChimiValue *below = cause_below(cause);
  if (below) {
    int64_t dropped_links = cause_integer(cause, CAUSE_DROPPED_LINKS);
    if (dropped_links > 0) cause_dropped(buffer, depth, dropped_links, "cause");
    cause_indent(buffer, depth);
    debug_text(buffer, "caused by: ");
    cause_render(buffer, below, depth + 1);
  }
}

static ChimiValue *cause_text(const ChimiValue *cause) {
  TextBuffer buffer = {0};
  buffer_reserve(&buffer, 0);
  cause_render(&buffer, cause, 0);
  return text_from_bytes(buffer.data, buffer.length);
}

/* Whether a root outcome's record shows its cause: only when the cause says
   more than the record already does. */
static bool cause_has_detail(const ChimiValue *cause) {
  return list_length(cause_field(cause, CAUSE_FRAMES)) > 0 || cause_below(cause) ||
         list_length(cause_field(cause, CAUSE_SUPPRESSED)) > 0;
}

static ChimiValue *cause_option(ChimiValue *cause) {
  if (!cause) return standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0);
  return standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, &cause, 1);
}

/* The native core of std/cause; execution/causes.rs implements the same. */
static ChimiControl *invoke_cause_operation(ChimiRuntime *runtime, uint32_t intrinsic,
                                            ChimiValue **arguments, uint32_t argument_count,
                                            uint64_t span, uint32_t call_node,
                                            ChimiEnvironment *environment) {
  if (intrinsic == CHIMI_INTRINSIC_CAUSE_CONTEXT) {
    if (argument_count != 2 || arguments[0]->kind != VALUE_CALLABLE || arguments[1]->kind != VALUE_TEXT)
      return intrinsic_shape_defect(call_node);
    ChimiControl *control = invoke(runtime, arguments[0]->as.callable, NULL, 0, span, call_node, environment);
    ChimiValue *cause = control_cause(control);
    if (!cause) return control;
    return control_with_cause(control, cause_with_frame(cause, arguments[1], call_node));
  }
  if (intrinsic == CHIMI_INTRINSIC_CAUSE_OF) {
    if (argument_count != 1 || arguments[0]->kind != VALUE_CONSTRUCTED) return intrinsic_shape_defect(call_node);
    return success(cause_option(stored_cause(arguments[0])));
  }
  if (argument_count != 1 || arguments[0]->kind != VALUE_CAUSE) return intrinsic_shape_defect(call_node);
  ChimiValue *cause = arguments[0];
  switch (intrinsic) {
    case CHIMI_INTRINSIC_CAUSE_KIND_CODE: return success(cause_field(cause, CAUSE_KIND));
    case CHIMI_INTRINSIC_CAUSE_SUMMARY: {
      ChimiValue *payload = cause_field(cause, CAUSE_PAYLOAD);
      return success(cause_integer(cause, CAUSE_KIND) == CAUSE_EXPECTED_FAILURE
                       ? debug_render(payload, false) : payload);
    }
    case CHIMI_INTRINSIC_CAUSE_FRAMES: {
      ChimiValue *frames = cause_field(cause, CAUSE_FRAMES);
      uint32_t length = list_length(frames);
      ChimiValue **texts = new_value_slots(length);
      for (uint32_t index = 0; index < length; ++index)
        texts[index] = list_get(frames, index)->as.record.fields[0].value;
      return success(list_from_values(texts, length));
    }
    case CHIMI_INTRINSIC_CAUSE_CAUSED_BY: return success(cause_option(cause_below(cause)));
    case CHIMI_INTRINSIC_CAUSE_SUPPRESSED: return success(cause_field(cause, CAUSE_SUPPRESSED));
    case CHIMI_INTRINSIC_CAUSE_REASON_CODE: return success(cause_field(cause, CAUSE_REASON));
    case CHIMI_INTRINSIC_CAUSE_TO_TEXT: return success(cause_text(cause));
    default: return intrinsic_shape_defect(call_node);
  }
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
    /* A spec record with `handoff` set, as the interpreter builds it. It has no
       name: Process.register takes only a Process.Spec. */
    case CHIMI_INTRINSIC_PROCESS_HANDOFF_SPEC: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_CALLABLE ||
          arguments[1]->kind != VALUE_INTEGER || arguments[2]->kind != VALUE_DURATION)
        return intrinsic_shape_defect(call_node);
      if (arguments[1]->as.integer < 1)
        return process_range_defect(
          "Process.handoff_spec: mailbox capacity must be at least 1, got %" PRId64,
          arguments[1]->as.integer, call_node);
      if (arguments[2]->as.integer < 0)
        return process_duration_defect(
          "Process.handoff_spec: shutdown_within must be at least zero, got %s",
          arguments[2]->as.integer, call_node);
      static const char *const names[] = {"body", "capacity", "handoff", "name", "shutdown_within"};
      ChimiValue *values[] = {arguments[0], arguments[1], boolean_value(true),
                              standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0), arguments[2]};
      if (!values[3]) return intrinsic_shape_defect(call_node);
      return success(process_record(names, values, 5));
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

/* What every incarnation of one spec runs: its body, copied into each new heap. A
   handoff body also takes its start argument, which it asks the scheduler for. */
typedef struct {
  ChimiRuntime *runtime;
  RuntimeFragment *body;
  bool handoff;
} ProcessSpecContext;

/* The largest peak of live payload bytes of any incarnation's heap. */
static size_t process_heap_peak_value_bytes;

/* The defect messages of incarnations whose body defected, kept until the
   scheduler reports the incarnation's end (record_process_defect), which
   follows before any other incarnation of the same body can run. */
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
  ChimiValue *arguments[] = {new_value(VALUE_PROCESS_INBOX), NULL};
  uint32_t argument_count = 1;
  ChimiControl *control = NULL;
  if (context->handoff) {
    /* The start argument travelled as a message: adopting it makes this heap
       its owner, and the reply moved its capabilities to this incarnation. */
    ChimiProcessRequest request = {.kind = CHIMI_REQUEST_PROCESS_ARGUMENT};
    ChimiFiberCommand command = scheduler_request(&request);
    if (command == CHIMI_FIBER_CANCEL) {
      if (request.status == CHIMI_RESPONSE_OK) fragment_free((RuntimeFragment *)(uintptr_t)request.message);
      control = cancelled();
    } else if (request.status != CHIMI_RESPONSE_OK) {
      control = defect("Supervisor.start_handoff: the incarnation has no start argument", UINT32_MAX);
    } else {
      void *argument_roots[FRAGMENT_ROOTS];
      fragment_adopt((RuntimeFragment *)(uintptr_t)request.message, argument_roots);
      arguments[argument_count++] = argument_roots[0];
    }
  }
  uint64_t span = runtime->next_span++;
  if (!control)
    control = invoke(runtime, body->as.callable, arguments, argument_count, span, UINT32_MAX,
                     new_environment(0));
  ChimiFiberResult result = {.state = CHIMI_FIBER_SUCCEEDED, .value = CHIMI_PROCESS_END_RETURNED};
  switch (control->kind) {
    case CONTROL_SUCCESS: break;
    case CONTROL_FAILURE: result.value = CHIMI_PROCESS_END_FAILED; break;
    case CONTROL_CANCEL: result = (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED}; break;
    case CONTROL_DEFECT:
    case CONTROL_BREAK:
      remember_process_defect(control->message);
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
  ChimiValue *handoff = process_field(spec, "handoff");
  *context = (ProcessSpecContext){
    .runtime = runtime,
    .body = copy,
    .handoff = handoff && handoff->kind == VALUE_BOOLEAN && handoff->as.boolean,
  };
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
                                           uint32_t argument_count, uint64_t span, uint32_t call_node,
                                           ChimiEnvironment *environment) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_PROCESS_OWN_ADDRESS: {
      if (argument_count != 1 || !is_kind(arguments, 0, VALUE_PROCESS_INBOX))
        return intrinsic_shape_defect(call_node);
      ChimiProcessRequest request = {.kind = CHIMI_REQUEST_PROCESS_SELF};
      if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status != CHIMI_RESPONSE_OK) return defect("Process.address scheduler request failed", call_node);
      return success(process_address_value(CHIMI_ADDRESS_DIRECT, request.address));
    }
    case CHIMI_INTRINSIC_PROCESS_QUEUED:
    case CHIMI_INTRINSIC_PROCESS_CAPACITY: {
      if (argument_count != 1 || !is_kind(arguments, 0, VALUE_PROCESS_INBOX))
        return intrinsic_shape_defect(call_node);
      ChimiProcessRequest request = {.kind = CHIMI_REQUEST_PROCESS_MAILBOX};
      if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status != CHIMI_RESPONSE_OK) return defect("Process mailbox scheduler request failed", call_node);
      return success(integer_value(intrinsic == CHIMI_INTRINSIC_PROCESS_QUEUED ? (int64_t)request.queued
                                                                              : (int64_t)request.capacity));
    }
    case CHIMI_INTRINSIC_PROCESS_MONITOR: {
      if (argument_count != 3 || !is_kind(arguments, 0, VALUE_PROCESS_INBOX) ||
          !is_kind(arguments, 1, VALUE_PROCESS_ADDRESS) || !is_kind(arguments, 2, VALUE_CALLABLE))
        return intrinsic_shape_defect(call_node);
      /* The notice outlives this call in the scheduler, like an accepted message,
         until the receive that runs it adopts it back into this heap. */
      void *roots[] = {arguments[2]};
      AllocationKind kinds[] = {ALLOCATION_VALUE};
      RuntimeFragment *notice = NULL;
      if (fragment_copy(roots, kinds, 1, &notice) != HEAP_OK)
        return defect(numeric_message("Process.monitor: the notice points into owner %" PRIu64
                                      ", outside the watcher's heap (owner %" PRIu64 ")",
                                      fragment_copy_foreign_owner, current_heap->owner),
                      call_node);
      ChimiProcessRequest request = {
        .kind = CHIMI_REQUEST_PROCESS_MONITOR,
        .address_kind = arguments[1]->as.address.kind,
        .address = arguments[1]->as.address.target,
        .message = (uint64_t)(uintptr_t)notice,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      if (request.status != CHIMI_RESPONSE_OK) fragment_free(notice);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status != CHIMI_RESPONSE_OK) return defect("Process.monitor scheduler request failed", call_node);
      ChimiValue *monitor = new_value(VALUE_PROCESS_MONITOR);
      monitor->as.address.target = request.monitor;
      return success(monitor);
    }
    case CHIMI_INTRINSIC_PROCESS_UNMONITOR: {
      if (argument_count != 2 || !is_kind(arguments, 0, VALUE_PROCESS_INBOX) ||
          !is_kind(arguments, 1, VALUE_PROCESS_MONITOR))
        return intrinsic_shape_defect(call_node);
      ChimiProcessRequest request = {.kind = CHIMI_REQUEST_PROCESS_UNMONITOR,
                                     .monitor = arguments[1]->as.address.target};
      if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
      if (request.status != CHIMI_RESPONSE_OK) return defect("Process.unmonitor scheduler request failed", call_node);
      return success(void_value());
    }
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
      /* Receivers and Completions in the message travel with it, as through a
         channel: an accepted message puts them in flight, and the process that
         receives it owns them. A refused one leaves them with the sender. */
      CapabilityTransfer carried = {.collect = true};
      transfer_captures(&carried, message->roots[0]);
      ChimiMessageCapabilities capabilities = {carried.carried, carried.carried_count};
      ChimiProcessRequest request = {
        .kind = CHIMI_REQUEST_PROCESS_SEND,
        .address_kind = arguments[0]->as.address.kind,
        .address = arguments[0]->as.address.target,
        .message = (uint64_t)(uintptr_t)message,
        .carried = capabilities.count ? &capabilities : NULL,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      free(carried.carried);
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
        if (request.down == 0) return success(roots[0]);
        /* A monitor notice: the watcher's own fn(Down) -> M makes the message. */
        if (request.down > CHIMI_DOWN_UNAVAILABLE || ((ChimiValue *)roots[0])->kind != VALUE_CALLABLE)
          return defect("Process.receive scheduler request failed", call_node);
        ChimiValue *down[] = {
          standard_value(CHIMI_CONSTRUCTOR_DOWN_RETURNED + (request.down - CHIMI_DOWN_RETURNED), NULL, 0)};
        return invoke(runtime, ((ChimiValue *)roots[0])->as.callable, down, 1, span, call_node, environment);
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
    case CHIMI_INTRINSIC_SUPERVISOR_START_HANDOFF: {
      if (argument_count != 3 || !is_kind(arguments, 0, VALUE_SUPERVISOR_RUNNING))
        return intrinsic_shape_defect(call_node);
      ProcessMarshal marshal = {0};
      ChimiProcessSpec spec;
      ChimiControl *failed = marshal_spec(runtime, arguments[1], &marshal, &spec,
                                          "Supervisor.start_handoff", call_node);
      if (failed) {
        marshal_finish(&marshal, false);
        return failed;
      }
      /* The argument travels like a Process.send message: a copy in the caller's
         heap whose Receivers and Completions go in flight once the start is
         accepted. The new incarnation takes it before its body runs. */
      void *roots[] = {arguments[2]};
      AllocationKind kinds[] = {ALLOCATION_VALUE};
      RuntimeFragment *argument = NULL;
      if (fragment_copy(roots, kinds, 1, &argument) != HEAP_OK) {
        marshal_finish(&marshal, false);
        return defect(FOREIGN_POINTER_DEFECT, call_node);
      }
      CapabilityTransfer carried = {.collect = true};
      transfer_captures(&carried, argument->roots[0]);
      ChimiMessageCapabilities capabilities = {carried.carried, carried.carried_count};
      ChimiProcessRequest request = {
        .kind = CHIMI_REQUEST_SUPERVISOR_START_PROCESS,
        .supervisor = arguments[0]->as.managed.supervisor,
        .restart = CHIMI_RESTART_NEVER,
        .spec = &spec,
        .message = (uint64_t)(uintptr_t)argument,
        .carried = capabilities.count ? &capabilities : NULL,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      free(carried.carried);
      marshal_finish(&marshal, request.status != CHIMI_RESPONSE_UNSET);
      /* Only an accepted start keeps the argument; a refused one hands it back. */
      if (request.status != CHIMI_RESPONSE_OK) fragment_free(argument);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      ChimiValue *fields[] = {arguments[2]};
      switch (request.status) {
        case CHIMI_RESPONSE_OK:
          return success(managed_value(VALUE_MANAGED_PROCESS, arguments[0]->as.managed.supervisor,
                                       request.membership));
        case CHIMI_RESPONSE_CAPACITY_REACHED:
          return failure(standard_value(CHIMI_CONSTRUCTOR_HANDOFF_CAPACITY_REACHED, fields, 1));
        case CHIMI_RESPONSE_STOPPED:
          return failure(standard_value(CHIMI_CONSTRUCTOR_HANDOFF_STOPPED, fields, 1));
      }
      return defect("Supervisor.start_handoff scheduler request failed", call_node);
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
                                    uint32_t argument_count, uint64_t span, uint32_t call_node,
                                    ChimiEnvironment *environment) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_PROCESS_SPEC:
    case CHIMI_INTRINSIC_PROCESS_HANDOFF_SPEC:
    case CHIMI_INTRINSIC_PROCESS_REGISTER:
    case CHIMI_INTRINSIC_SUPERVISOR_PROCESS:
    case CHIMI_INTRINSIC_SUPERVISOR_SUPERVISOR:
    case CHIMI_INTRINSIC_SUPERVISOR_POLICY:
    case CHIMI_INTRINSIC_SUPERVISOR_RESTARTS:
      return invoke_process_data(intrinsic, arguments, argument_count, call_node);
  }
#if CHIMI_PROCESS_KERNEL
  return invoke_process_kernel(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
#else
  (void)runtime;
  (void)span;
  (void)environment;
  /* The compiler links the process runtime into every program that reaches one. */
  return defect("this runtime was built without the supervised process runtime", call_node);
#endif
}

/* Calls `step` with the state, starting at `initial`, then the key of each node
   of `entries` and, when `with_value`, its value; stops at the first call that does
   not succeed. Frees `entries`. */
static ChimiControl *reduce_in_order(ChimiRuntime *runtime, ChimiCallable *step, ChimiValue *initial,
                                     MapEntries entries, bool with_value, uint64_t span,
                                     uint32_t call_node, ChimiEnvironment *environment) {
  ChimiControl *control = success(initial);
  for (size_t index = 0; index < entries.length; ++index) {
    ChimiValue *step_arguments[] = {control->value, entries.nodes[index]->key, entries.nodes[index]->value};
    control = invoke(runtime, step, step_arguments, with_value ? 3 : 2, span, call_node, environment);
    if (control->kind != CONTROL_SUCCESS) break;
  }
  free(entries.nodes);
  return control;
}

static ChimiControl *option_value(ChimiValue *found) {
  if (!found) return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
  ChimiValue *fields[] = {found};
  return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
}

/* The native core of std/map; execution/map_operations.rs is the interpreter's. */
static ChimiControl *invoke_map_operation(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  if (intrinsic == CHIMI_INTRINSIC_MAP_EMPTY)
    return argument_count == 0 ? success(map_value(NULL)) : intrinsic_shape_defect(call_node);
  if (argument_count == 0 || arguments[0]->kind != VALUE_MAP) return intrinsic_shape_defect(call_node);
  MapNode *map = arguments[0]->as.map;
  switch (intrinsic) {
    case CHIMI_INTRINSIC_MAP_GET:
      if (argument_count != 2) break;
      return option_value(map_get_node(map, arguments[1]));
    case CHIMI_INTRINSIC_MAP_PUT:
      if (argument_count != 3) break;
      return success(map_value(map_put_node(map, arguments[1], arguments[2])));
    case CHIMI_INTRINSIC_MAP_REMOVE: {
      if (argument_count != 2) break;
      MapNode *removed = map_remove_node(map, arguments[1]);
      return success(removed == map ? arguments[0] : map_value(removed));
    }
    case CHIMI_INTRINSIC_MAP_SIZE:
      if (argument_count != 1) break;
      return success(integer_value(map_size(map)));
    case CHIMI_INTRINSIC_MAP_REDUCE_ENTRIES:
      if (argument_count != 3 || arguments[2]->kind != VALUE_CALLABLE) break;
      return reduce_in_order(runtime, arguments[2]->as.callable, arguments[1], map_entries(map), true,
                             span, call_node, environment);
  }
  return intrinsic_shape_defect(call_node);
}

/* The native core of std/set: a map whose values are Void. */
static ChimiControl *invoke_set_operation(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  if (intrinsic == CHIMI_INTRINSIC_SET_EMPTY)
    return argument_count == 0 ? success(set_value(NULL)) : intrinsic_shape_defect(call_node);
  if (argument_count == 0 || arguments[0]->kind != VALUE_SET) return intrinsic_shape_defect(call_node);
  MapNode *set = arguments[0]->as.map;
  switch (intrinsic) {
    case CHIMI_INTRINSIC_SET_CONTAINS:
      if (argument_count != 2) break;
      return success(boolean_value(map_get_node(set, arguments[1]) != NULL));
    case CHIMI_INTRINSIC_SET_ADD:
      if (argument_count != 2) break;
      return success(set_value(map_put_node(set, arguments[1], void_value())));
    case CHIMI_INTRINSIC_SET_REMOVE: {
      if (argument_count != 2) break;
      MapNode *removed = map_remove_node(set, arguments[1]);
      return success(removed == set ? arguments[0] : set_value(removed));
    }
    case CHIMI_INTRINSIC_SET_SIZE:
      if (argument_count != 1) break;
      return success(integer_value(map_size(set)));
    case CHIMI_INTRINSIC_SET_REDUCE_ITEMS:
      if (argument_count != 3 || arguments[2]->kind != VALUE_CALLABLE) break;
      return reduce_in_order(runtime, arguments[2]->as.callable, arguments[1], map_entries(set), false,
                             span, call_node, environment);
  }
  return intrinsic_shape_defect(call_node);
}

/* The exact value of a JSON number as +-significant x 10^exponent, where
   `significant` (the spelling's digits, '.' skipped) has no leading or trailing
   zeros and is empty for zero, and the scale the spelling writes: fraction digits
   minus the written exponent. As in json.rs, a written exponent is clamped to
   JSON_EXPONENT_CLAMP, which only numbers far outside Int and Decimal reach. */
#define JSON_EXPONENT_CLAMP 1000000000000000LL
typedef struct { bool negative; char *significant; int64_t length; int64_t exponent; int64_t written_scale; } JsonExactNumber;
static JsonExactNumber json_exact_number(const char *text) {
  JsonExactNumber number = {0};
  number.negative = *text == '-'; if (number.negative) ++text;
  size_t mantissa = strcspn(text, "eE"), point = strcspn(text, ".eE");
  int64_t fraction_digits = point < mantissa ? (int64_t)(mantissa - point - 1) : 0;
  int64_t written = 0; bool written_negative = false;
  if (text[mantissa]) {
    const char *cursor = text + mantissa + 1;
    written_negative = *cursor == '-'; if (*cursor == '-' || *cursor == '+') ++cursor;
    for (; *cursor; ++cursor) {
      written = written * 10 + (*cursor - '0');
      if (written > JSON_EXPONENT_CLAMP) written = JSON_EXPONENT_CLAMP;
    }
  }
  if (written_negative) written = -written;
  number.significant = allocate(mantissa + 1, 1);
  int64_t length = 0;
  for (size_t index = 0; index < mantissa; ++index)
    if (text[index] != '.' && (length > 0 || text[index] != '0')) number.significant[length++] = text[index];
  int64_t trailing = 0;
  while (length > 0 && number.significant[length - 1] == '0') { --length; ++trailing; }
  number.length = length;
  number.exponent = written - fraction_digits + trailing;
  number.written_scale = fraction_digits - written;
  return number;
}

/* json.rs `number_to_int`: true with `output` set, or false with `not_integer`
   telling NotInteger from OutOfRange. */
static bool json_number_to_int(const char *text, int64_t *output, bool *not_integer) {
  JsonExactNumber number = json_exact_number(text);
  *not_integer = false; *output = 0;
  if (number.length == 0) return true;
  if (number.exponent < 0) { *not_integer = true; return false; }
  if (number.length + number.exponent > 19) return false;
  unsigned __int128 magnitude = 0;
  for (int64_t index = 0; index < number.length; ++index) magnitude = magnitude * 10 + (unsigned)(number.significant[index] - '0');
  for (int64_t zero = 0; zero < number.exponent; ++zero) magnitude *= 10;
  if (magnitude > (unsigned __int128)INT64_MAX + (number.negative ? 1u : 0u)) return false;
  *output = number.negative ? (int64_t)(0 - (uint64_t)magnitude) : (int64_t)magnitude;
  return true;
}

/* json.rs `number_to_decimal`: the exact value at the spelling's own scale, or
   the largest smaller scale that fits; false past 38 significant or fraction digits. */
static bool json_number_to_decimal(const char *text, Decimal *output) {
  JsonExactNumber number = json_exact_number(text);
  int64_t max_scale = DECIMAL_MAX_SCALE;
  if (number.length == 0) {
    int64_t scale = number.written_scale < 0 ? 0 : number.written_scale > max_scale ? max_scale : number.written_scale;
    return decimal_make(false, 0, (uint32_t)scale, output);
  }
  if (number.length + (number.exponent > 0 ? number.exponent : 0) > max_scale || -number.exponent > max_scale) return false;
  /* At least the smallest scale, max(0, -exponent), as in json.rs. */
  int64_t scale = number.written_scale < 0 ? 0 : number.written_scale;
  if (scale > max_scale) scale = max_scale;
  if (scale > max_scale - number.length - number.exponent) scale = max_scale - number.length - number.exponent;
  DecimalMagnitude magnitude = 0;
  for (int64_t index = 0; index < number.length; ++index) magnitude = magnitude * 10 + (DecimalMagnitude)(number.significant[index] - '0');
  for (int64_t zero = 0; zero < number.exponent + scale; ++zero) magnitude *= 10;
  return decimal_make(number.negative, magnitude, (uint32_t)scale, output);
}

/* The native core of std/json. */
static ChimiControl *invoke_json_operation(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  (void)runtime; (void)span; (void)environment;
  switch (intrinsic) {
    case CHIMI_INTRINSIC_JSON_DECODE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      JsonParser parser = {arguments[0]->as.text.bytes, arguments[0]->as.text.length, 0, NULL};
      ChimiValue *value = json_parse_value(&parser, 0); if (value) json_skip_space(&parser);
      if (value && parser.offset == parser.length) return success(value);
      if (!parser.message) parser.message = "unexpected characters after JSON value";
      ChimiValue *fields[] = {text_value(parser.message), integer_value((int64_t)parser.offset)};
      return failure(standard_value(CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON, fields, 2));
    }
    case CHIMI_INTRINSIC_JSON_ENCODE: {
      if (argument_count != 1) break;
      TextBuffer buffer = {0};
      if (!encode_json_value(&buffer, arguments[0])) break;
      if (!buffer.data) buffer.data = allocate(1, 1);
      return success(text_value(buffer.data));
    }
    case CHIMI_INTRINSIC_JSON_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) break;
      char *number = allocate(32, 1); snprintf(number, 32, "%" PRId64, arguments[0]->as.integer);
      ChimiValue *raw = json_number_value(number); ChimiValue *fields[] = {raw};
      return success(standard_value(CHIMI_CONSTRUCTOR_JSON_NUMBER, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_NUMBER_TO_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_JSON_NUMBER) break;
      int64_t value; bool not_integer;
      if (json_number_to_int(arguments[0]->as.json_number, &value, &not_integer)) return success(integer_value(value));
      return failure(standard_value(not_integer ? CHIMI_CONSTRUCTOR_JSON_NOT_INTEGER : CHIMI_CONSTRUCTOR_JSON_OUT_OF_RANGE, NULL, 0));
    }
    case CHIMI_INTRINSIC_JSON_NUMBER_TO_DECIMAL: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_JSON_NUMBER) break;
      Decimal value;
      if (json_number_to_decimal(arguments[0]->as.json_number, &value)) return success(decimal_value(value));
      return failure(standard_value(CHIMI_CONSTRUCTOR_JSON_OUT_OF_RANGE, NULL, 0));
    }
  }
  return intrinsic_shape_defect(call_node);
}

/* `Some(value)` when `fits`, otherwise `None`. */
static ChimiControl *optional_decimal(bool fits, Decimal value) {
  if (!fits) return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
  ChimiValue *fields[] = {decimal_value(value)};
  return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
}

/* The native core of std/decimal. Requires: the Chimi wrappers checked the
   documented preconditions; a broken one is a shape defect, as in the interpreter. */
static ChimiControl *invoke_decimal_operation(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  (void)runtime; (void)span; (void)environment;
  Decimal output;
  switch (intrinsic) {
    case CHIMI_INTRINSIC_DECIMAL_SCALED: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_INTEGER || arguments[1]->kind != VALUE_INTEGER ||
          arguments[1]->as.integer < 0 || arguments[1]->as.integer > DECIMAL_MAX_SCALE)
        break;
      int64_t coefficient = arguments[0]->as.integer;
      DecimalMagnitude magnitude = coefficient < 0 ? (DecimalMagnitude)(-(coefficient + 1)) + 1 : (DecimalMagnitude)coefficient;
      decimal_make(coefficient < 0, magnitude, (uint32_t)arguments[1]->as.integer, &output);
      return success(decimal_value(output));
    }
    case CHIMI_INTRINSIC_DECIMAL_READ:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      /* decimal_parse reads a C string, so a U+0000 inside is no digit: Invalid. */
      if (text_holds_nul(arguments[0])) return optional_decimal(false, (Decimal){0});
      return optional_decimal(decimal_parse(arguments[0]->as.text.bytes, &output), output);
    case CHIMI_INTRINSIC_DECIMAL_TO_TEXT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_DECIMAL) break;
      char text[DECIMAL_TEXT_BYTES]; decimal_text(decimal_of(arguments[0]), text);
      return success(text_value(text));
    }
    /* The nearest double, ties to even: strtod rounds the exact text correctly. */
    case CHIMI_INTRINSIC_DECIMAL_TO_FLOAT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_DECIMAL) break;
      char text[DECIMAL_TEXT_BYTES]; decimal_text(decimal_of(arguments[0]), text);
      return success(float_value(strtod(text, NULL)));
    }
    case CHIMI_INTRINSIC_DECIMAL_ADD_CHECKED:
    case CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED:
    case CHIMI_INTRINSIC_DECIMAL_MULTIPLY_CHECKED: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_DECIMAL || arguments[1]->kind != VALUE_DECIMAL) break;
      Decimal left = decimal_of(arguments[0]), right = decimal_of(arguments[1]);
      bool fits = intrinsic == CHIMI_INTRINSIC_DECIMAL_ADD_CHECKED ? decimal_add(left, right, &output)
                : intrinsic == CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED ? decimal_add(left, decimal_negate(right), &output)
                : decimal_multiply(left, right, &output);
      return optional_decimal(fits, output);
    }
    case CHIMI_INTRINSIC_DECIMAL_DIVIDE_ROUNDED: {
      if (argument_count != 4 || arguments[0]->kind != VALUE_DECIMAL || arguments[1]->kind != VALUE_DECIMAL ||
          arguments[2]->kind != VALUE_INTEGER || arguments[3]->kind != VALUE_INTEGER ||
          arguments[2]->as.integer < 0 || arguments[2]->as.integer > DECIMAL_MAX_SCALE ||
          arguments[3]->as.integer < ROUND_TOWARD_ZERO || arguments[3]->as.integer > ROUND_HALF_EVEN)
        break;
      bool fits = decimal_divide(decimal_of(arguments[0]), decimal_of(arguments[1]),
                                 (uint32_t)arguments[2]->as.integer, (DecimalRounding)arguments[3]->as.integer, &output);
      return optional_decimal(fits, output);
    }
  }
  return intrinsic_shape_defect(call_node);
}

/* std/clock and timeouts (time-and-outcomes.md, "Time sources" and "Timeouts and
   deadlines").

   A `clock` provider bound by `using` sits in CHIMI_CLOCK_REQUIREMENT_SLOT, and every
   std/clock function reads time through it. With none bound, the runner's execution
   clock provider answers: the scheduler's real or virtual source. Its methods are
   HOST_CLOCK_* callables, and referencing the `clock` need builds it as a record, so
   it can be passed on, for example to `Clock.starting_at`.

   `Task.timeout` and `Task.deadline` read and wait on the execution time source, not
   on a bound provider: a scope's deadline is a scheduler timer. */
static ChimiValue *instant_value(int64_t nanoseconds) {
  ChimiValue *v = new_value(VALUE_INSTANT); v->as.integer = nanoseconds; return v;
}
static ChimiValue *timestamp_value(int64_t nanoseconds) {
  ChimiValue *v = new_value(VALUE_TIMESTAMP); v->as.integer = nanoseconds; return v;
}
static ChimiValue *clock_host_value(HostFunction function) {
  ChimiCallable *callable = allocate(1, sizeof(*callable));
  allocation_kind(callable, ALLOCATION_CALLABLE);
  callable->kind = CALL_HOST; callable->host = function; callable->name = "clock";
  return callable_value(callable);
}
/* A Clock record: `now`, `sleep_until`, `timestamp`, in field order. */
static ChimiValue *clock_record(ChimiValue *now, ChimiValue *sleep_until, ChimiValue *timestamp) {
  ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = 3;
  value->as.record.fields = allocate(3, sizeof(*value->as.record.fields));
  allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
  value->as.record.fields[0] = (RuntimeField){"now", now};
  value->as.record.fields[1] = (RuntimeField){"sleep_until", sleep_until};
  value->as.record.fields[2] = (RuntimeField){"timestamp", timestamp};
  return value;
}
static ChimiValue *execution_clock_value(void) {
  return clock_record(clock_host_value(HOST_CLOCK_NOW), clock_host_value(HOST_CLOCK_SLEEP_UNTIL),
                      clock_host_value(HOST_CLOCK_TIMESTAMP));
}

/* Reads the execution source's now. Reading checks cancellation. NULL on success. */
static ChimiControl *execution_now(int64_t *now, uint32_t call_node) {
  ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_TIMER_NOW};
  if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
  if (request.status != CHIMI_RESPONSE_OK) return defect("Clock scheduler request failed", call_node);
  *now = request.value;
  return NULL;
}
/* Parks until `deadline`; at or before now it returns without parking. */
static ChimiControl *execution_sleep_until(int64_t deadline, uint32_t call_node) {
  for (;;) {
    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_TIMER_SLEEP_UNTIL, .value = deadline};
    if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
    if (request.status == CHIMI_RESPONSE_OK) return success(void_value());
    if (request.status != CHIMI_RESPONSE_PENDING)
      return defect("Clock scheduler request failed", call_node);
  }
}
/* Virtual wall time is `--clock-start` plus now, so both advance together; real wall
   time is the host's. */
static ChimiControl *execution_timestamp(ChimiRuntime *runtime, uint32_t call_node) {
  int64_t now;
  ChimiControl *stopped = execution_now(&now, call_node);
  if (stopped) return stopped;
  if (runtime->virtual_clock) {
    int64_t wall;
    if (__builtin_add_overflow(runtime->clock_start, now, &wall))
      return defect("`Clock.timestamp` overflows the Timestamp range", call_node);
    return success(timestamp_value(wall));
  }
  struct timespec host;
  clock_gettime(CLOCK_REALTIME, &host);
  return success(timestamp_value((int64_t)host.tv_sec * INT64_C(1000000000) + host.tv_nsec));
}

static ChimiControl *invoke_clock_host(ChimiRuntime *runtime, ChimiCallable *callable,
                                       ChimiValue **arguments, uint32_t argument_count,
                                       uint64_t span, uint32_t call_node,
                                       ChimiEnvironment *environment) {
  switch (callable->host) {
    case HOST_CLOCK_NOW: {
      int64_t now;
      ChimiControl *stopped = execution_now(&now, call_node);
      return stopped ? stopped : success(instant_value(now));
    }
    case HOST_CLOCK_TIMESTAMP: return execution_timestamp(runtime, call_node);
    case HOST_CLOCK_SLEEP_UNTIL:
      if (argument_count != 1 || arguments[0]->kind != VALUE_INSTANT)
        return intrinsic_shape_defect(call_node);
      return execution_sleep_until(arguments[0]->as.integer, call_node);
    case HOST_CLOCK_SHIFTED_TIMESTAMP: {
      ChimiControl *now = invoke(runtime, callable->inner, NULL, 0, span, call_node, environment);
      if (now->kind != CONTROL_SUCCESS) return now;
      if (now->value->kind != VALUE_INSTANT) return intrinsic_shape_defect(call_node);
      int64_t wall;
      if (__builtin_add_overflow(now->value->as.integer, callable->shift, &wall))
        return defect("`Clock.starting_at` clock's timestamp overflows the Timestamp range", call_node);
      return success(timestamp_value(wall));
    }
    case HOST_SELECT_SLEEP: {
      ChimiValue *at[] = {instant_value(callable->shift)};
      return invoke(runtime, callable->inner, at, 1, span, call_node, environment);
    }
    default: return defect("unknown clock host function", call_node);
  }
}

/* Calls `method` of the bound clock provider, or of the execution clock. */
static ChimiControl *clock_method(ChimiRuntime *runtime, ChimiEnvironment *environment,
                                  const char *method, ChimiValue **arguments,
                                  uint32_t argument_count, uint64_t span, uint32_t call_node) {
  ChimiValue *provider = environment->requirements[CHIMI_CLOCK_REQUIREMENT_SLOT];
  if (!provider) provider = execution_clock_value();
  ChimiValue *callable = select_value(provider, method);
  if (!callable || callable->kind != VALUE_CALLABLE)
    return defect("provider has no callable field", call_node);
  return invoke(runtime, callable->as.callable, arguments, argument_count, span, call_node, environment);
}

static ChimiControl *timed_out(int64_t limit, uint32_t call_node) {
  ChimiValue *fields[] = {duration_value(limit)};
  ChimiValue *error = standard_value(CHIMI_CONSTRUCTOR_TIMEOUT_TIMED_OUT, fields, 1);
  return error ? failure(error) : defect("Timeout.TimedOut is missing from the program", call_node);
}

/* Runs `operation` in a cancellation scope of the calling task that the scheduler
   fires at `deadline` (time-and-outcomes.md, "Semantics" of timeouts):
   - A deadline at or before now never starts the operation.
   - The operation's own outcome stands: success, failure, defect, or a cancellation
     that is not this scope's (the task's, an enclosing scope's, or shutdown's).
   - The scope's cancellation becomes `Timeout.TimedOut(limit)` here. The scheduler
     says whether it is that cancellation when the scope exits; a terminal
     cancellation exits nothing, because the fiber must not park again. */
static ChimiControl *run_in_scope(ChimiRuntime *runtime, ChimiValue *operation, int64_t now,
                                  int64_t deadline, int64_t limit, uint64_t span,
                                  uint32_t call_node, ChimiEnvironment *environment) {
  if (deadline <= now) return timed_out(limit, call_node);
  ChimiSchedulerRequest enter = {.kind = CHIMI_REQUEST_SCOPE_ENTER, .value = deadline};
  if (scheduler_request(&enter) == CHIMI_FIBER_CANCEL) return cancelled();
  if (enter.status != CHIMI_RESPONSE_OK) return defect("Task.timeout scheduler request failed", call_node);
  ChimiControl *control = invoke(runtime, operation->as.callable, NULL, 0, span, call_node, environment);
  if (control->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal()) return control;
  for (;;) {
    /* A cancelled operation's values are discarded, so the scope's capabilities go. */
    ChimiSchedulerRequest exit = {.kind = CHIMI_REQUEST_SCOPE_EXIT, .token = enter.token,
                                  .value = control->kind == CONTROL_CANCEL};
    if (scheduler_request(&exit) == CHIMI_FIBER_CANCEL) return cancelled();
    if (exit.status == CHIMI_RESPONSE_PENDING) continue;
    if (exit.status != CHIMI_RESPONSE_OK) return defect("Task.timeout scheduler request failed", call_node);
    if (control->kind == CONTROL_CANCEL && exit.value == 1) return timed_out(limit, call_node);
    return control;
  }
}

/* The outcome of Resource.scoped once release has run (std/resource.chimi): a use
   defect stays; then a release defect; then a use cancellation; then a release
   failure or cancellation replaces the use's value or failure. */
static ChimiControl *after_release(ChimiControl *used, ChimiControl *released) {
  if (used->kind == CONTROL_DEFECT) return used;
  if (released->kind == CONTROL_DEFECT) return released;
  if (used->kind == CONTROL_CANCEL) return used;
  if (released->kind == CONTROL_FAILURE || released->kind == CONTROL_CANCEL) return released;
  return used;
}

/* Issues a Resource.scoped entry request; true when the fiber resumed with a
   cancellation instead of a plain reply (the request may still have been
   served: check its status). */
static bool resource_request(uint32_t kind, ChimiSchedulerRequest *request) {
  request->kind = kind;
  return scheduler_request(request) == CHIMI_FIBER_CANCEL;
}

/* A cancellation in place of `outcome`, which a defect outranks. */
static ChimiControl *cancelled_unless_defect(ChimiControl *outcome) {
  return outcome->kind == CONTROL_DEFECT ? outcome : cancelled();
}

/* Resource.scoped(acquire, release, use) (std/resource.chimi). Under the scheduler
   the call is an entry on the task's scope stack (scheduler/scope.rs): entered
   before acquire, so a cancellation of the task always finds it; masked before
   release, so no cancellation interrupts release; left after it, which delivers a
   cancellation release held off. A forced end (a terminal cancel) issues no
   request and runs no release it has not begun: the fiber must not park. */
static ChimiControl *resource_scoped(ChimiRuntime *runtime, ChimiValue **arguments,
                                     uint32_t argument_count, uint64_t span,
                                     uint32_t call_node, ChimiEnvironment *environment) {
  if (argument_count != 3 || arguments[0]->kind != VALUE_CALLABLE ||
      arguments[1]->kind != VALUE_CALLABLE || arguments[2]->kind != VALUE_CALLABLE)
    return intrinsic_shape_defect(call_node);
  ChimiValue *acquire = arguments[0], *release = arguments[1], *use = arguments[2];
  bool scheduled = CHIMI_REQUIRES_SCHEDULER;
  ChimiSchedulerRequest entry = {0};
  if (scheduled) {
    bool cancel = resource_request(CHIMI_REQUEST_RESOURCE_ENTER, &entry);
    if (entry.status == CHIMI_RESPONSE_UNSET) return cancelled();
    if (entry.status != CHIMI_RESPONSE_OK) return defect("Resource.scoped scheduler request failed", call_node);
    if (cancel) {
      /* Nothing is acquired yet: leave the entry and pass the cancellation on. */
      if (!chimi_fiber_cancel_is_terminal()) {
        ChimiSchedulerRequest exit = {.token = entry.token};
        resource_request(CHIMI_REQUEST_RESOURCE_EXIT, &exit);
      }
      return cancelled();
    }
  }
  ChimiControl *acquired = invoke(runtime, acquire->as.callable, NULL, 0, span, call_node, environment);
  if (acquired->kind != CONTROL_SUCCESS) {
    if (scheduled && !(acquired->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal())) {
      ChimiSchedulerRequest exit = {.token = entry.token};
      bool cancel = resource_request(CHIMI_REQUEST_RESOURCE_EXIT, &exit);
      if (cancel || exit.value == 1) return cancelled_unless_defect(acquired);
      if (exit.status != CHIMI_RESPONSE_OK) return defect("Resource.scoped scheduler request failed", call_node);
    }
    return acquired;
  }
  ChimiValue *handle = acquired->value;
  ChimiControl *used = invoke(runtime, use->as.callable, &handle, 1, span, call_node, environment);
  if (used->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal()) return used;
  if (scheduled) {
    ChimiSchedulerRequest mask = {.token = entry.token};
    bool cancel = resource_request(CHIMI_REQUEST_RESOURCE_RELEASE, &mask);
    if (cancel && chimi_fiber_cancel_is_terminal()) return cancelled_unless_defect(used);
    if (mask.status != CHIMI_RESPONSE_OK) {
      if (mask.status == CHIMI_RESPONSE_UNSET) return cancelled_unless_defect(used);
      return defect("Resource.scoped scheduler request failed", call_node);
    }
  }
  ChimiControl *released = invoke(runtime, release->as.callable, &handle, 1, span, call_node, environment);
  ChimiControl *outcome = after_release(used, released);
  if (released->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal()) return outcome;
  if (scheduled) {
    ChimiSchedulerRequest exit = {.token = entry.token};
    bool cancel = resource_request(CHIMI_REQUEST_RESOURCE_EXIT, &exit);
    if (cancel || exit.value == 1) return cancelled_unless_defect(outcome);
    if (exit.status != CHIMI_RESPONSE_OK) return defect("Resource.scoped scheduler request failed", call_node);
  }
  return outcome;
}

static ChimiControl *invoke_clock(ChimiRuntime *runtime, uint32_t intrinsic, ChimiValue **arguments,
                                  uint32_t argument_count, uint64_t span, uint32_t call_node,
                                  ChimiEnvironment *environment) {
#define CLOCK_SHAPE(count, ...) \
  do { if (argument_count != (count)) return intrinsic_shape_defect(call_node); \
       ValueKind kinds[] = {__VA_ARGS__}; \
       for (uint32_t i = 0; i < (count); ++i) \
         if (arguments[i]->kind != kinds[i]) return intrinsic_shape_defect(call_node); } while (0)
  int64_t result;
  switch (intrinsic) {
    case CHIMI_INTRINSIC_CLOCK_NOW:
      if (argument_count != 0) return intrinsic_shape_defect(call_node);
      return clock_method(runtime, environment, "now", NULL, 0, span, call_node);
    case CHIMI_INTRINSIC_CLOCK_TIMESTAMP:
      if (argument_count != 0) return intrinsic_shape_defect(call_node);
      return clock_method(runtime, environment, "timestamp", NULL, 0, span, call_node);
    case CHIMI_INTRINSIC_CLOCK_SLEEP_UNTIL:
      CLOCK_SHAPE(1, VALUE_INSTANT);
      return clock_method(runtime, environment, "sleep_until", arguments, 1, span, call_node);
    case CHIMI_INTRINSIC_CLOCK_SINCE: {
      CLOCK_SHAPE(1, VALUE_INSTANT);
      ChimiControl *now = clock_method(runtime, environment, "now", NULL, 0, span, call_node);
      if (now->kind != CONTROL_SUCCESS) return now;
      if (now->value->kind != VALUE_INSTANT) return intrinsic_shape_defect(call_node);
      if (__builtin_sub_overflow(now->value->as.integer, arguments[0]->as.integer, &result))
        return defect("`Clock.since` overflows the Duration range", call_node);
      return success(duration_value(result));
    }
    case CHIMI_INTRINSIC_CLOCK_ADD:
      CLOCK_SHAPE(2, VALUE_INSTANT, VALUE_DURATION);
      if (__builtin_add_overflow(arguments[0]->as.integer, arguments[1]->as.integer, &result))
        return defect("`Clock.add` overflows the Instant range", call_node);
      return success(instant_value(result));
    case CHIMI_INTRINSIC_CLOCK_BETWEEN:
      CLOCK_SHAPE(2, VALUE_INSTANT, VALUE_INSTANT);
      if (__builtin_sub_overflow(arguments[1]->as.integer, arguments[0]->as.integer, &result))
        return defect("`Clock.between` overflows the Duration range", call_node);
      return success(duration_value(result));
    case CHIMI_INTRINSIC_CLOCK_TIMESTAMP_TO_TEXT: {
      CLOCK_SHAPE(1, VALUE_TIMESTAMP);
      char text[48]; timestamp_text(arguments[0]->as.integer, text);
      return success(text_value(text));
    }
    case CHIMI_INTRINSIC_CLOCK_UNIX_NANOSECONDS:
      CLOCK_SHAPE(1, VALUE_TIMESTAMP);
      return success(integer_value(arguments[0]->as.integer));
    case CHIMI_INTRINSIC_CLOCK_FROM_UNIX_NANOSECONDS:
      CLOCK_SHAPE(1, VALUE_INTEGER);
      return success(timestamp_value(arguments[0]->as.integer));
    case CHIMI_INTRINSIC_CLOCK_TIMESTAMP_BETWEEN:
      CLOCK_SHAPE(2, VALUE_TIMESTAMP, VALUE_TIMESTAMP);
      if (__builtin_sub_overflow(arguments[1]->as.integer, arguments[0]->as.integer, &result))
        return defect("`Clock.timestamp_between` overflows the Duration range", call_node);
      return success(duration_value(result));
    case CHIMI_INTRINSIC_CLOCK_STARTING_AT: {
      CLOCK_SHAPE(2, VALUE_RECORD, VALUE_TIMESTAMP);
      ChimiValue *now = select_value(arguments[0], "now");
      ChimiValue *sleep_until = select_value(arguments[0], "sleep_until");
      if (!now || now->kind != VALUE_CALLABLE || !sleep_until)
        return defect("provider has no callable field", call_node);
      ChimiControl *started = invoke(runtime, now->as.callable, NULL, 0, span, call_node, environment);
      if (started->kind != CONTROL_SUCCESS) return started;
      if (started->value->kind != VALUE_INSTANT) return intrinsic_shape_defect(call_node);
      int64_t shift;
      if (__builtin_sub_overflow(arguments[1]->as.integer, started->value->as.integer, &shift))
        return defect("`Clock.starting_at` overflows the Timestamp range", call_node);
      ChimiValue *timestamp = clock_host_value(HOST_CLOCK_SHIFTED_TIMESTAMP);
      timestamp->as.callable->inner = now->as.callable;
      timestamp->as.callable->shift = shift;
      return success(clock_record(now, sleep_until, timestamp));
    }
    case CHIMI_INTRINSIC_TASK_TIMEOUT:
    case CHIMI_INTRINSIC_TASK_DEADLINE: {
      bool timeout = intrinsic == CHIMI_INTRINSIC_TASK_TIMEOUT;
      CLOCK_SHAPE(2, VALUE_CALLABLE, timeout ? VALUE_DURATION : VALUE_INSTANT);
      int64_t now, deadline, limit;
      ChimiControl *stopped = execution_now(&now, call_node);
      if (stopped) return stopped;
      if (timeout) {
        limit = arguments[1]->as.integer;
        if (__builtin_add_overflow(now, limit, &deadline)) {
          char text[32]; duration_text(limit, text);
          return defect(numeric_message("`Task.timeout(%s)` overflows the Instant range", text), call_node);
        }
      } else {
        deadline = arguments[1]->as.integer;
        if (__builtin_sub_overflow(deadline, now, &limit))
          return defect("`Task.deadline` is too far from now for a Duration", call_node);
      }
      return run_in_scope(runtime, arguments[0], now, deadline, limit, span, call_node, environment);
    }
  }
#undef CLOCK_SHAPE
  return defect("unknown standard intrinsic", call_node);
}

/* Starts `callable` as a child of the calling task (Task.start's contract) and
   returns its handle. */
static ChimiControl *start_task(ChimiRuntime *runtime, ChimiValue *callable, uint64_t span,
                                uint32_t call_node, ChimiEnvironment *environment) {
  /* The child runs in its own heap, so it gets a copy of what it captured. */
  void *roots[] = {callable, capture_requirements(environment)};
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
     transfer below rewrites the capabilities in the child's copy, and the
     context is still the child's to read. */
  context->parent = request.owner;
  CapabilityTransfer transfer = {.child = request.task};
  bool transferred = transfer_captures(&transfer, captures->roots[0]);
  if (!transferred) {
    if (transfer.cancelled) return cancelled();
    return defect("Task.start could not transfer an exclusive capture", call_node);
  }
  return success(task_value(request.task));
}

/* Task.all, Task.results and Task.race (time-and-outcomes.md, "Task combinators").

   Each operation runs as a child of the calling task. The children still running
   are `waiting[0, live)`, and `positions` holds each one's index in the input list.
   TASK_ANY answers with the child that terminated first, in termination order, and
   reports its outcome without applying it to the caller.

   Once the outcome is decided (Task.all's first expected failure, Task.race's
   first success or failure, or a child cancelled or overflowing), the rest are
   cancelled and drained with TASK_DRAIN, which is not a cancellation point: a scope
   that fires meanwhile cannot replace the decided outcome. Task.all keeps the
   failures of siblings that terminated before their cancellation took effect as
   suppressed causes with reason Sibling, in termination order.

   A child defect needs no code here: the scheduler unwinds this task's containment
   when the child terminates, which resumes this fiber with a terminal cancel. */
typedef struct {
  uint64_t *waiting;
  uint32_t *positions;
  uint32_t live;
} CombinatorChildren;

enum { CAUSE_REASON_SIBLING = 1 };

static void combinator_children_free(CombinatorChildren *children) {
  free(children->waiting);
  free(children->positions);
}

/* Waits for the first live child to terminate and removes it. Returns its outcome
   (NULL when it was cancelled) and sets `*position`, or sets `*interrupted` to the
   control this task must return at once. */
static ChimiControl *combinator_next(CombinatorChildren *children, bool drain, uint32_t *position,
                                     ChimiControl **interrupted, bool *child_cancelled,
                                     uint32_t call_node) {
  *child_cancelled = false;
  for (;;) {
    ChimiSchedulerRequest request = {
      .kind = drain ? CHIMI_REQUEST_TASK_DRAIN : CHIMI_REQUEST_TASK_ANY,
      .value = children->live,
      .context = children->waiting,
    };
    ChimiFiberCommand command = scheduler_request(&request);
    if (command == CHIMI_FIBER_CANCEL) { *interrupted = cancelled(); return NULL; }
    if (request.status == CHIMI_RESPONSE_PENDING) continue;
    uint32_t found = 0;
    while (found < children->live && children->waiting[found] != request.task) ++found;
    if (found == children->live || request.status == CHIMI_RESPONSE_ERROR ||
        request.status == CHIMI_RESPONSE_UNSET) {
      *interrupted = defect("Task combinator scheduler request failed", call_node);
      return NULL;
    }
    *position = children->positions[found];
    --children->live;
    children->waiting[found] = children->waiting[children->live];
    children->positions[found] = children->positions[children->live];
    switch (request.status) {
      case CHIMI_RESPONSE_SUCCEEDED: return task_reply(request.value);
      case CHIMI_RESPONSE_CANCELLED: *child_cancelled = true; return NULL;
      case CHIMI_RESPONSE_DEFECTED:
        if (request.value > 0) return task_reply(request.value);
        *interrupted = defect("Task combinator child defected", call_node);
        return NULL;
      case CHIMI_RESPONSE_STACK_OVERFLOW: return defect("Task stack overflow", call_node);
      default:
        *interrupted = defect("Task combinator scheduler request failed", call_node);
        return NULL;
    }
  }
}

/* `cause` with `sibling` appended to its suppressed causes, marked Sibling. */
static ChimiValue *cause_with_sibling(const ChimiValue *cause, const ChimiValue *sibling) {
  ChimiValue *marked = cause_copy(sibling);
  cause_set(marked, CAUSE_REASON, integer_value(CAUSE_REASON_SIBLING));
  ChimiValue *copy = cause_copy(cause);
  bool dropped = false;
  cause_set(copy, CAUSE_SUPPRESSED, cause_push_bounded(cause_field(cause, CAUSE_SUPPRESSED), marked,
                                                       CAUSE_SUPPRESSED_LIMIT, &dropped));
  if (dropped)
    cause_set(copy, CAUSE_DROPPED_SUPPRESSED,
              integer_value(cause_integer(cause, CAUSE_DROPPED_SUPPRESSED) + 1));
  return copy;
}

/* `Ok(value)`, or `Err(error)` keeping the failure's cause, as Result.capture
   builds them. */
static ChimiControl *stored_outcome(ChimiControl *outcome, uint32_t call_node) {
  bool failed = outcome->kind == CONTROL_FAILURE;
  ChimiValue *fields[] = {outcome->value};
  ChimiValue *stored = standard_value(failed ? CHIMI_CONSTRUCTOR_RESULT_ERR : CHIMI_CONSTRUCTOR_RESULT_OK,
                                      fields, 1);
  if (!stored) return defect("Result is missing from the program", call_node);
  return success(failed ? with_stored_cause(stored, control_cause(outcome)) : stored);
}

/* Cancels every child still live; false when this task was cancelled meanwhile. */
static bool cancel_live_children(const CombinatorChildren *children) {
  for (uint32_t index = 0; index < children->live; ++index) {
    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_TASK_CANCEL, .task = children->waiting[index]};
    if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return false;
  }
  return true;
}

static ChimiControl *run_combinator(ChimiRuntime *runtime, uint32_t intrinsic, ChimiValue *operations,
                                    uint64_t span, uint32_t call_node,
                                    ChimiEnvironment *environment) {
  uint32_t count = list_length(operations);
  if (count == 0) {
    if (intrinsic == CHIMI_INTRINSIC_TASK_RACE)
      return defect("Task.race needs at least one operation", call_node);
    return success(list_from_values(NULL, 0));
  }
  CombinatorChildren children = {
    .waiting = malloc(count * sizeof(uint64_t)),
    .positions = malloc(count * sizeof(uint32_t)),
  };
  if (!children.waiting || !children.positions) exhausted_memory();
  for (uint32_t index = 0; index < count; ++index) {
    ChimiValue *operation = list_get(operations, index);
    if (operation->kind != VALUE_CALLABLE) {
      combinator_children_free(&children);
      return intrinsic_shape_defect(call_node);
    }
    /* A child that cannot start ends this call; the ones already started are
       this task's children, which its unwinding cancels and drains. */
    ChimiControl *started = start_task(runtime, operation, span, call_node, environment);
    if (started->kind != CONTROL_SUCCESS) {
      combinator_children_free(&children);
      return started;
    }
    children.waiting[children.live] = started->value->as.task;
    children.positions[children.live] = index;
    ++children.live;
  }

  ChimiValue **values = new_value_slots(count);
  ChimiControl *decided = NULL;
  while (children.live > 0 && !decided) {
    uint32_t position = 0;
    ChimiControl *interrupted = NULL;
    bool child_cancelled = false;
    ChimiControl *outcome =
      combinator_next(&children, false, &position, &interrupted, &child_cancelled, call_node);
    if (interrupted) {
      /* A child defect reaches here only when this task absorbs it (a process
         body that is shutting down); the siblings stop now, not when the body
         ends. */
      cancel_live_children(&children);
      combinator_children_free(&children);
      return interrupted;
    }
    if (child_cancelled) {
      /* A child that ends cancelled cancels this call, as `Task.result` of it
         would: a scope or shutdown cancelled this task too, or the child ran
         `Task.result` on a task it cancelled itself. */
      decided = cancelled();
    } else if (outcome->kind != CONTROL_SUCCESS && outcome->kind != CONTROL_FAILURE) {
      decided = outcome;
    } else if (intrinsic == CHIMI_INTRINSIC_TASK_RESULTS) {
      ChimiControl *stored = stored_outcome(outcome, call_node);
      if (stored->kind != CONTROL_SUCCESS) decided = stored;
      else values[position] = stored->value;
    } else if (intrinsic == CHIMI_INTRINSIC_TASK_RACE || outcome->kind == CONTROL_FAILURE) {
      decided = outcome;
    } else {
      values[position] = outcome->value;
    }
  }

  if (children.live > 0) {
    if (!cancel_live_children(&children)) {
      combinator_children_free(&children);
      return cancelled();
    }
    bool collect_siblings = intrinsic == CHIMI_INTRINSIC_TASK_ALL && decided->kind == CONTROL_FAILURE;
    ChimiValue *cause = collect_siblings ? control_cause(decided) : NULL;
    bool suppressed = false;
    while (children.live > 0) {
      uint32_t position = 0;
      ChimiControl *interrupted = NULL;
      bool child_cancelled = false;
      ChimiControl *outcome =
        combinator_next(&children, true, &position, &interrupted, &child_cancelled, call_node);
      if (interrupted) {
        combinator_children_free(&children);
        return interrupted;
      }
      if (collect_siblings && outcome && outcome->kind == CONTROL_FAILURE) {
        cause = cause_with_sibling(cause, control_cause(outcome));
        suppressed = true;
      }
    }
    if (suppressed) decided = control_with_cause(decided, cause);
  }
  combinator_children_free(&children);
  if (decided) return decided;
  return success(list_from_values(values, count));
}

/* std/select (time-and-outcomes.md, "Choosing among events"). An Event is a record
   that is never shown to source code: `kind` is a SELECT_EVENT_* code, `source` the
   receiver, inbox, task, Duration, or Instant it waits on, `map` the mapping
   function (the value itself for a timer), and `clock` the provider a timer
   snapshotted when it was built, or Void for the execution clock. */
enum { SELECT_EVENT_RECEIVE = 1, SELECT_EVENT_INBOX, SELECT_EVENT_TASK, SELECT_EVENT_AFTER,
       SELECT_EVENT_AT };

static ChimiValue *select_event(int64_t kind, ChimiValue *source, ChimiValue *map, ChimiValue *clock) {
  ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = 4;
  value->as.record.fields = allocate(4, sizeof(*value->as.record.fields));
  allocation_kind(value->as.record.fields, ALLOCATION_FIELDS);
  value->as.record.fields[0] = (RuntimeField){"kind", integer_value(kind)};
  value->as.record.fields[1] = (RuntimeField){"source", source};
  value->as.record.fields[2] = (RuntimeField){"map", map};
  value->as.record.fields[3] = (RuntimeField){"clock", clock};
  return value;
}

static bool is_host(ChimiValue *value, HostFunction host) {
  return value && value->kind == VALUE_CALLABLE && value->as.callable->kind == CALL_HOST &&
         value->as.callable->host == host;
}

/* True when the provider's `now` and `sleep_until` are the execution clock's, so
   its timers go straight onto the scheduler's timer queue. `Clock.starting_at`
   shifts only `timestamp`, so it counts. */
static bool on_execution_clock(ChimiValue *provider) {
  if (!provider || provider->kind == VALUE_VOID) return true;
  if (provider->kind != VALUE_RECORD) return false;
  return is_host(select_value(provider, "now"), HOST_CLOCK_NOW) &&
         is_host(select_value(provider, "sleep_until"), HOST_CLOCK_SLEEP_UNTIL);
}

/* The deadline of a timer event and the `now` it is measured against: the
   execution clock's (read once per Select.first) or the event's own provider's.
   NULL on success. */
static ChimiControl *timer_deadline(ChimiRuntime *runtime, ChimiValue *event, int64_t *now_io,
                                    int64_t *deadline, uint64_t span, uint32_t call_node,
                                    ChimiEnvironment *environment) {
  ChimiValue *source = select_value(event, "source");
  ChimiValue *provider = select_value(event, "clock");
  int64_t now = *now_io;
  if (!on_execution_clock(provider)) {
    ChimiValue *read = select_value(provider, "now");
    if (!read || read->kind != VALUE_CALLABLE) return defect("provider has no callable field", call_node);
    ChimiControl *control = invoke(runtime, read->as.callable, NULL, 0, span, call_node, environment);
    if (control->kind != CONTROL_SUCCESS) return control;
    if (control->value->kind != VALUE_INSTANT) return intrinsic_shape_defect(call_node);
    now = control->value->as.integer;
  }
  *now_io = now;
  if (select_value(event, "kind")->as.integer == SELECT_EVENT_AT) {
    *deadline = source->as.integer;
    return NULL;
  }
  if (__builtin_add_overflow(now, source->as.integer, deadline)) {
    char text[32]; duration_text(source->as.integer, text);
    return defect(numeric_message("`Select.after(%s)` overflows the Instant range", text), call_node);
  }
  return NULL;
}

/* Starts the child that stands for a timer on a user Clock: it runs the provider's
   `sleep_until(deadline)`, and the select waits for it to end. */
static ChimiControl *start_user_timer(ChimiRuntime *runtime, ChimiValue *provider, int64_t deadline,
                                      uint64_t *task, uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  ChimiValue *sleep_until = select_value(provider, "sleep_until");
  if (!sleep_until || sleep_until->kind != VALUE_CALLABLE)
    return defect("provider has no callable field", call_node);
  ChimiValue *sleeper = clock_host_value(HOST_SELECT_SLEEP);
  sleeper->as.callable->inner = sleep_until->as.callable;
  sleeper->as.callable->shift = deadline;
  ChimiValue *arguments[] = {sleeper};
  ChimiControl *started = invoke_intrinsic(runtime, CHIMI_INTRINSIC_TASK_START, arguments, 1, span,
                                           call_node, environment);
  if (started->kind != CONTROL_SUCCESS) return started;
  *task = started->value->as.task;
  return NULL;
}

/* Cancels the user-clock timer children except `keep` (an index, or -1). */
static void cancel_user_timers(ChimiRuntime *runtime, const ChimiSelectSource *sources,
                               const bool *user_timer, uint32_t count, int64_t keep, uint64_t span,
                               uint32_t call_node, ChimiEnvironment *environment) {
  for (uint32_t index = 0; index < count; ++index) {
    if (!user_timer[index] || (int64_t)index == keep) continue;
    ChimiValue *arguments[] = {task_value(sources[index].task)};
    invoke_intrinsic(runtime, CHIMI_INTRINSIC_TASK_CANCEL, arguments, 1, span, call_node, environment);
  }
}

/* Takes the committed event's value: its source's own operation, which answers
   at once because the scheduler reported it ready, nothing else can consume it,
   and no other task runs before this request (the select's reply woke this task
   at the front of the run queue). Nothing may be issued between the select and
   this. A timer's value is its event value. */
static ChimiControl *select_take(ChimiRuntime *runtime, ChimiValue *event, uint64_t user_timer_task,
                                 uint64_t span, uint32_t call_node, ChimiEnvironment *environment) {
  int64_t kind = select_value(event, "kind")->as.integer;
  ChimiValue *map = select_value(event, "map");
  ChimiValue *source = select_value(event, "source");
  uint32_t operation;
  switch (kind) {
    case SELECT_EVENT_RECEIVE: operation = CHIMI_INTRINSIC_CHANNEL_RECEIVE; break;
    case SELECT_EVENT_INBOX: operation = CHIMI_INTRINSIC_PROCESS_RECEIVE; break;
    case SELECT_EVENT_TASK: operation = CHIMI_INTRINSIC_TASK_RESULT; break;
    default: {
      if (!user_timer_task) return success(map);
      /* Observe the timer child: a cancelled sleep cancels this task, as it would
         a direct `sleep_until`. */
      ChimiValue *arguments[] = {task_value(user_timer_task)};
      ChimiControl *slept = invoke_intrinsic(runtime, CHIMI_INTRINSIC_TASK_RESULT, arguments, 1,
                                             span, call_node, environment);
      return slept->kind == CONTROL_SUCCESS ? success(map) : slept;
    }
  }
  ChimiValue *arguments[] = {source};
  return invoke_intrinsic(runtime, operation, arguments, 1, span, call_node, environment);
}

/* Maps a taken value with the providers the mapping function snapshotted when
   the event was built. A timer has no mapping function. */
static ChimiControl *select_map(ChimiRuntime *runtime, ChimiValue *event, ChimiControl *taken,
                                uint64_t span, uint32_t call_node) {
  int64_t kind = select_value(event, "kind")->as.integer;
  if (taken->kind != CONTROL_SUCCESS || kind == SELECT_EVENT_AFTER || kind == SELECT_EVENT_AT)
    return taken;
  ChimiValue *mapped[] = {taken->value};
  return invoke(runtime, select_value(event, "map")->as.callable, mapped, 1, span, call_node,
                new_environment(0));
}

/* Select.first (time-and-outcomes.md, "Semantics of `Select.first`"): one
   CHIMI_REQUEST_SELECT over every event in list order, reissued after each wake,
   then the committed event. A user-Clock timer is a child running that clock's
   `sleep_until`, so it is a task source to the scheduler. */
static ChimiControl *select_first(ChimiRuntime *runtime, ChimiValue *events, uint64_t span,
                                  uint32_t call_node, ChimiEnvironment *environment) {
  uint32_t count = list_length(events);
  if (count == 0) return defect("`Select.first` needs at least one event", call_node);
  ChimiSelectSource *sources = calloc(count, sizeof(*sources));
  bool *user_timer = calloc(count, sizeof(*user_timer));
  if (!sources || !user_timer) exhausted_memory();
  ChimiControl *stopped = NULL;
  bool read_now = false;
  int64_t now = 0;
  for (uint32_t index = 0; index < count && !stopped; ++index) {
    ChimiValue *event = list_get(events, index);
    ChimiValue *source = select_value(event, "source");
    ChimiSelectSource *entry = &sources[index];
    switch (select_value(event, "kind")->as.integer) {
      case SELECT_EVENT_RECEIVE:
        *entry = (ChimiSelectSource){.kind = CHIMI_SELECT_RECEIVE,
                                     .channel = source->as.capability.channel,
                                     .owner = source->as.capability.owner,
                                     .token = source->as.capability.token};
        break;
      case SELECT_EVENT_INBOX: entry->kind = CHIMI_SELECT_INBOX; break;
      case SELECT_EVENT_TASK: *entry = (ChimiSelectSource){.kind = CHIMI_SELECT_TASK, .task = source->as.task}; break;
      default: {
        /* Every timer measures from the start of this call. */
        if (!read_now && (stopped = execution_now(&now, call_node))) break;
        read_now = true;
        int64_t deadline, measured = now;
        if ((stopped = timer_deadline(runtime, event, &measured, &deadline, span, call_node, environment)))
          break;
        ChimiValue *provider = select_value(event, "clock");
        if (on_execution_clock(provider)) {
          *entry = (ChimiSelectSource){.kind = CHIMI_SELECT_TIMER, .deadline = deadline};
        } else if (deadline <= measured) {
          /* Already due on its own clock: ready now, in list order. */
          *entry = (ChimiSelectSource){.kind = CHIMI_SELECT_TIMER, .deadline = INT64_MIN};
        } else {
          entry->kind = CHIMI_SELECT_TASK;
          stopped = start_user_timer(runtime, provider, deadline, &entry->task, span, call_node, environment);
          user_timer[index] = !stopped;
        }
      }
    }
  }
  int64_t committed = -1;
  while (!stopped) {
    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_SELECT, .context = sources, .value = count};
    if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) { stopped = cancelled(); break; }
    if (request.status == CHIMI_RESPONSE_PENDING) continue;
    if (request.status != CHIMI_RESPONSE_OK || request.value < 0 || request.value >= count) {
      stopped = defect("Select.first scheduler request failed", call_node);
      break;
    }
    committed = request.value;
    break;
  }
  ChimiValue *event = committed >= 0 ? list_get(events, (uint32_t)committed) : NULL;
  ChimiControl *taken = NULL;
  if (!stopped) {
    uint64_t timer_task = user_timer[committed] ? sources[committed].task : 0;
    taken = select_take(runtime, event, timer_task, span, call_node, environment);
  }
  /* Only after the take: a cancel reply lets other tasks run first. */
  if (!stopped || stopped->kind != CONTROL_CANCEL || !chimi_fiber_cancel_is_terminal())
    cancel_user_timers(runtime, sources, user_timer, count, committed, span, call_node, environment);
  free(sources);
  free(user_timer);
  if (stopped) return stopped;
  return select_map(runtime, event, taken, span, call_node);
}

static ChimiControl *invoke_select(ChimiRuntime *runtime, uint32_t intrinsic, ChimiValue **arguments,
                                   uint32_t argument_count, uint64_t span, uint32_t call_node,
                                   ChimiEnvironment *environment) {
  ValueKind source_kind;
  int64_t kind;
  switch (intrinsic) {
    case CHIMI_INTRINSIC_SELECT_RECEIVE: source_kind = VALUE_CHANNEL_RECEIVER; kind = SELECT_EVENT_RECEIVE; break;
    /* A watch subscription is a receiver: its event is a receive event. */
    case CHIMI_INTRINSIC_SELECT_WATCH: source_kind = VALUE_CHANNEL_RECEIVER; kind = SELECT_EVENT_RECEIVE; break;
    case CHIMI_INTRINSIC_SELECT_INBOX: source_kind = VALUE_PROCESS_INBOX; kind = SELECT_EVENT_INBOX; break;
    case CHIMI_INTRINSIC_SELECT_TASK: source_kind = VALUE_TASK; kind = SELECT_EVENT_TASK; break;
    case CHIMI_INTRINSIC_SELECT_AFTER: source_kind = VALUE_DURATION; kind = SELECT_EVENT_AFTER; break;
    case CHIMI_INTRINSIC_SELECT_AT: source_kind = VALUE_INSTANT; kind = SELECT_EVENT_AT; break;
    case CHIMI_INTRINSIC_SELECT_FIRST:
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST) return intrinsic_shape_defect(call_node);
      return select_first(runtime, arguments[0], span, call_node, environment);
    default: return defect("unknown standard intrinsic", call_node);
  }
  if (argument_count != 2 || arguments[0]->kind != source_kind) return intrinsic_shape_defect(call_node);
  bool timer = kind == SELECT_EVENT_AFTER || kind == SELECT_EVENT_AT;
  if (!timer && arguments[1]->kind != VALUE_CALLABLE) return intrinsic_shape_defect(call_node);
  ChimiValue *clock = NULL;
  if (timer) clock = environment->requirements[CHIMI_CLOCK_REQUIREMENT_SLOT];
  return success(select_event(kind, arguments[0], arguments[1], clock ? clock : void_value()));
}

extern char **environ;

/* The host core of the system modules (std/env); the interpreter's twin is
   execution/host_operations.rs. A host call checks cancellation before it enters
   the operating system. */
static ChimiControl *invoke_host_operation(ChimiRuntime *runtime, uint32_t intrinsic,
                                           uint32_t argument_count, uint32_t call_node) {
  if (argument_count != 0) return intrinsic_shape_defect(call_node);
  if (runtime->cancel) return cancelled();
  switch (intrinsic) {
    case CHIMI_INTRINSIC_ENV_HOST_VARIABLES: {
      /* environ in order; a pair whose name or value is not UTF-8 is left out. */
      ChimiValue *list = list_from_values(NULL, 0);
      for (char **entry = environ; entry && *entry; ++entry) {
        /* As Rust's vars_os: a name may start with '=', so the separator is
           the first '=' after the first byte. */
        const char *equals = **entry ? strchr(*entry + 1, '=') : NULL;
        if (!equals) continue;
        size_t name_length = (size_t)(equals - *entry), value_length = strlen(equals + 1);
        if (!valid_utf8((const unsigned char *)*entry, name_length) ||
            !valid_utf8((const unsigned char *)equals + 1, value_length))
          continue;
        ChimiValue *pair = new_value(VALUE_TUPLE);
        pair->as.record.length = 2;
        pair->as.record.fields = allocate(2, sizeof(*pair->as.record.fields));
        allocation_kind(pair->as.record.fields, ALLOCATION_FIELDS);
        pair->as.record.fields[0] = (RuntimeField){NULL, text_from_bytes(*entry, name_length)};
        pair->as.record.fields[1] = (RuntimeField){NULL, text_from_bytes(equals + 1, value_length)};
        list = list_push(list, pair);
      }
      return success(list);
    }
    case CHIMI_INTRINSIC_ENV_HOST_CURRENT_DIRECTORY: {
      size_t capacity = 256;
      for (;;) {
        char *buffer = malloc(capacity);
        if (!buffer) return defect("out of memory reading the working directory", call_node);
        if (getcwd(buffer, capacity)) {
          ChimiValue *path = NULL;
          if (valid_utf8((const unsigned char *)buffer, strlen(buffer))) path = text_value(buffer);
          free(buffer);
          if (!path) return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
          ChimiValue *fields[] = {path};
          return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
        }
        free(buffer);
        if (errno != ERANGE) return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
        capacity *= 2;
      }
    }
    default: return intrinsic_shape_defect(call_node);
  }
}

static ChimiControl *invoke_intrinsic(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  if ((intrinsic >= CHIMI_INTRINSIC_MAP_REMOVE && intrinsic <= CHIMI_INTRINSIC_MAP_REDUCE_ENTRIES) ||
      (intrinsic >= CHIMI_INTRINSIC_MAP_EMPTY && intrinsic <= CHIMI_INTRINSIC_MAP_PUT))
    return invoke_map_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_SET_EMPTY && intrinsic <= CHIMI_INTRINSIC_SET_REDUCE_ITEMS)
    return invoke_set_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_JSON_DECODE && intrinsic <= CHIMI_INTRINSIC_JSON_NUMBER_TO_DECIMAL)
    return invoke_json_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_DECIMAL_SCALED && intrinsic <= CHIMI_INTRINSIC_DECIMAL_TO_FLOAT)
    return invoke_decimal_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_BYTES_FROM && intrinsic <= CHIMI_INTRINSIC_EMBED_BYTES)
    return invoke_bytes_operation(intrinsic, arguments, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_CAUSE_CONTEXT && intrinsic <= CHIMI_INTRINSIC_CAUSE_TO_TEXT)
    return invoke_cause_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_TEST_ASSERT_EQUAL && intrinsic <= CHIMI_INTRINSIC_TEST_FAIL)
    return invoke_assertion(intrinsic, arguments, argument_count, call_node);
  if (intrinsic == CHIMI_INTRINSIC_RESOURCE_SCOPED)
    return resource_scoped(runtime, arguments, argument_count, span, call_node, environment);
  if (intrinsic == CHIMI_INTRINSIC_TEST_CAPTURE_DEFECT)
    return capture_defect(runtime, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_ENV_HOST_VARIABLES && intrinsic <= CHIMI_INTRINSIC_ENV_HOST_CURRENT_DIRECTORY)
    return invoke_host_operation(runtime, intrinsic, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_DURATION_NANOSECONDS && intrinsic <= CHIMI_INTRINSIC_DURATION_PARSE)
    return invoke_duration(intrinsic, arguments, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_PROCESS_SPEC && intrinsic <= CHIMI_INTRINSIC_SUPERVISOR_WAIT)
    return invoke_process(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if ((intrinsic >= CHIMI_INTRINSIC_SELECT_RECEIVE && intrinsic <= CHIMI_INTRINSIC_SELECT_FIRST) ||
      intrinsic == CHIMI_INTRINSIC_SELECT_WATCH)
    return invoke_select(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_CLOCK_NOW && intrinsic <= CHIMI_INTRINSIC_TASK_DEADLINE)
    return invoke_clock(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_DEBUG_CRASH && intrinsic <= CHIMI_INTRINSIC_LIST_SET)
    return invoke_core_operation(intrinsic, arguments, argument_count, call_node);
  switch (intrinsic) {
    case CHIMI_INTRINSIC_IO_PATH:
    case CHIMI_INTRINSIC_PATH_FROM:
    case CHIMI_INTRINSIC_PATH_TO_TEXT:
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
    case CHIMI_INTRINSIC_TEXT_APPEND: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT ||
          arguments[1]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      TextBuffer buffer = {0};
      buffer_bytes(&buffer, arguments[0]->as.text.bytes, arguments[0]->as.text.length);
      buffer_bytes(&buffer, arguments[1]->as.text.bytes, arguments[1]->as.text.length);
      return success(text_from_bytes(buffer.data ? buffer.data : "", buffer.length));
    }
    case CHIMI_INTRINSIC_TEXT_EQUAL:
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT ||
          arguments[1]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      return success(boolean_value(text_equals(arguments[0], arguments[1]->as.text.bytes, arguments[1]->as.text.length)));
    case CHIMI_INTRINSIC_INT_PARSE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      int64_t value = 0; uint32_t error = text_holds_nul(arguments[0]) ? CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT
                                                                       : parse_int_text(arguments[0]->as.text.bytes, &value);
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
      double value = 0; uint32_t error = text_holds_nul(arguments[0]) ? CHIMI_CONSTRUCTOR_FLOAT_PARSE_INVALID
                                                                      : parse_float_text(arguments[0]->as.text.bytes, &value);
      return error ? failure(standard_value(error, NULL, 0)) : success(float_value(value));
    }
    case CHIMI_INTRINSIC_TASK_START:
      if (argument_count != 1 || arguments[0]->kind != VALUE_CALLABLE)
        return intrinsic_shape_defect(call_node);
      return start_task(runtime, arguments[0], span, call_node, environment);
    case CHIMI_INTRINSIC_TASK_ALL:
    case CHIMI_INTRINSIC_TASK_RESULTS:
    case CHIMI_INTRINSIC_TASK_RACE:
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST)
        return intrinsic_shape_defect(call_node);
      return run_combinator(runtime, intrinsic, arguments[0], span, call_node, environment);
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
    case CHIMI_INTRINSIC_CHANNEL_ADD_RECEIVER: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CHANNEL_RECEIVER)
        return intrinsic_shape_defect(call_node);
      RuntimeCapability capability = arguments[0]->as.capability;
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_ADD_RECEIVER,
        .channel = capability.channel,
        .owner = capability.owner,
        .token = capability.token,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(channel_capability_value(VALUE_CHANNEL_RECEIVER, request.channel,
                                           request.owner, request.token))
        : defect("Channel.add_receiver scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_BROADCAST_SUBSCRIBE:
    case CHIMI_INTRINSIC_WATCH_SUBSCRIBE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_CHANNEL_SENDER)
        return intrinsic_shape_defect(call_node);
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_SUBSCRIBE,
        .channel = arguments[0]->as.capability.channel,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(channel_capability_value(VALUE_CHANNEL_RECEIVER, request.channel,
                                           request.owner, request.token))
        : defect("subscribe scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_BROADCAST_BOUNDED: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER)
        return intrinsic_shape_defect(call_node);
      if (arguments[0]->as.integer < 1)
        return defect(numeric_message("`Broadcast.bounded(%" PRId64 ")` needs a positive capacity",
                                      arguments[0]->as.integer),
                      call_node);
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_BROADCAST,
        .value = arguments[0]->as.integer,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(publisher_bundle_value(request.channel, request.owner, request.token))
        : defect("Broadcast.bounded scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_WATCH_STARTING_WITH: {
      if (argument_count != 1) return intrinsic_shape_defect(call_node);
      void *roots[] = {arguments[0]};
      AllocationKind kinds[] = {ALLOCATION_VALUE};
      RuntimeFragment *initial = NULL;
      if (fragment_copy(roots, kinds, 1, &initial) != HEAP_OK)
        return defect(FOREIGN_POINTER_DEFECT, call_node);
      CapabilityTransfer carried = {.collect = true};
      transfer_captures(&carried, initial->roots[0]);
      size_t carried_count = carried.carried_count;
      free(carried.carried);
      if (carried_count) {
        fragment_free(initial);
        return defect("channel capability violation: Watch.starting_with cannot hold a value "
                      "with a Receiver or Completion, because every subscriber gets a copy",
                      call_node);
      }
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_CHANNEL_WATCH,
        .value = (int64_t)(intptr_t)initial,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      /* The scheduler takes the initial value only with an OK reply. */
      if (request.status != CHIMI_RESPONSE_OK) fragment_free(initial);
      if (command == CHIMI_FIBER_CANCEL) return cancelled();
      return request.status == CHIMI_RESPONSE_OK
        ? success(publisher_bundle_value(request.channel, request.owner, request.token))
        : defect("Watch.starting_with scheduler request failed", call_node);
    }
    case CHIMI_INTRINSIC_BROADCAST_PUBLISH:
    case CHIMI_INTRINSIC_WATCH_PUBLISH:
    /* A publisher is a Sender of its broadcast or watch: the scheduler
       publishes a send to one, and defects the task if the message carries a
       Receiver or Completion, which every subscriber's copy would duplicate. */
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
      /* Receivers and Completions in the message travel with it: the scheduler
         puts them in flight, and the message's copies take the in-flight token
         before anything can receive them. */
      CapabilityTransfer carried = {.collect = true};
      transfer_captures(&carried, message->roots[0]);
      ChimiMessageCapabilities capabilities = {carried.carried, carried.carried_count};
      bool first = true;
      for (;;) {
        ChimiSchedulerRequest request = {
          .kind = CHIMI_REQUEST_CHANNEL_SEND,
          .channel = arguments[0]->as.capability.channel,
          .value = (int64_t)(intptr_t)message,
          .context = first && capabilities.count ? &capabilities : NULL,
        };
        first = false;
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
        free(carried.carried);
        carried.carried = NULL;
        if (command == CHIMI_FIBER_CANCEL) return cancelled();
        if (request.status == CHIMI_RESPONSE_PENDING) continue;
        if (request.status == CHIMI_RESPONSE_OK)
          return success(intrinsic == CHIMI_INTRINSIC_BROADCAST_PUBLISH
                           ? integer_value(request.value)
                           : void_value());
        ChimiValue *fields[] = {arguments[1]};
        if (request.status == CHIMI_RESPONSE_CLOSED)
          return failure(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_SEND_CLOSED, fields, 1));
        if (request.status == CHIMI_RESPONSE_ABORTED)
          return failure(standard_value(CHIMI_CONSTRUCTOR_CHANNEL_SEND_ABORTED, fields, 1));
        return defect("Channel.send scheduler request failed", call_node);
      }
    }
    /* A watch subscription is a receiver of a queue that keeps only the
       latest value. */
    case CHIMI_INTRINSIC_WATCH_RECEIVE:
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
      if (environment->requirements[CHIMI_CLOCK_REQUIREMENT_SLOT]) {
        /* A bound provider: `sleep_until(add(now(), d))` through its methods. */
        ChimiControl *read = clock_method(runtime, environment, "now", NULL, 0, span, call_node);
        if (read->kind != CONTROL_SUCCESS) return read;
        if (read->value->kind != VALUE_INSTANT) return intrinsic_shape_defect(call_node);
        int64_t until;
        if (__builtin_add_overflow(read->value->as.integer, arguments[0]->as.integer, &until)) {
          char duration[32];
          duration_text(arguments[0]->as.integer, duration);
          return defect(numeric_message("`Clock.sleep(%s)` overflows the Instant range", duration),
                        call_node);
        }
        ChimiValue *at[] = {instant_value(until)};
        return clock_method(runtime, environment, "sleep_until", at, 1, span, call_node);
      }
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
    if (strcmp(name, "clock") == 0)
      environment->requirements[CHIMI_CLOCK_REQUIREMENT_SLOT] = constant_values[pairs[2 * i + 1]];
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
    case CHIMI_CONSTANT_TEXT: return string_text(raw->a);
    case CHIMI_CONSTANT_BYTES: return bytes_from_hex(CHIMI_STRINGS[raw->a], CHIMI_STRING_LENGTHS[raw->a]);
    case CHIMI_CONSTANT_BOOLEAN: return boolean_value(raw->integer != 0);
    case CHIMI_CONSTANT_JSON_NUMBER: return json_number_value(CHIMI_STRINGS[raw->a]);
    case CHIMI_CONSTANT_LIST: {
      ChimiValue **items = allocate(raw->length, sizeof(*items));
      for (uint32_t i = 0; i < raw->length; ++i) items[i] = constant_values[ids[i]];
      return list_from_values(items, raw->length);
    }
    case CHIMI_CONSTANT_DURATION: return duration_value(raw->integer);
    case CHIMI_CONSTANT_DECIMAL: {
      uint64_t high = ((uint64_t)raw->offset << 32) | raw->length;
      return decimal_value((Decimal){(raw->a & 0x100) != 0, ((DecimalMagnitude)high << 64) | (uint64_t)raw->integer, raw->a & 0xff});
    }
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
    case CHIMI_CONSTANT_SET: {
      MapNode *set = NULL;
      for (uint32_t i = 0; i < raw->length; ++i)
        set = map_put_node(set, constant_values[ids[i]], void_value());
      return set_value(set);
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
      callable->local_count = function->local_count;
      callable->environment = capture_requirements(new_environment(0)); callable->name = "function";
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
      ChimiEnvironment *environment = new_environment(node->d);
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
      callable->local_count = function->local_count;
      callable->environment = capture_requirements(environment); callable->name = "function";
      return success(callable_value(callable));
    }
    case CHIMI_SYMBOL_CONSTANT:
      return success(constant_values[shape->constant]);
    case CHIMI_SYMBOL_REQUIREMENT:
      if (environment->requirements[shape->environment_slot])
        return success(environment->requirements[shape->environment_slot]);
      /* The runner closes `clock: Clock` with the execution clock provider. */
      if (strcmp(CHIMI_STRINGS[shape->name], "clock") == 0) return success(execution_clock_value());
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
    uint32_t profile_outer = profile_enter(node_id, OPERATION); \
    uint64_t span = begin_node(runtime, node_id, parent, OPERATION); ChimiEnvironment *environment = incoming; \
    (void)environment; \
    ChimiControl *result = NULL; do { __VA_ARGS__ } while (0); \
    result = finish_node(runtime, node_id, parent, span, OPERATION, result); \
    profile_leave(profile_outer); return result; \
  }
#define RUN(CHILD) run_node((CHILD), runtime, environment, span)
#define REQUIRE_SUCCESS(CONTROL, TARGET) do { ChimiControl *_control = (CONTROL); \
  if (_control->kind != CONTROL_SUCCESS) { result = _control; break; } (TARGET) = _control->value; } while (0)

NODE_WRAPPER(constant, "constant", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value;
  if (node->a == CHIMI_LITERAL_INTEGER) value = integer_value(node->integer);
  else if (node->a == CHIMI_LITERAL_FLOAT) {
    double raw; memcpy(&raw, &node->integer, sizeof(raw)); value = float_value(raw);
  } else if (node->a == CHIMI_LITERAL_TEXT) value = string_text(node->b);
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
    else if (standard_type == CHIMI_TYPE_CLOCK && node->c <= 2)
      inner->host = node->c == 0 ? HOST_CLOCK_NOW : node->c == 1 ? HOST_CLOCK_TIMESTAMP : HOST_CLOCK_SLEEP_UNTIL;
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
  if (result->kind == CONTROL_FAILURE) result->node = node_id;
  /* A Failure(...) or Result.Err(...) in a cause arm: `b` is the arm's cause
     local plus one, and `c` says whether it keeps that cause. */
  if (node->b == 0) break;
  ChimiValue *matched = environment->locals[environment_slot(node->b - 1)];
  if (!matched || matched->kind != VALUE_CAUSE) matched = NULL;
  if (result->kind == CONTROL_FAILURE) {
    ChimiValue *cause = cause_derived(matched, node->c != 0, result->value, node_id);
    if (cause) result = control_with_cause(result, cause);
  } else if (result->kind == CONTROL_SUCCESS && result->value->kind == VALUE_CONSTRUCTED &&
             result->value->as.constructed.length == 1) {
    ChimiValue *cause = cause_derived(matched, node->c != 0,
                                      result->value->as.constructed.fields[0].value, node_id);
    if (cause) result = success(with_stored_cause(result->value, cause));
  }
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
  if (stopped) break;
  result = invoke(runtime, callee->value->as.callable, arguments, node->length, span, node_id, environment);
  /* A failure an operation raised without a source construction starts at its call. */
  if (result->kind == CONTROL_FAILURE && !result->cause && result->node < 0) {
    result = new_control(CONTROL_FAILURE, result->value, NULL, node_id);
  }
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
  if (control->value->kind == VALUE_DECIMAL) { result = success(decimal_value(decimal_negate(decimal_of(control->value)))); break; }
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
    else if (left->value->kind == VALUE_DECIMAL && right->value->kind == VALUE_DECIMAL)
      result = decimal_arithmetic(node->a, decimal_of(left->value), decimal_of(right->value), node_id);
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
    if (strcmp(name, "clock") == 0) environment->requirements[CHIMI_CLOCK_REQUIREMENT_SLOT] = control->value;
  } if (!stopped) result = RUN(node->a);
})

NODE_WRAPPER(sequence, "sequence", {
  /* The locals are cloned at the first binding step, so a block that binds nothing
     (a loop body such as `{ case ... }`) allocates no environment. Steps before that
     one read the incoming locals, which the clone would have copied unchanged. */
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *last = NULL; bool stopped = false;
  for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiStep *step = &CHIMI_STEPS[node->offset + i]; ChimiControl *control = RUN(step->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    last = control->value;
    if (step->kind == 0) {
      if (environment == incoming) environment = clone_local_environment(incoming);
      environment->locals[environment_slot(step->symbol)] = last;
    }
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
/* `control` is the subject's outcome, which holds a failure's cause. */
typedef struct { MatchKind kind; ChimiValue *value; ChimiControl *control; } MatchValue;

static bool literal_matches(const ChimiPattern *pattern, const ChimiValue *value) {
  if (pattern->a == CHIMI_LITERAL_INTEGER) return value->kind == VALUE_INTEGER && value->as.integer == pattern->integer;
  if (pattern->a == CHIMI_LITERAL_TEXT)
    return value->kind == VALUE_TEXT && text_equals(value, CHIMI_STRINGS[pattern->offset], CHIMI_STRING_LENGTHS[pattern->offset]);
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
  /* Most matches have one or two subjects, so their values live on the C stack. */
  enum { MATCH_STACK_SUBJECTS = 8 };
  const ChimiNode *node = &CHIMI_NODES[node_id]; MatchValue stack_values[MATCH_STACK_SUBJECTS];
  MatchValue *values = node->length <= MATCH_STACK_SUBJECTS ? stack_values
                                                              : allocate(node->length, sizeof(*values));
  bool stopped = false;
  for (uint32_t subject = 0; subject < node->length; ++subject) {
    bool contextual = false;
    for (uint32_t arm_index = 0; arm_index < node->b; ++arm_index) {
      const ChimiArm *arm = &CHIMI_ARMS[node->a + arm_index];
      if (CHIMI_PATTERNS[CHIMI_IDS[arm->patterns_offset + subject]].kind == CHIMI_PATTERN_OUTCOME) contextual = true;
    }
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + subject]);
    if (contextual && control->kind == CONTROL_SUCCESS) values[subject] = (MatchValue){MATCH_SUCCESS, control->value, control};
    else if (contextual && control->kind == CONTROL_FAILURE) values[subject] = (MatchValue){MATCH_FAILURE, control->value, control};
    else if (!contextual && control->kind == CONTROL_SUCCESS) values[subject] = (MatchValue){MATCH_PLAIN, control->value, control};
    else { result = control; stopped = true; break; }
  }
  if (stopped) break;
  for (uint32_t arm_index = 0; arm_index < node->b; ++arm_index) {
    const ChimiArm *arm = &CHIMI_ARMS[node->a + arm_index]; bool matched = true;
    for (uint32_t subject = 0; subject < node->length; ++subject)
      if (!match_value(CHIMI_IDS[arm->patterns_offset + subject], values[subject], NULL)) { matched = false; break; }
    if (!matched) continue;
    bool binds = arm->cause_local != UINT32_MAX;
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
    if (arm->cause_local != UINT32_MAX)
      arm_environment->locals[environment_slot(arm->cause_local)] =
        matched_cause_value(values[arm->cause_subject].control);
    result = run_node(arm->body, runtime, arm_environment, span); stopped = true; break;
  }
  if (!stopped) {
    for (uint32_t i = 0; i < node->length; ++i) if (values[i].kind == MATCH_FAILURE) { result = values[i].control; stopped = true; break; }
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
static ChimiControl *run_loop_passes(ChimiRuntime *runtime, uint32_t node_id,
                                     ChimiEnvironment *incoming, uint64_t span, ChimiValue *state) {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  ChimiEnvironment *environment = clone_local_environment(incoming);
  uint32_t state_slot = environment_slot(node->a);
  ChimiControl *result = NULL;
  AllocationScope passes;
  allocation_scope_begin(&passes);
  uint32_t polls = 0;
  for (;;) {
    if (runtime->cancel) { result = cancelled(); break; }
    if (CHIMI_REQUIRES_SCHEDULER &&
        (chimi_fiber_scope_cancel_pending() ||
         (chimi_fiber_scope_polls() && (++polls & 1023u) == 0))) {
      /* A fired timeout scope, or one whose real-clock deadline may have
         passed while this loop never parked: a loop pass is a cancellation
         point. */
      ChimiSchedulerRequest check = {.kind = CHIMI_REQUEST_SCOPE_CHECK};
      if (scheduler_request(&check) == CHIMI_FIBER_CANCEL) { result = cancelled(); break; }
    }
    environment->locals[state_slot] = state;
    ChimiControl *pass = RUN(node->c);
    if (pass->kind == CONTROL_SUCCESS) {
      state = pass->value;
      if (allocation_scope_collect(&passes, &state, NULL) != HEAP_OK) {
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
  return end_measured_allocation_scope(runtime, &passes, result);
}

NODE_WRAPPER(loop, "loop", {
  ChimiControl *initial = RUN(CHIMI_NODES[node_id].b);
  if (initial->kind != CONTROL_SUCCESS) { result = initial; break; }
  result = run_loop_passes(runtime, node_id, incoming, span, initial->value);
})

/* A loop whose state is an Int and whose body is integer arithmetic and matches over
   literals (CHI-90). The compiler emits one pass as `pass`, which returns
   CHIMI_DIRECT_PASS_CONTINUE with the next state, CHIMI_DIRECT_PASS_BREAK with the
   loop's value, or CHIMI_DIRECT_PASS_FALLBACK when the pass would defect (overflow,
   division by zero, no matching arm). A pass that falls back is rerun from the same
   state by the runtime loop, which reports the defect exactly as it would have. The
   passes allocate nothing, and cancellation is checked before each one, as the
   runtime loop does. Only trace-off, unprofiled runs take this path: tracing keeps
   every node's span and profiling counts every node, so both run the runtime loop
   from the start. */
ChimiControl *chimi_rt_direct_loop(ChimiRuntime *runtime, uint32_t node_id,
                                   ChimiEnvironment *incoming, uint64_t parent,
                                   ChimiDirectPass pass) {
  if (!runtime->silent || profile_rows) return chimi_rt_loop(runtime, node_id, incoming, parent);
  ChimiControl *initial = run_node(CHIMI_NODES[node_id].b, runtime, incoming, parent);
  if (initial->kind != CONTROL_SUCCESS) return initial;
  /* A body that never reads its state (`loop s = "x" { break 5 }`) lowers whatever
     the state's type, so a state that is not an Int runs on the runtime loop. */
  if (initial->value->kind != VALUE_INTEGER)
    return run_loop_passes(runtime, node_id, incoming, parent, initial->value);
  int64_t state = initial->value->as.integer;
  for (;;) {
    if (runtime->cancel) return cancelled();
    int64_t next = 0;
    switch (pass(state, &next)) {
      case CHIMI_DIRECT_PASS_CONTINUE: state = next; continue;
      case CHIMI_DIRECT_PASS_BREAK: return success(integer_value(next));
      default: return run_loop_passes(runtime, node_id, incoming, parent, integer_value(state));
    }
  }
}
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
      fputs("\"outcome\":\"expected_failure\",\"error\":", output); json_value(output, control->value);
      /* A test build also writes the debug rendering `chimi test` reports. */
      if (CHIMI_TEST_COUNT > 0) {
        ChimiValue *rendered = debug_render(control->value, true);
        fputs(",\"rendering\":", output);
        json_string(output, rendered->as.text.bytes);
      }
      break;
    case CONTROL_DEFECT:
      fputs("\"outcome\":\"defect\",\"message\":", output); json_string(output, control->message);
      if (control->node < 0) fputs(",\"core_node_id\":null,\"origin_id\":null", output);
      else fprintf(output, ",\"core_node_id\":%" PRId64 ",\"origin_id\":%u",
                   control->node, CHIMI_NODES[control->node].origin);
      break;
    case CONTROL_CANCEL:
      fputs("\"outcome\":\"cancellation\",\"reason\":", output); json_string(output, CANCELLATION_REASON);
      break;
    case CONTROL_BREAK: break;
  }
  ChimiValue *cause = control->kind == CONTROL_SUCCESS ? NULL : control_cause(control);
  if (cause && cause_has_detail(cause)) {
    fputs(",\"cause\":", output);
    ChimiValue *text = cause_text(cause);
    json_bytes(output, text->as.text.bytes, text->as.text.length);
  }
  fputs("}\n", output);
}

static bool close_output(FILE *output) {
  if (output == NULL) return false;
  bool failed = fflush(output) != 0;
  if (fclose(output) != 0) failed = true;
  return failed;
}

static int compare_profile_rows(const void *left, const void *right) {
  uint32_t a = *(const uint32_t *)left, b = *(const uint32_t *)right;
  const ProfileRow *x = &profile_rows[a], *y = &profile_rows[b];
  if (x->allocated_bytes != y->allocated_bytes) return x->allocated_bytes > y->allocated_bytes ? -1 : 1;
  if (x->evaluations != y->evaluations) return x->evaluations > y->evaluations ? -1 : 1;
  return a < b ? -1 : a > b;
}

/* One JSON object: every evaluated node, most allocated bytes first, then most
   evaluations, then node id, so the same program always writes the same profile. */
static bool write_profile(const char *path) {
  if (!path) return false;
  FILE *output = fopen(path, "wx");
  if (!output) {
    fprintf(stderr, "cannot open profile file %s: %s\n", path, strerror(errno));
    return true;
  }
  uint32_t *order = calloc(CHIMI_NODE_COUNT ? CHIMI_NODE_COUNT : 1, sizeof(*order));
  if (!order) exhausted_memory();
  uint32_t count = 0;
  for (uint32_t node = 0; node < CHIMI_NODE_COUNT; ++node)
    if (profile_rows[node].evaluations) order[count++] = node;
  qsort(order, count, sizeof(*order), compare_profile_rows);
  fputs("{\"schema\":\"chimi.runtime-profile.v1\",\"operations\":[", output);
  for (uint32_t index = 0; index < count; ++index) {
    uint32_t node = order[index];
    const ProfileRow *row = &profile_rows[node];
    const ChimiOrigin *origin = &CHIMI_ORIGINS[CHIMI_NODES[node].origin];
    fprintf(output,
            "%s{\"core_node_id\":%u,\"origin_id\":%u,\"file_id\":%u,\"source_start\":%u,"
            "\"source_end\":%u,\"operation\":",
            index ? "," : "", node, CHIMI_NODES[node].origin, origin->file_id, origin->start, origin->end);
    json_string(output, row->operation);
    fprintf(output, ",\"evaluations\":%" PRIu64 ",\"allocations\":%" PRIu64
            ",\"allocated_bytes\":%" PRIu64 "}", row->evaluations, row->allocations, row->allocated_bytes);
  }
  fputs("]}\n", output);
  free(order);
  return close_output(output);
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
          ",\"trace_events_constructed\":%" PRIu64 ",\"memberships_live\":%zu}\n",
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
          footprint_peak(),
          heap_totals.peak_fragment_footprint_bytes,
          capture_transfer_probes,
          /* Reported per allocation: young chunks are not counted separately. */
          heap_totals.live_allocations,
          footprint_peak(),
          heap_totals.live_fragments,
          heap_totals.peak_fragment_footprint_bytes,
#if CHIMI_PROCESS_KERNEL
          process_heap_peak_value_bytes,
#else
          (size_t)0,
#endif
          copied_values,
          copied_bytes,
          runtime->trace_events_constructed,
#if CHIMI_PROCESS_KERNEL
          chimi_scheduler_live_memberships());
#else
          (size_t)0);
#endif
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
    "clock_timer_cancelled", "clock_deadlock_waiter", "clock_scope_fired",
    "clock_select_parked", "clock_select_committed", "clock_signal_received",
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
static void emit_process_fields(const ChimiRuntime *runtime, FILE *output,
                                const ChimiSchedulerTraceEvent *event, bool attributes) {
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
  const char *named = NULL;
  if (event->kind == CHIMI_PROCESS_TRACE_TERMINATED && event->cause == CHIMI_PROCESS_CAUSE_DEFECTED) {
    if (event->outcome == CHIMI_PROCESS_DEFECT_STACK_OVERFLOW) named = "Process stack overflow";
    if (!named && runtime->last_defect_incarnation == event->incarnation)
      named = runtime->last_defect_message;
  }
  json_word(output, "defect", named);
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
  if (process) emit_process_fields(runtime, output, event, false);
  fprintf(output,
          ",\"attributes\":{\"scheduler_sequence\":%" PRIu64
          ",\"capacity\":%" PRIu64 ",\"queue_depth\":%" PRIu64
          ",\"waiter_depth\":%" PRIu64 ",\"discard_count\":%" PRIu64,
          event->sequence, event->capacity, event->queue_depth,
          event->waiter_depth, event->discard_count);
  if (clock && event->kind == CHIMI_CLOCK_TRACE_SELECT_COMMITTED)
    fprintf(output, ",\"time_ns\":%" PRId64 ",\"index\":%" PRIu64, event->time, event->cause);
  else if (clock)
    fprintf(output, ",\"time_ns\":%" PRId64 ",\"request_kind\":%" PRIu64,
            event->time, event->cause);
  if (process) emit_process_fields(runtime, output, event, true);
  fputs("}}\n", output);
}

/* The operation a parked task waits on, by its scheduler request kind. */
static const char *waited_operation(uint32_t request_kind) {
  switch (request_kind) {
    case CHIMI_REQUEST_TASK_RESULT: return "Task.result";
    case CHIMI_REQUEST_TASK_ANY: return "Task.all, Task.results or Task.race";
    case CHIMI_REQUEST_TASK_DRAIN: return "draining a Task combinator's children";
    case CHIMI_REQUEST_CHANNEL_SEND: return "Channel.send";
    case CHIMI_REQUEST_CHANNEL_RECEIVE: return "Channel.receive";
    case CHIMI_REQUEST_SELECT: return "Select.first";
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
    case CHIMI_REQUEST_CHANNEL_ADD_RECEIVER: return "Channel.add_receiver";
    case CHIMI_REQUEST_CHANNEL_SUBSCRIBE: return "subscribe";
    case CHIMI_REQUEST_PROCESS_SEND: return "Process.send";
    case CHIMI_REQUEST_SUPERVISOR_START_PROCESS: return "Supervisor.start_handoff";
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
     another copy has replaced it. The owner may already have closed or aborted
     the current copy, so the message names ownership, not a live copy. */
  else if (violation->owner == violation->task)
    snprintf(holder, sizeof(holder), "this copy is stale; task %" PRIu64
             " owns the capability", violation->task);
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
    case CHIMI_VIOLATION_IN_FLIGHT:
      snprintf(text, sizeof(text),
               "channel capability violation: task %" PRIu64 " cannot %s the %s of channel %" PRIu64
               "%s%s: this copy is stale; the capability was sent in a message no task has "
               "received yet", violation->task, transfer ? "hand" : "use",
               capability_name(violation->capability), violation->channel,
               transfer ? "" : " in ", transfer ? "" : channel_operation(violation->request_kind));
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
    case CHIMI_VIOLATION_PUBLISHED_CAPABILITY:
      snprintf(text, sizeof(text),
               "channel capability violation: task %" PRIu64 " cannot publish a value holding a "
               "Receiver or Completion on channel %" PRIu64 ", because every subscriber gets a copy",
               violation->task, violation->channel);
      break;
    case CHIMI_VIOLATION_WRONG_KIND:
      snprintf(text, sizeof(text),
               "channel capability violation: task %" PRIu64 " cannot use channel %" PRIu64
               " in %s: that operation does not apply to this kind of channel",
               violation->task, violation->channel, channel_operation(violation->request_kind));
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

#if CHIMI_PROCESS_KERNEL
/* Keeps the latest defected incarnation and its message: the body's own defect,
   remembered when it ended, or a stack overflow the scheduler saw. */
static void record_process_defect(void *context, const ChimiProcessTermination *termination) {
  ChimiRuntime *runtime = context;
  char *message = take_process_defect(termination->incarnation);
  if (termination->stack_overflow) {
    free(message);
    message = strdup("Process stack overflow");
    if (!message) exhausted_memory();
  }
  free(runtime->last_defect_message);
  runtime->last_defect_message = message;
  runtime->last_defect_incarnation = termination->incarnation;
  runtime->last_defect_membership = termination->membership;
  ++runtime->process_defect_count;
}
#endif

/* `message`, followed by the latest process defect when one happened: a root that
   cannot finish is often waiting on a process that defected, and with tracing off
   nothing else says so. The result belongs to the defect it is passed to. */
static const char *with_process_defect(ChimiRuntime *runtime, const char *message) {
  if (!runtime->process_defect_count) return message;
  const char *defect = runtime->last_defect_message ? runtime->last_defect_message : "defect";
  char count[64] = "";
  if (runtime->process_defect_count > 1)
    snprintf(count, sizeof(count), " (the last of %" PRIu64 " process defects)",
             runtime->process_defect_count);
  size_t capacity = strlen(message) + strlen(defect) + 160;
  char *combined = malloc(capacity);
  if (!combined) exhausted_memory();
  snprintf(combined, capacity, "%s; process incarnation %" PRIu64 " of membership %" PRIu64
           " defected%s: %s", message, runtime->last_defect_incarnation,
           runtime->last_defect_membership, count, defect);
  return combined;
}

/* SIGINT and SIGTERM (G22). The first one cancels main. In a program on the
   scheduler, the scheduler delivers it at its next step: main's children are
   cancelled and every root supervisor it started stops, each member draining
   within its shutdown_within. A program without the scheduler sets `cancel`,
   which its host operations poll; blocking reads return EINTR to see it. A
   second signal ends the program at once. */
static volatile int32_t received_signal = 0;
static int signal_pipe[2] = {-1, -1};
static ChimiRuntime *signal_runtime = NULL;

static void end_by_signal(int number) {
  /* Dying by the signal tells a parent (a shell, systemd, Kubernetes) that the
     program stopped because it was asked to. As process 1 in a container a
     default-action signal is ignored, so fall back to the shell's status. */
  signal(number, SIG_DFL);
  /* Inside the handler the signal is blocked; unblock it so it lands now. */
  sigset_t unblock;
  sigemptyset(&unblock);
  sigaddset(&unblock, number);
  sigprocmask(SIG_UNBLOCK, &unblock, NULL);
  raise(number);
  _exit(128 + number);
}

static void on_stop_signal(int number) {
  if (received_signal != 0) end_by_signal(number);
  received_signal = number;
  int saved = errno;
  if (signal_pipe[1] >= 0) (void)!write(signal_pipe[1], "", 1);
  errno = saved;
  if (signal_runtime) signal_runtime->cancel = 1;
}

/* A descriptor above standard input, non-blocking and closed on exec. */
static bool prepare_signal_descriptor(int *descriptor) {
  if (*descriptor <= STDERR_FILENO) {
    int moved = fcntl(*descriptor, F_DUPFD, STDERR_FILENO + 1);
    if (moved < 0) return false;
    close(*descriptor);
    *descriptor = moved;
  }
  return fcntl(*descriptor, F_SETFL, O_NONBLOCK) == 0 && fcntl(*descriptor, F_SETFD, FD_CLOEXEC) == 0;
}

static void install_stop_signals(ChimiRuntime *runtime) {
  bool scheduled = CHIMI_REQUIRES_SCHEDULER != 0;
  if (scheduled) {
    if (pipe(signal_pipe) != 0 || !prepare_signal_descriptor(&signal_pipe[0]) ||
        !prepare_signal_descriptor(&signal_pipe[1])) {
      fputs("Chimi runtime could not create its signal pipe\n", stderr);
      exit(70);
    }
  } else {
    signal_runtime = runtime;
  }
  struct sigaction action;
  memset(&action, 0, sizeof(action));
  action.sa_handler = on_stop_signal;
  sigemptyset(&action.sa_mask);
  sigaddset(&action.sa_mask, SIGINT);
  sigaddset(&action.sa_mask, SIGTERM);
  /* The scheduler sees a signal between steps, so interrupted reads restart;
     without it, a read returns EINTR so the host operation sees `cancel`. */
  action.sa_flags = scheduled ? SA_RESTART : 0;
  sigaction(SIGINT, &action, NULL);
  sigaction(SIGTERM, &action, NULL);
}

/* `--signal=TERM@2s`: a signal the virtual clock delivers at that instant.
   The time is an integer with a unit of ns, us, ms, s, m, or h. */
static bool parse_injected_signal(const char *text, int32_t *signal_number, int64_t *at) {
  const char *rest;
  if (strncmp(text, "INT@", 4) == 0) { *signal_number = SIGINT; rest = text + 4; }
  else if (strncmp(text, "TERM@", 5) == 0) { *signal_number = SIGTERM; rest = text + 5; }
  else return false;
  if (*rest < '0' || *rest > '9') return false;
  errno = 0;
  char *unit = NULL;
  unsigned long long amount = strtoull(rest, &unit, 10);
  if (errno == ERANGE) return false;
  static const struct { const char *name; uint64_t nanoseconds; } units[] = {
    {"ns", 1}, {"us", 1000}, {"ms", 1000000}, {"s", 1000000000},
    {"m", UINT64_C(60000000000)}, {"h", UINT64_C(3600000000000)},
  };
  for (size_t index = 0; index < sizeof(units) / sizeof(*units); ++index) {
    if (strcmp(unit, units[index].name) != 0) continue;
    if (amount > (uint64_t)INT64_MAX / units[index].nanoseconds) return false;
    *at = (int64_t)(amount * units[index].nanoseconds);
    return true;
  }
  return false;
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
#if CHIMI_PROCESS_KERNEL
    .process_defect_report = record_process_defect,
    .process_defect_context = runtime,
#endif
    .release_task_value = release_task_word,
    .release_channel_value = release_channel_word,
    .copy_channel_value = copy_channel_word,
    .release_task_context = release_task_context,
#if CHIMI_PROCESS_KERNEL
    .release_message = release_message_word,
    .release_spec_context = release_spec_context_word,
#endif
    .signal = &received_signal,
    .signal_fd = signal_pipe[0],
    .injected_signal = runtime->injected_signal,
    .injected_signal_at = runtime->injected_signal_at,
  };
  channel_endpoints_reset();
  AllocationScope *entry_scope = main_heap.innermost;
  AllocationArea *entry_target = allocation_target;
  ChimiSchedulerOutcome outcome = chimi_scheduler_run(&config);
  current_heap = &main_heap;
  /* The root runs on main_heap, and each region it enters records its scope in
     its own fiber frame. A root that overflowed, parked or was ended by the
     scheduler never closed those regions, and its frames will never run again,
     so drop them before the outcome below allocates; an overflow during an
     in-place list compaction also left allocation_target in such a region.
     Their allocations are left to exit. */
  main_heap.innermost = entry_scope;
  allocation_target = entry_target;
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
    return defect(with_process_defect(runtime, runtime->deadlock_waiters), -1);
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
    return defect(with_process_defect(runtime,
                                      "Task scheduler defected: the root task could not finish, "
                                      "because every task is parked (deadlock) or the scheduler failed"),
                  -1);
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
  int64_t test_index = -1;
  bool clock_start_given = false;
  const char *telemetry_path = NULL;
  const char *outcome_path = NULL;
  const char *memory_report_path = NULL;
  const char *profile_path = NULL;
  decode_constants();
  if (!CHIMI_REQUIRES_SCHEDULER) guard_host_stack();
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
    else if (strncmp(argv[index], "--profile-file=", 15) == 0) profile_path = argv[index] + 15;
    else if (strncmp(argv[index], "--execution-id=", 15) == 0) runtime.execution_id = argv[index] + 15;
    else if (strcmp(argv[index], "--cancel") == 0) runtime.cancel = 1;
    else if (strncmp(argv[index], "--test=", 7) == 0) {
      errno = 0;
      char *end = NULL;
      unsigned long value = strtoul(argv[index] + 7, &end, 10);
      if (errno == ERANGE || end == argv[index] + 7 || *end != '\0' || value >= CHIMI_TEST_COUNT) {
        fprintf(stderr, "--test requires a test index below %u\n", CHIMI_TEST_COUNT);
        return 64;
      }
      test_index = (int64_t)value;
    }
    else if (strncmp(argv[index], "--signal=", 9) == 0) {
      if (!parse_injected_signal(argv[index] + 9, &runtime.injected_signal, &runtime.injected_signal_at)) {
        fputs("--signal requires INT or TERM, @, and a time such as 2s or 150ms\n", stderr);
        return 64;
      }
    }
    else if (strcmp(argv[index], "--clock=real") == 0) runtime.virtual_clock = false;
    else if (strcmp(argv[index], "--clock=virtual") == 0) runtime.virtual_clock = true;
    else if (strncmp(argv[index], "--clock-start=", 14) == 0) {
      if (!parse_timestamp(argv[index] + 14, &runtime.clock_start)) {
        fputs("--clock-start requires an RFC 3339 timestamp from 1678 through 2262, "
              "such as 2026-11-10T09:30:00Z\n", stderr);
        return 64;
      }
      clock_start_given = true;
    }
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
    if (runtime.injected_signal && !runtime.virtual_clock) {
      fputs("--signal requires --clock=virtual; under the real clock, send the signal\n", stderr);
      return 64;
    }
    if (clock_start_given && !runtime.virtual_clock) {
      fputs("--clock-start requires --clock=virtual; the real clock reads the host's wall time\n", stderr);
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
  if (profile_path && ((telemetry_path && strcmp(profile_path, telemetry_path) == 0) ||
                       (outcome_path && strcmp(profile_path, outcome_path) == 0) ||
                       (memory_report_path && strcmp(profile_path, memory_report_path) == 0))) {
    fputs("profile, memory report, telemetry, and runner outcome require distinct files\n", stderr);
    return 64;
  }
  if (profile_path && benchmark_repeats) {
    fputs("--profile-file cannot be combined with --benchmark-repeats\n", stderr);
    return 64;
  }
  if (profile_path) {
    profile_rows = calloc(CHIMI_NODE_COUNT ? CHIMI_NODE_COUNT : 1, sizeof(*profile_rows));
    if (!profile_rows) exhausted_memory();
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
  if (test_index >= 0) {
    entry_function = &CHIMI_FUNCTIONS[CHIMI_TEST_FUNCTIONS[test_index]];
    entry_node = entry_function->body;
    entry = CHIMI_NODE_FUNCTIONS[entry_node];
  }
  for (uint32_t i = 0; entry_function == NULL && i < CHIMI_FUNCTION_COUNT; ++i)
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
      ChimiEnvironment *environment = new_environment(entry_function->local_count);
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
    write_failed = write_profile(profile_path) || write_failed;
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
  install_stop_signals(&runtime);
  if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
  ChimiEnvironment *environment = new_environment(entry_function->local_count);
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
  write_failed = write_profile(profile_path) || write_failed;
  write_failed = close_output(runtime.telemetry) || write_failed;
  write_failed = close_output(runtime.outcome) || write_failed;
  if (write_failed) return 74;
  switch (control->kind) {
    case CONTROL_SUCCESS: return 0;
    case CONTROL_FAILURE: return 1;
    case CONTROL_DEFECT: return 70;
    case CONTROL_CANCEL:
      /* A signal ended the program: an injected one reports the shell's
         status, a real one is re-raised. */
      if (received_signal != 0 && runtime.injected_signal) return 128 + received_signal;
      if (received_signal != 0) end_by_signal(received_signal);
      return 130;
  }
  return 70;
}
