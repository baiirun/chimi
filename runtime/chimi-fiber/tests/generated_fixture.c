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
