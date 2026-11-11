#ifndef CHIMI_FIBER_H
#define CHIMI_FIBER_H

#include <stddef.h>
#include <stdint.h>

#define CHIMI_FIBER_ABI_VERSION 3u
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

enum {
  CHIMI_SCHEDULER_TRACE_TASK = 1,
  CHIMI_SCHEDULER_TRACE_CHANNEL = 2,
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
} ChimiSchedulerTraceEvent;

typedef void (*ChimiSchedulerTraceSink)(void *context,
                                        const ChimiSchedulerTraceEvent *event);

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

#endif
