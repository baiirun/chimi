/* The C Float text code that runtime.c used before it moved to Rust
   (runtime/chimi-runtime/src/float.rs, CHI-307), kept as the oracle for
   differential fuzzing: the float target checks that both print every value
   the same way. It is included after runtime.c; the code is the shipped code
   with its names prefixed. */

/* Canonical Float text: the shortest digits that parse back to the same value,
   positional for decimal exponents -5..15 and scientific otherwise, always with a
   `.` or an `e`. `output` holds at least 40 bytes. */
/* True when `significand` x 10^`exponent` (decimal) parses back to `magnitude`. */
static bool reference_decimal_round_trips(uint64_t significand, int exponent, double magnitude) {
  char text[48]; snprintf(text, sizeof(text), "%" PRIu64 "e%d", significand, exponent);
  return strtod(text, NULL) == magnitude;
}

/* The shortest significand (and its decimal exponent) that parses back to
   `magnitude`, chosen like Rust's `{:e}`: at the shortest length, the candidate
   closest to the value, and the larger one on an exact tie. The correctly rounded
   candidate can miss the round-trip interval at a power of two, where the interval
   below the value is half as wide, so its neighbors are tried too. */
static void reference_shortest_digits(double magnitude, char *digits, int *exponent) {
  for (int precision = 0; precision <= 16; ++precision) {
    char scientific[40]; snprintf(scientific, sizeof(scientific), "%.*e", precision, magnitude);
    uint64_t nearest = 0; const char *cursor = scientific;
    for (; *cursor != 'e'; ++cursor) if (*cursor != '.') nearest = nearest * 10 + (uint64_t)(*cursor - '0');
    int point = atoi(cursor + 1), scale = point - precision;
    uint64_t smallest = 1; for (int digit = 0; digit < precision; ++digit) smallest *= 10;
    uint64_t chosen = 0; bool found = false;
    if (reference_decimal_round_trips(nearest, scale, magnitude)) {
      chosen = nearest; found = true;
      if (reference_decimal_round_trips(nearest + 1, scale, magnitude)) {
        /* Both neighbors fit only on a tie: the exact value is `nearest`5000... */
        char exact[800]; snprintf(exact, sizeof(exact), "%.770e", magnitude);
        char wanted[24]; snprintf(wanted, sizeof(wanted), "%" PRIu64, nearest);
        char actual[800]; size_t length = 0;
        for (const char *c = exact; *c != 'e'; ++c) if (*c != '.') actual[length++] = *c;
        actual[length] = '\0';
        size_t prefix = strlen(wanted);
        bool tie = strncmp(actual, wanted, prefix) == 0 && actual[prefix] == '5' &&
                   strspn(actual + prefix + 1, "0") == length - prefix - 1;
        if (tie) chosen = nearest + 1;
      }
    } else if (reference_decimal_round_trips(nearest + 1, scale, magnitude)) {
      chosen = nearest + 1; found = true;
    } else if (nearest > smallest && reference_decimal_round_trips(nearest - 1, scale, magnitude)) {
      chosen = nearest - 1; found = true;
    }
    if (!found) continue;
    snprintf(digits, 24, "%" PRIu64, chosen);
    size_t count = strlen(digits);
    /* A carry such as 99 + 1 adds a digit; drop the zero it leaves. */
    *exponent = scale + (int)count - 1;
    while (count > 1 && digits[count - 1] == '0') digits[--count] = '\0';
    return;
  }
  /* 17 significant digits always round-trip. */
  char scientific[40]; snprintf(scientific, sizeof(scientific), "%.16e", magnitude);
  size_t count = 0; const char *cursor = scientific;
  for (; *cursor != 'e'; ++cursor) if (*cursor != '.') digits[count++] = *cursor;
  digits[count] = '\0'; *exponent = atoi(cursor + 1);
  while (count > 1 && digits[count - 1] == '0') digits[--count] = '\0';
}

static void reference_float_text(double value, char *output) {
  const char *sign = signbit(value) ? "-" : "";
  if (value == 0) { snprintf(output, 40, "%s0.0", sign); return; }
  char digits[24]; int exponent = 0;
  reference_shortest_digits(fabs(value), digits, &exponent);
  size_t count = strlen(digits);
  if (exponent >= -5 && exponent < 16) {
    if (exponent < 0) {
      snprintf(output, 40, "%s0.%.*s%s", sign, -exponent - 1, "0000", digits);
    } else if (count <= (size_t)exponent + 1) {
      snprintf(output, 40, "%s%s%.*s.0", sign, digits, (int)((size_t)exponent + 1 - count),
               "000000000000000");
    } else {
      snprintf(output, 40, "%s%.*s.%s", sign, exponent + 1, digits, digits + exponent + 1);
    }
  } else if (count == 1) {
    snprintf(output, 40, "%s%se%d", sign, digits, exponent);
  } else {
    snprintf(output, 40, "%s%c.%se%d", sign, digits[0], digits + 1, exponent);
  }
}

