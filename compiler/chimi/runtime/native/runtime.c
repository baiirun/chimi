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
               /* A `std/cause.Cause` in `as.record`: its slots, unnamed (intrinsics/causes.rs). */
               VALUE_CAUSE,
               /* A `std/test.Recorder`, `Script` or `Mailbox`: its log id in
                  `as.address.target`. */
               VALUE_TEST_RECORDER, VALUE_TEST_SCRIPT, VALUE_TEST_MAILBOX,
               /* A binding `native type` (G20) in `as.handle`: plain data naming a
                  slot of the runtime's handle table, never the resource itself. */
               VALUE_NATIVE_HANDLE } ValueKind;
/* A third address kind, beside CHIMI_ADDRESS_NAMED and CHIMI_ADDRESS_DIRECT, whose
   target is a std/test Mailbox's id (intrinsics/test_log.rs). No process owns it,
   so it never reaches the scheduler: Process.send delivers to the mailbox there. */
enum { CHIMI_ADDRESS_TEST = 3 };
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
   NULL means the fresh cause of the outcome itself (see control_cause in intrinsics/causes.rs). */
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
     trace_events_constructed change only under the trace lock (chimi-runtime
     telemetry.rs), which an emitter holds for a whole event, so events from
     several carriers come out whole and numbered in the order written. */
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
   so each count is a relaxed atomic add (allocator.rs and eval.rs). */
