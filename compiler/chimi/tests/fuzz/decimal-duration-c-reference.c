/* The C decimal and duration code that runtime.c used before it moved to Rust
   (runtime/chimi-runtime/src/decimal.rs and duration.rs, CHI-307), kept as the
   oracle for differential fuzzing: the decimal and duration targets check that
   both give the same values, texts, orders and parse results. It is included
   after runtime.c and uses its constants; the code is the shipped code with
   its names prefixed. */

typedef unsigned __int128 ReferenceDecimalMagnitude;
typedef struct { bool negative; ReferenceDecimalMagnitude magnitude; uint32_t scale; } ReferenceDecimal;
typedef enum { REFERENCE_DECIMAL_REMAINDER_ZERO, REFERENCE_DECIMAL_BELOW_HALF, REFERENCE_DECIMAL_HALF, REFERENCE_DECIMAL_ABOVE_HALF } ReferenceDecimalRemainder;
/* The codes `std/decimal.rounding_code` passes, in `Rounding` declaration order. */
typedef enum { REFERENCE_ROUND_TOWARD_ZERO, REFERENCE_ROUND_AWAY_FROM_ZERO, REFERENCE_ROUND_FLOOR, REFERENCE_ROUND_CEILING,
               REFERENCE_ROUND_HALF_TOWARD_ZERO, REFERENCE_ROUND_HALF_AWAY_FROM_ZERO, REFERENCE_ROUND_HALF_EVEN } ReferenceDecimalRounding;
/* Large enough for a sign, 39 digits, and a point. */

/* 10^exponent, or false when it exceeds the magnitude type (from 10^39). */
static bool reference_decimal_power(uint32_t exponent, ReferenceDecimalMagnitude *output) {
  if (exponent > DECIMAL_MAX_SCALE) return false;
  ReferenceDecimalMagnitude power = 1;
  for (uint32_t i = 0; i < exponent; ++i) power *= 10;
  *output = power;
  return true;
}

static ReferenceDecimalMagnitude reference_decimal_max_magnitude(void) {
  ReferenceDecimalMagnitude power = 0; reference_decimal_power(DECIMAL_MAX_SCALE, &power); return power - 1;
}

static bool reference_decimal_make(bool negative, ReferenceDecimalMagnitude magnitude, uint32_t scale, ReferenceDecimal *output) {
  if (magnitude > reference_decimal_max_magnitude() || scale > DECIMAL_MAX_SCALE) return false;
  *output = (ReferenceDecimal){negative && magnitude != 0, magnitude, scale};
  return true;
}

static ReferenceDecimal reference_decimal_negate(ReferenceDecimal value) {
  value.negative = !value.negative && value.magnitude != 0;
  return value;
}

static bool reference_decimal_add(ReferenceDecimal left, ReferenceDecimal right, ReferenceDecimal *output) {
  uint32_t scale = left.scale > right.scale ? left.scale : right.scale;
  ReferenceDecimalMagnitude left_power = 0, right_power = 0, l = 0, r = 0;
  reference_decimal_power(scale - left.scale, &left_power); reference_decimal_power(scale - right.scale, &right_power);
  /* Either aligned magnitude may exceed the range while the sum does not; past the
     magnitude type it cannot, because the other is at most 38 digits. */
  if (__builtin_mul_overflow(left.magnitude, left_power, &l) ||
      __builtin_mul_overflow(right.magnitude, right_power, &r)) return false;
  if (left.negative == right.negative) {
    ReferenceDecimalMagnitude sum = 0;
    if (__builtin_add_overflow(l, r, &sum)) return false;
    return reference_decimal_make(left.negative, sum, scale, output);
  }
  if (l >= r) return reference_decimal_make(left.negative, l - r, scale, output);
  return reference_decimal_make(right.negative, r - l, scale, output);
}

static bool reference_decimal_multiply(ReferenceDecimal left, ReferenceDecimal right, ReferenceDecimal *output) {
  ReferenceDecimalMagnitude product = 0;
  if (__builtin_mul_overflow(left.magnitude, right.magnitude, &product)) return false;
  return reference_decimal_make(left.negative != right.negative, product, left.scale + right.scale, output);
}

/* Requires: remainder < denominator. */
static ReferenceDecimalRemainder reference_decimal_remainder(ReferenceDecimalMagnitude remainder, ReferenceDecimalMagnitude denominator) {
  if (remainder == 0) return REFERENCE_DECIMAL_REMAINDER_ZERO;
  ReferenceDecimalMagnitude rest = denominator - remainder;
  return remainder < rest ? REFERENCE_DECIMAL_BELOW_HALF : remainder == rest ? REFERENCE_DECIMAL_HALF : REFERENCE_DECIMAL_ABOVE_HALF;
}

