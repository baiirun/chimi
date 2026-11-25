/* Fuzz targets for the native runtime's parsers and text code: JSON, decimal,
   duration, float text, metrics and text. Each target turns one input into a canonical output line
   and checks the target's invariants, aborting on a broken one.

   The output is the contract a replacement must keep: when a subsystem moves
   to Rust (CHI-305), the same inputs run through both versions must print the
   same lines. `tests/runtime_fuzz.rs` replays the committed corpus,
   `corpus/<target>.txt`, against the outputs recorded in it, and
   `scripts/fuzz-runtime.sh` runs libFuzzer.

   The replay driver and libFuzzer entry points are fuzz-main.h's. Both builds
   link the Rust runtime archive (libchimi_runtime_standalone.a; see
   scripts/fuzz-runtime.sh and tests/runtime_fuzz.rs).

   Targets call the runtime's intrinsic entry points, as compiled programs do,
   so the checks hold for whatever implements those intrinsics. The json
   target also decodes each input with the C decoder the Rust one replaced
   (json-c-reference.c), and the decimal and duration targets compute with
   the C code their Rust replaced (decimal-duration-c-reference.c), as the
   text target does with its case mapping, trimming and graphemes
   (text-c-reference.c), the float target with its Float text
   (float-c-reference.c) and the metrics target with its aggregate
   (metrics-c-reference.c); each checks that the two agree. The runtime is
   included whole, with the few standard constructors the targets build in
   place of a compiled program's tables. */
#include "runtime.c"
#include "json-c-reference.c"
#include "decimal-duration-c-reference.c"
#include "float-c-reference.c"
#include "metrics-c-reference.c"
#include "text-c-reference.c"

/* Program tables: one constructor symbol per standard constructor below. */
enum {
  S_EMPTY, S_JSON, S_OPTION, S_DECODE_ERROR, S_DURATION_ERROR, S_NULL, S_BOOLEAN, S_NUMBER,
  S_TEXT, S_ARRAY, S_OBJECT, S_INVALID_JSON, S_NOT_INTEGER, S_OUT_OF_RANGE, S_SOME, S_NONE,
  S_PARSE_EMPTY, S_PARSE_INVALID, S_PARSE_OUT_OF_RANGE, S_VALUE, S_MESSAGE, S_OFFSET,
  S_STRING_COUNT
};
const char *CHIMI_STRINGS[] = {
  "", "Json", "Option", "DecodeError", "DurationParseError", "Null", "Boolean", "Number",
  "Text", "Array", "Object", "InvalidJson", "NotInteger", "OutOfRange", "Some", "None",
  "Empty", "Invalid", "OutOfRange", "value", "message", "offset",
};
const uint32_t CHIMI_STRING_LENGTHS[S_STRING_COUNT] = {0};
/* Field names: [0] value; [1] message, [2] offset. */
const uint32_t CHIMI_IDS[] = {S_VALUE, S_MESSAGE, S_OFFSET};

#define FUZZ_CONSTRUCTORS(X)                                                    \
  X(CHIMI_CONSTRUCTOR_JSON_NULL, S_JSON, S_NULL, 0, 0)                          \
  X(CHIMI_CONSTRUCTOR_JSON_BOOLEAN, S_JSON, S_BOOLEAN, 0, 1)                    \
  X(CHIMI_CONSTRUCTOR_JSON_NUMBER, S_JSON, S_NUMBER, 0, 1)                      \
  X(CHIMI_CONSTRUCTOR_JSON_TEXT, S_JSON, S_TEXT, 0, 1)                          \
  X(CHIMI_CONSTRUCTOR_JSON_ARRAY, S_JSON, S_ARRAY, 0, 1)                        \
  X(CHIMI_CONSTRUCTOR_JSON_OBJECT, S_JSON, S_OBJECT, 0, 1)                      \
  X(CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON, S_DECODE_ERROR, S_INVALID_JSON, 1, 2) \
  X(CHIMI_CONSTRUCTOR_JSON_NOT_INTEGER, S_DECODE_ERROR, S_NOT_INTEGER, 0, 0)    \
  X(CHIMI_CONSTRUCTOR_JSON_OUT_OF_RANGE, S_DECODE_ERROR, S_OUT_OF_RANGE, 0, 0)  \
  X(CHIMI_CONSTRUCTOR_OPTION_SOME, S_OPTION, S_SOME, 0, 1)                      \
  X(CHIMI_CONSTRUCTOR_OPTION_NONE, S_OPTION, S_NONE, 0, 0)                      \
  X(CHIMI_CONSTRUCTOR_DURATION_PARSE_EMPTY, S_DURATION_ERROR, S_PARSE_EMPTY, 0, 0) \
  X(CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID, S_DURATION_ERROR, S_PARSE_INVALID, 0, 0) \
  X(CHIMI_CONSTRUCTOR_DURATION_PARSE_OUT_OF_RANGE, S_DURATION_ERROR, S_PARSE_OUT_OF_RANGE, 0, 0)

