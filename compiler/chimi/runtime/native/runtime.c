/* Chimi native runtime ABI version 22. */
/* glibc hides strdup and clock_gettime under -std=c11; macOS
   exposes them by default, and a POSIX feature macro there would hide others. */
#if defined(__linux__) && !defined(_GNU_SOURCE)
#define _GNU_SOURCE
#endif
#include "runtime.h"
#include "chimi_fiber.h"

#include <inttypes.h>
#include <float.h>
#include <math.h>
#include <stdarg.h>
#include <errno.h>
#include <limits.h>
#include <stdatomic.h>
#include <stddef.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>
#include <fcntl.h>
#include <poll.h>
#include <pthread.h>
#ifdef __linux__
#include <sched.h>
#endif
#include <signal.h>
#include <sys/wait.h>
#include <dirent.h>
#include <sys/stat.h>
#include <sys/socket.h>

/* The stack of every scheduler fiber: the root task, child tasks, and incarnations.
   AddressSanitizer's redzones make frames several times larger, so a sanitized
   test build (CHIMI_TEST_RUNTIME_SANITIZE) gives each fiber four times as much,
   and a program reaches the same call depths as in an ordinary build. */
#if defined(__SANITIZE_ADDRESS__)
#define CHIMI_FIBER_STACK_BYTES (4u * 1024u * 1024u)
#elif defined(__has_feature)
#if __has_feature(address_sanitizer)
#define CHIMI_FIBER_STACK_BYTES (4u * 1024u * 1024u)
#endif
#endif
#ifndef CHIMI_FIBER_STACK_BYTES
#define CHIMI_FIBER_STACK_BYTES (1024u * 1024u)
#endif

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
__attribute__((weak)) size_t chimi_scheduler_peak_task_records(void) { return 0; }
/* Without the fiber archive nothing is ever cancelled and no scope fires. */
__attribute__((weak)) uint32_t chimi_fiber_cancel_is_terminal(void) { return 1; }
__attribute__((weak)) uint32_t chimi_fiber_scope_cancel_pending(void) { return 0; }
__attribute__((weak)) uint32_t chimi_fiber_scope_polls(void) { return 0; }
__attribute__((weak)) uint64_t chimi_fiber_current_incarnation(void) { return 0; }
__attribute__((weak)) void chimi_fiber_report_defect(const char *message) { (void)message; }
/* Without the fiber archive no channel or task value exists to count. */
__attribute__((weak)) void chimi_fiber_handle_retain(uint64_t key) { (void)key; }
__attribute__((weak)) void chimi_fiber_handle_drop(uint64_t key) { (void)key; }
/* Without the fiber archive no time passes. */
__attribute__((weak)) int64_t chimi_fiber_now(void) { return 0; }
/* Program data from a build defines the trace's module list; C fixtures that
   write their own program data need not. */
__attribute__((weak)) const char *CHIMI_TRACE_MODULES = "";

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
               VALUE_CAUSE,
               /* A `std/test.Recorder`, `Script` or `Mailbox`: its TestLog id in
                  `as.address.target`. */
               VALUE_TEST_RECORDER, VALUE_TEST_SCRIPT, VALUE_TEST_MAILBOX,
               /* A binding `native type` (G20) in `as.handle`: plain data naming a
                  slot of the runtime's handle table, never the resource itself. */
               VALUE_NATIVE_HANDLE } ValueKind;
/* CALL_CONSTRUCTOR is a constructor with fields named as a value; calling it constructs. */
typedef enum { CALL_DECLARED, CALL_CLOSURE, CALL_DEPENDENCY, CALL_HOST, CALL_INTRINSIC,
               CALL_CONSTRUCTOR } CallableKind;
/* The HOST_CLOCK_* functions are the execution clock provider's methods. A shifted
   timestamp is `Clock.starting_at`'s: its `inner` reads now, plus `shift`. */
typedef enum { HOST_PROCESS_UTF8_LINES, HOST_WRITE_LINE, HOST_WRITE_ERROR,
               /* `Random.next_int` of the runner's `random` provider. */
               HOST_RANDOM_NEXT_INT,
               HOST_CLOCK_NOW, HOST_CLOCK_TIMESTAMP, HOST_CLOCK_SLEEP_UNTIL,
               HOST_CLOCK_SHIFTED_TIMESTAMP,
               /* A Wait timer on a user Clock: its `inner` is the provider's
                  sleep_until, called with the Instant in `shift`. */
               HOST_SELECT_SLEEP } HostFunction;
/* CONTROL_BREAK carries the target loop's state symbol in `node` and its value in `value`.
   Verified programs keep it inside one function, so it never reaches a call or root. */
typedef enum { CONTROL_SUCCESS, CONTROL_FAILURE, CONTROL_DEFECT, CONTROL_CANCEL, CONTROL_BREAK } ControlKind;

typedef struct ChimiValue ChimiValue;
/* A `Decimal`: decimal_abi.rs `ChimiDecimal`. The magnitude's halves (an
   `unsigned __int128` member would raise the value's alignment), at most 38
   digits; zero is never negative. */
typedef struct { uint64_t high; uint64_t low; uint32_t scale; bool negative; } ChimiDecimal;
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
    ChimiDecimal decimal;
    /* `name` is the binding type's, for Debug.render. */
    struct { uint32_t slot; uint32_t generation; const char *name; } handle;
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

enum { CHIMI_MAX_INJECTED_SIGNALS = 64 };

struct ChimiRuntime {
  const char *execution_id;
  /* The root computation's name: the program entry's, or under `--test=<k>`
     test k's, so each test's telemetry and outcome name the test. */
  const char *computation;
  bool full_trace;
  /* `--clock=virtual`: the scheduler's deterministic time source. */
  bool virtual_clock;
  /* `--carriers=<n>`: how many carriers run the program's fibers, by
     default one per core the process may run on (default_carriers);
     carriers 1 and up run on threads the scheduler starts. */
  uint32_t carriers;
  /* `--clock-start`: the virtual source's Timestamp at Instant zero, in Unix ns. */
  int64_t clock_start;
  /* The Unix time at the execution clock's origin, set as the computation
     starts (start_unix_nanos). */
  int64_t start_unix_ns;
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
  /* `--signal=HUP@...` and the user signals: std/signal signals the virtual
     clock latches. */
  ChimiInjectedSignal injected_user_signals[CHIMI_MAX_INJECTED_SIGNALS];
  size_t injected_user_signal_count;
  bool silent;
  FILE *telemetry;
  FILE *outcome;
  uint32_t max_line_bytes;
  /* std/log: the least severe level written (0 debug .. 3 error), and the
     file named by --log-file, or NULL for standard error. */
  int64_t log_level;
  FILE *log_file;
  /* std/metrics: the file named by --metrics-file, written at exit, or NULL
     when nothing is recorded. */
  FILE *metrics_file;
  /* The telemetry stream's next sequence number. It and
     trace_events_constructed change only under trace_lock, which an emitter
     holds for a whole event, so events from several carriers come out whole
     and numbered in the order written. */
  uint64_t next_sequence;
  /* Span ids are unique across the process, so every carrier takes them
     from one counter. */
  _Atomic uint64_t next_span;
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
     compaction, allocates into the area of the value it updates, and leaves
     module constants alone when carriers run on threads). So a value
     never points to a value allocated after it, and a survivor never points to
     a young value. A minor pass therefore marks from the state through young
     allocations only, and survivors are walked only by a full pass.
   - Environments that nodes write after creating them are frame environments
     (ALLOCATION_FRAME_ENVIRONMENT). No value points to one; marking and copying
     check the kind of every allocation they reach, so a violation aborts.

   A pass boundary moves the young chunk allocations that the roots reach into
   survivor blocks (heap/collector.rs) and rewrites the roots, which are passed by
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
     counters (see heap/young.rs, flush_counts). */
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
   headers and fragment records as well as payloads; static constants are excluded.
   Each carrier keeps its own copy, a signed delta: an allocation made on one
   carrier can be freed on another, so one carrier's count can fall below zero,
   and only the sum over carriers (heap_totals_sum) is the process's. A peak is
   recorded per carrier (record_peak in chimi-runtime heap/allocator.rs), which is the process's with one
   carrier or in the simulation; with several carrier threads the report's
   peaks are the process-wide ones its fold keeps. */
typedef struct {
  int64_t live_allocations;
  int64_t footprint_bytes;
  int64_t peak_footprint_bytes;
  int64_t live_fragments;
  int64_t fragment_footprint_bytes;
  int64_t peak_fragment_footprint_bytes;
  int64_t live_heaps;
  /* Freed small blocks kept for reuse (heap/small_blocks.rs); not live. */
  int64_t cached_bytes;
} HeapTotals;

enum { REGION_FULL_MIN_GROWTH = 4 * 1024 };

/* The main heap is owner 1; heap owners are unique across the process, and any
   carrier creates heaps, taking ids from one counter (heap/fragment.rs). */
static RuntimeHeap main_heap = {.owner = 1, .base = {.role = AREA_BASE, .heap = &main_heap}};
static AllocationArea static_area = {.role = AREA_STATIC};
/* Mark epochs are unique across the process, not per carrier or per heap, so an
   allocation that moves between heaps or carriers can never carry a stale epoch
   that a later pass reuses (parallel-scheduler.md, "Runtime state by owner");
   the collector (heap/collector.rs) takes them from one process-wide counter.
   Epochs a carrier takes from that counter at once. A loop pass collects,
   so one atomic add per pass made the counter's cache line bounce between
   carriers on every pass: a counting loop ran 1.7 times faster on four
   carriers instead of close to four (step 3 PR 6). One carrier alone takes
   consecutive blocks, so it sees the same epochs as with a counter. */
enum { MARK_EPOCH_BLOCK = 4096 };

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
/* Allocated once at startup and indexed by node, and shared by every carrier,
   so each count is a relaxed atomic add (profile_count). */
#define profile_rows chimi_runtime_profile_rows
ProfileRow *chimi_runtime_profile_rows;

static void profile_count(uint64_t *field, uint64_t by) { __atomic_fetch_add(field, by, __ATOMIC_RELAXED); }

/* Ends the process for a fatal runtime error, from any carrier thread. exit
   is not safe to call twice at once, so only the first caller runs it (and
   its atexit drain of queued console lines); a carrier that fails at the
   same time ends the process at once with the same code, since it may hold
   a lock the drain needs. */
static atomic_flag runtime_exiting = ATOMIC_FLAG_INIT;
static _Noreturn void runtime_exit(int code) {
  if (atomic_flag_test_and_set(&runtime_exiting)) _exit(code);
  exit(code);
}

static void exhausted_memory(void) { fputs("Chimi runtime exhausted memory\n", stderr); runtime_exit(70); }
/* The same end for the Rust runtime archive's allocator (chimi-runtime-standalone). */
_Noreturn void chimi_runtime_exhausted_memory(void);
_Noreturn void chimi_runtime_exhausted_memory(void) { exhausted_memory(); runtime_exit(70); }
_Noreturn void chimi_runtime_list_length_overflow(void);
_Noreturn void chimi_runtime_list_length_overflow(void) {
  fputs("Chimi runtime list length overflow\n", stderr); runtime_exit(70);
}

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
/* One carrier's free lists, one per size class, and their lengths. The cache
   that keeps them is Rust (chimi-runtime heap/small_blocks.rs). */
typedef struct {
  SmallBlock *lists[SMALL_BLOCK_LIMIT / SMALL_BLOCK_GRAIN];
  uint32_t counts[SMALL_BLOCK_LIMIT / SMALL_BLOCK_GRAIN];
} SmallBlockCache;

/* A growable stack of allocations, used by marking and copying so neither recurses:
   both run on 64 KiB fiber stacks. */
typedef struct { void **items; size_t length; size_t capacity; } PointerStack;

/* CHIMI_HEAP_VERIFY is a checking build, on by default under AddressSanitizer
   (where the small-block cache is off) and off with -DCHIMI_HEAP_VERIFY=0:
   after every pass the Rust oracle (chimi-runtime heap/verify.rs) reads the
   region again through the layout mirrors and ends the program if the pass
   left the young generation non-empty, miscounted the survivors or the
   promotions, kept garbage in a full pass, or freed something still
   reachable. What a pass frees is recorded by address, so the check never
   reads a freed block. */
#ifndef CHIMI_HEAP_VERIFY
#define CHIMI_HEAP_VERIFY (!CHIMI_SMALL_BLOCK_CACHE)
#endif
#if CHIMI_HEAP_VERIFY
static _Thread_local PointerStack heap_verify_freed, heap_verify_chunks;
static void pointer_stack_push(PointerStack *stack, void *pointer);
#endif

/* The state of the fiber that is running: the heap it allocates in and, when set, the
   area allocate() uses instead of that heap's innermost one (static constants,
   fragment copies, and list_slice's in-place compaction). It belongs to the fiber,
   not to the thread, so scheduler_request saves it before a park and restores it on
   resume. */
typedef struct {
  RuntimeHeap *heap;
  AllocationArea *target;
} FiberContext;

/* What reclaiming scopes allocated and kept, for the memory report's
   `scoped_*` fields. */
typedef struct {
  size_t total_allocated_bytes;
  size_t allocated_bytes_by_kind[ALLOCATION_KIND_COUNT];
  size_t peak_live_bytes;
  size_t retained_bytes;
  uint64_t collections;
  uint64_t full_collections;
  uint64_t scanned_allocations;
} ScopeTotals;

/* Everything one carrier thread mutates while it runs fibers (R4 step 1,
   docs/architecture/parallel-scheduler.md "Runtime state by owner"). There is one
   carrier today, so `carrier` names one static instance and every access folds to a
   fixed address, as the statics it replaced did. A field here may be scratch that a
   run leaves clean, a cache, or a counter that a later step sums across carriers;
   none of it may be held across a scheduler request, because once fibers move
   between carriers the fiber may resume on another one. Process-wide tables whose
   ids travel inside values (handle counts, files, bindings, test logs), signal state
   and constants stay file-scope globals. */
typedef struct {
  FiberContext fiber;
  /* Process-wide accounting, so tests can prove that memory stays flat and that
     destroying heaps and fragments returns every allocation. */
  HeapTotals heap_totals;
  /* Footprint of the young chunk allocations whose counters are still deferred to
     their scope's next flush. Bumping one only adds to this, so the true footprint
     (footprint_bytes plus this) peaks just before it falls: footprint_sub and
     footprint_add record the peak there. */
  int64_t pending_footprint_bytes;
  /* This carrier's footprints as last folded into the process-wide counters
     (heap/allocator.rs fold); used only with several carrier threads. */
  int64_t folded_footprint_bytes;
  int64_t folded_fragment_footprint_bytes;
  /* Bytes allocated outside every region, static constants included. Nothing
     reclaims them before exit. */
  uint64_t unscoped_allocated_bytes;
  /* Visited-set slots examined by every capture transfer so far; the memory
     report's `capture_transfer_probes`, so a test can bound transfer work without
     timing it. */
  uint64_t capture_transfer_probes;
  /* The largest peak of live payload bytes of any incarnation's heap. */
  size_t process_heap_peak_value_bytes;
  /* Values and payload bytes that message copies (Process.send) duplicated: the
     memory report's `copied_values` and `copied_bytes`. Only a copy made with
     `copy_counting` set adds to them. */
  bool copy_counting;
  uint64_t copied_values;
  uint64_t copied_bytes;
  /* The foreign owner the last failed fragment_copy reached. */
  uint64_t fragment_copy_foreign_owner;
  SmallBlockCache small_blocks;
  YoungChunk *young_chunk_cache;
  uint32_t young_chunk_cache_count;
  /* The pass in progress. Marking reaches only this region's young allocations, plus
     its survivors in a full pass: everything else is older than the region's values
     or than the pass, so it cannot reach them. */
  AllocationScope *collecting_scope;
  bool collecting_full;
  bool collecting_foreign;
  uint64_t collecting_epoch;
  /* This carrier's unused mark epochs, [next_epoch, end_epoch): taken from
     the process-wide counter a block at a time (heap/collector.rs). */
  uint64_t next_epoch;
  uint64_t end_epoch;
  /* This carrier's unused span ids, [next_span, end_span), taken from the
     runtime's counter a block at a time when several carriers run
     (take_spans). */
  uint64_t next_span;
  uint64_t end_span;
  PointerStack mark_stack;
  PointerStack copy_stack;
  /* Copies that the fragment copy (heap/fragment.rs) still has to redirect. Copying never yields, so one stack,
     kept between copies, serves every heap. */
  PointerStack copy_pending;
  /* fs_read's buffer: read(2) fills it and the bytes are copied out before any
     park. NULL until the first read. */
  char *fs_read_chunk;
  /* What the outermost reclaiming scopes that ended on this carrier
     allocated and kept (end_measured_allocation_scope), summed in the memory
     report. */
  ScopeTotals scope_totals;
  /* The address below which a call on this carrier's own thread stack
     defects (guard_host_stack); 0 disables the check. Each carrier thread has
     its own stack. */
  uintptr_t host_stack_limit;
  /* The thread carrier_bind gave this carrier to; chimi_task_entry checks it
     when several carriers run, since a thread that never bound aliases
     carrier 0. */
  pthread_t thread;
  bool bound;
  /* This carrier's thread holds console_lock. A Carrier field rather than a
     thread-local of its own: a writer parks between leaving and entering the
     lock, and may resume on another thread (`carrier` is read afresh). */
  bool console_lock_held;
} Carrier;

/* Carrier 0 is the main thread's and runs every fiber until more carriers are
   created; the others are allocated once, by carriers_create, before any
   carrier thread starts, and live until exit. */
static Carrier carrier_first = {.fiber = {.heap = &main_heap}};

#ifdef CHIMI_CARRIER_SIMULATION
/* A test build (parallel-scheduler.md, "Runtime state by owner"): every fiber
   resumes on the other of two carriers from the one it parked on, each with its
   own scratch, caches, and counters, the way a fiber migrates
   once N carriers run. It finds a pointer into a carrier kept across a park and
   a field filed as carrier state that must follow the fiber. Since an
   allocation is often freed on the other carrier, it also exercises the
   counters' signed deltas. */
static Carrier carrier_second = {.fiber = {.heap = &main_heap}};
static Carrier *carrier_others = &carrier_second;
#define carrier_count chimi_runtime_carrier_count
size_t chimi_runtime_carrier_count = 2;
/* How the carriers run, for the Rust allocator's footprint accounting
   (heap/allocator.rs): 0 one carrier, 1 several carrier threads, 2 this
   simulation. */
uint8_t chimi_runtime_carrier_mode = 2;
/* Unused here (the simulation's `carrier` is a plain pointer), but the Rust
   allocator names it on Linux; see the threaded build's. */
intptr_t chimi_runtime_carrier_tls_offset;
static Carrier *carrier = &carrier_first;

static Carrier *carrier_at(size_t index) { return index == 0 ? &carrier_first : &carrier_others[index - 1]; }

static void carrier_bind(Carrier *chosen) {
  chosen->thread = pthread_self();
  chosen->bound = true;
  carrier = chosen;
}

static void carriers_create(size_t count) { (void)count; }

/* Continues on the carrier other than `parked`. */
static void carrier_migrate(Carrier *parked) {
  Carrier *from = carrier;
  Carrier *to = carrier_at(parked == &carrier_first);
  if (to != from) {
    /* Both simulated carriers run on one thread, so they share its stack. */
    to->host_stack_limit = from->host_stack_limit;
    to->thread = from->thread;
    to->bound = from->bound;
  }
  carrier = to;
}
#else
/* The carrier the calling thread runs. A parked fiber may resume on another
   thread (parallel-scheduler.md, step 4 PR 4b), and a compiler may keep a
   thread-local's address, or the thread pointer it comes from, from before a
   call that parks. So `carrier` reads the thread pointer afresh at every use:
   on Linux with one volatile instruction plus the variable's offset from it,
   which is the same on every thread for an initial-exec variable; elsewhere
   through a call the compiler cannot merge or move. Leaf code that never
   parks reads it once (`Carrier *const here = carrier;`), since a read now
   costs more than a plain load. */
#if defined(__APPLE__)
#define CARRIER_TLS_MODEL
#else
#define CARRIER_TLS_MODEL __attribute__((tls_model("initial-exec")))
#endif
/* Every thread starts on carrier 0, which is what the main thread and the C
   test fixtures that include this file use. */
static _Thread_local Carrier *carrier_this_thread CARRIER_TLS_MODEL = &carrier_first;
#if defined(__linux__) && (defined(__x86_64__) || defined(__aarch64__))
/* Set before main, on the main thread, and read on every thread. */
#define carrier_tls_offset chimi_runtime_carrier_tls_offset
intptr_t carrier_tls_offset;
static inline char *carrier_thread_pointer(void) {
  char *pointer;
#if defined(__x86_64__)
  __asm__ volatile("movq %%fs:0, %0" : "=r"(pointer));
#else
  __asm__ volatile("mrs %0, tpidr_el0" : "=r"(pointer));
#endif
  return pointer;
}
__attribute__((constructor)) static void carrier_tls_offset_init(void) {
  carrier_tls_offset = (char *)&carrier_this_thread - carrier_thread_pointer();
}
static inline Carrier *carrier_current(void) {
  return *(Carrier **)(carrier_thread_pointer() + carrier_tls_offset);
}
#else
__attribute__((noinline)) static Carrier *carrier_current(void) {
  return *(Carrier *volatile const *)&carrier_this_thread;
}
#endif
#define carrier (carrier_current())
static Carrier *carrier_others;
#define carrier_count chimi_runtime_carrier_count
size_t chimi_runtime_carrier_count = 1;
/* As in the simulation: 0 one carrier, 1 several carrier threads. */
uint8_t chimi_runtime_carrier_mode = 0;

static Carrier *carrier_at(size_t index) { return index == 0 ? &carrier_first : &carrier_others[index - 1]; }

/* Makes `chosen` the calling thread's carrier. A carrier thread binds once,
   through carrier_thread_enter, before it runs anything that allocates; the
   main thread binds carrier 0 at startup. */
static void carrier_bind(Carrier *chosen) {
  chosen->thread = pthread_self();
  chosen->bound = true;
  carrier_this_thread = chosen;
}

static void footprint_seed_process(void);

/* Sizes the carriers at `count`, at least 1. Called once, on the main thread,
   before any carrier thread starts and before anything runs on carrier 1 or
   later, so those carriers start zeroed. */
static void carriers_create(size_t count) {
  if (count <= 1 || carrier_others) return;
  carrier_others = calloc(count - 1, sizeof(*carrier_others));
  if (!carrier_others) {
    fputs("Chimi runtime exhausted memory\n", stderr);
    runtime_exit(70);
  }
  for (size_t index = 0; index + 1 < count; ++index) carrier_others[index].fiber.heap = &main_heap;
  carrier_count = count;
  chimi_runtime_carrier_mode = 1;
  footprint_seed_process();
}

static void carrier_migrate(Carrier *parked) { (void)parked; }
#endif

/* Sums every carrier's counters into the process's, with the largest peak of
   each; footprint_bytes includes the young bytes still deferred. A carrier
   thread may be changing its own while this reads, so it is exact only once no
   fiber runs (the memory report) or in the simulation, whose carriers share one
   thread. */
static HeapTotals heap_totals_sum(void) {
  HeapTotals sum = {0};
  for (size_t index = 0; index < carrier_count; ++index) {
    const HeapTotals *own = &carrier_at(index)->heap_totals;
    sum.live_allocations += own->live_allocations;
    sum.footprint_bytes += own->footprint_bytes + carrier_at(index)->pending_footprint_bytes;
    if (own->peak_footprint_bytes > sum.peak_footprint_bytes) sum.peak_footprint_bytes = own->peak_footprint_bytes;
    sum.live_fragments += own->live_fragments;
    sum.fragment_footprint_bytes += own->fragment_footprint_bytes;
    if (own->peak_fragment_footprint_bytes > sum.peak_fragment_footprint_bytes)
      sum.peak_fragment_footprint_bytes = own->peak_fragment_footprint_bytes;
    sum.live_heaps += own->live_heaps;
    sum.cached_bytes += own->cached_bytes;
  }
  return sum;
}

static AllocationArea *current_area(void) {
  Carrier *const here = carrier;
  if (here->fiber.target) return here->fiber.target;
  return here->fiber.heap->innermost ? &here->fiber.heap->innermost->young : &here->fiber.heap->base;
}


/* Process-wide footprints and their peaks with several carrier threads
   (step 3 PR 6b). Each carrier folds its change into them once it has moved
   FOOTPRINT_FOLD_BYTES from what it last folded, so the counters are within
   carrier_count * FOOTPRINT_FOLD_SLACK of the true sum and a peak is the
   largest folded sum. Young allocations are checked once per chunk
   (young_bump), not per allocation, hence the chunk in the slack. One
   atomic add per 64 KiB of change, instead of one per allocation, keeps the
   carriers off one cache line (as the mark epochs). */
enum { FOOTPRINT_FOLD_BYTES = 64 * 1024, FOOTPRINT_FOLD_SLACK = FOOTPRINT_FOLD_BYTES + YOUNG_CHUNK_BYTES };
#define process_footprint_bytes chimi_runtime_process_footprint_bytes
#define process_peak_footprint_bytes chimi_runtime_process_peak_footprint_bytes
#define process_fragment_footprint_bytes chimi_runtime_process_fragment_footprint_bytes
#define process_peak_fragment_footprint_bytes chimi_runtime_process_peak_fragment_footprint_bytes
_Atomic int64_t process_footprint_bytes;
_Atomic int64_t process_peak_footprint_bytes;
_Atomic int64_t process_fragment_footprint_bytes;
_Atomic int64_t process_peak_fragment_footprint_bytes;

/* The footprint accounting is Rust (chimi-runtime heap/allocator.rs), with the
   allocator that keeps it; the counters above stay here with the memory report
   that reads them. */
void chimi_footprint_add_v1(Carrier *here, size_t bytes);
void chimi_footprint_sub_v1(Carrier *here, size_t bytes);
void chimi_fragment_footprint_add_v1(Carrier *here, size_t bytes);
void chimi_fragment_footprint_sub_v1(Carrier *here, size_t bytes);
void chimi_footprint_seed_process_v1(Carrier *first);
static void footprint_add(size_t bytes) { chimi_footprint_add_v1(carrier, bytes); }
static void footprint_sub(size_t bytes) { chimi_footprint_sub_v1(carrier, bytes); }
static void fragment_footprint_add(size_t bytes) { chimi_fragment_footprint_add_v1(carrier, bytes); }
static void fragment_footprint_sub(size_t bytes) { chimi_fragment_footprint_sub_v1(carrier, bytes); }
#ifndef CHIMI_CARRIER_SIMULATION
/* Starts the process-wide counters from carrier 0 (carriers_create). */
static void footprint_seed_process(void) { chimi_footprint_seed_process_v1(&carrier_first); }
#endif

/* What the Rust allocator needs from here. */
const bool chimi_runtime_small_block_cache = CHIMI_SMALL_BLOCK_CACHE;
Carrier *chimi_runtime_carrier_at(size_t index);
Carrier *chimi_runtime_carrier_at(size_t index) { return carrier_at(index); }
/* The running carrier, for the Rust allocator where it cannot read the
   thread-local itself (other platforms, and the simulation). */
Carrier *chimi_runtime_current_carrier(void);
Carrier *chimi_runtime_current_carrier(void) { return carrier; }



/* The young chunks and the regions' start and end are Rust (chimi-runtime
   heap/young.rs): its per-carrier cache of empty chunks, the deferred counts of a
   pass's chunk allocations, the reset of a region's chunks at a pass boundary,
   and a region's start and the end of its accounting. */
void chimi_scope_begin_v1(Carrier *here, AllocationScope *scope);
/* Adds the chunk allocations made since the last call to the memory counters; the
   collector calls it, and so does the list harness. */
void chimi_young_flush_counts_v1(Carrier *here, AllocationScope *scope);
static void allocation_scope_begin(AllocationScope *scope) { chimi_scope_begin_v1(carrier, scope); }

/* CHIMI_POISON_YOUNG_CHUNKS is a checking build: every pass boundary frees its
   chunks after filling them with a pattern, so a pointer kept across the boundary
   reads garbage, or fails as a use after free under AddressSanitizer. */
#ifdef CHIMI_POISON_YOUNG_CHUNKS
const bool chimi_runtime_poison_young_chunks = true;
#else
const bool chimi_runtime_poison_young_chunks = false;
#endif
const bool chimi_runtime_heap_verify = CHIMI_HEAP_VERIFY;
void chimi_runtime_heap_verify_chunk(const unsigned char *data, const unsigned char *used);
void chimi_runtime_heap_verify_chunk(const unsigned char *data, const unsigned char *used) {
#if CHIMI_HEAP_VERIFY
  pointer_stack_push(&heap_verify_chunks, (void *)data);
  pointer_stack_push(&heap_verify_chunks, (void *)used);
#else
  (void)data;
  (void)used;
#endif
}

/* The allocator is Rust (chimi-runtime heap/allocator.rs): `count` zeroed items
   of `size` bytes in the running fiber's area, from the innermost region's young
   chunks when they fit. It ends the program on overflow or exhausted memory. */
void *chimi_allocate_v1(size_t count, size_t size);
/* It reads the running carrier itself, the way `carrier` does here: passing
   it from an inlined wrapper grew a small program by 13 KB, and an out-of-line
   wrapper cost a loop 4% more instructions. */
static void *allocate(size_t count, size_t size) { return chimi_allocate_v1(count, size); }

/* What the allocator needs from the C that is still here. */
_Noreturn void chimi_runtime_allocation_overflow(void);
_Noreturn void chimi_runtime_allocation_overflow(void) {
  fputs("Chimi runtime allocation overflow\n", stderr); runtime_exit(70);
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

/* Handle counts (docs/architecture/supervised-process-runtime.md, "Channel
   release"). Each live Sender, Receiver, Completion or Task value counts as one
   reference to its channel or task, and each live native handle value as one
   reference to its handle, in whichever heap or fragment it is allocated, so a
   copy in transit (a message, a task's captures or result) counts while the
   scheduler holds it. channel_capability_value, task_value and
   native_handle_value create these values, the fragment copy is the only thing
   that copies them, and the allocator's release the only thing that frees them; those
   keep the counts (counted_handle_retain, counted_handle_drop).

   Channel and task keys are counted by the scheduler (chimi_fiber_handle_retain
   in chimi_fiber.h): the channel id, or the task id with CHIMI_TASK_HANDLE_KEY
   set. At zero no value can name the channel or task again, and the scheduler
   releases it itself at its next chance on the dropping thread, so a count may
   reach zero anywhere, inside a collection or a scheduler release hook
   included. A raw task id held without a value (a select's hidden timer, a
   combinator's children) is safe because no collection runs inside one
   intrinsic, so the value it came from is still live.

   Native handles (sockets, listeners, TLS sessions, exec children and pipes)
   are counted here, under NATIVE_HANDLE_KEY with their slot and generation, so
   a handle that no value names any more is closed then, whichever end freed
   its last copy: a forced or discarded process, a task cancelled before it
   ran, or a collection (native_handle_unreferenced). An explicit close leaves
   the count alone, and the later zero finds a newer generation and does
   nothing. One table serves every carrier (parallel-scheduler.md, "Handle
   counts"): a handle copied on one carrier can be freed on another, so only
   the total is meaningful. The counts and their lock live in Rust
   (chimi-runtime handle_counts.rs, handle_counts_abi.rs): each call below
   takes a pthread mutex there, after the scheduler's kernel lock and
   test_log_lock, and nothing under it retains, drops, parks or runs a hook.
   The standalone allocator can exit with it held when the map cannot grow,
   which is safe while no atexit handler touches the counts. Keys are never
   0. */
#define NATIVE_HANDLE_KEY (UINT64_C(1) << 62)

static uint64_t native_handle_key(uint32_t slot, uint32_t generation) {
  return NATIVE_HANDLE_KEY | (uint64_t)slot << 32 | generation;
}

static void native_handle_unreferenced(uint64_t key);
bool chimi_handle_counts_retain_v1(uint64_t key);
bool chimi_handle_counts_drop_v1(uint64_t key);
void chimi_handle_counts_reset_v1(void);
size_t chimi_handle_counts_len_v1(void);
size_t chimi_handle_counts_capacity_v1(void);

/* Set at an execution's first retain of any counted handle, and kept until
   the next execution starts (channel_endpoints_reset). It is one flag for
   both tables, so the freers' shortcut below costs one load: every freed
   allocation pays it. */
#define handles_counted chimi_runtime_handles_counted
_Atomic bool handles_counted;

/* False only while no value this execution made can be a counted handle, so
   a freer may skip looking. A freer's own value was retained before it, so
   that retain's store is visible here. */
static bool handles_live(void) {
  return atomic_load_explicit(&handles_counted, memory_order_relaxed);
}

static void note_handle_counted(void) {
  /* A load first, so retains after the first do not write the shared line. */
  if (!atomic_load_explicit(&handles_counted, memory_order_relaxed))
    atomic_store_explicit(&handles_counted, true, memory_order_relaxed);
}

static bool is_channel_endpoint(const ChimiValue *value) {
  return value->kind == VALUE_CHANNEL_SENDER || value->kind == VALUE_CHANNEL_RECEIVER ||
         value->kind == VALUE_CHANNEL_COMPLETION;
}

/* The table key a counted value holds, or 0 for a value that is not counted. */
static uint64_t counted_handle_key(const ChimiValue *value) {
  if (is_channel_endpoint(value)) return value->as.capability.channel;
  if (value->kind == VALUE_TASK) return value->as.task | CHIMI_TASK_HANDLE_KEY;
  if (value->kind == VALUE_NATIVE_HANDLE)
    return native_handle_key(value->as.handle.slot, value->as.handle.generation);
  return 0;
}

static void native_handle_retain(uint64_t key) {
  if (!chimi_handle_counts_retain_v1(key)) exhausted_memory();
}

/* Removes one reference to `key`. Ensures: true exactly when this removed the
   last one, so the caller owns the key's release. No other thread can retain
   the same key meanwhile: once the count is zero, no value names the key. */
static bool native_handle_release(uint64_t key) {
  return chimi_handle_counts_drop_v1(key);
}

static bool is_native_handle_key(uint64_t key) {
  return (key & (CHIMI_TASK_HANDLE_KEY | NATIVE_HANDLE_KEY)) == NATIVE_HANDLE_KEY;
}

/* Counts one more live value naming `key`. */
static void counted_handle_retain(uint64_t key) {
  note_handle_counted();
  if (is_native_handle_key(key)) native_handle_retain(key);
  else chimi_fiber_handle_retain(key);
}

/* Counts one live value naming `key` fewer. A native handle no value names
   any more closes at once; the scheduler releases a channel or task itself. */
static void counted_handle_drop(uint64_t key) {
  if (!is_native_handle_key(key)) {
    chimi_fiber_handle_drop(key);
    return;
  }
  if (native_handle_release(key)) native_handle_unreferenced(key);
}

/* Each execution starts a new scheduler, whose channel and task ids start over
   (the scheduler clears its own counts). Runs before the scheduler starts, so
   no fiber holds a handle on any carrier. */
static void channel_endpoints_reset(void) {
  chimi_handle_counts_reset_v1();
  atomic_store_explicit(&handles_counted, false, memory_order_relaxed);
}

/* Drops the count a freed value holds when it is a counted handle; the Rust
   allocator's release calls it once handles are live. */
void chimi_runtime_release_counted_value(const ChimiValue *value);
void chimi_runtime_release_counted_value(const ChimiValue *value) {
  uint64_t key = counted_handle_key(value);
  if (key) counted_handle_drop(key);
}

/* Adds the count a copied value holds when it is a counted handle; the Rust
   fragment copy (heap/fragment.rs) calls it for every value it copies. */
void chimi_runtime_retain_counted_value(const ChimiValue *value);
void chimi_runtime_retain_counted_value(const ChimiValue *value) {
  uint64_t key = counted_handle_key(value);
  if (key) counted_handle_retain(key);
}

/* A fragment's record. Its area is the first member, so once the area is adopted
   the record lives on as that area and is freed with it (heap/fragment.rs). */
enum { FRAGMENT_ROOTS = 2 };
typedef struct RuntimeFragment {
  AllocationArea area;
  uint32_t root_count;
  void *roots[FRAGMENT_ROOTS];
} RuntimeFragment;

/* The heap and value structs above are mirrored in Rust (chimi-runtime
   heap/layout.rs) for the allocator and collector moving there (CHI-305 step
   5). These asserts hold the C side to the mirror's numbers; the
   runtime_layout test holds the mirror to these lines. */
/* A list vector's trie width (mirrored in chimi-runtime heap/layout.rs). */
enum { LIST_BITS = 5, LIST_WIDTH = 1 << LIST_BITS, LIST_MASK = LIST_WIDTH - 1 };

/* BEGIN generated layout asserts: chimi-runtime heap/layout.rs LAYOUT.
   Regenerate with CHIMI_LAYOUT_BLESS=1 cargo test -p chimi --test runtime_layout. */
#define CHIMI_LAYOUT_CLASS(x) _Generic((x), _Bool: 3, float: 4, double: 4, \
  default: ((__typeof__(x))-1 < (__typeof__(x))1 ? 1 : 2))
