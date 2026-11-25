/* Fuzz targets for the native runtime's parsers and text code: JSON, decimal,
   duration and text. Each target turns one input into a canonical output line
   and checks the target's invariants, aborting on a broken one.

   The output is the contract a replacement must keep: when a subsystem moves
   to Rust (CHI-305), the same inputs run through both versions must print the
   same lines. `tests/runtime_fuzz.rs` replays the committed corpus against
   `corpus/<target>.expected`, and `scripts/fuzz-runtime.sh` runs libFuzzer.

   A corpus file, `corpus/<target>.txt`, holds one input per line: the input
   quoted as outputs are (see out_quoted), a tab, and the output the target
   printed for it when the file was last blessed.

   Built two ways:
     cc ... runtime-fuzz.c                       the replay driver:
         runtime-fuzz TARGET CORPUS              prints the corpus file again with
                                                 each output recomputed
         runtime-fuzz TARGET --files FILE...     prints one corpus line per file
         runtime-fuzz --unpack CORPUS DIR        writes each input to DIR/<line>
     clang -fsanitize=fuzzer -DCHIMI_FUZZ_LIBFUZZER ... runtime-fuzz.c
         CHIMI_FUZZ_TARGET=TARGET runtime-fuzz DIR...    libFuzzer

   Targets call the runtime's intrinsic entry points, as compiled programs do,
   so the checks hold for whatever implements those intrinsics. The runtime is
   included whole, with the few standard constructors the targets build in
   place of a compiled program's tables. */
#include "runtime.c"

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

/* The input that broke an invariant, set while a target runs. */
static const uint8_t *fuzz_input;
static size_t fuzz_input_size;

