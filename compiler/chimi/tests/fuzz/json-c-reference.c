/* The C JSON decoder that runtime.c used before the decoder moved to Rust
   (runtime/chimi-runtime/src/json.rs, CHI-307), kept as the oracle for
   differential fuzzing: the json target checks that both decode every input
   to the same value or the same error. It is included after runtime.c and
   uses its value constructors.

   One change from the shipped code: a duplicate key's message kept its
   length, so a key holding U+0000 is quoted whole, as the interpreter and the
   Rust decoder quote it. The shipped code measured the message with strlen
   and cut it at the U+0000. */

typedef struct {
  const char *source; size_t length; size_t offset; const char *message; size_t message_length;
} ReferenceJsonParser;
static void reference_json_skip_space(ReferenceJsonParser *parser) {
  /* Not strchr: it also finds the terminator, so it would skip a U+0000 byte. */
  while (parser->offset < parser->length) {
    char next = parser->source[parser->offset];
    if (next != ' ' && next != '\t' && next != '\r' && next != '\n') break;
    ++parser->offset;
  }
}
static bool reference_json_hex(char value, uint32_t *digit) {
  if (value >= '0' && value <= '9') *digit = (uint32_t)(value - '0');
  else if (value >= 'a' && value <= 'f') *digit = (uint32_t)(value - 'a' + 10);
  else if (value >= 'A' && value <= 'F') *digit = (uint32_t)(value - 'A' + 10);
  else return false;
  return true;
}
/* Each decode error carries the message and byte offset that the reference
   interpreter's parser (compiler/chimi/src/json.rs) reports for the same input,
   so DecodeError.InvalidJson is identical on both backends. */
static void *reference_json_fail(ReferenceJsonParser *parser, size_t offset, const char *message) {
  parser->offset = offset; parser->message = message; parser->message_length = strlen(message); return NULL;
}
/* Reads four hex digits; on failure leaves the offset at the first digit. */
static bool reference_json_code_unit(ReferenceJsonParser *parser, uint32_t *value) {
  size_t start = parser->offset;
  if (start + 4 > parser->length) { reference_json_fail(parser, start, "incomplete JSON Unicode escape"); return false; }
  *value = 0;
  for (uint32_t index = 0; index < 4; ++index) {
    uint32_t digit;
    if (!reference_json_hex(parser->source[start + index], &digit)) {
      reference_json_fail(parser, start, "invalid JSON Unicode escape"); return false;
    }
    *value = *value * 16 + digit;
  }
  parser->offset = start + 4;
  return true;
}
/* A JSON string as Text; `\u0000` decodes to U+0000. NULL on failure. */
static ChimiValue *reference_json_parse_string(ReferenceJsonParser *parser) {
  if (parser->offset >= parser->length || parser->source[parser->offset++] != '"') return NULL;
  TextBuffer buffer = {0};
  while (parser->offset < parser->length) {
    unsigned char value = (unsigned char)parser->source[parser->offset];
    if (value < 0x20) return reference_json_fail(parser, parser->offset, "unescaped control character in JSON string");
    ++parser->offset;
    if (value == '"') return text_from_bytes(buffer.data ? buffer.data : "", buffer.length);
    if (value != '\\') { buffer_char(&buffer, (char)value); continue; }
    if (parser->offset >= parser->length) return reference_json_fail(parser, parser->offset, "unterminated JSON escape");
    char escaped = parser->source[parser->offset++];
    switch (escaped) {
      case '"': case '\\': case '/': buffer_char(&buffer, escaped); break;
      case 'b': buffer_char(&buffer, '\b'); break; case 'f': buffer_char(&buffer, '\f'); break;
      case 'n': buffer_char(&buffer, '\n'); break; case 'r': buffer_char(&buffer, '\r'); break;
      case 't': buffer_char(&buffer, '\t'); break;
      case 'u': {
        static const char *const unpaired = "high surrogate must be followed by a low surrogate";
        uint32_t code;
        if (!reference_json_code_unit(parser, &code)) return NULL;
        if (code >= 0xd800 && code <= 0xdbff) {
          if (parser->offset >= parser->length || parser->source[parser->offset] != '\\')
            return reference_json_fail(parser, parser->offset, unpaired);
          if (parser->offset + 1 >= parser->length || parser->source[parser->offset + 1] != 'u')
            return reference_json_fail(parser, parser->offset + 1, unpaired);
          parser->offset += 2; uint32_t low;
          if (!reference_json_code_unit(parser, &low)) return NULL;
          if (low < 0xdc00 || low > 0xdfff) return reference_json_fail(parser, parser->offset, unpaired);
          code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
        } else if (code >= 0xdc00 && code <= 0xdfff) {
          return reference_json_fail(parser, parser->offset, "isolated low surrogate in JSON string");
        }
        buffer_utf8(&buffer, code); break;
      }
      default: return reference_json_fail(parser, parser->offset, "invalid JSON escape");
    }
  }
  return reference_json_fail(parser, parser->offset, "unterminated JSON string");
}