_Static_assert(LIST_BITS == 5, "layout: LIST_BITS");
_Static_assert(VALUE_VOID == 0, "layout: VALUE_VOID");
_Static_assert(VALUE_INTEGER == 1, "layout: VALUE_INTEGER");
_Static_assert(VALUE_FLOAT == 2, "layout: VALUE_FLOAT");
_Static_assert(VALUE_TEXT == 3, "layout: VALUE_TEXT");
_Static_assert(VALUE_BOOLEAN == 4, "layout: VALUE_BOOLEAN");
_Static_assert(VALUE_LIST == 5, "layout: VALUE_LIST");
_Static_assert(VALUE_RECORD == 6, "layout: VALUE_RECORD");
_Static_assert(VALUE_CONSTRUCTED == 7, "layout: VALUE_CONSTRUCTED");
_Static_assert(VALUE_CALLABLE == 8, "layout: VALUE_CALLABLE");
_Static_assert(VALUE_MAP == 9, "layout: VALUE_MAP");
_Static_assert(VALUE_JSON_NUMBER == 10, "layout: VALUE_JSON_NUMBER");
_Static_assert(VALUE_TASK == 11, "layout: VALUE_TASK");
_Static_assert(VALUE_CHANNEL_SENDER == 12, "layout: VALUE_CHANNEL_SENDER");
_Static_assert(VALUE_CHANNEL_RECEIVER == 13, "layout: VALUE_CHANNEL_RECEIVER");
_Static_assert(VALUE_CHANNEL_COMPLETION == 14, "layout: VALUE_CHANNEL_COMPLETION");
_Static_assert(VALUE_TUPLE == 15, "layout: VALUE_TUPLE");
_Static_assert(VALUE_RANGE == 16, "layout: VALUE_RANGE");
_Static_assert(VALUE_DURATION == 17, "layout: VALUE_DURATION");
_Static_assert(VALUE_INSTANT == 18, "layout: VALUE_INSTANT");
_Static_assert(VALUE_TIMESTAMP == 19, "layout: VALUE_TIMESTAMP");
_Static_assert(VALUE_DECIMAL == 20, "layout: VALUE_DECIMAL");
_Static_assert(VALUE_PROCESS_ADDRESS == 21, "layout: VALUE_PROCESS_ADDRESS");
_Static_assert(VALUE_PROCESS_INBOX == 22, "layout: VALUE_PROCESS_INBOX");
_Static_assert(VALUE_SUPERVISOR_RUNNING == 23, "layout: VALUE_SUPERVISOR_RUNNING");
_Static_assert(VALUE_MANAGED_PROCESS == 24, "layout: VALUE_MANAGED_PROCESS");
_Static_assert(VALUE_MANAGED_SUPERVISOR == 25, "layout: VALUE_MANAGED_SUPERVISOR");
_Static_assert(VALUE_PROCESS_MONITOR == 26, "layout: VALUE_PROCESS_MONITOR");
_Static_assert(VALUE_SET == 27, "layout: VALUE_SET");
_Static_assert(VALUE_BYTES == 28, "layout: VALUE_BYTES");
_Static_assert(VALUE_CAUSE == 29, "layout: VALUE_CAUSE");
_Static_assert(VALUE_TEST_RECORDER == 30, "layout: VALUE_TEST_RECORDER");
_Static_assert(VALUE_TEST_SCRIPT == 31, "layout: VALUE_TEST_SCRIPT");
_Static_assert(VALUE_TEST_MAILBOX == 32, "layout: VALUE_TEST_MAILBOX");
_Static_assert(VALUE_NATIVE_HANDLE == 33, "layout: VALUE_NATIVE_HANDLE");
_Static_assert(sizeof(ValueKind) == 4, "layout: sizeof(ValueKind)");
_Static_assert(CALL_DECLARED == 0, "layout: CALL_DECLARED");
_Static_assert(CALL_CLOSURE == 1, "layout: CALL_CLOSURE");
_Static_assert(CALL_DEPENDENCY == 2, "layout: CALL_DEPENDENCY");
_Static_assert(CALL_HOST == 3, "layout: CALL_HOST");
_Static_assert(CALL_INTRINSIC == 4, "layout: CALL_INTRINSIC");
_Static_assert(CALL_CONSTRUCTOR == 5, "layout: CALL_CONSTRUCTOR");
_Static_assert(sizeof(CallableKind) == 4, "layout: sizeof(CallableKind)");
_Static_assert(sizeof(HostFunction) == 4, "layout: sizeof(HostFunction)");
_Static_assert(CONTROL_SUCCESS == 0, "layout: CONTROL_SUCCESS");
_Static_assert(CONTROL_FAILURE == 1, "layout: CONTROL_FAILURE");
_Static_assert(CONTROL_DEFECT == 2, "layout: CONTROL_DEFECT");
_Static_assert(CONTROL_CANCEL == 3, "layout: CONTROL_CANCEL");
_Static_assert(CONTROL_BREAK == 4, "layout: CONTROL_BREAK");
_Static_assert(sizeof(ControlKind) == 4, "layout: sizeof(ControlKind)");
_Static_assert(ALLOCATION_BYTES == 0, "layout: ALLOCATION_BYTES");
_Static_assert(ALLOCATION_VALUE == 1, "layout: ALLOCATION_VALUE");
_Static_assert(ALLOCATION_CONTROL == 2, "layout: ALLOCATION_CONTROL");
_Static_assert(ALLOCATION_ENVIRONMENT == 3, "layout: ALLOCATION_ENVIRONMENT");
_Static_assert(ALLOCATION_VALUE_POINTERS == 4, "layout: ALLOCATION_VALUE_POINTERS");
_Static_assert(ALLOCATION_FIELDS == 5, "layout: ALLOCATION_FIELDS");
_Static_assert(ALLOCATION_CALLABLE == 6, "layout: ALLOCATION_CALLABLE");
_Static_assert(ALLOCATION_MAP_NODE == 7, "layout: ALLOCATION_MAP_NODE");
_Static_assert(ALLOCATION_LIST_NODE == 8, "layout: ALLOCATION_LIST_NODE");
_Static_assert(ALLOCATION_FRAME_ENVIRONMENT == 9, "layout: ALLOCATION_FRAME_ENVIRONMENT");
_Static_assert(ALLOCATION_KIND_COUNT == 10, "layout: ALLOCATION_KIND_COUNT");
_Static_assert(sizeof(AllocationKind) == 4, "layout: sizeof(AllocationKind)");
_Static_assert(AREA_YOUNG == 0, "layout: AREA_YOUNG");
_Static_assert(AREA_SURVIVOR == 1, "layout: AREA_SURVIVOR");
_Static_assert(AREA_BASE == 2, "layout: AREA_BASE");
_Static_assert(AREA_FRAGMENT == 3, "layout: AREA_FRAGMENT");
_Static_assert(AREA_STATIC == 4, "layout: AREA_STATIC");
_Static_assert(sizeof(AreaRole) == 4, "layout: sizeof(AreaRole)");
_Static_assert(YOUNG_CHUNK_BYTES == 16384, "layout: YOUNG_CHUNK_BYTES");
_Static_assert(YOUNG_CHUNK_CACHE == 16, "layout: YOUNG_CHUNK_CACHE");
_Static_assert(YOUNG_CHUNK_CAPACITY == 16368, "layout: YOUNG_CHUNK_CAPACITY");
_Static_assert(YOUNG_CHUNK_LARGEST == 4092, "layout: YOUNG_CHUNK_LARGEST");
_Static_assert(SMALL_BLOCK_GRAIN == 16, "layout: SMALL_BLOCK_GRAIN");
_Static_assert(SMALL_BLOCK_LIMIT == 256, "layout: SMALL_BLOCK_LIMIT");
_Static_assert(SMALL_BLOCK_CACHE == 4096, "layout: SMALL_BLOCK_CACHE");
_Static_assert(FRAGMENT_ROOTS == 2, "layout: FRAGMENT_ROOTS");
_Static_assert(REGION_FULL_MIN_GROWTH == 4096, "layout: REGION_FULL_MIN_GROWTH");
_Static_assert(MARK_EPOCH_BLOCK == 4096, "layout: MARK_EPOCH_BLOCK");
_Static_assert(HEAP_OK == 0, "layout: HEAP_OK");
_Static_assert(HEAP_FOREIGN_POINTER == 1, "layout: HEAP_FOREIGN_POINTER");
_Static_assert(sizeof(HeapStatus) == 4, "layout: sizeof(HeapStatus)");
_Static_assert(sizeof(ChimiDecimal) == 24, "layout: sizeof(ChimiDecimal)");
_Static_assert(_Alignof(ChimiDecimal) == 8, "layout: _Alignof(ChimiDecimal)");
_Static_assert(offsetof(ChimiDecimal, high) == 0, "layout: offsetof(ChimiDecimal, high)");
_Static_assert(sizeof(((ChimiDecimal *)0)->high) == 8, "layout: sizeof(((ChimiDecimal *)0)->high)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiDecimal *)0)->high) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiDecimal *)0)->high)");
_Static_assert(offsetof(ChimiDecimal, low) == 8, "layout: offsetof(ChimiDecimal, low)");
_Static_assert(sizeof(((ChimiDecimal *)0)->low) == 8, "layout: sizeof(((ChimiDecimal *)0)->low)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiDecimal *)0)->low) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiDecimal *)0)->low)");
_Static_assert(offsetof(ChimiDecimal, scale) == 16, "layout: offsetof(ChimiDecimal, scale)");
_Static_assert(sizeof(((ChimiDecimal *)0)->scale) == 4, "layout: sizeof(((ChimiDecimal *)0)->scale)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiDecimal *)0)->scale) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiDecimal *)0)->scale)");
_Static_assert(offsetof(ChimiDecimal, negative) == 20, "layout: offsetof(ChimiDecimal, negative)");
_Static_assert(sizeof(((ChimiDecimal *)0)->negative) == 1, "layout: sizeof(((ChimiDecimal *)0)->negative)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiDecimal *)0)->negative) == 3, "layout: CHIMI_LAYOUT_CLASS(((ChimiDecimal *)0)->negative)");
_Static_assert(sizeof(RuntimeField) == 16, "layout: sizeof(RuntimeField)");
_Static_assert(offsetof(RuntimeField, name) == 0, "layout: offsetof(RuntimeField, name)");
_Static_assert(sizeof(((RuntimeField *)0)->name) == 8, "layout: sizeof(((RuntimeField *)0)->name)");
_Static_assert(offsetof(RuntimeField, value) == 8, "layout: offsetof(RuntimeField, value)");
_Static_assert(sizeof(((RuntimeField *)0)->value) == 8, "layout: sizeof(((RuntimeField *)0)->value)");
_Static_assert(sizeof(RuntimeCapability) == 24, "layout: sizeof(RuntimeCapability)");
_Static_assert(offsetof(RuntimeCapability, channel) == 0, "layout: offsetof(RuntimeCapability, channel)");
_Static_assert(sizeof(((RuntimeCapability *)0)->channel) == 8, "layout: sizeof(((RuntimeCapability *)0)->channel)");
_Static_assert(CHIMI_LAYOUT_CLASS(((RuntimeCapability *)0)->channel) == 2, "layout: CHIMI_LAYOUT_CLASS(((RuntimeCapability *)0)->channel)");
_Static_assert(offsetof(RuntimeCapability, owner) == 8, "layout: offsetof(RuntimeCapability, owner)");
_Static_assert(sizeof(((RuntimeCapability *)0)->owner) == 8, "layout: sizeof(((RuntimeCapability *)0)->owner)");
_Static_assert(CHIMI_LAYOUT_CLASS(((RuntimeCapability *)0)->owner) == 2, "layout: CHIMI_LAYOUT_CLASS(((RuntimeCapability *)0)->owner)");
_Static_assert(offsetof(RuntimeCapability, token) == 16, "layout: offsetof(RuntimeCapability, token)");
_Static_assert(sizeof(((RuntimeCapability *)0)->token) == 8, "layout: sizeof(((RuntimeCapability *)0)->token)");
_Static_assert(CHIMI_LAYOUT_CLASS(((RuntimeCapability *)0)->token) == 2, "layout: CHIMI_LAYOUT_CLASS(((RuntimeCapability *)0)->token)");
_Static_assert(sizeof(ListNode) == 8, "layout: sizeof(ListNode)");
_Static_assert(_Alignof(ListNode) == 8, "layout: _Alignof(ListNode)");
_Static_assert(offsetof(ListNode, height) == 0, "layout: offsetof(ListNode, height)");
_Static_assert(sizeof(((ListNode *)0)->height) == 4, "layout: sizeof(((ListNode *)0)->height)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ListNode *)0)->height) == 2, "layout: CHIMI_LAYOUT_CLASS(((ListNode *)0)->height)");
_Static_assert(offsetof(ListNode, count) == 4, "layout: offsetof(ListNode, count)");
_Static_assert(sizeof(((ListNode *)0)->count) == 4, "layout: sizeof(((ListNode *)0)->count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ListNode *)0)->count) == 2, "layout: CHIMI_LAYOUT_CLASS(((ListNode *)0)->count)");
_Static_assert(offsetof(ListNode, slots) == 8, "layout: offsetof(ListNode, slots)");
_Static_assert(sizeof(ListVector) == 24, "layout: sizeof(ListVector)");
_Static_assert(offsetof(ListVector, root) == 0, "layout: offsetof(ListVector, root)");
_Static_assert(sizeof(((ListVector *)0)->root) == 8, "layout: sizeof(((ListVector *)0)->root)");
_Static_assert(offsetof(ListVector, tail) == 8, "layout: offsetof(ListVector, tail)");
_Static_assert(sizeof(((ListVector *)0)->tail) == 8, "layout: sizeof(((ListVector *)0)->tail)");
_Static_assert(offsetof(ListVector, length) == 16, "layout: offsetof(ListVector, length)");
_Static_assert(sizeof(((ListVector *)0)->length) == 4, "layout: sizeof(((ListVector *)0)->length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ListVector *)0)->length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ListVector *)0)->length)");
_Static_assert(sizeof(ChimiSymbol) == 40, "layout: sizeof(ChimiSymbol)");
_Static_assert(offsetof(ChimiSymbol, kind) == 0, "layout: offsetof(ChimiSymbol, kind)");
_Static_assert(sizeof(((ChimiSymbol *)0)->kind) == 4, "layout: sizeof(((ChimiSymbol *)0)->kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->kind)");
_Static_assert(offsetof(ChimiSymbol, name) == 4, "layout: offsetof(ChimiSymbol, name)");
_Static_assert(sizeof(((ChimiSymbol *)0)->name) == 4, "layout: sizeof(((ChimiSymbol *)0)->name)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->name) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->name)");
_Static_assert(offsetof(ChimiSymbol, function) == 8, "layout: offsetof(ChimiSymbol, function)");
_Static_assert(sizeof(((ChimiSymbol *)0)->function) == 4, "layout: sizeof(((ChimiSymbol *)0)->function)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->function) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->function)");
_Static_assert(offsetof(ChimiSymbol, constant) == 12, "layout: offsetof(ChimiSymbol, constant)");
_Static_assert(sizeof(((ChimiSymbol *)0)->constant) == 4, "layout: sizeof(((ChimiSymbol *)0)->constant)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->constant) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->constant)");
_Static_assert(offsetof(ChimiSymbol, constructor) == 16, "layout: offsetof(ChimiSymbol, constructor)");
_Static_assert(sizeof(((ChimiSymbol *)0)->constructor) == 4, "layout: sizeof(((ChimiSymbol *)0)->constructor)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->constructor) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->constructor)");
_Static_assert(offsetof(ChimiSymbol, intrinsic) == 20, "layout: offsetof(ChimiSymbol, intrinsic)");
_Static_assert(sizeof(((ChimiSymbol *)0)->intrinsic) == 4, "layout: sizeof(((ChimiSymbol *)0)->intrinsic)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->intrinsic) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->intrinsic)");
_Static_assert(offsetof(ChimiSymbol, standard_constructor) == 24, "layout: offsetof(ChimiSymbol, standard_constructor)");
_Static_assert(sizeof(((ChimiSymbol *)0)->standard_constructor) == 4, "layout: sizeof(((ChimiSymbol *)0)->standard_constructor)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->standard_constructor) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->standard_constructor)");
_Static_assert(offsetof(ChimiSymbol, standard_type) == 28, "layout: offsetof(ChimiSymbol, standard_type)");
_Static_assert(sizeof(((ChimiSymbol *)0)->standard_type) == 4, "layout: sizeof(((ChimiSymbol *)0)->standard_type)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->standard_type) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->standard_type)");
_Static_assert(offsetof(ChimiSymbol, environment_slot) == 32, "layout: offsetof(ChimiSymbol, environment_slot)");
_Static_assert(sizeof(((ChimiSymbol *)0)->environment_slot) == 4, "layout: sizeof(((ChimiSymbol *)0)->environment_slot)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->environment_slot) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->environment_slot)");
_Static_assert(offsetof(ChimiSymbol, binding) == 36, "layout: offsetof(ChimiSymbol, binding)");
_Static_assert(sizeof(((ChimiSymbol *)0)->binding) == 4, "layout: sizeof(((ChimiSymbol *)0)->binding)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->binding) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSymbol *)0)->binding)");
_Static_assert(sizeof(ChimiConstructor) == 28, "layout: sizeof(ChimiConstructor)");
_Static_assert(offsetof(ChimiConstructor, type_name) == 0, "layout: offsetof(ChimiConstructor, type_name)");
_Static_assert(sizeof(((ChimiConstructor *)0)->type_name) == 4, "layout: sizeof(((ChimiConstructor *)0)->type_name)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->type_name) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->type_name)");
_Static_assert(offsetof(ChimiConstructor, variant_name) == 4, "layout: offsetof(ChimiConstructor, variant_name)");
_Static_assert(sizeof(((ChimiConstructor *)0)->variant_name) == 4, "layout: sizeof(((ChimiConstructor *)0)->variant_name)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->variant_name) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->variant_name)");
_Static_assert(offsetof(ChimiConstructor, fields_offset) == 8, "layout: offsetof(ChimiConstructor, fields_offset)");
_Static_assert(sizeof(((ChimiConstructor *)0)->fields_offset) == 4, "layout: sizeof(((ChimiConstructor *)0)->fields_offset)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->fields_offset) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->fields_offset)");
_Static_assert(offsetof(ChimiConstructor, fields_length) == 12, "layout: offsetof(ChimiConstructor, fields_length)");
_Static_assert(sizeof(((ChimiConstructor *)0)->fields_length) == 4, "layout: sizeof(((ChimiConstructor *)0)->fields_length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->fields_length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->fields_length)");
_Static_assert(offsetof(ChimiConstructor, outcome_success) == 16, "layout: offsetof(ChimiConstructor, outcome_success)");
_Static_assert(sizeof(((ChimiConstructor *)0)->outcome_success) == 4, "layout: sizeof(((ChimiConstructor *)0)->outcome_success)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->outcome_success) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->outcome_success)");
_Static_assert(offsetof(ChimiConstructor, ordinal) == 20, "layout: offsetof(ChimiConstructor, ordinal)");
_Static_assert(sizeof(((ChimiConstructor *)0)->ordinal) == 4, "layout: sizeof(((ChimiConstructor *)0)->ordinal)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->ordinal) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->ordinal)");
_Static_assert(offsetof(ChimiConstructor, program_exit) == 24, "layout: offsetof(ChimiConstructor, program_exit)");
_Static_assert(sizeof(((ChimiConstructor *)0)->program_exit) == 4, "layout: sizeof(((ChimiConstructor *)0)->program_exit)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->program_exit) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiConstructor *)0)->program_exit)");
_Static_assert(CHIMI_INTRINSIC_MAP_EMPTY == 3, "layout: CHIMI_INTRINSIC_MAP_EMPTY");
_Static_assert(CHIMI_INTRINSIC_MAP_GET == 4, "layout: CHIMI_INTRINSIC_MAP_GET");
_Static_assert(CHIMI_INTRINSIC_MAP_PUT == 5, "layout: CHIMI_INTRINSIC_MAP_PUT");
_Static_assert(CHIMI_INTRINSIC_MAP_REMOVE == 133, "layout: CHIMI_INTRINSIC_MAP_REMOVE");
_Static_assert(CHIMI_INTRINSIC_MAP_SIZE == 134, "layout: CHIMI_INTRINSIC_MAP_SIZE");
_Static_assert(CHIMI_INTRINSIC_MAP_REDUCE_ENTRIES == 135, "layout: CHIMI_INTRINSIC_MAP_REDUCE_ENTRIES");
_Static_assert(CHIMI_INTRINSIC_SET_EMPTY == 136, "layout: CHIMI_INTRINSIC_SET_EMPTY");
_Static_assert(CHIMI_INTRINSIC_SET_CONTAINS == 137, "layout: CHIMI_INTRINSIC_SET_CONTAINS");
_Static_assert(CHIMI_INTRINSIC_SET_ADD == 138, "layout: CHIMI_INTRINSIC_SET_ADD");
_Static_assert(CHIMI_INTRINSIC_SET_REMOVE == 139, "layout: CHIMI_INTRINSIC_SET_REMOVE");
_Static_assert(CHIMI_INTRINSIC_SET_SIZE == 140, "layout: CHIMI_INTRINSIC_SET_SIZE");
_Static_assert(CHIMI_INTRINSIC_SET_REDUCE_ITEMS == 141, "layout: CHIMI_INTRINSIC_SET_REDUCE_ITEMS");
_Static_assert(CHIMI_CONSTRUCTOR_OPTION_SOME == 6, "layout: CHIMI_CONSTRUCTOR_OPTION_SOME");
_Static_assert(CHIMI_CONSTRUCTOR_OPTION_NONE == 7, "layout: CHIMI_CONSTRUCTOR_OPTION_NONE");
_Static_assert(CHIMI_INTRINSIC_JSON_DECODE == 142, "layout: CHIMI_INTRINSIC_JSON_DECODE");
_Static_assert(CHIMI_INTRINSIC_JSON_ENCODE == 143, "layout: CHIMI_INTRINSIC_JSON_ENCODE");
_Static_assert(CHIMI_INTRINSIC_JSON_INT == 144, "layout: CHIMI_INTRINSIC_JSON_INT");
_Static_assert(CHIMI_INTRINSIC_JSON_NUMBER_TO_INT == 145, "layout: CHIMI_INTRINSIC_JSON_NUMBER_TO_INT");
_Static_assert(CHIMI_INTRINSIC_JSON_NUMBER_TO_DECIMAL == 146, "layout: CHIMI_INTRINSIC_JSON_NUMBER_TO_DECIMAL");
_Static_assert(CHIMI_INTRINSIC_DECIMAL_SCALED == 147, "layout: CHIMI_INTRINSIC_DECIMAL_SCALED");
_Static_assert(CHIMI_INTRINSIC_DECIMAL_READ == 148, "layout: CHIMI_INTRINSIC_DECIMAL_READ");
_Static_assert(CHIMI_INTRINSIC_DECIMAL_TO_TEXT == 149, "layout: CHIMI_INTRINSIC_DECIMAL_TO_TEXT");
_Static_assert(CHIMI_INTRINSIC_DECIMAL_ADD_CHECKED == 150, "layout: CHIMI_INTRINSIC_DECIMAL_ADD_CHECKED");
_Static_assert(CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED == 151, "layout: CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED");
_Static_assert(CHIMI_INTRINSIC_DECIMAL_MULTIPLY_CHECKED == 152, "layout: CHIMI_INTRINSIC_DECIMAL_MULTIPLY_CHECKED");
_Static_assert(CHIMI_INTRINSIC_DECIMAL_DIVIDE_ROUNDED == 153, "layout: CHIMI_INTRINSIC_DECIMAL_DIVIDE_ROUNDED");
_Static_assert(CHIMI_INTRINSIC_DECIMAL_TO_FLOAT == 154, "layout: CHIMI_INTRINSIC_DECIMAL_TO_FLOAT");
_Static_assert(CHIMI_INTRINSIC_BYTES_FROM == 155, "layout: CHIMI_INTRINSIC_BYTES_FROM");
_Static_assert(CHIMI_INTRINSIC_BYTES_LENGTH == 156, "layout: CHIMI_INTRINSIC_BYTES_LENGTH");
_Static_assert(CHIMI_INTRINSIC_BYTES_GET == 157, "layout: CHIMI_INTRINSIC_BYTES_GET");
_Static_assert(CHIMI_INTRINSIC_BYTES_INDEX_OF == 158, "layout: CHIMI_INTRINSIC_BYTES_INDEX_OF");
_Static_assert(CHIMI_INTRINSIC_BYTES_CONCAT == 159, "layout: CHIMI_INTRINSIC_BYTES_CONCAT");
_Static_assert(CHIMI_INTRINSIC_BYTES_SLICE == 160, "layout: CHIMI_INTRINSIC_BYTES_SLICE");
_Static_assert(CHIMI_INTRINSIC_BYTES_TO_LIST == 161, "layout: CHIMI_INTRINSIC_BYTES_TO_LIST");
_Static_assert(CHIMI_INTRINSIC_BYTES_FROM_TEXT == 162, "layout: CHIMI_INTRINSIC_BYTES_FROM_TEXT");
_Static_assert(CHIMI_INTRINSIC_BYTES_TO_TEXT_LOSSY == 163, "layout: CHIMI_INTRINSIC_BYTES_TO_TEXT_LOSSY");
_Static_assert(CHIMI_INTRINSIC_BYTES_VALID_UTF8_LENGTH == 164, "layout: CHIMI_INTRINSIC_BYTES_VALID_UTF8_LENGTH");
_Static_assert(CHIMI_INTRINSIC_BYTES_READ_INT_CODE == 165, "layout: CHIMI_INTRINSIC_BYTES_READ_INT_CODE");
_Static_assert(CHIMI_INTRINSIC_BYTES_INT_BYTES == 166, "layout: CHIMI_INTRINSIC_BYTES_INT_BYTES");
_Static_assert(CHIMI_INTRINSIC_HTTP_COUNT_ENDS == 216, "layout: CHIMI_INTRINSIC_HTTP_COUNT_ENDS");
_Static_assert(CHIMI_INTRINSIC_DURATION_NANOSECONDS == 41, "layout: CHIMI_INTRINSIC_DURATION_NANOSECONDS");
_Static_assert(CHIMI_INTRINSIC_DURATION_MICROSECONDS == 42, "layout: CHIMI_INTRINSIC_DURATION_MICROSECONDS");
_Static_assert(CHIMI_INTRINSIC_DURATION_MILLISECONDS == 43, "layout: CHIMI_INTRINSIC_DURATION_MILLISECONDS");
_Static_assert(CHIMI_INTRINSIC_DURATION_SECONDS == 44, "layout: CHIMI_INTRINSIC_DURATION_SECONDS");
_Static_assert(CHIMI_INTRINSIC_DURATION_MINUTES == 45, "layout: CHIMI_INTRINSIC_DURATION_MINUTES");
_Static_assert(CHIMI_INTRINSIC_DURATION_HOURS == 46, "layout: CHIMI_INTRINSIC_DURATION_HOURS");
_Static_assert(CHIMI_INTRINSIC_DURATION_TO_NANOSECONDS == 47, "layout: CHIMI_INTRINSIC_DURATION_TO_NANOSECONDS");
_Static_assert(CHIMI_INTRINSIC_DURATION_TO_MICROSECONDS == 48, "layout: CHIMI_INTRINSIC_DURATION_TO_MICROSECONDS");
_Static_assert(CHIMI_INTRINSIC_DURATION_TO_MILLISECONDS == 49, "layout: CHIMI_INTRINSIC_DURATION_TO_MILLISECONDS");
_Static_assert(CHIMI_INTRINSIC_DURATION_TO_SECONDS == 50, "layout: CHIMI_INTRINSIC_DURATION_TO_SECONDS");
_Static_assert(CHIMI_INTRINSIC_DURATION_ADD == 51, "layout: CHIMI_INTRINSIC_DURATION_ADD");
_Static_assert(CHIMI_INTRINSIC_DURATION_SUBTRACT == 52, "layout: CHIMI_INTRINSIC_DURATION_SUBTRACT");
_Static_assert(CHIMI_INTRINSIC_DURATION_MULTIPLY == 53, "layout: CHIMI_INTRINSIC_DURATION_MULTIPLY");
_Static_assert(CHIMI_INTRINSIC_DURATION_DIVIDE == 54, "layout: CHIMI_INTRINSIC_DURATION_DIVIDE");
_Static_assert(CHIMI_INTRINSIC_DURATION_NEGATE == 55, "layout: CHIMI_INTRINSIC_DURATION_NEGATE");
_Static_assert(CHIMI_INTRINSIC_DURATION_ABSOLUTE == 56, "layout: CHIMI_INTRINSIC_DURATION_ABSOLUTE");
_Static_assert(CHIMI_INTRINSIC_DURATION_TO_TEXT == 57, "layout: CHIMI_INTRINSIC_DURATION_TO_TEXT");
_Static_assert(CHIMI_INTRINSIC_DURATION_PARSE == 58, "layout: CHIMI_INTRINSIC_DURATION_PARSE");
_Static_assert(CHIMI_INTRINSIC_DEBUG_CRASH == 114, "layout: CHIMI_INTRINSIC_DEBUG_CRASH");
_Static_assert(CHIMI_INTRINSIC_OPTION_UNWRAP_OR_FAIL == 115, "layout: CHIMI_INTRINSIC_OPTION_UNWRAP_OR_FAIL");
_Static_assert(CHIMI_INTRINSIC_TEXT_CONCAT == 116, "layout: CHIMI_INTRINSIC_TEXT_CONCAT");
_Static_assert(CHIMI_INTRINSIC_TEXT_BYTE_LENGTH == 117, "layout: CHIMI_INTRINSIC_TEXT_BYTE_LENGTH");
_Static_assert(CHIMI_INTRINSIC_TEXT_BYTE_SLICE == 118, "layout: CHIMI_INTRINSIC_TEXT_BYTE_SLICE");
_Static_assert(CHIMI_INTRINSIC_TEXT_CONTAINS == 119, "layout: CHIMI_INTRINSIC_TEXT_CONTAINS");
_Static_assert(CHIMI_INTRINSIC_TEXT_STARTS_WITH == 120, "layout: CHIMI_INTRINSIC_TEXT_STARTS_WITH");
_Static_assert(CHIMI_INTRINSIC_TEXT_ENDS_WITH == 121, "layout: CHIMI_INTRINSIC_TEXT_ENDS_WITH");
_Static_assert(CHIMI_INTRINSIC_TEXT_SPLIT == 122, "layout: CHIMI_INTRINSIC_TEXT_SPLIT");
_Static_assert(CHIMI_INTRINSIC_TEXT_REPLACE == 123, "layout: CHIMI_INTRINSIC_TEXT_REPLACE");
_Static_assert(CHIMI_INTRINSIC_TEXT_TRIM_START == 124, "layout: CHIMI_INTRINSIC_TEXT_TRIM_START");
_Static_assert(CHIMI_INTRINSIC_TEXT_TRIM_END == 125, "layout: CHIMI_INTRINSIC_TEXT_TRIM_END");
_Static_assert(CHIMI_INTRINSIC_TEXT_LOWERCASE == 126, "layout: CHIMI_INTRINSIC_TEXT_LOWERCASE");
_Static_assert(CHIMI_INTRINSIC_TEXT_UPPERCASE == 127, "layout: CHIMI_INTRINSIC_TEXT_UPPERCASE");
_Static_assert(CHIMI_INTRINSIC_TEXT_GRAPHEMES == 128, "layout: CHIMI_INTRINSIC_TEXT_GRAPHEMES");
_Static_assert(CHIMI_INTRINSIC_LIST_GET == 129, "layout: CHIMI_INTRINSIC_LIST_GET");
_Static_assert(CHIMI_INTRINSIC_LIST_PUSH == 130, "layout: CHIMI_INTRINSIC_LIST_PUSH");
_Static_assert(CHIMI_INTRINSIC_LIST_SLICE == 131, "layout: CHIMI_INTRINSIC_LIST_SLICE");
_Static_assert(CHIMI_INTRINSIC_LIST_SET == 132, "layout: CHIMI_INTRINSIC_LIST_SET");
_Static_assert(CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY == 33, "layout: CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY");
_Static_assert(CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID == 34, "layout: CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID");
_Static_assert(CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE == 35, "layout: CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE");
_Static_assert(CHIMI_CONSTRUCTOR_JSON_NULL == 8, "layout: CHIMI_CONSTRUCTOR_JSON_NULL");
_Static_assert(CHIMI_CONSTRUCTOR_JSON_BOOLEAN == 9, "layout: CHIMI_CONSTRUCTOR_JSON_BOOLEAN");
_Static_assert(CHIMI_CONSTRUCTOR_JSON_NUMBER == 10, "layout: CHIMI_CONSTRUCTOR_JSON_NUMBER");
_Static_assert(CHIMI_CONSTRUCTOR_JSON_TEXT == 11, "layout: CHIMI_CONSTRUCTOR_JSON_TEXT");
_Static_assert(CHIMI_CONSTRUCTOR_JSON_ARRAY == 12, "layout: CHIMI_CONSTRUCTOR_JSON_ARRAY");
_Static_assert(CHIMI_CONSTRUCTOR_JSON_OBJECT == 13, "layout: CHIMI_CONSTRUCTOR_JSON_OBJECT");
_Static_assert(CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON == 14, "layout: CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON");
_Static_assert(CHIMI_CONSTRUCTOR_JSON_NOT_INTEGER == 15, "layout: CHIMI_CONSTRUCTOR_JSON_NOT_INTEGER");
_Static_assert(CHIMI_CONSTRUCTOR_JSON_OUT_OF_RANGE == 16, "layout: CHIMI_CONSTRUCTOR_JSON_OUT_OF_RANGE");
_Static_assert(sizeof(MapNode) == 40, "layout: sizeof(MapNode)");
_Static_assert(offsetof(MapNode, key) == 0, "layout: offsetof(MapNode, key)");
_Static_assert(sizeof(((MapNode *)0)->key) == 8, "layout: sizeof(((MapNode *)0)->key)");
_Static_assert(offsetof(MapNode, value) == 8, "layout: offsetof(MapNode, value)");
_Static_assert(sizeof(((MapNode *)0)->value) == 8, "layout: sizeof(((MapNode *)0)->value)");
_Static_assert(offsetof(MapNode, left) == 16, "layout: offsetof(MapNode, left)");
_Static_assert(sizeof(((MapNode *)0)->left) == 8, "layout: sizeof(((MapNode *)0)->left)");
_Static_assert(offsetof(MapNode, right) == 24, "layout: offsetof(MapNode, right)");
_Static_assert(sizeof(((MapNode *)0)->right) == 8, "layout: sizeof(((MapNode *)0)->right)");
_Static_assert(offsetof(MapNode, height) == 32, "layout: offsetof(MapNode, height)");
_Static_assert(sizeof(((MapNode *)0)->height) == 2, "layout: sizeof(((MapNode *)0)->height)");
_Static_assert(CHIMI_LAYOUT_CLASS(((MapNode *)0)->height) == 2, "layout: CHIMI_LAYOUT_CLASS(((MapNode *)0)->height)");
_Static_assert(sizeof(ChimiEnvironment) == 24, "layout: sizeof(ChimiEnvironment)");
_Static_assert(offsetof(ChimiEnvironment, locals) == 0, "layout: offsetof(ChimiEnvironment, locals)");
_Static_assert(sizeof(((ChimiEnvironment *)0)->locals) == 8, "layout: sizeof(((ChimiEnvironment *)0)->locals)");
_Static_assert(offsetof(ChimiEnvironment, requirements) == 8, "layout: offsetof(ChimiEnvironment, requirements)");
_Static_assert(sizeof(((ChimiEnvironment *)0)->requirements) == 8, "layout: sizeof(((ChimiEnvironment *)0)->requirements)");
_Static_assert(offsetof(ChimiEnvironment, local_count) == 16, "layout: offsetof(ChimiEnvironment, local_count)");
_Static_assert(sizeof(((ChimiEnvironment *)0)->local_count) == 4, "layout: sizeof(((ChimiEnvironment *)0)->local_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiEnvironment *)0)->local_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiEnvironment *)0)->local_count)");
_Static_assert(sizeof(ChimiCallable) == 80, "layout: sizeof(ChimiCallable)");
_Static_assert(offsetof(ChimiCallable, kind) == 0, "layout: offsetof(ChimiCallable, kind)");
_Static_assert(sizeof(((ChimiCallable *)0)->kind) == 4, "layout: sizeof(((ChimiCallable *)0)->kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->kind)");
_Static_assert(offsetof(ChimiCallable, symbol) == 4, "layout: offsetof(ChimiCallable, symbol)");
_Static_assert(sizeof(((ChimiCallable *)0)->symbol) == 4, "layout: sizeof(((ChimiCallable *)0)->symbol)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->symbol) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->symbol)");
_Static_assert(offsetof(ChimiCallable, body) == 8, "layout: offsetof(ChimiCallable, body)");
_Static_assert(sizeof(((ChimiCallable *)0)->body) == 4, "layout: sizeof(((ChimiCallable *)0)->body)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->body) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->body)");
_Static_assert(offsetof(ChimiCallable, parameters_offset) == 12, "layout: offsetof(ChimiCallable, parameters_offset)");
_Static_assert(sizeof(((ChimiCallable *)0)->parameters_offset) == 4, "layout: sizeof(((ChimiCallable *)0)->parameters_offset)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->parameters_offset) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->parameters_offset)");
_Static_assert(offsetof(ChimiCallable, parameters_length) == 16, "layout: offsetof(ChimiCallable, parameters_length)");
_Static_assert(sizeof(((ChimiCallable *)0)->parameters_length) == 4, "layout: sizeof(((ChimiCallable *)0)->parameters_length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->parameters_length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->parameters_length)");
_Static_assert(offsetof(ChimiCallable, local_count) == 20, "layout: offsetof(ChimiCallable, local_count)");
_Static_assert(sizeof(((ChimiCallable *)0)->local_count) == 4, "layout: sizeof(((ChimiCallable *)0)->local_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->local_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->local_count)");
_Static_assert(offsetof(ChimiCallable, environment) == 24, "layout: offsetof(ChimiCallable, environment)");
_Static_assert(sizeof(((ChimiCallable *)0)->environment) == 8, "layout: sizeof(((ChimiCallable *)0)->environment)");
_Static_assert(offsetof(ChimiCallable, inner) == 32, "layout: offsetof(ChimiCallable, inner)");
_Static_assert(sizeof(((ChimiCallable *)0)->inner) == 8, "layout: sizeof(((ChimiCallable *)0)->inner)");
_Static_assert(offsetof(ChimiCallable, requirement) == 40, "layout: offsetof(ChimiCallable, requirement)");
_Static_assert(sizeof(((ChimiCallable *)0)->requirement) == 4, "layout: sizeof(((ChimiCallable *)0)->requirement)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->requirement) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->requirement)");
_Static_assert(offsetof(ChimiCallable, interface_symbol) == 44, "layout: offsetof(ChimiCallable, interface_symbol)");
_Static_assert(sizeof(((ChimiCallable *)0)->interface_symbol) == 4, "layout: sizeof(((ChimiCallable *)0)->interface_symbol)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->interface_symbol) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->interface_symbol)");
_Static_assert(offsetof(ChimiCallable, method) == 48, "layout: offsetof(ChimiCallable, method)");
_Static_assert(sizeof(((ChimiCallable *)0)->method) == 4, "layout: sizeof(((ChimiCallable *)0)->method)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->method) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->method)");
_Static_assert(offsetof(ChimiCallable, name) == 56, "layout: offsetof(ChimiCallable, name)");
_Static_assert(sizeof(((ChimiCallable *)0)->name) == 8, "layout: sizeof(((ChimiCallable *)0)->name)");
_Static_assert(offsetof(ChimiCallable, intrinsic) == 64, "layout: offsetof(ChimiCallable, intrinsic)");
_Static_assert(sizeof(((ChimiCallable *)0)->intrinsic) == 4, "layout: sizeof(((ChimiCallable *)0)->intrinsic)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->intrinsic) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->intrinsic)");
_Static_assert(offsetof(ChimiCallable, host) == 68, "layout: offsetof(ChimiCallable, host)");
_Static_assert(sizeof(((ChimiCallable *)0)->host) == 4, "layout: sizeof(((ChimiCallable *)0)->host)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->host) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->host)");
_Static_assert(offsetof(ChimiCallable, shift) == 72, "layout: offsetof(ChimiCallable, shift)");
_Static_assert(sizeof(((ChimiCallable *)0)->shift) == 8, "layout: sizeof(((ChimiCallable *)0)->shift)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->shift) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiCallable *)0)->shift)");
_Static_assert(sizeof(ChimiControl) == 40, "layout: sizeof(ChimiControl)");
_Static_assert(offsetof(ChimiControl, kind) == 0, "layout: offsetof(ChimiControl, kind)");
_Static_assert(sizeof(((ChimiControl *)0)->kind) == 4, "layout: sizeof(((ChimiControl *)0)->kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiControl *)0)->kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiControl *)0)->kind)");
_Static_assert(offsetof(ChimiControl, value) == 8, "layout: offsetof(ChimiControl, value)");
_Static_assert(sizeof(((ChimiControl *)0)->value) == 8, "layout: sizeof(((ChimiControl *)0)->value)");
_Static_assert(offsetof(ChimiControl, message) == 16, "layout: offsetof(ChimiControl, message)");
_Static_assert(sizeof(((ChimiControl *)0)->message) == 8, "layout: sizeof(((ChimiControl *)0)->message)");
_Static_assert(offsetof(ChimiControl, node) == 24, "layout: offsetof(ChimiControl, node)");
_Static_assert(sizeof(((ChimiControl *)0)->node) == 8, "layout: sizeof(((ChimiControl *)0)->node)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiControl *)0)->node) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiControl *)0)->node)");
_Static_assert(offsetof(ChimiControl, cause) == 32, "layout: offsetof(ChimiControl, cause)");
_Static_assert(sizeof(((ChimiControl *)0)->cause) == 8, "layout: sizeof(((ChimiControl *)0)->cause)");
_Static_assert(sizeof(ChimiValue) == 48, "layout: sizeof(ChimiValue)");
_Static_assert(_Alignof(ChimiValue) == 8, "layout: _Alignof(ChimiValue)");
_Static_assert(offsetof(ChimiValue, kind) == 0, "layout: offsetof(ChimiValue, kind)");
_Static_assert(sizeof(((ChimiValue *)0)->kind) == 4, "layout: sizeof(((ChimiValue *)0)->kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->kind)");
_Static_assert(offsetof(ChimiValue, as) == 8, "layout: offsetof(ChimiValue, as)");
_Static_assert(sizeof(((ChimiValue *)0)->as) == 40, "layout: sizeof(((ChimiValue *)0)->as)");
_Static_assert(offsetof(ChimiValue, as.integer) == 8, "layout: offsetof(ChimiValue, as.integer)");
_Static_assert(sizeof(((ChimiValue *)0)->as.integer) == 8, "layout: sizeof(((ChimiValue *)0)->as.integer)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.integer) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.integer)");
_Static_assert(offsetof(ChimiValue, as.floating) == 8, "layout: offsetof(ChimiValue, as.floating)");
_Static_assert(sizeof(((ChimiValue *)0)->as.floating) == 8, "layout: sizeof(((ChimiValue *)0)->as.floating)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.floating) == 4, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.floating)");
_Static_assert(offsetof(ChimiValue, as.boolean) == 8, "layout: offsetof(ChimiValue, as.boolean)");
_Static_assert(sizeof(((ChimiValue *)0)->as.boolean) == 1, "layout: sizeof(((ChimiValue *)0)->as.boolean)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.boolean) == 3, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.boolean)");
_Static_assert(offsetof(ChimiValue, as.callable) == 8, "layout: offsetof(ChimiValue, as.callable)");
_Static_assert(sizeof(((ChimiValue *)0)->as.callable) == 8, "layout: sizeof(((ChimiValue *)0)->as.callable)");
_Static_assert(offsetof(ChimiValue, as.map) == 8, "layout: offsetof(ChimiValue, as.map)");
_Static_assert(sizeof(((ChimiValue *)0)->as.map) == 8, "layout: sizeof(((ChimiValue *)0)->as.map)");
_Static_assert(offsetof(ChimiValue, as.json_number) == 8, "layout: offsetof(ChimiValue, as.json_number)");
_Static_assert(sizeof(((ChimiValue *)0)->as.json_number) == 8, "layout: sizeof(((ChimiValue *)0)->as.json_number)");
_Static_assert(offsetof(ChimiValue, as.task) == 8, "layout: offsetof(ChimiValue, as.task)");
_Static_assert(sizeof(((ChimiValue *)0)->as.task) == 8, "layout: sizeof(((ChimiValue *)0)->as.task)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.task) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.task)");
_Static_assert(offsetof(ChimiValue, as.text) == 8, "layout: offsetof(ChimiValue, as.text)");
_Static_assert(sizeof(((ChimiValue *)0)->as.text) == 16, "layout: sizeof(((ChimiValue *)0)->as.text)");
_Static_assert(offsetof(ChimiValue, as.text.bytes) == 8, "layout: offsetof(ChimiValue, as.text.bytes)");
_Static_assert(sizeof(((ChimiValue *)0)->as.text.bytes) == 8, "layout: sizeof(((ChimiValue *)0)->as.text.bytes)");
_Static_assert(offsetof(ChimiValue, as.text.length) == 16, "layout: offsetof(ChimiValue, as.text.length)");
_Static_assert(sizeof(((ChimiValue *)0)->as.text.length) == 8, "layout: sizeof(((ChimiValue *)0)->as.text.length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.text.length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.text.length)");
_Static_assert(offsetof(ChimiValue, as.list.vector.root) == 8, "layout: offsetof(ChimiValue, as.list.vector.root)");
_Static_assert(sizeof(((ChimiValue *)0)->as.list.vector.root) == 8, "layout: sizeof(((ChimiValue *)0)->as.list.vector.root)");
_Static_assert(offsetof(ChimiValue, as.list.vector.tail) == 16, "layout: offsetof(ChimiValue, as.list.vector.tail)");
_Static_assert(sizeof(((ChimiValue *)0)->as.list.vector.tail) == 8, "layout: sizeof(((ChimiValue *)0)->as.list.vector.tail)");
_Static_assert(offsetof(ChimiValue, as.list.vector.length) == 24, "layout: offsetof(ChimiValue, as.list.vector.length)");
_Static_assert(sizeof(((ChimiValue *)0)->as.list.vector.length) == 4, "layout: sizeof(((ChimiValue *)0)->as.list.vector.length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.list.vector.length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.list.vector.length)");
_Static_assert(offsetof(ChimiValue, as.list.start) == 32, "layout: offsetof(ChimiValue, as.list.start)");
_Static_assert(sizeof(((ChimiValue *)0)->as.list.start) == 4, "layout: sizeof(((ChimiValue *)0)->as.list.start)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.list.start) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.list.start)");
_Static_assert(offsetof(ChimiValue, as.list.length) == 36, "layout: offsetof(ChimiValue, as.list.length)");
_Static_assert(sizeof(((ChimiValue *)0)->as.list.length) == 4, "layout: sizeof(((ChimiValue *)0)->as.list.length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.list.length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.list.length)");
_Static_assert(offsetof(ChimiValue, as.record.fields) == 8, "layout: offsetof(ChimiValue, as.record.fields)");
_Static_assert(sizeof(((ChimiValue *)0)->as.record.fields) == 8, "layout: sizeof(((ChimiValue *)0)->as.record.fields)");
_Static_assert(offsetof(ChimiValue, as.record.length) == 16, "layout: offsetof(ChimiValue, as.record.length)");
_Static_assert(sizeof(((ChimiValue *)0)->as.record.length) == 4, "layout: sizeof(((ChimiValue *)0)->as.record.length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.record.length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.record.length)");
_Static_assert(offsetof(ChimiValue, as.constructed.constructor) == 8, "layout: offsetof(ChimiValue, as.constructed.constructor)");
_Static_assert(sizeof(((ChimiValue *)0)->as.constructed.constructor) == 4, "layout: sizeof(((ChimiValue *)0)->as.constructed.constructor)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.constructed.constructor) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.constructed.constructor)");
_Static_assert(offsetof(ChimiValue, as.constructed.type_name) == 16, "layout: offsetof(ChimiValue, as.constructed.type_name)");
_Static_assert(sizeof(((ChimiValue *)0)->as.constructed.type_name) == 8, "layout: sizeof(((ChimiValue *)0)->as.constructed.type_name)");
_Static_assert(offsetof(ChimiValue, as.constructed.variant_name) == 24, "layout: offsetof(ChimiValue, as.constructed.variant_name)");
_Static_assert(sizeof(((ChimiValue *)0)->as.constructed.variant_name) == 8, "layout: sizeof(((ChimiValue *)0)->as.constructed.variant_name)");
_Static_assert(offsetof(ChimiValue, as.constructed.fields) == 32, "layout: offsetof(ChimiValue, as.constructed.fields)");
_Static_assert(sizeof(((ChimiValue *)0)->as.constructed.fields) == 8, "layout: sizeof(((ChimiValue *)0)->as.constructed.fields)");
_Static_assert(offsetof(ChimiValue, as.constructed.length) == 40, "layout: offsetof(ChimiValue, as.constructed.length)");
_Static_assert(sizeof(((ChimiValue *)0)->as.constructed.length) == 4, "layout: sizeof(((ChimiValue *)0)->as.constructed.length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.constructed.length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.constructed.length)");
_Static_assert(offsetof(ChimiValue, as.range.start) == 8, "layout: offsetof(ChimiValue, as.range.start)");
_Static_assert(sizeof(((ChimiValue *)0)->as.range.start) == 8, "layout: sizeof(((ChimiValue *)0)->as.range.start)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.range.start) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.range.start)");
_Static_assert(offsetof(ChimiValue, as.range.end) == 16, "layout: offsetof(ChimiValue, as.range.end)");
_Static_assert(sizeof(((ChimiValue *)0)->as.range.end) == 8, "layout: sizeof(((ChimiValue *)0)->as.range.end)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.range.end) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.range.end)");
_Static_assert(offsetof(ChimiValue, as.capability.channel) == 8, "layout: offsetof(ChimiValue, as.capability.channel)");
_Static_assert(sizeof(((ChimiValue *)0)->as.capability.channel) == 8, "layout: sizeof(((ChimiValue *)0)->as.capability.channel)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.capability.channel) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.capability.channel)");
_Static_assert(offsetof(ChimiValue, as.capability.owner) == 16, "layout: offsetof(ChimiValue, as.capability.owner)");
_Static_assert(sizeof(((ChimiValue *)0)->as.capability.owner) == 8, "layout: sizeof(((ChimiValue *)0)->as.capability.owner)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.capability.owner) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.capability.owner)");
_Static_assert(offsetof(ChimiValue, as.capability.token) == 24, "layout: offsetof(ChimiValue, as.capability.token)");
_Static_assert(sizeof(((ChimiValue *)0)->as.capability.token) == 8, "layout: sizeof(((ChimiValue *)0)->as.capability.token)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.capability.token) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.capability.token)");
_Static_assert(offsetof(ChimiValue, as.address.kind) == 8, "layout: offsetof(ChimiValue, as.address.kind)");
_Static_assert(sizeof(((ChimiValue *)0)->as.address.kind) == 4, "layout: sizeof(((ChimiValue *)0)->as.address.kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.address.kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.address.kind)");
_Static_assert(offsetof(ChimiValue, as.address.target) == 16, "layout: offsetof(ChimiValue, as.address.target)");
_Static_assert(sizeof(((ChimiValue *)0)->as.address.target) == 8, "layout: sizeof(((ChimiValue *)0)->as.address.target)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.address.target) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.address.target)");
_Static_assert(offsetof(ChimiValue, as.managed.supervisor) == 8, "layout: offsetof(ChimiValue, as.managed.supervisor)");
_Static_assert(sizeof(((ChimiValue *)0)->as.managed.supervisor) == 8, "layout: sizeof(((ChimiValue *)0)->as.managed.supervisor)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.managed.supervisor) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.managed.supervisor)");
_Static_assert(offsetof(ChimiValue, as.managed.membership) == 16, "layout: offsetof(ChimiValue, as.managed.membership)");
_Static_assert(sizeof(((ChimiValue *)0)->as.managed.membership) == 8, "layout: sizeof(((ChimiValue *)0)->as.managed.membership)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.managed.membership) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.managed.membership)");
_Static_assert(offsetof(ChimiValue, as.decimal) == 8, "layout: offsetof(ChimiValue, as.decimal)");
_Static_assert(sizeof(((ChimiValue *)0)->as.decimal) == 24, "layout: sizeof(((ChimiValue *)0)->as.decimal)");
_Static_assert(offsetof(ChimiValue, as.handle.slot) == 8, "layout: offsetof(ChimiValue, as.handle.slot)");
_Static_assert(sizeof(((ChimiValue *)0)->as.handle.slot) == 4, "layout: sizeof(((ChimiValue *)0)->as.handle.slot)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.handle.slot) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.handle.slot)");
_Static_assert(offsetof(ChimiValue, as.handle.generation) == 12, "layout: offsetof(ChimiValue, as.handle.generation)");
_Static_assert(sizeof(((ChimiValue *)0)->as.handle.generation) == 4, "layout: sizeof(((ChimiValue *)0)->as.handle.generation)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.handle.generation) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiValue *)0)->as.handle.generation)");
_Static_assert(offsetof(ChimiValue, as.handle.name) == 16, "layout: offsetof(ChimiValue, as.handle.name)");
_Static_assert(sizeof(((ChimiValue *)0)->as.handle.name) == 8, "layout: sizeof(((ChimiValue *)0)->as.handle.name)");
_Static_assert(sizeof(AllocationHeader) == 40, "layout: sizeof(AllocationHeader)");
_Static_assert(_Alignof(AllocationHeader) == 8, "layout: _Alignof(AllocationHeader)");
_Static_assert(offsetof(AllocationHeader, next) == 0, "layout: offsetof(AllocationHeader, next)");
_Static_assert(sizeof(((AllocationHeader *)0)->next) == 8, "layout: sizeof(((AllocationHeader *)0)->next)");
_Static_assert(offsetof(AllocationHeader, area) == 8, "layout: offsetof(AllocationHeader, area)");
_Static_assert(sizeof(((AllocationHeader *)0)->area) == 8, "layout: sizeof(((AllocationHeader *)0)->area)");
_Static_assert(offsetof(AllocationHeader, bytes) == 16, "layout: offsetof(AllocationHeader, bytes)");
_Static_assert(sizeof(((AllocationHeader *)0)->bytes) == 8, "layout: sizeof(((AllocationHeader *)0)->bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationHeader *)0)->bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationHeader *)0)->bytes)");
_Static_assert(offsetof(AllocationHeader, mark_epoch) == 24, "layout: offsetof(AllocationHeader, mark_epoch)");
_Static_assert(sizeof(((AllocationHeader *)0)->mark_epoch) == 8, "layout: sizeof(((AllocationHeader *)0)->mark_epoch)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationHeader *)0)->mark_epoch) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationHeader *)0)->mark_epoch)");
_Static_assert(offsetof(AllocationHeader, kind) == 32, "layout: offsetof(AllocationHeader, kind)");
_Static_assert(sizeof(((AllocationHeader *)0)->kind) == 4, "layout: sizeof(((AllocationHeader *)0)->kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationHeader *)0)->kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationHeader *)0)->kind)");
_Static_assert(offsetof(AllocationHeader, in_chunk) == 36, "layout: offsetof(AllocationHeader, in_chunk)");
_Static_assert(sizeof(((AllocationHeader *)0)->in_chunk) == 1, "layout: sizeof(((AllocationHeader *)0)->in_chunk)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationHeader *)0)->in_chunk) == 3, "layout: CHIMI_LAYOUT_CLASS(((AllocationHeader *)0)->in_chunk)");
_Static_assert(sizeof(YoungChunk) == 16, "layout: sizeof(YoungChunk)");
_Static_assert(_Alignof(YoungChunk) == 16, "layout: _Alignof(YoungChunk)");
_Static_assert(offsetof(YoungChunk, next) == 0, "layout: offsetof(YoungChunk, next)");
_Static_assert(sizeof(((YoungChunk *)0)->next) == 8, "layout: sizeof(((YoungChunk *)0)->next)");
_Static_assert(offsetof(YoungChunk, used) == 8, "layout: offsetof(YoungChunk, used)");
_Static_assert(sizeof(((YoungChunk *)0)->used) == 8, "layout: sizeof(((YoungChunk *)0)->used)");
_Static_assert(offsetof(YoungChunk, data) == 16, "layout: offsetof(YoungChunk, data)");
_Static_assert(sizeof(AllocationArea) == 56, "layout: sizeof(AllocationArea)");
_Static_assert(offsetof(AllocationArea, role) == 0, "layout: offsetof(AllocationArea, role)");
_Static_assert(sizeof(((AllocationArea *)0)->role) == 4, "layout: sizeof(((AllocationArea *)0)->role)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationArea *)0)->role) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationArea *)0)->role)");
_Static_assert(offsetof(AllocationArea, heap) == 8, "layout: offsetof(AllocationArea, heap)");
_Static_assert(sizeof(((AllocationArea *)0)->heap) == 8, "layout: sizeof(((AllocationArea *)0)->heap)");
_Static_assert(offsetof(AllocationArea, scope) == 16, "layout: offsetof(AllocationArea, scope)");
_Static_assert(sizeof(((AllocationArea *)0)->scope) == 8, "layout: sizeof(((AllocationArea *)0)->scope)");
_Static_assert(offsetof(AllocationArea, allocations) == 24, "layout: offsetof(AllocationArea, allocations)");
_Static_assert(sizeof(((AllocationArea *)0)->allocations) == 8, "layout: sizeof(((AllocationArea *)0)->allocations)");
_Static_assert(offsetof(AllocationArea, bytes) == 32, "layout: offsetof(AllocationArea, bytes)");
_Static_assert(sizeof(((AllocationArea *)0)->bytes) == 8, "layout: sizeof(((AllocationArea *)0)->bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationArea *)0)->bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationArea *)0)->bytes)");
_Static_assert(offsetof(AllocationArea, count) == 40, "layout: offsetof(AllocationArea, count)");
_Static_assert(sizeof(((AllocationArea *)0)->count) == 8, "layout: sizeof(((AllocationArea *)0)->count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationArea *)0)->count) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationArea *)0)->count)");
_Static_assert(offsetof(AllocationArea, next) == 48, "layout: offsetof(AllocationArea, next)");
_Static_assert(sizeof(((AllocationArea *)0)->next) == 8, "layout: sizeof(((AllocationArea *)0)->next)");
_Static_assert(sizeof(AllocationScope) == 344, "layout: sizeof(AllocationScope)");
_Static_assert(offsetof(AllocationScope, parent) == 0, "layout: offsetof(AllocationScope, parent)");
_Static_assert(sizeof(((AllocationScope *)0)->parent) == 8, "layout: sizeof(((AllocationScope *)0)->parent)");
_Static_assert(offsetof(AllocationScope, heap) == 8, "layout: offsetof(AllocationScope, heap)");
_Static_assert(sizeof(((AllocationScope *)0)->heap) == 8, "layout: sizeof(((AllocationScope *)0)->heap)");
_Static_assert(offsetof(AllocationScope, young) == 16, "layout: offsetof(AllocationScope, young)");
_Static_assert(sizeof(((AllocationScope *)0)->young) == 56, "layout: sizeof(((AllocationScope *)0)->young)");
_Static_assert(offsetof(AllocationScope, adopted) == 72, "layout: offsetof(AllocationScope, adopted)");
_Static_assert(sizeof(((AllocationScope *)0)->adopted) == 8, "layout: sizeof(((AllocationScope *)0)->adopted)");
_Static_assert(offsetof(AllocationScope, survivors) == 80, "layout: offsetof(AllocationScope, survivors)");
_Static_assert(sizeof(((AllocationScope *)0)->survivors) == 56, "layout: sizeof(((AllocationScope *)0)->survivors)");
_Static_assert(offsetof(AllocationScope, full_baseline) == 136, "layout: offsetof(AllocationScope, full_baseline)");
_Static_assert(sizeof(((AllocationScope *)0)->full_baseline) == 8, "layout: sizeof(((AllocationScope *)0)->full_baseline)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->full_baseline) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->full_baseline)");
_Static_assert(offsetof(AllocationScope, has_baseline) == 144, "layout: offsetof(AllocationScope, has_baseline)");
_Static_assert(sizeof(((AllocationScope *)0)->has_baseline) == 1, "layout: sizeof(((AllocationScope *)0)->has_baseline)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->has_baseline) == 3, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->has_baseline)");
_Static_assert(offsetof(AllocationScope, first_chunk) == 152, "layout: offsetof(AllocationScope, first_chunk)");
_Static_assert(sizeof(((AllocationScope *)0)->first_chunk) == 8, "layout: sizeof(((AllocationScope *)0)->first_chunk)");
_Static_assert(offsetof(AllocationScope, last_chunk) == 160, "layout: offsetof(AllocationScope, last_chunk)");
_Static_assert(sizeof(((AllocationScope *)0)->last_chunk) == 8, "layout: sizeof(((AllocationScope *)0)->last_chunk)");
_Static_assert(offsetof(AllocationScope, bump) == 168, "layout: offsetof(AllocationScope, bump)");
_Static_assert(sizeof(((AllocationScope *)0)->bump) == 8, "layout: sizeof(((AllocationScope *)0)->bump)");
_Static_assert(offsetof(AllocationScope, bump_end) == 176, "layout: offsetof(AllocationScope, bump_end)");
_Static_assert(sizeof(((AllocationScope *)0)->bump_end) == 8, "layout: sizeof(((AllocationScope *)0)->bump_end)");
_Static_assert(offsetof(AllocationScope, pending_bytes) == 184, "layout: offsetof(AllocationScope, pending_bytes)");
_Static_assert(sizeof(((AllocationScope *)0)->pending_bytes) == 8, "layout: sizeof(((AllocationScope *)0)->pending_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->pending_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->pending_bytes)");
_Static_assert(offsetof(AllocationScope, pending_count) == 192, "layout: offsetof(AllocationScope, pending_count)");
_Static_assert(sizeof(((AllocationScope *)0)->pending_count) == 8, "layout: sizeof(((AllocationScope *)0)->pending_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->pending_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->pending_count)");
_Static_assert(offsetof(AllocationScope, links_young) == 200, "layout: offsetof(AllocationScope, links_young)");
_Static_assert(sizeof(((AllocationScope *)0)->links_young) == 1, "layout: sizeof(((AllocationScope *)0)->links_young)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->links_young) == 3, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->links_young)");
_Static_assert(offsetof(AllocationScope, live_bytes) == 208, "layout: offsetof(AllocationScope, live_bytes)");
_Static_assert(sizeof(((AllocationScope *)0)->live_bytes) == 8, "layout: sizeof(((AllocationScope *)0)->live_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->live_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->live_bytes)");
_Static_assert(offsetof(AllocationScope, peak_live_bytes) == 216, "layout: offsetof(AllocationScope, peak_live_bytes)");
_Static_assert(sizeof(((AllocationScope *)0)->peak_live_bytes) == 8, "layout: sizeof(((AllocationScope *)0)->peak_live_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->peak_live_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->peak_live_bytes)");
_Static_assert(offsetof(AllocationScope, total_allocated_bytes) == 224, "layout: offsetof(AllocationScope, total_allocated_bytes)");
_Static_assert(sizeof(((AllocationScope *)0)->total_allocated_bytes) == 8, "layout: sizeof(((AllocationScope *)0)->total_allocated_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->total_allocated_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->total_allocated_bytes)");
_Static_assert(offsetof(AllocationScope, allocated_bytes_by_kind) == 232, "layout: offsetof(AllocationScope, allocated_bytes_by_kind)");
_Static_assert(sizeof(((AllocationScope *)0)->allocated_bytes_by_kind) == 80, "layout: sizeof(((AllocationScope *)0)->allocated_bytes_by_kind)");
_Static_assert(offsetof(AllocationScope, collections) == 312, "layout: offsetof(AllocationScope, collections)");
_Static_assert(sizeof(((AllocationScope *)0)->collections) == 8, "layout: sizeof(((AllocationScope *)0)->collections)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->collections) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->collections)");
_Static_assert(offsetof(AllocationScope, full_collections) == 320, "layout: offsetof(AllocationScope, full_collections)");
_Static_assert(sizeof(((AllocationScope *)0)->full_collections) == 8, "layout: sizeof(((AllocationScope *)0)->full_collections)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->full_collections) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->full_collections)");
_Static_assert(offsetof(AllocationScope, scanned_allocations) == 328, "layout: offsetof(AllocationScope, scanned_allocations)");
_Static_assert(sizeof(((AllocationScope *)0)->scanned_allocations) == 8, "layout: sizeof(((AllocationScope *)0)->scanned_allocations)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->scanned_allocations) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->scanned_allocations)");
_Static_assert(offsetof(AllocationScope, promoted_bytes) == 336, "layout: offsetof(AllocationScope, promoted_bytes)");
_Static_assert(sizeof(((AllocationScope *)0)->promoted_bytes) == 8, "layout: sizeof(((AllocationScope *)0)->promoted_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->promoted_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((AllocationScope *)0)->promoted_bytes)");
_Static_assert(sizeof(RuntimeHeap) == 104, "layout: sizeof(RuntimeHeap)");
_Static_assert(offsetof(RuntimeHeap, owner) == 0, "layout: offsetof(RuntimeHeap, owner)");
_Static_assert(sizeof(((RuntimeHeap *)0)->owner) == 8, "layout: sizeof(((RuntimeHeap *)0)->owner)");
_Static_assert(CHIMI_LAYOUT_CLASS(((RuntimeHeap *)0)->owner) == 2, "layout: CHIMI_LAYOUT_CLASS(((RuntimeHeap *)0)->owner)");
_Static_assert(offsetof(RuntimeHeap, profiled_node_plus_one) == 8, "layout: offsetof(RuntimeHeap, profiled_node_plus_one)");
_Static_assert(sizeof(((RuntimeHeap *)0)->profiled_node_plus_one) == 4, "layout: sizeof(((RuntimeHeap *)0)->profiled_node_plus_one)");
_Static_assert(CHIMI_LAYOUT_CLASS(((RuntimeHeap *)0)->profiled_node_plus_one) == 2, "layout: CHIMI_LAYOUT_CLASS(((RuntimeHeap *)0)->profiled_node_plus_one)");
_Static_assert(offsetof(RuntimeHeap, live_bytes) == 16, "layout: offsetof(RuntimeHeap, live_bytes)");
_Static_assert(sizeof(((RuntimeHeap *)0)->live_bytes) == 8, "layout: sizeof(((RuntimeHeap *)0)->live_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((RuntimeHeap *)0)->live_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((RuntimeHeap *)0)->live_bytes)");
_Static_assert(offsetof(RuntimeHeap, peak_live_bytes) == 24, "layout: offsetof(RuntimeHeap, peak_live_bytes)");
_Static_assert(sizeof(((RuntimeHeap *)0)->peak_live_bytes) == 8, "layout: sizeof(((RuntimeHeap *)0)->peak_live_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((RuntimeHeap *)0)->peak_live_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((RuntimeHeap *)0)->peak_live_bytes)");
_Static_assert(offsetof(RuntimeHeap, innermost) == 32, "layout: offsetof(RuntimeHeap, innermost)");
_Static_assert(sizeof(((RuntimeHeap *)0)->innermost) == 8, "layout: sizeof(((RuntimeHeap *)0)->innermost)");
_Static_assert(offsetof(RuntimeHeap, base) == 40, "layout: offsetof(RuntimeHeap, base)");
_Static_assert(sizeof(((RuntimeHeap *)0)->base) == 56, "layout: sizeof(((RuntimeHeap *)0)->base)");
_Static_assert(offsetof(RuntimeHeap, adopted) == 96, "layout: offsetof(RuntimeHeap, adopted)");
_Static_assert(sizeof(((RuntimeHeap *)0)->adopted) == 8, "layout: sizeof(((RuntimeHeap *)0)->adopted)");
_Static_assert(sizeof(HeapTotals) == 64, "layout: sizeof(HeapTotals)");
_Static_assert(offsetof(HeapTotals, live_allocations) == 0, "layout: offsetof(HeapTotals, live_allocations)");
_Static_assert(sizeof(((HeapTotals *)0)->live_allocations) == 8, "layout: sizeof(((HeapTotals *)0)->live_allocations)");
_Static_assert(CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->live_allocations) == 1, "layout: CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->live_allocations)");
_Static_assert(offsetof(HeapTotals, footprint_bytes) == 8, "layout: offsetof(HeapTotals, footprint_bytes)");
_Static_assert(sizeof(((HeapTotals *)0)->footprint_bytes) == 8, "layout: sizeof(((HeapTotals *)0)->footprint_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->footprint_bytes) == 1, "layout: CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->footprint_bytes)");
_Static_assert(offsetof(HeapTotals, peak_footprint_bytes) == 16, "layout: offsetof(HeapTotals, peak_footprint_bytes)");
_Static_assert(sizeof(((HeapTotals *)0)->peak_footprint_bytes) == 8, "layout: sizeof(((HeapTotals *)0)->peak_footprint_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->peak_footprint_bytes) == 1, "layout: CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->peak_footprint_bytes)");
_Static_assert(offsetof(HeapTotals, live_fragments) == 24, "layout: offsetof(HeapTotals, live_fragments)");
_Static_assert(sizeof(((HeapTotals *)0)->live_fragments) == 8, "layout: sizeof(((HeapTotals *)0)->live_fragments)");
_Static_assert(CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->live_fragments) == 1, "layout: CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->live_fragments)");
_Static_assert(offsetof(HeapTotals, fragment_footprint_bytes) == 32, "layout: offsetof(HeapTotals, fragment_footprint_bytes)");
_Static_assert(sizeof(((HeapTotals *)0)->fragment_footprint_bytes) == 8, "layout: sizeof(((HeapTotals *)0)->fragment_footprint_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->fragment_footprint_bytes) == 1, "layout: CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->fragment_footprint_bytes)");
_Static_assert(offsetof(HeapTotals, peak_fragment_footprint_bytes) == 40, "layout: offsetof(HeapTotals, peak_fragment_footprint_bytes)");
_Static_assert(sizeof(((HeapTotals *)0)->peak_fragment_footprint_bytes) == 8, "layout: sizeof(((HeapTotals *)0)->peak_fragment_footprint_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->peak_fragment_footprint_bytes) == 1, "layout: CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->peak_fragment_footprint_bytes)");
_Static_assert(offsetof(HeapTotals, live_heaps) == 48, "layout: offsetof(HeapTotals, live_heaps)");
_Static_assert(sizeof(((HeapTotals *)0)->live_heaps) == 8, "layout: sizeof(((HeapTotals *)0)->live_heaps)");
_Static_assert(CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->live_heaps) == 1, "layout: CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->live_heaps)");
_Static_assert(offsetof(HeapTotals, cached_bytes) == 56, "layout: offsetof(HeapTotals, cached_bytes)");
_Static_assert(sizeof(((HeapTotals *)0)->cached_bytes) == 8, "layout: sizeof(((HeapTotals *)0)->cached_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->cached_bytes) == 1, "layout: CHIMI_LAYOUT_CLASS(((HeapTotals *)0)->cached_bytes)");
_Static_assert(sizeof(ScopeTotals) == 128, "layout: sizeof(ScopeTotals)");
_Static_assert(offsetof(ScopeTotals, total_allocated_bytes) == 0, "layout: offsetof(ScopeTotals, total_allocated_bytes)");
_Static_assert(sizeof(((ScopeTotals *)0)->total_allocated_bytes) == 8, "layout: sizeof(((ScopeTotals *)0)->total_allocated_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->total_allocated_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->total_allocated_bytes)");
_Static_assert(offsetof(ScopeTotals, allocated_bytes_by_kind) == 8, "layout: offsetof(ScopeTotals, allocated_bytes_by_kind)");
_Static_assert(sizeof(((ScopeTotals *)0)->allocated_bytes_by_kind) == 80, "layout: sizeof(((ScopeTotals *)0)->allocated_bytes_by_kind)");
_Static_assert(offsetof(ScopeTotals, peak_live_bytes) == 88, "layout: offsetof(ScopeTotals, peak_live_bytes)");
_Static_assert(sizeof(((ScopeTotals *)0)->peak_live_bytes) == 8, "layout: sizeof(((ScopeTotals *)0)->peak_live_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->peak_live_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->peak_live_bytes)");
_Static_assert(offsetof(ScopeTotals, retained_bytes) == 96, "layout: offsetof(ScopeTotals, retained_bytes)");
_Static_assert(sizeof(((ScopeTotals *)0)->retained_bytes) == 8, "layout: sizeof(((ScopeTotals *)0)->retained_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->retained_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->retained_bytes)");
_Static_assert(offsetof(ScopeTotals, collections) == 104, "layout: offsetof(ScopeTotals, collections)");
_Static_assert(sizeof(((ScopeTotals *)0)->collections) == 8, "layout: sizeof(((ScopeTotals *)0)->collections)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->collections) == 2, "layout: CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->collections)");
_Static_assert(offsetof(ScopeTotals, full_collections) == 112, "layout: offsetof(ScopeTotals, full_collections)");
_Static_assert(sizeof(((ScopeTotals *)0)->full_collections) == 8, "layout: sizeof(((ScopeTotals *)0)->full_collections)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->full_collections) == 2, "layout: CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->full_collections)");
_Static_assert(offsetof(ScopeTotals, scanned_allocations) == 120, "layout: offsetof(ScopeTotals, scanned_allocations)");
_Static_assert(sizeof(((ScopeTotals *)0)->scanned_allocations) == 8, "layout: sizeof(((ScopeTotals *)0)->scanned_allocations)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->scanned_allocations) == 2, "layout: CHIMI_LAYOUT_CLASS(((ScopeTotals *)0)->scanned_allocations)");
_Static_assert(sizeof(FiberContext) == 16, "layout: sizeof(FiberContext)");
_Static_assert(offsetof(FiberContext, heap) == 0, "layout: offsetof(FiberContext, heap)");
_Static_assert(sizeof(((FiberContext *)0)->heap) == 8, "layout: sizeof(((FiberContext *)0)->heap)");
_Static_assert(offsetof(FiberContext, target) == 8, "layout: offsetof(FiberContext, target)");
_Static_assert(sizeof(((FiberContext *)0)->target) == 8, "layout: sizeof(((FiberContext *)0)->target)");
_Static_assert(sizeof(SmallBlock) == 8, "layout: sizeof(SmallBlock)");
_Static_assert(offsetof(SmallBlock, next) == 0, "layout: offsetof(SmallBlock, next)");
_Static_assert(sizeof(((SmallBlock *)0)->next) == 8, "layout: sizeof(((SmallBlock *)0)->next)");
_Static_assert(sizeof(SmallBlockCache) == 192, "layout: sizeof(SmallBlockCache)");
_Static_assert(offsetof(SmallBlockCache, lists) == 0, "layout: offsetof(SmallBlockCache, lists)");
_Static_assert(sizeof(((SmallBlockCache *)0)->lists) == 128, "layout: sizeof(((SmallBlockCache *)0)->lists)");
_Static_assert(offsetof(SmallBlockCache, counts) == 128, "layout: offsetof(SmallBlockCache, counts)");
_Static_assert(sizeof(((SmallBlockCache *)0)->counts) == 64, "layout: sizeof(((SmallBlockCache *)0)->counts)");
_Static_assert(sizeof(PointerStack) == 24, "layout: sizeof(PointerStack)");
_Static_assert(offsetof(PointerStack, items) == 0, "layout: offsetof(PointerStack, items)");
_Static_assert(sizeof(((PointerStack *)0)->items) == 8, "layout: sizeof(((PointerStack *)0)->items)");
_Static_assert(offsetof(PointerStack, length) == 8, "layout: offsetof(PointerStack, length)");
_Static_assert(sizeof(((PointerStack *)0)->length) == 8, "layout: sizeof(((PointerStack *)0)->length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((PointerStack *)0)->length) == 2, "layout: CHIMI_LAYOUT_CLASS(((PointerStack *)0)->length)");
_Static_assert(offsetof(PointerStack, capacity) == 16, "layout: offsetof(PointerStack, capacity)");
_Static_assert(sizeof(((PointerStack *)0)->capacity) == 8, "layout: sizeof(((PointerStack *)0)->capacity)");
_Static_assert(CHIMI_LAYOUT_CLASS(((PointerStack *)0)->capacity) == 2, "layout: CHIMI_LAYOUT_CLASS(((PointerStack *)0)->capacity)");
_Static_assert(sizeof(Carrier) == 656, "layout: sizeof(Carrier)");
_Static_assert(offsetof(Carrier, fiber) == 0, "layout: offsetof(Carrier, fiber)");
_Static_assert(sizeof(((Carrier *)0)->fiber) == 16, "layout: sizeof(((Carrier *)0)->fiber)");
_Static_assert(offsetof(Carrier, heap_totals) == 16, "layout: offsetof(Carrier, heap_totals)");
_Static_assert(sizeof(((Carrier *)0)->heap_totals) == 64, "layout: sizeof(((Carrier *)0)->heap_totals)");
_Static_assert(offsetof(Carrier, pending_footprint_bytes) == 80, "layout: offsetof(Carrier, pending_footprint_bytes)");
_Static_assert(sizeof(((Carrier *)0)->pending_footprint_bytes) == 8, "layout: sizeof(((Carrier *)0)->pending_footprint_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->pending_footprint_bytes) == 1, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->pending_footprint_bytes)");
_Static_assert(offsetof(Carrier, folded_footprint_bytes) == 88, "layout: offsetof(Carrier, folded_footprint_bytes)");
_Static_assert(sizeof(((Carrier *)0)->folded_footprint_bytes) == 8, "layout: sizeof(((Carrier *)0)->folded_footprint_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->folded_footprint_bytes) == 1, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->folded_footprint_bytes)");
_Static_assert(offsetof(Carrier, folded_fragment_footprint_bytes) == 96, "layout: offsetof(Carrier, folded_fragment_footprint_bytes)");
_Static_assert(sizeof(((Carrier *)0)->folded_fragment_footprint_bytes) == 8, "layout: sizeof(((Carrier *)0)->folded_fragment_footprint_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->folded_fragment_footprint_bytes) == 1, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->folded_fragment_footprint_bytes)");
_Static_assert(offsetof(Carrier, unscoped_allocated_bytes) == 104, "layout: offsetof(Carrier, unscoped_allocated_bytes)");
_Static_assert(sizeof(((Carrier *)0)->unscoped_allocated_bytes) == 8, "layout: sizeof(((Carrier *)0)->unscoped_allocated_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->unscoped_allocated_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->unscoped_allocated_bytes)");
_Static_assert(offsetof(Carrier, capture_transfer_probes) == 112, "layout: offsetof(Carrier, capture_transfer_probes)");
_Static_assert(sizeof(((Carrier *)0)->capture_transfer_probes) == 8, "layout: sizeof(((Carrier *)0)->capture_transfer_probes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->capture_transfer_probes) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->capture_transfer_probes)");
_Static_assert(offsetof(Carrier, process_heap_peak_value_bytes) == 120, "layout: offsetof(Carrier, process_heap_peak_value_bytes)");
_Static_assert(sizeof(((Carrier *)0)->process_heap_peak_value_bytes) == 8, "layout: sizeof(((Carrier *)0)->process_heap_peak_value_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->process_heap_peak_value_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->process_heap_peak_value_bytes)");
_Static_assert(offsetof(Carrier, copy_counting) == 128, "layout: offsetof(Carrier, copy_counting)");
_Static_assert(sizeof(((Carrier *)0)->copy_counting) == 1, "layout: sizeof(((Carrier *)0)->copy_counting)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->copy_counting) == 3, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->copy_counting)");
_Static_assert(offsetof(Carrier, copied_values) == 136, "layout: offsetof(Carrier, copied_values)");
_Static_assert(sizeof(((Carrier *)0)->copied_values) == 8, "layout: sizeof(((Carrier *)0)->copied_values)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->copied_values) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->copied_values)");
_Static_assert(offsetof(Carrier, copied_bytes) == 144, "layout: offsetof(Carrier, copied_bytes)");
_Static_assert(sizeof(((Carrier *)0)->copied_bytes) == 8, "layout: sizeof(((Carrier *)0)->copied_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->copied_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->copied_bytes)");
_Static_assert(offsetof(Carrier, fragment_copy_foreign_owner) == 152, "layout: offsetof(Carrier, fragment_copy_foreign_owner)");
_Static_assert(sizeof(((Carrier *)0)->fragment_copy_foreign_owner) == 8, "layout: sizeof(((Carrier *)0)->fragment_copy_foreign_owner)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->fragment_copy_foreign_owner) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->fragment_copy_foreign_owner)");
_Static_assert(offsetof(Carrier, small_blocks) == 160, "layout: offsetof(Carrier, small_blocks)");
_Static_assert(sizeof(((Carrier *)0)->small_blocks) == 192, "layout: sizeof(((Carrier *)0)->small_blocks)");
_Static_assert(offsetof(Carrier, young_chunk_cache) == 352, "layout: offsetof(Carrier, young_chunk_cache)");
_Static_assert(sizeof(((Carrier *)0)->young_chunk_cache) == 8, "layout: sizeof(((Carrier *)0)->young_chunk_cache)");
_Static_assert(offsetof(Carrier, young_chunk_cache_count) == 360, "layout: offsetof(Carrier, young_chunk_cache_count)");
_Static_assert(sizeof(((Carrier *)0)->young_chunk_cache_count) == 4, "layout: sizeof(((Carrier *)0)->young_chunk_cache_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->young_chunk_cache_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->young_chunk_cache_count)");
_Static_assert(offsetof(Carrier, collecting_scope) == 368, "layout: offsetof(Carrier, collecting_scope)");
_Static_assert(sizeof(((Carrier *)0)->collecting_scope) == 8, "layout: sizeof(((Carrier *)0)->collecting_scope)");
_Static_assert(offsetof(Carrier, collecting_full) == 376, "layout: offsetof(Carrier, collecting_full)");
_Static_assert(sizeof(((Carrier *)0)->collecting_full) == 1, "layout: sizeof(((Carrier *)0)->collecting_full)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->collecting_full) == 3, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->collecting_full)");
_Static_assert(offsetof(Carrier, collecting_foreign) == 377, "layout: offsetof(Carrier, collecting_foreign)");
_Static_assert(sizeof(((Carrier *)0)->collecting_foreign) == 1, "layout: sizeof(((Carrier *)0)->collecting_foreign)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->collecting_foreign) == 3, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->collecting_foreign)");
_Static_assert(offsetof(Carrier, collecting_epoch) == 384, "layout: offsetof(Carrier, collecting_epoch)");
_Static_assert(sizeof(((Carrier *)0)->collecting_epoch) == 8, "layout: sizeof(((Carrier *)0)->collecting_epoch)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->collecting_epoch) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->collecting_epoch)");
_Static_assert(offsetof(Carrier, next_epoch) == 392, "layout: offsetof(Carrier, next_epoch)");
_Static_assert(sizeof(((Carrier *)0)->next_epoch) == 8, "layout: sizeof(((Carrier *)0)->next_epoch)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->next_epoch) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->next_epoch)");
_Static_assert(offsetof(Carrier, end_epoch) == 400, "layout: offsetof(Carrier, end_epoch)");
_Static_assert(sizeof(((Carrier *)0)->end_epoch) == 8, "layout: sizeof(((Carrier *)0)->end_epoch)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->end_epoch) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->end_epoch)");
_Static_assert(offsetof(Carrier, next_span) == 408, "layout: offsetof(Carrier, next_span)");
_Static_assert(sizeof(((Carrier *)0)->next_span) == 8, "layout: sizeof(((Carrier *)0)->next_span)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->next_span) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->next_span)");
_Static_assert(offsetof(Carrier, end_span) == 416, "layout: offsetof(Carrier, end_span)");
_Static_assert(sizeof(((Carrier *)0)->end_span) == 8, "layout: sizeof(((Carrier *)0)->end_span)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->end_span) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->end_span)");
_Static_assert(offsetof(Carrier, mark_stack) == 424, "layout: offsetof(Carrier, mark_stack)");
_Static_assert(sizeof(((Carrier *)0)->mark_stack) == 24, "layout: sizeof(((Carrier *)0)->mark_stack)");
_Static_assert(offsetof(Carrier, copy_stack) == 448, "layout: offsetof(Carrier, copy_stack)");
_Static_assert(sizeof(((Carrier *)0)->copy_stack) == 24, "layout: sizeof(((Carrier *)0)->copy_stack)");
_Static_assert(offsetof(Carrier, copy_pending) == 472, "layout: offsetof(Carrier, copy_pending)");
_Static_assert(sizeof(((Carrier *)0)->copy_pending) == 24, "layout: sizeof(((Carrier *)0)->copy_pending)");
_Static_assert(offsetof(Carrier, fs_read_chunk) == 496, "layout: offsetof(Carrier, fs_read_chunk)");
_Static_assert(sizeof(((Carrier *)0)->fs_read_chunk) == 8, "layout: sizeof(((Carrier *)0)->fs_read_chunk)");
_Static_assert(offsetof(Carrier, scope_totals) == 504, "layout: offsetof(Carrier, scope_totals)");
_Static_assert(sizeof(((Carrier *)0)->scope_totals) == 128, "layout: sizeof(((Carrier *)0)->scope_totals)");
_Static_assert(offsetof(Carrier, host_stack_limit) == 632, "layout: offsetof(Carrier, host_stack_limit)");
_Static_assert(sizeof(((Carrier *)0)->host_stack_limit) == 8, "layout: sizeof(((Carrier *)0)->host_stack_limit)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->host_stack_limit) == 2, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->host_stack_limit)");
_Static_assert(offsetof(Carrier, bound) == 648, "layout: offsetof(Carrier, bound)");
_Static_assert(sizeof(((Carrier *)0)->bound) == 1, "layout: sizeof(((Carrier *)0)->bound)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->bound) == 3, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->bound)");
_Static_assert(offsetof(Carrier, console_lock_held) == 649, "layout: offsetof(Carrier, console_lock_held)");
_Static_assert(sizeof(((Carrier *)0)->console_lock_held) == 1, "layout: sizeof(((Carrier *)0)->console_lock_held)");
_Static_assert(CHIMI_LAYOUT_CLASS(((Carrier *)0)->console_lock_held) == 3, "layout: CHIMI_LAYOUT_CLASS(((Carrier *)0)->console_lock_held)");
_Static_assert(sizeof(ProfileRow) == 32, "layout: sizeof(ProfileRow)");
_Static_assert(offsetof(ProfileRow, operation) == 0, "layout: offsetof(ProfileRow, operation)");
_Static_assert(sizeof(((ProfileRow *)0)->operation) == 8, "layout: sizeof(((ProfileRow *)0)->operation)");
_Static_assert(offsetof(ProfileRow, evaluations) == 8, "layout: offsetof(ProfileRow, evaluations)");
_Static_assert(sizeof(((ProfileRow *)0)->evaluations) == 8, "layout: sizeof(((ProfileRow *)0)->evaluations)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ProfileRow *)0)->evaluations) == 2, "layout: CHIMI_LAYOUT_CLASS(((ProfileRow *)0)->evaluations)");
_Static_assert(offsetof(ProfileRow, allocations) == 16, "layout: offsetof(ProfileRow, allocations)");
_Static_assert(sizeof(((ProfileRow *)0)->allocations) == 8, "layout: sizeof(((ProfileRow *)0)->allocations)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ProfileRow *)0)->allocations) == 2, "layout: CHIMI_LAYOUT_CLASS(((ProfileRow *)0)->allocations)");
_Static_assert(offsetof(ProfileRow, allocated_bytes) == 24, "layout: offsetof(ProfileRow, allocated_bytes)");
_Static_assert(sizeof(((ProfileRow *)0)->allocated_bytes) == 8, "layout: sizeof(((ProfileRow *)0)->allocated_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ProfileRow *)0)->allocated_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((ProfileRow *)0)->allocated_bytes)");
_Static_assert(sizeof(RuntimeFragment) == 80, "layout: sizeof(RuntimeFragment)");
_Static_assert(offsetof(RuntimeFragment, area) == 0, "layout: offsetof(RuntimeFragment, area)");
_Static_assert(sizeof(((RuntimeFragment *)0)->area) == 56, "layout: sizeof(((RuntimeFragment *)0)->area)");
_Static_assert(offsetof(RuntimeFragment, root_count) == 56, "layout: offsetof(RuntimeFragment, root_count)");
_Static_assert(sizeof(((RuntimeFragment *)0)->root_count) == 4, "layout: sizeof(((RuntimeFragment *)0)->root_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((RuntimeFragment *)0)->root_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((RuntimeFragment *)0)->root_count)");
_Static_assert(offsetof(RuntimeFragment, roots) == 64, "layout: offsetof(RuntimeFragment, roots)");
_Static_assert(sizeof(((RuntimeFragment *)0)->roots) == 16, "layout: sizeof(((RuntimeFragment *)0)->roots)");
#undef CHIMI_LAYOUT_CLASS
/* END generated layout asserts. */

/* Heaps, fragments and freeing a whole area are Rust (heap/fragment.rs). */
RuntimeHeap *chimi_heap_create_v1(Carrier *here);
void chimi_heap_destroy_v1(Carrier *here, RuntimeHeap *heap);
/* A new, empty heap with a fresh owner id. */
static RuntimeHeap *heap_create(void) { return chimi_heap_create_v1(carrier); }

/* Requires: no region of `heap` is open. Frees every allocation the heap owns,
   adopted fragments included, without touching any other heap. */
static void heap_destroy(RuntimeHeap *heap) { chimi_heap_destroy_v1(carrier, heap); }
_Noreturn void chimi_runtime_heap_destroyed_open(void);
_Noreturn void chimi_runtime_heap_destroyed_open(void) {
  fputs("Chimi runtime destroyed a heap with an open region\n", stderr); runtime_exit(70);
}

static void *reallocate_bytes(void *value, size_t old_bytes, size_t new_bytes) {
  void *replacement = allocate(new_bytes, 1);
  if (value) memcpy(replacement, value, old_bytes < new_bytes ? old_bytes : new_bytes);
  return replacement;
}

/* The value cells are Rust (chimi-runtime heap/values.rs). */
ChimiValue *chimi_value_new_v1(ValueKind kind);
ChimiControl *chimi_control_new_v1(ControlKind kind, ChimiValue *value, const char *message, int64_t node);
ChimiValue **chimi_value_slots_new_v1(uint32_t count);
ChimiValue **chimi_value_slots_clone_v1(ChimiValue *const *source, uint32_t count);
ChimiEnvironment *chimi_environment_new_v1(uint32_t local_count);
ChimiEnvironment *chimi_environment_clone_v1(const ChimiEnvironment *source, bool locals, bool requirements);
ChimiEnvironment *chimi_environment_capture_requirements_v1(const ChimiEnvironment *source);
ChimiValue *chimi_value_integer_v1(ValueKind kind, int64_t raw);
ChimiValue *chimi_value_float_v1(double raw);
ChimiValue *chimi_value_range_v1(int64_t start, int64_t end);
ChimiValue *chimi_value_decimal_v1(ChimiDecimal decimal);
ChimiValue *chimi_value_pointer_v1(ValueKind kind, void *raw);
char *chimi_bytes_copy_v1(const char *bytes, size_t length);
ChimiValue *chimi_value_text_v1(const char *bytes, size_t length);
ChimiValue *chimi_value_json_number_v1(const char *raw);

static ChimiValue *new_value(ValueKind kind) { return chimi_value_new_v1(kind); }
static ChimiControl *new_control(ControlKind kind, ChimiValue *value, const char *message, int64_t node) {
  return chimi_control_new_v1(kind, value, message, node);
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
  carrier->host_stack_limit = here - bottom > STACK_UNLIMITED_BYTES ? here - STACK_UNLIMITED_BYTES
                                                         : bottom + STACK_HEADROOM_BYTES;
}

/* The most carriers `--carriers` accepts: each has a wake pipe, so this
   bounds the descriptors they take. */
enum { CARRIERS_MAX = 256 };

/* The carrier count when `--carriers` is not given: one per core this
   process may run on, at most CARRIERS_MAX. One when the program needs no
   scheduler, under the virtual clock (time moves only when every carrier is
   idle), and in the carrier simulation build, whose carriers share one
   thread. On Linux the cores are those of the process's affinity mask, so
   `taskset` and container CPU sets are respected. */
static uint32_t default_carriers(bool virtual_clock) {
#ifdef CHIMI_CARRIER_SIMULATION
  (void)virtual_clock;
  return 1;
#else
  if (!CHIMI_REQUIRES_SCHEDULER || virtual_clock) return 1;
  long cores = 0;
#ifdef __linux__
  cpu_set_t allowed;
  if (sched_getaffinity(0, sizeof(allowed), &allowed) == 0) cores = CPU_COUNT(&allowed);
#endif
  if (cores <= 0) cores = sysconf(_SC_NPROCESSORS_ONLN);
  if (cores <= 1) return 1;
  return cores > CARRIERS_MAX ? CARRIERS_MAX : (uint32_t)cores;
#endif
}

/* Gives the calling thread carrier `index`, which carriers_create made, and
   then the stack check the main thread has, so a carrier thread does both
   before its first fiber. */
static void carrier_thread_enter(size_t index) {
  carrier_bind(carrier_at(index));
  if (!CHIMI_REQUIRES_SCHEDULER) guard_host_stack();
}

/* The scheduler's carrier-enter hook: binds each carrier thread it starts. */
static void enter_carrier_thread(void *context, uint32_t index) {
  (void)context;
  carrier_thread_enter(index);
}

static bool host_stack_exhausted(void) {
  return (uintptr_t)__builtin_frame_address(0) < carrier->host_stack_limit;
}
static ChimiControl *cancelled(void) { return new_control(CONTROL_CANCEL, NULL, NULL, -1); }

/* Timestamps (time-and-outcomes.md, "Timestamp"): signed nanoseconds since
   1970-01-01T00:00:00Z ignoring leap seconds, written in RFC 3339 UTC by the
   proleptic Gregorian calendar. The text and parsing live in Rust (chimi-runtime
   timestamp.rs), shared with the compiler's --clock-start check; each text form
   fits in CHIMI_TIMESTAMP_TEXT bytes with its NUL. */
#define CHIMI_TIMESTAMP_TEXT 33
extern size_t chimi_timestamp_text_v1(int64_t nanoseconds, char *out);
extern bool chimi_timestamp_parse_v1(const char *text, size_t length, int64_t *nanoseconds);
extern size_t chimi_timestamp_date_time_v1(int64_t seconds, char *out);
/* Writes `nanoseconds` as `2026-11-10T09:30:00.25Z`. */
static void timestamp_text(int64_t nanoseconds, char *out) { chimi_timestamp_text_v1(nanoseconds, out); }
/* Parses an RFC 3339 date-time into Unix nanoseconds (timestamp.rs `parse`). */
static bool parse_timestamp(const char *text, int64_t *out) {
  return chimi_timestamp_parse_v1(text, strlen(text), out);
}

static ChimiValue **new_value_slots(uint32_t count) { return chimi_value_slots_new_v1(count); }
static ChimiValue **clone_value_slots(ChimiValue **source, uint32_t count) {
  return chimi_value_slots_clone_v1(source, count);
}
static ChimiEnvironment *new_environment(uint32_t local_count) { return chimi_environment_new_v1(local_count); }
static ChimiEnvironment *clone_environment(const ChimiEnvironment *source) {
  return chimi_environment_clone_v1(source, true, true);
}
/* A boundary copies only the table it changes (see clone_environment in values.rs). */
static ChimiEnvironment *clone_local_environment(const ChimiEnvironment *source) {
  return chimi_environment_clone_v1(source, true, false);
}
static ChimiEnvironment *clone_requirement_environment(const ChimiEnvironment *source) {
  return chimi_environment_clone_v1(source, false, true);
}
static ChimiEnvironment *capture_requirements(const ChimiEnvironment *source) {
  return chimi_environment_capture_requirements_v1(source);
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

/* The list vector's builders are Rust (chimi-runtime heap/list.rs); the reads
   below stay here, inline, for the C intrinsics that walk lists. */
ListVector chimi_list_vector_from_values_v1(ChimiValue *const *values, uint32_t length);
ChimiValue *chimi_list_value_v1(ListVector vector, uint32_t start, uint32_t length);
ChimiValue *chimi_list_push_v1(const ChimiValue *list, ChimiValue *item);
ChimiValue *chimi_list_slice_v1(Carrier *here, ChimiValue *list, uint32_t from, uint32_t to);

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
static ListVector vector_from_values(ChimiValue *const *values, uint32_t length) {
  return chimi_list_vector_from_values_v1(values, length);
}
static ChimiValue *list_value(ListVector vector, uint32_t start, uint32_t length) {
  return chimi_list_value_v1(vector, start, length);
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
/* A new list with `item` appended; `list` is unchanged. */
static ChimiValue *list_push(const ChimiValue *list, ChimiValue *item) { return chimi_list_push_v1(list, item); }
/* Window [from, to) of `list`, or NULL when the range is reversed or past the end;
   a short window of a long vector compacts or copies (see slice in list.rs). */
static ChimiValue *list_slice(ChimiValue *list, uint32_t from, uint32_t to) {
  return chimi_list_slice_v1(carrier, list, from, to);
}

static void allocation_graph_inconsistent(void) {
  fputs("Chimi runtime allocation graph is inconsistent\n", stderr); runtime_exit(70);
}

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


/* The collector is Rust (heap/collector.rs): a pass marks from the roots passed
   by address, moves the marked young chunk allocations into survivor blocks and
   rewrites the roots, frees what it did not reach, and on a full pass sweeps the
   survivors too. These are what it calls back. */
void chimi_runtime_allocation_graph_inconsistent(void);
void chimi_runtime_allocation_graph_inconsistent(void) { allocation_graph_inconsistent(); }
void chimi_runtime_mark_epochs_exhausted(void);
void chimi_runtime_mark_epochs_exhausted(void) {
  fputs("Chimi runtime allocation mark epoch exhausted\n", stderr); runtime_exit(70);
}

#if CHIMI_HEAP_VERIFY
uint32_t chimi_heap_verify_v1(const AllocationScope *scope, const ChimiValue *value,
                              const ChimiControl *control, uint64_t epoch, bool full,
                              size_t promoted, void *const *freed, size_t freed_count,
                              void *const *chunks, size_t chunk_bounds);
const char *chimi_heap_verify_message_v1(uint32_t code);
#endif
/* The collector's oracle after a pass, in a checking build (CHIMI_HEAP_VERIFY);
   the collector calls these only then. */
void chimi_runtime_heap_verify_freed(void *payload);
void chimi_runtime_heap_verify_freed(void *payload) {
#if CHIMI_HEAP_VERIFY
  pointer_stack_push(&heap_verify_freed, payload);
#else
  (void)payload;
#endif
}
void chimi_runtime_heap_verify_pass(const AllocationScope *scope, const ChimiValue *value,
                                    const ChimiControl *control, uint64_t epoch, bool full, size_t promoted);
void chimi_runtime_heap_verify_pass(const AllocationScope *scope, const ChimiValue *value,
                                    const ChimiControl *control, uint64_t epoch, bool full, size_t promoted) {
#if CHIMI_HEAP_VERIFY
  uint32_t code = chimi_heap_verify_v1(scope, value, control, epoch, full, promoted, heap_verify_freed.items,
                                       heap_verify_freed.length, heap_verify_chunks.items,
                                       heap_verify_chunks.length);
  heap_verify_freed.length = 0;
  heap_verify_chunks.length = 0;
  if (code == 0) return;
  fprintf(stderr, "Chimi runtime heap check failed: %s\n", chimi_heap_verify_message_v1(code));
  abort();
#else
  (void)scope, (void)value, (void)control, (void)epoch, (void)full, (void)promoted;
#endif
}

HeapStatus chimi_scope_collect_v1(Carrier *here, AllocationScope *scope, ChimiValue **value,
                                  ChimiControl **control);
HeapStatus chimi_scope_finish_v1(Carrier *here, AllocationScope *scope, ChimiValue **value,
                                 ChimiControl **control);
/* One pass of a chosen kind, and a fresh mark epoch: for the C test harnesses. */
HeapStatus chimi_scope_sweep_v1(Carrier *here, AllocationScope *scope, ChimiValue **value,
                                ChimiControl **control, bool full, bool ending);
uint64_t chimi_take_mark_epoch_v1(Carrier *here);

/* A pass boundary: a minor pass, or a full one once the survivors have doubled
   since the last full pass (see full_baseline). Returns HEAP_FOREIGN_POINTER,
   after collecting as usual, when a root reaches a value in another heap or in
   a fragment. */
static HeapStatus allocation_scope_collect(AllocationScope *scope, ChimiValue **value,
                                           ChimiControl **control) {
  return chimi_scope_collect_v1(carrier, scope, value, control);
}

/* Region exit: a full pass keeps what `value` and `control` reach, and the survivors
   join the enclosing region's young generation, or the heap's base area. */
static HeapStatus allocation_scope_end(AllocationScope *scope, ChimiValue **value,
                                       ChimiControl **control) {
  return chimi_scope_finish_v1(carrier, scope, value, control);
}

/* Fragments: copies for a transfer between owners, in Rust (heap/fragment.rs). A
   copy keeps sharing, rebuilds each List window as a compact vector, shares static
   constants, and reports HEAP_FOREIGN_POINTER instead of following a pointer out
   of its source. */

HeapStatus chimi_fragment_copy_v1(Carrier *here, void *const *roots, const AllocationKind *kinds, uint32_t count,
                                  RuntimeFragment **out);
void chimi_fragment_adopt_v1(Carrier *here, RuntimeFragment *fragment, void **roots);
void chimi_fragment_copy_into_current_v1(Carrier *here, const RuntimeFragment *fragment, void **roots);
RuntimeFragment *chimi_fragment_duplicate_v1(Carrier *here, const RuntimeFragment *source);
void chimi_fragment_free_v1(Carrier *here, RuntimeFragment *fragment);

/* Transfer boundary. Requires: the current heap owns every value under `roots`, or
   they are static. Ensures: a detached fragment holding one copy of the graph.
   Returns HEAP_FOREIGN_POINTER, and no fragment, if the graph leaves the heap. */
static HeapStatus fragment_copy(void *const *roots, const AllocationKind *kinds, uint32_t count,
                                RuntimeFragment **out) {
  return chimi_fragment_copy_v1(carrier, roots, kinds, count, out);
}

/* Hands the fragment's allocations to the current heap's innermost region (or its
   base area) in O(1), without copying, and writes its roots to `roots`. Consumes it. */
static void fragment_adopt(RuntimeFragment *fragment, void **roots) { chimi_fragment_adopt_v1(carrier, fragment, roots); }

/* Copies the fragment's graph into the current area, keeping sharing, and leaves the
   fragment intact, so it can be copied again (Task.result). O(fragment). */
static void fragment_copy_into_current(const RuntimeFragment *fragment, void **roots) {
  chimi_fragment_copy_into_current_v1(carrier, fragment, roots);
}

/* A second detached copy of an unadopted fragment, for another broadcast or watch
   subscription. O(fragment). */
static RuntimeFragment *fragment_duplicate(const RuntimeFragment *source) {
  return chimi_fragment_duplicate_v1(carrier, source);
}

/* Discards an unadopted fragment, for example a message handed back to its sender. */
static void fragment_free(RuntimeFragment *fragment) { chimi_fragment_free_v1(carrier, fragment); }

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

/* Held while one telemetry event is written (emit_event, emit_dependency,
   emit_scheduler_event). The scheduler's events come from its trace hook,
   under the kernel lock, so this lock is taken after the kernel's. */
static pthread_mutex_t trace_lock = PTHREAD_MUTEX_INITIALIZER;

/* The Unix time at the execution clock's origin: `--clock-start` under the
   virtual clock, and under the real clock the host's wall time when the
   computation starts, just before the scheduler takes its origin. */
static int64_t start_unix_nanos(const ChimiRuntime *runtime) {
  if (runtime->virtual_clock) return runtime->clock_start;
  struct timespec host;
  clock_gettime(CLOCK_REALTIME, &host);
  return (int64_t)host.tv_sec * INT64_C(1000000000) + host.tv_nsec;
}

static void emit_event(ChimiRuntime *runtime, const char *event, uint32_t node_id,
                       uint64_t span, int64_t parent, const char *operation,
                       const ChimiControl *outcome) {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  FILE *output = runtime->telemetry;
  pthread_mutex_lock(&trace_lock);
  ++runtime->trace_events_constructed;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":", output); json_string(output, event);
  fprintf(output, ",\"sequence\":%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, runtime->computation); fputs(",\"execution_id\":", output);
  json_string(output, runtime->execution_id);
  fprintf(output, ",\"span_id\":%" PRIu64 ",\"parent_span_id\":", span);
  if (parent < 0) fputs("null", output); else fprintf(output, "%" PRId64, parent);
  fprintf(output, ",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":",
         node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  json_string(output, operation); fputs(",\"attributes\":{", output);
  if (outcome) { fputs("\"outcome\":", output); json_string(output, control_name(outcome)); fputc(',', output); }
  bool started = strcmp(event, "computation_started") == 0;
  /* The computation starts at the clock's origin, before the scheduler runs. */
  fprintf(output, "\"time_ns\":%" PRId64, started ? INT64_C(0) : chimi_fiber_now());
  if (started) {
    fprintf(output, ",\"start_unix_ns\":%" PRId64, runtime->start_unix_ns);
    fputs(CHIMI_TRACE_MODULES, output);
  }
  fputs("}}\n", output);
  pthread_mutex_unlock(&trace_lock);
}

static void emit_dependency(ChimiRuntime *runtime, uint32_t node_id, uint64_t span,
                            uint64_t parent, const ChimiCallable *callable) {
  const ChimiNode *node = &CHIMI_NODES[node_id]; const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  FILE *output = runtime->telemetry;
  pthread_mutex_lock(&trace_lock);
  ++runtime->trace_events_constructed;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":\"dependency_called\",\"sequence\":", output);
  fprintf(output, "%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, runtime->computation);
  fputs(",\"execution_id\":", output); json_string(output, runtime->execution_id);
  fprintf(output, ",\"span_id\":%" PRIu64 ",\"parent_span_id\":%" PRIu64
         ",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":\"dependency_call\",\"attributes\":{",
         span, parent, node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  fputs("\"requirement\":", output);
  json_string(output, CHIMI_STRINGS[CHIMI_SYMBOLS[callable->requirement].name]);
  fprintf(output, ",\"requirement_symbol_id\":%u,\"interface_symbol_id\":%u,\"method\":",
          callable->requirement, callable->interface_symbol); json_string(output, callable->name);
  fprintf(output, ",\"method_semantic_id\":%u,\"time_ns\":%" PRId64 "}}\n", callable->method,
          chimi_fiber_now());
  pthread_mutex_unlock(&trace_lock);
}

static void json_identity(FILE *output, const char *key, uint64_t identity);

/* A process body's own span (CHI-312): `process_body_started` when the body
   begins and `process_body_completed` with its outcome when it ends, both
   naming the incarnation, so a trace reader can hang the body's operations
   under that incarnation's `process_started`. A body the scheduler abandons
   at a park writes no completion. */
static void emit_process_body(ChimiRuntime *runtime, uint64_t span, uint64_t incarnation,
                              const ChimiControl *outcome) {
  FILE *output = runtime->telemetry;
  pthread_mutex_lock(&trace_lock);
  ++runtime->trace_events_constructed;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":", output);
  json_string(output, outcome ? "process_body_completed" : "process_body_started");
  fprintf(output, ",\"sequence\":%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, runtime->computation);
  fputs(",\"execution_id\":", output);
  json_string(output, runtime->execution_id);
  fprintf(output, ",\"span_id\":%" PRIu64 ",\"parent_span_id\":null", span);
  json_identity(output, "incarnation_id", incarnation);
  fputs(",\"operation\":\"process_body\",\"attributes\":{", output);
  if (outcome) { fputs("\"outcome\":", output); json_string(output, control_name(outcome)); fputc(',', output); }
  fprintf(output, "\"time_ns\":%" PRId64 "}}\n", chimi_fiber_now());
  pthread_mutex_unlock(&trace_lock);
}

/* Charges the current heap's allocations to `node` until profile_leave. */
static uint32_t profile_enter(uint32_t node, const char *operation) {
  if (!profile_rows) return 0;
  ProfileRow *row = &profile_rows[node];
  __atomic_store_n(&row->operation, operation, __ATOMIC_RELAXED);
  profile_count(&row->evaluations, 1);
  uint32_t outer = carrier->fiber.heap->profiled_node_plus_one;
  carrier->fiber.heap->profiled_node_plus_one = node + 1;
  return outer;
}
static void profile_leave(uint32_t outer) {
  if (profile_rows) carrier->fiber.heap->profiled_node_plus_one = outer;
}

/* Span ids a carrier takes from the runtime's counter at once when several
   carriers run. Traced modes take a span per node, and one shared fetch-add
   per node made a traced counting loop slower on four carriers than on one
   (step 3 PR 6a review). */
enum { SPAN_BLOCK = 4096 };

/* `count` consecutive span ids that no other node has; returns the first.
   One carrier takes them straight from the counter, so its ids stay dense
   and in order, as a transcript expects; with several, ids are unique but
   neither dense nor ordered across carriers. */
static uint64_t take_spans(ChimiRuntime *runtime, uint64_t count) {
#ifdef CHIMI_CARRIER_SIMULATION
  /* The simulated carriers share one thread, so nothing contends. */
  return atomic_fetch_add_explicit(&runtime->next_span, count, memory_order_relaxed);
#else
  if (carrier_count == 1) return atomic_fetch_add_explicit(&runtime->next_span, count, memory_order_relaxed);
  if (carrier->end_span - carrier->next_span < count) {
    uint64_t size = count > SPAN_BLOCK ? count : SPAN_BLOCK;
    carrier->next_span = atomic_fetch_add_explicit(&runtime->next_span, size, memory_order_relaxed);
    carrier->end_span = carrier->next_span + size;
  }
  uint64_t first = carrier->next_span;
  carrier->next_span += count;
  return first;
#endif
}

static uint64_t begin_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent, const char *operation) {
  if (runtime->silent) return parent;
  uint64_t span = take_spans(runtime, 1);
  if (runtime->full_trace) emit_event(runtime, "operation_started", node, span, (int64_t)parent, operation, NULL);
  return span;
}
static ChimiControl *finish_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent,
                                 uint64_t span, const char *operation, ChimiControl *control) {
  if (runtime->full_trace) emit_event(runtime, "operation_completed", node, span, (int64_t)parent, operation, control);
  return control;
}

static ChimiValue *integer_value(int64_t raw) { return chimi_value_integer_v1(VALUE_INTEGER, raw); }
static ChimiValue *range_value(int64_t start, int64_t end) { return chimi_value_range_v1(start, end); }
/* Void, True and False are each one shared value in the static area, because values
   never change after construction and conditions and effects produce them on almost
   every pass. Static values are never collected, copied or freed. decode_constants
   creates all three at startup, before any carrier runs. While carriers run on
   threads, nothing appends to the static area: list_slice copies a constant's
   window rather than compacting it there. */
static ChimiValue *shared_values[3];
static ChimiValue *shared_value(uint32_t index) {
  Carrier *const here = carrier;
  if (!shared_values[index]) {
    AllocationArea *previous = here->fiber.target;
    here->fiber.target = &static_area;
    ChimiValue *value = new_value(index == 0 ? VALUE_VOID : VALUE_BOOLEAN);
    if (index == 2) value->as.boolean = true;
    here->fiber.target = previous;
    shared_values[index] = value;
  }
  return shared_values[index];
}
static ChimiValue *void_value(void) { return shared_value(0); }
static char *copy_text(const char *raw) { return chimi_bytes_copy_v1(raw, strlen(raw)); }
/* A binding may pass NULL for no bytes. */
static ChimiValue *text_from_bytes(const char *bytes, size_t length) { return chimi_value_text_v1(bytes, length); }
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
static ChimiValue *boolean_value(bool raw) { return shared_value(raw ? 2 : 1); }
static ChimiValue *float_value(double raw) { return chimi_value_float_v1(raw); }

/* The numeric model. It mirrors compiler/chimi/src/numeric.rs value for value and
   message for message, because both execution paths must report identical outcomes. */

/* A defect message that outlives the collector: a defect ends its unit of work. */
static const char *numeric_message(const char *format, ...) {
  va_list arguments; va_start(arguments, format);
  char *message = malloc(256);
  if (!message) { fputs("Chimi runtime allocation failed\n", stderr); runtime_exit(70); }
  vsnprintf(message, 256, format, arguments); va_end(arguments);
  return message;
}

/* Canonical Float text (chimi_runtime::float, which the interpreter prints
   with too): the shortest digits that parse back to the same value, positional
   for decimal exponents -5..15 and scientific otherwise, always with a `.` or
   an `e`. `output` holds at least 40 bytes. */
extern size_t chimi_float_text_v1(double value, char *out);
static void float_text(double value, char *output) { chimi_float_text_v1(value, output); }

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

/* std/decimal: the interpreter's own model, chimi-runtime's decimal.rs, through
   decimal_abi.rs. A sign, a magnitude of at most 38 digits, and a scale from 0
   to 38; no operation rounds unless it takes a rounding mode, and none wraps or
   saturates: a result outside the range makes the function return false. */
typedef ChimiDecimal Decimal;
/* decimal.rs TEXT_BYTES and a NUL. */
#define DECIMAL_TEXT_BYTES 48
ChimiDecimal chimi_decimal_negate_v1(ChimiDecimal value);
bool chimi_decimal_add_v1(ChimiDecimal left, ChimiDecimal right, ChimiDecimal *out);
bool chimi_decimal_subtract_v1(ChimiDecimal left, ChimiDecimal right, ChimiDecimal *out);
bool chimi_decimal_multiply_v1(ChimiDecimal left, ChimiDecimal right, ChimiDecimal *out);
int32_t chimi_decimal_compare_v1(ChimiDecimal left, ChimiDecimal right);
size_t chimi_decimal_text_v1(ChimiDecimal value, bool normalized, char *out);

static Decimal decimal_of(const ChimiValue *value) { return value->as.decimal; }
static ChimiValue *decimal_value(Decimal decimal) { return chimi_value_decimal_v1(decimal); }
__attribute__((noinline)) static ChimiValue *decimal_negated(const ChimiValue *value) {
  return decimal_value(chimi_decimal_negate_v1(decimal_of(value)));
}
/* `Decimal.to_text`: exactly `scale` fraction digits. `output` holds DECIMAL_TEXT_BYTES. */
static void decimal_text(Decimal value, char *output) { chimi_decimal_text_v1(value, false, output); }
/* The debug form: trailing fraction zeros removed, so equal values render the same. */
static void decimal_normalized_text(Decimal value, char *output) { chimi_decimal_text_v1(value, true, output); }

/* Applies `+`, `-`, or `*`; typing rejects `/` and `%` on Decimal (T4306).
   Kept out of line, as decimal_negated is: chimi_rt_binary and chimi_rt_unary sit
   in every frame of a recursion through an operator, and the 24-byte decimals
   and text buffers would grow each of those frames (the task stack test in
   tests/cli.rs recurses 1,000 calls through `+`). */
__attribute__((noinline)) static ChimiControl *decimal_arithmetic(uint32_t operator, const ChimiValue *left,
                                                                  const ChimiValue *right, int64_t node) {
  Decimal l = decimal_of(left), r = decimal_of(right);
  const char *symbol = binary_symbol(operator); Decimal output; bool fits = false;
  switch (operator) {
    case CHIMI_BINARY_ADD: fits = chimi_decimal_add_v1(l, r, &output); break;
    case CHIMI_BINARY_SUBTRACT: fits = chimi_decimal_subtract_v1(l, r, &output); break;
    case CHIMI_BINARY_MULTIPLY: fits = chimi_decimal_multiply_v1(l, r, &output); break;
    default: return defect(numeric_message("`%s` is not a `Decimal` operator", symbol), node);
  }
  if (fits) return success(decimal_value(output));
  char left_text[DECIMAL_TEXT_BYTES], right_text[DECIMAL_TEXT_BYTES];
  decimal_text(l, left_text); decimal_text(r, right_text);
  return defect(numeric_message("decimal overflow in `%s`: %s %s %s needs more than 38 digits or scale 38",
                                symbol, left_text, symbol, right_text), node);
}
static ChimiValue *callable_value(ChimiCallable *raw) { return chimi_value_pointer_v1(VALUE_CALLABLE, raw); }
static ChimiValue *map_value(MapNode *raw) { return chimi_value_pointer_v1(VALUE_MAP, raw); }
static ChimiValue *set_value(MapNode *raw) { return chimi_value_pointer_v1(VALUE_SET, raw); }
static ChimiValue *json_number_value(const char *raw) { return chimi_value_json_number_v1(raw); }
static ChimiValue *task_value(uint64_t raw) {
  ChimiValue *v = new_value(VALUE_TASK);
  v->as.task = raw;
  counted_handle_retain(raw | CHIMI_TASK_HANDLE_KEY);
  return v;
}
static ChimiValue *channel_capability_value(ValueKind kind, uint64_t channel,
                                            uint64_t owner, uint64_t token) {
  ChimiValue *value = new_value(kind);
  value->as.capability = (RuntimeCapability){channel, owner, token};
  counted_handle_retain(channel);
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

/* Parks the running fiber with `request`. The fiber may resume on another
   carrier, so nothing read through `carrier` before this call is used after it. */
static ChimiFiberCommand carrier_request(void *request) {
  Carrier *parked = carrier;
  ChimiFiberCommand command = chimi_fiber_request(request, NULL);
  carrier_migrate(parked);
  return command;
}

/* Every scheduler request goes through here. A fiber resumes only by returning from
   its request, so restoring the caller's heap and allocation target here switches
   both on every resume: another fiber that ran meanwhile may have changed them, or
   overflowed its stack while a copy had the target set, leaving it pointing into an
   area that fiber's end destroyed. Each fiber entry starts with no target.
   The scheduler applies the channel and task releases this thread queued
   before it serves `request` (chimi_fiber_handle_drop). */
static ChimiFiberCommand scheduler_request(void *request) {
  FiberContext fiber = carrier->fiber;
  ChimiFiberCommand command = carrier_request(request);
  carrier->fiber = fiber;
  return command;
}

/* Capability transfer for Task.start and the sends: the walk over the value graph
   is Rust (chimi-runtime heap/transfer.rs) and calls back here for each Receiver,
   Completion and managed handle it meets. For a transfer, `child` takes each
   Receiver and Completion; when collecting for a send, each capability found is
   recorded with the sender's token, and its copy in the message takes the
   in-flight token (ChimiCarriedCapability). */
typedef struct {
  uint64_t child;
  bool cancelled;
  bool collect;
  ChimiCarriedCapability *carried;
  size_t carried_count;
  size_t carried_capacity;
  /* Managed process and supervisor handles met while collecting. Each has one
     owner, so a value a broadcast or watch would copy must not hold one. */
  size_t managed_count;
} CapabilityTransfer;

bool chimi_transfer_captures_v1(CapabilityTransfer *transfer, ChimiValue *root, uint64_t *probes);
bool chimi_runtime_transfer_capability_v1(CapabilityTransfer *transfer, ChimiValue *value);

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

/* The defect for publishing a managed handle where every subscriber would get a copy:
   a handle has one owner, so two copies would both look valid. */
#define MANAGED_COPY_DEFECT(function)                                                    \
  "channel capability violation: " function " cannot hold a managed process or "         \
  "supervisor handle, because every subscriber gets a copy"

/* Called by the walk for each capability value it meets. False when a transfer
   request fails, or when the parent is cancelled meanwhile (`cancelled` is then
   set); the walk then stops. */
bool chimi_runtime_transfer_capability_v1(CapabilityTransfer *transfer, ChimiValue *value) {
  switch (value->kind) {
    case VALUE_CHANNEL_RECEIVER:
      return transfer_captured_capability(transfer, value, CHIMI_REQUEST_CHANNEL_TRANSFER_RECEIVER);
    case VALUE_CHANNEL_COMPLETION:
      return transfer_captured_capability(transfer, value, CHIMI_REQUEST_CHANNEL_TRANSFER_COMPLETION);
    case VALUE_MANAGED_PROCESS:
    case VALUE_MANAGED_SUPERVISOR:
      if (transfer->collect) ++transfer->managed_count;
      return true;
    default: return false;
  }
}

/* Transfers every capability reachable from `root`. False when a transfer request
   fails, or when the parent is cancelled meanwhile (`cancelled` is then set). The
   walk's probes count on the carrier it ends on, since a transfer request can move
   the task to another carrier. */
static bool transfer_captures(CapabilityTransfer *transfer, ChimiValue *root) {
  uint64_t probes = 0;
  bool transferred = chimi_transfer_captures_v1(transfer, root, &probes);
  carrier->capture_transfer_probes += probes;
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
  if (!runtime->silent) take_spans(runtime, node_count);
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

/* The persistent Map tree, which Set shares, and structural order live in
   runtime/chimi-runtime/src/heap/map.rs and compare.rs. */
int32_t chimi_value_compare_v1(const ChimiValue *left, const ChimiValue *right);
int32_t chimi_json_number_compare_v1(const char *left, const char *right);
char *chimi_json_number_canonical_text_v1(const char *text, size_t *length);
MapNode *chimi_map_put_v1(MapNode *node, ChimiValue *key, ChimiValue *value);
MapNode *chimi_map_remove_v1(MapNode *node, const ChimiValue *key);
ChimiValue *chimi_map_get_v1(const MapNode *node, const ChimiValue *key);
int64_t chimi_map_size_v1(const MapNode *node);
const MapNode **chimi_map_entries_v1(const MapNode *root, size_t *length);

/* Structural order: -1, 0 or 1; both values have the same equatable type. */
static int chimi_value_compare(const ChimiValue *left, const ChimiValue *right) {
  return chimi_value_compare_v1(left, right);
}
/* The order of two JSON numbers by the exact decimal they denote. */
static int json_number_compare(const char *left, const char *right) {
  return chimi_json_number_compare_v1(left, right);
}
static MapNode *map_put_node(MapNode *node, ChimiValue *key, ChimiValue *value) {
  return chimi_map_put_v1(node, key, value);
}
/* `node` without `key`; `node` itself when it has no such key. */
static MapNode *map_remove_node(MapNode *node, const ChimiValue *key) { return chimi_map_remove_v1(node, key); }
static int64_t map_size(const MapNode *node) { return chimi_map_size_v1(node); }
static ChimiValue *map_get_node(MapNode *node, const ChimiValue *key) { return chimi_map_get_v1(node, key); }

typedef struct { const MapNode **nodes; size_t length; } MapEntries;
/* In key order, in a block the caller frees. */
static MapEntries map_entries(const MapNode *root) {
  MapEntries entries;
  entries.nodes = chimi_map_entries_v1(root, &entries.length);
  return entries;
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
      case VALUE_TEST_RECORDER: fputs("{\"type\":\"Recorder\",\"fields\":{}}", output); break;
      case VALUE_TEST_SCRIPT: fputs("{\"type\":\"Script\",\"fields\":{}}", output); break;
      case VALUE_TEST_MAILBOX: fputs("{\"type\":\"Mailbox\",\"fields\":{}}", output); break;
      case VALUE_CAUSE: fputs("{\"type\":\"Cause\",\"fields\":{}}", output); break;
      case VALUE_NATIVE_HANDLE:
        fputs("{\"type\":", output);
        json_string(output, value->as.handle.name);
        fputs(",\"fields\":{}}", output);
        break;
    }
  }
  free(steps.items);
}

/* The declaration-order ordinal of the constructor named by `symbol`, for
   structural order in heap/compare.rs. */
uint32_t chimi_runtime_constructor_ordinal_v1(uint32_t symbol) {
  return CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[symbol].constructor].ordinal;
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
/* Json.decode builds values from the Rust decoder's tape in
   runtime/chimi-runtime/src/intrinsics/json.rs, and ends the program here on a
   tape that breaks the decoder's contract. */
_Noreturn void chimi_runtime_json_tape_defect_v1(void) {
  fputs("Chimi runtime: malformed JSON tape\n", stderr); runtime_exit(70);
}

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment);

static ChimiControl *intrinsic_shape_defect(uint32_t call_node);

#define COUNT_OF(table) (sizeof (table) / sizeof (table)[0])

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

/* A std/test Recorder, Script or Mailbox, as TestLog in execution/core_operations.rs
   (a Mailbox is native-only). The value is plain data (its id in
   `as.address.target`); the log lives here, outside every heap, so a value in it is
   one detached fragment. A Recorder copies each fragment out on every `recorded`; a
   Script hands each out once, by adoption; a Mailbox holds at most `limit` messages
   sent to its address, and `received` adopts them all. */
typedef struct {
  ValueKind kind;
  RuntimeFragment **values;
  uint32_t length, capacity, taken;
  int64_t limit;
} TestLog;

/* A third address kind, beside CHIMI_ADDRESS_NAMED and CHIMI_ADDRESS_DIRECT, whose
   target is a Mailbox's TestLog id. No process owns it, so it never reaches the
   scheduler: Process.send delivers to the mailbox here. */
enum { CHIMI_ADDRESS_TEST = 3 };
static TestLog *test_logs;
static ChimiValue *process_address_value(uint32_t kind, uint64_t target);
static uint64_t test_log_count;
/* Held across every operation on the logs (invoke_test_log,
   send_to_test_mailbox): a log's address travels to tasks on any carrier.
   Nothing under it parks; it is taken before the handle counts' lock. */
static pthread_mutex_t test_log_lock = PTHREAD_MUTEX_INITIALIZER;

static ChimiValue *new_test_log(ValueKind kind) {
  if (test_log_count % 16 == 0) {
    test_logs = realloc(test_logs, (size_t)(test_log_count + 16) * sizeof(*test_logs));
    if (!test_logs) abort();
  }
  test_logs[test_log_count] = (TestLog){.kind = kind};
  ChimiValue *value = new_value(kind);
  value->as.address.target = ++test_log_count;
  return value;
}

static TestLog *test_log(const ChimiValue *value) {
  uint64_t id = value->as.address.target;
  if (id == 0 || id > test_log_count || test_logs[id - 1].kind != value->kind) abort();
  return &test_logs[id - 1];
}

static void test_log_append(TestLog *log, RuntimeFragment *fragment) {
  if (log->length == log->capacity) {
    log->capacity = log->capacity ? log->capacity * 2 : 8;
    log->values = realloc(log->values, log->capacity * sizeof(*log->values));
    if (!log->values) abort();
  }
  log->values[log->length++] = fragment;
}

/* NULL, with the defect in `*failure`, when `value` reaches outside this heap. */
static bool test_log_push(TestLog *log, ChimiValue *value, const char *operation, uint32_t call_node,
                          ChimiControl **failure) {
  void *roots[] = {value};
  AllocationKind kinds[] = {ALLOCATION_VALUE};
  RuntimeFragment *fragment = NULL;
  if (fragment_copy(roots, kinds, 1, &fragment) != HEAP_OK) {
    *failure = defect(numeric_message("%s: the value points into owner %" PRIu64
                                      ", outside the caller's heap (owner %" PRIu64 ")",
                                      operation, carrier->fragment_copy_foreign_owner, carrier->fiber.heap->owner),
                      call_node);
    return false;
  }
  test_log_append(log, fragment);
  return true;
}

static ChimiControl *invoke_test_log_locked(uint32_t intrinsic, ChimiValue **arguments,
                                            uint32_t argument_count, uint32_t call_node) {
  ChimiControl *failure = NULL;
  switch (intrinsic) {
    case CHIMI_INTRINSIC_TEST_RECORDER:
      if (argument_count != 0) break;
      return success(new_test_log(VALUE_TEST_RECORDER));
    case CHIMI_INTRINSIC_TEST_SCRIPT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST) break;
      ChimiValue *script = new_test_log(VALUE_TEST_SCRIPT);
      for (uint32_t index = 0; index < list_length(arguments[0]); ++index)
        if (!test_log_push(test_log(script), list_get(arguments[0], index), "Test.script", call_node, &failure))
          return failure;
      return success(script);
    }
    case CHIMI_INTRINSIC_TEST_RECORD:
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEST_RECORDER) break;
      if (!test_log_push(test_log(arguments[0]), arguments[1], "Test.record", call_node, &failure))
        return failure;
      return success(void_value());
    case CHIMI_INTRINSIC_TEST_RECORDED: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEST_RECORDER) break;
      TestLog *log = test_log(arguments[0]);
      ChimiValue **values = malloc((log->length ? log->length : 1) * sizeof(*values));
      if (!values) abort();
      for (uint32_t index = 0; index < log->length; ++index)
        fragment_copy_into_current(log->values[index], (void **)&values[index]);
      ChimiValue *list = list_from_values(values, log->length);
      free(values);
      return success(list);
    }
    case CHIMI_INTRINSIC_TEST_NEXT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEST_SCRIPT) break;
      TestLog *log = test_log(arguments[0]);
      if (log->taken == log->length)
        return defect(numeric_message("Test.next: script exhausted after %" PRIu32 " values", log->taken),
                      call_node);
      void *root = NULL;
      fragment_adopt(log->values[log->taken], &root);
      log->values[log->taken++] = NULL;
      return success(root);
    }
    case CHIMI_INTRINSIC_TEST_MAILBOX: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) break;
      if (arguments[0]->as.integer < 1)
        return defect(numeric_message("Test.mailbox: the capacity must be at least 1, not %" PRId64,
                                      arguments[0]->as.integer),
                      call_node);
      ChimiValue *mailbox = new_test_log(VALUE_TEST_MAILBOX);
      test_log(mailbox)->limit = arguments[0]->as.integer;
      ChimiValue *pair = new_value(VALUE_TUPLE);
      pair->as.record.length = 2;
      pair->as.record.fields = allocate(2, sizeof(*pair->as.record.fields));
      allocation_kind(pair->as.record.fields, ALLOCATION_FIELDS);
      pair->as.record.fields[0] =
        (RuntimeField){NULL, process_address_value(CHIMI_ADDRESS_TEST, mailbox->as.address.target)};
      pair->as.record.fields[1] = (RuntimeField){NULL, mailbox};
      return success(pair);
    }
    case CHIMI_INTRINSIC_TEST_RECEIVED: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEST_MAILBOX) break;
      TestLog *log = test_log(arguments[0]);
      ChimiValue **values = malloc((log->length ? log->length : 1) * sizeof(*values));
      if (!values) abort();
      for (uint32_t index = 0; index < log->length; ++index) {
        fragment_adopt(log->values[index], (void **)&values[index]);
        log->values[index] = NULL;
      }
      ChimiValue *list = list_from_values(values, log->length);
      free(values);
      log->length = 0;
      return success(list);
    }
  }
  return intrinsic_shape_defect(call_node);
}

