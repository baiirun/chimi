/* White-box kernel scenarios of the supervised-process conformance lab
   (docs/specification/supervised-process-conformance.md: RA06 under "Runtime:
   restart accounting", SD04 under "Runtime: shutdown and forced
   termination"). It drives the scheduler's C ABI (chimi_fiber.h) directly, with
   C process bodies, and links libchimi_fiber.a.

   usage: kernel_faults <scenario ID>   runs that scenario's checks; prints "ok"
   Exits 1 on the first failed check and 2 for an unknown scenario.

   Fault injection: the scheduler allocates every fiber stack with one
   PROT_NONE anonymous mmap (corosensei's DefaultStack). This file defines mmap,
   which the static scheduler archive binds to, and fails the next
   `failing_stacks` such calls, so a chosen incarnation fails to start exactly
   as it would if its stack could not be allocated. Every other mmap goes to
   the system's. */
#define _GNU_SOURCE 1
#include <dlfcn.h>
#include <errno.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/mman.h>

#include "chimi_fiber.h"

#define CHECK(condition, ...)                                                       \
  do {                                                                              \
    if (!(condition)) {                                                             \
      fprintf(stderr, "%s:%d: check failed: %s: ", __FILE__, __LINE__, #condition); \
      fprintf(stderr, __VA_ARGS__);                                                 \
      fputc('\n', stderr);                                                          \
      exit(1);                                                                      \
    }                                                                               \
  } while (0)

typedef void *(*MmapFunction)(void *, size_t, int, int, int, off_t);
static MmapFunction system_mmap;
static int failing_stacks;

void *mmap(void *address, size_t length, int protection, int flags, int descriptor, off_t offset) {
  if (failing_stacks > 0 && protection == PROT_NONE && descriptor == -1) {
    --failing_stacks;
    errno = ENOMEM;
    return MAP_FAILED;
  }
  return system_mmap(address, length, protection, flags, descriptor, offset);
}

/* The process events the scheduler emitted, in order (supervisor_started, kind
   10, is left out, as in the lab's transcripts before U9). */
typedef struct {
  uint32_t kind;
  uint64_t membership;
  uint64_t incarnation;
  uint64_t cause;
  uint64_t attempts;
} Event;

static Event events[64];
static size_t event_count;

static void record_event(void *context, const ChimiSchedulerTraceEvent *event) {
  (void)context;
  if (event->family != CHIMI_SCHEDULER_TRACE_PROCESS || event->kind == CHIMI_PROCESS_TRACE_SUPERVISOR_STARTED)
    return;
  CHECK(event_count < sizeof(events) / sizeof(*events), "more than %zu process events", event_count);
  events[event_count++] = (Event){event->kind, event->membership, event->incarnation, event->cause,
                                  event->attempts_in_window};
}

static void release_message(void *context, uint64_t message) {
  (void)context;
  (void)message;
}

static void release_spec_context(void *context, void *spec_context) {
  (void)context;
  (void)spec_context;
}

/* Issues a process request, reissuing it while the scheduler answers PENDING.
   CHIMI_RESPONSE_CANCELLED when the fiber is cancelled instead. */
static uint32_t process_request(ChimiProcessRequest *request) {
  for (;;) {
    request->status = CHIMI_RESPONSE_UNSET;
    if (chimi_fiber_request(request, NULL) == CHIMI_FIBER_CANCEL) return CHIMI_RESPONSE_CANCELLED;
    if (request->status != CHIMI_RESPONSE_PENDING) return request->status;
  }
}

static void sleep_for(int64_t nanoseconds) {
  ChimiSchedulerRequest now = {.kind = CHIMI_REQUEST_TIMER_NOW};
  CHECK(chimi_fiber_request(&now, NULL) == CHIMI_FIBER_RESUME && now.status == CHIMI_RESPONSE_OK, "TIMER_NOW");
  ChimiSchedulerRequest sleep = {.kind = CHIMI_REQUEST_TIMER_SLEEP_UNTIL, .value = now.value + nanoseconds};
  for (;;) {
    sleep.status = CHIMI_RESPONSE_UNSET;
    CHECK(chimi_fiber_request(&sleep, NULL) == CHIMI_FIBER_RESUME, "sleep cancelled");
    if (sleep.status != CHIMI_RESPONSE_PENDING) break;
  }
  CHECK(sleep.status == CHIMI_RESPONSE_OK, "sleep answered %u", sleep.status);
}

/* A body that defects on its first message. */
static ChimiFiberResult crash_on_message(void *context) {
  (void)context;
  ChimiProcessRequest receive = {.kind = CHIMI_REQUEST_PROCESS_RECEIVE};
  uint32_t status = process_request(&receive);
  if (status != CHIMI_RESPONSE_SUCCEEDED) return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
  return (ChimiFiberResult){.state = CHIMI_FIBER_DEFECTED, .defect_code = 1};
}

enum { WORKER_NAME = 1 };

static void send_crash(void) {
  ChimiProcessRequest send = {
    .kind = CHIMI_REQUEST_PROCESS_SEND,
    .address_kind = CHIMI_ADDRESS_NAMED,
    .address = WORKER_NAME,
    .message = 7,
  };
  CHECK(process_request(&send) == CHIMI_RESPONSE_OK, "send answered %u", send.status);
}

/* RA06: one OnDefect member under restarts(3, 30s). Its first defect charges
   attempt 1, whose replacement fails to start (its stack cannot be
   allocated): `admission_rejected reason=start_failed`. The failed start is a
   defect of the membership, so the supervisor tries again and charges attempt
   2, which starts. The member's next defect is then attempt 3: the failed
   attempt still counts in the window. */
static ChimiFiberResult ra06_root(void *context) {
  (void)context;
  ChimiSupervisorMember member = {
    .kind = CHIMI_MEMBER_PROCESS,
    .restart = CHIMI_RESTART_ON_DEFECT,
    .process = {
      .entry = crash_on_message,
      .context = &member,
      .mailbox_capacity = 1,
      .shutdown_within = INT64_C(1000000000),
      .name = WORKER_NAME,
    },
  };
  ChimiSupervisorPolicy policy = {
    .members = &member,
    .member_count = 1,
    .capacity = 1,
    .restart_limit = 3,
    .restart_within = INT64_C(30000000000),
  };
  ChimiProcessRequest start = {.kind = CHIMI_REQUEST_SUPERVISOR_START, .policy = &policy};
  CHECK(process_request(&start) == CHIMI_RESPONSE_OK, "start answered %u", start.status);
  failing_stacks = 1;
  send_crash();
  sleep_for(1000000);
  CHECK(failing_stacks == 0, "the replacement's stack was never allocated");
  send_crash();
  sleep_for(1000000);
  ChimiProcessRequest stop = {.kind = CHIMI_REQUEST_SUPERVISOR_STOP, .supervisor = start.supervisor};
  CHECK(process_request(&stop) == CHIMI_RESPONSE_OK, "stop answered %u", stop.status);
  return (ChimiFiberResult){.state = CHIMI_FIBER_SUCCEEDED, .value = 0};
}

static void ra06(void) {
  ChimiSchedulerConfig config = {
    .entry = ra06_root,
    .stack_bytes = 256 * 1024,
    .trace_mode = 1,
    .clock_mode = CHIMI_SCHEDULER_CLOCK_VIRTUAL,
    .trace_sink = record_event,
    .release_message = release_message,
    .release_spec_context = release_spec_context,
  };
  ChimiSchedulerOutcome outcome = chimi_scheduler_run(&config);
  CHECK(outcome.state == CHIMI_FIBER_SUCCEEDED, "root ended in state %u (defect %lld)", outcome.state,
        (long long)outcome.defect_code);
  const Event expected[] = {
    {CHIMI_PROCESS_TRACE_STARTED, 1, 1, 0, 0},
    {CHIMI_PROCESS_TRACE_TERMINATED, 1, 1, CHIMI_PROCESS_CAUSE_DEFECTED, 0},
    {CHIMI_PROCESS_TRACE_RESTART_ATTEMPTED, 1, 0, 0, 1},
    {CHIMI_PROCESS_TRACE_ADMISSION_REJECTED, 0, 0, CHIMI_ADMISSION_START_FAILED, 0},
    {CHIMI_PROCESS_TRACE_RESTART_ATTEMPTED, 1, 0, 0, 2},
    {CHIMI_PROCESS_TRACE_STARTED, 1, 2, 0, 0},
    {CHIMI_PROCESS_TRACE_TERMINATED, 1, 2, CHIMI_PROCESS_CAUSE_DEFECTED, 0},
    {CHIMI_PROCESS_TRACE_RESTART_ATTEMPTED, 1, 0, 0, 3},
    {CHIMI_PROCESS_TRACE_STARTED, 1, 3, 0, 0},
    {CHIMI_PROCESS_TRACE_TERMINATED, 1, 3, CHIMI_PROCESS_CAUSE_CANCELLED, 0},
    {CHIMI_PROCESS_TRACE_SUPERVISOR_STOPPED, 0, 0, 0, 0},
  };
  size_t count = sizeof(expected) / sizeof(*expected);
  size_t index = 0;
  for (size_t seen = 0; seen < event_count; ++seen) {
    const Event *actual = &events[seen];
    if (actual->kind == CHIMI_PROCESS_TRACE_SEND) continue;
    CHECK(index < count, "unexpected event %zu: kind %u", seen, actual->kind);
    const Event *want = &expected[index++];
    CHECK(actual->kind == want->kind, "event %zu: kind %u, expected %u", index, actual->kind, want->kind);
    if (want->membership)
      CHECK(actual->membership == want->membership, "event %zu: membership %llu, expected %llu", index,
            (unsigned long long)actual->membership, (unsigned long long)want->membership);
    if (want->incarnation)
      CHECK(actual->incarnation == want->incarnation, "event %zu: incarnation %llu, expected %llu", index,
            (unsigned long long)actual->incarnation, (unsigned long long)want->incarnation);
    if (want->cause)
      CHECK(actual->cause == want->cause, "event %zu: cause %llu, expected %llu", index,
            (unsigned long long)actual->cause, (unsigned long long)want->cause);
    if (want->kind == CHIMI_PROCESS_TRACE_RESTART_ATTEMPTED)
      CHECK(actual->attempts == want->attempts, "event %zu: attempts_in_window %llu, expected %llu", index,
            (unsigned long long)actual->attempts, (unsigned long long)want->attempts);
  }
  CHECK(index == count, "%zu process events, expected %zu", index, count);
}

/* SD04: cleanup that parks again after cancellation. */
static int sd04_forced_replies;
static int sd04_ran_after_forced;

static int64_t now(void) {
  ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_TIMER_NOW};
  CHECK(chimi_fiber_request(&request, NULL) == CHIMI_FIBER_RESUME && request.status == CHIMI_RESPONSE_OK,
        "TIMER_NOW");
  return request.value;
}