#define FUZZ_CONSTRUCTOR(standard, type, variant, offset, length) \
  {type, variant, offset, length, 1, 0, 0},
const ChimiConstructor CHIMI_CONSTRUCTORS[] = {FUZZ_CONSTRUCTORS(FUZZ_CONSTRUCTOR)};
/* Symbol i names constructor i. */
#define FUZZ_INDEX(standard, type, variant, offset, length) FUZZ_INDEX_##standard,
enum { FUZZ_CONSTRUCTORS(FUZZ_INDEX) };
#define FUZZ_SYMBOL(standard, type, variant, offset, length)                         \
  {.kind = CHIMI_SYMBOL_CONSTRUCTOR, .name = variant, .function = UINT32_MAX,         \
   .constant = UINT32_MAX, .constructor = FUZZ_INDEX_##standard,                     \
   .intrinsic = UINT32_MAX, .standard_constructor = standard,                        \
   .standard_type = UINT32_MAX, .environment_slot = UINT32_MAX, .binding = UINT32_MAX},
const ChimiSymbol CHIMI_SYMBOLS[] = {FUZZ_CONSTRUCTORS(FUZZ_SYMBOL)};
const uint32_t CHIMI_SYMBOL_COUNT = COUNT_OF(CHIMI_SYMBOLS);

const char *CHIMI_COMPUTATION = "runtime-fuzz";
const uint32_t CHIMI_REQUIRES_SCHEDULER = 0;
const ChimiOrigin CHIMI_ORIGINS[] = {{0}};
const ChimiNode CHIMI_NODES[] = {{0}};
const uint32_t CHIMI_NODE_COUNT = 0;
const uint32_t CHIMI_CLOCK_REQUIREMENT_SLOT = 0;
const uint32_t CHIMI_REQUIREMENT_SLOT_COUNT = 1;
const ChimiFunction CHIMI_FUNCTIONS[] = {{0}};
const uint32_t CHIMI_FUNCTION_COUNT = 0;
const uint32_t CHIMI_TEST_FUNCTIONS[] = {0};
const uint32_t CHIMI_TEST_COUNT = 0;
const uint32_t CHIMI_ASSERTION_NODES[] = {0};
const uint32_t CHIMI_ASSERTION_NODE_COUNT = 0;
const ChimiNamedNode CHIMI_NAMED_NODES[] = {{0}};
const ChimiStep CHIMI_STEPS[] = {{0}};
const ChimiArm CHIMI_ARMS[] = {{0}};
const ChimiPattern CHIMI_PATTERNS[] = {{0}};
const ChimiConstantValue CHIMI_CONSTANT_VALUES[] = {{0}};
const uint32_t CHIMI_CONSTANT_VALUE_COUNT = 0;
const ChimiBinding CHIMI_BINDINGS[] = {{0}};
const uint32_t CHIMI_BINDING_COUNT = 0;
ChimiNodeFn CHIMI_NODE_FUNCTIONS[] = {NULL};

#define FUZZ_NAME "runtime-fuzz"
#include "fuzz-output.h"
static void out_text(FuzzOutput *out, const ChimiValue *text) {
  out_quoted(out, text->as.text.bytes, text->as.text.length);
}

/* Intrinsic calls. A defect is a broken invariant: every call below meets the
   preconditions the Chimi wrappers check. */