static ChimiControl *invoke_test_log(uint32_t intrinsic, ChimiValue **arguments,
                                     uint32_t argument_count, uint32_t call_node) {
  pthread_mutex_lock(&test_log_lock);
  ChimiControl *control = invoke_test_log_locked(intrinsic, arguments, argument_count, call_node);
  pthread_mutex_unlock(&test_log_lock);
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
    (void)runtime;
    ScopeTotals *totals = &carrier->scope_totals;
    totals->total_allocated_bytes += scope->total_allocated_bytes;
    for (uint32_t kind = 0; kind < ALLOCATION_KIND_COUNT; ++kind)
      totals->allocated_bytes_by_kind[kind] += scope->allocated_bytes_by_kind[kind];
    if (scope->peak_live_bytes > totals->peak_live_bytes) totals->peak_live_bytes = scope->peak_live_bytes;
    totals->retained_bytes += scope->live_bytes;
    totals->collections += scope->collections;
    totals->full_collections += scope->full_collections;
    totals->scanned_allocations += scope->scanned_allocations;
  }
  return status == HEAP_OK ? control : defect(FOREIGN_POINTER_DEFECT, -1);
}

/* Bytes read from a descriptor and not yet consumed. Standard input's buffer is
   process-wide, so the bytes a read takes past an early exit (a failing callback,
   a cancellation) wait for the next read of "-", as stdio's buffer kept them. */