/* Receives until the removal cancels it, then ignores the cancellation and
   sleeps far beyond its deadline. The deadline answers that sleep FORCED; the
   body then parks once more instead of unwinding. The scheduler must abandon
   it there, neither resuming it (the counter after the park stays 0) nor
   destroying its parked fiber, which would abort the process. */
static ChimiFiberResult sd04_stubborn(void *context) {
  (void)context;
  for (;;) {
    ChimiProcessRequest receive = {.kind = CHIMI_REQUEST_PROCESS_RECEIVE};
    if (process_request(&receive) != CHIMI_RESPONSE_SUCCEEDED) break;
  }
  ChimiSchedulerRequest sleep = {.kind = CHIMI_REQUEST_TIMER_SLEEP_UNTIL, .value = INT64_MAX};
  for (;;) {
    sleep.status = CHIMI_RESPONSE_UNSET;
    chimi_fiber_request(&sleep, NULL);
    if (sleep.status == CHIMI_RESPONSE_FORCED) break;
  }
  ++sd04_forced_replies;
  chimi_fiber_park(NULL);
  ++sd04_ran_after_forced;
  return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
}

/* One dynamic member with shutdown_within 3s, removed at 1ms. The removal
   returns at the deadline, 3.001s, with the member ended `forced`. */