static ChimiValue *reference_json_parse_value(ReferenceJsonParser *parser, uint32_t depth);
static ChimiValue *reference_json_parse_array(ReferenceJsonParser *parser, uint32_t depth) {
  ++parser->offset; reference_json_skip_space(parser);
  ChimiValue **items = NULL; uint32_t length = 0, capacity = 0;
  if (parser->offset < parser->length && parser->source[parser->offset] == ']') ++parser->offset;
  else for (;;) {
    ChimiValue *item = reference_json_parse_value(parser, depth + 1); if (!item) return NULL;
    if (length == capacity) {
      uint32_t old_capacity = capacity;
      capacity = capacity ? capacity * 2 : 4;
      items = reallocate_bytes(items, old_capacity * sizeof(*items), capacity * sizeof(*items));
      allocation_kind(items, ALLOCATION_VALUE_POINTERS);
    }
    items[length++] = item; reference_json_skip_space(parser);
    char separator = parser->offset < parser->length ? parser->source[parser->offset] : '\0';
    if (separator != ']' && separator != ',')
      return reference_json_fail(parser, parser->offset, "expected `,` or `]` in JSON array");
    ++parser->offset; if (separator == ']') break;
    reference_json_skip_space(parser);
  }
  ChimiValue *fields[] = {list_from_values(items, length)};
  return standard_value(CHIMI_CONSTRUCTOR_JSON_ARRAY, fields, 1);
}
static ChimiValue *reference_json_parse_object(ReferenceJsonParser *parser, uint32_t depth) {
  ++parser->offset; reference_json_skip_space(parser); MapNode *map = NULL;
  if (parser->offset < parser->length && parser->source[parser->offset] == '}') ++parser->offset;
  else for (;;) {
    if (parser->offset >= parser->length || parser->source[parser->offset] != '"')
      return reference_json_fail(parser, parser->offset, "expected a string key in JSON object");
    size_t key_offset = parser->offset;
    ChimiValue *key = reference_json_parse_string(parser); if (!key) return NULL;
    if (map_get_node(map, key)) {
      TextBuffer message = {0};
      static const char prefix[] = "duplicate JSON object key `";
      buffer_bytes(&message, prefix, sizeof(prefix) - 1);
      buffer_bytes(&message, key->as.text.bytes, key->as.text.length); buffer_char(&message, '`');
      reference_json_fail(parser, key_offset, message.data);
      parser->message_length = message.length;
      return NULL;
    }
    reference_json_skip_space(parser);
    if (parser->offset >= parser->length || parser->source[parser->offset] != ':')
      return reference_json_fail(parser, parser->offset, "expected `:` after JSON object key");
    ++parser->offset;
    ChimiValue *item = reference_json_parse_value(parser, depth + 1); if (!item) return NULL;
    map = map_put_node(map, key, item); reference_json_skip_space(parser);
    char separator = parser->offset < parser->length ? parser->source[parser->offset] : '\0';
    if (separator != '}' && separator != ',')
      return reference_json_fail(parser, parser->offset, "expected `,` or `}` in JSON object");
    ++parser->offset; if (separator == '}') break;
    reference_json_skip_space(parser);
  }
  ChimiValue *fields_value = map_value(map); ChimiValue *fields[] = {fields_value};
  return standard_value(CHIMI_CONSTRUCTOR_JSON_OBJECT, fields, 1);
}
static ChimiValue *reference_json_parse_number(ReferenceJsonParser *parser) {
  size_t start = parser->offset;
  if (parser->source[parser->offset] == '-') ++parser->offset;
  if (parser->offset >= parser->length) return reference_json_fail(parser, parser->offset, "expected digits in JSON number");
  if (parser->source[parser->offset] == '0') {
    ++parser->offset;
    if (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9')
      return reference_json_fail(parser, parser->offset, "JSON numbers cannot contain leading zeroes");
  } else if (parser->source[parser->offset] >= '1' && parser->source[parser->offset] <= '9')
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
  else return reference_json_fail(parser, parser->offset, "expected digits in JSON number");
  if (parser->offset < parser->length && parser->source[parser->offset] == '.') {
    ++parser->offset; size_t digits = parser->offset;
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
    if (digits == parser->offset) return reference_json_fail(parser, parser->offset, "expected digits after JSON decimal point");
  }
  if (parser->offset < parser->length && (parser->source[parser->offset] == 'e' || parser->source[parser->offset] == 'E')) {
    ++parser->offset; if (parser->offset < parser->length && (parser->source[parser->offset] == '+' || parser->source[parser->offset] == '-')) ++parser->offset;
    size_t digits = parser->offset;
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
    if (digits == parser->offset) return reference_json_fail(parser, parser->offset, "expected digits in JSON exponent");
  }
  size_t length = parser->offset - start; char *number = allocate(length + 1, 1);
  memcpy(number, parser->source + start, length); ChimiValue *raw = json_number_value(number);
  ChimiValue *fields[] = {raw}; return standard_value(CHIMI_CONSTRUCTOR_JSON_NUMBER, fields, 1);
}
static ChimiValue *reference_json_parse_value(ReferenceJsonParser *parser, uint32_t depth) {
  reference_json_skip_space(parser);
  if (depth > 128) return reference_json_fail(parser, parser->offset, "JSON nesting exceeds the configured limit");
  if (parser->offset >= parser->length) return reference_json_fail(parser, parser->offset, "expected a JSON value");
  char value = parser->source[parser->offset];
  if (value == 'n' && parser->length - parser->offset >= 4 && memcmp(parser->source + parser->offset, "null", 4) == 0) {
    parser->offset += 4; return standard_value(CHIMI_CONSTRUCTOR_JSON_NULL, NULL, 0);
  }
  if ((value == 't' && parser->length - parser->offset >= 4 && memcmp(parser->source + parser->offset, "true", 4) == 0) ||
      (value == 'f' && parser->length - parser->offset >= 5 && memcmp(parser->source + parser->offset, "false", 5) == 0)) {
    bool raw = value == 't'; parser->offset += raw ? 4 : 5; ChimiValue *boolean = boolean_value(raw);
    ChimiValue *fields[] = {boolean}; return standard_value(CHIMI_CONSTRUCTOR_JSON_BOOLEAN, fields, 1);
  }
  if (value == '"') { ChimiValue *raw = reference_json_parse_string(parser); if (!raw) return NULL;
    ChimiValue *fields[] = {raw}; return standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1); }
  if (value == '[') return reference_json_parse_array(parser, depth);
  if (value == '{') return reference_json_parse_object(parser, depth);
  if (value == '-' || (value >= '0' && value <= '9')) return reference_json_parse_number(parser);
  if (value == 'n' || value == 't' || value == 'f') return reference_json_fail(parser, parser->offset, "invalid JSON literal");
  return reference_json_fail(parser, parser->offset, "expected a JSON value");
}

/* The old CHIMI_INTRINSIC_JSON_DECODE. */
static ChimiControl *reference_json_decode(const ChimiValue *text) {
  ReferenceJsonParser parser = {text->as.text.bytes, text->as.text.length, 0, NULL, 0};
  ChimiValue *value = reference_json_parse_value(&parser, 0);
  if (value) reference_json_skip_space(&parser);
  if (value && parser.offset == parser.length) return success(value);
  if (!parser.message) reference_json_fail(&parser, parser.offset, "unexpected characters after JSON value");
  ChimiValue *fields[] = {text_from_bytes(parser.message, parser.message_length),
                          integer_value((int64_t)parser.offset)};
  return failure(standard_value(CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON, fields, 2));
}
