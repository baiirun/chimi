#include <stddef.h>
#include <stdint.h>

#define CHIMI_PROTOTYPE_CANCELLED INT64_MIN
#define CHIMI_PROTOTYPE_DEFECT (INT64_MIN + 1)

typedef struct {
  const int64_t *slots;
  size_t length;
} ChimiPrototypeRootFrame;

extern int64_t chimi_prototype_park(const ChimiPrototypeRootFrame *frame);

__attribute__((noinline)) int64_t chimi_prototype_nested_call(int64_t input) {
  int64_t live_value = input + 2;
  ChimiPrototypeRootFrame roots = {
      .slots = &live_value,
      .length = 1,
  };
  int64_t command = chimi_prototype_park(&roots);
  if (command == CHIMI_PROTOTYPE_CANCELLED) {
    return CHIMI_PROTOTYPE_CANCELLED;
  }
  return live_value;
}

__attribute__((noinline)) int64_t chimi_prototype_generated_child(void) {
  volatile int64_t result = chimi_prototype_nested_call(40);
  return result;
}

__attribute__((noinline)) int64_t chimi_prototype_generated_defect(void) {
  return CHIMI_PROTOTYPE_DEFECT;
}