typedef struct {
  int fd;
  unsigned char *bytes;
  size_t capacity;
  size_t start;
  size_t end;
  /* Standard input's lock, since tasks on any carrier may read "-"; NULL for a
     file one call reads. A reader holds it while it takes a line's bytes and
     lets go before each callback and while it waits for more input, so, as
     with one carrier, two readers of "-" may split a line that arrives in
     pieces; it keeps the buffer whole, not the lines. */
  pthread_mutex_t *lock;
} ByteInput;

enum { INPUT_BUFFER_BYTES = 64 * 1024 };
static unsigned char standard_input_bytes[INPUT_BUFFER_BYTES];
static pthread_mutex_t standard_input_lock = PTHREAD_MUTEX_INITIALIZER;
static ByteInput standard_input = {STDIN_FILENO, standard_input_bytes, INPUT_BUFFER_BYTES, 0, 0,
                                   &standard_input_lock};

static void input_lock(ByteInput *input) {
  if (input->lock) pthread_mutex_lock(input->lock);
}
static void input_unlock(ByteInput *input) {
  if (input->lock) pthread_mutex_unlock(input->lock);
}

enum { INPUT_END = -1, INPUT_FAILED = -2, INPUT_CANCELLED = -3 };

/* Waits until `fd` is readable. Under the scheduler the task parks in
   CHIMI_REQUEST_IO_WAIT, so other tasks run and a signal is delivered meanwhile;
   its OK reply resumes this task before any other, so the read that follows does
   not block unless another OS process drained the descriptor first. A closed
   descriptor answers ERROR, and the read reports it. It also ends when another
   task reading "-" has refilled the shared buffer meanwhile. False when
   cancelled. */
