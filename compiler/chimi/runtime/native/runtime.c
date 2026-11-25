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
   headers and fragment records as well as payloads; static constants are excluded.
   Each carrier keeps its own copy, a signed delta: an allocation made on one
   carrier can be freed on another, so one carrier's count can fall below zero,
   and only the sum over carriers (heap_totals_sum) is the process's. A peak is
   recorded per carrier (footprint_record_peak), which is the process's with one
   carrier or in the simulation; with several carrier threads the report's
   peaks are the process-wide ones footprint_fold keeps. */
typedef struct {
  int64_t live_allocations;
  int64_t footprint_bytes;
  int64_t peak_footprint_bytes;
  int64_t live_fragments;
  int64_t fragment_footprint_bytes;
  int64_t peak_fragment_footprint_bytes;
  int64_t live_heaps;
  /* Freed small blocks kept for reuse (see small_block_take); not live. */
  int64_t cached_bytes;
} HeapTotals;

enum { REGION_FULL_MIN_GROWTH = 4 * 1024 };

static RuntimeHeap main_heap = {.owner = 1, .base = {.role = AREA_BASE, .heap = &main_heap}};
/* Heap owners are unique across the process; any carrier creates heaps. */
static _Atomic uint64_t next_heap_owner = 2;
static AllocationArea static_area = {.role = AREA_STATIC};
/* Epochs are unique across the process, not per carrier or per heap, so an
   allocation that moves between heaps or carriers can never carry a stale epoch
   that a later pass reuses (parallel-scheduler.md, "Runtime state by owner"). */
static _Atomic uint64_t next_mark_epoch = 1;
/* Epochs a carrier takes from next_mark_epoch at once. A loop pass collects,
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
static ProfileRow *profile_rows;

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

/* A growable stack of allocations, used by marking and copying so neither recurses:
   both run on 64 KiB fiber stacks. */
typedef struct { void **items; size_t length; size_t capacity; } PointerStack;

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
     (footprint_fold); used only with several carrier threads. */
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
  SmallBlock *small_blocks[SMALL_BLOCK_LIMIT / SMALL_BLOCK_GRAIN];
  uint32_t small_block_counts[SMALL_BLOCK_LIMIT / SMALL_BLOCK_GRAIN];
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
     next_mark_epoch a block at a time (take_mark_epoch). */
  uint64_t next_epoch;
  uint64_t end_epoch;
  /* This carrier's unused span ids, [next_span, end_span), taken from the
     runtime's counter a block at a time when several carriers run
     (take_spans). */
  uint64_t next_span;
  uint64_t end_span;
  PointerStack mark_stack;
  PointerStack copy_stack;
  /* Copies that copy_scan still has to redirect. Copying never yields, so one stack,
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
static size_t carrier_count = 2;
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
static intptr_t carrier_tls_offset;
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
static size_t carrier_count = 1;

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

/* A zeroed block of at least `bytes` bytes, to be returned with small_block_give. */
static void *small_block_take(size_t bytes) {
  Carrier *const here = carrier;
  size_t rounded = (bytes + SMALL_BLOCK_GRAIN - 1) / SMALL_BLOCK_GRAIN * SMALL_BLOCK_GRAIN;
  if (CHIMI_SMALL_BLOCK_CACHE && rounded <= SMALL_BLOCK_LIMIT) {
    size_t class = rounded / SMALL_BLOCK_GRAIN - 1;
    SmallBlock *block = here->small_blocks[class];
    if (block) {
      here->small_blocks[class] = block->next;
      --here->small_block_counts[class];
      here->heap_totals.cached_bytes -= rounded;
      memset(block, 0, rounded);
      return block;
    }
  }
  void *block = calloc(1, rounded);
  if (!block) exhausted_memory();
  return block;
}

static void small_block_give(void *pointer, size_t bytes) {
  Carrier *const here = carrier;
  size_t rounded = (bytes + SMALL_BLOCK_GRAIN - 1) / SMALL_BLOCK_GRAIN * SMALL_BLOCK_GRAIN;
  if (CHIMI_SMALL_BLOCK_CACHE && rounded <= SMALL_BLOCK_LIMIT) {
    size_t class = rounded / SMALL_BLOCK_GRAIN - 1;
    if (here->small_block_counts[class] < SMALL_BLOCK_CACHE) {
      SmallBlock *block = pointer;
      block->next = here->small_blocks[class];
      here->small_blocks[class] = block;
      ++here->small_block_counts[class];
      here->heap_totals.cached_bytes += rounded;
      return;
    }
  }
  free(pointer);
}

static AllocationArea *current_area(void) {
  Carrier *const here = carrier;
  if (here->fiber.target) return here->fiber.target;
  return here->fiber.heap->innermost ? &here->fiber.heap->innermost->young : &here->fiber.heap->base;
}


/* The footprints a peak check compares. With one carrier its own delta is the
   process's. The simulation's two carriers share one thread, so it compares
   their sum, as a single carrier would. With several carrier threads one
   carrier's delta is not the process's (an allocation made on one carrier
   can be freed on another, so one carrier's count drifts without bound), and
   the report's peaks come from footprint_fold instead. */
#ifdef CHIMI_CARRIER_SIMULATION
static int64_t footprint_now(void) {
  int64_t sum = 0;
  for (size_t index = 0; index < carrier_count; ++index)
    sum += carrier_at(index)->heap_totals.footprint_bytes + carrier_at(index)->pending_footprint_bytes;
  return sum;
}

static int64_t fragment_footprint_now(void) {
  int64_t sum = 0;
  for (size_t index = 0; index < carrier_count; ++index) sum += carrier_at(index)->heap_totals.fragment_footprint_bytes;
  return sum;
}
#else
static int64_t footprint_now(void) {
  Carrier *const here = carrier;
  return here->heap_totals.footprint_bytes + here->pending_footprint_bytes;
}

static int64_t fragment_footprint_now(void) { return carrier->heap_totals.fragment_footprint_bytes; }
#endif

/* Process-wide footprints and their peaks with several carrier threads
   (step 3 PR 6b). Each carrier folds its change into them once it has moved
   FOOTPRINT_FOLD_BYTES from what it last folded, so the counters are within
   carrier_count * FOOTPRINT_FOLD_SLACK of the true sum and a peak is the
   largest folded sum. Young allocations are checked once per chunk
   (young_bump), not per allocation, hence the chunk in the slack. One
   atomic add per 64 KiB of change, instead of one per allocation, keeps the
   carriers off one cache line (as take_mark_epoch). */
enum { FOOTPRINT_FOLD_BYTES = 64 * 1024, FOOTPRINT_FOLD_SLACK = FOOTPRINT_FOLD_BYTES + YOUNG_CHUNK_BYTES };
static _Atomic int64_t process_footprint_bytes;
static _Atomic int64_t process_peak_footprint_bytes;
static _Atomic int64_t process_fragment_footprint_bytes;
static _Atomic int64_t process_peak_fragment_footprint_bytes;

#ifndef CHIMI_CARRIER_SIMULATION
static void atomic_raise(_Atomic int64_t *peak, int64_t value) {
  int64_t high = atomic_load_explicit(peak, memory_order_relaxed);
  while (value > high &&
         !atomic_compare_exchange_weak_explicit(peak, &high, value, memory_order_relaxed, memory_order_relaxed)) {
  }
}
#endif

#ifndef CHIMI_CARRIER_SIMULATION
/* Out of line, so the allocator's inlined check stays one compare. */
__attribute__((noinline, cold)) static void footprint_fold_now(int64_t now, int64_t *folded,
                                                               _Atomic int64_t *process,
                                                               _Atomic int64_t *peak) {
  int64_t change = now - *folded;
  *folded = now;
  atomic_raise(peak, atomic_fetch_add_explicit(process, change, memory_order_relaxed) + change);
}
#endif

/* Folds this carrier's change since its last fold into a process-wide
   footprint once it reaches FOOTPRINT_FOLD_BYTES either way, and raises the
   peak to the new sum. Nothing with one carrier, or in the simulation, whose
   own peaks are already the process's. */
static inline void footprint_fold(int64_t now, int64_t *folded, _Atomic int64_t *process, _Atomic int64_t *peak) {
#ifdef CHIMI_CARRIER_SIMULATION
  (void)now, (void)folded, (void)process, (void)peak;
#else
  if (carrier_count == 1) return;
  int64_t change = now - *folded;
  if (change < FOOTPRINT_FOLD_BYTES && change > -FOOTPRINT_FOLD_BYTES) return;
  footprint_fold_now(now, folded, process, peak);
#endif
}

#ifndef CHIMI_CARRIER_SIMULATION
/* Starts the process-wide counters from carrier 0, the only carrier before
   carriers_create, so the peak keeps whatever carrier 0 reached before. */
