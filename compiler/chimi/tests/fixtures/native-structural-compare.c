/* White-box test of the native runtime's structural comparison. It includes the
   runtime source so chimi_value_compare is reachable, and supplies an empty
   program so the runtime links without generated program data.
   Exits nonzero on the first failure and prints "ok" otherwise. */
#include "runtime.c"

const char *CHIMI_COMPUTATION = "structural-compare-test";
const uint32_t CHIMI_REQUIRES_SCHEDULER = 0;
const char *CHIMI_STRINGS[] = {""};
const uint32_t CHIMI_STRING_LENGTHS[] = {0};
const uint32_t CHIMI_IDS[] = {0};
const ChimiOrigin CHIMI_ORIGINS[] = {{0}};
const ChimiNode CHIMI_NODES[] = {{0}};
const uint32_t CHIMI_NODE_COUNT = 0;
const ChimiSymbol CHIMI_SYMBOLS[] = {{0}};
const uint32_t CHIMI_SYMBOL_COUNT = 0;
const uint32_t CHIMI_CLOCK_REQUIREMENT_SLOT = 0;
const uint32_t CHIMI_REQUIREMENT_SLOT_COUNT = 1;
const ChimiFunction CHIMI_FUNCTIONS[] = {{0}};
const uint32_t CHIMI_FUNCTION_COUNT = 0;
const uint32_t CHIMI_TEST_FUNCTIONS[] = {0};
const uint32_t CHIMI_TEST_COUNT = 0;
const ChimiConstructor CHIMI_CONSTRUCTORS[] = {{0}};
const ChimiNamedNode CHIMI_NAMED_NODES[] = {{0}};
const ChimiStep CHIMI_STEPS[] = {{0}};
const ChimiArm CHIMI_ARMS[] = {{0}};
const ChimiPattern CHIMI_PATTERNS[] = {{0}};
ChimiNodeFn CHIMI_NODE_FUNCTIONS[] = {NULL};
const ChimiConstantValue CHIMI_CONSTANT_VALUES[] = {{0}};
const uint32_t CHIMI_CONSTANT_VALUE_COUNT = 0;

#define CHECK(condition, ...)                                         \
  do {                                                                \
    if (!(condition)) {                                               \
      fprintf(stderr, "%s:%d: check failed: %s: ", __FILE__, __LINE__, #condition); \
      fprintf(stderr, __VA_ARGS__);                                   \
      fputc('\n', stderr);                                            \
      exit(1);                                                        \
    }                                                                 \
  } while (0)

static ChimiValue *nested(uint32_t depth, int64_t leaf) {
  ChimiValue *value = integer_value(leaf);
  for (uint32_t level = 0; level < depth; ++level) value = list_from_values(&value, 1);
  return value;
}

int main(void) {
  /* E18: a million levels cost heap, never host stack. */
  ChimiValue *left = nested(1000000, 1), *same = nested(1000000, 1), *larger = nested(1000000, 2);
  CHECK(chimi_value_compare(left, same) == 0, "equal deep lists");
  CHECK(chimi_value_compare(left, larger) < 0, "deep lists differing at the leaf");
  CHECK(chimi_value_compare(larger, left) > 0, "deep lists differing at the leaf, reversed");

  /* E15/E16: numbers compare by the exact decimal they denote, as in json.rs. */
  static const struct { const char *left, *right; int order; } numbers[] = {
    {"1", "1.0", 0}, {"1", "10e-1", 0}, {"100", "1E2", 0}, {"1.5", "1.50", 0},
    {"-0", "0", 0}, {"0.000", "-0e5", 0}, {"2", "10", -1}, {"0.25", "0.3", -1},
    {"-2", "-10", 1}, {"-1", "0", -1}, {"1e400", "1e399", 1},
    {"12345678901234567890123", "12345678901234567890124", -1},
    {"0.1", "0.09999999999999999999", 1},
    /* Exponents beyond any machine integer stay exact and never overflow. */
    {"1e9999999999999999999", "1", 1},
    {"1e9999999999999999999", "1e9999999999999999998", 1},
    {"1e-9999999999999999999", "1e-9999999999999999998", -1},
    {"1e1152921504606846977", "1e1152921504606846976", 1},
    {"10e1152921504606846976", "1e1152921504606846977", 0},
    {"0.01e100000000000000000002", "1e100000000000000000000", 0},
    {"-1e99999999999999999999999", "-1e99999999999999999999998", -1},
    {"1e-00000000000000000000001", "0.1", 0},
    {"0e99999999999999999999999", "-0", 0},
    {"1e9223372036854775807", "10e9223372036854775806", 0},
    {"1e9223372036854775808", "1e9223372036854775807", 1},
    {"0.001e-9223372036854775807", "1e-9223372036854775810", 0},
    {"100e9999999999999999999", "1e10000000000000000001", 0},
    {"0.001e10000000000000000000", "1e9999999999999999997", 0},
  };
  for (size_t index = 0; index < sizeof(numbers) / sizeof(numbers[0]); ++index) {
    int order = json_number_compare(numbers[index].left, numbers[index].right);
    CHECK(order == numbers[index].order, "%s vs %s gave %d", numbers[index].left, numbers[index].right, order);
    order = json_number_compare(numbers[index].right, numbers[index].left);
    CHECK(order == -numbers[index].order, "%s vs %s gave %d", numbers[index].right, numbers[index].left, order);
  }
  puts("ok");
  return 0;
}