enum { INPUT_WAIT_READY, INPUT_WAIT_CANCELLED, INPUT_WAIT_FAILED };

/* Waits until `input` may be read. INPUT_WAIT_FAILED, with errno set, when the
   reactor cannot wait on the descriptor (it is closed, or poll failed): a
   read then could block the scheduler thread. */
static int wait_readable(ByteInput *input) {
  int fd = input->fd;
  if (CHIMI_REQUIRES_SCHEDULER) {
    for (;;) {
      if (input->start < input->end) return INPUT_WAIT_READY;
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_IO_WAIT, .value = fd, .token = CHIMI_IO_READABLE,
      };
      /* The caller holds the input's lock, which is never held across a park. */
      input_unlock(input);
      bool cancel = scheduler_request(&request) == CHIMI_FIBER_CANCEL;
      input_lock(input);
      if (cancel) return INPUT_WAIT_CANCELLED;
      if (request.status == CHIMI_RESPONSE_ERROR) {
        errno = fcntl(fd, F_GETFD) < 0 ? EBADF : EIO;
        return INPUT_WAIT_FAILED;
      }
      if (request.status != CHIMI_RESPONSE_PENDING) return INPUT_WAIT_READY;
    }
  }
  /* Without the scheduler a signal interrupts poll with EINTR and sets `cancel`,
     which the caller checks. */
  struct pollfd poll_fd = {.fd = fd, .events = POLLIN};
  input_unlock(input);
  (void)poll(&poll_fd, 1, -1);
  input_lock(input);
  return INPUT_WAIT_READY;
}

/* The next byte, INPUT_END, INPUT_CANCELLED, or INPUT_FAILED with errno set.
   The caller holds the input's lock; a wait lets go of it meanwhile. */
