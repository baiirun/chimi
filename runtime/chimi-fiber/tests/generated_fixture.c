#include "chimi_fiber.h"

static int cleanup_count = 0;

__attribute__((noinline)) static ChimiFiberResult
chimi_fixture_nested_call(uintptr_t input, const ChimiRootFrame *previous) {
  uintptr_t live_value = input + 2;
  ChimiRootFrame roots = {
      .previous = previous,
      .slots = &live_value,
      .length = 1,
      .function_name = "chimi_fixture_nested_call",
  };
  ChimiFiberCommand command = chimi_fiber_park(&roots);
  cleanup_count += 1;
  if (command == CHIMI_FIBER_CANCEL) {
    return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
  }
  return (ChimiFiberResult){
      .state = CHIMI_FIBER_SUCCEEDED,
      .value = (int64_t)live_value,
  };
}

__attribute__((noinline)) ChimiFiberResult
chimi_fixture_generated_entry(void *context) {
  (void)context;
  ChimiRootFrame entry = {
      .previous = 0,
      .slots = 0,
      .length = 0,
      .function_name = "chimi_fixture_generated_entry",
  };
  return chimi_fixture_nested_call(40, &entry);
}

__attribute__((noinline)) ChimiFiberResult
chimi_fixture_generated_defect(void *context) {
  (void)context;
  cleanup_count += 1;
  return (ChimiFiberResult){
      .state = CHIMI_FIBER_DEFECTED,
      .defect_code = 77,
  };
}

__attribute__((noinline)) ChimiFiberResult
chimi_fixture_generated_expected_failure_value(void *context) {
  (void)context;
  return (ChimiFiberResult){
      .state = CHIMI_FIBER_SUCCEEDED,
      .value = -5,
  };
}

__attribute__((noinline)) ChimiFiberResult
chimi_fixture_generated_cancel_defect(void *context) {
  (void)context;
  ChimiFiberCommand command = chimi_fiber_park(0);
  if (command == CHIMI_FIBER_CANCEL) {
    return (ChimiFiberResult){
        .state = CHIMI_FIBER_DEFECTED,
        .defect_code = 88,
    };
  }
  return (ChimiFiberResult){.state = CHIMI_FIBER_SUCCEEDED};
}

__attribute__((noinline)) ChimiFiberResult
chimi_fixture_generated_many_parks(void *context) {
  (void)context;
  for (int64_t park_count = 0; park_count < 131072; park_count += 1) {
    ChimiFiberCommand command = chimi_fiber_park(0);
    if (command == CHIMI_FIBER_CANCEL) {
      return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
    }
  }
  return (ChimiFiberResult){
      .state = CHIMI_FIBER_SUCCEEDED,
      .value = 131072,
  };
}

typedef ChimiFiberResult (*ChimiOverflowCall)(uintptr_t depth);

__attribute__((noinline)) static ChimiFiberResult
chimi_fixture_overflow_recursive(uintptr_t depth);

static ChimiOverflowCall volatile chimi_fixture_overflow_next =
    chimi_fixture_overflow_recursive;

__attribute__((noinline)) static ChimiFiberResult
chimi_fixture_overflow_recursive(uintptr_t depth) {
  volatile uintptr_t stack_words[128] = {depth};
  ChimiFiberResult result = chimi_fixture_overflow_next(depth + 1);
  if (stack_words[0] == UINTPTR_MAX) {
    cleanup_count += 1;
  }
  return result;
}

__attribute__((noinline)) ChimiFiberResult
chimi_fixture_generated_overflow(void *context) {
  (void)context;
  return chimi_fixture_overflow_recursive(0);
}

void chimi_fixture_reset_cleanup(void) { cleanup_count = 0; }
int chimi_fixture_cleanup_count(void) { return cleanup_count; }

/* ----- Supervised-process fixtures ------------------------------------- */

#include <stddef.h>

#define FIXTURE_MS 1000000LL
#define FIXTURE_S 1000000000LL
#define FIXTURE_CRASH 0u

static int64_t fixture_now(void) {
  ChimiSchedulerRequest request = {.kind = CHIMI_REQUEST_TIMER_NOW};
  chimi_fiber_request(&request, 0);
  return request.value;
}

/* Returns the final status of a sleep, reissuing it after every wake. */
static uint32_t fixture_sleep_until(int64_t deadline) {
  for (;;) {
    ChimiSchedulerRequest request = {
        .kind = CHIMI_REQUEST_TIMER_SLEEP_UNTIL,
        .value = deadline,
    };
    ChimiFiberCommand command = chimi_fiber_request(&request, 0);
    if (command == CHIMI_FIBER_CANCEL) return request.status;
    if (request.status == CHIMI_RESPONSE_PENDING) continue;
    return request.status;
  }
}