static ChimiFiberResult sd04_root(void *context) {
  (void)context;
  ChimiSupervisorPolicy policy = {.capacity = 1, .restart_limit = 1, .restart_within = INT64_C(30000000000)};
  ChimiProcessRequest start = {.kind = CHIMI_REQUEST_SUPERVISOR_START, .policy = &policy};
  CHECK(process_request(&start) == CHIMI_RESPONSE_OK, "start answered %u", start.status);
  ChimiProcessSpec spec = {
    .entry = sd04_stubborn,
    .context = &spec,
    .mailbox_capacity = 1,
    .shutdown_within = INT64_C(3000000000),
  };
  ChimiProcessRequest add = {
    .kind = CHIMI_REQUEST_SUPERVISOR_START_PROCESS,
    .supervisor = start.supervisor,
    .restart = CHIMI_RESTART_ON_DEFECT,
    .spec = &spec,
  };
  CHECK(process_request(&add) == CHIMI_RESPONSE_OK, "start_process answered %u", add.status);
  sleep_for(1000000);
  ChimiProcessRequest remove = {
    .kind = CHIMI_REQUEST_SUPERVISOR_REMOVE_PROCESS,
    .supervisor = start.supervisor,
    .membership = add.membership,
  };
  CHECK(process_request(&remove) == CHIMI_RESPONSE_OK, "remove answered %u", remove.status);
  CHECK(now() == INT64_C(3001000000), "the removal returned at %lld ns, not at the 3.001s deadline",
        (long long)now());
  ChimiProcessRequest stop = {.kind = CHIMI_REQUEST_SUPERVISOR_STOP, .supervisor = start.supervisor};
  CHECK(process_request(&stop) == CHIMI_RESPONSE_OK, "stop answered %u", stop.status);
  return (ChimiFiberResult){.state = CHIMI_FIBER_SUCCEEDED, .value = 0};
}