typedef ChimiControl *(*FuzzIntrinsic)(uint32_t, ChimiValue **, uint32_t, uint32_t);
static ChimiControl *fz_checked(ChimiControl *control, const char *operation) {
  FUZZ_CHECK(control && control->kind != CONTROL_DEFECT, "%s defected: %s", operation,
             control && control->message ? control->message : "(no control)");
  FUZZ_CHECK(control->kind == CONTROL_SUCCESS || control->kind == CONTROL_FAILURE,
             "%s ended with control kind %d", operation, (int)control->kind);
  return control;
}
static ChimiControl *call_core(uint32_t intrinsic, ChimiValue **arguments, uint32_t count) {
  return fz_checked(invoke_core_operation(intrinsic, arguments, count, 0), "core intrinsic");
}
static ChimiControl *call_bytes(uint32_t intrinsic, ChimiValue **arguments, uint32_t count) {
  return fz_checked(invoke_bytes_operation(intrinsic, arguments, count, 0), "bytes intrinsic");
}
static ChimiControl *call_json(uint32_t intrinsic, ChimiValue **arguments, uint32_t count) {
  return fz_checked(invoke_json_operation(NULL, intrinsic, arguments, count, 0, 0, NULL), "json intrinsic");
}
static ChimiControl *call_decimal(uint32_t intrinsic, ChimiValue **arguments, uint32_t count) {
  return fz_checked(invoke_decimal_operation(NULL, intrinsic, arguments, count, 0, 0, NULL), "decimal intrinsic");
}
static ChimiControl *call_duration(uint32_t intrinsic, ChimiValue **arguments, uint32_t count) {
  return fz_checked(invoke_duration(intrinsic, arguments, count, 0), "duration intrinsic");
}
static uint32_t fz_standard_of(const ChimiValue *value) {
  FUZZ_CHECK(value && value->kind == VALUE_CONSTRUCTED, "expected a constructed value");
  return CHIMI_SYMBOLS[value->as.constructed.constructor].standard_constructor;
}
static ChimiValue *fz_field(const ChimiValue *value, uint32_t index) {
  FUZZ_CHECK(index < value->as.constructed.length, "field %u of %u", index, value->as.constructed.length);
  return value->as.constructed.fields[index].value;
}
static bool fz_texts_equal(const ChimiValue *left, const ChimiValue *right) {
  return left->as.text.length == right->as.text.length &&
         memcmp(left->as.text.bytes, right->as.text.bytes, left->as.text.length) == 0;
}

/* Raw input as Text the way a program gets it from bytes: Bytes.to_text_lossy. */
static ChimiValue *fz_input_text(const uint8_t *data, size_t size) {
  ChimiValue *arguments[] = {bytes_from((const char *)data, size)};
  ChimiValue *text = call_bytes(CHIMI_INTRINSIC_BYTES_TO_TEXT_LOSSY, arguments, 1)->value;
  FUZZ_CHECK(text->kind == VALUE_TEXT, "lossy conversion gave kind %d", (int)text->kind);
  return text;
}
/* Bytes.valid_utf8_length of a Text's bytes; equal to its length for valid Text. */
static int64_t fz_valid_prefix(const ChimiValue *text) {
  ChimiValue *arguments[] = {bytes_from(text->as.text.bytes, text->as.text.length)};
  return call_bytes(CHIMI_INTRINSIC_BYTES_VALID_UTF8_LENGTH, arguments, 1)->value->as.integer;
}
static void fz_check_valid_text(const ChimiValue *text, const char *what) {
  FUZZ_CHECK(text->kind == VALUE_TEXT, "%s is kind %d, not Text", what, (int)text->kind);
  FUZZ_CHECK(fz_valid_prefix(text) == (int64_t)text->as.text.length, "%s is not valid UTF-8", what);
}

