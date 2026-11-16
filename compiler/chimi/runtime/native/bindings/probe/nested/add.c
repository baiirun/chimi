/* A source in a subdirectory of the probe package: binding packages compile
   every .c file at any depth. */
#include "probe_layout.h"

int probe_nested_add(int64_t left, int64_t right, int64_t *sum) {
  return !__builtin_add_overflow(left, right, sum);
}