static void sd04(void) {
  ChimiSchedulerConfig config = {
    .entry = sd04_root,
    .stack_bytes = 256 * 1024,
    .trace_mode = 1,
    .clock_mode = CHIMI_SCHEDULER_CLOCK_VIRTUAL,
    .trace_sink = record_event,
    .release_message = release_message,
    .release_spec_context = release_spec_context,
  };
  ChimiSchedulerOutcome outcome = chimi_scheduler_run(&config);
  CHECK(outcome.state == CHIMI_FIBER_SUCCEEDED, "root ended in state %u (defect %lld)", outcome.state,
        (long long)outcome.defect_code);
  CHECK(sd04_forced_replies == 1, "%d forced replies, expected 1", sd04_forced_replies);
  CHECK(sd04_ran_after_forced == 0, "the body ran on after its forced reply");
  const uint32_t expected[] = {
    CHIMI_PROCESS_TRACE_STARTED,
    CHIMI_PROCESS_TRACE_TERMINATED,
    CHIMI_PROCESS_TRACE_MEMBERSHIP_REMOVED,
    CHIMI_PROCESS_TRACE_SUPERVISOR_STOPPED,
  };
  size_t count = sizeof(expected) / sizeof(*expected);
  CHECK(event_count == count, "%zu process events, expected %zu", event_count, count);
  for (size_t index = 0; index < count; ++index)
    CHECK(events[index].kind == expected[index], "event %zu: kind %u, expected %u", index, events[index].kind,
          expected[index]);
  CHECK(events[1].cause == CHIMI_PROCESS_CAUSE_FORCED, "the member ended with cause %llu, not forced",
        (unsigned long long)events[1].cause);
}

int main(int argc, char **argv) {
  system_mmap = (MmapFunction)dlsym(RTLD_NEXT, "mmap");
  if (!system_mmap) {
    fputs("cannot find the system mmap\n", stderr);
    return 1;
  }
  if (argc != 2) {
    fputs("usage: kernel_faults <scenario ID>\n", stderr);
    return 2;
  }
  if (strcmp(argv[1], "RA06") == 0) ra06();
  else if (strcmp(argv[1], "SD04") == 0) sd04();
  else {
    fprintf(stderr, "unknown scenario %s\n", argv[1]);
    return 2;
  }
  puts("ok");
  return 0;
}
