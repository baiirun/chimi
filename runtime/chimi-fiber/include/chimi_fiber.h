#ifndef CHIMI_FIBER_H
#define CHIMI_FIBER_H

#include <stddef.h>
#include <stdint.h>

#define CHIMI_FIBER_ABI_VERSION 4u
#define CHIMI_FIBER_MIN_STACK_BYTES (32u * 1024u)
#define CHIMI_FIBER_MAX_STACK_BYTES (8u * 1024u * 1024u)

typedef enum {
  CHIMI_FIBER_RESUME = 1,
  CHIMI_FIBER_CANCEL = 2,
} ChimiFiberCommand;

typedef enum {
  CHIMI_FIBER_PARKED = 1,
  CHIMI_FIBER_SUCCEEDED = 2,
  CHIMI_FIBER_CANCELLED = 3,
  CHIMI_FIBER_DEFECTED = 4,
  CHIMI_FIBER_STACK_OVERFLOW = 5,
} ChimiFiberState;

typedef struct ChimiRootFrame {
  const struct ChimiRootFrame *previous;
  uintptr_t *slots;
  size_t length;
  const char *function_name;
} ChimiRootFrame;

typedef struct {
  uint32_t state;
  int64_t value;
  int64_t defect_code;
  const ChimiRootFrame *roots;
  void *request;
} ChimiFiberEvent;

typedef struct {
  uint32_t state;
  int64_t value;
  int64_t defect_code;
} ChimiFiberResult;

typedef struct {
  size_t stack_bytes;
  uint32_t capture_backtraces;
} ChimiFiberConfig;

typedef ChimiFiberResult (*ChimiFiberEntry)(void *context);
typedef void (*ChimiRootVisitor)(void *context, uintptr_t *slot);
typedef struct ChimiFiber ChimiFiber;

enum {
  CHIMI_REQUEST_TASK_START = 1,
  CHIMI_REQUEST_TASK_RESULT = 2,
  CHIMI_REQUEST_TASK_CANCEL = 3,
  CHIMI_REQUEST_CHANNEL_BOUNDED = 4,
  CHIMI_REQUEST_CHANNEL_SEND = 5,
  CHIMI_REQUEST_CHANNEL_RECEIVE = 6,
  CHIMI_REQUEST_CHANNEL_CLOSE = 7,
  CHIMI_REQUEST_CHANNEL_ABORT = 8,
  CHIMI_REQUEST_CHANNEL_TRANSFER_RECEIVER = 9,
  CHIMI_REQUEST_CHANNEL_TRANSFER_COMPLETION = 10,
  /* ChimiSchedulerRequest: `value` receives the current Instant. */
  CHIMI_REQUEST_TIMER_NOW = 11,
  /* ChimiSchedulerRequest: `value` is the deadline Instant. PENDING until it
     passes; reissue the request after every wake. */
  CHIMI_REQUEST_TIMER_SLEEP_UNTIL = 12,
  /* The remaining kinds take a ChimiProcessRequest. */
  CHIMI_REQUEST_PROCESS_SELF = 13,
  CHIMI_REQUEST_PROCESS_SEND = 14,
  CHIMI_REQUEST_PROCESS_RECEIVE = 15,
  CHIMI_REQUEST_SUPERVISOR_START = 16,
  CHIMI_REQUEST_SUPERVISOR_START_PROCESS = 17,
  CHIMI_REQUEST_SUPERVISOR_ADDRESS = 18,
  CHIMI_REQUEST_SUPERVISOR_REMOVE_PROCESS = 19,
  CHIMI_REQUEST_SUPERVISOR_STOP = 20,
  CHIMI_REQUEST_SUPERVISOR_WAIT = 21,
};

enum {
  CHIMI_RESPONSE_UNSET = 0,
  CHIMI_RESPONSE_OK = 1,
  CHIMI_RESPONSE_PENDING = 2,
  CHIMI_RESPONSE_SUCCEEDED = 3,
  CHIMI_RESPONSE_CANCELLED = 4,
  CHIMI_RESPONSE_DEFECTED = 5,
  CHIMI_RESPONSE_STACK_OVERFLOW = 6,
  CHIMI_RESPONSE_ERROR = 7,
  CHIMI_RESPONSE_CLOSED = 8,
  CHIMI_RESPONSE_ABORTED = 9,
  CHIMI_RESPONSE_DRAINED = 10,
  CHIMI_RESPONSE_FULL = 11,
  CHIMI_RESPONSE_UNAVAILABLE = 12,
  CHIMI_RESPONSE_CAPACITY_REACHED = 13,
  CHIMI_RESPONSE_NAME_TAKEN = 14,
  CHIMI_RESPONSE_STOPPED = 15,
  /* Cleanup missed its shutdown deadline. The body must unwind to its entry
     without running further user code. */
  CHIMI_RESPONSE_FORCED = 16,
};