static void footprint_seed_process(void) {
  int64_t footprint = carrier_first.heap_totals.footprint_bytes + carrier_first.pending_footprint_bytes;
  int64_t fragments = carrier_first.heap_totals.fragment_footprint_bytes;
  carrier_first.folded_footprint_bytes = footprint;
  carrier_first.folded_fragment_footprint_bytes = fragments;
  atomic_store_explicit(&process_footprint_bytes, footprint, memory_order_relaxed);
  atomic_store_explicit(&process_fragment_footprint_bytes, fragments, memory_order_relaxed);
  atomic_store_explicit(&process_peak_footprint_bytes, carrier_first.heap_totals.peak_footprint_bytes,
                        memory_order_relaxed);
  atomic_store_explicit(&process_peak_fragment_footprint_bytes,
                        carrier_first.heap_totals.peak_fragment_footprint_bytes, memory_order_relaxed);
}
#endif

static inline void footprint_record_peak(void) {
  Carrier *const here = carrier;
  int64_t footprint = footprint_now();
  if (footprint > here->heap_totals.peak_footprint_bytes) here->heap_totals.peak_footprint_bytes = footprint;
  footprint_fold(footprint, &here->folded_footprint_bytes, &process_footprint_bytes, &process_peak_footprint_bytes);
}

static void footprint_add(size_t bytes) {
  carrier->heap_totals.footprint_bytes += (int64_t)bytes;
  footprint_record_peak();
}

/* The peak is recorded before the fall (the young bytes deferred in
   pending_footprint_bytes peak just before it), and the fall is folded after
   it, so a carrier that only frees still brings the process count down. */
static inline void footprint_sub(size_t bytes) {
  Carrier *const here = carrier;
  footprint_record_peak();
  here->heap_totals.footprint_bytes -= (int64_t)bytes;
  footprint_fold(footprint_now(), &here->folded_footprint_bytes, &process_footprint_bytes,
                 &process_peak_footprint_bytes);
}

static void fragment_footprint_fold(void) {
  footprint_fold(fragment_footprint_now(), &carrier->folded_fragment_footprint_bytes,
                 &process_fragment_footprint_bytes, &process_peak_fragment_footprint_bytes);
}

static void fragment_footprint_add(size_t bytes) {
  Carrier *const here = carrier;
  here->heap_totals.fragment_footprint_bytes += (int64_t)bytes;
  int64_t footprint = fragment_footprint_now();
  if (footprint > here->heap_totals.peak_fragment_footprint_bytes)
    here->heap_totals.peak_fragment_footprint_bytes = footprint;
  fragment_footprint_fold();
}