/* numerator x 10^shift / denominator, or false once the quotient leaves the
   magnitude type. Requires: 0 < denominator < 10^38, so the remainder doubled fits. */
static bool reference_decimal_long_division(ReferenceDecimalMagnitude numerator, ReferenceDecimalMagnitude denominator, uint32_t shift,
                                  ReferenceDecimalMagnitude *quotient, ReferenceDecimalRemainder *remainder) {
  ReferenceDecimalMagnitude q = numerator / denominator, r = numerator % denominator;
  for (uint32_t step = 0; step < shift; ++step) {
    /* r x 10 = digit x denominator + next, without leaving the magnitude type. */
    ReferenceDecimalMagnitude digit = 0, next = 0;
    for (int i = 0; i < 10; ++i) {
      next += r;
      if (next >= denominator) { next -= denominator; ++digit; }
    }
    if (__builtin_mul_overflow(q, (ReferenceDecimalMagnitude)10, &q) || __builtin_add_overflow(q, digit, &q)) return false;
    r = next;
  }
  if (q > reference_decimal_max_magnitude()) return false;
  *quotient = q; *remainder = reference_decimal_remainder(r, denominator);
  return true;
}

/* dividend / divisor with `scale` fraction digits, or false when the divisor is
   zero or the rounded quotient is out of range. Requires: scale <= 38. */
static bool reference_decimal_divide(ReferenceDecimal dividend, ReferenceDecimal divisor, uint32_t scale, ReferenceDecimalRounding rounding,
                           ReferenceDecimal *output) {
  if (divisor.magnitude == 0) return false;
  int64_t shift = (int64_t)scale + divisor.scale - dividend.scale;
  ReferenceDecimalMagnitude quotient = 0; ReferenceDecimalRemainder remainder = REFERENCE_DECIMAL_REMAINDER_ZERO;
  if (shift >= 0) {
    if (!reference_decimal_long_division(dividend.magnitude, divisor.magnitude, (uint32_t)shift, &quotient, &remainder))
      return false;
  } else {
    ReferenceDecimalMagnitude power = 0, denominator = 0;
    if (reference_decimal_power((uint32_t)-shift, &power) && !__builtin_mul_overflow(divisor.magnitude, power, &denominator)) {
      quotient = dividend.magnitude / denominator;
      remainder = reference_decimal_remainder(dividend.magnitude % denominator, denominator);
    } else {
      /* The divisor exceeds the magnitude type, so it is more than twice the dividend. */
      remainder = dividend.magnitude == 0 ? REFERENCE_DECIMAL_REMAINDER_ZERO : REFERENCE_DECIMAL_BELOW_HALF;
    }
  }
  bool negative = dividend.negative != divisor.negative, inexact = remainder != REFERENCE_DECIMAL_REMAINDER_ZERO, away = false;
  switch (rounding) {
    case REFERENCE_ROUND_TOWARD_ZERO: away = false; break;
    case REFERENCE_ROUND_AWAY_FROM_ZERO: away = inexact; break;
    case REFERENCE_ROUND_FLOOR: away = negative && inexact; break;
    case REFERENCE_ROUND_CEILING: away = !negative && inexact; break;
    case REFERENCE_ROUND_HALF_TOWARD_ZERO: away = remainder == REFERENCE_DECIMAL_ABOVE_HALF; break;
    case REFERENCE_ROUND_HALF_AWAY_FROM_ZERO: away = remainder >= REFERENCE_DECIMAL_HALF; break;
    case REFERENCE_ROUND_HALF_EVEN:
      away = remainder == REFERENCE_DECIMAL_ABOVE_HALF || (remainder == REFERENCE_DECIMAL_HALF && quotient % 2 == 1); break;
  }
  return reference_decimal_make(negative, quotient + (away ? 1 : 0), scale, output);
}

/* Orders magnitudes, aligning the one with the smaller scale; past the magnitude
   type the aligned side exceeds any 38-digit magnitude. */