static uint32_t fixture_send_named(uint64_t name, uint64_t message) {
  ChimiProcessRequest request = {
      .kind = CHIMI_REQUEST_PROCESS_SEND,
      .address_kind = CHIMI_ADDRESS_NAMED,
      .address = name,
      .message = message,
  };
  chimi_fiber_request(&request, 0);
  return request.status;
}

/* Receives until cancellation. Message FIXTURE_CRASH defects. */
static ChimiFiberResult fixture_counter(void *context) {
  (void)context;
  for (;;) {
    ChimiProcessRequest request = {.kind = CHIMI_REQUEST_PROCESS_RECEIVE};
    ChimiFiberCommand command = chimi_fiber_request(&request, 0);
    if (command == CHIMI_FIBER_CANCEL) {
      return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
    }
    if (request.status == CHIMI_RESPONSE_PENDING) continue;
    if (request.status != CHIMI_RESPONSE_SUCCEEDED) {
      return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
    }
    if (request.message == FIXTURE_CRASH) {
      return (ChimiFiberResult){.state = CHIMI_FIBER_DEFECTED, .defect_code = 9};
    }
  }
}

static ChimiProcessSpec fixture_spec(ChimiFiberEntry entry, uint64_t name,
                                     int64_t shutdown_within) {
  return (ChimiProcessSpec){
      .entry = entry,
      .context = 0,
      .mailbox_capacity = 8,
      .shutdown_within = shutdown_within,
      .name = name,
  };
}

/* ES01: counter and peer under restarts(1, 30s). Main sends Crash to the
   counter, settles, sends Crash again, and waits on the root. */
ChimiFiberResult chimi_fixture_es01_main(void *context) {
  (void)context;
  ChimiSupervisorMember members[2] = {
      {.kind = CHIMI_MEMBER_PROCESS,
       .restart = CHIMI_RESTART_ON_DEFECT,
       .process = fixture_spec(fixture_counter, 1, 5 * FIXTURE_S)},
      {.kind = CHIMI_MEMBER_PROCESS,
       .restart = CHIMI_RESTART_ON_DEFECT,
       .process = fixture_spec(fixture_counter, 2, 5 * FIXTURE_S)},
  };
  ChimiSupervisorPolicy policy = {
      .members = members,
      .member_count = 2,
      .capacity = 16,
      .restart_limit = 1,
      .restart_within = 30 * FIXTURE_S,
  };
  ChimiProcessRequest start = {.kind = CHIMI_REQUEST_SUPERVISOR_START, .policy = &policy};
  if (chimi_fiber_request(&start, 0) == CHIMI_FIBER_CANCEL ||
      start.status != CHIMI_RESPONSE_OK) {
    return (ChimiFiberResult){.state = CHIMI_FIBER_DEFECTED, .defect_code = 1};
  }
  if (fixture_send_named(1, FIXTURE_CRASH) != CHIMI_RESPONSE_OK) {
    return (ChimiFiberResult){.state = CHIMI_FIBER_DEFECTED, .defect_code = 2};
  }
  fixture_sleep_until(fixture_now() + FIXTURE_MS);
  if (fixture_send_named(1, FIXTURE_CRASH) != CHIMI_RESPONSE_OK) {
    return (ChimiFiberResult){.state = CHIMI_FIBER_DEFECTED, .defect_code = 3};
  }
  ChimiProcessRequest wait = {
      .kind = CHIMI_REQUEST_SUPERVISOR_WAIT,
      .supervisor = start.supervisor,
  };
  if (chimi_fiber_request(&wait, 0) == CHIMI_FIBER_CANCEL) {
    return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
  }
  /* A wait that returns is wrong: the escalation terminates the program. */
  return (ChimiFiberResult){.state = CHIMI_FIBER_SUCCEEDED, .value = 1};
}

static int sd04_ran_after_forced = 0;
static int sd04_forced_replies = 0;

/* Cleanup that ignores cancellation: it sleeps again, and after the forced
   reply it parks once more instead of unwinding. */