/* --- text: lossy conversion, case mapping, trimming, graphemes ------------ */
static void fuzz_text(const uint8_t *data, size_t size, FuzzOutput *out) {
  ChimiValue *raw[] = {bytes_from((const char *)data, size)};
  int64_t valid = call_bytes(CHIMI_INTRINSIC_BYTES_VALID_UTF8_LENGTH, raw, 1)->value->as.integer;
  FUZZ_CHECK(valid >= 0 && (size_t)valid <= size, "valid prefix %" PRId64 " of %zu", valid, size);
  ChimiValue *text = fz_input_text(data, size);
  fz_check_valid_text(text, "lossy text");
  if ((size_t)valid == size)
    FUZZ_CHECK(text->as.text.length == size && memcmp(text->as.text.bytes, data, size) == 0,
               "lossy conversion changed valid UTF-8");
  out_word(out, "valid="); out_integer(out, valid);
  out_word(out, " text="); out_text(out, text);

  static const struct { uint32_t intrinsic; const char *name; } maps[] = {
    {CHIMI_INTRINSIC_TEXT_LOWERCASE, "lower"}, {CHIMI_INTRINSIC_TEXT_UPPERCASE, "upper"},
    {CHIMI_INTRINSIC_TEXT_TRIM_START, "trim_start"}, {CHIMI_INTRINSIC_TEXT_TRIM_END, "trim_end"},
  };
  for (size_t index = 0; index < COUNT_OF(maps); ++index) {
    ChimiValue *arguments[] = {text};
    ChimiValue *mapped = call_core(maps[index].intrinsic, arguments, 1)->value;
    fz_check_valid_text(mapped, maps[index].name);
    ChimiValue *expected = reference_text_map(index, text);
    FUZZ_CHECK(fz_texts_equal(mapped, expected), "%s gave %s, the C reference %s", maps[index].name,
               mapped->as.text.bytes, expected->as.text.bytes);
    out_word(out, " "); out_word(out, maps[index].name); out_word(out, "="); out_text(out, mapped);
  }

  /* Graphemes cover the text exactly, each non-empty and valid. */
  ChimiValue *arguments[] = {text};
  ChimiValue *graphemes = call_core(CHIMI_INTRINSIC_TEXT_GRAPHEMES, arguments, 1)->value;
  FUZZ_CHECK(graphemes->kind == VALUE_LIST, "graphemes gave kind %d", (int)graphemes->kind);
  out_word(out, " graphemes=");
  size_t covered = 0;
  for (uint32_t index = 0; index < list_length(graphemes); ++index) {
    ChimiValue *grapheme = list_get(graphemes, index);
    fz_check_valid_text(grapheme, "grapheme");
    FUZZ_CHECK(grapheme->as.text.length > 0, "grapheme %u is empty", index);
    FUZZ_CHECK(covered + grapheme->as.text.length <= text->as.text.length &&
                   memcmp(text->as.text.bytes + covered, grapheme->as.text.bytes, grapheme->as.text.length) == 0,
               "grapheme %u does not continue the text at byte %zu", index, covered);
    size_t expected_end = reference_grapheme_end(text->as.text.bytes, text->as.text.length, covered);
    covered += grapheme->as.text.length;
    FUZZ_CHECK(covered == expected_end, "grapheme %u ends at byte %zu, the C reference's at %zu", index, covered,
               expected_end);
    if (index) out_word(out, ",");
    out_integer(out, (int64_t)grapheme->as.text.length);
  }
  FUZZ_CHECK(covered == text->as.text.length, "graphemes cover %zu of %zu bytes", covered, text->as.text.length);
}

/* --- json: decode, encode, and number conversions ------------------------- */
static ChimiControl *fz_json_decode(ChimiValue *text) {
  ChimiValue *arguments[] = {text};
  return call_json(CHIMI_INTRINSIC_JSON_DECODE, arguments, 1);
}
static ChimiValue *fz_json_encode(ChimiValue *json) {
  ChimiValue *arguments[] = {json};
  ChimiValue *encoded = call_json(CHIMI_INTRINSIC_JSON_ENCODE, arguments, 1)->value;
  fz_check_valid_text(encoded, "encoded JSON");
  return encoded;
}
/* The Rust decoder agrees with the C decoder it replaced: the same error, or
   values with the same canonical encoding (which keeps numbers as written
   and lists object members once, in key order). */