#define FUZZ_CHECK(condition, ...)                                                   \
  do {                                                                               \
    if (!(condition)) {                                                              \
      fprintf(stderr, "runtime-fuzz: invariant failed at %s:%d: %s: ", __FILE__,     \
              __LINE__, #condition);                                                 \
      fprintf(stderr, __VA_ARGS__);                                                  \
      fprintf(stderr, "\ninput (%zu bytes):", fuzz_input_size);                      \
      for (size_t at = 0; at < fuzz_input_size; ++at) fprintf(stderr, " %02x", fuzz_input[at]); \
      fputc('\n', stderr);                                                           \
      abort();                                                                       \
    }                                                                                \
  } while (0)

/* The canonical output, in plain malloc memory so it outlives the input's
   allocation scope. */
typedef struct { char *data; size_t length, capacity; } FuzzOutput;
static void out_bytes(FuzzOutput *out, const char *bytes, size_t length) {
  if (out->length + length + 1 > out->capacity) {
    size_t capacity = out->capacity ? out->capacity : 256;
    while (capacity < out->length + length + 1) capacity *= 2;
    out->data = realloc(out->data, capacity);
    if (!out->data) abort();
    out->capacity = capacity;
  }
  memcpy(out->data + out->length, bytes, length);
  out->length += length;
  out->data[out->length] = '\0';
}
static void out_word(FuzzOutput *out, const char *word) { out_bytes(out, word, strlen(word)); }
static void out_integer(FuzzOutput *out, int64_t value) {
  char text[24];
  snprintf(text, sizeof(text), "%" PRId64, value);
  out_word(out, text);
}
/* `bytes` quoted, with `"`, `\` and every byte outside printable ASCII escaped,
   so an output line holds no tab or newline of its own. */
static void out_quoted(FuzzOutput *out, const char *bytes, size_t length) {
  out_bytes(out, "\"", 1);
  for (size_t at = 0; at < length; ++at) {
    unsigned char byte = (unsigned char)bytes[at];
    if (byte == '"' || byte == '\\') {
      char escaped[2] = {'\\', (char)byte};
      out_bytes(out, escaped, 2);
    } else if (byte >= 0x20 && byte < 0x7f) {
      out_bytes(out, (const char *)&byte, 1);
    } else {
      char escaped[5];
      snprintf(escaped, sizeof(escaped), "\\x%02x", byte);
      out_bytes(out, escaped, 4);
    }
  }
  out_bytes(out, "\"", 1);
}
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
    covered += grapheme->as.text.length;
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
static void fuzz_json(const uint8_t *data, size_t size, FuzzOutput *out) {
  ChimiValue *text = fz_input_text(data, size);
  ChimiControl *decoded = fz_json_decode(text);
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
/* Reads `text` as a Decimal and checks that its printed form reads back to the
   same printed form. */
static ChimiValue *fz_read_operand(ChimiValue *text, FuzzOutput *out, const char *name) {
  ChimiValue *decimal = fz_decimal_read(text);
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
  for (size_t index = 0; index < COUNT_OF(operations); ++index) {
    ChimiValue *arguments[] = {left, right};
    out_word(out, operations[index].name);
    fz_out_decimal(out, fz_option(call_decimal(operations[index].intrinsic, arguments, 2)->value));
  }
  /* Decimal.divide crashes on a zero divisor before reaching the intrinsic. */
  if (decimal_of(right).magnitude == 0) return;
  uint8_t pick = size ? data[0] : 0;
  int64_t scale = pick % (DECIMAL_MAX_SCALE + 1), rounding = pick % (ROUND_HALF_EVEN + 1);
  ChimiValue *arguments[] = {left, right, integer_value(scale), integer_value(rounding)};
  out_word(out, " divide("); out_integer(out, scale); out_word(out, ","); out_integer(out, rounding);
  out_word(out, ")=");
  fz_out_decimal(out, fz_option(call_decimal(CHIMI_INTRINSIC_DECIMAL_DIVIDE_ROUNDED, arguments, 4)->value));
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
static void fuzz_duration(const uint8_t *data, size_t size, FuzzOutput *out) {
  ChimiControl *parsed = fz_duration_parse(fz_input_text(data, size));
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

typedef void (*FuzzTarget)(const uint8_t *, size_t, FuzzOutput *);
static const struct { const char *name; FuzzTarget run; } TARGETS[] = {
  {"text", fuzz_text}, {"json", fuzz_json}, {"decimal", fuzz_decimal}, {"duration", fuzz_duration},
};
static FuzzTarget fz_find_target(const char *name) {
  for (size_t index = 0; name && index < COUNT_OF(TARGETS); ++index)
    if (strcmp(TARGETS[index].name, name) == 0) return TARGETS[index].run;
  fprintf(stderr, "runtime-fuzz: unknown target `%s`; targets:", name ? name : "(none)");
  for (size_t index = 0; index < COUNT_OF(TARGETS); ++index) fprintf(stderr, " %s", TARGETS[index].name);
  fputc('\n', stderr);
  return NULL;
}

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

#ifdef CHIMI_FUZZ_LIBFUZZER
static FuzzTarget libfuzzer_target;
int LLVMFuzzerInitialize(int *argc, char ***argv) {
  (void)argc; (void)argv;
  libfuzzer_target = fz_find_target(getenv("CHIMI_FUZZ_TARGET"));
  if (!libfuzzer_target) exit(2);
  return 0;
}
int LLVMFuzzerTestOneInput(const uint8_t *data, size_t size) {
  FuzzOutput out = {0};
  fz_run_one(libfuzzer_target, data, size, &out);
  free(out.data);
  return 0;
}
#else
static uint8_t *fz_read_file(const char *path, size_t *size) {
  FILE *file = fopen(path, "rb");
  if (!file) return NULL;
  uint8_t *data = NULL;
  size_t length = 0, capacity = 0;
  for (;;) {
    if (length == capacity) {
      capacity = capacity ? capacity * 2 : 4096;
      data = realloc(data, capacity);
      if (!data) abort();
    }
    size_t read = fread(data + length, 1, capacity - length, file);
    length += read;
    if (read == 0) break;
  }
  bool failed = ferror(file);
  fclose(file);
  if (failed) { free(data); return NULL; }
  *size = length;
  return data;
}
static int fz_hex(char digit) {
  if (digit >= '0' && digit <= '9') return digit - '0';
  if (digit >= 'a' && digit <= 'f') return digit - 'a' + 10;
  return -1;
}
/* The input quoted at the start of `line` (out_quoted's form), into `input`;
   false when the line is not in that form. */
static bool fz_unquote(const char *line, size_t length, FuzzOutput *input) {
  if (length < 2 || line[0] != '"') return false;
  for (size_t at = 1; at < length; ++at) {
    char next = line[at];
    if (next == '"') return at + 1 == length || line[at + 1] == '\t';
    if (next != '\\') { out_bytes(input, &next, 1); continue; }
    if (++at >= length) return false;
    if (line[at] == '"' || line[at] == '\\') { out_bytes(input, &line[at], 1); continue; }
    if (line[at] != 'x' || at + 2 >= length) return false;
    int high = fz_hex(line[at + 1]), low = fz_hex(line[at + 2]);
    if (high < 0 || low < 0) return false;
    char byte = (char)(high << 4 | low);
    out_bytes(input, &byte, 1);
    at += 2;
  }
  return false;
}
static void fz_print_line(FuzzTarget target, const uint8_t *data, size_t size) {
  FuzzOutput quoted = {0}, out = {0};
  out_quoted(&quoted, (const char *)data, size);
  fz_run_one(target, data, size, &out);
  printf("%s\t%s\n", quoted.data, out.data ? out.data : "");
  free(quoted.data);
  free(out.data);
}
/* Calls `each` with every input of the corpus file `path`, in order; 2 when
   the file cannot be read or a line is malformed. */
static int fz_each_input(const char *path, void (*each)(void *, const uint8_t *, size_t, size_t), void *context) {
  size_t size = 0;
  uint8_t *corpus = fz_read_file(path, &size);
  if (!corpus) {
    fprintf(stderr, "runtime-fuzz: cannot read %s\n", path);
    return 2;
  }
  size_t number = 0;
  for (size_t start = 0; start < size; ++number) {
    const uint8_t *newline = memchr(corpus + start, '\n', size - start);
    size_t end = newline ? (size_t)(newline - corpus) : size;
    FuzzOutput input = {0};
    if (!fz_unquote((const char *)corpus + start, end - start, &input)) {
      fprintf(stderr, "runtime-fuzz: %s:%zu: not a quoted input\n", path, number + 1);
      free(input.data);
      free(corpus);
      return 2;
    }
    each(context, (const uint8_t *)(input.data ? input.data : ""), input.length, number);
    free(input.data);
    start = end + 1;
  }
  free(corpus);
  return 0;
}
static void fz_replay_input(void *context, const uint8_t *data, size_t size, size_t number) {
  (void)number;
  fz_print_line((FuzzTarget)context, data, size);
}
static void fz_unpack_input(void *context, const uint8_t *data, size_t size, size_t number) {
  char path[4096];
  snprintf(path, sizeof(path), "%s/%06zu", (const char *)context, number + 1);
  FILE *file = fopen(path, "wb");
  if (!file || fwrite(data, 1, size, file) != size || fclose(file) != 0) {
    fprintf(stderr, "runtime-fuzz: cannot write %s\n", path);
    exit(2);
  }
}
int main(int argc, char **argv) {
  if (argc == 4 && strcmp(argv[1], "--unpack") == 0) return fz_each_input(argv[2], fz_unpack_input, argv[3]);
  if (argc < 3) {
    fputs("usage: runtime-fuzz TARGET CORPUS | TARGET --files FILE... | --unpack CORPUS DIR\n", stderr);
    return 2;
  }
  FuzzTarget target = fz_find_target(argv[1]);
  if (!target) return 2;
  if (strcmp(argv[2], "--files") != 0) {
    if (argc != 3) {
      fputs("runtime-fuzz: replay takes one corpus file\n", stderr);
      return 2;
    }
    return fz_each_input(argv[2], fz_replay_input, (void *)target);
  }
  for (int index = 3; index < argc; ++index) {
    size_t size = 0;
    uint8_t *data = fz_read_file(argv[index], &size);
    if (!data) {
      fprintf(stderr, "runtime-fuzz: cannot read %s\n", argv[index]);
      return 2;
    }
    fz_print_line(target, data, size);
    free(data);
  }
  return 0;
}
#endif