static ChimiFiberResult sd04_stubborn(void *context) {
  (void)context;
  for (;;) {
    ChimiProcessRequest request = {.kind = CHIMI_REQUEST_PROCESS_RECEIVE};
    ChimiFiberCommand command = chimi_fiber_request(&request, 0);
    if (command == CHIMI_FIBER_CANCEL || request.status == CHIMI_RESPONSE_CANCELLED) break;
  }
  for (;;) {
    if (fixture_sleep_until(INT64_MAX) == CHIMI_RESPONSE_FORCED) {
      sd04_forced_replies += 1;
      chimi_fiber_park(0);
      sd04_ran_after_forced += 1;
      return (ChimiFiberResult){.state = CHIMI_FIBER_CANCELLED};
    }
  }
}

/* SD04: one stubborn member with shutdown_within 5s. Main settles, stops the
   root, and returns the instant at which the stop returned. */
ChimiFiberResult chimi_fixture_sd04_main(void *context) {
  (void)context;
  ChimiSupervisorMember member = {
      .kind = CHIMI_MEMBER_PROCESS,
      .restart = CHIMI_RESTART_ON_DEFECT,
      .process = fixture_spec(sd04_stubborn, 0, 5 * FIXTURE_S),
  };
  ChimiSupervisorPolicy policy = {
      .members = &member,
      .member_count = 1,
      .capacity = 1,
      .restart_limit = 0,
      .restart_within = FIXTURE_S,
  };
  ChimiProcessRequest start = {.kind = CHIMI_REQUEST_SUPERVISOR_START, .policy = &policy};
  chimi_fiber_request(&start, 0);
  if (start.status != CHIMI_RESPONSE_OK) {
    return (ChimiFiberResult){.state = CHIMI_FIBER_DEFECTED, .defect_code = 1};
  }
  fixture_sleep_until(fixture_now() + FIXTURE_MS);
  ChimiProcessRequest stop = {
      .kind = CHIMI_REQUEST_SUPERVISOR_STOP,
      .supervisor = start.supervisor,
  };
  if (chimi_fiber_request(&stop, 0) == CHIMI_FIBER_CANCEL ||
      stop.status != CHIMI_RESPONSE_OK) {
    return (ChimiFiberResult){.state = CHIMI_FIBER_DEFECTED, .defect_code = 2};
  }
  return (ChimiFiberResult){.state = CHIMI_FIBER_SUCCEEDED, .value = fixture_now()};
}

void chimi_fixture_sd04_reset(void) {
  sd04_ran_after_forced = 0;
  sd04_forced_replies = 0;
}
int chimi_fixture_sd04_ran_after_forced(void) { return sd04_ran_after_forced; }
int chimi_fixture_sd04_forced_replies(void) { return sd04_forced_replies; }

/* Main parks with no way to be woken. */
ChimiFiberResult chimi_fixture_deadlocked_main(void *context) {
  (void)context;
  chimi_fiber_park(0);
  return (ChimiFiberResult){.state = CHIMI_FIBER_SUCCEEDED};
}

#define LAYOUT(type, field) offsetof(type, field)

/* Sizes and field offsets of the process ABI records, then the values of
   the nested-supervisor request and trace kinds, in a fixed order the Rust
   test mirrors. Returns the number written. */