static void fz_check_against_reference(ChimiValue *text, ChimiControl *decoded) {
  ChimiControl *reference = reference_json_decode(text);
  FUZZ_CHECK(reference->kind == decoded->kind, "the C reference %s where the decoder %s",
             reference->kind == CONTROL_SUCCESS ? "decodes" : "fails",
             decoded->kind == CONTROL_SUCCESS ? "decodes" : "fails");
  if (decoded->kind == CONTROL_FAILURE) {
    ChimiValue *message = fz_field(decoded->value, 0), *offset = fz_field(decoded->value, 1);
    ChimiValue *expected_message = fz_field(reference->value, 0), *expected_offset = fz_field(reference->value, 1);
    FUZZ_CHECK(fz_texts_equal(message, expected_message) && offset->as.integer == expected_offset->as.integer,
               "error `%s` at %" PRId64 ", the C reference `%s` at %" PRId64, message->as.text.bytes,
               offset->as.integer, expected_message->as.text.bytes, expected_offset->as.integer);
    return;
  }
  ChimiValue *encoded = fz_json_encode(decoded->value), *expected = fz_json_encode(reference->value);
  FUZZ_CHECK(fz_texts_equal(encoded, expected), "decoded %s, the C reference %s", encoded->as.text.bytes,
             expected->as.text.bytes);
}
static void fuzz_json(const uint8_t *data, size_t size, FuzzOutput *out) {
  ChimiValue *text = fz_input_text(data, size);
  ChimiControl *decoded = fz_json_decode(text);
  fz_check_against_reference(text, decoded);
  if (decoded->kind == CONTROL_FAILURE) {
    ChimiValue *error = decoded->value;
    FUZZ_CHECK(fz_standard_of(error) == CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON, "decode failed with constructor %u",
               fz_standard_of(error));
    ChimiValue *message = fz_field(error, 0), *offset = fz_field(error, 1);
    fz_check_valid_text(message, "decode error message");
    FUZZ_CHECK(offset->kind == VALUE_INTEGER && offset->as.integer >= 0 &&
                   (uint64_t)offset->as.integer <= text->as.text.length,
               "decode error offset %" PRId64 " of %zu", offset->as.integer, text->as.text.length);
    out_word(out, "error offset="); out_integer(out, offset->as.integer);
    out_word(out, " message="); out_text(out, message);
    return;
  }
  /* Encoding is canonical: the encoded text decodes and encodes to itself. */
  ChimiValue *encoded = fz_json_encode(decoded->value);
  ChimiControl *again = fz_json_decode(encoded);
  FUZZ_CHECK(again->kind == CONTROL_SUCCESS, "encoded JSON does not decode: %s", encoded->as.text.bytes);
  ChimiValue *reencoded = fz_json_encode(again->value);
  FUZZ_CHECK(fz_texts_equal(encoded, reencoded), "encoding is not stable: %s then %s", encoded->as.text.bytes,
             reencoded->as.text.bytes);
  out_word(out, "ok "); out_text(out, encoded);

  if (fz_standard_of(decoded->value) != CHIMI_CONSTRUCTOR_JSON_NUMBER) return;
  ChimiValue *number[] = {fz_field(decoded->value, 0)};
  ChimiControl *integer = call_json(CHIMI_INTRINSIC_JSON_NUMBER_TO_INT, number, 1);
  out_word(out, " int=");
  if (integer->kind == CONTROL_SUCCESS) out_integer(out, integer->value->as.integer);
  else out_word(out, CHIMI_STRINGS[CHIMI_CONSTRUCTORS[integer->value->as.constructed.constructor].variant_name]);
  ChimiControl *decimal = call_json(CHIMI_INTRINSIC_JSON_NUMBER_TO_DECIMAL, number, 1);
  out_word(out, " decimal=");
  if (decimal->kind == CONTROL_SUCCESS) {
    ChimiValue *arguments[] = {decimal->value};
    out_text(out, call_decimal(CHIMI_INTRINSIC_DECIMAL_TO_TEXT, arguments, 1)->value);
  } else {
    out_word(out, CHIMI_STRINGS[CHIMI_CONSTRUCTORS[decimal->value->as.constructed.constructor].variant_name]);
  }
}

/* --- decimal: read, print, arithmetic ------------------------------------- */
/* `Some(value)`'s value, or NULL for None. */
static ChimiValue *fz_option(ChimiValue *option) {
  uint32_t kind = fz_standard_of(option);
  FUZZ_CHECK(kind == CHIMI_CONSTRUCTOR_OPTION_SOME || kind == CHIMI_CONSTRUCTOR_OPTION_NONE,
             "expected an Option, got constructor %u", kind);
  return kind == CHIMI_CONSTRUCTOR_OPTION_SOME ? fz_field(option, 0) : NULL;
}
static ChimiValue *fz_decimal_read(ChimiValue *text) {
  ChimiValue *arguments[] = {text};
  return fz_option(call_decimal(CHIMI_INTRINSIC_DECIMAL_READ, arguments, 1)->value);
}
static ChimiValue *fz_decimal_to_text(ChimiValue *decimal) {
  ChimiValue *arguments[] = {decimal};
  ChimiValue *text = call_decimal(CHIMI_INTRINSIC_DECIMAL_TO_TEXT, arguments, 1)->value;
  fz_check_valid_text(text, "decimal text");
  return text;
}
static void fz_out_decimal(FuzzOutput *out, ChimiValue *decimal) {
  if (decimal) out_text(out, fz_decimal_to_text(decimal));
  else out_word(out, "none");
}
/* Checks a runtime result, a Decimal or NULL for None, against the C
   reference's: the same presence, text and debug text. */