typedef struct {
  uint32_t kind;
  uint32_t status;
  uint64_t task;
  uint64_t channel;
  uint64_t owner;
  uint64_t token;
  int64_t value;
  ChimiFiberEntry entry;
  void *context;
} ChimiSchedulerRequest;

typedef struct {
  uint32_t state;
  int64_t value;
  uint64_t origin;
  int64_t defect_code;
} ChimiSchedulerOutcome;

/* Scheduler defect codes reported in ChimiSchedulerOutcome.defect_code. */
enum {
  CHIMI_SCHEDULER_DEFECT_DEADLOCK = -5,
  CHIMI_SCHEDULER_DEFECT_RESTART_LIMIT_EXCEEDED = -6,
};

enum {
  CHIMI_ADDRESS_NAMED = 1,
  CHIMI_ADDRESS_DIRECT = 2,
};

enum {
  CHIMI_RESTART_NEVER = 1,
  CHIMI_RESTART_ON_DEFECT = 2,
};

enum {
  CHIMI_MEMBER_PROCESS = 1,
  CHIMI_MEMBER_SUPERVISOR = 2,
};

/* A process entry returns CHIMI_FIBER_SUCCEEDED with one of these values. */
enum {
  CHIMI_PROCESS_END_RETURNED = 0,
  CHIMI_PROCESS_END_FAILED = 1,
};

/* A process specification. `context` is passed to every incarnation's entry.
   The scheduler owns the context from the request that hands it over and
   releases it once through `release_spec_context` when no record needs it. */
typedef struct {
  ChimiFiberEntry entry;
  void *context;
  uint32_t mailbox_capacity;
  int64_t shutdown_within;
  uint64_t name;
} ChimiProcessSpec;

struct ChimiSupervisorPolicy;

typedef struct {
  uint32_t kind;
  uint32_t restart;
  ChimiProcessSpec process;
  const struct ChimiSupervisorPolicy *supervisor;
} ChimiSupervisorMember;

typedef struct ChimiSupervisorPolicy {
  const ChimiSupervisorMember *members;
  size_t member_count;
  uint32_t capacity;
  uint32_t restart_limit;
  int64_t restart_within;
} ChimiSupervisorPolicy;

/* Request record for CHIMI_REQUEST_PROCESS_* and CHIMI_REQUEST_SUPERVISOR_*.
   `kind` and `status` share their offsets with ChimiSchedulerRequest. */
typedef struct {
  uint32_t kind;
  uint32_t status;
  uint32_t address_kind;
  uint32_t restart;
  uint64_t address;
  uint64_t supervisor;
  uint64_t membership;
  uint64_t message;
  uint64_t name;
  uint64_t owner;
  const ChimiSupervisorPolicy *policy;
  const ChimiProcessSpec *spec;
} ChimiProcessRequest;

enum {
  CHIMI_SCHEDULER_TRACE_TASK = 1,
  CHIMI_SCHEDULER_TRACE_CHANNEL = 2,
  CHIMI_SCHEDULER_TRACE_PROCESS = 3,
  CHIMI_SCHEDULER_TRACE_CLOCK = 4,
};

enum {
  CHIMI_PROCESS_TRACE_STARTED = 1,
  CHIMI_PROCESS_TRACE_ADMISSION_REJECTED = 2,
  CHIMI_PROCESS_TRACE_TERMINATED = 3,
  CHIMI_PROCESS_TRACE_RESTART_ATTEMPTED = 4,
  CHIMI_PROCESS_TRACE_RESTART_LIMIT_EXCEEDED = 5,
  CHIMI_PROCESS_TRACE_SUPERVISOR_ESCALATED = 6,
  CHIMI_PROCESS_TRACE_MEMBERSHIP_REMOVED = 7,
  CHIMI_PROCESS_TRACE_SUPERVISOR_STOPPED = 8,
  CHIMI_PROCESS_TRACE_SEND = 9,
};