size_t chimi_fixture_process_layouts(size_t *out) {
  size_t values[] = {
      sizeof(ChimiProcessRequest),
      LAYOUT(ChimiProcessRequest, status),
      LAYOUT(ChimiProcessRequest, address_kind),
      LAYOUT(ChimiProcessRequest, restart),
      LAYOUT(ChimiProcessRequest, address),
      LAYOUT(ChimiProcessRequest, supervisor),
      LAYOUT(ChimiProcessRequest, membership),
      LAYOUT(ChimiProcessRequest, message),
      LAYOUT(ChimiProcessRequest, name),
      LAYOUT(ChimiProcessRequest, owner),
      LAYOUT(ChimiProcessRequest, policy),
      LAYOUT(ChimiProcessRequest, spec),
      sizeof(ChimiProcessSpec),
      LAYOUT(ChimiProcessSpec, context),
      LAYOUT(ChimiProcessSpec, mailbox_capacity),
      LAYOUT(ChimiProcessSpec, shutdown_within),
      LAYOUT(ChimiProcessSpec, name),
      sizeof(ChimiSupervisorMember),
      LAYOUT(ChimiSupervisorMember, restart),
      LAYOUT(ChimiSupervisorMember, process),
      LAYOUT(ChimiSupervisorMember, supervisor),
      sizeof(ChimiSupervisorPolicy),
      LAYOUT(ChimiSupervisorPolicy, member_count),
      LAYOUT(ChimiSupervisorPolicy, capacity),
      LAYOUT(ChimiSupervisorPolicy, restart_limit),
      LAYOUT(ChimiSupervisorPolicy, restart_within),
      sizeof(ChimiSchedulerTraceEvent),
      LAYOUT(ChimiSchedulerTraceEvent, discard_count),
      LAYOUT(ChimiSchedulerTraceEvent, supervisor),
      LAYOUT(ChimiSchedulerTraceEvent, membership),
      LAYOUT(ChimiSchedulerTraceEvent, incarnation),
      LAYOUT(ChimiSchedulerTraceEvent, name),
      LAYOUT(ChimiSchedulerTraceEvent, parent_supervisor),
      LAYOUT(ChimiSchedulerTraceEvent, attempts_in_window),
      LAYOUT(ChimiSchedulerTraceEvent, time),
      LAYOUT(ChimiSchedulerTraceEvent, address_kind),
      LAYOUT(ChimiSchedulerTraceEvent, outcome),
      sizeof(ChimiSchedulerConfig),
      LAYOUT(ChimiSchedulerConfig, stack_bytes),
      LAYOUT(ChimiSchedulerConfig, trace_mode),
      LAYOUT(ChimiSchedulerConfig, clock_mode),
      LAYOUT(ChimiSchedulerConfig, trace_sink),
      LAYOUT(ChimiSchedulerConfig, trace_context),
      LAYOUT(ChimiSchedulerConfig, release_message),
      LAYOUT(ChimiSchedulerConfig, release_spec_context),
      LAYOUT(ChimiSchedulerConfig, release_context),
      LAYOUT(ChimiSchedulerConfig, deadlock_report),
      LAYOUT(ChimiSchedulerConfig, deadlock_context),
      LAYOUT(ChimiSchedulerConfig, release_task_value),
      LAYOUT(ChimiSchedulerConfig, release_channel_value),
      LAYOUT(ChimiSchedulerConfig, release_task_context),
      LAYOUT(ChimiSchedulerConfig, capability_violation_report),
      LAYOUT(ChimiSchedulerConfig, capability_violation_context),
      sizeof(ChimiDeadlockWaiter),
      LAYOUT(ChimiDeadlockWaiter, request_kind),
      sizeof(ChimiCapabilityViolation),
      LAYOUT(ChimiCapabilityViolation, channel),
      LAYOUT(ChimiCapabilityViolation, owner),
      LAYOUT(ChimiCapabilityViolation, destination),
      LAYOUT(ChimiCapabilityViolation, request_kind),
      LAYOUT(ChimiCapabilityViolation, capability),
      LAYOUT(ChimiCapabilityViolation, reason),
      CHIMI_VIOLATION_WRONG_OWNER,
      CHIMI_VIOLATION_CHANNEL_STILL_LIVE,
      CHIMI_CAPABILITY_RECEIVER,
      CHIMI_CAPABILITY_COMPLETION,
      CHIMI_REQUEST_SUPERVISOR_START_SUPERVISOR,
      CHIMI_REQUEST_SUPERVISOR_REMOVE_SUPERVISOR,
      CHIMI_PROCESS_TRACE_SUPERVISOR_STARTED,
  };
  size_t count = sizeof(values) / sizeof(*values);
  for (size_t index = 0; index < count; index += 1) out[index] = values[index];
  return count;
}

/* Size and field offsets of ChimiSchedulerExecuteConfig, in the order the
   Rust test mirrors. Returns the number written. */
size_t chimi_fixture_execute_config_layout(size_t *out) {
  size_t values[] = {
      sizeof(ChimiSchedulerExecuteConfig),
      LAYOUT(ChimiSchedulerExecuteConfig, context),
      LAYOUT(ChimiSchedulerExecuteConfig, stack_bytes),
      LAYOUT(ChimiSchedulerExecuteConfig, trace_mode),
      LAYOUT(ChimiSchedulerExecuteConfig, trace_sink),
      LAYOUT(ChimiSchedulerExecuteConfig, trace_context),
      LAYOUT(ChimiSchedulerExecuteConfig, release_task_value),
      LAYOUT(ChimiSchedulerExecuteConfig, release_channel_value),
      LAYOUT(ChimiSchedulerExecuteConfig, release_context),
      LAYOUT(ChimiSchedulerExecuteConfig, release_task_context),
      CHIMI_REQUEST_CHANNEL_RELEASE,
  };
  size_t count = sizeof(values) / sizeof(*values);
  for (size_t index = 0; index < count; index += 1) out[index] = values[index];
  return count;
}