static void fz_check_decimal(ChimiValue *actual, bool fits, ReferenceDecimal expected, const char *what) {
  FUZZ_CHECK((actual != NULL) == fits, "%s: the C reference %s a value", what, fits ? "gives" : "gives no");
  if (!actual) return;
  char want[DECIMAL_TEXT_BYTES], got[DECIMAL_TEXT_BYTES];
  reference_decimal_text(expected, want);
  decimal_text(decimal_of(actual), got);
  FUZZ_CHECK(strcmp(want, got) == 0, "%s: %s, the C reference %s", what, got, want);
  reference_decimal_normalized_text(expected, want);
  decimal_normalized_text(decimal_of(actual), got);
  FUZZ_CHECK(strcmp(want, got) == 0, "%s: debug text %s, the C reference %s", what, got, want);
}
/* Reads `text` as a Decimal and checks that its printed form reads back to the
   same printed form. */
static ChimiValue *fz_read_operand(ChimiValue *text, FuzzOutput *out, const char *name) {
  ChimiValue *decimal = fz_decimal_read(text);
  ReferenceDecimal expected = {0};
  /* The reference reads a C string, so a U+0000 inside is no digit. */
  bool fits = !text_holds_nul(text) && reference_decimal_parse(text->as.text.bytes, &expected);
  fz_check_decimal(decimal, fits, expected, "read");
  out_word(out, name); out_word(out, "="); fz_out_decimal(out, decimal);
  if (decimal) {
    ChimiValue *printed = fz_decimal_to_text(decimal), *reread = fz_decimal_read(printed);
    FUZZ_CHECK(reread, "printed decimal %s does not read back", printed->as.text.bytes);
    FUZZ_CHECK(fz_texts_equal(printed, fz_decimal_to_text(reread)), "printed decimal %s reads back as %s",
               printed->as.text.bytes, fz_decimal_to_text(reread)->as.text.bytes);
  }
  return decimal;
}
/* Input: two operands separated by the first newline. The first byte also picks
   the division's scale and rounding. */
static void fuzz_decimal(const uint8_t *data, size_t size, FuzzOutput *out) {
  const uint8_t *newline = memchr(data, '\n', size);
  size_t left_size = newline ? (size_t)(newline - data) : size;
  ChimiValue *left = fz_read_operand(fz_input_text(data, left_size), out, "a");
  if (!newline) return;
  ChimiValue *right = fz_read_operand(fz_input_text(newline + 1, size - left_size - 1), out, " b");
  if (!left || !right) return;
  static const struct { uint32_t intrinsic; const char *name; } operations[] = {
    {CHIMI_INTRINSIC_DECIMAL_ADD_CHECKED, " add="},
    {CHIMI_INTRINSIC_DECIMAL_SUBTRACT_CHECKED, " subtract="},
    {CHIMI_INTRINSIC_DECIMAL_MULTIPLY_CHECKED, " multiply="},
  };
  ReferenceDecimal l = reference_decimal_of(left), r = reference_decimal_of(right), expected = {0};
  for (size_t index = 0; index < COUNT_OF(operations); ++index) {
    ChimiValue *arguments[] = {left, right};
    out_word(out, operations[index].name);
    ChimiValue *result = fz_option(call_decimal(operations[index].intrinsic, arguments, 2)->value);
    bool fits = index == 0 ? reference_decimal_add(l, r, &expected)
              : index == 1 ? reference_decimal_add(l, reference_decimal_negate(r), &expected)
              : reference_decimal_multiply(l, r, &expected);
    fz_check_decimal(result, fits, expected, operations[index].name + 1);
    fz_out_decimal(out, result);
  }
  FUZZ_CHECK(chimi_decimal_compare_v1(decimal_of(left), decimal_of(right)) == reference_decimal_compare(l, r),
             "compare: %d, the C reference %d", chimi_decimal_compare_v1(decimal_of(left), decimal_of(right)),
             reference_decimal_compare(l, r));
  /* Decimal.divide crashes on a zero divisor before reaching the intrinsic. */
  if (r.magnitude == 0) return;
  uint8_t pick = size ? data[0] : 0;
  int64_t scale = pick % (DECIMAL_MAX_SCALE + 1), rounding = pick % (ROUND_HALF_EVEN + 1);
  ChimiValue *arguments[] = {left, right, integer_value(scale), integer_value(rounding)};
  out_word(out, " divide("); out_integer(out, scale); out_word(out, ","); out_integer(out, rounding);
  out_word(out, ")=");
  ChimiValue *quotient = fz_option(call_decimal(CHIMI_INTRINSIC_DECIMAL_DIVIDE_ROUNDED, arguments, 4)->value);
  fz_check_decimal(quotient, reference_decimal_divide(l, r, (uint32_t)scale, (ReferenceDecimalRounding)rounding, &expected),
                   expected, "divide");
  fz_out_decimal(out, quotient);
}