#define profile_rows chimi_runtime_profile_rows
ProfileRow *chimi_runtime_profile_rows;

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
  /* The thread carrier_bind gave this carrier to; chimi_runtime_task_entry_v1 checks it
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
   the test logs' lock (intrinsics/test_log.rs), and nothing under it retains, drops, parks or runs a hook.
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

/* One call of a C binding function (chimi_binding.h). */
struct ChimiBindingCall {
  ChimiRuntime *runtime;
  ChimiValue **arguments;
  uint32_t argument_count;
  uint32_t call_node;
  /* Which arguments are open handles the call holds (binding_hold). */
  bool *held;
};

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
_Static_assert(sizeof(ChimiOrigin) == 20, "layout: sizeof(ChimiOrigin)");
_Static_assert(offsetof(ChimiOrigin, file_id) == 0, "layout: offsetof(ChimiOrigin, file_id)");
_Static_assert(sizeof(((ChimiOrigin *)0)->file_id) == 4, "layout: sizeof(((ChimiOrigin *)0)->file_id)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->file_id) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->file_id)");
_Static_assert(offsetof(ChimiOrigin, semantic_id) == 4, "layout: offsetof(ChimiOrigin, semantic_id)");
_Static_assert(sizeof(((ChimiOrigin *)0)->semantic_id) == 4, "layout: sizeof(((ChimiOrigin *)0)->semantic_id)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->semantic_id) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->semantic_id)");
_Static_assert(offsetof(ChimiOrigin, start) == 8, "layout: offsetof(ChimiOrigin, start)");
_Static_assert(sizeof(((ChimiOrigin *)0)->start) == 4, "layout: sizeof(((ChimiOrigin *)0)->start)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->start) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->start)");
_Static_assert(offsetof(ChimiOrigin, end) == 12, "layout: offsetof(ChimiOrigin, end)");
_Static_assert(sizeof(((ChimiOrigin *)0)->end) == 4, "layout: sizeof(((ChimiOrigin *)0)->end)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->end) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->end)");
_Static_assert(offsetof(ChimiOrigin, location) == 16, "layout: offsetof(ChimiOrigin, location)");
_Static_assert(sizeof(((ChimiOrigin *)0)->location) == 4, "layout: sizeof(((ChimiOrigin *)0)->location)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->location) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiOrigin *)0)->location)");
_Static_assert(sizeof(ChimiNode) == 40, "layout: sizeof(ChimiNode)");
_Static_assert(offsetof(ChimiNode, origin) == 0, "layout: offsetof(ChimiNode, origin)");
_Static_assert(sizeof(((ChimiNode *)0)->origin) == 4, "layout: sizeof(((ChimiNode *)0)->origin)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->origin) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->origin)");
_Static_assert(offsetof(ChimiNode, a) == 4, "layout: offsetof(ChimiNode, a)");
_Static_assert(sizeof(((ChimiNode *)0)->a) == 4, "layout: sizeof(((ChimiNode *)0)->a)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->a) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->a)");
_Static_assert(offsetof(ChimiNode, b) == 8, "layout: offsetof(ChimiNode, b)");
_Static_assert(sizeof(((ChimiNode *)0)->b) == 4, "layout: sizeof(((ChimiNode *)0)->b)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->b) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->b)");
_Static_assert(offsetof(ChimiNode, c) == 12, "layout: offsetof(ChimiNode, c)");
_Static_assert(sizeof(((ChimiNode *)0)->c) == 4, "layout: sizeof(((ChimiNode *)0)->c)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->c) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->c)");
_Static_assert(offsetof(ChimiNode, d) == 16, "layout: offsetof(ChimiNode, d)");
_Static_assert(sizeof(((ChimiNode *)0)->d) == 4, "layout: sizeof(((ChimiNode *)0)->d)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->d) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->d)");
_Static_assert(offsetof(ChimiNode, offset) == 20, "layout: offsetof(ChimiNode, offset)");
_Static_assert(sizeof(((ChimiNode *)0)->offset) == 4, "layout: sizeof(((ChimiNode *)0)->offset)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->offset) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->offset)");
_Static_assert(offsetof(ChimiNode, length) == 24, "layout: offsetof(ChimiNode, length)");
_Static_assert(sizeof(((ChimiNode *)0)->length) == 4, "layout: sizeof(((ChimiNode *)0)->length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->length)");
_Static_assert(offsetof(ChimiNode, integer) == 32, "layout: offsetof(ChimiNode, integer)");
_Static_assert(sizeof(((ChimiNode *)0)->integer) == 8, "layout: sizeof(((ChimiNode *)0)->integer)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->integer) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiNode *)0)->integer)");
_Static_assert(sizeof(ChimiInjectedSignal) == 16, "layout: sizeof(ChimiInjectedSignal)");
_Static_assert(offsetof(ChimiInjectedSignal, code) == 0, "layout: offsetof(ChimiInjectedSignal, code)");
_Static_assert(sizeof(((ChimiInjectedSignal *)0)->code) == 4, "layout: sizeof(((ChimiInjectedSignal *)0)->code)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiInjectedSignal *)0)->code) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiInjectedSignal *)0)->code)");
_Static_assert(offsetof(ChimiInjectedSignal, at) == 8, "layout: offsetof(ChimiInjectedSignal, at)");
_Static_assert(sizeof(((ChimiInjectedSignal *)0)->at) == 8, "layout: sizeof(((ChimiInjectedSignal *)0)->at)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiInjectedSignal *)0)->at) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiInjectedSignal *)0)->at)");
_Static_assert(sizeof(ChimiRuntime) == 1216, "layout: sizeof(ChimiRuntime)");
_Static_assert(offsetof(ChimiRuntime, execution_id) == 0, "layout: offsetof(ChimiRuntime, execution_id)");
_Static_assert(sizeof(((ChimiRuntime *)0)->execution_id) == 8, "layout: sizeof(((ChimiRuntime *)0)->execution_id)");
_Static_assert(offsetof(ChimiRuntime, computation) == 8, "layout: offsetof(ChimiRuntime, computation)");
_Static_assert(sizeof(((ChimiRuntime *)0)->computation) == 8, "layout: sizeof(((ChimiRuntime *)0)->computation)");
_Static_assert(offsetof(ChimiRuntime, full_trace) == 16, "layout: offsetof(ChimiRuntime, full_trace)");
_Static_assert(sizeof(((ChimiRuntime *)0)->full_trace) == 1, "layout: sizeof(((ChimiRuntime *)0)->full_trace)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->full_trace) == 3, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->full_trace)");
_Static_assert(offsetof(ChimiRuntime, virtual_clock) == 17, "layout: offsetof(ChimiRuntime, virtual_clock)");
_Static_assert(sizeof(((ChimiRuntime *)0)->virtual_clock) == 1, "layout: sizeof(((ChimiRuntime *)0)->virtual_clock)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->virtual_clock) == 3, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->virtual_clock)");
_Static_assert(offsetof(ChimiRuntime, carriers) == 20, "layout: offsetof(ChimiRuntime, carriers)");
_Static_assert(sizeof(((ChimiRuntime *)0)->carriers) == 4, "layout: sizeof(((ChimiRuntime *)0)->carriers)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->carriers) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->carriers)");
_Static_assert(offsetof(ChimiRuntime, clock_start) == 24, "layout: offsetof(ChimiRuntime, clock_start)");
_Static_assert(sizeof(((ChimiRuntime *)0)->clock_start) == 8, "layout: sizeof(((ChimiRuntime *)0)->clock_start)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->clock_start) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->clock_start)");
_Static_assert(offsetof(ChimiRuntime, start_unix_ns) == 32, "layout: offsetof(ChimiRuntime, start_unix_ns)");
_Static_assert(sizeof(((ChimiRuntime *)0)->start_unix_ns) == 8, "layout: sizeof(((ChimiRuntime *)0)->start_unix_ns)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->start_unix_ns) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->start_unix_ns)");
_Static_assert(offsetof(ChimiRuntime, deadlock_waiters) == 40, "layout: offsetof(ChimiRuntime, deadlock_waiters)");
_Static_assert(sizeof(((ChimiRuntime *)0)->deadlock_waiters) == 8, "layout: sizeof(((ChimiRuntime *)0)->deadlock_waiters)");
_Static_assert(offsetof(ChimiRuntime, last_defect_incarnation) == 48, "layout: offsetof(ChimiRuntime, last_defect_incarnation)");
_Static_assert(sizeof(((ChimiRuntime *)0)->last_defect_incarnation) == 8, "layout: sizeof(((ChimiRuntime *)0)->last_defect_incarnation)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->last_defect_incarnation) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->last_defect_incarnation)");
_Static_assert(offsetof(ChimiRuntime, last_defect_membership) == 56, "layout: offsetof(ChimiRuntime, last_defect_membership)");
_Static_assert(sizeof(((ChimiRuntime *)0)->last_defect_membership) == 8, "layout: sizeof(((ChimiRuntime *)0)->last_defect_membership)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->last_defect_membership) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->last_defect_membership)");
_Static_assert(offsetof(ChimiRuntime, last_defect_message) == 64, "layout: offsetof(ChimiRuntime, last_defect_message)");
_Static_assert(sizeof(((ChimiRuntime *)0)->last_defect_message) == 8, "layout: sizeof(((ChimiRuntime *)0)->last_defect_message)");
_Static_assert(offsetof(ChimiRuntime, process_defect_count) == 72, "layout: offsetof(ChimiRuntime, process_defect_count)");
_Static_assert(sizeof(((ChimiRuntime *)0)->process_defect_count) == 8, "layout: sizeof(((ChimiRuntime *)0)->process_defect_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->process_defect_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->process_defect_count)");
_Static_assert(offsetof(ChimiRuntime, capability_violations) == 80, "layout: offsetof(ChimiRuntime, capability_violations)");
_Static_assert(sizeof(((ChimiRuntime *)0)->capability_violations) == 8, "layout: sizeof(((ChimiRuntime *)0)->capability_violations)");
_Static_assert(offsetof(ChimiRuntime, cancel) == 88, "layout: offsetof(ChimiRuntime, cancel)");
_Static_assert(sizeof(((ChimiRuntime *)0)->cancel) == 4, "layout: sizeof(((ChimiRuntime *)0)->cancel)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->cancel) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->cancel)");
_Static_assert(offsetof(ChimiRuntime, injected_signal) == 92, "layout: offsetof(ChimiRuntime, injected_signal)");
_Static_assert(sizeof(((ChimiRuntime *)0)->injected_signal) == 4, "layout: sizeof(((ChimiRuntime *)0)->injected_signal)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->injected_signal) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->injected_signal)");
_Static_assert(offsetof(ChimiRuntime, injected_signal_at) == 96, "layout: offsetof(ChimiRuntime, injected_signal_at)");
_Static_assert(sizeof(((ChimiRuntime *)0)->injected_signal_at) == 8, "layout: sizeof(((ChimiRuntime *)0)->injected_signal_at)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->injected_signal_at) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->injected_signal_at)");
_Static_assert(offsetof(ChimiRuntime, injected_user_signals) == 104, "layout: offsetof(ChimiRuntime, injected_user_signals)");
_Static_assert(sizeof(((ChimiRuntime *)0)->injected_user_signals) == 1024, "layout: sizeof(((ChimiRuntime *)0)->injected_user_signals)");
_Static_assert(offsetof(ChimiRuntime, injected_user_signal_count) == 1128, "layout: offsetof(ChimiRuntime, injected_user_signal_count)");
_Static_assert(sizeof(((ChimiRuntime *)0)->injected_user_signal_count) == 8, "layout: sizeof(((ChimiRuntime *)0)->injected_user_signal_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->injected_user_signal_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->injected_user_signal_count)");
_Static_assert(offsetof(ChimiRuntime, silent) == 1136, "layout: offsetof(ChimiRuntime, silent)");
_Static_assert(sizeof(((ChimiRuntime *)0)->silent) == 1, "layout: sizeof(((ChimiRuntime *)0)->silent)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->silent) == 3, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->silent)");
_Static_assert(offsetof(ChimiRuntime, telemetry) == 1144, "layout: offsetof(ChimiRuntime, telemetry)");
_Static_assert(sizeof(((ChimiRuntime *)0)->telemetry) == 8, "layout: sizeof(((ChimiRuntime *)0)->telemetry)");
_Static_assert(offsetof(ChimiRuntime, outcome) == 1152, "layout: offsetof(ChimiRuntime, outcome)");
_Static_assert(sizeof(((ChimiRuntime *)0)->outcome) == 8, "layout: sizeof(((ChimiRuntime *)0)->outcome)");
_Static_assert(offsetof(ChimiRuntime, max_line_bytes) == 1160, "layout: offsetof(ChimiRuntime, max_line_bytes)");
_Static_assert(sizeof(((ChimiRuntime *)0)->max_line_bytes) == 4, "layout: sizeof(((ChimiRuntime *)0)->max_line_bytes)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->max_line_bytes) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->max_line_bytes)");
_Static_assert(offsetof(ChimiRuntime, log_level) == 1168, "layout: offsetof(ChimiRuntime, log_level)");
_Static_assert(sizeof(((ChimiRuntime *)0)->log_level) == 8, "layout: sizeof(((ChimiRuntime *)0)->log_level)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->log_level) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->log_level)");
_Static_assert(offsetof(ChimiRuntime, log_file) == 1176, "layout: offsetof(ChimiRuntime, log_file)");
_Static_assert(sizeof(((ChimiRuntime *)0)->log_file) == 8, "layout: sizeof(((ChimiRuntime *)0)->log_file)");
_Static_assert(offsetof(ChimiRuntime, metrics_file) == 1184, "layout: offsetof(ChimiRuntime, metrics_file)");
_Static_assert(sizeof(((ChimiRuntime *)0)->metrics_file) == 8, "layout: sizeof(((ChimiRuntime *)0)->metrics_file)");
_Static_assert(offsetof(ChimiRuntime, next_sequence) == 1192, "layout: offsetof(ChimiRuntime, next_sequence)");
_Static_assert(sizeof(((ChimiRuntime *)0)->next_sequence) == 8, "layout: sizeof(((ChimiRuntime *)0)->next_sequence)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->next_sequence) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->next_sequence)");
_Static_assert(offsetof(ChimiRuntime, next_span) == 1200, "layout: offsetof(ChimiRuntime, next_span)");
_Static_assert(sizeof(((ChimiRuntime *)0)->next_span) == 8, "layout: sizeof(((ChimiRuntime *)0)->next_span)");
_Static_assert(offsetof(ChimiRuntime, trace_events_constructed) == 1208, "layout: offsetof(ChimiRuntime, trace_events_constructed)");
_Static_assert(sizeof(((ChimiRuntime *)0)->trace_events_constructed) == 8, "layout: sizeof(((ChimiRuntime *)0)->trace_events_constructed)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->trace_events_constructed) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiRuntime *)0)->trace_events_constructed)");
_Static_assert(sizeof(ChimiSchedulerTraceEvent) == 144, "layout: sizeof(ChimiSchedulerTraceEvent)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, family) == 0, "layout: offsetof(ChimiSchedulerTraceEvent, family)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->family) == 4, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->family)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->family) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->family)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, kind) == 4, "layout: offsetof(ChimiSchedulerTraceEvent, kind)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->kind) == 4, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->kind)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, sequence) == 8, "layout: offsetof(ChimiSchedulerTraceEvent, sequence)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->sequence) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->sequence)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->sequence) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->sequence)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, task) == 16, "layout: offsetof(ChimiSchedulerTraceEvent, task)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->task) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->task)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->task) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->task)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, parent) == 24, "layout: offsetof(ChimiSchedulerTraceEvent, parent)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->parent) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->parent)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->parent) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->parent)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, cause) == 32, "layout: offsetof(ChimiSchedulerTraceEvent, cause)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->cause) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->cause)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->cause) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->cause)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, channel) == 40, "layout: offsetof(ChimiSchedulerTraceEvent, channel)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->channel) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->channel)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->channel) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->channel)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, capacity) == 48, "layout: offsetof(ChimiSchedulerTraceEvent, capacity)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->capacity) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->capacity)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->capacity) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->capacity)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, queue_depth) == 56, "layout: offsetof(ChimiSchedulerTraceEvent, queue_depth)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->queue_depth) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->queue_depth)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->queue_depth) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->queue_depth)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, waiter_depth) == 64, "layout: offsetof(ChimiSchedulerTraceEvent, waiter_depth)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->waiter_depth) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->waiter_depth)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->waiter_depth) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->waiter_depth)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, discard_count) == 72, "layout: offsetof(ChimiSchedulerTraceEvent, discard_count)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->discard_count) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->discard_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->discard_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->discard_count)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, supervisor) == 80, "layout: offsetof(ChimiSchedulerTraceEvent, supervisor)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->supervisor) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->supervisor)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->supervisor) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->supervisor)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, membership) == 88, "layout: offsetof(ChimiSchedulerTraceEvent, membership)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->membership) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->membership)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->membership) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->membership)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, incarnation) == 96, "layout: offsetof(ChimiSchedulerTraceEvent, incarnation)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->incarnation) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->incarnation)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->incarnation) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->incarnation)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, name) == 104, "layout: offsetof(ChimiSchedulerTraceEvent, name)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->name) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->name)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->name) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->name)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, parent_supervisor) == 112, "layout: offsetof(ChimiSchedulerTraceEvent, parent_supervisor)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->parent_supervisor) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->parent_supervisor)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->parent_supervisor) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->parent_supervisor)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, attempts_in_window) == 120, "layout: offsetof(ChimiSchedulerTraceEvent, attempts_in_window)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->attempts_in_window) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->attempts_in_window)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->attempts_in_window) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->attempts_in_window)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, time) == 128, "layout: offsetof(ChimiSchedulerTraceEvent, time)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->time) == 8, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->time)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->time) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->time)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, address_kind) == 136, "layout: offsetof(ChimiSchedulerTraceEvent, address_kind)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->address_kind) == 4, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->address_kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->address_kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->address_kind)");
_Static_assert(offsetof(ChimiSchedulerTraceEvent, outcome) == 140, "layout: offsetof(ChimiSchedulerTraceEvent, outcome)");
_Static_assert(sizeof(((ChimiSchedulerTraceEvent *)0)->outcome) == 4, "layout: sizeof(((ChimiSchedulerTraceEvent *)0)->outcome)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->outcome) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerTraceEvent *)0)->outcome)");
_Static_assert(CHIMI_MAX_INJECTED_SIGNALS == 64, "layout: CHIMI_MAX_INJECTED_SIGNALS");
_Static_assert(CHIMI_SCHEDULER_TRACE_TASK == 1, "layout: CHIMI_SCHEDULER_TRACE_TASK");
_Static_assert(CHIMI_SCHEDULER_TRACE_CHANNEL == 2, "layout: CHIMI_SCHEDULER_TRACE_CHANNEL");
_Static_assert(CHIMI_SCHEDULER_TRACE_PROCESS == 3, "layout: CHIMI_SCHEDULER_TRACE_PROCESS");
_Static_assert(CHIMI_SCHEDULER_TRACE_CLOCK == 4, "layout: CHIMI_SCHEDULER_TRACE_CLOCK");
_Static_assert(CHIMI_PROCESS_TRACE_ADMISSION_REJECTED == 2, "layout: CHIMI_PROCESS_TRACE_ADMISSION_REJECTED");
_Static_assert(CHIMI_PROCESS_TRACE_TERMINATED == 3, "layout: CHIMI_PROCESS_TRACE_TERMINATED");
_Static_assert(CHIMI_PROCESS_TRACE_SEND == 9, "layout: CHIMI_PROCESS_TRACE_SEND");
_Static_assert(CHIMI_PROCESS_CAUSE_DEFECTED == 3, "layout: CHIMI_PROCESS_CAUSE_DEFECTED");
_Static_assert(CHIMI_PROCESS_DEFECT_STACK_OVERFLOW == 1, "layout: CHIMI_PROCESS_DEFECT_STACK_OVERFLOW");
_Static_assert(CHIMI_CLOCK_TRACE_SELECT_COMMITTED == 8, "layout: CHIMI_CLOCK_TRACE_SELECT_COMMITTED");
_Static_assert(CHIMI_ADDRESS_NAMED == 1, "layout: CHIMI_ADDRESS_NAMED");
_Static_assert(CHIMI_ADDRESS_DIRECT == 2, "layout: CHIMI_ADDRESS_DIRECT");
_Static_assert(sizeof(ChimiBindingCall) == 32, "layout: sizeof(ChimiBindingCall)");
_Static_assert(offsetof(ChimiBindingCall, runtime) == 0, "layout: offsetof(ChimiBindingCall, runtime)");
_Static_assert(sizeof(((ChimiBindingCall *)0)->runtime) == 8, "layout: sizeof(((ChimiBindingCall *)0)->runtime)");
_Static_assert(offsetof(ChimiBindingCall, arguments) == 8, "layout: offsetof(ChimiBindingCall, arguments)");
_Static_assert(sizeof(((ChimiBindingCall *)0)->arguments) == 8, "layout: sizeof(((ChimiBindingCall *)0)->arguments)");
_Static_assert(offsetof(ChimiBindingCall, argument_count) == 16, "layout: offsetof(ChimiBindingCall, argument_count)");
_Static_assert(sizeof(((ChimiBindingCall *)0)->argument_count) == 4, "layout: sizeof(((ChimiBindingCall *)0)->argument_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiBindingCall *)0)->argument_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiBindingCall *)0)->argument_count)");
_Static_assert(offsetof(ChimiBindingCall, call_node) == 20, "layout: offsetof(ChimiBindingCall, call_node)");
_Static_assert(sizeof(((ChimiBindingCall *)0)->call_node) == 4, "layout: sizeof(((ChimiBindingCall *)0)->call_node)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiBindingCall *)0)->call_node) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiBindingCall *)0)->call_node)");
_Static_assert(offsetof(ChimiBindingCall, held) == 24, "layout: offsetof(ChimiBindingCall, held)");
_Static_assert(sizeof(((ChimiBindingCall *)0)->held) == 8, "layout: sizeof(((ChimiBindingCall *)0)->held)");
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
_Static_assert(sizeof(ChimiFunction) == 20, "layout: sizeof(ChimiFunction)");
_Static_assert(offsetof(ChimiFunction, symbol) == 0, "layout: offsetof(ChimiFunction, symbol)");
_Static_assert(sizeof(((ChimiFunction *)0)->symbol) == 4, "layout: sizeof(((ChimiFunction *)0)->symbol)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->symbol) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->symbol)");
_Static_assert(offsetof(ChimiFunction, body) == 4, "layout: offsetof(ChimiFunction, body)");
_Static_assert(sizeof(((ChimiFunction *)0)->body) == 4, "layout: sizeof(((ChimiFunction *)0)->body)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->body) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->body)");
_Static_assert(offsetof(ChimiFunction, parameters_offset) == 8, "layout: offsetof(ChimiFunction, parameters_offset)");
_Static_assert(sizeof(((ChimiFunction *)0)->parameters_offset) == 4, "layout: sizeof(((ChimiFunction *)0)->parameters_offset)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->parameters_offset) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->parameters_offset)");
_Static_assert(offsetof(ChimiFunction, parameters_length) == 12, "layout: offsetof(ChimiFunction, parameters_length)");
_Static_assert(sizeof(((ChimiFunction *)0)->parameters_length) == 4, "layout: sizeof(((ChimiFunction *)0)->parameters_length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->parameters_length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->parameters_length)");
_Static_assert(offsetof(ChimiFunction, local_count) == 16, "layout: offsetof(ChimiFunction, local_count)");
_Static_assert(sizeof(((ChimiFunction *)0)->local_count) == 4, "layout: sizeof(((ChimiFunction *)0)->local_count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->local_count) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiFunction *)0)->local_count)");
_Static_assert(sizeof(ChimiNamedNode) == 8, "layout: sizeof(ChimiNamedNode)");
_Static_assert(offsetof(ChimiNamedNode, name) == 0, "layout: offsetof(ChimiNamedNode, name)");
_Static_assert(sizeof(((ChimiNamedNode *)0)->name) == 4, "layout: sizeof(((ChimiNamedNode *)0)->name)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNamedNode *)0)->name) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiNamedNode *)0)->name)");
_Static_assert(offsetof(ChimiNamedNode, node) == 4, "layout: offsetof(ChimiNamedNode, node)");
_Static_assert(sizeof(((ChimiNamedNode *)0)->node) == 4, "layout: sizeof(((ChimiNamedNode *)0)->node)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiNamedNode *)0)->node) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiNamedNode *)0)->node)");
_Static_assert(sizeof(ChimiStep) == 12, "layout: sizeof(ChimiStep)");
_Static_assert(offsetof(ChimiStep, kind) == 0, "layout: offsetof(ChimiStep, kind)");
_Static_assert(sizeof(((ChimiStep *)0)->kind) == 4, "layout: sizeof(((ChimiStep *)0)->kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiStep *)0)->kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiStep *)0)->kind)");
_Static_assert(offsetof(ChimiStep, symbol) == 4, "layout: offsetof(ChimiStep, symbol)");
_Static_assert(sizeof(((ChimiStep *)0)->symbol) == 4, "layout: sizeof(((ChimiStep *)0)->symbol)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiStep *)0)->symbol) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiStep *)0)->symbol)");
_Static_assert(offsetof(ChimiStep, node) == 8, "layout: offsetof(ChimiStep, node)");
_Static_assert(sizeof(((ChimiStep *)0)->node) == 4, "layout: sizeof(((ChimiStep *)0)->node)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiStep *)0)->node) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiStep *)0)->node)");
_Static_assert(sizeof(ChimiArm) == 24, "layout: sizeof(ChimiArm)");
_Static_assert(offsetof(ChimiArm, patterns_offset) == 0, "layout: offsetof(ChimiArm, patterns_offset)");
_Static_assert(sizeof(((ChimiArm *)0)->patterns_offset) == 4, "layout: sizeof(((ChimiArm *)0)->patterns_offset)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->patterns_offset) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->patterns_offset)");
_Static_assert(offsetof(ChimiArm, patterns_length) == 4, "layout: offsetof(ChimiArm, patterns_length)");
_Static_assert(sizeof(((ChimiArm *)0)->patterns_length) == 4, "layout: sizeof(((ChimiArm *)0)->patterns_length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->patterns_length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->patterns_length)");
_Static_assert(offsetof(ChimiArm, guard) == 8, "layout: offsetof(ChimiArm, guard)");
_Static_assert(sizeof(((ChimiArm *)0)->guard) == 4, "layout: sizeof(((ChimiArm *)0)->guard)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->guard) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->guard)");
_Static_assert(offsetof(ChimiArm, body) == 12, "layout: offsetof(ChimiArm, body)");
_Static_assert(sizeof(((ChimiArm *)0)->body) == 4, "layout: sizeof(((ChimiArm *)0)->body)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->body) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->body)");
_Static_assert(offsetof(ChimiArm, cause_subject) == 16, "layout: offsetof(ChimiArm, cause_subject)");
_Static_assert(sizeof(((ChimiArm *)0)->cause_subject) == 4, "layout: sizeof(((ChimiArm *)0)->cause_subject)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->cause_subject) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->cause_subject)");
_Static_assert(offsetof(ChimiArm, cause_local) == 20, "layout: offsetof(ChimiArm, cause_local)");
_Static_assert(sizeof(((ChimiArm *)0)->cause_local) == 4, "layout: sizeof(((ChimiArm *)0)->cause_local)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->cause_local) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiArm *)0)->cause_local)");
_Static_assert(sizeof(ChimiSchedulerRequest) == 64, "layout: sizeof(ChimiSchedulerRequest)");
_Static_assert(offsetof(ChimiSchedulerRequest, kind) == 0, "layout: offsetof(ChimiSchedulerRequest, kind)");
_Static_assert(sizeof(((ChimiSchedulerRequest *)0)->kind) == 4, "layout: sizeof(((ChimiSchedulerRequest *)0)->kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->kind)");
_Static_assert(offsetof(ChimiSchedulerRequest, status) == 4, "layout: offsetof(ChimiSchedulerRequest, status)");
_Static_assert(sizeof(((ChimiSchedulerRequest *)0)->status) == 4, "layout: sizeof(((ChimiSchedulerRequest *)0)->status)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->status) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->status)");
_Static_assert(offsetof(ChimiSchedulerRequest, task) == 8, "layout: offsetof(ChimiSchedulerRequest, task)");
_Static_assert(sizeof(((ChimiSchedulerRequest *)0)->task) == 8, "layout: sizeof(((ChimiSchedulerRequest *)0)->task)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->task) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->task)");
_Static_assert(offsetof(ChimiSchedulerRequest, channel) == 16, "layout: offsetof(ChimiSchedulerRequest, channel)");
_Static_assert(sizeof(((ChimiSchedulerRequest *)0)->channel) == 8, "layout: sizeof(((ChimiSchedulerRequest *)0)->channel)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->channel) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->channel)");
_Static_assert(offsetof(ChimiSchedulerRequest, owner) == 24, "layout: offsetof(ChimiSchedulerRequest, owner)");
_Static_assert(sizeof(((ChimiSchedulerRequest *)0)->owner) == 8, "layout: sizeof(((ChimiSchedulerRequest *)0)->owner)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->owner) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->owner)");
_Static_assert(offsetof(ChimiSchedulerRequest, token) == 32, "layout: offsetof(ChimiSchedulerRequest, token)");
_Static_assert(sizeof(((ChimiSchedulerRequest *)0)->token) == 8, "layout: sizeof(((ChimiSchedulerRequest *)0)->token)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->token) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->token)");
_Static_assert(offsetof(ChimiSchedulerRequest, value) == 40, "layout: offsetof(ChimiSchedulerRequest, value)");
_Static_assert(sizeof(((ChimiSchedulerRequest *)0)->value) == 8, "layout: sizeof(((ChimiSchedulerRequest *)0)->value)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->value) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiSchedulerRequest *)0)->value)");
_Static_assert(offsetof(ChimiSchedulerRequest, entry) == 48, "layout: offsetof(ChimiSchedulerRequest, entry)");
_Static_assert(sizeof(((ChimiSchedulerRequest *)0)->entry) == 8, "layout: sizeof(((ChimiSchedulerRequest *)0)->entry)");
_Static_assert(offsetof(ChimiSchedulerRequest, context) == 56, "layout: offsetof(ChimiSchedulerRequest, context)");
_Static_assert(sizeof(((ChimiSchedulerRequest *)0)->context) == 8, "layout: sizeof(((ChimiSchedulerRequest *)0)->context)");
_Static_assert(sizeof(ChimiMessageCapabilities) == 16, "layout: sizeof(ChimiMessageCapabilities)");
_Static_assert(offsetof(ChimiMessageCapabilities, items) == 0, "layout: offsetof(ChimiMessageCapabilities, items)");
_Static_assert(sizeof(((ChimiMessageCapabilities *)0)->items) == 8, "layout: sizeof(((ChimiMessageCapabilities *)0)->items)");
_Static_assert(offsetof(ChimiMessageCapabilities, count) == 8, "layout: offsetof(ChimiMessageCapabilities, count)");
_Static_assert(sizeof(((ChimiMessageCapabilities *)0)->count) == 8, "layout: sizeof(((ChimiMessageCapabilities *)0)->count)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiMessageCapabilities *)0)->count) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiMessageCapabilities *)0)->count)");
_Static_assert(CHIMI_FIBER_CANCEL == 2, "layout: CHIMI_FIBER_CANCEL");
_Static_assert(CHIMI_REQUEST_TASK_START == 1, "layout: CHIMI_REQUEST_TASK_START");
_Static_assert(CHIMI_REQUEST_TASK_RESULT == 2, "layout: CHIMI_REQUEST_TASK_RESULT");
_Static_assert(CHIMI_REQUEST_TASK_ANY == 42, "layout: CHIMI_REQUEST_TASK_ANY");
_Static_assert(CHIMI_REQUEST_TASK_DRAIN == 43, "layout: CHIMI_REQUEST_TASK_DRAIN");
_Static_assert(CHIMI_RESPONSE_ERROR == 7, "layout: CHIMI_RESPONSE_ERROR");
_Static_assert(sizeof(ChimiTaskContext) == 48, "layout: sizeof(ChimiTaskContext)");
_Static_assert(offsetof(ChimiTaskContext, runtime) == 0, "layout: offsetof(ChimiTaskContext, runtime)");
_Static_assert(sizeof(((ChimiTaskContext *)0)->runtime) == 8, "layout: sizeof(((ChimiTaskContext *)0)->runtime)");
_Static_assert(offsetof(ChimiTaskContext, environment) == 8, "layout: offsetof(ChimiTaskContext, environment)");
_Static_assert(sizeof(((ChimiTaskContext *)0)->environment) == 8, "layout: sizeof(((ChimiTaskContext *)0)->environment)");
_Static_assert(offsetof(ChimiTaskContext, entry) == 16, "layout: offsetof(ChimiTaskContext, entry)");
_Static_assert(sizeof(((ChimiTaskContext *)0)->entry) == 8, "layout: sizeof(((ChimiTaskContext *)0)->entry)");
_Static_assert(offsetof(ChimiTaskContext, captures) == 24, "layout: offsetof(ChimiTaskContext, captures)");
_Static_assert(sizeof(((ChimiTaskContext *)0)->captures) == 8, "layout: sizeof(((ChimiTaskContext *)0)->captures)");
_Static_assert(offsetof(ChimiTaskContext, span) == 32, "layout: offsetof(ChimiTaskContext, span)");
_Static_assert(sizeof(((ChimiTaskContext *)0)->span) == 8, "layout: sizeof(((ChimiTaskContext *)0)->span)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiTaskContext *)0)->span) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiTaskContext *)0)->span)");
_Static_assert(offsetof(ChimiTaskContext, parent) == 40, "layout: offsetof(ChimiTaskContext, parent)");
_Static_assert(sizeof(((ChimiTaskContext *)0)->parent) == 8, "layout: sizeof(((ChimiTaskContext *)0)->parent)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiTaskContext *)0)->parent) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiTaskContext *)0)->parent)");
_Static_assert(CHIMI_REQUEST_TASK_CANCEL == 3, "layout: CHIMI_REQUEST_TASK_CANCEL");
_Static_assert(CHIMI_REQUEST_CHANNEL_BOUNDED == 4, "layout: CHIMI_REQUEST_CHANNEL_BOUNDED");
_Static_assert(CHIMI_REQUEST_CHANNEL_SEND == 5, "layout: CHIMI_REQUEST_CHANNEL_SEND");
_Static_assert(CHIMI_REQUEST_CHANNEL_RECEIVE == 6, "layout: CHIMI_REQUEST_CHANNEL_RECEIVE");
_Static_assert(CHIMI_REQUEST_CHANNEL_CLOSE == 7, "layout: CHIMI_REQUEST_CHANNEL_CLOSE");
_Static_assert(CHIMI_REQUEST_CHANNEL_ABORT == 8, "layout: CHIMI_REQUEST_CHANNEL_ABORT");
_Static_assert(CHIMI_REQUEST_CHANNEL_ADD_RECEIVER == 36, "layout: CHIMI_REQUEST_CHANNEL_ADD_RECEIVER");
_Static_assert(CHIMI_REQUEST_CHANNEL_BROADCAST == 37, "layout: CHIMI_REQUEST_CHANNEL_BROADCAST");
_Static_assert(CHIMI_REQUEST_CHANNEL_WATCH == 38, "layout: CHIMI_REQUEST_CHANNEL_WATCH");
_Static_assert(CHIMI_REQUEST_CHANNEL_SUBSCRIBE == 39, "layout: CHIMI_REQUEST_CHANNEL_SUBSCRIBE");
_Static_assert(CHIMI_RESPONSE_SUCCEEDED == 3, "layout: CHIMI_RESPONSE_SUCCEEDED");
_Static_assert(CHIMI_RESPONSE_CANCELLED == 4, "layout: CHIMI_RESPONSE_CANCELLED");
_Static_assert(CHIMI_RESPONSE_DEFECTED == 5, "layout: CHIMI_RESPONSE_DEFECTED");
_Static_assert(CHIMI_RESPONSE_STACK_OVERFLOW == 6, "layout: CHIMI_RESPONSE_STACK_OVERFLOW");
_Static_assert(CHIMI_RESPONSE_CLOSED == 8, "layout: CHIMI_RESPONSE_CLOSED");
_Static_assert(CHIMI_RESPONSE_ABORTED == 9, "layout: CHIMI_RESPONSE_ABORTED");
_Static_assert(CHIMI_RESPONSE_DRAINED == 10, "layout: CHIMI_RESPONSE_DRAINED");
_Static_assert(CHIMI_REQUEST_SCOPE_ENTER == 25, "layout: CHIMI_REQUEST_SCOPE_ENTER");
_Static_assert(CHIMI_REQUEST_SCOPE_EXIT == 26, "layout: CHIMI_REQUEST_SCOPE_EXIT");
_Static_assert(CHIMI_REQUEST_SCOPE_CHECK == 27, "layout: CHIMI_REQUEST_SCOPE_CHECK");
_Static_assert(CHIMI_REQUEST_RESOURCE_ENTER == 33, "layout: CHIMI_REQUEST_RESOURCE_ENTER");
_Static_assert(CHIMI_REQUEST_RESOURCE_RELEASE == 34, "layout: CHIMI_REQUEST_RESOURCE_RELEASE");
_Static_assert(CHIMI_REQUEST_RESOURCE_EXIT == 35, "layout: CHIMI_REQUEST_RESOURCE_EXIT");
_Static_assert(CHIMI_REQUEST_SCOPE_CANCEL == 41, "layout: CHIMI_REQUEST_SCOPE_CANCEL");
_Static_assert(CHIMI_REQUEST_RESOURCE_USE == 49, "layout: CHIMI_REQUEST_RESOURCE_USE");
_Static_assert(CHIMI_RESPONSE_UNSET == 0, "layout: CHIMI_RESPONSE_UNSET");
_Static_assert(CHIMI_RESPONSE_OK == 1, "layout: CHIMI_RESPONSE_OK");
_Static_assert(CHIMI_RESPONSE_PENDING == 2, "layout: CHIMI_RESPONSE_PENDING");
_Static_assert(sizeof(ChimiPattern) == 24, "layout: sizeof(ChimiPattern)");
_Static_assert(offsetof(ChimiPattern, kind) == 0, "layout: offsetof(ChimiPattern, kind)");
_Static_assert(sizeof(((ChimiPattern *)0)->kind) == 4, "layout: sizeof(((ChimiPattern *)0)->kind)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->kind) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->kind)");
_Static_assert(offsetof(ChimiPattern, a) == 4, "layout: offsetof(ChimiPattern, a)");
_Static_assert(sizeof(((ChimiPattern *)0)->a) == 4, "layout: sizeof(((ChimiPattern *)0)->a)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->a) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->a)");
_Static_assert(offsetof(ChimiPattern, offset) == 8, "layout: offsetof(ChimiPattern, offset)");
_Static_assert(sizeof(((ChimiPattern *)0)->offset) == 4, "layout: sizeof(((ChimiPattern *)0)->offset)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->offset) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->offset)");
_Static_assert(offsetof(ChimiPattern, length) == 12, "layout: offsetof(ChimiPattern, length)");
_Static_assert(sizeof(((ChimiPattern *)0)->length) == 4, "layout: sizeof(((ChimiPattern *)0)->length)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->length) == 2, "layout: CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->length)");
_Static_assert(offsetof(ChimiPattern, integer) == 16, "layout: offsetof(ChimiPattern, integer)");
_Static_assert(sizeof(((ChimiPattern *)0)->integer) == 8, "layout: sizeof(((ChimiPattern *)0)->integer)");
_Static_assert(CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->integer) == 1, "layout: CHIMI_LAYOUT_CLASS(((ChimiPattern *)0)->integer)");
_Static_assert(CHIMI_SYMBOL_FUNCTION == 0, "layout: CHIMI_SYMBOL_FUNCTION");
_Static_assert(CHIMI_SYMBOL_CONSTANT == 1, "layout: CHIMI_SYMBOL_CONSTANT");
_Static_assert(CHIMI_SYMBOL_REQUIREMENT == 2, "layout: CHIMI_SYMBOL_REQUIREMENT");
_Static_assert(CHIMI_SYMBOL_LOCAL == 3, "layout: CHIMI_SYMBOL_LOCAL");
_Static_assert(CHIMI_SYMBOL_CONSTRUCTOR == 4, "layout: CHIMI_SYMBOL_CONSTRUCTOR");
_Static_assert(CHIMI_SYMBOL_INTRINSIC == 5, "layout: CHIMI_SYMBOL_INTRINSIC");
_Static_assert(CHIMI_SYMBOL_PROCESS_NAME == 7, "layout: CHIMI_SYMBOL_PROCESS_NAME");
_Static_assert(CHIMI_LITERAL_INTEGER == 0, "layout: CHIMI_LITERAL_INTEGER");
_Static_assert(CHIMI_LITERAL_TEXT == 1, "layout: CHIMI_LITERAL_TEXT");
_Static_assert(CHIMI_LITERAL_BOOLEAN == 2, "layout: CHIMI_LITERAL_BOOLEAN");
_Static_assert(CHIMI_LITERAL_FLOAT == 3, "layout: CHIMI_LITERAL_FLOAT");
_Static_assert(CHIMI_LITERAL_VOID == 4, "layout: CHIMI_LITERAL_VOID");
_Static_assert(CHIMI_BINARY_ADD == 0, "layout: CHIMI_BINARY_ADD");
_Static_assert(CHIMI_BINARY_SUBTRACT == 1, "layout: CHIMI_BINARY_SUBTRACT");
_Static_assert(CHIMI_BINARY_MULTIPLY == 2, "layout: CHIMI_BINARY_MULTIPLY");
_Static_assert(CHIMI_BINARY_DIVIDE == 3, "layout: CHIMI_BINARY_DIVIDE");
_Static_assert(CHIMI_BINARY_LESS == 4, "layout: CHIMI_BINARY_LESS");
_Static_assert(CHIMI_BINARY_LESS_EQUAL == 5, "layout: CHIMI_BINARY_LESS_EQUAL");
_Static_assert(CHIMI_BINARY_GREATER == 6, "layout: CHIMI_BINARY_GREATER");
_Static_assert(CHIMI_BINARY_GREATER_EQUAL == 7, "layout: CHIMI_BINARY_GREATER_EQUAL");
_Static_assert(CHIMI_BINARY_REMAINDER == 8, "layout: CHIMI_BINARY_REMAINDER");
_Static_assert(CHIMI_BINARY_EQUAL == 9, "layout: CHIMI_BINARY_EQUAL");
_Static_assert(CHIMI_BINARY_RANGE == 10, "layout: CHIMI_BINARY_RANGE");
_Static_assert(CHIMI_BINARY_RANGE_INCLUSIVE == 11, "layout: CHIMI_BINARY_RANGE_INCLUSIVE");
_Static_assert(CHIMI_PATTERN_WILDCARD == 0, "layout: CHIMI_PATTERN_WILDCARD");
_Static_assert(CHIMI_PATTERN_BINDING == 1, "layout: CHIMI_PATTERN_BINDING");
_Static_assert(CHIMI_PATTERN_LITERAL == 2, "layout: CHIMI_PATTERN_LITERAL");
_Static_assert(CHIMI_PATTERN_CONSTRUCTOR == 3, "layout: CHIMI_PATTERN_CONSTRUCTOR");
_Static_assert(CHIMI_PATTERN_OUTCOME == 4, "layout: CHIMI_PATTERN_OUTCOME");
_Static_assert(CHIMI_PATTERN_LIST == 5, "layout: CHIMI_PATTERN_LIST");
_Static_assert(CHIMI_PATTERN_ALIAS == 6, "layout: CHIMI_PATTERN_ALIAS");
_Static_assert(CHIMI_PATTERN_TUPLE == 7, "layout: CHIMI_PATTERN_TUPLE");
_Static_assert(CHIMI_TYPE_CONSOLE == 0, "layout: CHIMI_TYPE_CONSOLE");
_Static_assert(CHIMI_TYPE_FILE_SYSTEM == 3, "layout: CHIMI_TYPE_FILE_SYSTEM");
_Static_assert(CHIMI_TYPE_CLOCK == 37, "layout: CHIMI_TYPE_CLOCK");
_Static_assert(CHIMI_TYPE_RANDOM == 38, "layout: CHIMI_TYPE_RANDOM");
_Static_assert(HOST_PROCESS_UTF8_LINES == 0, "layout: HOST_PROCESS_UTF8_LINES");
_Static_assert(HOST_WRITE_LINE == 1, "layout: HOST_WRITE_LINE");
_Static_assert(HOST_WRITE_ERROR == 2, "layout: HOST_WRITE_ERROR");
_Static_assert(HOST_RANDOM_NEXT_INT == 3, "layout: HOST_RANDOM_NEXT_INT");
_Static_assert(HOST_CLOCK_NOW == 4, "layout: HOST_CLOCK_NOW");
_Static_assert(HOST_CLOCK_TIMESTAMP == 5, "layout: HOST_CLOCK_TIMESTAMP");
_Static_assert(HOST_CLOCK_SLEEP_UNTIL == 6, "layout: HOST_CLOCK_SLEEP_UNTIL");
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
_Static_assert(CHIMI_CONSTRUCTOR_CHANNEL_SEND_CLOSED == 17, "layout: CHIMI_CONSTRUCTOR_CHANNEL_SEND_CLOSED");
_Static_assert(CHIMI_CONSTRUCTOR_CHANNEL_SEND_ABORTED == 18, "layout: CHIMI_CONSTRUCTOR_CHANNEL_SEND_ABORTED");
_Static_assert(CHIMI_CONSTRUCTOR_CHANNEL_RECEIVED == 19, "layout: CHIMI_CONSTRUCTOR_CHANNEL_RECEIVED");
_Static_assert(CHIMI_CONSTRUCTOR_CHANNEL_DRAINED == 20, "layout: CHIMI_CONSTRUCTOR_CHANNEL_DRAINED");
_Static_assert(CHIMI_CONSTRUCTOR_CHANNEL_ABORTED == 21, "layout: CHIMI_CONSTRUCTOR_CHANNEL_ABORTED");
_Static_assert(CHIMI_CONSTRUCTOR_INT_PARSE_EMPTY == 22, "layout: CHIMI_CONSTRUCTOR_INT_PARSE_EMPTY");
_Static_assert(CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT == 23, "layout: CHIMI_CONSTRUCTOR_INT_PARSE_INVALID_DIGIT");
_Static_assert(CHIMI_CONSTRUCTOR_INT_PARSE_OUT_OF_RANGE == 24, "layout: CHIMI_CONSTRUCTOR_INT_PARSE_OUT_OF_RANGE");
_Static_assert(CHIMI_CONSTRUCTOR_FLOAT_PARSE_EMPTY == 25, "layout: CHIMI_CONSTRUCTOR_FLOAT_PARSE_EMPTY");
_Static_assert(CHIMI_CONSTRUCTOR_FLOAT_PARSE_INVALID == 26, "layout: CHIMI_CONSTRUCTOR_FLOAT_PARSE_INVALID");
_Static_assert(CHIMI_CONSTRUCTOR_FLOAT_PARSE_OUT_OF_RANGE == 27, "layout: CHIMI_CONSTRUCTOR_FLOAT_PARSE_OUT_OF_RANGE");
_Static_assert(CHIMI_CONSTRUCTOR_FLOAT_NOT_INTEGER == 28, "layout: CHIMI_CONSTRUCTOR_FLOAT_NOT_INTEGER");
_Static_assert(CHIMI_CONSTRUCTOR_FLOAT_OUT_OF_RANGE == 29, "layout: CHIMI_CONSTRUCTOR_FLOAT_OUT_OF_RANGE");
_Static_assert(CHIMI_CONSTRUCTOR_OPTION_NONE == 7, "layout: CHIMI_CONSTRUCTOR_OPTION_NONE");
_Static_assert(CHIMI_CONSTRUCTOR_RESULT_OK == 49, "layout: CHIMI_CONSTRUCTOR_RESULT_OK");
_Static_assert(CHIMI_CONSTRUCTOR_RESULT_ERR == 50, "layout: CHIMI_CONSTRUCTOR_RESULT_ERR");
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
_Static_assert(CHIMI_INTRINSIC_EMBED_BYTES == 168, "layout: CHIMI_INTRINSIC_EMBED_BYTES");
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
_Static_assert(CHIMI_INTRINSIC_TASK_START == 8, "layout: CHIMI_INTRINSIC_TASK_START");
_Static_assert(CHIMI_INTRINSIC_TASK_RESULT == 9, "layout: CHIMI_INTRINSIC_TASK_RESULT");
_Static_assert(CHIMI_INTRINSIC_TASK_ALL == 73, "layout: CHIMI_INTRINSIC_TASK_ALL");
_Static_assert(CHIMI_INTRINSIC_TASK_RESULTS == 74, "layout: CHIMI_INTRINSIC_TASK_RESULTS");
_Static_assert(CHIMI_INTRINSIC_TASK_RACE == 75, "layout: CHIMI_INTRINSIC_TASK_RACE");
_Static_assert(CHIMI_INTRINSIC_TASK_CANCEL == 10, "layout: CHIMI_INTRINSIC_TASK_CANCEL");
_Static_assert(CHIMI_INTRINSIC_CHANNEL_SEND == 11, "layout: CHIMI_INTRINSIC_CHANNEL_SEND");
_Static_assert(CHIMI_INTRINSIC_CHANNEL_RECEIVE == 12, "layout: CHIMI_INTRINSIC_CHANNEL_RECEIVE");
_Static_assert(CHIMI_INTRINSIC_CHANNEL_CLOSE == 13, "layout: CHIMI_INTRINSIC_CHANNEL_CLOSE");
_Static_assert(CHIMI_INTRINSIC_CHANNEL_ABORT == 14, "layout: CHIMI_INTRINSIC_CHANNEL_ABORT");
_Static_assert(CHIMI_INTRINSIC_CHANNEL_BOUNDED == 15, "layout: CHIMI_INTRINSIC_CHANNEL_BOUNDED");
_Static_assert(CHIMI_INTRINSIC_CHANNEL_ADD_RECEIVER == 82, "layout: CHIMI_INTRINSIC_CHANNEL_ADD_RECEIVER");
_Static_assert(CHIMI_INTRINSIC_BROADCAST_BOUNDED == 83, "layout: CHIMI_INTRINSIC_BROADCAST_BOUNDED");
_Static_assert(CHIMI_INTRINSIC_BROADCAST_PUBLISH == 84, "layout: CHIMI_INTRINSIC_BROADCAST_PUBLISH");
_Static_assert(CHIMI_INTRINSIC_BROADCAST_SUBSCRIBE == 85, "layout: CHIMI_INTRINSIC_BROADCAST_SUBSCRIBE");
_Static_assert(CHIMI_INTRINSIC_WATCH_STARTING_WITH == 86, "layout: CHIMI_INTRINSIC_WATCH_STARTING_WITH");
_Static_assert(CHIMI_INTRINSIC_WATCH_PUBLISH == 87, "layout: CHIMI_INTRINSIC_WATCH_PUBLISH");
_Static_assert(CHIMI_INTRINSIC_WATCH_SUBSCRIBE == 88, "layout: CHIMI_INTRINSIC_WATCH_SUBSCRIBE");
_Static_assert(CHIMI_INTRINSIC_WATCH_RECEIVE == 89, "layout: CHIMI_INTRINSIC_WATCH_RECEIVE");
_Static_assert(CHIMI_INTRINSIC_CAUSE_CONTEXT == 169, "layout: CHIMI_INTRINSIC_CAUSE_CONTEXT");
_Static_assert(CHIMI_INTRINSIC_CAUSE_OF == 170, "layout: CHIMI_INTRINSIC_CAUSE_OF");
_Static_assert(CHIMI_INTRINSIC_CAUSE_KIND_CODE == 171, "layout: CHIMI_INTRINSIC_CAUSE_KIND_CODE");
_Static_assert(CHIMI_INTRINSIC_CAUSE_SUMMARY == 172, "layout: CHIMI_INTRINSIC_CAUSE_SUMMARY");
_Static_assert(CHIMI_INTRINSIC_CAUSE_FRAMES == 173, "layout: CHIMI_INTRINSIC_CAUSE_FRAMES");
_Static_assert(CHIMI_INTRINSIC_CAUSE_CAUSED_BY == 174, "layout: CHIMI_INTRINSIC_CAUSE_CAUSED_BY");
_Static_assert(CHIMI_INTRINSIC_CAUSE_SUPPRESSED == 175, "layout: CHIMI_INTRINSIC_CAUSE_SUPPRESSED");
_Static_assert(CHIMI_INTRINSIC_CAUSE_REASON_CODE == 176, "layout: CHIMI_INTRINSIC_CAUSE_REASON_CODE");
_Static_assert(CHIMI_INTRINSIC_CAUSE_TO_TEXT == 177, "layout: CHIMI_INTRINSIC_CAUSE_TO_TEXT");
_Static_assert(CHIMI_INTRINSIC_RESOURCE_SCOPED == 182, "layout: CHIMI_INTRINSIC_RESOURCE_SCOPED");
_Static_assert(CHIMI_INTRINSIC_TEST_CAPTURE_DEFECT == 183, "layout: CHIMI_INTRINSIC_TEST_CAPTURE_DEFECT");
_Static_assert(CHIMI_INTRINSIC_TEST_ASSERT_EQUAL == 178, "layout: CHIMI_INTRINSIC_TEST_ASSERT_EQUAL");
_Static_assert(CHIMI_INTRINSIC_TEST_ASSERT_NOT_EQUAL == 179, "layout: CHIMI_INTRINSIC_TEST_ASSERT_NOT_EQUAL");
_Static_assert(CHIMI_INTRINSIC_TEST_ASSERT == 180, "layout: CHIMI_INTRINSIC_TEST_ASSERT");
_Static_assert(CHIMI_INTRINSIC_TEST_FAIL == 181, "layout: CHIMI_INTRINSIC_TEST_FAIL");
_Static_assert(CHIMI_INTRINSIC_TEST_RECORDER == 184, "layout: CHIMI_INTRINSIC_TEST_RECORDER");
_Static_assert(CHIMI_INTRINSIC_TEST_RECORD == 185, "layout: CHIMI_INTRINSIC_TEST_RECORD");
_Static_assert(CHIMI_INTRINSIC_TEST_RECORDED == 186, "layout: CHIMI_INTRINSIC_TEST_RECORDED");
_Static_assert(CHIMI_INTRINSIC_TEST_SCRIPT == 187, "layout: CHIMI_INTRINSIC_TEST_SCRIPT");
_Static_assert(CHIMI_INTRINSIC_TEST_NEXT == 188, "layout: CHIMI_INTRINSIC_TEST_NEXT");
_Static_assert(CHIMI_INTRINSIC_TEST_MAILBOX == 189, "layout: CHIMI_INTRINSIC_TEST_MAILBOX");
_Static_assert(CHIMI_INTRINSIC_TEST_RECEIVED == 190, "layout: CHIMI_INTRINSIC_TEST_RECEIVED");
_Static_assert(CHIMI_INTRINSIC_IO_PATH == 0, "layout: CHIMI_INTRINSIC_IO_PATH");
_Static_assert(CHIMI_INTRINSIC_LIST_LENGTH == 2, "layout: CHIMI_INTRINSIC_LIST_LENGTH");
_Static_assert(CHIMI_INTRINSIC_TEXT_APPEND == 6, "layout: CHIMI_INTRINSIC_TEXT_APPEND");
_Static_assert(CHIMI_INTRINSIC_TEXT_EQUAL == 7, "layout: CHIMI_INTRINSIC_TEXT_EQUAL");
_Static_assert(CHIMI_INTRINSIC_INT_PARSE == 16, "layout: CHIMI_INTRINSIC_INT_PARSE");
_Static_assert(CHIMI_INTRINSIC_INT_TO_TEXT == 17, "layout: CHIMI_INTRINSIC_INT_TO_TEXT");
_Static_assert(CHIMI_INTRINSIC_INT_ADD_CHECKED == 18, "layout: CHIMI_INTRINSIC_INT_ADD_CHECKED");
_Static_assert(CHIMI_INTRINSIC_INT_SUBTRACT_CHECKED == 19, "layout: CHIMI_INTRINSIC_INT_SUBTRACT_CHECKED");
_Static_assert(CHIMI_INTRINSIC_INT_MULTIPLY_CHECKED == 20, "layout: CHIMI_INTRINSIC_INT_MULTIPLY_CHECKED");
_Static_assert(CHIMI_INTRINSIC_INT_DIVIDE_CHECKED == 21, "layout: CHIMI_INTRINSIC_INT_DIVIDE_CHECKED");
_Static_assert(CHIMI_INTRINSIC_INT_REMAINDER_CHECKED == 22, "layout: CHIMI_INTRINSIC_INT_REMAINDER_CHECKED");
_Static_assert(CHIMI_INTRINSIC_INT_MODULO == 23, "layout: CHIMI_INTRINSIC_INT_MODULO");
_Static_assert(CHIMI_INTRINSIC_FLOAT_FROM_INT == 24, "layout: CHIMI_INTRINSIC_FLOAT_FROM_INT");
_Static_assert(CHIMI_INTRINSIC_FLOAT_TRUNCATE == 25, "layout: CHIMI_INTRINSIC_FLOAT_TRUNCATE");
_Static_assert(CHIMI_INTRINSIC_FLOAT_FLOOR == 26, "layout: CHIMI_INTRINSIC_FLOAT_FLOOR");
_Static_assert(CHIMI_INTRINSIC_FLOAT_CEILING == 27, "layout: CHIMI_INTRINSIC_FLOAT_CEILING");
_Static_assert(CHIMI_INTRINSIC_FLOAT_ROUND == 28, "layout: CHIMI_INTRINSIC_FLOAT_ROUND");
_Static_assert(CHIMI_INTRINSIC_FLOAT_TO_INT == 29, "layout: CHIMI_INTRINSIC_FLOAT_TO_INT");
_Static_assert(CHIMI_INTRINSIC_FLOAT_TO_TEXT == 30, "layout: CHIMI_INTRINSIC_FLOAT_TO_TEXT");
_Static_assert(CHIMI_INTRINSIC_FLOAT_PARSE == 31, "layout: CHIMI_INTRINSIC_FLOAT_PARSE");
_Static_assert(CHIMI_INTRINSIC_DEBUG_RENDER == 32, "layout: CHIMI_INTRINSIC_DEBUG_RENDER");
_Static_assert(CHIMI_INTRINSIC_DEBUG_RENDER_INDENTED == 33, "layout: CHIMI_INTRINSIC_DEBUG_RENDER_INDENTED");
_Static_assert(CHIMI_INTRINSIC_RANGE_START == 37, "layout: CHIMI_INTRINSIC_RANGE_START");
_Static_assert(CHIMI_INTRINSIC_RANGE_END == 38, "layout: CHIMI_INTRINSIC_RANGE_END");
_Static_assert(CHIMI_INTRINSIC_RANGE_LENGTH == 39, "layout: CHIMI_INTRINSIC_RANGE_LENGTH");
_Static_assert(CHIMI_INTRINSIC_RANGE_CONTAINS == 40, "layout: CHIMI_INTRINSIC_RANGE_CONTAINS");
_Static_assert(CHIMI_INTRINSIC_PATH_FROM == 192, "layout: CHIMI_INTRINSIC_PATH_FROM");
_Static_assert(CHIMI_INTRINSIC_PATH_TO_TEXT == 193, "layout: CHIMI_INTRINSIC_PATH_TO_TEXT");
_Static_assert(CHIMI_INTRINSIC_ENV_HOST_VARIABLES == 194, "layout: CHIMI_INTRINSIC_ENV_HOST_VARIABLES");
_Static_assert(CHIMI_INTRINSIC_ENV_HOST_CURRENT_DIRECTORY == 195, "layout: CHIMI_INTRINSIC_ENV_HOST_CURRENT_DIRECTORY");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_OPEN == 196, "layout: CHIMI_INTRINSIC_FS_HOST_OPEN");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_CREATE == 197, "layout: CHIMI_INTRINSIC_FS_HOST_CREATE");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_READ == 198, "layout: CHIMI_INTRINSIC_FS_HOST_READ");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_WRITE == 199, "layout: CHIMI_INTRINSIC_FS_HOST_WRITE");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_CLOSE == 200, "layout: CHIMI_INTRINSIC_FS_HOST_CLOSE");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_METADATA == 201, "layout: CHIMI_INTRINSIC_FS_HOST_METADATA");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_LIST == 202, "layout: CHIMI_INTRINSIC_FS_HOST_LIST");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_CREATE_DIRECTORY == 203, "layout: CHIMI_INTRINSIC_FS_HOST_CREATE_DIRECTORY");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_REMOVE == 204, "layout: CHIMI_INTRINSIC_FS_HOST_REMOVE");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_RENAME == 205, "layout: CHIMI_INTRINSIC_FS_HOST_RENAME");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_IS_DIRECTORY == 206, "layout: CHIMI_INTRINSIC_FS_HOST_IS_DIRECTORY");
_Static_assert(CHIMI_INTRINSIC_FS_HOST_TEMPORARY_DIRECTORY == 207, "layout: CHIMI_INTRINSIC_FS_HOST_TEMPORARY_DIRECTORY");
_Static_assert(CHIMI_INTRINSIC_FS_TEST_ROOT == 208, "layout: CHIMI_INTRINSIC_FS_TEST_ROOT");
_Static_assert(CHIMI_ADDRESS_TEST == 3, "layout: CHIMI_ADDRESS_TEST");
_Static_assert(CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY == 33, "layout: CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY");
_Static_assert(CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID == 34, "layout: CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID");
_Static_assert(CHIMI_CONSTRUCTOR_PROCESS_SEND_FULL == 36, "layout: CHIMI_CONSTRUCTOR_PROCESS_SEND_FULL");
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
static ChimiEnvironment *capture_requirements(const ChimiEnvironment *source) {
  return chimi_environment_capture_requirements_v1(source);
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

/* The Unix time at the execution clock's origin: `--clock-start` under the
   virtual clock, and under the real clock the host's wall time when the
   computation starts, just before the scheduler takes its origin. */
static int64_t start_unix_nanos(const ChimiRuntime *runtime) {
  if (runtime->virtual_clock) return runtime->clock_start;
  struct timespec host;
  clock_gettime(CLOCK_REALTIME, &host);
  return (int64_t)host.tv_sec * INT64_C(1000000000) + host.tv_nsec;
}

/* The telemetry events are Rust (chimi-runtime telemetry.rs), written under
   its own lock, which is taken after the kernel's. */
extern void chimi_trace_event_v1(ChimiRuntime *runtime, const char *event, uint32_t node_id, uint64_t span,
                                 int64_t parent, const char *operation, const ChimiControl *outcome);
extern void chimi_trace_dependency_v1(ChimiRuntime *runtime, uint32_t node_id, uint64_t span, uint64_t parent,
                                      const ChimiCallable *callable);
extern void chimi_trace_process_body_v1(ChimiRuntime *runtime, uint64_t span, uint64_t incarnation,
                                        const ChimiControl *outcome);
extern void chimi_trace_scheduler_event_v1(void *context, const ChimiSchedulerTraceEvent *event);
#define emit_event chimi_trace_event_v1
#define emit_process_body chimi_trace_process_body_v1

/* The evaluator is Rust (chimi-runtime eval.rs): the core nodes, `invoke`
   for the calls a program defines, and the span and profile frame of each
   node. It calls back into chimi_runtime_invoke_native_v1 below for the
   intrinsics, host functions and C bindings. */
ChimiControl *chimi_runtime_invoke_v1(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                                      uint32_t argument_count, uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *call_environment);
#define invoke chimi_runtime_invoke_v1
ChimiControl *chimi_runtime_construct_value_v1(uint32_t symbol, ChimiValue *const *values);
#define construct_value chimi_runtime_construct_value_v1
ChimiValue *chimi_runtime_select_value_v1(const ChimiValue *value, const char *name);
#define select_value chimi_runtime_select_value_v1
const ChimiFunction *chimi_runtime_find_function_v1(uint32_t symbol);
#define find_function chimi_runtime_find_function_v1
void chimi_runtime_bind_requirement_v1(ChimiEnvironment *environment, const char *name, ChimiValue *value);
uint64_t chimi_runtime_take_spans_v1(ChimiRuntime *runtime, uint64_t count);
#define take_spans chimi_runtime_take_spans_v1
uint64_t chimi_runtime_node_begin_v1(ChimiRuntime *runtime, uint32_t node, uint64_t parent,
                                     const char *operation, uint32_t *outer);
ChimiControl *chimi_runtime_node_finish_v1(ChimiRuntime *runtime, uint32_t node, uint64_t parent, uint64_t span,
                                           const char *operation, uint32_t outer, ChimiControl *control);

static ChimiValue *integer_value(int64_t raw) { return chimi_value_integer_v1(VALUE_INTEGER, raw); }
static ChimiValue *range_value(int64_t start, int64_t end) { return chimi_value_range_v1(start, end); }
/* Void, True and False are each one shared value in the static area, because values
   never change after construction and conditions and effects produce them on almost
   every pass. Static values are never collected, copied or freed. decode_constants
   creates all three at startup, before any carrier runs. While carriers run on
   threads, nothing appends to the static area: list_slice copies a constant's
   window rather than compacting it there. */
#define shared_values chimi_runtime_shared_values
ChimiValue *shared_values[3];
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
/* `bytes` may be NULL when `length` is 0. */
static ChimiValue *text_from_bytes(const char *bytes, size_t length) { return chimi_value_text_v1(bytes, length); }
static char hex_digit(unsigned value) { return "0123456789abcdef"[value & 0xf]; }
/* Text from a C string, which cannot hold U+0000: a path, a message, a number. */
static ChimiValue *text_value(const char *raw) { return text_from_bytes(raw, strlen(raw)); }
/* A Text literal or constant from the string table, which may hold U+0000. */
static ChimiValue *string_text(uint32_t id) { return text_from_bytes(CHIMI_STRINGS[id], CHIMI_STRING_LENGTHS[id]); }
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
/* `Decimal.to_text`: exactly `scale` fraction digits. `output` holds DECIMAL_TEXT_BYTES. */
static void decimal_text(Decimal value, char *output) { chimi_decimal_text_v1(value, false, output); }
/* The debug form: trailing fraction zeros removed, so equal values render the same. */
static void decimal_normalized_text(Decimal value, char *output) { chimi_decimal_text_v1(value, true, output); }

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

/* scheduler_request for the Rust intrinsics (chimi-runtime scheduler.rs). */
uint32_t chimi_runtime_scheduler_request_v1(ChimiSchedulerRequest *request);
uint32_t chimi_runtime_scheduler_request_v1(ChimiSchedulerRequest *request) {
  return scheduler_request(request);
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

/* For the Rust channel intrinsics (chimi-runtime intrinsics/channels.rs): a
   Receiver, Completion or Sender value, which counts as a live reference to
   its channel; the Receivers and Completions a message carries, collected as
   for a send (free the list), with the number of managed handles met; and
   the control a task's result word names. */
ChimiValue *chimi_runtime_capability_value_v1(uint32_t kind, uint64_t channel, uint64_t owner, uint64_t token);
ChimiCarriedCapability *chimi_runtime_carried_capabilities_v1(ChimiValue *root, size_t *count, size_t *managed);
ChimiControl *chimi_runtime_task_reply_v1(int64_t raw);
ChimiControl *chimi_runtime_channel_operation_v1(uint32_t intrinsic, ChimiValue **arguments,
                                                 uint32_t argument_count, uint32_t call_node);
ChimiControl *chimi_runtime_task_operation_v1(ChimiRuntime *runtime, uint32_t intrinsic, ChimiValue **arguments,
                                              uint32_t argument_count, uint64_t span, uint32_t call_node,
                                              ChimiEnvironment *environment);
ChimiValue *chimi_runtime_capability_value_v1(uint32_t kind, uint64_t channel, uint64_t owner, uint64_t token) {
  return channel_capability_value((ValueKind)kind, channel, owner, token);
}
ChimiCarriedCapability *chimi_runtime_carried_capabilities_v1(ChimiValue *root, size_t *count, size_t *managed) {
  CapabilityTransfer carried = {.collect = true};
  transfer_captures(&carried, root);
  *count = carried.carried_count;
  *managed = carried.managed_count;
  return carried.carried;
}

/* For Task.start in Rust (intrinsics/tasks.rs): a Task handle value, counted
   as a live reference; and the transfer of the exclusive capabilities under
   `root` to the child `child`, false (with `*cancelled` when this task was
   cancelled meanwhile) when one cannot move. */
ChimiValue *chimi_runtime_task_value_v1(uint64_t raw);
bool chimi_runtime_transfer_to_child_v1(ChimiValue *root, uint64_t child, bool *cancelled);
ChimiValue *chimi_runtime_task_value_v1(uint64_t raw) { return task_value(raw); }
bool chimi_runtime_transfer_to_child_v1(ChimiValue *root, uint64_t child, bool *cancelled) {
  CapabilityTransfer transfer = {.child = child};
  bool transferred = transfer_captures(&transfer, root);
  *cancelled = transfer.cancelled;
  return transferred;
}

/* Whether `root` reaches a Receiver or Completion: a std/test Mailbox refuses a
   message with one (intrinsics/test_log.rs), since only a process receive makes
   the capability transfer it needs. Collecting marks the copies' tokens in flight,
   so `root` must be a detached copy the caller frees. */
bool chimi_runtime_carries_capability_v1(ChimiValue *root) {
  CapabilityTransfer carried = {.collect = true};
  transfer_captures(&carried, root);
  size_t carried_count = carried.carried_count;
  free(carried.carried);
  return carried_count != 0;
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

/* std/test. */
static ChimiValue *debug_render(const ChimiValue *value, bool indented);

/* Test.assert_equal, assert_not_equal, assert and fail, in intrinsics/assertions.rs. */
ChimiControl *chimi_intrinsic_assertion_v1(uint32_t intrinsic, ChimiValue **arguments, uint32_t argument_count,
                                           uint32_t call_node);

/* A std/test Recorder, Script or Mailbox, in intrinsics/test_log.rs: the logs live
   there, outside every heap, behind their own lock, which is taken before the
   handle counts' lock. */
ChimiControl *chimi_intrinsic_test_log_v1(uint32_t intrinsic, ChimiValue **arguments, uint32_t argument_count,
                                          uint32_t call_node);
/* Process.send to a test address, in intrinsics/test_log.rs. */
ChimiControl *chimi_runtime_send_to_test_mailbox_v1(const ChimiValue *address, ChimiValue *message,
                                                    uint32_t call_node);

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

/* Ends a reclaiming scope (eval.rs end_measured_scope): keeps only what
   `control` reaches and returns it, or a defect when the scope reached into
   another heap. */
ChimiControl *chimi_runtime_end_measured_scope_v1(AllocationScope *scope, ChimiControl *control);
static ChimiControl *end_measured_allocation_scope(ChimiRuntime *runtime, AllocationScope *scope,
                                                   ChimiControl *control) {
  (void)runtime;
  return chimi_runtime_end_measured_scope_v1(scope, control);
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
   one result as ChimiBindingResult, which is ChimiControl. The call
   (struct ChimiBindingCall, above the layout asserts) is mirrored in Rust,
   which reads a call's arguments and builds its values and results
   (intrinsics/binding_values.rs); handles, waits and cancellation checks stay
   here with the handle lock and the scheduler requests they make. */
static ChimiBindingValue *binding_value(ChimiValue *value) { return (ChimiBindingValue *)value; }

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

static ChimiControl *invoke_clock_host(ChimiRuntime *runtime, ChimiCallable *callable,
                                       ChimiValue **arguments, uint32_t argument_count,
                                       uint64_t span, uint32_t call_node,
                                       ChimiEnvironment *environment);

/* The calls the evaluator hands back to C: an intrinsic, a host function, or a
   C binding. */
ChimiControl *chimi_runtime_invoke_native_v1(ChimiRuntime *runtime, ChimiCallable *callable,
                                             ChimiValue **arguments, uint32_t argument_count, uint64_t span,
                                             uint32_t call_node, ChimiEnvironment *call_environment);
ChimiControl *chimi_runtime_invoke_native_v1(ChimiRuntime *runtime, ChimiCallable *callable,
                                             ChimiValue **arguments, uint32_t argument_count, uint64_t span,
                                             uint32_t call_node, ChimiEnvironment *call_environment) {
  if (callable->kind == CALL_INTRINSIC && callable->intrinsic == CHIMI_INTRINSIC_BINDING_CALL)
    return invoke_binding(runtime, CHIMI_SYMBOLS[callable->symbol].binding, arguments,
                          argument_count, call_node);
  if (callable->kind == CALL_INTRINSIC)
    return invoke_intrinsic(runtime, callable->intrinsic, arguments, argument_count,
                            span, call_node, call_environment);
  if (callable->kind == CALL_HOST && callable->host >= HOST_CLOCK_NOW)
    return invoke_clock_host(runtime, callable, arguments, argument_count, span, call_node,
                             call_environment);
  return invoke_host(runtime, callable->host, arguments, argument_count,
                     span, call_node, call_environment);
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

/* The fiber entry of a task Task.start made (chimi-runtime intrinsics/tasks.rs
   passes it with the context it allocated). */
ChimiFiberResult chimi_runtime_task_entry_v1(void *raw_context);
ChimiFiberResult chimi_runtime_task_entry_v1(void *raw_context) {
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
ChimiControl *chimi_runtime_task_reply_v1(int64_t raw) { return task_reply(raw); }

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

/* Failure causes (time-and-outcomes.md, "Failure context") are built and read in
   intrinsics/causes.rs; the interpreter's execution/causes.rs is the reference.
   A live outcome holds its cause in `ChimiControl.cause`; NULL means the fresh
   cause of the outcome itself, built only when read. */
ChimiValue *chimi_runtime_control_cause_v1(const ChimiControl *control);
ChimiControl *chimi_runtime_control_with_cause_v1(const ChimiControl *control, ChimiValue *cause);
ChimiValue *chimi_runtime_cause_with_sibling_v1(const ChimiValue *cause, const ChimiValue *sibling);
ChimiValue *chimi_runtime_cause_derived_v1(ChimiValue *matched, bool keep, ChimiValue *error, int64_t node);
ChimiValue *chimi_runtime_with_stored_cause_v1(const ChimiValue *value, ChimiValue *cause);
ChimiValue *chimi_runtime_matched_cause_value_v1(const ChimiControl *subject);
ChimiValue *chimi_runtime_cause_text_v1(const ChimiValue *cause);
bool chimi_runtime_cause_has_detail_v1(const ChimiValue *cause);

/* A cancellation's reason in an outcome record; a cancellation's cause
   (intrinsics/causes.rs) carries the same text. */
static const char CANCELLATION_REASON[] = "requested before the next semantic operation";

/* The source location of node `node`, for a rendered cause (intrinsics/causes.rs). */
const char *chimi_runtime_node_location_v1(int64_t node);
const char *chimi_runtime_node_location_v1(int64_t node) {
  return CHIMI_STRINGS[CHIMI_ORIGINS[CHIMI_NODES[node].origin].location];
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
        return chimi_runtime_send_to_test_mailbox_v1(arguments[0], arguments[1], call_node);
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
/* The shared Void, False and True, for the code in Rust. */
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
ChimiValue *chimi_runtime_execution_clock_value_v1(void);
ChimiValue *chimi_runtime_execution_clock_value_v1(void) { return execution_clock_value(); }

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

/* The host cores of std/env and std/fs, and the open-file table, are Rust
   (chimi-runtime intrinsics/host.rs). A read or write that would block parks
   here until dispatch moves. */
extern ChimiControl *chimi_intrinsic_scalar_v1(uint32_t intrinsic, ChimiValue **arguments, uint32_t argument_count,
                                               uint32_t call_node);
extern ChimiControl *chimi_intrinsic_host_v1(ChimiRuntime *runtime, bool cancel, uint32_t intrinsic,
                                             ChimiValue **arguments, uint32_t argument_count, uint32_t call_node);

/* Waits for `fd` to be readable, or writable; false when cancelled. */
bool chimi_runtime_fs_wait_v1(ChimiRuntime *runtime, int fd, bool writable);
bool chimi_runtime_fs_wait_v1(ChimiRuntime *runtime, int fd, bool writable) {
  int64_t interest = writable ? CHIMI_IO_WRITABLE : CHIMI_IO_READABLE;
  if (CHIMI_REQUIRES_SCHEDULER) {
    ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_IO_WAIT, .value = fd, .token = interest};
    return scheduler_request(&request) != CHIMI_FIBER_CANCEL;
  }
  struct pollfd poll_fd = {.fd = fd, .events = writable ? POLLOUT : POLLIN};
  (void)poll(&poll_fd, 1, -1);
  return !runtime->cancel;
}

static ChimiControl *invoke_signal_operation(uint32_t intrinsic, ChimiValue **arguments,
                                             uint32_t argument_count, uint32_t call_node);

/* The intrinsics runtime.c still serves; the evaluator calls the Rust families
   directly (chimi-runtime intrinsics/dispatch.rs). */
static ChimiControl *invoke_intrinsic(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  if (intrinsic == CHIMI_INTRINSIC_SIGNAL_HOST_INSTALL || intrinsic == CHIMI_INTRINSIC_SIGNAL_HOST_WAIT)
    return invoke_signal_operation(intrinsic, arguments, argument_count, call_node);
  if (intrinsic == CHIMI_INTRINSIC_LOG_HOST_MINIMUM_LEVEL || intrinsic == CHIMI_INTRINSIC_LOG_HOST_WRITE)
    return invoke_log_operation(runtime, intrinsic, arguments, argument_count, call_node);
  if (intrinsic == CHIMI_INTRINSIC_METRICS_HOST_ENABLED || intrinsic == CHIMI_INTRINSIC_METRICS_HOST_RECORD ||
      intrinsic == CHIMI_INTRINSIC_METRICS_HOST_DOCUMENT)
    return invoke_metrics_operation(runtime, intrinsic, arguments, argument_count, call_node);
  if (intrinsic >= CHIMI_INTRINSIC_PROCESS_SPEC && intrinsic <= CHIMI_INTRINSIC_SUPERVISOR_WAIT)
    return invoke_process(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if ((intrinsic >= CHIMI_INTRINSIC_SELECT_RECEIVE && intrinsic <= CHIMI_INTRINSIC_SELECT_FIRST) ||
      intrinsic == CHIMI_INTRINSIC_SELECT_WATCH)
    return invoke_select(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_CLOCK_NOW && intrinsic <= CHIMI_INTRINSIC_TASK_DEADLINE)
    return invoke_clock(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if ((intrinsic >= CHIMI_INTRINSIC_TASK_RESULT && intrinsic <= CHIMI_INTRINSIC_CHANNEL_BOUNDED) ||
      (intrinsic >= CHIMI_INTRINSIC_CHANNEL_ADD_RECEIVER && intrinsic <= CHIMI_INTRINSIC_WATCH_RECEIVE))
    return chimi_runtime_channel_operation_v1(intrinsic, arguments, argument_count, call_node);
  if (intrinsic == CHIMI_INTRINSIC_TASK_START ||
      (intrinsic >= CHIMI_INTRINSIC_TASK_ALL && intrinsic <= CHIMI_INTRINSIC_TASK_RACE))
    return chimi_runtime_task_operation_v1(runtime, intrinsic, arguments, argument_count, span, call_node,
                                           environment);
  switch (intrinsic) {
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
  }
  return defect("unknown standard intrinsic", call_node);
}

/* Module constants, decoded from CHIMI_CONSTANT_VALUES before the entry runs. They are
   allocated in the static area, so no collection frees them, and values are
   immutable, so every task shares them. Decoding cannot fail and emits no events. */
#define constant_values chimi_runtime_constant_values
ChimiValue **constant_values;

/* Binds each (requirement name, value id) pair to every requirement of that name, as a
   provision does. */
static void bind_constant_requirements(ChimiEnvironment *environment, const uint32_t *pairs,
                                       int64_t count) {
  for (int64_t i = 0; i < count; ++i)
    chimi_runtime_bind_requirement_v1(environment, CHIMI_STRINGS[pairs[2 * i]], constant_values[pairs[2 * i + 1]]);
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
  ChimiValue *cause = control->kind == CONTROL_SUCCESS ? NULL : chimi_runtime_control_cause_v1(control);
  if (cause && chimi_runtime_cause_has_detail_v1(cause)) {
    fputs(",\"cause\":", output);
    ChimiValue *text = chimi_runtime_cause_text_v1(cause);
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
    .entry = chimi_runtime_task_entry_v1,
    .context = &context,
    /* Every task and incarnation gets a guarded 1 MiB stack. Pages are touched on
       use, so a parked fiber's resident cost follows its deepest call. */
    .stack_bytes = CHIMI_FIBER_STACK_BYTES,
    .trace_mode = runtime->silent ? 0 : (runtime->full_trace ? 2 : 1),
    .clock_mode = runtime->virtual_clock ? CHIMI_SCHEDULER_CLOCK_VIRTUAL : CHIMI_SCHEDULER_CLOCK_REAL,
    .trace_sink = chimi_trace_scheduler_event_v1,
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