static void fragment_footprint_sub(size_t bytes) {
  carrier->heap_totals.fragment_footprint_bytes -= (int64_t)bytes;
  fragment_footprint_fold();
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


static YoungChunk *young_chunk_take(void) {
  Carrier *const here = carrier;
  YoungChunk *chunk = here->young_chunk_cache;
  if (chunk) {
    here->young_chunk_cache = chunk->next;
    --here->young_chunk_cache_count;
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
  Carrier *const here = carrier;
#ifdef CHIMI_POISON_YOUNG_CHUNKS
  memset(chunk->data, 0xdb, YOUNG_CHUNK_CAPACITY);
  free(chunk);
  return;
#endif
  if (here->young_chunk_cache_count < YOUNG_CHUNK_CACHE) {
    memset(chunk->data, 0, (size_t)(chunk->used - chunk->data));
    chunk->next = here->young_chunk_cache;
    here->young_chunk_cache = chunk;
    ++here->young_chunk_cache_count;
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
  Carrier *const here = carrier;
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
  here->heap_totals.live_allocations += count;
  here->pending_footprint_bytes -= count * sizeof(AllocationHeader) + bytes;
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
    /* The deferred young bytes count toward the process footprint here,
       once per chunk, so a long pass cannot hide them from its peak. */
    footprint_fold(footprint_now(), &carrier->folded_footprint_bytes, &process_footprint_bytes,
                   &process_peak_footprint_bytes);
  }
  AllocationHeader *header = (AllocationHeader *)scope->bump;
  scope->bump += rounded;
  return header;
}

static size_t young_rounded(size_t bytes) {
  return (sizeof(AllocationHeader) + bytes + SMALL_BLOCK_GRAIN - 1) / SMALL_BLOCK_GRAIN * SMALL_BLOCK_GRAIN;
}

static void *allocate(size_t count, size_t size) {
  Carrier *const here = carrier;
  if (count == 0) count = 1;
  size_t bytes;
  if (__builtin_mul_overflow(count, size, &bytes) ||
      bytes > SIZE_MAX - sizeof(AllocationHeader) - SMALL_BLOCK_GRAIN) {
    fputs("Chimi runtime allocation overflow\n", stderr); runtime_exit(70);
  }
  AllocationScope *innermost = here->fiber.target ? NULL : here->fiber.heap->innermost;
  if (innermost && !innermost->links_young && bytes <= YOUNG_CHUNK_LARGEST) {
    size_t rounded = young_rounded(bytes);
    AllocationHeader *header = young_bump(innermost, rounded);
    header->bytes = bytes;
    header->area = &innermost->young;
    header->in_chunk = true;
    innermost->pending_bytes += bytes;
    ++innermost->pending_count;
    here->pending_footprint_bytes += sizeof(AllocationHeader) + bytes;
    if (profile_rows && here->fiber.heap->profiled_node_plus_one) {
      ProfileRow *row = &profile_rows[here->fiber.heap->profiled_node_plus_one - 1];
      profile_count(&row->allocations, 1);
      profile_count(&row->allocated_bytes, bytes);
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
    here->unscoped_allocated_bytes += bytes;
  }
  if (profile_rows && here->fiber.heap->profiled_node_plus_one) {
    ProfileRow *row = &profile_rows[here->fiber.heap->profiled_node_plus_one - 1];
    profile_count(&row->allocations, 1);
    profile_count(&row->allocated_bytes, bytes);
  }
  if (area->role != AREA_STATIC) {
    ++here->heap_totals.live_allocations;
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

/* Handle counts (docs/architecture/supervised-process-runtime.md, "Channel
   release"). Each live Sender, Receiver, Completion or Task value counts as one
   reference to its channel or task, and each live native handle value as one
   reference to its handle, in whichever heap or fragment it is allocated, so a
   copy in transit (a message, a task's captures or result) counts while the
   scheduler holds it. channel_capability_value, task_value and
   native_handle_value create these values, copy_forward is the only thing that
   copies them, and release_allocation the only thing that frees them; those
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
   the total is meaningful, and retain and drop take the table's lock. The
   table is an open-addressing map keyed by handle key; keys are never 0. */
#define NATIVE_HANDLE_KEY (UINT64_C(1) << 62)

static uint64_t native_handle_key(uint32_t slot, uint32_t generation) {
  return NATIVE_HANDLE_KEY | (uint64_t)slot << 32 | generation;
}

static void native_handle_unreferenced(uint64_t key);
typedef struct { uint64_t channel; size_t endpoints; } ChannelEndpoints;
typedef struct {
  pthread_mutex_t lock;
  ChannelEndpoints *slots;
  size_t capacity;
  /* Written under `lock`. */
  _Atomic size_t count;
} HandleTable;
static HandleTable handle_table = {.lock = PTHREAD_MUTEX_INITIALIZER};

/* Set at an execution's first retain of any counted handle, and kept until
   the next execution starts (channel_endpoints_reset). It is one flag for
   both tables, so the freers' shortcut below costs one load: every freed
   allocation pays it. */
static _Atomic bool handles_counted;

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

static size_t channel_slot(uint64_t channel, size_t capacity) {
  return (size_t)((channel * UINT64_C(0x9E3779B97F4A7C15)) >> 32) & (capacity - 1);
}

/* Requires: handle_table.lock held. Nothing that runs under the lock retains,
   drops, parks or runs a hook; exhausted_memory exits with it held, which is
   safe while no atexit handler touches the table. */
static void handle_table_grow(void) {
  size_t capacity = handle_table.capacity ? handle_table.capacity * 2 : 64;
  ChannelEndpoints *table = calloc(capacity, sizeof(*table));
  if (!table) exhausted_memory();
  for (size_t index = 0; index < handle_table.capacity; ++index) {
    if (!handle_table.slots[index].channel) continue;
    size_t slot = channel_slot(handle_table.slots[index].channel, capacity);
    while (table[slot].channel) slot = (slot + 1) & (capacity - 1);
    table[slot] = handle_table.slots[index];
  }
  free(handle_table.slots);
  handle_table.slots = table;
  handle_table.capacity = capacity;
}

static void handle_table_set_count(size_t count) {
  atomic_store_explicit(&handle_table.count, count, memory_order_relaxed);
}

static void native_handle_retain(uint64_t channel) {
  pthread_mutex_lock(&handle_table.lock);
  size_t count = atomic_load_explicit(&handle_table.count, memory_order_relaxed);
  if (2 * (count + 1) > handle_table.capacity) handle_table_grow();
  ChannelEndpoints *slots = handle_table.slots;
  size_t mask = handle_table.capacity - 1;
  size_t slot = channel_slot(channel, handle_table.capacity);
  while (slots[slot].channel && slots[slot].channel != channel) slot = (slot + 1) & mask;
  if (!slots[slot].channel) {
    slots[slot].channel = channel;
    handle_table_set_count(count + 1);
  }
  ++slots[slot].endpoints;
  pthread_mutex_unlock(&handle_table.lock);
}

/* Removes one reference to `channel`. Ensures: true exactly when this removed the
   last one, so the caller owns the key's release. Requires: handle_table.lock.
   No other thread can queue the same key meanwhile: once the count is zero, no
   value names the key, so none can retain or drop it. */
static bool handle_table_drop(uint64_t channel) {
  ChannelEndpoints *slots = handle_table.slots;
  if (!slots) return false;
  size_t mask = handle_table.capacity - 1;
  size_t slot = channel_slot(channel, handle_table.capacity);
  while (slots[slot].channel != channel) {
    /* An endpoint of an earlier execution's scheduler (benchmark repeats). */
    if (!slots[slot].channel) return false;
    slot = (slot + 1) & mask;
  }
  if (--slots[slot].endpoints != 0) return false;
  /* Backward-shift deletion keeps every probe sequence unbroken. */
  size_t hole = slot;
  for (size_t next = (hole + 1) & mask; slots[next].channel; next = (next + 1) & mask) {
    size_t home = channel_slot(slots[next].channel, handle_table.capacity);
    if (((next - home) & mask) >= ((next - hole) & mask)) {
      slots[hole] = slots[next];
      hole = next;
    }
  }
  slots[hole] = (ChannelEndpoints){0};
  handle_table_set_count(atomic_load_explicit(&handle_table.count, memory_order_relaxed) - 1);
  return true;
}

/* handle_table_drop under the table's lock. */
static bool handle_table_release(uint64_t channel) {
  pthread_mutex_lock(&handle_table.lock);
  bool last = handle_table_drop(channel);
  pthread_mutex_unlock(&handle_table.lock);
  return last;
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
  if (handle_table_release(key)) native_handle_unreferenced(key);
}

/* Each execution starts a new scheduler, whose channel and task ids start over
   (the scheduler clears its own counts). Runs before the scheduler starts, so
   no fiber holds a handle on any carrier. */
static void channel_endpoints_reset(void) {
  pthread_mutex_lock(&handle_table.lock);
  if (handle_table.slots)
    memset(handle_table.slots, 0, handle_table.capacity * sizeof(*handle_table.slots));
  handle_table_set_count(0);
  pthread_mutex_unlock(&handle_table.lock);
  atomic_store_explicit(&handles_counted, false, memory_order_relaxed);
}

/* Frees one allocation that its area has already unlinked. */
static void release_allocation(AllocationHeader *header) {
  if (handles_live() && header->kind == ALLOCATION_VALUE) {
    uint64_t key = counted_handle_key((ChimiValue *)(header + 1));
    if (key) counted_handle_drop(key);
  }
  if (header->area->heap) header->area->heap->live_bytes -= header->bytes;
  if (header->area->role != AREA_STATIC) {
    --carrier->heap_totals.live_allocations;
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
  heap->owner = atomic_fetch_add_explicit(&next_heap_owner, 1, memory_order_relaxed);
  heap->base.role = AREA_BASE;
  heap->base.heap = heap;
  ++carrier->heap_totals.live_heaps;
  return heap;
}

/* Requires: no region of `heap` is open. Frees every allocation the heap owns,
   adopted fragments included, without touching any other heap. */
static void heap_destroy(RuntimeHeap *heap) {
  if (heap->innermost) { fputs("Chimi runtime destroyed a heap with an open region\n", stderr); runtime_exit(70); }
  area_release_all(&heap->base);
  while (heap->adopted) {
    AllocationArea *area = heap->adopted;
    heap->adopted = area->next;
    area_release_all(area);
    area_free(area);
  }
  free(heap);
  --carrier->heap_totals.live_heaps;
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
  if (vector.length == UINT32_MAX) { fputs("Chimi runtime list length overflow\n", stderr); runtime_exit(70); }
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
  if (!level) { fputs("Chimi runtime exhausted memory\n", stderr); runtime_exit(70); }
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
  Carrier *const here = carrier;
  ChimiValue **values = malloc((size_t)length * sizeof(*values));
  if (!values) exhausted_memory();
  for (uint32_t index = 0; index < length; ++index) values[index] = list_get(list, from + index);
  AllocationArea *previous = here->fiber.target;
  if (area) here->fiber.target = area;
  ListVector copy = vector_from_values(values, length);
  here->fiber.target = previous;
  free(values);
  return copy;
}
/* A window shorter than 1/LIST_RETAINED_FRACTION of its vector is copied. */
enum { LIST_RETAINED_FRACTION = 4 };
static bool list_retains_too_much(uint32_t window, uint32_t vector) {
  return (uint64_t)window * LIST_RETAINED_FRACTION < vector;
}
/* True when `list` is a module constant and carriers run on threads. Compacting it
   would rewrite a value other threads read, and allocate in the static area while
   they run. The simulation's carriers share one thread, so it compacts as one
   carrier does. decode_constant builds every list constant whole, and a whole
   list is never compacted, so today this guards only a constant built as a
   window of a longer vector. */
static bool list_shared_across_threads(const ChimiValue *list) {
#ifdef CHIMI_CARRIER_SIMULATION
  (void)list;
  return false;
#else
  return carrier_count > 1 && allocation_header(list)->area->role == AREA_STATIC;
#endif
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
   in-place update is safe because a heap value belongs to one fiber, which does not
   switch inside list_slice. A module constant is the exception: when carriers run
   on threads, every carrier may read it at once, so its window is copied instead
   (list_shared_across_threads). */
static ChimiValue *list_slice(ChimiValue *list, uint32_t from, uint32_t to) {
  if (from > to || to > list->as.list.length) return NULL;
  if (from == to) return list_value((ListVector){NULL, NULL, 0}, 0, 0);
  uint32_t length = to - from;
  if (list_retains_too_much(length, list->as.list.vector.length)) {
    if (list_retains_too_much(length, list->as.list.length) || list_shared_across_threads(list))
      return list_value(list_copy_window(list, from, length, NULL), 0, length);
    list->as.list.vector =
      list_copy_window(list, 0, list->as.list.length, allocation_header(list)->area);
    list->as.list.start = 0;
  }
  return list_value(list->as.list.vector, list->as.list.start + from, length);
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


static void mark_push(void *pointer, AllocationKind kind) {
  Carrier *const here = carrier;
  if (!pointer) return;
  AllocationHeader *header = allocation_header(pointer);
  if (header->kind != kind) allocation_graph_inconsistent();
  AllocationArea *area = header->area;
  if (area->scope != here->collecting_scope) {
    if (area->heap != here->collecting_scope->heap && area->role != AREA_STATIC) here->collecting_foreign = true;
    return;
  }
  if (area->role == AREA_SURVIVOR && !here->collecting_full) return;
  if (header->mark_epoch == here->collecting_epoch) return;
  header->mark_epoch = here->collecting_epoch;
  ++here->collecting_scope->scanned_allocations;
  pointer_stack_push(&here->mark_stack, pointer);
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
        case VALUE_PROCESS_MONITOR:
        case VALUE_TEST_RECORDER:
        case VALUE_TEST_SCRIPT:
        case VALUE_TEST_MAILBOX: break;
        case VALUE_NATIVE_HANDLE: break;
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

static void *forward_young(void *pointer) {
  Carrier *const here = carrier;
  if (!pointer) return NULL;
  AllocationHeader *header = allocation_header(pointer);
  if (!header->in_chunk || header->area->scope != here->collecting_scope) return pointer;
  if (header->next) return header->next + 1;
  if (header->mark_epoch != here->collecting_epoch) allocation_graph_inconsistent();
  size_t total = sizeof(*header) + header->bytes;
  AllocationHeader *copy = small_block_take(total);
  memcpy(copy, header, total);
  copy->in_chunk = false;
  area_link(&here->collecting_scope->survivors, copy);
  here->collecting_scope->promoted_bytes += copy->bytes;
  header->next = copy;
  pointer_stack_push(&here->copy_stack, copy + 1);
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
    if (header->mark_epoch == carrier->collecting_epoch) forward_children(header + 1);
}

/* Whether this pass of `scope` has bumped an allocation into its chunks. */
static bool young_chunks_used(const AllocationScope *scope) {
  return scope->first_chunk && (scope->first_chunk->next || scope->bump != scope->first_chunk->data);
}

/* Moves what the roots reach out of `scope`'s chunks and updates the roots. */
static void forward_survivors(AllocationScope *scope, ChimiValue **value, ChimiControl **control) {
  Carrier *const here = carrier;
  if (!young_chunks_used(scope)) return;
  if (value) *value = forward_young(*value);
  if (control) *control = forward_young(*control);
  forward_marked_list(&scope->young);
  for (AllocationArea *area = scope->adopted; area; area = area->next) forward_marked_list(area);
  while (here->copy_stack.length) forward_children(here->copy_stack.items[--here->copy_stack.length]);
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
      if (handles_live() && header->kind == ALLOCATION_VALUE) {
        uint64_t key = counted_handle_key((ChimiValue *)(header + 1));
        if (key) counted_handle_drop(key);
      }
    }
  }
  scope->live_bytes -= dead_bytes;
  scope->heap->live_bytes -= dead_bytes;
  carrier->heap_totals.live_allocations -= dead_count;
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
  RuntimeHeap *heap = carrier->fiber.heap;
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
    if (header->mark_epoch == carrier->collecting_epoch) {
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
    if (header->mark_epoch == carrier->collecting_epoch) {
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
/* A mark epoch no pass on any carrier has used. */
static uint64_t take_mark_epoch(void) {
  Carrier *const here = carrier;
  if (here->next_epoch == here->end_epoch) {
    uint64_t start = atomic_fetch_add_explicit(&next_mark_epoch, MARK_EPOCH_BLOCK, memory_order_relaxed);
    if (start > UINT64_MAX - MARK_EPOCH_BLOCK) {
      fputs("Chimi runtime allocation mark epoch exhausted\n", stderr); runtime_exit(70);
    }
    here->next_epoch = start;
    here->end_epoch = start + MARK_EPOCH_BLOCK;
  }
  return here->next_epoch++;
}

static HeapStatus allocation_scope_sweep(AllocationScope *scope, ChimiValue **value,
                                         ChimiControl **control, bool full, bool ending) {
  Carrier *const here = carrier;
  here->collecting_epoch = take_mark_epoch();
  flush_young_counts(scope);
  here->collecting_scope = scope;
  here->collecting_full = full;
  here->collecting_foreign = false;
  mark_push(value ? *value : NULL, ALLOCATION_VALUE);
  mark_push(control ? *control : NULL, ALLOCATION_CONTROL);
  while (here->mark_stack.length) mark_children(here->mark_stack.items[--here->mark_stack.length]);
  bool foreign = here->collecting_foreign;
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
  here->collecting_scope = NULL;
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
  Carrier *const here = carrier;
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
  AllocationArea *previous = here->fiber.target;
  here->fiber.target = copy->target;
  void *duplicate = allocate(header->bytes, 1);
  here->fiber.target = previous;
  allocation_kind(duplicate, kind);
  memcpy(duplicate, pointer, header->bytes);
  if (here->copy_counting) {
    here->copied_bytes += header->bytes;
    if (kind == ALLOCATION_VALUE) ++here->copied_values;
  }
  if (kind == ALLOCATION_VALUE) {
    uint64_t key = counted_handle_key(duplicate);
    if (key) counted_handle_retain(key);
  }
  copy_table_insert(copy, pointer, duplicate);
  pointer_stack_push(&here->copy_pending, duplicate);
  return duplicate;
}

static void copy_list_window(FragmentCopy *copy, ChimiValue *list) {
  Carrier *const here = carrier;
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
  AllocationArea *previous = here->fiber.target;
  here->fiber.target = copy->target;
  list->as.list.vector = vector_from_values(values, length);
  here->fiber.target = previous;
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
        case VALUE_PROCESS_MONITOR:
        case VALUE_TEST_RECORDER:
        case VALUE_TEST_SCRIPT:
        case VALUE_TEST_MAILBOX: break;
        case VALUE_NATIVE_HANDLE: break;
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
  Carrier *const here = carrier;
  memset(copy->inline_keys, 0, sizeof(copy->inline_keys));
  copy->keys = copy->inline_keys;
  copy->copies = copy->inline_copies;
  copy->capacity = COPY_INLINE_SLOTS;
  for (uint32_t index = 0; index < count; ++index)
    out[index] = copy_forward(copy, roots[index], kinds[index]);
  while (here->copy_pending.length) copy_scan(copy, here->copy_pending.items[--here->copy_pending.length]);
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
  Carrier *const here = carrier;
  if (count > FRAGMENT_ROOTS) abort();
  RuntimeFragment *fragment = small_block_take(sizeof(*fragment));
  fragment->area.role = AREA_FRAGMENT;
  fragment->root_count = count;
  ++here->heap_totals.live_fragments;
  footprint_add(sizeof(*fragment));
  fragment_footprint_add(sizeof(*fragment));
  FragmentCopy copy = {.source_heap = here->fiber.heap, .target = &fragment->area};
  HeapStatus status = copy_graph(&copy, roots, kinds, count, fragment->roots);
  if (status != HEAP_OK) {
    here->fragment_copy_foreign_owner = copy.foreign_owner;
    fragment_footprint_sub(fragment_footprint(fragment));
    area_release_all(&fragment->area);
    --here->heap_totals.live_fragments;
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
  Carrier *const here = carrier;
  for (uint32_t index = 0; index < fragment->root_count; ++index) roots[index] = fragment->roots[index];
  fragment_footprint_sub(fragment_footprint(fragment));
  --here->heap_totals.live_fragments;
  AllocationArea *area = &fragment->area;
  RuntimeHeap *heap = here->fiber.heap;
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
    here->unscoped_allocated_bytes += area->bytes;
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
  ++carrier->heap_totals.live_fragments;
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
  fragment_footprint_sub(fragment_footprint(fragment));
  --carrier->heap_totals.live_fragments;
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

static ChimiValue *integer_value(int64_t raw) { ChimiValue *v = new_value(VALUE_INTEGER); v->as.integer = raw; return v; }
static ChimiValue *range_value(int64_t start, int64_t end) {
  ChimiValue *v = new_value(VALUE_RANGE); v->as.range.start = start; v->as.range.end = end; return v;
}
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
static char *copy_text(const char *raw) {
  size_t length = strlen(raw);
  char *copy = allocate(length + 1, 1);
  memcpy(copy, raw, length);
  return copy;
}
static ChimiValue *text_from_bytes(const char *bytes, size_t length) {
  char *raw = allocate(length + 1, 1);
  /* A binding may pass NULL for no bytes, and memcpy from NULL is undefined. */
  if (length) memcpy(raw, bytes, length);
  raw[length] = '\0';
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
  if (!message) { fputs("Chimi runtime allocation failed\n", stderr); runtime_exit(70); }
  vsnprintf(message, 256, format, arguments); va_end(arguments);
  return message;
}

/* A defect message copied outside every reclaimable heap, like
   numeric_message's: a control's message is not traced, and a loop region or a
   child task's heap may be freed while the defect it ended with is still read. */
static const char *lasting_message(const char *raw) {
  size_t length = strlen(raw);
  char *message = malloc(length + 1);
  if (!message) { fputs("Chimi runtime allocation failed\n", stderr); runtime_exit(70); }
  memcpy(message, raw, length + 1);
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
typedef unsigned __int128 DecimalMagnitude;
/* The codes `std/decimal.rounding_code` passes, in `Rounding` declaration order. */
typedef enum { ROUND_TOWARD_ZERO, ROUND_AWAY_FROM_ZERO, ROUND_FLOOR, ROUND_CEILING,
               ROUND_HALF_TOWARD_ZERO, ROUND_HALF_AWAY_FROM_ZERO, ROUND_HALF_EVEN } DecimalRounding;
#define DECIMAL_MAX_SCALE 38u
/* decimal.rs TEXT_BYTES and a NUL. */
#define DECIMAL_TEXT_BYTES 48
bool chimi_decimal_new_v1(bool negative, uint64_t high, uint64_t low, uint32_t scale, ChimiDecimal *out);
ChimiDecimal chimi_decimal_negate_v1(ChimiDecimal value);
bool chimi_decimal_add_v1(ChimiDecimal left, ChimiDecimal right, ChimiDecimal *out);
bool chimi_decimal_subtract_v1(ChimiDecimal left, ChimiDecimal right, ChimiDecimal *out);
bool chimi_decimal_multiply_v1(ChimiDecimal left, ChimiDecimal right, ChimiDecimal *out);
bool chimi_decimal_divide_v1(ChimiDecimal dividend, ChimiDecimal divisor, uint32_t scale, uint32_t rounding,
                             ChimiDecimal *out);
int32_t chimi_decimal_compare_v1(ChimiDecimal left, ChimiDecimal right);
size_t chimi_decimal_text_v1(ChimiDecimal value, bool normalized, char *out);
bool chimi_decimal_parse_v1(const char *text, size_t length, ChimiDecimal *out);

static bool decimal_make(bool negative, DecimalMagnitude magnitude, uint32_t scale, Decimal *output) {
  return chimi_decimal_new_v1(negative, (uint64_t)(magnitude >> 64), (uint64_t)magnitude, scale, output);
}
static Decimal decimal_of(const ChimiValue *value) { return value->as.decimal; }
static ChimiValue *decimal_value(Decimal decimal) {
  ChimiValue *v = new_value(VALUE_DECIMAL); v->as.decimal = decimal; return v;
}
/* Numeric order, -1, 0 or 1, out of line so chimi_value_compare's frame keeps no
   by-value decimals. */
__attribute__((noinline)) static int decimal_order(const ChimiValue *left, const ChimiValue *right) {
  return chimi_decimal_compare_v1(decimal_of(left), decimal_of(right));
}
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
static ChimiValue *callable_value(ChimiCallable *raw) { ChimiValue *v = new_value(VALUE_CALLABLE); v->as.callable = raw; return v; }
static ChimiValue *map_value(MapNode *raw) { ChimiValue *v = new_value(VALUE_MAP); v->as.map = raw; return v; }
static ChimiValue *set_value(MapNode *raw) { ChimiValue *v = new_value(VALUE_SET); v->as.map = raw; return v; }
static ChimiValue *json_number_value(const char *raw) { ChimiValue *v = new_value(VALUE_JSON_NUMBER); v->as.json_number = copy_text(raw); return v; }
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
  /* Managed process and supervisor handles met while collecting. Each has one
     owner, so a value a broadcast or watch would copy must not hold one. */
  size_t managed_count;
} CapabilityTransfer;


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
    ++carrier->capture_transfer_probes;
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

/* The defect for publishing a managed handle where every subscriber would get a copy:
   a handle has one owner, so two copies would both look valid. */
#define MANAGED_COPY_DEFECT(function)                                                    \
  "channel capability violation: " function " cannot hold a managed process or "         \
  "supervisor handle, because every subscriber gets a copy"

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
    case VALUE_PROCESS_MONITOR:
    case VALUE_TEST_RECORDER:
    case VALUE_TEST_SCRIPT:
    case VALUE_TEST_MAILBOX: return true;
    case VALUE_NATIVE_HANDLE: return true;
    case VALUE_MANAGED_PROCESS:
    case VALUE_MANAGED_SUPERVISOR:
      if (transfer->collect) ++transfer->managed_count;
      return true;
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
    if (steps == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); runtime_exit(70); }
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
  if (exponent.digits == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); runtime_exit(70); }
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
  if (result == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); runtime_exit(70); }
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
  if (number.owned == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); runtime_exit(70); }
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
  if (sorted == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); runtime_exit(70); }
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
  if (nodes == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); runtime_exit(70); }
  const MapNode *node = root;
  for (;;) {
    while (node) { pending[pending_length++] = node; node = node->left; }
    if (pending_length == 0) break;
    node = pending[--pending_length];
    if (length == capacity) {
      capacity *= 2; const MapNode **grown = realloc(nodes, capacity * sizeof(*nodes));
      if (grown == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); runtime_exit(70); }
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
      case VALUE_DECIMAL: order = decimal_order(left, right); break;
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
      case VALUE_PROCESS_MONITOR: case VALUE_TEST_RECORDER: case VALUE_TEST_SCRIPT:
      case VALUE_TEST_MAILBOX:
        order = order_of((int64_t)left->as.address.target, (int64_t)right->as.address.target); break;
      case VALUE_NATIVE_HANDLE:
        order = order_of(left->as.handle.slot, right->as.handle.slot);
        if (order == 0) order = order_of(left->as.handle.generation, right->as.handle.generation);
        break;
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
/* Json.decode. The decoder of the untrusted bytes is Rust
   (runtime/chimi-runtime/src/json.rs): it checks the whole input and returns
   either an error, with the message and offset the interpreter reports, or a
   tape of the document's values in post-order. This side only builds the
   values from a tape, so it reads no input byte the decoder has not checked.
   The two structs mirror json_abi.rs; the `_v1` suffix changes with them. */
typedef struct { uint32_t kind; size_t offset; size_t length; } ChimiJsonNode;
enum {
  JSON_NODE_NULL, JSON_NODE_FALSE, JSON_NODE_TRUE, JSON_NODE_NUMBER, JSON_NODE_SOURCE_TEXT,
  JSON_NODE_DECODED_TEXT, JSON_NODE_ARRAY, JSON_NODE_OBJECT, JSON_NODE_SOURCE_KEY,
  JSON_NODE_DECODED_KEY
};
typedef struct {
  const ChimiJsonNode *nodes; size_t node_count; const char *decoded;
  const char *error_message; size_t error_message_length; size_t error_offset; void *owned;
} ChimiJsonParse;
bool chimi_json_parse_v1(const char *source, size_t length, ChimiJsonParse *out);
void chimi_json_release_v1(ChimiJsonParse *parse);
_Static_assert(sizeof(ChimiJsonNode) == 3 * sizeof(size_t), "ChimiJsonNode mirrors json::Node");

static _Noreturn void json_tape_defect(void) {
  fputs("Chimi runtime: malformed JSON tape\n", stderr); runtime_exit(70);
}
/* Builds the value a tape describes: each node's members are the values on
   top of the stack when it is reached. The tape's shape is the decoder's
   contract; a node that breaks it ends the program rather than reading past
   the stack. */
static ChimiValue *json_build(const ChimiJsonParse *parse, const char *source) {
  if (parse->node_count == 0 || parse->node_count > SIZE_MAX / sizeof(ChimiValue *)) json_tape_defect();
  ChimiValue **stack = malloc(parse->node_count * sizeof(*stack));
  if (!stack) exhausted_memory();
  size_t depth = 0;
  for (size_t index = 0; index < parse->node_count; ++index) {
    const ChimiJsonNode *node = &parse->nodes[index];
    const char *text = node->kind == JSON_NODE_DECODED_TEXT || node->kind == JSON_NODE_DECODED_KEY
                           ? parse->decoded : source;
    ChimiValue *value;
    switch (node->kind) {
      case JSON_NODE_NULL: value = standard_value(CHIMI_CONSTRUCTOR_JSON_NULL, NULL, 0); break;
      case JSON_NODE_FALSE: case JSON_NODE_TRUE: {
        ChimiValue *fields[] = {boolean_value(node->kind == JSON_NODE_TRUE)};
        value = standard_value(CHIMI_CONSTRUCTOR_JSON_BOOLEAN, fields, 1); break;
      }
      case JSON_NODE_NUMBER: {
        char *number = allocate(node->length + 1, 1);
        memcpy(number, source + node->offset, node->length);
        ChimiValue *fields[] = {json_number_value(number)};
        value = standard_value(CHIMI_CONSTRUCTOR_JSON_NUMBER, fields, 1); break;
      }
      case JSON_NODE_SOURCE_TEXT: case JSON_NODE_DECODED_TEXT: {
        ChimiValue *fields[] = {text_from_bytes(text + node->offset, node->length)};
        value = standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1); break;
      }
      case JSON_NODE_SOURCE_KEY: case JSON_NODE_DECODED_KEY:
        value = text_from_bytes(text + node->offset, node->length); break;
      case JSON_NODE_ARRAY: {
        if (node->length > depth || node->length > UINT32_MAX) json_tape_defect();
        depth -= node->length;
        ChimiValue *fields[] = {list_from_values(stack + depth, (uint32_t)node->length)};
        value = standard_value(CHIMI_CONSTRUCTOR_JSON_ARRAY, fields, 1); break;
      }
      case JSON_NODE_OBJECT: {
        if (node->length > depth / 2) json_tape_defect();
        depth -= 2 * node->length;
        MapNode *map = NULL;
        for (size_t member = 0; member < node->length; ++member)
          map = map_put_node(map, stack[depth + 2 * member], stack[depth + 2 * member + 1]);
        ChimiValue *fields[] = {map_value(map)};
        value = standard_value(CHIMI_CONSTRUCTOR_JSON_OBJECT, fields, 1); break;
      }
      default: json_tape_defect();
    }
    stack[depth++] = value;
  }
  if (depth != 1) json_tape_defect();
  ChimiValue *root = stack[0];
  free(stack);
  return root;
}
static ChimiControl *json_decode(const ChimiValue *text) {
  ChimiJsonParse parse;
  ChimiControl *result;
  if (chimi_json_parse_v1(text->as.text.bytes, text->as.text.length, &parse)) {
    result = success(json_build(&parse, text->as.text.bytes));
  } else {
    ChimiValue *fields[] = {text_from_bytes(parse.error_message, parse.error_message_length),
                            integer_value((int64_t)parse.error_offset)};
    result = failure(standard_value(CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON, fields, 2));
  }
  chimi_json_release_v1(&parse);
  return result;
}

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment);

static ChimiControl *intrinsic_shape_defect(uint32_t call_node);

static bool utf8_boundary(const char *text, size_t length, size_t offset) {
  return offset == length || ((unsigned char)text[offset] & 0xc0) != 0x80;
}
/* Clamps [start, end) to [0, length]; a reversed range clamps to empty. */
static void clamp_range(int64_t start, int64_t end, size_t length, size_t *from, size_t *to) {
  size_t low = start <= 0 ? 0 : (uint64_t)start > length ? length : (size_t)start;
  size_t high = end <= 0 ? 0 : (uint64_t)end > length ? length : (size_t)end;
  *from = low; *to = high < low ? low : high;
}

#define COUNT_OF(table) (sizeof (table) / sizeof (table)[0])

/* Unicode text operations: chimi-runtime's text.rs and graphemes.rs, which the
   interpreter uses too, through text_abi.rs. Each takes a Text value's bytes. */
typedef struct { size_t start, length; } ChimiTextSpan;
typedef struct { const char *bytes; size_t length; void *owned; } ChimiOwnedText;
ChimiTextSpan chimi_text_trim_v1(const char *bytes, size_t length, bool end);
ChimiOwnedText chimi_text_case_map_v1(const char *bytes, size_t length, bool upper);
void chimi_text_release_v1(ChimiOwnedText owned);
typedef struct { const size_t *ends; size_t count; void *owned; } ChimiGraphemeEnds;
ChimiGraphemeEnds chimi_text_grapheme_ends_v1(const char *bytes, size_t length);
void chimi_text_release_ends_v1(ChimiGraphemeEnds owned);

/* Text.lowercase and Text.uppercase: the Unicode default case conversion, Final_Sigma
   included. Mappings change length (U+00DF ß uppercases to "SS"). */
static ChimiValue *text_case_map(const ChimiValue *value, bool upper) {
  ChimiOwnedText mapped = chimi_text_case_map_v1(value->as.text.bytes, (size_t)value->as.text.length, upper);
  ChimiValue *result = text_from_bytes(mapped.bytes, mapped.length);
  chimi_text_release_v1(mapped);
  return result;
}
/* Text.trim_start, or with `end` Text.trim_end: Unicode White_Space removed. */
static ChimiValue *text_trim(const ChimiValue *value, bool end) {
  ChimiTextSpan kept = chimi_text_trim_v1(value->as.text.bytes, (size_t)value->as.text.length, end);
  return text_from_bytes(value->as.text.bytes + kept.start, kept.length);
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
      return success(text_trim(arguments[0], false));
    }
    case CHIMI_INTRINSIC_TEXT_TRIM_END: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      return success(text_trim(arguments[0], true));
    }
    case CHIMI_INTRINSIC_TEXT_LOWERCASE:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      return success(text_case_map(arguments[0], false));
    case CHIMI_INTRINSIC_TEXT_UPPERCASE:
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      return success(text_case_map(arguments[0], true));
    case CHIMI_INTRINSIC_TEXT_GRAPHEMES: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) break;
      const char *text = arguments[0]->as.text.bytes;
      size_t length = arguments[0]->as.text.length;
      /* Extended grapheme clusters (Unicode UAX #29). */
      ChimiGraphemeEnds ends = chimi_text_grapheme_ends_v1(text, length);
      ChimiValue *graphemes = list_value((ListVector){NULL, NULL, 0}, 0, 0);
      for (size_t index = 0, start = 0; index < ends.count; start = ends.ends[index++])
        graphemes = list_push(graphemes, text_from_bytes(text + start, ends.ends[index] - start));
      chimi_text_release_ends_v1(ends);
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

/* The native core of std/bytes, and std/http's byte scan. Defect messages match
   the interpreter's (execution/bytes_operations.rs). */
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
    case CHIMI_INTRINSIC_HTTP_COUNT_ENDS: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_BYTES || arguments[1]->kind != VALUE_BYTES) break;
      bool allowed[256] = {false};
      for (size_t at = 0; at < arguments[1]->as.text.length; ++at)
        allowed[(unsigned char)arguments[1]->as.text.bytes[at]] = true;
      const unsigned char *bytes = (const unsigned char *)arguments[0]->as.text.bytes;
      size_t length = arguments[0]->as.text.length, leading = 0, trailing = 0;
      while (leading < length && allowed[bytes[leading]]) ++leading;
      if (leading == length) trailing = length;
      while (trailing < length && allowed[bytes[length - 1 - trailing]]) ++trailing;
      ChimiValue *pair = new_value(VALUE_TUPLE);
      pair->as.record.length = 2;
      pair->as.record.fields = allocate(2, sizeof(*pair->as.record.fields));
      allocation_kind(pair->as.record.fields, ALLOCATION_FIELDS);
      pair->as.record.fields[0] = (RuntimeField){NULL, integer_value((int64_t)leading)};
      pair->as.record.fields[1] = (RuntimeField){NULL, integer_value((int64_t)trailing)};
      return success(pair);
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
   Nothing under it parks; it is taken before handle_table's lock. */
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
typedef struct ConsoleLine ConsoleLine;
struct ConsoleLine {
  uint64_t end; /* Queue offset just past the line. */
  int error;    /* Nonzero once a failed write dropped the line. */
  ConsoleLine *next;
  ConsoleLine *previous;
};

/* The bytes waiting for one open file. */
typedef struct {
  unsigned char *bytes;
  size_t capacity;
  size_t start;
  size_t end;
  /* Queue offset of bytes[start]: every byte before it is written or dropped. */
  uint64_t resolved;
  /* The writers waiting for their lines, in queue order. Each record is on
     the heap, not its writer's stack: a fiber that overflows its stack while
     it waits ends without returning, and its record must not dangle. The
     writer unlinks and frees it before returning. */
  ConsoleLine *first;
  ConsoleLine *last;
} ConsoleQueue;

typedef enum { CONSOLE_UNKNOWN, CONSOLE_DIRECT, CONSOLE_QUEUED } ConsoleMode;

typedef struct {
  int fd;
  const char *name;
  ConsoleMode mode;
  bool socket;
  FILE *file;
  /* Its own queue, or standard output's when both descriptors name the same
     open file (`2>&1`, one terminal), so their lines never interleave. */
  ConsoleQueue *queue;
} ConsoleStream;

enum { CONSOLE_STREAM_COUNT = 3 };
static ConsoleQueue console_queues[CONSOLE_STREAM_COUNT];
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
  stream->queue = &console_queues[index];
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

static void console_enqueue(ConsoleQueue *queue, const void *bytes, size_t length) {
  if (queue->end + length > queue->capacity) {
    size_t queued = queue->end - queue->start;
    /* An empty queue may have no buffer yet, and memmove from NULL is undefined. */
    if (queued) memmove(queue->bytes, queue->bytes + queue->start, queued);
    queue->start = 0;
    queue->end = queued;
    if (queued + length > queue->capacity) {
      size_t capacity = queue->capacity ? queue->capacity : 4096;
      while (capacity < queued + length) {
        if (capacity > SIZE_MAX / 2) exhausted_memory();
        capacity *= 2;
      }
      unsigned char *grown = realloc(queue->bytes, capacity);
      if (!grown) exhausted_memory();
      queue->bytes = grown;
      queue->capacity = capacity;
    }
  }
  memcpy(queue->bytes + queue->end, bytes, length);
  queue->end += length;
}

/* Drops every queued byte and fails each line not yet fully written with
   `error`. A line already written stays written. */
static void console_fail(ConsoleQueue *queue, int error) {
  for (ConsoleLine *line = queue->first; line; line = line->next)
    if (line->end > queue->resolved) line->error = error;
  queue->resolved += queue->end - queue->start;
  queue->start = queue->end = 0;
}

/* One write(2) from the front of the queue, at most `limit` bytes. False with
   errno set when it failed; an interruption or a descriptor a parent left
   non-blocking reports nothing written and true. */
static bool console_write_front(const ConsoleStream *stream, size_t limit) {
  ConsoleQueue *queue = stream->queue;
  size_t length = queue->end - queue->start;
  if (length > limit) length = limit;
  ssize_t count = stream->socket
    ? send(stream->fd, queue->bytes + queue->start, length, MSG_DONTWAIT)
    : write(stream->fd, queue->bytes + queue->start, length);
  if (count < 0) return errno == EINTR || errno == EAGAIN || errno == EWOULDBLOCK;
  queue->start += (size_t)count;
  queue->resolved += (uint64_t)count;
  return true;
}

/* Writes the whole queue, blocking this thread. Only the end of the program
   uses it, when no task is left to keep running. */
static void console_drain_blocking(const ConsoleStream *stream) {
  ConsoleQueue *queue = stream->queue;
  while (queue->start < queue->end) {
    struct pollfd poll_fd = {.fd = stream->fd, .events = POLLOUT};
    if (poll(&poll_fd, 1, -1) < 0 && errno != EINTR) {
      console_fail(queue, errno);
      return;
    }
    if (!console_write_front(stream, SIZE_MAX)) {
      console_fail(queue, errno);
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

static void console_unlink(ConsoleQueue *queue, ConsoleLine *line) {
  if (line->previous) line->previous->next = line->next;
  else queue->first = line->next;
  if (line->next) line->next->previous = line->previous;
  else queue->last = line->previous;
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
  ConsoleQueue *queue = stream->queue;
  if (request.status == CHIMI_RESPONSE_ERROR) {
    console_fail(queue, fcntl(stream->fd, F_GETFD) < 0 ? EBADF : EIO);
    return CONSOLE_WAITED;
  }
  /* Another writer may have emptied the queue while this one waited. */
  if (queue->start < queue->end && !console_write_front(stream, PIPE_BUF)) console_fail(queue, errno);
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
  ConsoleQueue *queue = stream->queue;
  /* A body ending under a terminal cancel must not park (CHI-265), and must
     not block the other tasks either: its line is queued for later writers or
     the end of the program, past the limit if need be. */
  bool terminal = chimi_fiber_cancel_is_terminal();
  /* Nothing is queued yet, so a cancellation here loses nothing. */
  while (!terminal && queue->start < queue->end &&
         queue->end - queue->start + length + 1 > CONSOLE_QUEUE_LIMIT)
    if (console_wait_and_write(stream) == CONSOLE_CANCELLED) return -1;
  console_enqueue(queue, text, length);
  console_enqueue(queue, "\n", 1);
  if (terminal) return 0;
  ConsoleLine *line = malloc(sizeof(*line));
  if (!line) exhausted_memory();
  *line = (ConsoleLine){.end = queue->resolved + (queue->end - queue->start), .previous = queue->last};
  if (queue->last) queue->last->next = line;
  else queue->first = line;
  queue->last = line;
  int outcome = 0;
  for (;;) {
    if (line->error) { outcome = line->error; break; }
    if (queue->resolved >= line->end) break;
    if (console_wait_and_write(stream) == CONSOLE_CANCELLED) { outcome = -1; break; }
  }
  console_unlink(queue, line);
  free(line);
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
  int64_t seconds = (int64_t)now.tv_sec, millis = (int64_t)now.tv_nsec / 1000000;
  int64_t days = seconds / 86400, of_day = seconds % 86400;
  if (of_day < 0) { of_day += 86400; days -= 1; }
  int64_t year, month, day;
  civil_from_days(days, &year, &month, &day);
  char time[64];
  snprintf(time, sizeof time, "%04" PRId64 "-%02" PRId64 "-%02" PRId64 "T%02" PRId64 ":%02" PRId64 ":%02" PRId64,
           year, month, day, of_day / 3600, of_day / 60 % 60, of_day % 60);
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
typedef struct {
  const ChimiBindingType *type;
  void *state;
  uint32_t generation;
  /* Binding calls holding the handle now. */
  uint32_t users;
  /* The type of a handle closed while held, whose state the last holder
     releases; the slot is not reused before then. */
  const ChimiBindingType *closing;
  /* The call running binding code on the handle, or NULL. It is one of
     the holders, so a slot is never freed or reused while it has one. */
  const ChimiBindingCall *runner;
} BindingHandleSlot;

/* A state to release once the lock is let go. */
typedef struct {
  const ChimiBindingType *type;
  void *state;
} BindingRelease;

enum { BINDING_HANDLE_LIMIT = 1u << 20 };
static BindingHandleSlot *binding_handles;
static uint32_t binding_handle_count = 1;
static uint32_t binding_handle_capacity;
static uint32_t binding_free_slot;
static pthread_mutex_t binding_handle_lock = PTHREAD_MUTEX_INITIALIZER;
/* Signalled when a call stops running on a handle. */
static pthread_cond_t binding_handle_free = PTHREAD_COND_INITIALIZER;

/* The open slot `value` names, or NULL. The caller holds binding_handle_lock. */
static BindingHandleSlot *binding_handle_slot(const ChimiValue *value) {
  if (value->kind != VALUE_NATIVE_HANDLE) return NULL;
  uint32_t slot = value->as.handle.slot;
  if (slot == 0 || slot >= binding_handle_count) return NULL;
  BindingHandleSlot *entry = &binding_handles[slot];
  return entry->type != NULL && entry->generation == value->as.handle.generation ? entry : NULL;
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

/* Puts a slot whose state is released, or about to be, on the free list. A slot
   whose generation would wrap is retired instead, so a stale copy can never
   match a later handle. The free list threads through `state`. The caller
   holds the lock. */
static void binding_slot_free(BindingHandleSlot *entry, uint32_t slot) {
  entry->type = NULL;
  entry->closing = NULL;
  entry->state = NULL;
  entry->runner = NULL;
  if (entry->generation != UINT32_MAX) {
    entry->state = (void *)(uintptr_t)binding_free_slot;
    binding_free_slot = slot;
  }
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
  bool closed = false;
  pthread_mutex_lock(&binding_handle_lock);
  BindingHandleSlot *entry = slot != 0 && slot < binding_handle_count ? &binding_handles[slot] : NULL;
  if (entry && entry->type != NULL && entry->generation == generation && (!type || entry->type == type)) {
    closed = true;
    ++entry->generation;
    if (entry->users > 0) {
      entry->closing = entry->type;
      entry->type = NULL;
    } else {
      release = (BindingRelease){entry->type, entry->state};
      binding_slot_free(entry, slot);
    }
  }
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
    if (slot >= binding_handle_count) {
      pthread_mutex_unlock(&binding_handle_lock);
      return;
    }
    BindingHandleSlot *entry = &binding_handles[slot];
    if (entry->type != NULL || entry->closing != NULL) {
      if (entry->type != NULL) ++entry->generation;
      release = (BindingRelease){entry->type ? entry->type : entry->closing, entry->state};
      entry->users = 0;
      binding_slot_free(entry, slot);
    }
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
    const BindingHandleSlot *entry = &binding_handles[call->arguments[index]->as.handle.slot];
    if (entry->runner != NULL) return true;
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
    binding_handles[call->arguments[index]->as.handle.slot].runner = run ? call : NULL;
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
    for (uint32_t index = 0; index < count; ++index) {
      BindingHandleSlot *entry = binding_handle_slot(arguments[index]);
      held[index] = entry != NULL;
      if (entry) ++entry->users;
    }
    binding_run_locked(call, true);
    pthread_mutex_unlock(&binding_handle_lock);
    return;
  }
  /* Every let-go is taken back before the binding code goes on. */
  for (uint32_t index = 0; index < count; ++index)
    if (held[index] && binding_handles[arguments[index]->as.handle.slot].runner != call) abort();
  binding_run_locked(call, false);
  for (uint32_t index = 0; index < count; ++index) {
    if (!held[index]) continue;
    uint32_t slot = arguments[index]->as.handle.slot;
    BindingHandleSlot *entry = &binding_handles[slot];
    if (--entry->users > 0 || entry->closing == NULL) continue;
    if (release_count == 8) {
      /* Rare: more closed handles than the buffer holds; run the batch first. */
      pthread_mutex_unlock(&binding_handle_lock);
      for (uint32_t pending = 0; pending < release_count; ++pending) binding_release_run(releases[pending]);
      release_count = 0;
      pthread_mutex_lock(&binding_handle_lock);
    }
    releases[release_count++] = (BindingRelease){entry->closing, entry->state};
    binding_slot_free(entry, slot);
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
  pthread_mutex_lock(&binding_handle_lock);
  BindingHandleSlot *entry = binding_handle_slot(value);
  void *state = entry != NULL && entry->type == type ? entry->state : NULL;
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
  pthread_mutex_lock(&binding_handle_lock);
  uint32_t slot = binding_free_slot;
  if (slot != 0) {
    binding_free_slot = (uint32_t)(uintptr_t)binding_handles[slot].state;
  } else {
    bool grown = binding_handle_count < binding_handle_capacity;
    if (!grown && binding_handle_count < BINDING_HANDLE_LIMIT) {
      uint32_t capacity = binding_handle_capacity ? binding_handle_capacity * 2 : 64;
      BindingHandleSlot *larger = realloc(binding_handles, capacity * sizeof(*larger));
      if (larger != NULL) {
        memset(larger + binding_handle_capacity, 0,
               (capacity - binding_handle_capacity) * sizeof(*larger));
        binding_handles = larger;
        binding_handle_capacity = capacity;
        grown = true;
      }
    }
    if (!grown) {
      pthread_mutex_unlock(&binding_handle_lock);
      if (type->release) type->release(state);
      return NULL;
    }
    slot = binding_handle_count++;
  }
  BindingHandleSlot *entry = &binding_handles[slot];
  entry->type = type;
  entry->state = state;
  uint32_t generation = entry->generation;
  pthread_mutex_unlock(&binding_handle_lock);
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
  bool closed = binding_handle_slot(value) == NULL;
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

/* `std/duration`. The text form and the parser are chimi-runtime's duration.rs, which
   the interpreter uses too; the arithmetic and its defect messages here mirror that
   file's, and the language tests run a generated set through both. */
static const int64_t DURATION_UNITS[6] = {
  3600000000000, 60000000000, 1000000000, 1000000, 1000, 1
};
static const char *const DURATION_CONSTRUCTORS[6] = {
  "hours", "minutes", "seconds", "milliseconds", "microseconds", "nanoseconds"
};
static ChimiValue *duration_value(int64_t nanoseconds) {
  ChimiValue *v = new_value(VALUE_DURATION); v->as.integer = nanoseconds; return v;
}

size_t chimi_duration_text_v1(int64_t nanoseconds, char *out);
uint32_t chimi_duration_parse_v1(const char *text, size_t length, int64_t *out);

/* The canonical text form (chimi-runtime's duration.rs) into `text`, which holds
   at least 32 bytes. */
static void duration_text(int64_t duration, char *text) { chimi_duration_text_v1(duration, text); }

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
    /* duration_abi.rs: 0, or 1 Empty, 2 Invalid, 3 OutOfRange. */
    static const uint32_t failures[] = {0, CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY, CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID,
                                        CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE};
    int64_t value = 0;
    uint32_t outcome = chimi_duration_parse_v1(arguments[0]->as.text.bytes, (size_t)arguments[0]->as.text.length, &value);
    if (outcome >= COUNT_OF(failures)) return intrinsic_shape_defect(call_node);
    return outcome ? failure(standard_value(failures[outcome], NULL, 0)) : success(duration_value(value));
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
        ? process_name_label(value->as.address.target)
        : value->as.address.kind == CHIMI_ADDRESS_TEST ? "test" : NULL;
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
    case VALUE_TEST_RECORDER: debug_text(buffer, "<Recorder>"); return;
    case VALUE_TEST_SCRIPT: debug_text(buffer, "<Script>"); return;
    case VALUE_TEST_MAILBOX: debug_text(buffer, "<Mailbox>"); return;
    case VALUE_NATIVE_HANDLE:
      debug_text(buffer, "<");
      debug_text(buffer, value->as.handle.name);
      debug_text(buffer, ">");
      return;
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
      return json_decode(arguments[0]);
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
      return optional_decimal(chimi_decimal_parse_v1(arguments[0]->as.text.bytes, (size_t)arguments[0]->as.text.length,
                                                     &output), output);
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
      bool fits = intrinsic == CHIMI_INTRINSIC_DECIMAL_ADD_CHECKED ? chimi_decimal_add_v1(left, right, &output)
                : intrinsic == CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED ? chimi_decimal_subtract_v1(left, right, &output)
                : chimi_decimal_multiply_v1(left, right, &output);
      return optional_decimal(fits, output);
    }
    case CHIMI_INTRINSIC_DECIMAL_DIVIDE_ROUNDED: {
      if (argument_count != 4 || arguments[0]->kind != VALUE_DECIMAL || arguments[1]->kind != VALUE_DECIMAL ||
          arguments[2]->kind != VALUE_INTEGER || arguments[3]->kind != VALUE_INTEGER ||
          arguments[2]->as.integer < 0 || arguments[2]->as.integer > DECIMAL_MAX_SCALE ||
          arguments[3]->as.integer < ROUND_TOWARD_ZERO || arguments[3]->as.integer > ROUND_HALF_EVEN)
        break;
      bool fits = chimi_decimal_divide_v1(decimal_of(arguments[0]), decimal_of(arguments[1]),
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

/* The open files. A handle is `slot | generation << 32`, so a handle used after
   its file was closed names a slot whose generation moved on and fails with
   FS_CLOSED instead of reaching another file. A handle is an integer that any
   task, on any carrier, may hold, so the table is behind fs_file_lock, and a
   read or write takes the descriptor for the length of its system call
   (fs_take, fs_give). A close during one leaves the descriptor open until it
   is given back, so the number is not reused under the call. The slot keeps
   `fd` until then, and only a slot without one is reused. */
typedef struct {
  int fd;
  uint32_t generation;
  bool open;
  uint32_t users;
} FsFile;
static FsFile *fs_files;
static size_t fs_file_count;
static pthread_mutex_t fs_file_lock = PTHREAD_MUTEX_INITIALIZER;

static int64_t fs_insert(int fd) {
  pthread_mutex_lock(&fs_file_lock);
  size_t slot = 0;
  while (slot < fs_file_count && (fs_files[slot].open || fs_files[slot].fd >= 0)) ++slot;
  if (slot == fs_file_count) {
    FsFile *grown = realloc(fs_files, (fs_file_count + 1) * sizeof(*fs_files));
    if (!grown) exhausted_memory();
    fs_files = grown;
    fs_files[fs_file_count++] = (FsFile){-1, 0, false, 0};
  }
  fs_files[slot].fd = fd;
  fs_files[slot].open = true;
  int64_t handle = (int64_t)slot | ((int64_t)fs_files[slot].generation << 32);
  pthread_mutex_unlock(&fs_file_lock);
  return handle;
}

/* The open file `handle` names, or NULL. The caller holds fs_file_lock. */
static FsFile *fs_get(int64_t handle) {
  uint64_t slot = (uint64_t)handle & 0xffffffffu;
  if (handle < 0 || slot >= fs_file_count) return NULL;
  FsFile *file = &fs_files[slot];
  return file->open && (int64_t)file->generation == handle >> 32 ? file : NULL;
}

/* The descriptor of open `handle`, to be given back with fs_give, or -1 when
   the handle is closed. */
static int fs_take(int64_t handle) {
  pthread_mutex_lock(&fs_file_lock);
  FsFile *file = fs_get(handle);
  int fd = -1;
  if (file) {
    ++file->users;
    fd = file->fd;
  }
  pthread_mutex_unlock(&fs_file_lock);
  return fd;
}

/* Gives back what fs_take(handle) returned; the last user of a closed file
   closes its descriptor. */
static void fs_give(int64_t handle) {
  pthread_mutex_lock(&fs_file_lock);
  FsFile *file = &fs_files[(uint64_t)handle & 0xffffffffu];
  int fd = -1;
  if (--file->users == 0 && !file->open) {
    fd = file->fd;
    file->fd = -1;
  }
  pthread_mutex_unlock(&fs_file_lock);
  if (fd >= 0) (void)close(fd);
}

/* Closes `handle`; false when it was already closed. As the interpreter,
   which drops the file, a close error is not reported. */
static bool fs_close(int64_t handle) {
  pthread_mutex_lock(&fs_file_lock);
  FsFile *file = fs_get(handle);
  int fd = -1;
  if (file) {
    file->open = false;
    ++file->generation;
    if (file->users == 0) {
      fd = file->fd;
      file->fd = -1;
    }
  }
  pthread_mutex_unlock(&fs_file_lock);
  if (fd >= 0) (void)close(fd);
  return file != NULL;
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
    return invoke_map_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_SET_EMPTY && intrinsic <= CHIMI_INTRINSIC_SET_REDUCE_ITEMS)
    return invoke_set_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_JSON_DECODE && intrinsic <= CHIMI_INTRINSIC_JSON_NUMBER_TO_DECIMAL)
    return invoke_json_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if (intrinsic >= CHIMI_INTRINSIC_DECIMAL_SCALED && intrinsic <= CHIMI_INTRINSIC_DECIMAL_TO_FLOAT)
    return invoke_decimal_operation(runtime, intrinsic, arguments, argument_count, span, call_node, environment);
  if ((intrinsic >= CHIMI_INTRINSIC_BYTES_FROM && intrinsic <= CHIMI_INTRINSIC_EMBED_BYTES) ||
      intrinsic == CHIMI_INTRINSIC_HTTP_COUNT_ENDS)
    return invoke_bytes_operation(intrinsic, arguments, argument_count, call_node);
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
   carrier's) with the process-wide peaks footprint_fold kept, raised to the
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
  footprint_record_peak();
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