/* --- duration: parse and print -------------------------------------------- */
static ChimiControl *fz_duration_parse(ChimiValue *text) {
  ChimiValue *arguments[] = {text};
  return call_duration(CHIMI_INTRINSIC_DURATION_PARSE, arguments, 1);
}
static ChimiValue *fz_duration_print(ChimiValue *duration) {
  ChimiValue *arguments[] = {duration};
  ChimiValue *text = call_duration(CHIMI_INTRINSIC_DURATION_TO_TEXT, arguments, 1)->value;
  fz_check_valid_text(text, "duration text");
  return text;
}
/* Checks `text`'s parse, and the text form of what it parses to, against the
   C reference's. */
static void fz_check_duration(ChimiValue *text, ChimiControl *parsed) {
  int64_t expected = 0;
  uint32_t failure = text_holds_nul(text) ? CHIMI_CONSTRUCTOR_DURATION_PARSE_INVALID
                                          : reference_duration_parse(text->as.text.bytes, &expected);
  uint32_t got = parsed->kind == CONTROL_FAILURE ? fz_standard_of(parsed->value) : 0;
  FUZZ_CHECK(got == failure, "parse: constructor %u, the C reference %u", got, failure);
  if (failure) return;
  FUZZ_CHECK(parsed->value->as.integer == expected, "parse: %" PRId64 "ns, the C reference %" PRId64 "ns",
             parsed->value->as.integer, expected);
  char want[32], printed[32];
  reference_duration_text(expected, want);
  duration_text(expected, printed);
  FUZZ_CHECK(strcmp(want, printed) == 0, "text: %s, the C reference %s", printed, want);
}
static void fuzz_duration(const uint8_t *data, size_t size, FuzzOutput *out) {
  ChimiValue *text = fz_input_text(data, size);
  ChimiControl *parsed = fz_duration_parse(text);
  fz_check_duration(text, parsed);
  /* Every value, not only the parsed ones: the input's first eight bytes too. */
  if (size >= 8) {
    int64_t raw; memcpy(&raw, data, sizeof(raw));
    char want[32], printed[32];
    reference_duration_text(raw, want);
    duration_text(raw, printed);
    FUZZ_CHECK(strcmp(want, printed) == 0, "text of %" PRId64 "ns: %s, the C reference %s", raw, printed, want);
  }
  if (parsed->kind == CONTROL_FAILURE) {
    out_word(out, "error ");
    out_word(out, CHIMI_STRINGS[CHIMI_CONSTRUCTORS[parsed->value->as.constructed.constructor].variant_name]);
    return;
  }
  FUZZ_CHECK(parsed->value->kind == VALUE_DURATION, "parse gave kind %d", (int)parsed->value->kind);
  int64_t nanoseconds = parsed->value->as.integer;
  ChimiValue *printed = fz_duration_print(parsed->value);
  ChimiControl *again = fz_duration_parse(printed);
  FUZZ_CHECK(again->kind == CONTROL_SUCCESS && again->value->as.integer == nanoseconds,
             "printed duration %s does not parse back to %" PRId64 "ns", printed->as.text.bytes, nanoseconds);
  out_word(out, "ok ns="); out_integer(out, nanoseconds);
  out_word(out, " text="); out_text(out, printed);
}