static int input_next(ChimiRuntime *runtime, ByteInput *input) {
  if (input->start < input->end) return input->bytes[input->start++];
  for (;;) {
    if (runtime->cancel) return INPUT_CANCELLED;
    if (CHIMI_REQUIRES_SCHEDULER) {
      int waited = wait_readable(input);
      if (waited == INPUT_WAIT_CANCELLED) return INPUT_CANCELLED;
      if (waited == INPUT_WAIT_FAILED) return INPUT_FAILED;
      /* Another task reading "-" may have refilled the shared buffer while this
         one waited; reading over it would lose those bytes. */
      if (input->start < input->end) return input->bytes[input->start++];
    }
    errno = 0;
    ssize_t count = read(input->fd, input->bytes, input->capacity);
    if (count > 0) {
      input->start = 0;
      input->end = (size_t)count;
      return input->bytes[input->start++];
    }
    if (count == 0) return INPUT_END;
    if (errno == EINTR) continue;
    /* A descriptor a parent left non-blocking: wait for it, then read again. */
    if (errno == EAGAIN || errno == EWOULDBLOCK) {
      if (!CHIMI_REQUIRES_SCHEDULER) (void)wait_readable(input);
      continue;
    }
    return INPUT_FAILED;
  }
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
  ByteInput file_input = {-1, NULL, INPUT_BUFFER_BYTES, 0, 0, NULL};
  ByteInput *input = &standard_input;
  if (owns_file) {
    file_input.fd = open(path, O_RDONLY | O_CLOEXEC);
    if (file_input.fd < 0)
      return host_file_message(CHIMI_CONSTRUCTOR_FILE_OPEN, path, strerror(errno));
    /* Freed below, so a loop over many files does not keep a buffer for each. */
    file_input.bytes = malloc(INPUT_BUFFER_BYTES);
    if (!file_input.bytes) {
      close(file_input.fd);
      return host_file_message(CHIMI_CONSTRUCTOR_FILE_OPEN, path, strerror(ENOMEM));
    }
    input = &file_input;
  }
  unsigned char *line = allocate((size_t)runtime->max_line_bytes + 1, 1);
  size_t length = 0;
  uint64_t line_number = 1;
  ChimiValue *state = arguments[1];
  ChimiControl *control = NULL;
  AllocationScope allocation_scope;
  allocation_scope_begin(&allocation_scope);
  /* The input's lock is held while a line's bytes are taken (except while
     input_next waits), and let go once the line ends (a newline, the end, a
     failure or a line too long). */
  bool held = false;
  for (;;) {
    if (!held) {
      input_lock(input);
      held = true;
    }
    if (runtime->cancel) {
      input_unlock(input);
      control = cancelled();
      allocation_scope_collect(&allocation_scope, NULL, &control);
      break;
    }
    int byte = input_next(runtime, input);
    if (byte < 0 || byte == '\n' || length == runtime->max_line_bytes) {
      input_unlock(input);
      held = false;
    }
    if (byte < 0) {
      if (byte == INPUT_CANCELLED) {
        control = cancelled();
        allocation_scope_collect(&allocation_scope, NULL, &control);
      } else if (byte == INPUT_FAILED) {
        control = host_file_message(CHIMI_CONSTRUCTOR_FILE_READ, path, strerror(errno));
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
  free(file_input.bytes);
  if (owns_file && close(file_input.fd) != 0 &&
      (control->kind == CONTROL_SUCCESS || control->kind == CONTROL_FAILURE)) {
    control = host_file_message(CHIMI_CONSTRUCTOR_FILE_CLOSE, path, strerror(errno));
    allocation_scope_collect(&allocation_scope, NULL, &control);
  }
  return end_measured_allocation_scope(runtime, &allocation_scope, control);
}

/* The `random` provider (testing.md, Randomness): xoshiro256** seeded through
   SplitMix64, in chimi_runtime::random, which the interpreter uses too, so a
   seed draws the same numbers on both backends. `--random-seed` fixes the seed
   (`chimi test` gives each test its own); without it the first draw seeds from
   the operating system. The seed read and the lock stay here: the Rust crate
   has no operating system to read from. */
typedef struct {
  uint64_t state[4];
} ChimiRandom;
_Static_assert(sizeof(ChimiRandom) == 4 * sizeof(uint64_t), "ChimiRandom matches chimi_runtime::random::Generator");
extern ChimiRandom chimi_random_seeded_v1(uint64_t seed);
extern uint64_t chimi_random_next_v1(ChimiRandom *generator);

static ChimiRandom random_generator;
static bool random_seed_given, random_read;
static uint64_t random_seed;
/* Guards the stream and random_read: every carrier draws from the one stream. */
static pthread_mutex_t random_lock = PTHREAD_MUTEX_INITIALIZER;

static uint64_t os_random_seed(void) {
  uint64_t seed = 0;
  FILE *source = fopen("/dev/urandom", "rb");
  if (!source || fread(&seed, sizeof(seed), 1, source) != 1) {
    struct timespec now;
    clock_gettime(CLOCK_REALTIME, &now);
    seed = ((uint64_t)now.tv_sec * UINT64_C(1000000000) + (uint64_t)now.tv_nsec) ^ ((uint64_t)getpid() << 32);
  }
  if (source) fclose(source);
  return seed;
}

static uint64_t random_next(void) {
  pthread_mutex_lock(&random_lock);
  if (!random_read) {
    random_generator = chimi_random_seeded_v1(random_seed_given ? random_seed : os_random_seed());
    random_read = true;
  }
  uint64_t result = chimi_random_next_v1(&random_generator);
  pthread_mutex_unlock(&random_lock);
  return result;
}

/* Console output (CHI-266). Under the scheduler a line written to a pipe, a
   terminal or a socket never stops the other tasks: the writer waits for the
   descriptor through CHIMI_REQUEST_IO_WAIT, as a read of "-" does. The
   descriptor stays blocking, because its open file description is shared with
   the parent shell and other processes; after poll reports it writable, one
   write(2) of at most PIPE_BUF bytes does not block on a pipe or a socket. A
   terminal may still hold such a write briefly.

   Lines from different tasks never interleave: each line goes whole onto its
   stream's queue before its writer waits (standard error shares standard
   output's queue when both name the same open file), and whichever writer the reactor
   wakes writes the front of the queue, its own bytes or an earlier line's.
   A writer returns once the queue has passed the end of its line. A line once
   queued is written whole, even when its writer is cancelled while it waits:
   later writers or the end of the program write it. A body ending under a
   terminal cancel queues its line and returns without waiting. A failed write
   drops the queue and fails every line not yet fully written.

   A socket is written with send(MSG_DONTWAIT), which never blocks and leaves
   the shared descriptor alone. The queue holds at most CONSOLE_QUEUE_LIMIT
   bytes: a writer waits before it queues a line that would pass the limit, so
   writers cancelled while they wait (a write_line under Task.timeout against
   a stalled reader) cannot grow it without bound. A line longer than the limit
   is queued when the queue is empty.

   A regular file, a program without the scheduler, and a program under the
   virtual clock write directly, as before: a regular file is always writable,
   without the scheduler there is no other task to keep running, and under the
   virtual clock the order tasks run in must not depend on how fast the reader
   drains the pipe. */
/* The queues and the lines their writers wait on live in the Rust runtime
   (runtime/chimi-runtime/src/console.rs), by index: 0 standard output, 1
   standard error, 2 the log file. Every call is made holding console_lock.
   A line's record outlives a fiber that overflows its stack while it waits,
   since the queue owns it rather than the writer's stack. */
extern bool chimi_console_push_v1(size_t queue, const void *bytes, size_t length);
extern size_t chimi_console_queued_v1(size_t queue);
extern const unsigned char *chimi_console_front_v1(size_t queue);
extern void chimi_console_advance_v1(size_t queue, size_t count);
extern void chimi_console_fail_v1(size_t queue, int error);
extern int64_t chimi_console_begin_line_v1(size_t queue);
extern int chimi_console_line_state_v1(size_t queue, int64_t line);
extern void chimi_console_end_line_v1(size_t queue, int64_t line);

typedef enum { CONSOLE_UNKNOWN, CONSOLE_DIRECT, CONSOLE_QUEUED } ConsoleMode;

typedef struct {
  int fd;
  const char *name;
  ConsoleMode mode;
  bool socket;
  FILE *file;
  /* Its own queue, or standard output's when both descriptors name the same
     open file (`2>&1`, one terminal), so their lines never interleave. */
  size_t queue;
} ConsoleStream;

enum { CONSOLE_STREAM_COUNT = 3 };
/* Standard output, standard error, and std/log's --log-file (fd -1 when
   there is none), which is written like the other two so a slow reader never
   blocks a carrier and its lines never tear or interleave. */
static ConsoleStream console_streams[CONSOLE_STREAM_COUNT] = {
  {.fd = STDOUT_FILENO, .name = "stdout"},
  {.fd = STDERR_FILENO, .name = "stderr"},
  {.fd = -1, .name = "log"},
};
/* Guards both streams and queues, since a task on any carrier writes lines.
   A writer holds it except while it parks. console_drain_all runs at exit,
   which may start inside a writer that holds it (exhausted_memory), so the
   holder is noted per carrier (Carrier.console_lock_held). */
static pthread_mutex_t console_lock = PTHREAD_MUTEX_INITIALIZER;

/* With one carrier there is no other writer, so a line takes no lock: the
   lock measured about 25 ns a line on a program that only prints.
   carrier_count changes only before carrier threads start. */
static void console_enter(void) {
  if (carrier_count == 1) return;
  pthread_mutex_lock(&console_lock);
  carrier->console_lock_held = true;
}
static void console_leave(void) {
  if (carrier_count == 1) return;
  carrier->console_lock_held = false;
  pthread_mutex_unlock(&console_lock);
}

enum { CONSOLE_QUEUE_LIMIT = 1 << 20 };

static ConsoleMode console_mode(const ChimiRuntime *runtime, ConsoleStream *stream) {
  if (stream->mode != CONSOLE_UNKNOWN) return stream->mode;
  size_t index = (size_t)(stream - console_streams);
  if (!stream->file) stream->file = stream->fd == STDOUT_FILENO ? stdout : stderr;
  stream->queue = index;
  struct stat status;
  bool known = fstat(stream->fd, &status) == 0;
  stream->socket = known && S_ISSOCK(status.st_mode);
  stream->mode = CHIMI_REQUIRES_SCHEDULER && !runtime->virtual_clock && known && !S_ISREG(status.st_mode)
    ? CONSOLE_QUEUED : CONSOLE_DIRECT;
  /* A stream naming the same open file as an earlier one shares its queue. */
  for (size_t earlier = 0; stream->mode == CONSOLE_QUEUED && earlier < index; ++earlier) {
    ConsoleStream *other_stream = &console_streams[earlier];
    struct stat other;
    if (other_stream->fd >= 0 && console_mode(runtime, other_stream) == CONSOLE_QUEUED &&
        fstat(other_stream->fd, &other) == 0 && other.st_dev == status.st_dev &&
        other.st_ino == status.st_ino) {
      stream->queue = other_stream->queue;
      break;
    }
  }
  return stream->mode;
}

static void console_enqueue(size_t queue, const void *bytes, size_t length) {
  if (!chimi_console_push_v1(queue, bytes, length)) exhausted_memory();
}

/* One write(2) from the front of the queue, at most `limit` bytes. False with
   errno set when it failed; an interruption or a descriptor a parent left
   non-blocking reports nothing written and true. */
static bool console_write_front(const ConsoleStream *stream, size_t limit) {
  size_t length = chimi_console_queued_v1(stream->queue);
  if (length > limit) length = limit;
  const unsigned char *front = chimi_console_front_v1(stream->queue);
  ssize_t count = stream->socket
    ? send(stream->fd, front, length, MSG_DONTWAIT)
    : write(stream->fd, front, length);
  if (count < 0) return errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK;
  chimi_console_advance_v1(stream->queue, (size_t)count);
  return true;
}

/* Writes the whole queue, blocking this thread. Only the end of the program
   uses it, when no task is left to keep running. */
static void console_drain_blocking(const ConsoleStream *stream) {
  while (chimi_console_queued_v1(stream->queue)) {
    struct pollfd poll_fd = {.fd = stream->fd, .events = POLLOUT};
    if (poll(&poll_fd, 1, -1) < 0 && errno != EINTR) {
      chimi_console_fail_v1(stream->queue, errno);
      return;
    }
    if (!console_write_front(stream, SIZE_MAX)) {
      chimi_console_fail_v1(stream->queue, errno);
      return;
    }
  }
}

static void console_drain_all(void) {
  bool entered = !carrier->console_lock_held;
  if (entered) console_enter();
  for (size_t index = 0; index < CONSOLE_STREAM_COUNT; ++index)
    if (console_streams[index].mode == CONSOLE_QUEUED) console_drain_blocking(&console_streams[index]);
  if (entered) console_leave();
}

/* Waits until `stream` is writable and writes the front of its queue.
   CONSOLE_WAITED after a write or a wake that only means "ask again",
   CONSOLE_CANCELLED when the writer was cancelled. When the reactor cannot
   wait on the descriptor (it is closed, or poll failed) the write fails, as a
   read of "-" does: writing anyway could block the scheduler thread. */
enum { CONSOLE_WAITED, CONSOLE_CANCELLED };

static int console_wait_and_write(const ConsoleStream *stream) {
  ChimiSchedulerRequest request = {
    .kind = CHIMI_REQUEST_IO_WAIT, .value = stream->fd, .token = CHIMI_IO_WRITABLE};
  /* The caller holds console_lock, which is never held across a park. */
  console_leave();
  bool cancel = scheduler_request(&request) == CHIMI_FIBER_CANCEL;
  console_enter();
  if (cancel) return CONSOLE_CANCELLED;
  if (request.status == CHIMI_RESPONSE_PENDING) return CONSOLE_WAITED;
  if (request.status == CHIMI_RESPONSE_ERROR) {
    chimi_console_fail_v1(stream->queue, fcntl(stream->fd, F_GETFD) < 0 ? EBADF : EIO);
    return CONSOLE_WAITED;
  }
  /* Another writer may have emptied the queue while this one waited. */
  if (chimi_console_queued_v1(stream->queue) && !console_write_front(stream, PIPE_BUF))
    chimi_console_fail_v1(stream->queue, errno);
  return CONSOLE_WAITED;
}

/* Writes `text` and a newline. 0 when written (or, for a body ending under a
   terminal cancel, queued), -1 when cancelled, otherwise the errno of the
   failed write. The caller holds console_lock. */
static int console_write_line_locked(const ChimiRuntime *runtime, ConsoleStream *stream,
                                     const char *text, size_t length) {
  if (console_mode(runtime, stream) == CONSOLE_DIRECT) {
    errno = 0;
    /* The newline goes out even after a short write, so a later line never
       joins a broken one. */
    bool written = fwrite(text, 1, length, stream->file) == length;
    written = fputc('\n', stream->file) != EOF && written;
    written = fflush(stream->file) == 0 && written;
    if (written) return 0;
    int error = errno ? errno : EIO;
    clearerr(stream->file);
    return error;
  }
  size_t queue = stream->queue;
  /* A body ending under a terminal cancel must not park (CHI-265), and must
     not block the other tasks either: its line is queued for later writers or
     the end of the program, past the limit if need be. */
  bool terminal = chimi_fiber_cancel_is_terminal();
  /* Nothing is queued yet, so a cancellation here loses nothing. */
  size_t queued;
  while (!terminal && (queued = chimi_console_queued_v1(queue)) &&
         queued + length + 1 > CONSOLE_QUEUE_LIMIT)
    if (console_wait_and_write(stream) == CONSOLE_CANCELLED) return -1;
  console_enqueue(queue, text, length);
  console_enqueue(queue, "\n", 1);
  if (terminal) return 0;
  int64_t line = chimi_console_begin_line_v1(queue);
  if (line < 0) exhausted_memory();
  int outcome;
  while ((outcome = chimi_console_line_state_v1(queue, line)) < 0)
    if (console_wait_and_write(stream) == CONSOLE_CANCELLED) { outcome = -1; break; }
  chimi_console_end_line_v1(queue, line);
  return outcome;
}

static int console_write_line(const ChimiRuntime *runtime, ConsoleStream *stream,
                              const char *text, size_t length) {
  console_enter();
  int outcome = console_write_line_locked(runtime, stream, text, length);
  console_leave();
  return outcome;
}

static ChimiControl *invoke_host(ChimiRuntime *runtime, HostFunction function,
                                 ChimiValue **arguments, uint32_t argument_count,
                                 uint64_t span, uint32_t call_node,
                                 ChimiEnvironment *environment) {
  if (function == HOST_RANDOM_NEXT_INT) {
    if (argument_count != 0) return defect("verified host function has invalid runtime arguments", call_node);
    return runtime->cancel ? cancelled() : success(integer_value((int64_t)random_next()));
  }
  if (function == HOST_PROCESS_UTF8_LINES)
    return process_utf8_lines(runtime, arguments, argument_count, span, call_node, environment);
  if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT)
    return defect("verified host function has invalid runtime arguments", call_node);
  if (runtime->cancel) return cancelled();
  ConsoleStream *stream = &console_streams[function == HOST_WRITE_LINE ? 0 : 1];
  int written = console_write_line(runtime, stream, arguments[0]->as.text.bytes, arguments[0]->as.text.length);
  if (written < 0) return cancelled();
  if (written > 0) {
    ChimiValue *fields[] = {text_value(stream->name), text_value(strerror(written))};
    return failure(standard_value(CHIMI_CONSTRUCTOR_CONSOLE_WRITE_FAILED, fields, 2));
  }
  return runtime->cancel ? cancelled() : success(void_value());
}

/* std/log's host core. host_write gets the message and fields already encoded
   as JSON and writes one line, `{"time":..,"level":..,"message":..,"fields":..}`,
   whole: to the --log-file file, or to standard error through the console
   queue, so log lines and the program's own write_error lines never
   interleave. A line that cannot be written is lost: logging never fails. */
static ChimiControl *invoke_log_operation(ChimiRuntime *runtime, uint32_t intrinsic,
                                          ChimiValue **arguments, uint32_t argument_count,
                                          uint32_t call_node) {
  if (intrinsic == CHIMI_INTRINSIC_LOG_HOST_MINIMUM_LEVEL) {
    if (argument_count != 0) return intrinsic_shape_defect(call_node);
    return success(integer_value(runtime->log_level));
  }
  if (argument_count != 3 || arguments[0]->kind != VALUE_INTEGER || arguments[1]->kind != VALUE_TEXT ||
      arguments[2]->kind != VALUE_TEXT)
    return intrinsic_shape_defect(call_node);
  if (runtime->cancel) return cancelled();
  static const char *const names[] = {"debug", "info", "warn", "error"};
  int64_t level = arguments[0]->as.integer;
  const char *name = names[level < 0 ? 0 : level > 3 ? 3 : level];
  struct timespec now;
  clock_gettime(CLOCK_REALTIME, &now);
  int64_t millis = (int64_t)now.tv_nsec / 1000000;
  char time[CHIMI_TIMESTAMP_TEXT];
  chimi_timestamp_date_time_v1((int64_t)now.tv_sec, time);
  size_t message_length = arguments[1]->as.text.length, fields_length = arguments[2]->as.text.length;
  size_t capacity = 128 + message_length + fields_length;
  char *line = malloc(capacity);
  if (!line) exhausted_memory();
  int head = snprintf(line, capacity, "{\"time\":\"%s.%03" PRId64 "Z\",\"level\":\"%s\",\"message\":", time,
                      millis, name);
  size_t length = (size_t)head;
  memcpy(line + length, arguments[1]->as.text.bytes, message_length);
  length += message_length;
  memcpy(line + length, ",\"fields\":", 10);
  length += 10;
  memcpy(line + length, arguments[2]->as.text.bytes, fields_length);
  length += fields_length;
  line[length++] = '}';
  ConsoleStream *stream = &console_streams[runtime->log_file ? 2 : 1];
  int written = console_write_line(runtime, stream, line, length);
  free(line);
  if (written < 0) return cancelled();
  return runtime->cancel ? cancelled() : success(void_value());
}

/* std/metrics' host core: one aggregate per name and label set, written as
   one JSON document to the --metrics-file file when the program ends. It is
   chimi_runtime::metrics, which the interpreter keeps its measurements in
   too, so both backends write the same document. Every carrier records into
   the one aggregate under metrics_lock. */
typedef struct ChimiMetrics ChimiMetrics;
extern ChimiMetrics *chimi_metrics_new_v1(void);
extern void chimi_metrics_record_v1(ChimiMetrics *aggregate, int64_t kind, const char *name, size_t name_length,
                                    double value, const char *labels, size_t labels_length);
/* A text Rust built, which C copies and then hands back (text_abi.rs). */
typedef struct { const char *bytes; size_t length; void *owned; } ChimiOwnedText;
void chimi_text_release_v1(ChimiOwnedText owned);
extern ChimiOwnedText chimi_metrics_document_v1(const ChimiMetrics *aggregate, int64_t start_unix_ns,
                                                int64_t time_unix_ns);
extern void chimi_metrics_free_v1(ChimiMetrics *aggregate);

static pthread_mutex_t metrics_lock = PTHREAD_MUTEX_INITIALIZER;
/* Set by the first Metrics.runner_document: from then on the provider
   records without --metrics-file. */
static atomic_bool metrics_reading;
/* When the aggregate began: the run's start, or the first repeat's. */
static int64_t metrics_started_unix_ns;
/* Made by the first measurement or document; guarded by metrics_lock. */
static ChimiMetrics *metrics_aggregate;

static ChimiMetrics *metrics_held(void) {
  if (!metrics_aggregate) metrics_aggregate = chimi_metrics_new_v1();
  return metrics_aggregate;
}

/* Adds one measurement from any carrier. */
static void metrics_add(int64_t kind, const char *name, size_t name_length, double value, const char *labels,
                        size_t labels_length) {
  pthread_mutex_lock(&metrics_lock);
  chimi_metrics_record_v1(metrics_held(), kind, name, name_length, value, labels, labels_length);
  pthread_mutex_unlock(&metrics_lock);
}

static bool close_output(FILE *output);

/* Whether the provider records: with --metrics-file, or once the program
   has read the aggregate. */
static bool metrics_recording(const ChimiRuntime *runtime) {
  return runtime->metrics_file != NULL || atomic_load_explicit(&metrics_reading, memory_order_acquire);
}

/* Writes the metrics document, whose counts began at `started`, to `output`
   as it stands now. */
static void write_metrics_document(const ChimiRuntime *runtime, int64_t started, FILE *output) {
  /* At exit, `main` has ended and the clock is where its last fiber left it;
     mid-run, it is the reading fiber's time. */
  int64_t ended;
  if (__builtin_add_overflow(runtime->start_unix_ns, chimi_fiber_now(), &ended)) ended = INT64_MAX;
  pthread_mutex_lock(&metrics_lock);
  ChimiOwnedText document = chimi_metrics_document_v1(metrics_held(), started, ended);
  pthread_mutex_unlock(&metrics_lock);
  fwrite(document.bytes, 1, document.length, output);
  chimi_text_release_v1(document);
}

/* Writes the metrics document at exit; true when the write failed. */
static bool write_metrics(const ChimiRuntime *runtime, int64_t started, FILE *output) {
  if (!output) return false;
  write_metrics_document(runtime, started, output);
  return close_output(output);
}

/* Metrics.runner_document: the aggregate so far, and recording from now on. */
static ChimiValue *metrics_document_now(const ChimiRuntime *runtime) {
  atomic_store_explicit(&metrics_reading, true, memory_order_release);
  char *bytes = NULL;
  size_t length = 0;
  FILE *output = open_memstream(&bytes, &length);
  if (!output) exhausted_memory();
  write_metrics_document(runtime, metrics_started_unix_ns, output);
  if (fclose(output) != 0) exhausted_memory();
  ChimiValue *document = text_from_bytes(bytes, length);
  free(bytes);
  return document;
}

static ChimiControl *invoke_metrics_operation(ChimiRuntime *runtime, uint32_t intrinsic,
                                              ChimiValue **arguments, uint32_t argument_count,
                                              uint32_t call_node) {
  if (intrinsic == CHIMI_INTRINSIC_METRICS_HOST_ENABLED) {
    if (argument_count != 0) return intrinsic_shape_defect(call_node);
    return success(boolean_value(metrics_recording(runtime)));
  }
  if (intrinsic == CHIMI_INTRINSIC_METRICS_HOST_DOCUMENT) {
    if (argument_count != 0) return intrinsic_shape_defect(call_node);
    if (runtime->cancel) return cancelled();
    return success(metrics_document_now(runtime));
  }
  if (argument_count != 4 || arguments[0]->kind != VALUE_INTEGER || arguments[1]->kind != VALUE_TEXT ||
      arguments[2]->kind != VALUE_FLOAT || arguments[3]->kind != VALUE_TEXT)
    return intrinsic_shape_defect(call_node);
  if (runtime->cancel) return cancelled();
  if (metrics_recording(runtime))
    metrics_add(arguments[0]->as.integer, arguments[1]->as.text.bytes, arguments[1]->as.text.length,
                arguments[2]->as.floating, arguments[3]->as.text.bytes, arguments[3]->as.text.length);
  return runtime->cancel ? cancelled() : success(void_value());
}

/* C bindings (G20). A binding function (chimi_binding.h) sees a call, never
   the runtime: values cross as ChimiBindingValue, which is ChimiValue, and its
   one result as ChimiBindingResult, which is ChimiControl. */
struct ChimiBindingCall {
  ChimiRuntime *runtime;
  ChimiValue **arguments;
  uint32_t argument_count;
  uint32_t call_node;
  /* Which arguments are open handles the call holds (binding_hold). */
  bool *held;
};

static ChimiBindingValue *binding_value(ChimiValue *value) { return (ChimiBindingValue *)value; }
static ChimiValue *runtime_value(ChimiBindingValue *value) { return (ChimiValue *)value; }
static ChimiBindingResult *binding_result(ChimiControl *control) {
  return (ChimiBindingResult *)control;
}

/* Handles: a slot per live resource. A value names its slot and the slot's
   generation, so a copy of a closed handle stays closed when the slot is reused.
   Slot 0 is never used, so a zeroed value is never open.

   A handle value may reach tasks on any carrier, so the table is behind
   binding_handle_lock, which is taken after the kernel lock (a count's zero,
   native_handle_unreferenced, arrives in scheduler hooks) and is never held
   across a scheduler request or a type's release. A binding call holds every
   open handle among its arguments for its whole length (binding_hold), so a
   close by another task, or by the call itself, only marks the handle closed:
   its state stays valid until the last call holding it returns, which
   releases it.

   One binding call at a time runs on a handle (`runner`), so a binding's
   state needs no lock of its own when tasks on several carriers share the
   handle. A call takes every handle it holds when it starts, waiting on
   binding_handle_free while another call runs on any of them, and gives
   them back while it waits on a descriptor or a child or makes any other
   scheduler request (binding_run, binding_request), so calls on one handle
   interleave only where a task can park, as they do on one carrier. A
   call that would wait for a handle blocks its carrier's thread, not just
   its task: binding code between parks is meant to be short. */
/* The slots live in Rust (runtime/chimi-runtime/src/handles.rs): slots,
   free list, generations, holders and runners. Every call below is made with
   binding_handle_lock held, and none makes a system call, a scheduler
   request or a release: a state the table hands back is released here once
   the lock is let go. */
typedef struct {
  const ChimiBindingType *type;
  void *state;
} BindingRelease;

extern bool chimi_handles_open_v1(const ChimiBindingType *type, void *state, uint32_t *slot,
                                  uint32_t *generation);
extern void *chimi_handles_state_v1(uint32_t slot, uint32_t generation, const ChimiBindingType *type);
extern bool chimi_handles_is_open_v1(uint32_t slot, uint32_t generation);
enum { HANDLES_CLOSE_NOT_OPEN, HANDLES_CLOSE_HELD, HANDLES_CLOSE_RELEASED };
extern int32_t chimi_handles_close_v1(uint32_t slot, uint32_t generation, const ChimiBindingType *type,
                                      BindingRelease *out);
extern bool chimi_handles_hold_v1(uint32_t slot, uint32_t generation);
extern bool chimi_handles_let_go_v1(uint32_t slot, BindingRelease *out);
extern const void *chimi_handles_runner_v1(uint32_t slot);
extern void chimi_handles_set_runner_v1(uint32_t slot, const void *runner);
extern uint32_t chimi_handles_count_v1(void);
extern bool chimi_handles_end_v1(uint32_t slot, BindingRelease *out);

static pthread_mutex_t binding_handle_lock = PTHREAD_MUTEX_INITIALIZER;
/* Signalled when a call stops running on a handle. */
static pthread_cond_t binding_handle_free = PTHREAD_COND_INITIALIZER;

/* True when `value` names an open handle. The caller holds binding_handle_lock. */
static bool binding_handle_open(const ChimiValue *value) {
  return value->kind == VALUE_NATIVE_HANDLE &&
         chimi_handles_is_open_v1(value->as.handle.slot, value->as.handle.generation);
}

/* The scheduler's key for the waits on a handle's descriptors: nonzero,
   since slot 0 is unused, and distinct per generation. The scheduler reads
   it as slot and generation, and treats a withdraw as final for that
   generation and older ones of the slot (CHIMI_REQUEST_IO_WITHDRAW), so a
   slot's generation must only grow. */
static uint64_t binding_resource_key(uint32_t slot, uint32_t generation) {
  return (uint64_t)slot << 32 | generation;
}

static void binding_release_run(BindingRelease release) {
  if (release.type && release.type->release) release.type->release(release.state);
}

static ChimiFiberCommand binding_request(const ChimiBindingCall *call, void *request);

/* Closes the handle (slot, generation) of `type` (NULL for any) when it is
   open. A close from a binding call (`call`; NULL from a collection or a
   hook) then wakes every task waiting on the handle's descriptors, so none
   stays parked on a descriptor the release frees for reuse; each answers CHIMI_BINDING_WAIT_CLOSED. The
   handle reads as closed before the withdraw, so a waiter that wakes sees it
   closed. The state is released here unless a binding call holds the
   handle. */
static void binding_handle_close(uint32_t slot, uint32_t generation, const ChimiBindingType *type,
                                 const ChimiBindingCall *call) {
  BindingRelease release = {0};
  pthread_mutex_lock(&binding_handle_lock);
  bool closed = chimi_handles_close_v1(slot, generation, type, &release) != HANDLES_CLOSE_NOT_OPEN;
  pthread_mutex_unlock(&binding_handle_lock);
  /* The answer is always OK: the withdraw never parks. A forced process body
     that gets CANCEL here is abandoned at its next park, so releasing anyway
     is safe. */
  if (closed && call && CHIMI_REQUIRES_SCHEDULER) {
    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_IO_WITHDRAW,
                                     .channel = binding_resource_key(slot, generation)};
    binding_request(call, &request);
  }
  binding_release_run(release);
}

/* Closes the handle `key` names when it is still open: no value names it any
   more. No task can be waiting on its descriptors, since a waiting binding
   call holds the handle as an argument, so nothing is withdrawn; this runs
   inside collections and scheduler hooks, where no request may be issued. */
static void native_handle_unreferenced(uint64_t key) {
  binding_handle_close((uint32_t)((key & ~NATIVE_HANDLE_KEY) >> 32), (uint32_t)key, NULL, NULL);
}

/* Runs every handle's release once, when the program ends and nothing runs:
   the open ones, and the ones closed while held. A call can still count as
   holding a handle then, when its task was abandoned inside it (a forced
   process body that parked again), so holders are ignored. */
static void release_binding_handles(void) {
  for (uint32_t slot = 1;; ++slot) {
    BindingRelease release = {0};
    pthread_mutex_lock(&binding_handle_lock);
    if (slot >= chimi_handles_count_v1()) {
      pthread_mutex_unlock(&binding_handle_lock);
      return;
    }
    chimi_handles_end_v1(slot, &release);
    pthread_mutex_unlock(&binding_handle_lock);
    binding_release_run(release);
  }
}

/* True when some call runs on a handle `call` holds. `call` itself never
   does here: it takes its handles only when it runs on none of them. The
   caller holds binding_handle_lock. */
static bool binding_run_blocked(const ChimiBindingCall *call) {
  for (uint32_t index = 0; index < call->argument_count; ++index) {
    if (!call->held[index]) continue;
    if (chimi_handles_runner_v1(call->arguments[index]->as.handle.slot) != NULL) return true;
  }
  return false;
}

/* Starts (`run`) or stops `call` running on every handle it holds. Starting
   waits until no other call runs on any of them and then takes them all at
   once, so two calls sharing handles never each wait for the other. The
   caller holds binding_handle_lock. */
static void binding_run_locked(const ChimiBindingCall *call, bool run) {
  if (run)
    while (binding_run_blocked(call)) pthread_cond_wait(&binding_handle_free, &binding_handle_lock);
  bool any = false;
  for (uint32_t index = 0; index < call->argument_count; ++index) {
    if (!call->held[index]) continue;
    chimi_handles_set_runner_v1(call->arguments[index]->as.handle.slot, run ? call : NULL);
    any = true;
  }
  if (any && !run) pthread_cond_broadcast(&binding_handle_free);
}

/* Lets the call's handles go while it waits (`run` false), or takes them
   back when the wait ends (`run` true). */
static void binding_run(const ChimiBindingCall *call, bool run) {
  pthread_mutex_lock(&binding_handle_lock);
  binding_run_locked(call, run);
  pthread_mutex_unlock(&binding_handle_lock);
}

/* A scheduler request from inside `call`, made with its handles let go.
   Even a request that answers at once parks the fiber, and the scheduler
   abandons a forced process body at any park, which would leave a handle
   taken for good; and on one carrier another task may run first, whose
   call on the handle would then wait on the thread this one needs. */
static ChimiFiberCommand binding_request(const ChimiBindingCall *call, void *request) {
  binding_run(call, false);
  ChimiFiberCommand command = scheduler_request(request);
  binding_run(call, true);
  return command;
}

/* Holds (`hold`) or lets go of every open handle among a binding call's
   arguments, marking the ones held in `call->held`, and starts or stops
   the call running on them. The last call to let go of a handle closed
   meanwhile releases its state. */
static void binding_hold(ChimiBindingCall *call, bool hold) {
  ChimiValue **arguments = call->arguments;
  bool *held = call->held;
  uint32_t count = call->argument_count;
  BindingRelease releases[8];
  uint32_t release_count = 0;
  pthread_mutex_lock(&binding_handle_lock);
  if (hold) {
    for (uint32_t index = 0; index < count; ++index)
      held[index] = arguments[index]->kind == VALUE_NATIVE_HANDLE &&
                    chimi_handles_hold_v1(arguments[index]->as.handle.slot,
                                          arguments[index]->as.handle.generation);
    binding_run_locked(call, true);
    pthread_mutex_unlock(&binding_handle_lock);
    return;
  }
  /* Every let-go is taken back before the binding code goes on. */
  for (uint32_t index = 0; index < count; ++index)
    if (held[index] && chimi_handles_runner_v1(arguments[index]->as.handle.slot) != call) abort();
  binding_run_locked(call, false);
  for (uint32_t index = 0; index < count; ++index) {
    if (!held[index]) continue;
    BindingRelease release;
    if (!chimi_handles_let_go_v1(arguments[index]->as.handle.slot, &release)) continue;
    if (release_count == 8) {
      /* Rare: more closed handles than the buffer holds; run the batch first. */
      pthread_mutex_unlock(&binding_handle_lock);
      for (uint32_t pending = 0; pending < release_count; ++pending) binding_release_run(releases[pending]);
      release_count = 0;
      pthread_mutex_lock(&binding_handle_lock);
    }
    releases[release_count++] = release;
  }
  pthread_mutex_unlock(&binding_handle_lock);
  for (uint32_t index = 0; index < release_count; ++index) binding_release_run(releases[index]);
}

static const ChimiValue *binding_argument(const ChimiBindingCall *call, uint32_t index,
                                          ValueKind kind) {
  if (index >= call->argument_count || call->arguments[index]->kind != kind) return NULL;
  return call->arguments[index];
}

uint32_t chimi_binding_argument_count(const ChimiBindingCall *call) { return call->argument_count; }

int chimi_binding_integer(const ChimiBindingCall *call, uint32_t index, int64_t *out) {
  const ChimiValue *value = binding_argument(call, index, VALUE_INTEGER);
  if (value) *out = value->as.integer;
  return value != NULL;
}

int chimi_binding_boolean(const ChimiBindingCall *call, uint32_t index, int *out) {
  const ChimiValue *value = binding_argument(call, index, VALUE_BOOLEAN);
  if (value) *out = value->as.boolean;
  return value != NULL;
}

int chimi_binding_text(const ChimiBindingCall *call, uint32_t index, const char **bytes,
                       size_t *length) {
  const ChimiValue *value = binding_argument(call, index, VALUE_TEXT);
  if (value) { *bytes = value->as.text.bytes; *length = value->as.text.length; }
  return value != NULL;
}

int chimi_binding_bytes(const ChimiBindingCall *call, uint32_t index,
                        const unsigned char **bytes, size_t *length) {
  const ChimiValue *value = binding_argument(call, index, VALUE_BYTES);
  if (value) { *bytes = (const unsigned char *)value->as.text.bytes; *length = value->as.text.length; }
  return value != NULL;
}

void *chimi_binding_handle(const ChimiBindingCall *call, uint32_t index,
                           const ChimiBindingType *type) {
  const ChimiValue *value = binding_argument(call, index, VALUE_NATIVE_HANDLE);
  if (value == NULL) return NULL;
  if (type == NULL) return NULL;
  pthread_mutex_lock(&binding_handle_lock);
  void *state = chimi_handles_state_v1(value->as.handle.slot, value->as.handle.generation, type);
  pthread_mutex_unlock(&binding_handle_lock);
  return state;
}

ChimiBindingValue *chimi_binding_void(ChimiBindingCall *call) {
  (void)call;
  return binding_value(void_value());
}

ChimiBindingValue *chimi_binding_make_integer(ChimiBindingCall *call, int64_t value) {
  (void)call;
  return binding_value(integer_value(value));
}

ChimiBindingValue *chimi_binding_make_boolean(ChimiBindingCall *call, int value) {
  (void)call;
  return binding_value(boolean_value(value != 0));
}

/* Text that is not UTF-8 becomes NULL, which the result functions report as a
   defect: a binding must never hand Chimi invalid Text. */
ChimiBindingValue *chimi_binding_make_text(ChimiBindingCall *call, const char *bytes,
                                           size_t length) {
  (void)call;
  if (bytes == NULL) return NULL;
  /* Text may hold U+0000, which valid_utf8 refuses, so check between them. */
  for (size_t start = 0; start <= length;) {
    const char *nul = memchr(bytes + start, 0, length - start);
    size_t end = nul ? (size_t)(nul - bytes) : length;
    if (!valid_utf8((const unsigned char *)bytes + start, end - start)) return NULL;
    start = end + 1;
  }
  return binding_value(text_from_bytes(bytes, length));
}

ChimiBindingValue *chimi_binding_make_bytes(ChimiBindingCall *call, const unsigned char *bytes,
                                            size_t length) {
  (void)call;
  return binding_value(bytes_from((const char *)bytes, length));
}

ChimiBindingValue *chimi_binding_none(ChimiBindingCall *call) {
  (void)call;
  return binding_value(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
}

ChimiBindingValue *chimi_binding_some(ChimiBindingCall *call, ChimiBindingValue *value) {
  (void)call;
  if (value == NULL) return NULL;
  ChimiValue *fields[] = {runtime_value(value)};
  return binding_value(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
}

static bool binding_items_present(ChimiBindingValue **items, uint32_t length) {
  for (uint32_t index = 0; index < length; ++index)
    if (items[index] == NULL) return false;
  return true;
}

ChimiBindingValue *chimi_binding_make_tuple(ChimiBindingCall *call, ChimiBindingValue **items,
                                            uint32_t length) {
  (void)call;
  if (!binding_items_present(items, length)) return NULL;
  ChimiValue *tuple = new_value(VALUE_TUPLE);
  tuple->as.record.length = length;
  tuple->as.record.fields = allocate(length, sizeof(*tuple->as.record.fields));
  allocation_kind(tuple->as.record.fields, ALLOCATION_FIELDS);
  for (uint32_t index = 0; index < length; ++index)
    tuple->as.record.fields[index] = (RuntimeField){NULL, runtime_value(items[index])};
  return binding_value(tuple);
}

ChimiBindingValue *chimi_binding_make_list(ChimiBindingCall *call, ChimiBindingValue **items,
                                           uint32_t length) {
  (void)call;
  if (!binding_items_present(items, length)) return NULL;
  return binding_value(list_from_values((ChimiValue *const *)items, length));
}

ChimiBindingValue *chimi_binding_make_handle(ChimiBindingCall *call, const ChimiBindingType *type,
                                             void *state) {
  (void)call;
  uint32_t slot, generation;
  pthread_mutex_lock(&binding_handle_lock);
  bool opened = chimi_handles_open_v1(type, state, &slot, &generation);
  pthread_mutex_unlock(&binding_handle_lock);
  if (!opened) {
    if (type->release) type->release(state);
    return NULL;
  }
  ChimiValue *value = new_value(VALUE_NATIVE_HANDLE);
  value->as.handle.slot = slot;
  value->as.handle.generation = generation;
  value->as.handle.name = type->name;
  counted_handle_retain(native_handle_key(slot, generation));
  return binding_value(value);
}

void chimi_binding_close_handle(ChimiBindingCall *call, uint32_t index,
                                const ChimiBindingType *type) {
  const ChimiValue *value = binding_argument(call, index, VALUE_NATIVE_HANDLE);
  if (value != NULL) binding_handle_close(value->as.handle.slot, value->as.handle.generation, type, call);
}

ChimiBindingResult *chimi_binding_success(ChimiBindingCall *call, ChimiBindingValue *value) {
  if (value == NULL)
    return binding_result(defect("a C binding built an invalid value", call->call_node));
  return binding_result(success(runtime_value(value)));
}

ChimiBindingResult *chimi_binding_ok(ChimiBindingCall *call, ChimiBindingValue *value) {
  if (value == NULL)
    return binding_result(defect("a C binding built an invalid value", call->call_node));
  ChimiValue *fields[] = {runtime_value(value)};
  return binding_result(success(standard_value(CHIMI_CONSTRUCTOR_RESULT_OK, fields, 1)));
}

ChimiBindingResult *chimi_binding_failure(ChimiBindingCall *call, int64_t code,
                                          const char *message) {
  ChimiBindingValue *parts[] = {
    chimi_binding_make_integer(call, code),
    chimi_binding_make_text(call, message, message ? strlen(message) : 0),
  };
  if (parts[1] == NULL)
    return binding_result(defect("a C binding failed with a message that is not UTF-8",
                                 call->call_node));
  ChimiValue *fields[] = {runtime_value(chimi_binding_make_tuple(call, parts, 2))};
  return binding_result(success(standard_value(CHIMI_CONSTRUCTOR_RESULT_ERR, fields, 1)));
}

ChimiBindingResult *chimi_binding_defect(ChimiBindingCall *call, const char *message) {
  if (message == NULL || !valid_utf8((const unsigned char *)message, strlen(message)))
    return binding_result(defect("a C binding defected with a message that is not UTF-8",
                                 call->call_node));
  return binding_result(defect(copy_text(message), call->call_node));
}

ChimiBindingResult *chimi_binding_cancelled(ChimiBindingCall *call) {
  (void)call;
  return binding_result(cancelled());
}

int chimi_binding_is_cancelled(const ChimiBindingCall *call) {
  if (call->runtime->cancel) return 1;
  if (!CHIMI_REQUIRES_SCHEDULER) return 0;
  ChimiSchedulerRequest check = {.kind = CHIMI_REQUEST_SCOPE_CHECK};
  return binding_request(call, &check) == CHIMI_FIBER_CANCEL;
}

/* True when `handle` names a handle argument that is closed now. */
static bool binding_wait_handle_closed(const ChimiBindingCall *call, uint32_t handle) {
  if (handle == CHIMI_BINDING_NO_HANDLE) return false;
  const ChimiValue *value = binding_argument(call, handle, VALUE_NATIVE_HANDLE);
  if (value == NULL) return true;
  pthread_mutex_lock(&binding_handle_lock);
  bool closed = !binding_handle_open(value);
  pthread_mutex_unlock(&binding_handle_lock);
  return closed;
}

static int binding_wait_fd(ChimiBindingCall *call, uint32_t handle, int fd, int interest);

int chimi_binding_wait(ChimiBindingCall *call, uint32_t handle, int fd, int interest) {
  if (fd < 0 || interest < CHIMI_BINDING_READABLE ||
      interest > (CHIMI_BINDING_READABLE | CHIMI_BINDING_WRITABLE))
    return CHIMI_BINDING_WAIT_FAILED;
  if (binding_wait_handle_closed(call, handle)) return CHIMI_BINDING_WAIT_CLOSED;
  binding_run(call, false);
  int answer = binding_wait_fd(call, handle, fd, interest);
  binding_run(call, true);
  return answer;
}

/* chimi_binding_wait's wait, run while the call's handles are let go. */
static int binding_wait_fd(ChimiBindingCall *call, uint32_t handle, int fd, int interest) {
  if (CHIMI_REQUIRES_SCHEDULER) {
    uint64_t resource = 0;
    if (handle != CHIMI_BINDING_NO_HANDLE) {
      const ChimiValue *value = binding_argument(call, handle, VALUE_NATIVE_HANDLE);
      resource = binding_resource_key(value->as.handle.slot, value->as.handle.generation);
    }
    uint64_t token = ((interest & CHIMI_BINDING_READABLE) ? CHIMI_IO_READABLE : 0) |
                     ((interest & CHIMI_BINDING_WRITABLE) ? CHIMI_IO_WRITABLE : 0);
    for (;;) {
      ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_IO_WAIT, .value = fd, .token = token, .channel = resource};
      if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return CHIMI_BINDING_WAIT_CANCELLED;
      /* Another task may have closed the handle while this one was parked;
         its descriptor number may already belong to a new resource. */
      if (binding_wait_handle_closed(call, handle)) return CHIMI_BINDING_WAIT_CLOSED;
      if (request.status == CHIMI_RESPONSE_ERROR) return CHIMI_BINDING_WAIT_FAILED;
      if (request.status != CHIMI_RESPONSE_PENDING) return CHIMI_BINDING_READY;
    }
  }
  struct pollfd poll_fd = {
    .fd = fd,
    .events = (short)(((interest & CHIMI_BINDING_READABLE) ? POLLIN : 0) |
                      ((interest & CHIMI_BINDING_WRITABLE) ? POLLOUT : 0)),
  };
  for (;;) {
    if (call->runtime->cancel) return CHIMI_BINDING_WAIT_CANCELLED;
    int ready = poll(&poll_fd, 1, -1);
    if (ready > 0) return (poll_fd.revents & POLLNVAL) ? CHIMI_BINDING_WAIT_FAILED : CHIMI_BINDING_READY;
    if (ready < 0 && errno != EINTR) return CHIMI_BINDING_WAIT_FAILED;
  }
}

/* Calls the reached binding `binding`; its result is the call's outcome. */
static ChimiControl *invoke_binding(ChimiRuntime *runtime, uint32_t binding, ChimiValue **arguments,
                                    uint32_t argument_count, uint32_t call_node) {
  if (binding >= CHIMI_BINDING_COUNT) return defect("verified binding call has no binding", call_node);
  if (runtime->cancel) return cancelled();
  bool held_small[8];
  bool *held = argument_count <= 8 ? held_small : malloc(argument_count * sizeof(*held));
  if (!held) exhausted_memory();
  ChimiBindingCall call = {runtime, arguments, argument_count, call_node, held};
  binding_hold(&call, true);
  ChimiControl *control = (ChimiControl *)CHIMI_BINDINGS[binding].function(&call);
  binding_hold(&call, false);
  if (held != held_small) free(held);
  if (control == NULL) return defect("a C binding returned no result", call_node);
  return control;
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
      uint64_t dependency_span = take_spans(runtime, 1);
      emit_dependency(runtime, call_node, dependency_span, span, callable);
    }
    return invoke(runtime, callable->inner, arguments, argument_count, span, call_node, call_environment);
  }
  if (callable->kind == CALL_INTRINSIC && callable->intrinsic == CHIMI_INTRINSIC_BINDING_CALL)
    return invoke_binding(runtime, CHIMI_SYMBOLS[callable->symbol].binding, arguments,
                          argument_count, call_node);
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

/* Aborts unless this thread entered its own carrier. With one carrier every
   thread may use carrier 0; with several, a thread that never entered its
   carrier would share carrier 0's scratch. `what` names the fiber started. */
static void require_bound_carrier(const char *what) {
  if (carrier_count > 1 && !(carrier->bound && pthread_equal(carrier->thread, pthread_self()))) {
    fprintf(stderr, "Chimi runtime defect: %s started on a thread without its own carrier\n", what);
    abort();
  }
}

static ChimiFiberResult chimi_task_entry(void *raw_context) {
  ChimiTaskContext *context = raw_context;
  require_bound_carrier("a task");
  carrier->fiber.target = NULL;
  if (context->entry) {
    carrier->fiber.heap = &main_heap;
    return task_fiber_result(context->entry(context->runtime, context->environment, context->span));
  }
  ChimiRuntime *runtime = context->runtime;
  uint64_t span = context->span;
  RuntimeHeap *heap = heap_create();
  carrier->fiber.heap = heap;
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
  carrier->fiber.heap = NULL;
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
   running, so they free fragments without touching the fiber context. A task word is
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

/* `std/duration`'s intrinsics are Rust (intrinsics/durations.rs); the text form is
   chimi-runtime's duration.rs, which the debug renderer and messages here use too. */
static ChimiValue *duration_value(int64_t nanoseconds) { return chimi_value_integer_v1(VALUE_DURATION, nanoseconds); }

size_t chimi_duration_text_v1(int64_t nanoseconds, char *out);

/* The canonical text form (chimi-runtime's duration.rs) into `text`, which holds
   at least 32 bytes. */
static void duration_text(int64_t duration, char *text) { chimi_duration_text_v1(duration, text); }

static ChimiControl *intrinsic_shape_defect(uint32_t call_node);


/* The debug format of `std/debug` (G10), in intrinsics/debug.rs. It MUST
   match the interpreter's renderer in execution/debug.rs byte for byte; the
   language tests compare both. */
ChimiValue *chimi_runtime_debug_render_v1(const ChimiValue *value, bool indented);
static ChimiValue *debug_render(const ChimiValue *value, bool indented) {
  return chimi_runtime_debug_render_v1(value, indented);
}
static void debug_text(TextBuffer *buffer, const char *value) { buffer_bytes(buffer, value, strlen(value)); }
/* process_name_label, for the renderer's `<address name>`. */
const char *chimi_runtime_process_name_label_v1(uint64_t name);
const char *chimi_runtime_process_name_label_v1(uint64_t name) { return process_name_label(name); }

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

/* Process.send to a test address: the mailbox rules without a scheduler. It never
   parks; it refuses with SendError.Full once the mailbox holds its capacity, and
   the sender keeps the message. A message carrying a Receiver or Completion would
   need the capability transfer a process receive makes, so a test mailbox with
   room refuses one as a defect. */
static ChimiControl *send_to_test_mailbox_locked(ChimiValue *address, ChimiValue *message,
                                                 uint32_t call_node) {
  uint64_t id = address->as.address.target;
  if (id == 0 || id > test_log_count || test_logs[id - 1].kind != VALUE_TEST_MAILBOX) abort();
  TestLog *log = &test_logs[id - 1];
  ChimiValue *fields[] = {message};
  if (log->length >= log->limit)
    return failure(standard_value(CHIMI_CONSTRUCTOR_PROCESS_SEND_FULL, fields, 1));
  void *roots[] = {message};
  AllocationKind kinds[] = {ALLOCATION_VALUE};
  RuntimeFragment *fragment = NULL;
  carrier->copy_counting = true;
  HeapStatus status = fragment_copy(roots, kinds, 1, &fragment);
  carrier->copy_counting = false;
  if (status != HEAP_OK)
    return defect(numeric_message("Process.send: the message points into owner %" PRIu64
                                  ", outside the sender's heap (owner %" PRIu64 ")",
                                  carrier->fragment_copy_foreign_owner, carrier->fiber.heap->owner),
                  call_node);
  CapabilityTransfer carried = {.collect = true};
  transfer_captures(&carried, fragment->roots[0]);
  size_t carried_count = carried.carried_count;
  free(carried.carried);
  if (carried_count) {
    fragment_free(fragment);
    return defect("Process.send: a test mailbox cannot hold a message with a Receiver or Completion",
                  call_node);
  }
  test_log_append(log, fragment);
  return success(void_value());
}

static ChimiControl *send_to_test_mailbox(ChimiValue *address, ChimiValue *message, uint32_t call_node) {
  pthread_mutex_lock(&test_log_lock);
  ChimiControl *control = send_to_test_mailbox_locked(address, message, call_node);
  pthread_mutex_unlock(&test_log_lock);
  return control;
}

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


/* The entry of every incarnation (CHIMI_PROCESS_END_* for a body that ends). The name
   is external so a built executable shows that it links the process kernel (MF06). */
ChimiFiberResult chimi_rt_process_entry(void *raw_context) {
  ProcessSpecContext *context = raw_context;
  require_bound_carrier("a process");
  carrier->fiber.target = NULL;
  ChimiRuntime *runtime = context->runtime;
  RuntimeHeap *heap = heap_create();
  carrier->fiber.heap = heap;
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
  uint64_t span = take_spans(runtime, 1);
  bool traced = !runtime->silent && !control;
  uint64_t incarnation = traced ? chimi_fiber_current_incarnation() : 0;
  if (traced) emit_process_body(runtime, span, incarnation, NULL);
  if (!control)
    control = invoke(runtime, body->as.callable, arguments, argument_count, span, UINT32_MAX,
                     new_environment(0));
  if (traced) emit_process_body(runtime, span, incarnation, control);
  ChimiFiberResult result = {.state = CHIMI_FIBER_SUCCEEDED, .value = CHIMI_PROCESS_END_RETURNED};
  switch (control->kind) {
    case CONTROL_SUCCESS: break;
    case CONTROL_FAILURE: result.value = CHIMI_PROCESS_END_FAILED; break;
    case CONTROL_CANCEL: result = (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED}; break;
    case CONTROL_DEFECT:
    case CONTROL_BREAK:
      chimi_fiber_report_defect(control->message);
      result = (ChimiFiberResult){.state = CHIMI_FIBER_DEFECTED, .defect_code = 1};
      break;
  }
  if (heap->peak_live_bytes > carrier->process_heap_peak_value_bytes)
    carrier->process_heap_peak_value_bytes = heap->peak_live_bytes;
  heap_destroy(heap);
  carrier->fiber.heap = NULL;
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
   freed after the request. The contexts belong to the scheduler once the request is
   issued, served or not, so the host never frees one it has sent. */
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

/* Frees the arrays, and the contexts too unless the request was issued. */
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
      if (arguments[1]->as.address.kind == CHIMI_ADDRESS_TEST)
        return defect("Process.monitor: a test address has no process to watch", call_node);
      /* The notice outlives this call in the scheduler, like an accepted message,
         until the receive that runs it adopts it back into this heap. */
      void *roots[] = {arguments[2]};
      AllocationKind kinds[] = {ALLOCATION_VALUE};
      RuntimeFragment *notice = NULL;
      if (fragment_copy(roots, kinds, 1, &notice) != HEAP_OK)
        return defect(numeric_message("Process.monitor: the notice points into owner %" PRIu64
                                      ", outside the watcher's heap (owner %" PRIu64 ")",
                                      carrier->fragment_copy_foreign_owner, carrier->fiber.heap->owner),
                      call_node);
      /* Receivers and Completions the notice captures travel with it, as in a
         Process.send message, until the receive that runs it. */
      CapabilityTransfer carried = {.collect = true};
      transfer_captures(&carried, notice->roots[0]);
      ChimiMessageCapabilities capabilities = {carried.carried, carried.carried_count};
      ChimiProcessRequest request = {
        .kind = CHIMI_REQUEST_PROCESS_MONITOR,
        .address_kind = arguments[1]->as.address.kind,
        .address = arguments[1]->as.address.target,
        .message = (uint64_t)(uintptr_t)notice,
        .carried = capabilities.count ? &capabilities : NULL,
      };
      ChimiFiberCommand command = scheduler_request(&request);
      free(carried.carried);
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
      if (arguments[0]->as.address.kind == CHIMI_ADDRESS_TEST)
        return send_to_test_mailbox(arguments[0], arguments[1], call_node);
      void *roots[] = {arguments[1]};
      AllocationKind kinds[] = {ALLOCATION_VALUE};
      RuntimeFragment *message = NULL;
      carrier->copy_counting = true;
      HeapStatus status = fragment_copy(roots, kinds, 1, &message);
      carrier->copy_counting = false;
      if (status != HEAP_OK)
        return defect(numeric_message("Process.send: the message points into owner %" PRIu64
                                      ", outside the sender's heap (owner %" PRIu64 ")",
                                      carrier->fragment_copy_foreign_owner, carrier->fiber.heap->owner),
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
      marshal_finish(&marshal, true);
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
      marshal_finish(&marshal, true);
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
      marshal_finish(&marshal, true);
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
      return defect(numeric_message("%s: the handle names no current member of supervisor %" PRIu64
                                    "; it was removed already, through this handle or a copy of it, "
                                    "or the supervisor has stopped",
                                    function, arguments[0]->as.managed.supervisor),
                    call_node);
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

/* The native cores of std/map and std/set live in
   runtime/chimi-runtime/src/intrinsics/collections.rs. */
ChimiControl *chimi_intrinsic_map_v1(ChimiRuntime *runtime, uint32_t intrinsic, ChimiValue **arguments,
                                     uint32_t argument_count, uint64_t span, uint32_t call_node,
                                     ChimiEnvironment *environment);
ChimiControl *chimi_intrinsic_set_v1(ChimiRuntime *runtime, uint32_t intrinsic, ChimiValue **arguments,
                                     uint32_t argument_count, uint64_t span, uint32_t call_node,
                                     ChimiEnvironment *environment);
/* The native cores of std/json and std/decimal, in intrinsics/json.rs and
   intrinsics/decimals.rs. */
ChimiControl *chimi_intrinsic_json_v1(uint32_t intrinsic, ChimiValue **arguments, uint32_t argument_count,
                                      uint32_t call_node);
ChimiControl *chimi_intrinsic_decimal_v1(uint32_t intrinsic, ChimiValue **arguments, uint32_t argument_count,
                                         uint32_t call_node);
/* The native core of std/bytes and std/http's byte scan, in intrinsics/bytes.rs. */
ChimiControl *chimi_intrinsic_bytes_v1(uint32_t intrinsic, ChimiValue **arguments, uint32_t argument_count,
                                       uint32_t call_node);
/* The native core of std/duration, in intrinsics/durations.rs. */
ChimiControl *chimi_intrinsic_duration_v1(uint32_t intrinsic, ChimiValue **arguments, uint32_t argument_count,
                                          uint32_t call_node);
/* Debug.crash, Option.unwrap_or_fail, and the native cores of std/text and
   std/list, in intrinsics/core.rs. */
ChimiControl *chimi_intrinsic_core_v1(uint32_t intrinsic, ChimiValue **arguments, uint32_t argument_count,
                                      uint32_t call_node);
/* `invoke` and the shared Void, False and True, for the intrinsics in Rust. */
ChimiControl *chimi_runtime_invoke_v1(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                                      uint32_t argument_count, uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  return invoke(runtime, callable, arguments, argument_count, span, call_node, environment);
}
ChimiValue *chimi_runtime_shared_value_v1(uint32_t index) { return shared_value(index); }

/* std/clock and timeouts (time-and-outcomes.md, "Time sources" and "Timeouts and
   deadlines").

   A `clock` provider bound by `using` sits in CHIMI_CLOCK_REQUIREMENT_SLOT, and every
   std/clock function reads time through it. With none bound, the runner's execution
   clock provider answers: the scheduler's real or virtual source. Its methods are
   HOST_CLOCK_* callables, and referencing the `clock` need builds it as a record, so
   it can be passed on, for example to `Clock.starting_at`.

   `Task.timeout` and `Task.deadline` read and wait on the execution time source, not
   on a bound provider: a scope's deadline is a scheduler timer. */
static ChimiValue *instant_value(int64_t nanoseconds) { return chimi_value_integer_v1(VALUE_INSTANT, nanoseconds); }
static ChimiValue *timestamp_value(int64_t nanoseconds) { return chimi_value_integer_v1(VALUE_TIMESTAMP, nanoseconds); }
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
   masked before acquire, so a cancellation of the task always finds it and none
   lands between acquire returning and the entry owning its value; unmasked for
   use, which delivers a cancellation acquire held off; masked again before
   release, so no cancellation interrupts release; left after it, which delivers
   a cancellation release held off. A forced end (a terminal cancel) issues no
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
  ChimiControl *used = NULL;
  if (scheduled) {
    /* Acquire ran masked; unmask for use. A cancellation acquire held off is
       delivered here, and release then runs without use. */
    ChimiSchedulerRequest unmask = {.token = entry.token};
    bool cancel = resource_request(CHIMI_REQUEST_RESOURCE_USE, &unmask);
    if (cancel && chimi_fiber_cancel_is_terminal()) return cancelled();
    if (unmask.status == CHIMI_RESPONSE_UNSET) return cancelled();
    if (unmask.status != CHIMI_RESPONSE_OK) return defect("Resource.scoped scheduler request failed", call_node);
    if (cancel || unmask.value == 1) used = cancelled();
  }
  if (!used) used = invoke(runtime, use->as.callable, &handle, 1, span, call_node, environment);
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
      char text[CHIMI_TIMESTAMP_TEXT]; timestamp_text(arguments[0]->as.integer, text);
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
         ends. After a terminal cancel (a forced body, Task.cancel, a parent's
         end) the scheduler ends the children itself, so no request is issued: a
         forced body that parked now would have its fiber and heap abandoned
         (CHI-265). */
      if (!(interrupted->kind == CONTROL_CANCEL && chimi_fiber_cancel_is_terminal()))
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

/* std/wait (time-and-outcomes.md, "Choosing among events"). An Event is a record
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
   execution clock's (read once per Wait.first) or the event's own provider's.
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
    return defect(numeric_message("`Wait.after(%s)` overflows the Instant range", text), call_node);
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
   at once. A receive source's value was already taken by the select and kept
   for this task; an inbox or a task handle has this task as its only consumer.
   Nothing may be issued between the select and this. A timer's value is its
   event value. */
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

/* Wait.first (time-and-outcomes.md, "Semantics of `Wait.first`"): one
   CHIMI_REQUEST_SELECT over every event in list order, reissued after each wake,
   then the committed event. A user-Clock timer is a child running that clock's
   `sleep_until`, so it is a task source to the scheduler. */
static ChimiControl *select_first(ChimiRuntime *runtime, ChimiValue *events, uint64_t span,
                                  uint32_t call_node, ChimiEnvironment *environment) {
  uint32_t count = list_length(events);
  if (count == 0) return defect("`Wait.first` needs at least one event", call_node);
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
      stopped = defect("Wait.first scheduler request failed", call_node);
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

/* std/fs's host core. Results are tuples whose first field is a status: a
   non-negative result, or a portable failure code that std/fs maps to FsError,
   the same codes execution/host_operations.rs gives from Rust's ErrorKind. The
   message is strerror's text, which Rust's io::Error also prints. */
enum {
  FS_NOT_FOUND = -1, FS_PERMISSION_DENIED = -2, FS_ALREADY_EXISTS = -3, FS_NOT_A_DIRECTORY = -4,
  FS_IS_A_DIRECTORY = -5, FS_NOT_EMPTY = -6, FS_CLOSED = -7, FS_OTHER = -8,
};
enum { FS_READ_CHUNK_BYTES = 64 * 1024 };

static int64_t fs_failure_code(int error) {
  switch (error) {
    case ENOENT: return FS_NOT_FOUND;
    case EACCES: case EPERM: return FS_PERMISSION_DENIED;
    case EEXIST: return FS_ALREADY_EXISTS;
    case ENOTDIR: return FS_NOT_A_DIRECTORY;
    case EISDIR: return FS_IS_A_DIRECTORY;
    case ENOTEMPTY: return FS_NOT_EMPTY;
    default: return FS_OTHER;
  }
}

static ChimiValue *fs_tuple(ChimiValue *const *values, uint32_t length) {
  ChimiValue *tuple = new_value(VALUE_TUPLE);
  tuple->as.record.length = length;
  tuple->as.record.fields = allocate(length, sizeof(*tuple->as.record.fields));
  allocation_kind(tuple->as.record.fields, ALLOCATION_FIELDS);
  for (uint32_t index = 0; index < length; ++index)
    tuple->as.record.fields[index] = (RuntimeField){NULL, values[index]};
  return tuple;
}

/* (status, message) */
static ChimiValue *fs_status(int64_t status, const char *message) {
  ChimiValue *values[] = {integer_value(status), text_value(message)};
  return fs_tuple(values, 2);
}
static ChimiValue *fs_errno_status(int error) { return fs_status(fs_failure_code(error), strerror(error)); }
static ChimiValue *fs_result(int result) { return result == 0 ? fs_status(0, "") : fs_errno_status(errno); }

/* The path's text as a C string, or NULL when it holds U+0000: a C path stops
   there, so such a path names no file. The caller frees it. */
static const char FS_NUL_MESSAGE[] = "path contains a NUL byte";
static char *fs_path(const ChimiValue *path) {
  if (memchr(path->as.text.bytes, '\0', path->as.text.length)) return NULL;
  char *copy = malloc(path->as.text.length + 1);
  if (!copy) exhausted_memory();
  memcpy(copy, path->as.text.bytes, path->as.text.length);
  copy[path->as.text.length] = '\0';
  return copy;
}

/* The open files live in the Rust runtime's table (runtime/chimi-runtime/src/
   files.rs), which the interpreter shares: a handle is `slot | generation << 32`,
   so one used after its file was closed fails with FS_CLOSED instead of reaching
   another file. Any task, on any carrier, may hold a handle, so every call into
   the table holds fs_file_lock; the Rust crate is no_std and has no lock of its
   own, as for the random stream. A read or write borrows the descriptor for the
   length of its system call (fs_take, fs_give); a close during one leaves the
   descriptor open until it is given back, so the number is not reused under the
   call. The table hands back each descriptor to close exactly once, and it is
   closed here after the lock is released. */
extern int64_t chimi_files_insert_v1(int32_t fd);
extern int32_t chimi_files_take_v1(int64_t handle);
extern int32_t chimi_files_give_v1(int64_t handle);
extern int32_t chimi_files_close_v1(int64_t handle);
enum { FILES_NOT_OPEN = -2 };
static pthread_mutex_t fs_file_lock = PTHREAD_MUTEX_INITIALIZER;

static int64_t fs_insert(int fd) {
  pthread_mutex_lock(&fs_file_lock);
  int64_t handle = chimi_files_insert_v1(fd);
  pthread_mutex_unlock(&fs_file_lock);
  return handle;
}

/* The descriptor of open `handle`, to be given back with fs_give, or -1 when
   the handle is closed. */
static int fs_take(int64_t handle) {
  pthread_mutex_lock(&fs_file_lock);
  int fd = chimi_files_take_v1(handle);
  pthread_mutex_unlock(&fs_file_lock);
  return fd;
}

/* Gives back what fs_take(handle) returned; the last user of a closed file
   closes its descriptor. */
static void fs_give(int64_t handle) {
  pthread_mutex_lock(&fs_file_lock);
  int fd = chimi_files_give_v1(handle);
  pthread_mutex_unlock(&fs_file_lock);
  if (fd >= 0) (void)close(fd);
}

/* Closes `handle`; false when it was already closed. As the interpreter,
   which drops the file, a close error is not reported. */
static bool fs_close(int64_t handle) {
  pthread_mutex_lock(&fs_file_lock);
  int fd = chimi_files_close_v1(handle);
  pthread_mutex_unlock(&fs_file_lock);
  if (fd >= 0) (void)close(fd);
  return fd != FILES_NOT_OPEN;
}

/* A descriptor that is not a regular file (a pipe, a terminal, a socket) is made
   non-blocking, so a read or write that would block parks the task in
   CHIMI_REQUEST_IO_WAIT instead of stopping every task. */
static int fs_register(int fd) {
  struct stat status;
  if (fstat(fd, &status) != 0) return -1;
  if (!S_ISREG(status.st_mode)) {
    int flags = fcntl(fd, F_GETFL);
    if (flags != -1) (void)fcntl(fd, F_SETFL, flags | O_NONBLOCK);
  }
  return 0;
}

/* Waits for `interest` on `fd`; false when cancelled. */
static bool fs_wait(ChimiRuntime *runtime, int fd, int64_t interest) {
  if (CHIMI_REQUIRES_SCHEDULER) {
    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_IO_WAIT, .value = fd, .token = interest};
    return scheduler_request(&request) != CHIMI_FIBER_CANCEL;
  }
  struct pollfd poll_fd = {.fd = fd, .events = interest == CHIMI_IO_READABLE ? POLLIN : POLLOUT};
  (void)poll(&poll_fd, 1, -1);
  return !runtime->cancel;
}

static int fs_compare_names(const void *left, const void *right) {
  return strcmp(*(char *const *)left, *(char *const *)right);
}

static ChimiControl *fs_list(const char *path) {
  DIR *directory = opendir(path);
  if (!directory) {
    ChimiValue *values[] = {integer_value(fs_failure_code(errno)), list_from_values(NULL, 0),
                            text_value(strerror(errno))};
    return success(fs_tuple(values, 3));
  }
  char **names = NULL;
  size_t count = 0, capacity = 0;
  const char *failure = NULL;
  int error = 0;
  for (;;) {
    errno = 0;
    struct dirent *entry = readdir(directory);
    if (!entry) {
      error = errno;
      break;
    }
    if (strcmp(entry->d_name, ".") == 0 || strcmp(entry->d_name, "..") == 0) continue;
    if (!valid_utf8((const unsigned char *)entry->d_name, strlen(entry->d_name))) {
      failure = "a name is not UTF-8";
      break;
    }
    if (count == capacity) {
      capacity = capacity ? capacity * 2 : 16;
      char **grown = realloc(names, capacity * sizeof(*names));
      if (!grown) exhausted_memory();
      names = grown;
    }
    names[count] = strdup(entry->d_name);
    if (!names[count]) exhausted_memory();
    ++count;
  }
  closedir(directory);
  ChimiValue *list = list_from_values(NULL, 0);
  if (!failure && !error) {
    /* An empty directory leaves `names` NULL, which qsort must not get. */
    if (count) qsort(names, count, sizeof(*names), fs_compare_names);
    for (size_t index = 0; index < count; ++index) list = list_push(list, text_value(names[index]));
  }
  for (size_t index = 0; index < count; ++index) free(names[index]);
  free(names);
  int64_t status = failure ? FS_OTHER : error ? fs_failure_code(error) : 0;
  ChimiValue *values[] = {integer_value(status), failure || error ? list_from_values(NULL, 0) : list,
                          text_value(failure ? failure : error ? strerror(error) : "")};
  return success(fs_tuple(values, 3));
}

#if defined(__APPLE__)
#define FS_MODIFIED(status) ((status).st_mtimespec)
#else
#define FS_MODIFIED(status) ((status).st_mtim)
#endif

static ChimiControl *fs_metadata(const char *path) {
  struct stat status;
  if (stat(path, &status) != 0) {
    int error = errno;
    ChimiValue *values[] = {integer_value(fs_failure_code(error)), integer_value(0), integer_value(0),
                            integer_value(0), text_value(strerror(error))};
    return success(fs_tuple(values, 5));
  }
  int64_t kind = S_ISREG(status.st_mode) ? 0 : S_ISDIR(status.st_mode) ? 1 : 2;
  int64_t modified = (int64_t)FS_MODIFIED(status).tv_sec * 1000000000 + (int64_t)FS_MODIFIED(status).tv_nsec;
  ChimiValue *values[] = {integer_value(0), integer_value(kind), integer_value((int64_t)status.st_size),
                          integer_value(modified), text_value("")};
  return success(fs_tuple(values, 5));
}

/* The carrier's read buffer, allocated on first use: it is too large for a fiber
   stack, and most programs never read a file. */
static char *fs_read_chunk(void) {
  if (!carrier->fs_read_chunk && !(carrier->fs_read_chunk = malloc(FS_READ_CHUNK_BYTES)))
    exhausted_memory();
  return carrier->fs_read_chunk;
}

static ChimiControl *fs_read(ChimiRuntime *runtime, int64_t handle) {
  for (;;) {
    /* Take the descriptor and the buffer again on every pass: the wait below
       parks, another task may close the handle meanwhile, and a later step
       may resume this fiber on another carrier. */
    int fd = fs_take(handle);
    if (fd < 0) {
      ChimiValue *values[] = {integer_value(FS_CLOSED), bytes_from("", 0), text_value("file is closed")};
      return success(fs_tuple(values, 3));
    }
    char *chunk = fs_read_chunk();
    ssize_t count = read(fd, chunk, FS_READ_CHUNK_BYTES);
    int error = errno;
    fs_give(handle);
    if (count >= 0) {
      ChimiValue *values[] = {integer_value(count > 0 ? 1 : 0), bytes_from(chunk, (size_t)count), text_value("")};
      return success(fs_tuple(values, 3));
    }
    if (error == EINTR) continue;
    if (error == EAGAIN || error == EWOULDBLOCK) {
      if (!fs_wait(runtime, fd, CHIMI_IO_READABLE)) return cancelled();
      continue;
    }
    ChimiValue *values[] = {integer_value(fs_failure_code(error)), bytes_from("", 0), text_value(strerror(error))};
    return success(fs_tuple(values, 3));
  }
}

static ChimiControl *fs_write(ChimiRuntime *runtime, int64_t handle, const ChimiValue *bytes) {
  size_t written = 0;
  do {
    int fd = fs_take(handle);
    if (fd < 0) return success(fs_status(FS_CLOSED, "file is closed"));
    if (written == bytes->as.text.length) {
      fs_give(handle);
      break;
    }
    ssize_t count = write(fd, bytes->as.text.bytes + written, bytes->as.text.length - written);
    int error = errno;
    fs_give(handle);
    if (count > 0) {
      written += (size_t)count;
      continue;
    }
    if (count < 0 && error == EINTR) continue;
    if (count < 0 && (error == EAGAIN || error == EWOULDBLOCK)) {
      if (!fs_wait(runtime, fd, CHIMI_IO_WRITABLE)) return cancelled();
      continue;
    }
    return success(fs_errno_status(count == 0 ? EIO : error));
  } while (written < bytes->as.text.length);
  return success(fs_status(0, ""));
}

/* A new directory under $TMPDIR (else /tmp), as Rust's temp_dir finds it, with
   its symbolic links resolved. */
static ChimiControl *fs_temporary_directory(uint32_t call_node) {
  const char *base = getenv("TMPDIR");
  if (!base || !*base) base = "/tmp";
  size_t length = strlen(base);
  while (length > 1 && base[length - 1] == '/') --length;
  char *template = malloc(length + sizeof("/chimi-test-XXXXXX"));
  if (!template) exhausted_memory();
  memcpy(template, base, length);
  memcpy(template + length, "/chimi-test-XXXXXX", sizeof("/chimi-test-XXXXXX"));
  char *resolved = mkdtemp(template) ? realpath(template, NULL) : NULL;
  free(template);
  if (!resolved) return defect("cannot create a temporary directory", call_node);
  ChimiValue *path = valid_utf8((const unsigned char *)resolved, strlen(resolved)) ? text_value(resolved) : NULL;
  free(resolved);
  return path ? success(path) : defect("cannot create a temporary directory: it is not UTF-8", call_node);
}

static bool fs_path_argument(ChimiValue *const *arguments, uint32_t argument_count, uint32_t at) {
  return at < argument_count && arguments[at]->kind == VALUE_TEXT;
}

static ChimiControl *invoke_signal_operation(uint32_t intrinsic, ChimiValue **arguments,
                                             uint32_t argument_count, uint32_t call_node);

static ChimiControl *invoke_fs_operation(ChimiRuntime *runtime, uint32_t intrinsic, ChimiValue **arguments,
                                         uint32_t argument_count, uint32_t call_node) {
  static const uint32_t arity[] = {1, 2, 1, 2, 1, 1, 1, 1, 1, 2, 1, 0, 1};
  if (argument_count != arity[intrinsic - CHIMI_INTRINSIC_FS_HOST_OPEN]) return intrinsic_shape_defect(call_node);
  if (runtime->cancel) return cancelled();
  switch (intrinsic) {
    case CHIMI_INTRINSIC_FS_HOST_READ:
      if (arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      return fs_read(runtime, arguments[0]->as.integer);
    case CHIMI_INTRINSIC_FS_HOST_WRITE:
      if (arguments[0]->kind != VALUE_INTEGER || arguments[1]->kind != VALUE_BYTES)
        return intrinsic_shape_defect(call_node);
      return fs_write(runtime, arguments[0]->as.integer, arguments[1]);
    case CHIMI_INTRINSIC_FS_HOST_CLOSE: {
      if (arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      if (!fs_close(arguments[0]->as.integer)) return success(fs_status(FS_CLOSED, "file is closed"));
      return success(fs_status(0, ""));
    }
    case CHIMI_INTRINSIC_FS_HOST_TEMPORARY_DIRECTORY: return fs_temporary_directory(call_node);
    case CHIMI_INTRINSIC_FS_TEST_ROOT:
      if (!fs_path_argument(arguments, argument_count, 0)) return intrinsic_shape_defect(call_node);
      if (arguments[0]->as.text.length == 0 || arguments[0]->as.text.bytes[0] != '/')
        return defect("Fs.files_within needs an absolute root", call_node);
      return success(arguments[0]);
    default: break;
  }
  if (!fs_path_argument(arguments, argument_count, 0)) return intrinsic_shape_defect(call_node);
  char *path = fs_path(arguments[0]);
  char *second = NULL;
  if (intrinsic == CHIMI_INTRINSIC_FS_HOST_RENAME) {
    if (!fs_path_argument(arguments, argument_count, 1)) {
      free(path);
      return intrinsic_shape_defect(call_node);
    }
    second = fs_path(arguments[1]);
  }
  if (!path || (intrinsic == CHIMI_INTRINSIC_FS_HOST_RENAME && !second)) {
    free(path);
    free(second);
    switch (intrinsic) {
      case CHIMI_INTRINSIC_FS_HOST_METADATA: {
        ChimiValue *values[] = {integer_value(FS_OTHER), integer_value(0), integer_value(0), integer_value(0),
                                text_value(FS_NUL_MESSAGE)};
        return success(fs_tuple(values, 5));
      }
      case CHIMI_INTRINSIC_FS_HOST_LIST: {
        ChimiValue *values[] = {integer_value(FS_OTHER), list_from_values(NULL, 0), text_value(FS_NUL_MESSAGE)};
        return success(fs_tuple(values, 3));
      }
      default: return success(fs_status(FS_OTHER, FS_NUL_MESSAGE));
    }
  }
  ChimiControl *control;
  switch (intrinsic) {
    case CHIMI_INTRINSIC_FS_HOST_OPEN: {
      int fd = open(path, O_RDONLY | O_CLOEXEC);
      struct stat status = {0};
      if (fd < 0) {
        control = success(fs_errno_status(errno));
      } else if (fstat(fd, &status) != 0 || S_ISDIR(status.st_mode) || fs_register(fd) != 0) {
        int error = errno != 0 && !S_ISDIR(status.st_mode) ? errno : EISDIR;
        (void)close(fd);
        control = success(fs_errno_status(error));
      } else {
        control = success(fs_status(fs_insert(fd), ""));
      }
      break;
    }
    case CHIMI_INTRINSIC_FS_HOST_CREATE: {
      if (arguments[1]->kind != VALUE_INTEGER) {
        control = intrinsic_shape_defect(call_node);
        break;
      }
      int64_t mode = arguments[1]->as.integer;
      int flags = O_WRONLY | O_CREAT | O_CLOEXEC | (mode == 0 ? O_TRUNC : mode == 1 ? O_APPEND : O_EXCL);
      int fd = open(path, flags, 0666);
      if (fd < 0) {
        control = success(fs_errno_status(errno));
      } else if (fs_register(fd) != 0) {
        int error = errno;
        (void)close(fd);
        control = success(fs_errno_status(error));
      } else {
        control = success(fs_status(fs_insert(fd), ""));
      }
      break;
    }
    case CHIMI_INTRINSIC_FS_HOST_METADATA: control = fs_metadata(path); break;
    case CHIMI_INTRINSIC_FS_HOST_LIST: control = fs_list(path); break;
    case CHIMI_INTRINSIC_FS_HOST_CREATE_DIRECTORY: control = success(fs_result(mkdir(path, 0777))); break;
    case CHIMI_INTRINSIC_FS_HOST_REMOVE: {
      struct stat status;
      if (lstat(path, &status) != 0) control = success(fs_errno_status(errno));
      else control = success(fs_result(S_ISDIR(status.st_mode) ? rmdir(path) : unlink(path)));
      break;
    }
    case CHIMI_INTRINSIC_FS_HOST_RENAME: control = success(fs_result(rename(path, second))); break;
    case CHIMI_INTRINSIC_FS_HOST_IS_DIRECTORY: {
      struct stat status;
      if (lstat(path, &status) != 0) control = success(fs_errno_status(errno));
      else control = success(fs_status(S_ISDIR(status.st_mode) ? 1 : 0, ""));
      break;
    }
    default: control = intrinsic_shape_defect(call_node); break;
  }
  free(path);
  free(second);
  return control;
}

static ChimiControl *invoke_intrinsic(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  if ((intrinsic >= CHIMI_INTRINSIC_MAP_REMOVE && intrinsic <= CHIMI_INTRINSIC_MAP_REDUCE_ENTRIES) ||
      (intrinsic >= CHIMI_INTRINSIC_MAP_EMPTY && intrinsic <= CHIMI_INTRINSIC_MAP_PUT))
    return chimi_intrinsic_map_v1(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_SET_EMPTY && intrinsic <= CHIMI_INTRINSIC_SET_REDUCE_ITEMS)
    return chimi_intrinsic_set_v1(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_JSON_DECODE && intrinsic <= CHIMI_INTRINSIC_JSON_NUMBER_TO_DECIMAL)
    return chimi_intrinsic_json_v1(intrinsic, arguments, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_DECIMAL_SCALED && intrinsic <= CHIMI_INTRINSIC_DECIMAL_TO_FLOAT)
    return chimi_intrinsic_decimal_v1(intrinsic, arguments, argument_count, call_node);
  if ((intrinsic >= CHIMI_INTRINSIC_BYTES_FROM && intrinsic <= CHIMI_INTRINSIC_EMBED_BYTES) ||
      intrinsic == CHIMI_INTRINSIC_HTTP_COUNT_ENDS)
    return chimi_intrinsic_bytes_v1(intrinsic, arguments, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_CAUSE_CONTEXT && intrinsic <= CHIMI_INTRINSIC_CAUSE_TO_TEXT)
    return invoke_cause_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_TEST_ASSERT_EQUAL && intrinsic <= CHIMI_INTRINSIC_TEST_FAIL)
    return invoke_assertion(intrinsic, arguments, argument_count, call_node);
  if (intrinsic == CHIMI_INTRINSIC_RESOURCE_SCOPED)
    return resource_scoped(runtime, arguments, argument_count, span, call_node, environment);
  if (intrinsic == CHIMI_INTRINSIC_TEST_CAPTURE_DEFECT)
    return capture_defect(runtime, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_TEST_RECORDER && intrinsic <= CHIMI_INTRINSIC_TEST_RECEIVED)
    return invoke_test_log(intrinsic, arguments, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_ENV_HOST_VARIABLES && intrinsic <= CHIMI_INTRINSIC_ENV_HOST_CURRENT_DIRECTORY)
    return invoke_host_operation(runtime, intrinsic, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_FS_HOST_OPEN && intrinsic <= CHIMI_INTRINSIC_FS_TEST_ROOT)
    return invoke_fs_operation(runtime, intrinsic, arguments, argument_count, call_node);
  if (intrinsic == CHIMI_INTRINSIC_SIGNAL_HOST_INSTALL || intrinsic == CHIMI_INTRINSIC_SIGNAL_HOST_WAIT)
    return invoke_signal_operation(intrinsic, arguments, argument_count, call_node);
  if (intrinsic == CHIMI_INTRINSIC_LOG_HOST_MINIMUM_LEVEL || intrinsic == CHIMI_INTRINSIC_LOG_HOST_WRITE)
    return invoke_log_operation(runtime, intrinsic, arguments, argument_count, call_node);
  if (intrinsic == CHIMI_INTRINSIC_METRICS_HOST_ENABLED || intrinsic == CHIMI_INTRINSIC_METRICS_HOST_RECORD ||
      intrinsic == CHIMI_INTRINSIC_METRICS_HOST_DOCUMENT)
    return invoke_metrics_operation(runtime, intrinsic, arguments, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_DURATION_NANOSECONDS && intrinsic <= CHIMI_INTRINSIC_DURATION_PARSE)
    return chimi_intrinsic_duration_v1(intrinsic, arguments, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_PROCESS_SPEC && intrinsic <= CHIMI_INTRINSIC_SUPERVISOR_WAIT)
    return invoke_process(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if ((intrinsic >= CHIMI_INTRINSIC_SELECT_RECEIVE && intrinsic <= CHIMI_INTRINSIC_SELECT_FIRST) ||
      intrinsic == CHIMI_INTRINSIC_SELECT_WATCH)
    return invoke_select(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_CLOCK_NOW && intrinsic <= CHIMI_INTRINSIC_TASK_DEADLINE)
    return invoke_clock(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_DEBUG_CRASH && intrinsic <= CHIMI_INTRINSIC_LIST_SET)
    return chimi_intrinsic_core_v1(intrinsic, arguments, argument_count, call_node);
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
      if (carried_count || carried.managed_count) {
        fragment_free(initial);
        return defect(carried_count
                        ? "channel capability violation: Watch.starting_with cannot hold a value "
                          "with a Receiver or Completion, because every subscriber gets a copy"
                        : MANAGED_COPY_DEFECT("Watch.starting_with"),
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
      if (carried.managed_count && intrinsic != CHIMI_INTRINSIC_CHANNEL_SEND) {
        free(carried.carried);
        fragment_free(message);
        return defect(intrinsic == CHIMI_INTRINSIC_BROADCAST_PUBLISH
                        ? MANAGED_COPY_DEFECT("Broadcast.publish")
                        : MANAGED_COPY_DEFECT("Watch.publish"),
                      call_node);
      }
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
      return decimal_value((Decimal){high, (uint64_t)raw->integer, raw->a & 0xff, (raw->a & 0x100) != 0});
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
      callable->symbol = raw->a; callable->name = CHIMI_STRINGS[shape->name];
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
  for (uint32_t index = 0; index < 3; ++index) (void)shared_value(index);
  AllocationArea *previous = carrier->fiber.target;
  carrier->fiber.target = &static_area;
  constant_values = allocate(CHIMI_CONSTANT_VALUE_COUNT, sizeof(*constant_values));
  for (uint32_t i = 0; i < CHIMI_CONSTANT_VALUE_COUNT; ++i)
    constant_values[i] = decode_constant(&CHIMI_CONSTANT_VALUES[i]);
  carrier->fiber.target = previous;
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
      callable->intrinsic = shape->intrinsic; callable->symbol = symbol;
      callable->name = CHIMI_STRINGS[shape->name];
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
    else if (standard_type == CHIMI_TYPE_RANDOM && node->c == 0)
      inner->host = HOST_RANDOM_NEXT_INT;
    else if (standard_type == CHIMI_TYPE_CLOCK && node->c <= 2)
      inner->host = node->c == 0 ? HOST_CLOCK_NOW : node->c == 1 ? HOST_CLOCK_TIMESTAMP : HOST_CLOCK_SLEEP_UNTIL;
    else { result = defect("closed core has no provider", node_id); break; }
  } else {
    ChimiValue *method = select_value(provider, CHIMI_STRINGS[node->d]);
    if (!method || method->kind != VALUE_CALLABLE) {
      result = defect("provider has no callable field", node_id); break;
    }
    inner = method->as.callable;
    /* A provider built from another need's methods (`let c = console`, then
       `using { console: c }`) already holds dependency callables; one call is
       one dependency call, of the need this node reads. */
    if (inner->kind == CALL_DEPENDENCY) inner = inner->inner;
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
  if (control->value->kind == VALUE_DECIMAL) { result = success(decimal_negated(control->value)); break; }
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
      result = decimal_arithmetic(node->a, left->value, right->value, node_id);
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
    /* Each provider runs in the enclosing scope, as typing gives its own needs
       to the enclosing expression: no provider sees another of this clause. */
    const ChimiNamedNode *provider = &CHIMI_NAMED_NODES[node->offset + i];
    ChimiControl *control = run_node(provider->node, runtime, incoming, span);
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
  /* `chimi test` names the seed in the report of a failing test that drew one. */
  if (random_read) fputs(",\"random_read\":true", output);
  ChimiValue *cause = control->kind == CONTROL_SUCCESS ? NULL : control_cause(control);
  if (cause && cause_has_detail(cause)) {
    fputs(",\"cause\":", output);
    ChimiValue *text = cause_text(cause);
    json_bytes(output, text->as.text.bytes, text->as.text.length);
  }
  fputs("}\n", output);
}

/* A report file the runtime writes, closed in every child process it starts
   (std/exec), so a child cannot hold it open or write into it. */
static FILE *open_report(const char *path, const char *mode) {
  FILE *output = fopen(path, mode);
  if (output && fcntl(fileno(output), F_SETFD, FD_CLOEXEC) != 0) {
    int error = errno;
    fclose(output);
    errno = error;
    return NULL;
  }
  return output;
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
  FILE *output = open_report(path, "wx");
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

/* With several carrier threads, replaces the summed totals' peaks (each one
   carrier's) with the process-wide peaks heap/allocator.rs folds, raised to the
   exact sum now, which the report reads once no fiber runs. They are then
   within carrier_count * FOOTPRINT_FOLD_SLACK of the true peak. */
static void report_process_peaks(HeapTotals *totals) {
#ifdef CHIMI_CARRIER_SIMULATION
  (void)totals;
#else
  if (carrier_count == 1) return;
  int64_t peak = atomic_load_explicit(&process_peak_footprint_bytes, memory_order_relaxed);
  int64_t fragment_peak = atomic_load_explicit(&process_peak_fragment_footprint_bytes, memory_order_relaxed);
  totals->peak_footprint_bytes = peak > totals->footprint_bytes ? peak : totals->footprint_bytes;
  totals->peak_fragment_footprint_bytes =
    fragment_peak > totals->fragment_footprint_bytes ? fragment_peak : totals->fragment_footprint_bytes;
#endif
}

static bool write_memory_report(const char *path, const ChimiRuntime *runtime) {
  if (!path) return false;
  FILE *output = open_report(path, "wx");
  if (!output) {
    fprintf(stderr, "cannot open memory report file %s: %s\n", path, strerror(errno));
    return true;
  }
  footprint_add(0); /* records the peak */
  HeapTotals totals = heap_totals_sum();
  report_process_peaks(&totals);
  uint64_t unscoped_allocated_bytes = 0, capture_transfer_probes = 0, copied_values = 0, copied_bytes = 0;
  size_t process_heap_peak_value_bytes = 0;
  ScopeTotals scoped = {0};
  for (size_t index = 0; index < carrier_count; ++index) {
    const Carrier *own = carrier_at(index);
    const ScopeTotals *scopes = &own->scope_totals;
    scoped.total_allocated_bytes += scopes->total_allocated_bytes;
    for (uint32_t kind = 0; kind < ALLOCATION_KIND_COUNT; ++kind)
      scoped.allocated_bytes_by_kind[kind] += scopes->allocated_bytes_by_kind[kind];
    if (scopes->peak_live_bytes > scoped.peak_live_bytes) scoped.peak_live_bytes = scopes->peak_live_bytes;
    scoped.retained_bytes += scopes->retained_bytes;
    scoped.collections += scopes->collections;
    scoped.full_collections += scopes->full_collections;
    scoped.scanned_allocations += scopes->scanned_allocations;
    unscoped_allocated_bytes += own->unscoped_allocated_bytes;
    capture_transfer_probes += own->capture_transfer_probes;
    copied_values += own->copied_values;
    copied_bytes += own->copied_bytes;
    if (own->process_heap_peak_value_bytes > process_heap_peak_value_bytes)
      process_heap_peak_value_bytes = own->process_heap_peak_value_bytes;
  }
  fprintf(output,
          "{\"schema\":\"chimi.runtime-memory.v1\",\"scoped_total_allocated_bytes\":%zu,"
          "\"scoped_allocated_bytes_by_kind\":{\"bytes\":%zu,\"values\":%zu,"
          "\"controls\":%zu,\"environments\":%zu,\"value_pointers\":%zu,"
          "\"fields\":%zu,\"callables\":%zu,\"map_nodes\":%zu,\"list_nodes\":%zu},"
          "\"scoped_peak_live_bytes\":%zu,\"scoped_retained_bytes\":%zu,"
          "\"scoped_collections\":%" PRIu64 ",\"scoped_full_collections\":%" PRIu64
          ",\"scoped_scanned_allocations\":%" PRIu64 ",\"unscoped_allocated_bytes\":%" PRIu64 ",\"peak_footprint_bytes\":%zu,"
          "\"peak_fragment_footprint_bytes\":%zu,\"capture_transfer_probes\":%" PRIu64
          ",\"heap_chunks_live\":%zu,\"heap_chunk_bytes_peak\":%zu,\"fragments_live\":%zu"
          ",\"fragment_bytes_peak\":%zu,\"process_heap_peak_value_bytes\":%zu"
          ",\"copied_values\":%" PRIu64 ",\"copied_bytes\":%" PRIu64
          ",\"trace_events_constructed\":%" PRIu64 ",\"memberships_live\":%zu,\"task_records_peak\":%zu}\n",
          scoped.total_allocated_bytes,
          scoped.allocated_bytes_by_kind[ALLOCATION_BYTES],
          scoped.allocated_bytes_by_kind[ALLOCATION_VALUE],
          scoped.allocated_bytes_by_kind[ALLOCATION_CONTROL],
          scoped.allocated_bytes_by_kind[ALLOCATION_ENVIRONMENT] +
            scoped.allocated_bytes_by_kind[ALLOCATION_FRAME_ENVIRONMENT],
          scoped.allocated_bytes_by_kind[ALLOCATION_VALUE_POINTERS],
          scoped.allocated_bytes_by_kind[ALLOCATION_FIELDS],
          scoped.allocated_bytes_by_kind[ALLOCATION_CALLABLE],
          scoped.allocated_bytes_by_kind[ALLOCATION_MAP_NODE],
          scoped.allocated_bytes_by_kind[ALLOCATION_LIST_NODE],
          scoped.peak_live_bytes,
          scoped.retained_bytes,
          scoped.collections,
          scoped.full_collections,
          scoped.scanned_allocations,
          unscoped_allocated_bytes,
          (size_t)totals.peak_footprint_bytes,
          (size_t)totals.peak_fragment_footprint_bytes,
          capture_transfer_probes,
          /* Reported per allocation: young chunks are not counted separately. */
          (size_t)totals.live_allocations,
          (size_t)totals.peak_footprint_bytes,
          (size_t)totals.live_fragments,
          (size_t)totals.peak_fragment_footprint_bytes,
#if CHIMI_PROCESS_KERNEL
          process_heap_peak_value_bytes,
#else
          (size_t)0,
#endif
          copied_values,
          copied_bytes,
          runtime->trace_events_constructed,
#if CHIMI_PROCESS_KERNEL
          chimi_scheduler_live_memberships(),
#else
          (size_t)0,
#endif
          chimi_scheduler_peak_task_records());
  return close_output(output);
}

static const char *scheduler_event_name(const ChimiSchedulerTraceEvent *event) {
  static const char *const task_names[] = {
    "task_started", "task_parked", "task_woken", "task_cancellation_requested",
    "task_cleanup_completed", "task_terminal",
  };
  static const char *const channel_names[] = {
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
  static const char *const clock_names[] = {
    "clock_advanced", "clock_timer_started", "clock_timer_fired",
    "clock_timer_cancelled", "clock_deadlock_waiter", "clock_scope_fired",
    "clock_wait_parked", "clock_wait_committed", "clock_signal_received",
  };
  if (event->family == CHIMI_SCHEDULER_TRACE_CHANNEL &&
      event->kind > 0 && event->kind <= sizeof(channel_names) / sizeof(*channel_names))
    return channel_names[event->kind - 1];
  if (event->family == CHIMI_SCHEDULER_TRACE_CLOCK &&
      event->kind > 0 && event->kind <= sizeof(clock_names) / sizeof(*clock_names))
    return clock_names[event->kind - 1];
  /* The transcript table's names, in trace-kind order. */
  static const char *const process_names[] = {
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
  static const char *const terminal[] = {"returned", "failed", "defected", "cancelled", "forced"};
  static const char *const admission[] = {"capacity_reached", "name_taken", "stopped", "start_failed"};
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
  static const char *const outcomes[] = {"accepted", "full", "unavailable"};
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
  pthread_mutex_lock(&trace_lock);
  ++runtime->trace_events_constructed;
  fputs("{\"schema\":\"chimi.runtime.v1\",\"event\":", output);
  json_string(output, scheduler_event_name(event));
  fprintf(output, ",\"sequence\":%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(output, runtime->computation);
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
  pthread_mutex_unlock(&trace_lock);
}

/* The operation a parked task waits on, by its scheduler request kind. */
static const char *waited_operation(uint32_t request_kind) {
  switch (request_kind) {
    case CHIMI_REQUEST_TASK_RESULT: return "Task.result";
    case CHIMI_REQUEST_TASK_ANY: return "Task.all, Task.results or Task.race";
    case CHIMI_REQUEST_TASK_DRAIN: return "draining a Task combinator's children";
    case CHIMI_REQUEST_CHANNEL_SEND: return "Channel.send";
    case CHIMI_REQUEST_CHANNEL_RECEIVE: return "Channel.receive";
    case CHIMI_REQUEST_SELECT: return "Wait.first";
    case CHIMI_REQUEST_SIGNAL_WAIT: return "Signal.next";
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

/* Keeps `text` as the defect message of `task`, which the scheduler is ending. */
static void add_violation_note(ChimiRuntime *runtime, uint64_t task, const char *text) {
  CapabilityViolationNote *note = malloc(sizeof(*note));
  char *message = strdup(text);
  if (!note || !message) { fputs("Chimi runtime allocation failed\n", stderr); runtime_exit(70); }
  *note = (CapabilityViolationNote){.task = task, .message = message, .next = runtime->capability_violations};
  runtime->capability_violations = note;
}

/* The message of a task using a Task handle it does not own, or one naming no
   task. The handle travelled where the checker does not follow it, for
   example inside a closure held by an interface value (CHI-311, CHI-316). */
static void task_violation_text(const ChimiCapabilityViolation *violation, char *text, size_t size) {
  char action[96];
  switch (violation->request_kind) {
    case CHIMI_REQUEST_TASK_RESULT:
      snprintf(action, sizeof(action), "read the result of task %" PRIu64, violation->destination);
      break;
    case CHIMI_REQUEST_TASK_CANCEL:
      snprintf(action, sizeof(action), "cancel task %" PRIu64, violation->destination);
      break;
    default:
      snprintf(action, sizeof(action), "wait for task %" PRIu64, violation->destination);
      break;
  }
  char holder[64];
  if (violation->reason == CHIMI_VIOLATION_UNKNOWN_TASK)
    snprintf(holder, sizeof(holder), "there is no such task");
  else if (violation->owner == 0)
    snprintf(holder, sizeof(holder), "no task owns it");
  else
    snprintf(holder, sizeof(holder), "task %" PRIu64 " owns it", violation->owner);
  snprintf(text, size, "task capability violation: task %" PRIu64 " cannot %s: %s",
           violation->task, action, holder);
}

/* Records the defect message of a channel or task capability violation
   (task-and-channel-capabilities.md, "Checked at run time"). The exclusive-move
   rule does not follow a capability across function boundaries, so this is the
   check that turns two owners of one Receiver or Completion into a defect. */
static void record_capability_violation(void *context, const ChimiCapabilityViolation *violation) {
  ChimiRuntime *runtime = context;
  char text[320];
  if (violation->capability == CHIMI_CAPABILITY_TASK) {
    task_violation_text(violation, text, sizeof(text));
    add_violation_note(runtime, violation->task, text);
    return;
  }
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
    case CHIMI_VIOLATION_OWN_RECEIVER:
      snprintf(text, sizeof(text),
               "channel capability violation: task %" PRIu64 " cannot send the Receiver of channel %"
               PRIu64 " through that same channel, because nobody could ever receive the message",
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
  add_violation_note(runtime, violation->task, text);
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
  if (!message) { fputs("Chimi runtime allocation failed\n", stderr); runtime_exit(70); }
  size_t length = (size_t)snprintf(message, capacity, "%s", prefix);
  for (size_t index = 0; index < count; ++index)
    length += (size_t)snprintf(message + length, capacity - length, "%s task %" PRIu64 " waits on %s",
                               index == 0 ? "" : ";", waiters[index].task,
                               waited_operation(waiters[index].request_kind));
  runtime->deadlock_waiters = message;
}

#if CHIMI_PROCESS_KERNEL
/* Keeps the latest defected incarnation and its message: the one its body
   reported as it ended, or a stack overflow the scheduler saw. */
static void record_process_defect(void *context, const ChimiProcessTermination *termination) {
  ChimiRuntime *runtime = context;
  char *message = NULL;
  if (termination->stack_overflow || termination->message) {
    message = strdup(termination->stack_overflow ? "Process stack overflow" : termination->message);
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

/* A `std/cli.Exit` that `main` returned or failed with (CHI-252), applied
   after every release, the drain and the outcome record: `Code(n)` exits n,
   `Help(text)` writes text to standard output and exits 0, `Usage(_,
   message)` writes message to standard error and exits 2. A code outside 0
   to 255 is a defect, 70. False for any other outcome. */
static bool program_exit(const ChimiControl *control, int *status) {
  if (control->kind != CONTROL_SUCCESS && control->kind != CONTROL_FAILURE) return false;
  const ChimiValue *value = control->value;
  if (value == NULL || value->kind != VALUE_CONSTRUCTED) return false;
  const ChimiConstructor *shape = &CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[value->as.constructed.constructor].constructor];
  if (!shape->program_exit) return false;
  const ChimiValue *field = value->as.constructed.fields[value->as.constructed.length - 1].value;
  const char *variant = CHIMI_STRINGS[shape->variant_name];
  if (strcmp(variant, "Code") == 0) {
    if (field->as.integer < 0 || field->as.integer > 255) {
      fprintf(stderr, "Chimi runtime: Exit.Code(%" PRId64 ") is outside 0 to 255\n", field->as.integer);
      *status = 70;
    } else {
      *status = (int)field->as.integer;
    }
    return true;
  }
  bool help = strcmp(variant, "Help") == 0;
  FILE *stream = help ? stdout : stderr;
  bool written = fwrite(field->as.text.bytes, 1, field->as.text.length, stream) == field->as.text.length &&
                 fputc('\n', stream) != EOF && fflush(stream) == 0;
  *status = written ? (help ? 0 : 2) : 74;
  return true;
}

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
      runtime_exit(70);
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

/* std/signal (CHI-249): SIGHUP, SIGUSR1 and SIGUSR2 as events a task waits
   for. Their handler never stops the program: it sets bit `code - 1` of
   `user_signals` and wakes the scheduler through the stop signals' pipe; the
   scheduler latches the bits and hands each to one CHIMI_REQUEST_SIGNAL_WAIT.
   The handlers are installed when the program's `signals` provider is built,
   before main's body, and never removed, so a program that does not need
   `signals` keeps the default actions (SIGHUP ends it). */
static volatile uint32_t user_signals = 0;
static const int user_signal_numbers[] = {SIGHUP, SIGUSR1, SIGUSR2};

static void on_user_signal(int number) {
  for (uint32_t code = 1; code <= 3; ++code)
    if (user_signal_numbers[code - 1] == number) __atomic_fetch_or(&user_signals, 1u << (code - 1), __ATOMIC_SEQ_CST);
  int saved = errno;
  if (signal_pipe[1] >= 0) (void)!write(signal_pipe[1], "", 1);
  errno = saved;
}

/* Once per process, from whichever carrier first asks: a second caller
   returns only after the handlers are in place. */
static pthread_once_t user_signals_installed = PTHREAD_ONCE_INIT;
static void install_user_signals_once(void) {
  struct sigaction action;
  memset(&action, 0, sizeof(action));
  action.sa_handler = on_user_signal;
  sigemptyset(&action.sa_mask);
  for (size_t index = 0; index < 3; ++index) sigaddset(&action.sa_mask, user_signal_numbers[index]);
  action.sa_flags = SA_RESTART;
  for (size_t index = 0; index < 3; ++index) sigaction(user_signal_numbers[index], &action, NULL);
}
static void install_user_signals(void) { pthread_once(&user_signals_installed, install_user_signals_once); }

/* host_install() and host_wait(codes). The wait passes the distinct codes in
   their listed order and returns the one the scheduler hands it. Programs
   that reach either always run on the scheduler. */
static ChimiControl *invoke_signal_operation(uint32_t intrinsic, ChimiValue **arguments,
                                             uint32_t argument_count, uint32_t call_node) {
  if (intrinsic == CHIMI_INTRINSIC_SIGNAL_HOST_INSTALL) {
    if (argument_count != 0) return intrinsic_shape_defect(call_node);
    install_user_signals();
    return success(void_value());
  }
  if (argument_count != 1 || arguments[0]->kind != VALUE_LIST || !CHIMI_REQUIRES_SCHEDULER)
    return intrinsic_shape_defect(call_node);
  uint64_t packed = 0;
  uint32_t count = 0;
  for (uint32_t index = 0; index < list_length(arguments[0]); ++index) {
    ChimiValue *code = list_get(arguments[0], index);
    if (code->kind != VALUE_INTEGER || code->as.integer < 1 || code->as.integer > 3)
      return intrinsic_shape_defect(call_node);
    bool seen = false;
    for (uint32_t earlier = 0; earlier < count; ++earlier)
      seen = seen || ((packed >> (8 * earlier)) & 0xff) == (uint64_t)code->as.integer;
    if (!seen) packed |= (uint64_t)code->as.integer << (8 * count++);
  }
  if (count == 0) return defect("Signal.next needs at least one signal to wait for", call_node);
  for (;;) {
    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_SIGNAL_WAIT, .value = (int64_t)packed};
    if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return cancelled();
    if (request.status == CHIMI_RESPONSE_OK) return success(integer_value(request.value));
    if (request.status != CHIMI_RESPONSE_PENDING) return intrinsic_shape_defect(call_node);
  }
}

/* Child processes for binding packages (std/exec, CHI-250). SIGCHLD's
   handler only writes to the stop signals' pipe, which wakes an idle
   scheduler; CHIMI_REQUEST_CHILD_WAIT then checks every waiting child with
   waitid(WNOWAIT), so the binding reaps the child itself. Installed once, by
   the first chimi_binding_prepare_children, before the first spawn: an
   inherited SIG_IGN would make the kernel reap children on its own. */
static void on_child_signal(int number) {
  (void)number;
  int saved = errno;
  if (signal_pipe[1] >= 0) (void)!write(signal_pipe[1], "", 1);
  errno = saved;
}

/* Once per process, as install_user_signals. */
static pthread_once_t child_signal_installed = PTHREAD_ONCE_INIT;
static void prepare_children_once(void) {
  struct sigaction action;
  memset(&action, 0, sizeof(action));
  action.sa_handler = on_child_signal;
  sigemptyset(&action.sa_mask);
  action.sa_flags = SA_RESTART | SA_NOCLDSTOP;
  sigaction(SIGCHLD, &action, NULL);
}
void chimi_binding_prepare_children(void) { pthread_once(&child_signal_installed, prepare_children_once); }

static int binding_wait_pid(ChimiBindingCall *call, uint32_t handle, int pid);

int chimi_binding_wait_child(ChimiBindingCall *call, uint32_t handle, int pid) {
  if (pid <= 0) return CHIMI_BINDING_WAIT_FAILED;
  if (binding_wait_handle_closed(call, handle)) return CHIMI_BINDING_WAIT_CLOSED;
  chimi_binding_prepare_children();
  binding_run(call, false);
  int answer = binding_wait_pid(call, handle, pid);
  binding_run(call, true);
  return answer;
}

/* chimi_binding_wait_child's wait, run while the call's handles are let go. */
static int binding_wait_pid(ChimiBindingCall *call, uint32_t handle, int pid) {
  if (CHIMI_REQUIRES_SCHEDULER) {
    /* The handle's key, so a close of the handle by another task, which
       withdraws it, wakes this wait: the close only releases the child once
       this call returns. */
    uint64_t resource = 0;
    if (handle != CHIMI_BINDING_NO_HANDLE) {
      const ChimiValue *value = binding_argument(call, handle, VALUE_NATIVE_HANDLE);
      resource = binding_resource_key(value->as.handle.slot, value->as.handle.generation);
    }
    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_CHILD_WAIT, .value = pid, .channel = resource};
    if (scheduler_request(&request) == CHIMI_FIBER_CANCEL) return CHIMI_BINDING_WAIT_CANCELLED;
    if (binding_wait_handle_closed(call, handle)) return CHIMI_BINDING_WAIT_CLOSED;
    if (request.status == CHIMI_RESPONSE_ERROR) return CHIMI_BINDING_WAIT_FAILED;
    /* Exited, or woken to look again: either way the caller looks. A wake
       after another task reaped the child is answered from the caller's
       record, never by waiting on a pid the kernel may have reused. */
    return CHIMI_BINDING_READY;
  }
  for (;;) {
    if (call->runtime->cancel) return CHIMI_BINDING_WAIT_CANCELLED;
    siginfo_t info;
    memset(&info, 0, sizeof(info));
    if (waitid(P_PID, (id_t)pid, &info, WEXITED | WNOWAIT) == 0) return CHIMI_BINDING_READY;
    if (errno != EINTR) return CHIMI_BINDING_WAIT_FAILED;
  }
}

/* `--signal=TERM@2s`: a signal the virtual clock delivers at that instant.
   The time is an integer with a unit of ns, us, ms, s, m, or h. INT and TERM
   stop the program; HUP, USR1 and USR2 are std/signal's, latched for its
   waiters, and may be given more than once. */
static bool parse_injected_signal(const char *text, int32_t *signal_number, int64_t *at) {
  const char *rest;
  if (strncmp(text, "INT@", 4) == 0) { *signal_number = SIGINT; rest = text + 4; }
  else if (strncmp(text, "TERM@", 5) == 0) { *signal_number = SIGTERM; rest = text + 5; }
  else if (strncmp(text, "HUP@", 4) == 0) { *signal_number = SIGHUP; rest = text + 4; }
  else if (strncmp(text, "USR1@", 5) == 0) { *signal_number = SIGUSR1; rest = text + 5; }
  else if (strncmp(text, "USR2@", 5) == 0) { *signal_number = SIGUSR2; rest = text + 5; }
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
    .user_signals = &user_signals,
    .injected_user_signals = runtime->injected_user_signals,
    .injected_user_signal_count = runtime->injected_user_signal_count,
    .carriers = runtime->carriers,
    .carrier_enter = enter_carrier_thread,
  };
  channel_endpoints_reset();
  AllocationScope *entry_scope = main_heap.innermost;
  AllocationArea *entry_target = carrier->fiber.target;
  ChimiSchedulerOutcome outcome = chimi_scheduler_run(&config);
  carrier->fiber.heap = &main_heap;
  /* The root runs on main_heap, and each region it enters records its scope in
     its own fiber frame. A root that overflowed, parked or was ended by the
     scheduler never closed those regions, and its frames will never run again,
     so drop them before the outcome below allocates; an overflow during an
     in-place list compaction also left the allocation target in such a region.
     Their allocations are left to exit. */
  main_heap.innermost = entry_scope;
  carrier->fiber.target = entry_target;
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
     because a host event might wake it. The scheduler returns only when no
     task waits on a descriptor either, so nothing ever will: report the same
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
  /* A fatal runtime error exits from wherever it is; queued console lines still
     go out. A second stop signal (end_by_signal) skips this and drops them. */
  carrier_bind(&carrier_first);
  atexit(console_drain_all);
  struct timespec identity_time;
  clock_gettime(CLOCK_REALTIME, &identity_time);
  char generated_execution_id[96];
  snprintf(generated_execution_id, sizeof(generated_execution_id),
           "execution-%ld-%" PRId64 "-%ld", (long)getpid(),
           (int64_t)identity_time.tv_sec, identity_time.tv_nsec);
  ChimiRuntime runtime = {
    .execution_id = generated_execution_id,
    .computation = CHIMI_COMPUTATION,
    .silent = true,
    .max_line_bytes = 1024 * 1024,
    .log_level = 1,
    .next_span = 2,
    .carriers = 1,
  };
  uint32_t benchmark_repeats = 0;
  int64_t test_index = -1;
  bool clock_start_given = false;
  bool carriers_given = false;
  const char *telemetry_path = NULL;
  const char *outcome_path = NULL;
  const char *memory_report_path = NULL;
  const char *profile_path = NULL;
  const char *log_path = NULL;
  const char *metrics_path = NULL;
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
      int32_t number = 0;
      int64_t at = 0;
      if (!parse_injected_signal(argv[index] + 9, &number, &at)) {
        fputs("--signal requires INT, TERM, HUP, USR1 or USR2, @, and a time such as 2s or 150ms\n", stderr);
        return 64;
      }
      if (number == SIGINT || number == SIGTERM) {
        runtime.injected_signal = number;
        runtime.injected_signal_at = at;
      } else if (runtime.injected_user_signal_count == CHIMI_MAX_INJECTED_SIGNALS) {
        fputs("--signal is given too many times; at most 64 HUP, USR1 and USR2 signals\n", stderr);
        return 64;
      } else {
        uint32_t code = number == SIGHUP ? 1 : number == SIGUSR1 ? 2 : 3;
        runtime.injected_user_signals[runtime.injected_user_signal_count++] =
            (ChimiInjectedSignal){.code = code, .at = at};
      }
    }
    else if (strncmp(argv[index], "--random-seed=", 14) == 0) {
      errno = 0;
      char *end = NULL;
      unsigned long long value = strtoull(argv[index] + 14, &end, 10);
      if (errno == ERANGE || end == argv[index] + 14 || *end != '\0' || argv[index][14] == '-') {
        fputs("--random-seed requires an integer from 0 through 18446744073709551615\n", stderr);
        return 64;
      }
      random_seed = (uint64_t)value;
      random_seed_given = true;
    }
    else if (strncmp(argv[index], "--carriers=", 11) == 0) {
      /* strtoul skips spaces and takes a sign, so the count must start with
         a digit; one that overflows reads as ULONG_MAX, which the range
         refuses. */
      const char *count = argv[index] + 11;
      char *end = NULL;
      unsigned long value = *count >= '0' && *count <= '9' ? strtoul(count, &end, 10) : 0;
      if (value == 0 || *end != '\0' || value > CARRIERS_MAX) {
        fprintf(stderr, "--carriers requires an integer from 1 through %d\n", CARRIERS_MAX);
        return 64;
      }
      runtime.carriers = (uint32_t)value;
      carriers_given = true;
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
    else if (strncmp(argv[index], "--log-level=", 12) == 0) {
      const char *level = argv[index] + 12;
      if (strcmp(level, "debug") == 0) runtime.log_level = 0;
      else if (strcmp(level, "info") == 0) runtime.log_level = 1;
      else if (strcmp(level, "warn") == 0) runtime.log_level = 2;
      else if (strcmp(level, "error") == 0) runtime.log_level = 3;
      else {
        fputs("--log-level requires debug, info, warn or error\n", stderr);
        return 64;
      }
    }
    else if (strncmp(argv[index], "--log-file=", 11) == 0) log_path = argv[index] + 11;
    else if (strncmp(argv[index], "--metrics-file=", 15) == 0) metrics_path = argv[index] + 15;
    else if (strncmp(argv[index], "--benchmark-repeats=", 20) == 0)
      benchmark_repeats = (uint32_t)strtoul(argv[index] + 20, NULL, 10);
    else { fprintf(stderr, "unknown Chimi runtime argument: %s\n", argv[index]); return 64; }
    }
    if (!found_boundary) {
      fputs("Chimi runner configuration must end with --\n", stderr);
      return 64;
    }
    if ((runtime.injected_signal || runtime.injected_user_signal_count) && !runtime.virtual_clock) {
      fputs("--signal requires --clock=virtual; under the real clock, send the signal\n", stderr);
      return 64;
    }
    if (clock_start_given && !runtime.virtual_clock) {
      fputs("--clock-start requires --clock=virtual; the real clock reads the host's wall time\n", stderr);
      return 64;
    }
    if (runtime.carriers > 1 && runtime.virtual_clock) {
      fputs("--carriers above 1 requires --clock=real; virtual time moves only when every carrier is idle\n",
            stderr);
      return 64;
    }
#ifdef CHIMI_CARRIER_SIMULATION
    if (runtime.carriers > 1) {
      fputs("--carriers above 1 is not available in the carrier simulation build\n", stderr);
      return 64;
    }
#endif
  }
  if (!carriers_given) runtime.carriers = default_carriers(runtime.virtual_clock);
  /* Before the program runs anything, so carriers 1 and up start zeroed. */
  carriers_create(runtime.carriers);
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
  if (log_path && ((telemetry_path && strcmp(log_path, telemetry_path) == 0) ||
                   (outcome_path && strcmp(log_path, outcome_path) == 0) ||
                   (memory_report_path && strcmp(log_path, memory_report_path) == 0) ||
                   (profile_path && strcmp(log_path, profile_path) == 0))) {
    fputs("the log file must be distinct from the runner's report files\n", stderr);
    return 64;
  }
  if (metrics_path && ((telemetry_path && strcmp(metrics_path, telemetry_path) == 0) ||
                       (outcome_path && strcmp(metrics_path, outcome_path) == 0) ||
                       (memory_report_path && strcmp(metrics_path, memory_report_path) == 0) ||
                       (profile_path && strcmp(metrics_path, profile_path) == 0) ||
                       (log_path && strcmp(metrics_path, log_path) == 0))) {
    fputs("the metrics file must be distinct from the runner's report files and the log file\n", stderr);
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
    runtime.telemetry = open_report(telemetry_path, strcmp(telemetry_path, "/dev/null") == 0 ? "w" : "wx");
    if (!runtime.telemetry) {
      fprintf(stderr, "cannot open telemetry file %s: %s\n", telemetry_path, strerror(errno));
      return 74;
    }
  }
  if (outcome_path) {
    runtime.outcome = open_report(outcome_path, "wx");
    if (!runtime.outcome) {
      fprintf(stderr, "cannot open outcome file %s: %s\n", outcome_path, strerror(errno));
      if (runtime.telemetry) fclose(runtime.telemetry);
      return 74;
    }
  }
  if (log_path) {
    /* Appended to, unlike the runner's reports: a log outlives one run. */
    runtime.log_file = open_report(log_path, "a");
    if (!runtime.log_file) {
      fprintf(stderr, "cannot open log file %s: %s\n", log_path, strerror(errno));
      if (runtime.telemetry) fclose(runtime.telemetry);
      if (runtime.outcome) fclose(runtime.outcome);
      return 74;
    }
    /* The same file under another spelling (`./out.json`) is still a clash. */
    struct stat log_status, report_status;
    FILE *reports[] = {runtime.telemetry, runtime.outcome};
    for (size_t report = 0; report < 2; ++report)
      if (reports[report] && fstat(fileno(runtime.log_file), &log_status) == 0 &&
          fstat(fileno(reports[report]), &report_status) == 0 && log_status.st_dev == report_status.st_dev &&
          log_status.st_ino == report_status.st_ino && !S_ISCHR(log_status.st_mode)) {
        fputs("the log file must be distinct from the runner's report files\n", stderr);
        return 64;
      }
    console_streams[2].fd = fileno(runtime.log_file);
    console_streams[2].file = runtime.log_file;
  }
  if (metrics_path) {
    /* Compared before opening, since opening empties the file: the same file
       under another spelling is still a clash. */
    struct stat metrics_status, open_status;
    FILE *opened[] = {runtime.telemetry, runtime.outcome, runtime.log_file};
    if (stat(metrics_path, &metrics_status) == 0 && !S_ISCHR(metrics_status.st_mode))
      for (size_t file = 0; file < 3; ++file)
        if (opened[file] && fstat(fileno(opened[file]), &open_status) == 0 &&
            metrics_status.st_dev == open_status.st_dev && metrics_status.st_ino == open_status.st_ino) {
          fputs("the metrics file must be distinct from the runner's report files and the log file\n", stderr);
          return 64;
        }
    runtime.metrics_file = open_report(metrics_path, "w");
    if (!runtime.metrics_file) {
      fprintf(stderr, "cannot open metrics file %s: %s\n", metrics_path, strerror(errno));
      if (runtime.telemetry) fclose(runtime.telemetry);
      if (runtime.outcome) fclose(runtime.outcome);
      if (runtime.log_file) fclose(runtime.log_file);
      return 74;
    }
    /* The memory report and profile are opened only at exit, so their
       spellings are compared with the file just opened. */
    const char *later[] = {memory_report_path, profile_path};
    struct stat opened_status;
    for (size_t file = 0; file < 2; ++file)
      if (later[file] && stat(later[file], &open_status) == 0 &&
          fstat(fileno(runtime.metrics_file), &opened_status) == 0 && !S_ISCHR(opened_status.st_mode) &&
          open_status.st_dev == opened_status.st_dev && open_status.st_ino == opened_status.st_ino) {
        fputs("the metrics file must be distinct from the runner's report files and the log file\n", stderr);
        return 64;
      }
  }
  uint32_t entry_node = 0;
  const ChimiFunction *entry_function = NULL;
  if (test_index >= 0) {
    entry_function = &CHIMI_FUNCTIONS[CHIMI_TEST_FUNCTIONS[test_index]];
    runtime.computation = CHIMI_TEST_COMPUTATIONS[test_index];
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
    /* The metrics aggregate every repeat, so they start with the first. */
    int64_t metrics_started = 0;
    for (uint32_t repeat = 0; repeat < benchmark_repeats; ++repeat) {
      runtime.next_sequence = 0;
      runtime.next_span = 2;
      for (size_t carrier_index = 0; carrier_index < carrier_count; ++carrier_index)
        carrier_at(carrier_index)->next_span = carrier_at(carrier_index)->end_span = 0;
      runtime.start_unix_ns = start_unix_nanos(&runtime);
      if (repeat == 0) metrics_started_unix_ns = metrics_started = runtime.start_unix_ns;
      if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
      ChimiEnvironment *environment = new_environment(entry_function->local_count);
      if (entry_function->parameters_length == 1) {
        ChimiValue *arguments = application_arguments(argc - index, argv + index);
        environment->locals[environment_slot(CHIMI_IDS[entry_function->parameters_offset])] = arguments;
      }
      ChimiControl *control = runtime.cancel
        ? cancelled()
        : execute_program_entry(&runtime, entry, environment);
      console_drain_all();
      release_binding_handles();
      if (control == NULL) return 70;
      if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
    }
    bool write_failed = write_memory_report(memory_report_path, &runtime);
    write_failed = write_profile(profile_path) || write_failed;
    write_failed = write_metrics(&runtime, metrics_started, runtime.metrics_file) || write_failed;
    runtime.metrics_file = NULL;
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
  runtime.start_unix_ns = start_unix_nanos(&runtime);
  metrics_started_unix_ns = runtime.start_unix_ns;
  if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
  ChimiEnvironment *environment = new_environment(entry_function->local_count);
  if (entry_function->parameters_length == 1) {
    ChimiValue *arguments = application_arguments(argc - index, argv + index);
    environment->locals[environment_slot(CHIMI_IDS[entry_function->parameters_offset])] = arguments;
  }
  ChimiControl *control = runtime.cancel
    ? cancelled()
    : execute_program_entry(&runtime, entry, environment);
  /* Lines whose writers were cancelled while they waited (CHI-266). */
  console_drain_all();
  release_binding_handles();
  /* A log line already went out whole; a failed close loses nothing the
     program could act on. */
  if (runtime.log_file) {
    console_streams[2].fd = -1;
    fclose(runtime.log_file);
  }
  if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
  if (runtime.outcome) print_outcome(runtime.outcome, control);
  bool write_failed = write_memory_report(memory_report_path, &runtime);
  write_failed = write_profile(profile_path) || write_failed;
  write_failed = write_metrics(&runtime, runtime.start_unix_ns, runtime.metrics_file) || write_failed;
  write_failed = close_output(runtime.telemetry) || write_failed;
  write_failed = close_output(runtime.outcome) || write_failed;
  if (write_failed) return 74;
  int exit_status;
  if (program_exit(control, &exit_status)) return exit_status;
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