/* `cause` of a terminated event. */
enum {
  CHIMI_PROCESS_CAUSE_RETURNED = 1,
  CHIMI_PROCESS_CAUSE_FAILED = 2,
  CHIMI_PROCESS_CAUSE_DEFECTED = 3,
  CHIMI_PROCESS_CAUSE_CANCELLED = 4,
  CHIMI_PROCESS_CAUSE_FORCED = 5,
};

/* `cause` of an admission-rejected event. */
enum {
  CHIMI_ADMISSION_CAPACITY_REACHED = 1,
  CHIMI_ADMISSION_NAME_TAKEN = 2,
  CHIMI_ADMISSION_STOPPED = 3,
  CHIMI_ADMISSION_START_FAILED = 4,
};

/* `outcome` of a send event. */
enum {
  CHIMI_SEND_ACCEPTED = 1,
  CHIMI_SEND_FULL = 2,
  CHIMI_SEND_UNAVAILABLE = 3,
};

enum {
  CHIMI_CLOCK_TRACE_ADVANCED = 1,
  CHIMI_CLOCK_TRACE_TIMER_STARTED = 2,
  CHIMI_CLOCK_TRACE_TIMER_FIRED = 3,
  CHIMI_CLOCK_TRACE_TIMER_CANCELLED = 4,
  CHIMI_CLOCK_TRACE_DEADLOCK_WAITER = 5,
};

typedef struct {
  uint32_t family;
  uint32_t kind;
  uint64_t sequence;
  uint64_t task;
  uint64_t parent;
  uint64_t cause;
  uint64_t channel;
  uint64_t capacity;
  uint64_t queue_depth;
  uint64_t waiter_depth;
  uint64_t discard_count;
  /* Process and clock families. Identities are 0 when absent. */
  uint64_t supervisor;
  uint64_t membership;
  uint64_t incarnation;
  uint64_t name;
  uint64_t parent_supervisor;
  uint64_t attempts_in_window;
  int64_t time;
  uint32_t address_kind;
  uint32_t outcome;
} ChimiSchedulerTraceEvent;

typedef void (*ChimiSchedulerTraceSink)(void *context,
                                        const ChimiSchedulerTraceEvent *event);
typedef void (*ChimiMessageRelease)(void *context, uint64_t message);
typedef void (*ChimiSpecContextRelease)(void *context, void *spec_context);

enum {
  CHIMI_SCHEDULER_CLOCK_REAL = 0,
  CHIMI_SCHEDULER_CLOCK_VIRTUAL = 1,
};

typedef struct {
  ChimiFiberEntry entry;
  void *context;
  size_t stack_bytes;
  uint32_t trace_mode;
  uint32_t clock_mode;
  ChimiSchedulerTraceSink trace_sink;
  void *trace_context;
  /* Called once for each accepted message the scheduler discards unread. */
  ChimiMessageRelease release_message;
  ChimiSpecContextRelease release_spec_context;
  void *release_context;
} ChimiSchedulerConfig;

uint32_t chimi_fiber_abi_version(void);
ChimiFiber *chimi_fiber_create(ChimiFiberEntry entry, void *context,
                               ChimiFiberConfig config);
ChimiFiberEvent chimi_fiber_resume(ChimiFiber *fiber, uint32_t command);
void chimi_fiber_visit_roots(const ChimiFiber *fiber, ChimiRootVisitor visitor,
                             void *context);
size_t chimi_fiber_backtrace(const ChimiFiber *fiber, char *output,
                             size_t capacity);
void chimi_fiber_destroy(ChimiFiber *fiber);

ChimiFiberCommand chimi_fiber_park(const ChimiRootFrame *roots);
ChimiFiberCommand chimi_fiber_request(void *request, const ChimiRootFrame *roots);
ChimiSchedulerOutcome chimi_scheduler_execute(ChimiFiberEntry entry, void *context,
                                              size_t stack_bytes,
                                              uint32_t trace_mode,
                                              ChimiSchedulerTraceSink trace_sink,
                                              void *trace_context);
ChimiSchedulerOutcome chimi_scheduler_run(const ChimiSchedulerConfig *config);

#endif