/* --- float: print ---------------------------------------------------------- */
/* Checks one finite value's text against the C reference's and that it parses
   back to the value, and writes it. */
static void fz_check_float(double value, FuzzOutput *out) {
  if (!isfinite(value)) return;
  char want[40], printed[40];
  reference_float_text(value, want);
  float_text(value, printed);
  FUZZ_CHECK(strcmp(want, printed) == 0, "text of %a: %s, the C reference %s", value, printed, want);
  double back = strtod(printed, NULL);
  FUZZ_CHECK(back == value && signbit(back) == signbit(value), "%s does not parse back to %a", printed, value);
  out_word(out, printed);
  out_word(out, " ");
}
/* Each eight bytes of the input as a double, and the input as decimal text. */
static void fuzz_float(const uint8_t *data, size_t size, FuzzOutput *out) {
  for (size_t offset = 0; offset + 8 <= size; offset += 8) {
    double value; memcpy(&value, data + offset, sizeof(value));
    fz_check_float(value, out);
  }
  char text[64];
  size_t length = size < sizeof(text) - 1 ? size : sizeof(text) - 1;
  memcpy(text, data, length);
  text[length] = '\0';
  char *end = NULL;
  double parsed = strtod(text, &end);
  if (end != text) {
    out_word(out, "parsed ");
    fz_check_float(parsed, out);
  }
}

/* --- metrics: record and write ---------------------------------------------- */
/* Each 12 bytes of the input are one measurement: a kind, one of four names,
   a label set from two bytes, and a finite value from eight (a count's made
   whole and non-negative, as std/metrics checks). Both aggregates record them
   all, and their documents must be the same. */
static void fuzz_metrics(const uint8_t *data, size_t size, FuzzOutput *out) {
  static const char *const names[] = {"\"requests\"", "\"latency\"", "\"queue\"", "\"a\\\"b\""};
  ChimiMetrics *aggregate = chimi_metrics_new_v1();
  for (size_t offset = 0; offset + 12 <= size; offset += 12) {
    const uint8_t *record = data + offset;
    int64_t kind = record[0] % 4;
    const char *name = names[record[1] % 4];
    char labels[48];
    int labels_length = snprintf(labels, sizeof labels, "{\"k\":\"%u\"}", (unsigned)(record[2] | (record[3] & 7) << 8));
    double value; memcpy(&value, record + 4, sizeof(value));
    if (!isfinite(value)) value = record[4];
    if (kind == 0) value = trunc(fabs(value));
    chimi_metrics_record_v1(aggregate, kind, name, strlen(name), value, labels, (size_t)labels_length);
    reference_metrics_record(kind, name, strlen(name), value, labels, (size_t)labels_length);
  }
  ChimiOwnedText document = chimi_metrics_document_v1(aggregate, -1, 1);
  char *want = reference_metrics_document(-1, 1);
  FUZZ_CHECK(document.length == strlen(want) && memcmp(document.bytes, want, document.length) == 0,
             "document %.*s, the C reference %s", (int)document.length, document.bytes, want);
  out_bytes(out, document.bytes, document.length - 1);
  chimi_text_release_v1(document);
  free(want);
  chimi_metrics_free_v1(aggregate);
  reference_metrics_reset();
}

static const FuzzTargetEntry FUZZ_TARGETS[] = {
  {"text", fuzz_text}, {"json", fuzz_json}, {"decimal", fuzz_decimal}, {"duration", fuzz_duration},
  {"float", fuzz_float}, {"metrics", fuzz_metrics},
};

/* Runs `target` on one input inside its own allocation scope, which frees
   everything the input allocated. The caller frees `out->data`. */
static void fz_run_one(FuzzTarget target, const uint8_t *data, size_t size, FuzzOutput *out) {
  fuzz_input = data;
  fuzz_input_size = size;
  AllocationScope scope;
  allocation_scope_begin(&scope);
  target(data, size, out);
  allocation_scope_end(&scope, NULL, NULL);
}

#include "fuzz-main.h"