static int reference_decimal_magnitude_compare(ReferenceDecimal left, ReferenceDecimal right) {
  ReferenceDecimalMagnitude power = 0, aligned = 0;
  if (left.scale >= right.scale) {
    reference_decimal_power(left.scale - right.scale, &power);
    if (__builtin_mul_overflow(right.magnitude, power, &aligned)) return -1;
    return (left.magnitude > aligned) - (left.magnitude < aligned);
  }
  reference_decimal_power(right.scale - left.scale, &power);
  if (__builtin_mul_overflow(left.magnitude, power, &aligned)) return 1;
  return (aligned > right.magnitude) - (aligned < right.magnitude);
}

/* Numeric order, so 1.50 and 1.5 are equal. Returns -1, 0, or 1. */
static int reference_decimal_compare(ReferenceDecimal left, ReferenceDecimal right) {
  int left_sign = left.magnitude == 0 ? 0 : left.negative ? -1 : 1;
  int right_sign = right.magnitude == 0 ? 0 : right.negative ? -1 : 1;
  if (left_sign != right_sign) return (left_sign > right_sign) - (left_sign < right_sign);
  return left.negative ? reference_decimal_magnitude_compare(right, left) : reference_decimal_magnitude_compare(left, right);
}

/* `Decimal.to_text`: exactly `scale` fraction digits. `output` holds DECIMAL_TEXT_BYTES. */
static void reference_decimal_text(ReferenceDecimal value, char *output) {
  char digits[DECIMAL_TEXT_BYTES]; size_t count = 0;
  ReferenceDecimalMagnitude magnitude = value.magnitude;
  do { digits[count++] = (char)('0' + (int)(magnitude % 10)); magnitude /= 10; } while (magnitude != 0);
  while (count < (size_t)value.scale + 1) digits[count++] = '0';
  size_t length = 0;
  if (value.negative) output[length++] = '-';
  for (size_t index = count; index-- > 0;) {
    output[length++] = digits[index];
    if (index == value.scale && index != 0) output[length++] = '.';
  }
  output[length] = '\0';
}

/* The debug form: trailing fraction zeros removed, so equal values render the same. */
static void reference_decimal_normalized_text(ReferenceDecimal value, char *output) {
  while (value.scale > 0 && value.magnitude % 10 == 0) { value.magnitude /= 10; --value.scale; }
  reference_decimal_text(value, output);
}

/* Reads -?digits(.digits)?; leading zeros are allowed. False for any other text,
   and for more than 38 significant or fraction digits. */
static bool reference_decimal_parse(const char *text, ReferenceDecimal *output) {
  bool negative = *text == '-';
  const char *cursor = negative ? text + 1 : text;
  ReferenceDecimalMagnitude magnitude = 0, largest = reference_decimal_max_magnitude();
  size_t whole = 0, fraction = 0; bool point = false;
  for (; *cursor; ++cursor) {
    if (*cursor == '.' && !point) { point = true; continue; }
    if (*cursor < '0' || *cursor > '9') return false;
    /* Checked before multiplying: 38 nines times 10 leaves the magnitude type. */
    ReferenceDecimalMagnitude digit = (ReferenceDecimalMagnitude)(*cursor - '0');
    if (magnitude > (largest - digit) / 10) return false;
    magnitude = magnitude * 10 + digit;
    if (point) ++fraction; else ++whole;
  }
  if (whole == 0 || (point && fraction == 0) || fraction > DECIMAL_MAX_SCALE) return false;
  return reference_decimal_make(negative, magnitude, (uint32_t)fraction, output);
}

static const char *const REFERENCE_DURATION_SUFFIXES[6] = { "h", "m", "s", "ms", "us", "ns" };

/* Appends `value / scale` as an exact decimal with no trailing fraction zeros. */
static char *reference_duration_decimal(char *out, uint64_t value, uint64_t scale) {
  out += sprintf(out, "%" PRIu64, value / scale);
  uint64_t fraction = value % scale;
  if (fraction == 0) return out;
  int width = 0; for (uint64_t rest = scale; rest > 1; rest /= 10) ++width;
  char digits[24]; snprintf(digits, sizeof(digits), "%0*" PRIu64, width, fraction);
  size_t length = strlen(digits); while (length > 0 && digits[length - 1] == '0') --length;
  digits[length] = '\0';
  return out + sprintf(out, ".%s", digits);
}

