#ifndef CHIMI_FIBER_H
#define CHIMI_FIBER_H

#include <stddef.h>
#include <stdint.h>

#define CHIMI_FIBER_ABI_VERSION 5u
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
  /* Kinds 13 to 23 take a ChimiProcessRequest. */
  CHIMI_REQUEST_PROCESS_SELF = 13,
  CHIMI_REQUEST_PROCESS_SEND = 14,
  CHIMI_REQUEST_PROCESS_RECEIVE = 15,
  CHIMI_REQUEST_SUPERVISOR_START = 16,
  CHIMI_REQUEST_SUPERVISOR_START_PROCESS = 17,
  CHIMI_REQUEST_SUPERVISOR_ADDRESS = 18,
  CHIMI_REQUEST_SUPERVISOR_REMOVE_PROCESS = 19,
  /* `supervisor` is a root or a nested supervisor. Stopping a nested one
     ends its membership without a restart. */
  CHIMI_REQUEST_SUPERVISOR_STOP = 20,
  CHIMI_REQUEST_SUPERVISOR_WAIT = 21,
  /* `supervisor` is the parent, `policy` and `restart` describe the dynamic
     nested supervisor. OK replies carry its handle in `membership` and the
     nested supervisor's identity in `supervisor`. */
  CHIMI_REQUEST_SUPERVISOR_START_SUPERVISOR = 22,
  /* `supervisor` is the parent and `membership` the handle from
     START_SUPERVISOR. Parks until the nested subtree has stopped. */
  CHIMI_REQUEST_SUPERVISOR_REMOVE_SUPERVISOR = 23,
  /* ChimiSchedulerRequest: `channel` is a channel id from
     CHIMI_REQUEST_CHANNEL_BOUNDED. Any task may issue it, once, when the host
     holds no reference to any of the channel's endpoints any more. Replies
     OK. See ChimiValueRelease for what happens to the values the channel
     holds; a task parked on it is answered as if the channel were aborted.
     The id is invalid afterwards: any channel request naming it, a second
     release included, is a contract violation that defects the caller. */
  CHIMI_REQUEST_CHANNEL_RELEASE = 24,
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
  /* `supervisor`; for a nested supervisor also `parent_supervisor` and
     `membership` (0 for a root). Precedes its members' starts. */
  CHIMI_PROCESS_TRACE_SUPERVISOR_STARTED = 10,
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

/* `outcome` of a terminated event whose cause is defected: 0, or why the
   scheduler saw the body end without returning from its entry. */
