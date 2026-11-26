/* The C timestamp code that runtime.c used before it moved to Rust
   (runtime/chimi-runtime/src/timestamp.rs, CHI-305), kept as the oracle for
   differential fuzzing: the timestamp target checks that both print and parse
   every input the same way. It is included after runtime.c; the code is the
   shipped code with its names prefixed. */

static int64_t reference_days_from_civil(int64_t year, int64_t month, int64_t day) {
  year -= month <= 2;
  int64_t era = (year >= 0 ? year : year - 399) / 400;
  int64_t year_of_era = year - era * 400;
  int64_t day_of_year = (153 * (month + (month > 2 ? -3 : 9)) + 2) / 5 + day - 1;
  int64_t day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
  return era * 146097 + day_of_era - 719468;
}
static void reference_civil_from_days(int64_t days, int64_t *year, int64_t *month, int64_t *day) {
  days += 719468;
  int64_t era = (days >= 0 ? days : days - 146096) / 146097;
  int64_t day_of_era = days - era * 146097;
  int64_t year_of_era = (day_of_era - day_of_era / 1460 + day_of_era / 36524 - day_of_era / 146096) / 365;
  int64_t day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
  int64_t shifted = (5 * day_of_year + 2) / 153;
  *day = day_of_year - (153 * shifted + 2) / 5 + 1;
  *month = shifted < 10 ? shifted + 3 : shifted - 9;
  *year = year_of_era + era * 400 + (*month <= 2);
}
/* Writes `nanoseconds` as `2026-11-10T09:30:00.25Z`: the fraction loses its trailing
   zeros and is left out when it is zero. `out` holds at least 40 bytes. */
static void reference_timestamp_text(int64_t nanoseconds, char *out) {
  int64_t seconds = nanoseconds / 1000000000, fraction = nanoseconds % 1000000000;
  if (fraction < 0) { fraction += 1000000000; seconds -= 1; }
  int64_t days = seconds / 86400, of_day = seconds % 86400;
  if (of_day < 0) { of_day += 86400; days -= 1; }
  int64_t year, month, day;
  reference_civil_from_days(days, &year, &month, &day);
  int length = sprintf(out, "%04" PRId64 "-%02" PRId64 "-%02" PRId64 "T%02" PRId64 ":%02" PRId64 ":%02" PRId64,
                       year, month, day, of_day / 3600, of_day / 60 % 60, of_day % 60);
  if (fraction) {
    char digits[10]; sprintf(digits, "%09" PRId64, fraction);
    size_t used = 9; while (digits[used - 1] == '0') --used;
    digits[used] = '\0'; length += sprintf(out + length, ".%s", digits);
  }
  strcpy(out + length, "Z");
}
static bool reference_parse_digits(const char **cursor, int count, int64_t *value) {
  *value = 0;
  for (int i = 0; i < count; ++i, ++*cursor) {
    if (**cursor < '0' || **cursor > '9') return false;
    *value = *value * 10 + (**cursor - '0');
  }
  return true;
}
/* Parses an RFC 3339 date-time (`T` and `Z` in either case, a fraction of up to nine
   digits, `Z` or a `+HH:MM`/`-HH:MM` offset) into Unix nanoseconds. Leap second 60 and
   values outside the Timestamp range are rejected. */
static bool reference_parse_timestamp(const char *text, int64_t *out) {
  const char *cursor = text;
  int64_t year, month, day, hour, minute, second, fraction = 0, offset = 0;
  if (!reference_parse_digits(&cursor, 4, &year) || *cursor++ != '-' || !reference_parse_digits(&cursor, 2, &month) ||
      *cursor++ != '-' || !reference_parse_digits(&cursor, 2, &day) || (*cursor != 'T' && *cursor != 't'))
    return false;
  ++cursor;
  if (!reference_parse_digits(&cursor, 2, &hour) || *cursor++ != ':' || !reference_parse_digits(&cursor, 2, &minute) ||
      *cursor++ != ':' || !reference_parse_digits(&cursor, 2, &second))
    return false;
  if (*cursor == '.') {
    ++cursor; int digits = 0;
    while (*cursor >= '0' && *cursor <= '9') {
      if (++digits > 9) return false;
      fraction = fraction * 10 + (*cursor++ - '0');
    }
    if (digits == 0) return false;
    for (; digits < 9; ++digits) fraction *= 10;
  }
  if (*cursor == 'Z' || *cursor == 'z') ++cursor;
  else if (*cursor == '+' || *cursor == '-') {
    int64_t sign = *cursor++ == '-' ? -1 : 1, offset_hours, offset_minutes;
    if (!reference_parse_digits(&cursor, 2, &offset_hours) || *cursor++ != ':' ||
        !reference_parse_digits(&cursor, 2, &offset_minutes) || offset_hours > 23 || offset_minutes > 59)
      return false;
    offset = sign * (offset_hours * 3600 + offset_minutes * 60);
  } else return false;
  if (*cursor != '\0') return false;
  static const int month_days[] = {31, 29, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31};
  bool leap = (year % 4 == 0 && year % 100 != 0) || year % 400 == 0;
  if (year < 1678 || year > 2262 || month < 1 || month > 12 || day < 1 || day > month_days[month - 1] ||
      (month == 2 && day == 29 && !leap) || hour > 23 || minute > 59 || second > 59)
    return false;
  int64_t seconds = reference_days_from_civil(year, month, day) * 86400 + hour * 3600 + minute * 60 + second - offset;
  int64_t nanoseconds;
  if (__builtin_mul_overflow(seconds, INT64_C(1000000000), &nanoseconds) ||
      __builtin_add_overflow(nanoseconds, fraction, &nanoseconds))
    return false;
  *out = nanoseconds;
  return true;
}