/* The canonical text form into `text`, which holds at least 32 bytes. */
static void reference_duration_text(int64_t duration, char *text) {
  if (duration == 0) { strcpy(text, "0s"); return; }
  char *out = text;
  if (duration < 0) *out++ = '-';
  /* The magnitude of the minimum value does not fit int64_t. */
  uint64_t magnitude = duration < 0 ? (uint64_t)0 - (uint64_t)duration : (uint64_t)duration;
  if (magnitude < (uint64_t)DURATION_UNITS[2]) {
    int unit = magnitude >= (uint64_t)DURATION_UNITS[3] ? 3 : magnitude >= (uint64_t)DURATION_UNITS[4] ? 4 : 5;
    out = reference_duration_decimal(out, magnitude, (uint64_t)DURATION_UNITS[unit]);
    strcpy(out, REFERENCE_DURATION_SUFFIXES[unit]); return;
  }
  uint64_t hour = (uint64_t)DURATION_UNITS[0], minute = (uint64_t)DURATION_UNITS[1];
  /* Seconds stay in nanoseconds, so the fraction is exact. */
  uint64_t parts[3] = { magnitude / hour, magnitude % hour / minute, magnitude % minute };
  int first = 0, last = 2;
  while (parts[first] == 0) ++first;
  while (parts[last] == 0) --last;
  for (int part = first; part <= last; ++part) {
    if (part == 2) out = reference_duration_decimal(out, parts[part], (uint64_t)DURATION_UNITS[2]);
    else out += sprintf(out, "%" PRIu64, parts[part]);
    out += sprintf(out, "%s", REFERENCE_DURATION_SUFFIXES[part]);
  }
}

/* `Duration.parse`; returns 0 on success or the failure constructor. A grammar error
   anywhere outranks a range error, and an inexact fraction is INVALID, never rounded. */
static uint32_t reference_duration_parse(const char *text, int64_t *output) {
  if (*text == '\0') return CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY;
  if (strcmp(text, "0") == 0) { *output = 0; return 0; }
  bool negative = *text == '-', out_of_range = false;
  const char *cursor = negative ? text + 1 : text;
  if (*cursor == '\0') return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
  __int128 total = 0; int previous = -1;
  while (*cursor) {
    const char *integer = cursor;
    while (*cursor >= '0' && *cursor <= '9') ++cursor;
    size_t integer_length = (size_t)(cursor - integer);
    if (integer_length == 0) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    const char *fraction = cursor; size_t fraction_length = 0;
    if (*cursor == '.') {
      fraction = ++cursor;
      while (*cursor >= '0' && *cursor <= '9') ++cursor;
      fraction_length = (size_t)(cursor - fraction);
      if (fraction_length == 0) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    }
    int unit = -1;
    for (int candidate = 0; candidate < 6; ++candidate) {
      size_t length = strlen(REFERENCE_DURATION_SUFFIXES[candidate]);
      /* `ms` before `m`: the longest suffix wins. */
      if (strncmp(cursor, REFERENCE_DURATION_SUFFIXES[candidate], length) == 0 &&
          (unit < 0 || length > strlen(REFERENCE_DURATION_SUFFIXES[unit]))) unit = candidate;
    }
    if (unit < 0 || unit <= previous) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    cursor += strlen(REFERENCE_DURATION_SUFFIXES[unit]); previous = unit;
    while (fraction_length > 0 && fraction[fraction_length - 1] == '0') --fraction_length;
    /* An exact fraction has at most 13 significant digits: `h` is 2^13 * 5^11 * 9 ns. */
    if (fraction_length > 13) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    __int128 scale = DURATION_UNITS[unit], denominator = 1, numerator = 0;
    for (size_t i = 0; i < fraction_length; ++i) {
      denominator *= 10; numerator = numerator * 10 + (fraction[i] - '0');
    }
    numerator *= scale;
    if (numerator % denominator != 0) return CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID;
    while (integer_length > 0 && *integer == '0') { ++integer; --integer_length; }
    if (integer_length > 22) { out_of_range = true; continue; }
    __int128 whole = 0;
    for (size_t i = 0; i < integer_length; ++i) whole = whole * 10 + (integer[i] - '0');
    total += whole * scale + numerator / denominator;
  }
  if (out_of_range) return CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE;
  if (negative) total = -total;
  if (total > INT64_MAX || total < INT64_MIN) return CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE;
  *output = (int64_t)total; return 0;
}


/* The reference's reading of a runtime `Decimal` value. */
static ReferenceDecimal reference_decimal_of(const ChimiValue *value) {
  return (ReferenceDecimal){value->as.decimal.negative,
                            ((ReferenceDecimalMagnitude)value->as.decimal.high << 64) | value->as.decimal.low,
                            value->as.decimal.scale};
}