enum {
  CHIMI_PROCESS_DEFECT_STACK_OVERFLOW = 1,
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

/* One task parked in a detected deadlock and the request kind it waits on
   (0 for a bare park). */
typedef struct {
  uint64_t task;
  uint32_t request_kind;
} ChimiDeadlockWaiter;

typedef void (*ChimiDeadlockReport)(void *context,
                                    const ChimiDeadlockWaiter *waiters,
                                    size_t count);

enum {
  CHIMI_SCHEDULER_CLOCK_REAL = 0,
  CHIMI_SCHEDULER_CLOCK_VIRTUAL = 1,
};

/* Releases one host word the scheduler took over and will never hand back.

   Ownership. The scheduler takes over a word at these points:
   - A task value: the `value` of a CHIMI_FIBER_SUCCEEDED, or the
     `defect_code` of a CHIMI_FIBER_DEFECTED, that a task's fiber returns
     from its entry (or from its cleanup after a cancel). Process bodies are
     not tasks here: their end codes belong to the kernel. Scheduler-made
     defect codes (deadlock, capability violations) are not host words and
     are never released. Every word a task returns reaches the hook, so a
     host whose tasks return both owned and plain words must tell them
     apart itself, for example by a tag bit.
   - A channel value: the `value` of a CHIMI_REQUEST_CHANNEL_SEND once the
     reply is OK or PENDING. A reply of ERROR leaves it with the sender.

   It leaves the scheduler exactly once, by one of:
   - delivery: a CHIMI_REQUEST_CHANNEL_RECEIVE reply of SUCCEEDED hands a
     channel value to the receiver;
   - return: a CHIMI_REQUEST_CHANNEL_SEND reply of CLOSED or ABORTED hands
     the value back to its sender;
   - the outcome: chimi_scheduler_execute_with_config or
     chimi_scheduler_run returns the root's result, or the defect code of the task the outcome's `origin` names,
     and that word passes to the caller;
   - release: this hook, called once per word.

   Delivery and return happen when the scheduler writes the reply into the
   request record, not when the fiber reads it. A fiber resumed with
   CHIMI_FIBER_CANCEL still owns the value its record's last reply carries
   (SUCCEEDED for a receive, CLOSED or ABORTED for a send) and must free it.
   A record still saying PENDING carries nothing the fiber owns: a parked
   send's value stays the scheduler's.

   A CHIMI_REQUEST_TASK_RESULT reply copies a task value without consuming
   it (Task.result may be asked again), so every task value except the one
   the outcome carries is released, whether it was observed or not, after
   the root is terminal and before the call returns. A channel value is
   released when it is discarded: its parked send is cancelled, its channel
   is aborted or released (CHIMI_REQUEST_CHANNEL_RELEASE) while it is
   queued, or the task it was handed to (a receiver, or a refused sender) is
   cancelled before the reply is written. A release request releases each
   queued value before it replies; values already delivered are never
   released. Releasing a channel answers a parked send ABORTED, which hands
   its value back, and a parked receive ABORTED. Any value still held when
   the call returns is released then.

   The hook runs on the thread that called the scheduler, on that thread's
   own stack: never inside a task fiber, and with no task
   running, so it must not assume any task's heap is current. It must not
   call back into the scheduler. */
typedef void (*ChimiValueRelease)(void *context, int64_t value);

/* Releases the `context` of a CHIMI_REQUEST_TASK_START whose task never runs
   its entry.

   The scheduler takes over the context when it replies OK to the start; a
   reply of ERROR leaves it with the starter. Once the task's entry runs, the
   context belongs to the entry. A task can end before its entry runs: it is
   cancelled, or its owner ends, while it is still queued. Its fiber then
   ends without calling the entry, and this hook receives the context once;
   no entry ever sees it. A null context is never passed. The hook
   runs under the same conditions as ChimiValueRelease. */
typedef void (*ChimiTaskContextRelease)(void *context, void *task_context);

/* The arguments of chimi_scheduler_run. */
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
  /* Under the virtual clock, called once before chimi_scheduler_run returns
     when the root defected with CHIMI_SCHEDULER_DEFECT_DEADLOCK, with every
     parked task and the operation it waits on. May be null. */
  ChimiDeadlockReport deadlock_report;
  void *deadlock_context;
  /* The task and channel hooks of ChimiSchedulerExecuteConfig, with the same
     contract; both receive `release_context`. The root's value or defect
     code passes to the caller as the outcome, except when the call returns
     CHIMI_FIBER_PARKED: the suspended execution then keeps every word it
     holds. May be null. */
  ChimiValueRelease release_task_value;
  ChimiValueRelease release_channel_value;
  ChimiTaskContextRelease release_task_context;
} ChimiSchedulerConfig;

/* The arguments of chimi_scheduler_execute_with_config. The release hooks
   may be null; all three receive `release_context`. */
typedef struct {
  ChimiFiberEntry entry;
  void *context;
  size_t stack_bytes;
  uint32_t trace_mode;
  ChimiSchedulerTraceSink trace_sink;
  void *trace_context;
  ChimiValueRelease release_task_value;
  ChimiValueRelease release_channel_value;
  void *release_context;
  ChimiTaskContextRelease release_task_context;
} ChimiSchedulerExecuteConfig;

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
/* chimi_scheduler_execute with release hooks; chimi_scheduler_execute is
   this call with both hooks null. */
ChimiSchedulerOutcome chimi_scheduler_execute_with_config(
    const ChimiSchedulerExecuteConfig *config);
/* Runs the root to its outcome, then stops every supervisor still running.
   Under the real clock a root parked with no timer pending and no host
   operation in flight is not a deadlock, because a host event may still
   arrive: the call returns state CHIMI_FIBER_PARKED and leaves the
   suspended execution in place. */
ChimiSchedulerOutcome chimi_scheduler_run(const ChimiSchedulerConfig *config);

#endif
