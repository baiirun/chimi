/* Chimi native runtime ABI version 3. */
#include "runtime.h"

#include <inttypes.h>
#include <errno.h>
#include <limits.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include <unistd.h>

typedef enum { VALUE_INTEGER, VALUE_TEXT, VALUE_BOOLEAN, VALUE_LIST, VALUE_RECORD,
               VALUE_CONSTRUCTED, VALUE_CALLABLE, VALUE_MAP, VALUE_JSON_NUMBER } ValueKind;
typedef enum { CALL_DECLARED, CALL_CLOSURE, CALL_DEPENDENCY, CALL_INTRINSIC } CallableKind;
typedef enum { CONTROL_SUCCESS, CONTROL_FAILURE, CONTROL_DEFECT, CONTROL_CANCEL } ControlKind;

typedef struct ChimiValue ChimiValue;
typedef struct { const char *name; ChimiValue *value; } RuntimeField;
typedef struct MapNode MapNode;

struct MapNode {
  const char *key;
  ChimiValue *value;
  MapNode *left;
  MapNode *right;
  uint16_t height;
};

typedef struct ChimiCallable {
  CallableKind kind;
  uint32_t symbol;
  uint32_t body;
  uint32_t parameters_offset;
  uint32_t parameters_length;
  ChimiEnvironment *environment;
  struct ChimiCallable *inner;
  uint32_t requirement;
  uint32_t interface_symbol;
  uint32_t method;
  const char *name;
  uint32_t intrinsic;
} ChimiCallable;

struct ChimiValue {
  ValueKind kind;
  union {
    int64_t integer;
    const char *text;
    bool boolean;
    struct { ChimiValue **items; uint32_t length; } list;
    struct { RuntimeField *fields; uint32_t length; } record;
    struct { uint32_t constructor; const char *type_name; const char *variant_name;
             RuntimeField *fields; uint32_t length; } constructed;
    ChimiCallable *callable;
    MapNode *map;
    const char *json_number;
  } as;
};

struct ChimiEnvironment {
  ChimiValue **locals;
  ChimiValue **requirements;
};

struct ChimiControl {
  ControlKind kind;
  ChimiValue *value;
  const char *message;
  int64_t node;
};

struct ChimiRuntime {
  const char *execution_id;
  bool full_trace;
  bool cancel;
  bool silent;
  uint64_t next_sequence;
  uint64_t next_span;
  bool *active_bindings;
};

static void *allocate(size_t count, size_t size) {
  void *value = calloc(count == 0 ? 1 : count, size);
  if (value == NULL) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  return value;
}

static ChimiValue *new_value(ValueKind kind) {
  ChimiValue *value = allocate(1, sizeof(*value)); value->kind = kind; return value;
}
static ChimiControl *new_control(ControlKind kind, ChimiValue *value, const char *message, int64_t node) {
  ChimiControl *control = allocate(1, sizeof(*control));
  control->kind = kind; control->value = value; control->message = message; control->node = node;
  return control;
}
static ChimiControl *success(ChimiValue *value) { return new_control(CONTROL_SUCCESS, value, NULL, -1); }
static ChimiControl *failure(ChimiValue *value) { return new_control(CONTROL_FAILURE, value, NULL, -1); }
static ChimiControl *defect(const char *message, int64_t node) { return new_control(CONTROL_DEFECT, NULL, message, node); }
static ChimiControl *cancelled(void) { return new_control(CONTROL_CANCEL, NULL, NULL, -1); }

static ChimiEnvironment *new_environment(void) {
  ChimiEnvironment *environment = allocate(1, sizeof(*environment));
  environment->locals = allocate(CHIMI_SYMBOL_COUNT, sizeof(*environment->locals));
  environment->requirements = allocate(CHIMI_SYMBOL_COUNT, sizeof(*environment->requirements));
  return environment;
}
static ChimiEnvironment *clone_environment(const ChimiEnvironment *source) {
  ChimiEnvironment *environment = new_environment();
  memcpy(environment->locals, source->locals, CHIMI_SYMBOL_COUNT * sizeof(*environment->locals));
  memcpy(environment->requirements, source->requirements,
         CHIMI_SYMBOL_COUNT * sizeof(*environment->requirements));
  return environment;
}

static const ChimiFunction *find_function(uint32_t symbol) {
  for (uint32_t index = 0; index < CHIMI_FUNCTION_COUNT; ++index)
    if (CHIMI_FUNCTIONS[index].symbol == symbol) return &CHIMI_FUNCTIONS[index];
  return NULL;
}

static ChimiControl *run_node(uint32_t node, ChimiRuntime *runtime,
                              ChimiEnvironment *environment, uint64_t parent_span) {
  if (runtime->cancel) return cancelled();
  return CHIMI_NODE_FUNCTIONS[node](runtime, environment, parent_span);
}

static const char *control_name(const ChimiControl *control) {
  switch (control->kind) {
    case CONTROL_SUCCESS: return "success";
    case CONTROL_FAILURE: return "expected_failure";
    case CONTROL_DEFECT: return "defect";
    case CONTROL_CANCEL: return "cancellation";
  }
  return "defect";
}

static void json_string(const char *value) {
  putchar('"');
  for (const unsigned char *cursor = (const unsigned char *)value; *cursor; ++cursor) {
    switch (*cursor) {
      case '"': fputs("\\\"", stdout); break;
      case '\\': fputs("\\\\", stdout); break;
      case '\n': fputs("\\n", stdout); break;
      case '\r': fputs("\\r", stdout); break;
      case '\t': fputs("\\t", stdout); break;
      default:
        if (*cursor < 0x20) printf("\\u%04x", *cursor); else putchar(*cursor);
    }
  }
  putchar('"');
}

static void json_value(const ChimiValue *value);
static void json_fields(const RuntimeField *fields, uint32_t length) {
  putchar('{');
  for (uint32_t index = 0; index < length; ++index) {
    if (index) putchar(','); json_string(fields[index].name); putchar(':'); json_value(fields[index].value);
  }
  putchar('}');
}
static void json_value(const ChimiValue *value) {
  switch (value->kind) {
    case VALUE_INTEGER: printf("%" PRId64, value->as.integer); break;
    case VALUE_TEXT: json_string(value->as.text); break;
    case VALUE_BOOLEAN: fputs(value->as.boolean ? "true" : "false", stdout); break;
    case VALUE_LIST:
      putchar('['); for (uint32_t i = 0; i < value->as.list.length; ++i) {
        if (i) putchar(','); json_value(value->as.list.items[i]);
      } putchar(']'); break;
    case VALUE_RECORD: json_fields(value->as.record.fields, value->as.record.length); break;
    case VALUE_CONSTRUCTED:
      fputs("{\"type\":", stdout); json_string(value->as.constructed.type_name);
      if (value->as.constructed.variant_name) {
        fputs(",\"variant\":", stdout); json_string(value->as.constructed.variant_name);
      }
      fputs(",\"fields\":", stdout);
      json_fields(value->as.constructed.fields, value->as.constructed.length); putchar('}'); break;
    case VALUE_CALLABLE:
      fputs("{\"callable\":", stdout); json_string(value->as.callable->name); putchar('}'); break;
    case VALUE_MAP: fputs("{\"map\":\"opaque\"}", stdout); break;
    case VALUE_JSON_NUMBER: json_string(value->as.json_number); break;
  }
}

static void emit_event(ChimiRuntime *runtime, const char *event, uint32_t node_id,
                       uint64_t span, int64_t parent, const char *operation,
                       const ChimiControl *outcome) {
  const ChimiNode *node = &CHIMI_NODES[node_id];
  const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  fputs("{\"event\":", stdout); json_string(event);
  printf(",\"sequence\":%" PRIu64 ",\"computation\":", runtime->next_sequence++);
  json_string(CHIMI_COMPUTATION); fputs(",\"execution_id\":", stdout); json_string(runtime->execution_id);
  printf(",\"span_id\":%" PRIu64 ",\"parent_span_id\":", span);
  if (parent < 0) fputs("null", stdout); else printf("%" PRId64, parent);
  printf(",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":",
         node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  json_string(operation); fputs(",\"attributes\":{", stdout);
  if (outcome) { fputs("\"outcome\":", stdout); json_string(control_name(outcome)); }
  fputs("}}\n", stdout);
}

static void emit_dependency(ChimiRuntime *runtime, uint32_t node_id, uint64_t span,
                            uint64_t parent, const ChimiCallable *callable) {
  const ChimiNode *node = &CHIMI_NODES[node_id]; const ChimiOrigin *origin = &CHIMI_ORIGINS[node->origin];
  fputs("{\"event\":\"dependency_called\",\"sequence\":", stdout);
  printf("%" PRIu64 ",\"computation\":", runtime->next_sequence++); json_string(CHIMI_COMPUTATION);
  fputs(",\"execution_id\":", stdout); json_string(runtime->execution_id);
  printf(",\"span_id\":%" PRIu64 ",\"parent_span_id\":%" PRIu64
         ",\"core_node_id\":%u,\"origin_id\":%u,\"semantic_id\":%u,\"file_id\":%u,"
         "\"source_start\":%u,\"source_end\":%u,\"operation\":\"dependency_call\",\"attributes\":{",
         span, parent, node_id, node->origin, origin->semantic_id, origin->file_id, origin->start, origin->end);
  fputs("\"requirement\":", stdout); json_string(CHIMI_STRINGS[CHIMI_SYMBOLS[callable->requirement].name]);
  printf(",\"requirement_symbol_id\":%u,\"interface_symbol_id\":%u,\"method\":",
         callable->requirement, callable->interface_symbol); json_string(callable->name);
  printf(",\"method_semantic_id\":%u}}\n", callable->method);
}

static uint64_t begin_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent, const char *operation) {
  if (runtime->silent) return parent;
  uint64_t span = runtime->next_span++;
  if (runtime->full_trace) emit_event(runtime, "operation_started", node, span, (int64_t)parent, operation, NULL);
  return span;
}
static ChimiControl *finish_node(ChimiRuntime *runtime, uint32_t node, uint64_t parent,
                                 uint64_t span, const char *operation, ChimiControl *control) {
  if (runtime->full_trace) emit_event(runtime, "operation_completed", node, span, (int64_t)parent, operation, control);
  return control;
}

static ChimiValue *integer_value(int64_t raw) { ChimiValue *v = new_value(VALUE_INTEGER); v->as.integer = raw; return v; }
static ChimiValue *text_value(const char *raw) { ChimiValue *v = new_value(VALUE_TEXT); v->as.text = raw; return v; }
static ChimiValue *boolean_value(bool raw) { ChimiValue *v = new_value(VALUE_BOOLEAN); v->as.boolean = raw; return v; }
static ChimiValue *callable_value(ChimiCallable *raw) { ChimiValue *v = new_value(VALUE_CALLABLE); v->as.callable = raw; return v; }
static ChimiValue *map_value(MapNode *raw) { ChimiValue *v = new_value(VALUE_MAP); v->as.map = raw; return v; }
static ChimiValue *json_number_value(const char *raw) { ChimiValue *v = new_value(VALUE_JSON_NUMBER); v->as.json_number = raw; return v; }

int32_t chimi_rt_full_trace(const ChimiRuntime *runtime) { return runtime->full_trace; }
ChimiControl *chimi_rt_direct_integer_success(ChimiRuntime *runtime, uint32_t node_count,
                                               int64_t raw) {
  if (!runtime->silent) runtime->next_span += node_count;
  return success(integer_value(raw));
}

static ChimiValue *select_value(ChimiValue *value, const char *name) {
  RuntimeField *fields = NULL; uint32_t length = 0;
  if (value->kind == VALUE_RECORD) { fields = value->as.record.fields; length = value->as.record.length; }
  else if (value->kind == VALUE_CONSTRUCTED) { fields = value->as.constructed.fields; length = value->as.constructed.length; }
  for (uint32_t index = 0; index < length; ++index)
    if (strcmp(fields[index].name, name) == 0) return fields[index].value;
  return NULL;
}

static uint16_t map_height(const MapNode *node) { return node ? node->height : 0; }
static uint16_t map_max(uint16_t left, uint16_t right) { return left > right ? left : right; }
static MapNode *map_node(const char *key, ChimiValue *value, MapNode *left, MapNode *right) {
  MapNode *node = allocate(1, sizeof(*node));
  node->key = key; node->value = value; node->left = left; node->right = right;
  node->height = (uint16_t)(1 + map_max(map_height(left), map_height(right)));
  return node;
}
static MapNode *map_rotate_left(MapNode *node) {
  MapNode *right = node->right;
  MapNode *left = map_node(node->key, node->value, node->left, right->left);
  return map_node(right->key, right->value, left, right->right);
}
static MapNode *map_rotate_right(MapNode *node) {
  MapNode *left = node->left;
  MapNode *right = map_node(node->key, node->value, left->right, node->right);
  return map_node(left->key, left->value, left->left, right);
}
static MapNode *map_balance(MapNode *node) {
  int balance = (int)map_height(node->left) - (int)map_height(node->right);
  if (balance > 1) {
    if (map_height(node->left->right) > map_height(node->left->left))
      node = map_node(node->key, node->value, map_rotate_left(node->left), node->right);
    return map_rotate_right(node);
  }
  if (balance < -1) {
    if (map_height(node->right->left) > map_height(node->right->right))
      node = map_node(node->key, node->value, node->left, map_rotate_right(node->right));
    return map_rotate_left(node);
  }
  return node;
}
static MapNode *map_put_node(MapNode *node, const char *key, ChimiValue *value) {
  if (!node) return map_node(key, value, NULL, NULL);
  int order = strcmp(key, node->key);
  if (order == 0) return map_node(key, value, node->left, node->right);
  if (order < 0) return map_balance(map_node(node->key, node->value,
                                            map_put_node(node->left, key, value), node->right));
  return map_balance(map_node(node->key, node->value, node->left,
                              map_put_node(node->right, key, value)));
}
static ChimiValue *map_get_node(MapNode *node, const char *key) {
  while (node) {
    int order = strcmp(key, node->key);
    if (order == 0) return node->value;
    node = order < 0 ? node->left : node->right;
  }
  return NULL;
}

static uint32_t standard_constructor_symbol(uint32_t standard_constructor) {
  for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
    if (CHIMI_SYMBOLS[symbol].standard_constructor == standard_constructor) return symbol;
  return UINT32_MAX;
}
static ChimiValue *standard_value(uint32_t standard_constructor, ChimiValue **values,
                                  uint32_t value_count) {
  uint32_t symbol = standard_constructor_symbol(standard_constructor);
  if (symbol == UINT32_MAX) return NULL;
  const ChimiConstructor *shape = &CHIMI_CONSTRUCTORS[CHIMI_SYMBOLS[symbol].constructor];
  if (shape->fields_length != value_count) return NULL;
  ChimiValue *value = new_value(VALUE_CONSTRUCTED);
  value->as.constructed.constructor = symbol;
  value->as.constructed.type_name = CHIMI_STRINGS[shape->type_name];
  value->as.constructed.variant_name = shape->variant_name == UINT32_MAX ? NULL : CHIMI_STRINGS[shape->variant_name];
  value->as.constructed.length = value_count;
  value->as.constructed.fields = allocate(value_count, sizeof(*value->as.constructed.fields));
  for (uint32_t field = 0; field < value_count; ++field)
    value->as.constructed.fields[field] = (RuntimeField){
      CHIMI_STRINGS[CHIMI_IDS[shape->fields_offset + field]], values[field]
    };
  return value;
}

typedef struct { char *data; size_t length; size_t capacity; } TextBuffer;
static void buffer_reserve(TextBuffer *buffer, size_t extra) {
  if (buffer->length + extra + 1 <= buffer->capacity) return;
  size_t capacity = buffer->capacity ? buffer->capacity : 64;
  while (capacity < buffer->length + extra + 1) capacity *= 2;
  char *data = realloc(buffer->data, capacity);
  if (!data) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); }
  buffer->data = data; buffer->capacity = capacity;
}
static void buffer_bytes(TextBuffer *buffer, const char *value, size_t length) {
  buffer_reserve(buffer, length); memcpy(buffer->data + buffer->length, value, length);
  buffer->length += length; buffer->data[buffer->length] = '\0';
}
static void buffer_char(TextBuffer *buffer, char value) { buffer_bytes(buffer, &value, 1); }
static void buffer_json_string(TextBuffer *buffer, const char *value) {
  buffer_char(buffer, '"');
  for (const unsigned char *cursor = (const unsigned char *)value; *cursor; ++cursor) {
    switch (*cursor) {
      case '"': buffer_bytes(buffer, "\\\"", 2); break;
      case '\\': buffer_bytes(buffer, "\\\\", 2); break;
      case '\b': buffer_bytes(buffer, "\\b", 2); break;
      case '\f': buffer_bytes(buffer, "\\f", 2); break;
      case '\n': buffer_bytes(buffer, "\\n", 2); break;
      case '\r': buffer_bytes(buffer, "\\r", 2); break;
      case '\t': buffer_bytes(buffer, "\\t", 2); break;
      default:
        if (*cursor < 0x20) {
          char escaped[7]; snprintf(escaped, sizeof(escaped), "\\u%04x", *cursor);
          buffer_bytes(buffer, escaped, 6);
        } else buffer_char(buffer, (char)*cursor);
    }
  }
  buffer_char(buffer, '"');
}

static bool encode_json_value(TextBuffer *buffer, const ChimiValue *value);
static bool encode_json_map(TextBuffer *buffer, const MapNode *node, bool *first) {
  if (!node) return true;
  if (!encode_json_map(buffer, node->left, first)) return false;
  if (!*first) buffer_char(buffer, ','); *first = false;
  buffer_json_string(buffer, node->key); buffer_char(buffer, ':');
  if (!encode_json_value(buffer, node->value)) return false;
  return encode_json_map(buffer, node->right, first);
}
static bool encode_json_value(TextBuffer *buffer, const ChimiValue *value) {
  if (value->kind != VALUE_CONSTRUCTED) return false;
  uint32_t kind = CHIMI_SYMBOLS[value->as.constructed.constructor].standard_constructor;
  ChimiValue *field = value->as.constructed.length ? value->as.constructed.fields[0].value : NULL;
  switch (kind) {
    case CHIMI_CONSTRUCTOR_JSON_NULL: buffer_bytes(buffer, "null", 4); return true;
    case CHIMI_CONSTRUCTOR_JSON_BOOLEAN:
      if (!field || field->kind != VALUE_BOOLEAN) return false;
      buffer_bytes(buffer, field->as.boolean ? "true" : "false", field->as.boolean ? 4 : 5); return true;
    case CHIMI_CONSTRUCTOR_JSON_NUMBER:
      if (!field || field->kind != VALUE_JSON_NUMBER) return false;
      buffer_bytes(buffer, field->as.json_number, strlen(field->as.json_number)); return true;
    case CHIMI_CONSTRUCTOR_JSON_TEXT:
      if (!field || field->kind != VALUE_TEXT) return false;
      buffer_json_string(buffer, field->as.text); return true;
    case CHIMI_CONSTRUCTOR_JSON_ARRAY:
      if (!field || field->kind != VALUE_LIST) return false;
      buffer_char(buffer, '[');
      for (uint32_t index = 0; index < field->as.list.length; ++index) {
        if (index) buffer_char(buffer, ',');
        if (!encode_json_value(buffer, field->as.list.items[index])) return false;
      }
      buffer_char(buffer, ']'); return true;
    case CHIMI_CONSTRUCTOR_JSON_OBJECT: {
      if (!field || field->kind != VALUE_MAP) return false;
      buffer_char(buffer, '{'); bool first = true;
      if (!encode_json_map(buffer, field->as.map, &first)) return false;
      buffer_char(buffer, '}'); return true;
    }
    default: return false;
  }
}

typedef struct { const char *source; size_t length; size_t offset; const char *message; } JsonParser;
static void json_skip_space(JsonParser *parser) {
  while (parser->offset < parser->length &&
         strchr(" \t\r\n", parser->source[parser->offset])) ++parser->offset;
}
static bool json_hex(char value, uint32_t *digit) {
  if (value >= '0' && value <= '9') *digit = (uint32_t)(value - '0');
  else if (value >= 'a' && value <= 'f') *digit = (uint32_t)(value - 'a' + 10);
  else if (value >= 'A' && value <= 'F') *digit = (uint32_t)(value - 'A' + 10);
  else return false;
  return true;
}
static bool json_code_unit(JsonParser *parser, uint32_t *value) {
  if (parser->offset + 4 > parser->length) return false;
  *value = 0;
  for (uint32_t index = 0; index < 4; ++index) {
    uint32_t digit;
    if (!json_hex(parser->source[parser->offset++], &digit)) return false;
    *value = *value * 16 + digit;
  }
  return true;
}
static void buffer_utf8(TextBuffer *buffer, uint32_t value) {
  if (value <= 0x7f) buffer_char(buffer, (char)value);
  else if (value <= 0x7ff) {
    buffer_char(buffer, (char)(0xc0 | value >> 6)); buffer_char(buffer, (char)(0x80 | (value & 0x3f)));
  } else if (value <= 0xffff) {
    buffer_char(buffer, (char)(0xe0 | value >> 12)); buffer_char(buffer, (char)(0x80 | ((value >> 6) & 0x3f)));
    buffer_char(buffer, (char)(0x80 | (value & 0x3f)));
  } else {
    buffer_char(buffer, (char)(0xf0 | value >> 18)); buffer_char(buffer, (char)(0x80 | ((value >> 12) & 0x3f)));
    buffer_char(buffer, (char)(0x80 | ((value >> 6) & 0x3f))); buffer_char(buffer, (char)(0x80 | (value & 0x3f)));
  }
}
static char *json_parse_string(JsonParser *parser) {
  if (parser->offset >= parser->length || parser->source[parser->offset++] != '"') return NULL;
  TextBuffer buffer = {0};
  while (parser->offset < parser->length) {
    unsigned char value = (unsigned char)parser->source[parser->offset++];
    if (value == '"') return buffer.data ? buffer.data : strdup("");
    if (value < 0x20) { parser->message = "unescaped control character"; return NULL; }
    if (value != '\\') { buffer_char(&buffer, (char)value); continue; }
    if (parser->offset >= parser->length) break;
    char escaped = parser->source[parser->offset++];
    switch (escaped) {
      case '"': case '\\': case '/': buffer_char(&buffer, escaped); break;
      case 'b': buffer_char(&buffer, '\b'); break; case 'f': buffer_char(&buffer, '\f'); break;
      case 'n': buffer_char(&buffer, '\n'); break; case 'r': buffer_char(&buffer, '\r'); break;
      case 't': buffer_char(&buffer, '\t'); break;
      case 'u': {
        uint32_t code;
        if (!json_code_unit(parser, &code)) { parser->message = "invalid Unicode escape"; return NULL; }
        if (code >= 0xd800 && code <= 0xdbff) {
          if (parser->offset + 2 > parser->length || parser->source[parser->offset] != '\\' ||
              parser->source[parser->offset + 1] != 'u') { parser->message = "missing low surrogate"; return NULL; }
          parser->offset += 2; uint32_t low;
          if (!json_code_unit(parser, &low) || low < 0xdc00 || low > 0xdfff) {
            parser->message = "invalid low surrogate"; return NULL;
          }
          code = 0x10000 + ((code - 0xd800) << 10) + (low - 0xdc00);
        } else if (code >= 0xdc00 && code <= 0xdfff) {
          parser->message = "unexpected low surrogate"; return NULL;
        }
        buffer_utf8(&buffer, code); break;
      }
      default: parser->message = "invalid string escape"; return NULL;
    }
  }
  parser->message = "unterminated string"; return NULL;
}

static ChimiValue *json_parse_value(JsonParser *parser, uint32_t depth);
static ChimiValue *json_parse_array(JsonParser *parser, uint32_t depth) {
  ++parser->offset; json_skip_space(parser);
  ChimiValue **items = NULL; uint32_t length = 0, capacity = 0;
  if (parser->offset < parser->length && parser->source[parser->offset] == ']') ++parser->offset;
  else for (;;) {
    ChimiValue *item = json_parse_value(parser, depth + 1); if (!item) return NULL;
    if (length == capacity) { capacity = capacity ? capacity * 2 : 4; items = realloc(items, capacity * sizeof(*items));
      if (!items) { fputs("Chimi runtime exhausted memory\n", stderr); exit(70); } }
    items[length++] = item; json_skip_space(parser);
    if (parser->offset >= parser->length) { parser->message = "unterminated array"; return NULL; }
    char separator = parser->source[parser->offset++]; if (separator == ']') break;
    if (separator != ',') { parser->message = "expected comma in array"; return NULL; }
    json_skip_space(parser);
  }
  ChimiValue *list = new_value(VALUE_LIST);
  list->as.list.items = items; list->as.list.length = length;
  ChimiValue *fields[] = {list}; return standard_value(CHIMI_CONSTRUCTOR_JSON_ARRAY, fields, 1);
}
static ChimiValue *json_parse_object(JsonParser *parser, uint32_t depth) {
  ++parser->offset; json_skip_space(parser); MapNode *map = NULL;
  if (parser->offset < parser->length && parser->source[parser->offset] == '}') ++parser->offset;
  else for (;;) {
    if (parser->offset >= parser->length || parser->source[parser->offset] != '"') {
      parser->message = "expected object key"; return NULL;
    }
    char *key = json_parse_string(parser); if (!key) return NULL; json_skip_space(parser);
    if (parser->offset >= parser->length || parser->source[parser->offset++] != ':') {
      parser->message = "expected colon after object key"; return NULL;
    }
    if (map_get_node(map, key)) { parser->message = "duplicate object key"; return NULL; }
    ChimiValue *item = json_parse_value(parser, depth + 1); if (!item) return NULL;
    map = map_put_node(map, key, item); json_skip_space(parser);
    if (parser->offset >= parser->length) { parser->message = "unterminated object"; return NULL; }
    char separator = parser->source[parser->offset++]; if (separator == '}') break;
    if (separator != ',') { parser->message = "expected comma in object"; return NULL; }
    json_skip_space(parser);
  }
  ChimiValue *fields_value = map_value(map); ChimiValue *fields[] = {fields_value};
  return standard_value(CHIMI_CONSTRUCTOR_JSON_OBJECT, fields, 1);
}
static ChimiValue *json_parse_number(JsonParser *parser) {
  size_t start = parser->offset;
  if (parser->source[parser->offset] == '-') ++parser->offset;
  if (parser->offset >= parser->length) { parser->message = "invalid number"; return NULL; }
  if (parser->source[parser->offset] == '0') ++parser->offset;
  else if (parser->source[parser->offset] >= '1' && parser->source[parser->offset] <= '9')
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
  else { parser->message = "invalid number"; return NULL; }
  if (parser->offset < parser->length && parser->source[parser->offset] == '.') {
    ++parser->offset; size_t digits = parser->offset;
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
    if (digits == parser->offset) { parser->message = "invalid fraction"; return NULL; }
  }
  if (parser->offset < parser->length && (parser->source[parser->offset] == 'e' || parser->source[parser->offset] == 'E')) {
    ++parser->offset; if (parser->offset < parser->length && (parser->source[parser->offset] == '+' || parser->source[parser->offset] == '-')) ++parser->offset;
    size_t digits = parser->offset;
    while (parser->offset < parser->length && parser->source[parser->offset] >= '0' && parser->source[parser->offset] <= '9') ++parser->offset;
    if (digits == parser->offset) { parser->message = "invalid exponent"; return NULL; }
  }
  size_t length = parser->offset - start; char *number = allocate(length + 1, 1);
  memcpy(number, parser->source + start, length); ChimiValue *raw = json_number_value(number);
  ChimiValue *fields[] = {raw}; return standard_value(CHIMI_CONSTRUCTOR_JSON_NUMBER, fields, 1);
}
static ChimiValue *json_parse_value(JsonParser *parser, uint32_t depth) {
  if (depth > 128) { parser->message = "JSON nesting limit exceeded"; return NULL; }
  json_skip_space(parser); if (parser->offset >= parser->length) { parser->message = "expected JSON value"; return NULL; }
  char value = parser->source[parser->offset];
  if (value == 'n' && parser->length - parser->offset >= 4 && memcmp(parser->source + parser->offset, "null", 4) == 0) {
    parser->offset += 4; return standard_value(CHIMI_CONSTRUCTOR_JSON_NULL, NULL, 0);
  }
  if ((value == 't' && parser->length - parser->offset >= 4 && memcmp(parser->source + parser->offset, "true", 4) == 0) ||
      (value == 'f' && parser->length - parser->offset >= 5 && memcmp(parser->source + parser->offset, "false", 5) == 0)) {
    bool raw = value == 't'; parser->offset += raw ? 4 : 5; ChimiValue *boolean = boolean_value(raw);
    ChimiValue *fields[] = {boolean}; return standard_value(CHIMI_CONSTRUCTOR_JSON_BOOLEAN, fields, 1);
  }
  if (value == '"') { char *text = json_parse_string(parser); if (!text) return NULL;
    ChimiValue *raw = text_value(text); ChimiValue *fields[] = {raw}; return standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1); }
  if (value == '[') return json_parse_array(parser, depth);
  if (value == '{') return json_parse_object(parser, depth);
  if (value == '-' || (value >= '0' && value <= '9')) return json_parse_number(parser);
  parser->message = "expected JSON value"; return NULL;
}

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment);

static ChimiControl *invoke_intrinsic(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment);

static ChimiControl *invoke(ChimiRuntime *runtime, ChimiCallable *callable, ChimiValue **arguments,
                            uint32_t argument_count, uint64_t span, uint32_t call_node,
                            ChimiEnvironment *call_environment) {
  if (callable->kind == CALL_DEPENDENCY) {
    if (!runtime->silent) {
      uint64_t dependency_span = runtime->next_span++;
      emit_dependency(runtime, call_node, dependency_span, span, callable);
    }
    return invoke(runtime, callable->inner, arguments, argument_count, span, call_node, call_environment);
  }
  if (callable->kind == CALL_INTRINSIC)
    return invoke_intrinsic(runtime, callable->intrinsic, arguments, argument_count,
                            span, call_node, call_environment);
  ChimiEnvironment *environment = clone_environment(callable->environment);
  for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
    if (call_environment->requirements[symbol] != NULL)
      environment->requirements[symbol] = call_environment->requirements[symbol];
  uint32_t count = callable->parameters_length < argument_count ? callable->parameters_length : argument_count;
  for (uint32_t index = 0; index < count; ++index)
    environment->locals[CHIMI_IDS[callable->parameters_offset + index]] = arguments[index];
  return run_node(callable->body, runtime, environment, span);
}

static ChimiControl *map_values_node(ChimiRuntime *runtime, MapNode *node,
                                     ChimiCallable *callback, MapNode **output,
                                     uint64_t span, uint32_t call_node,
                                     ChimiEnvironment *environment) {
  if (!node) return success(NULL);
  ChimiControl *control = map_values_node(runtime, node->left, callback, output,
                                         span, call_node, environment);
  if (control->kind != CONTROL_SUCCESS) return control;
  ChimiValue *arguments[] = {node->value};
  control = invoke(runtime, callback, arguments, 1, span, call_node, environment);
  if (control->kind != CONTROL_SUCCESS) return control;
  *output = map_put_node(*output, node->key, control->value);
  return map_values_node(runtime, node->right, callback, output,
                         span, call_node, environment);
}

static ChimiControl *intrinsic_shape_defect(uint32_t call_node) {
  return defect("verified standard function has invalid runtime arguments", call_node);
}

static ChimiControl *invoke_intrinsic(ChimiRuntime *runtime, uint32_t intrinsic,
                                      ChimiValue **arguments, uint32_t argument_count,
                                      uint64_t span, uint32_t call_node,
                                      ChimiEnvironment *environment) {
  switch (intrinsic) {
    case CHIMI_INTRINSIC_MAP_EMPTY:
      return argument_count == 0 ? success(map_value(NULL)) : intrinsic_shape_defect(call_node);
    case CHIMI_INTRINSIC_MAP_GET: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_MAP || arguments[1]->kind != VALUE_TEXT)
        return intrinsic_shape_defect(call_node);
      ChimiValue *found = map_get_node(arguments[0]->as.map, arguments[1]->as.text);
      if (!found) return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0));
      ChimiValue *fields[] = {found};
      return success(standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1));
    }
    case CHIMI_INTRINSIC_MAP_PUT: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_MAP || arguments[1]->kind != VALUE_TEXT)
        return intrinsic_shape_defect(call_node);
      return success(map_value(map_put_node(arguments[0]->as.map, arguments[1]->as.text, arguments[2])));
    }
    case CHIMI_INTRINSIC_MAP_UPDATE: {
      if (argument_count != 3 || arguments[0]->kind != VALUE_MAP || arguments[1]->kind != VALUE_TEXT ||
          arguments[2]->kind != VALUE_CALLABLE) return intrinsic_shape_defect(call_node);
      ChimiValue *found = map_get_node(arguments[0]->as.map, arguments[1]->as.text);
      ChimiValue *current;
      if (found) { ChimiValue *fields[] = {found}; current = standard_value(CHIMI_CONSTRUCTOR_OPTION_SOME, fields, 1); }
      else current = standard_value(CHIMI_CONSTRUCTOR_OPTION_NONE, NULL, 0);
      ChimiValue *callback_arguments[] = {current};
      ChimiControl *control = invoke(runtime, arguments[2]->as.callable, callback_arguments, 1,
                                     span, call_node, environment);
      if (control->kind != CONTROL_SUCCESS) return control;
      return success(map_value(map_put_node(arguments[0]->as.map, arguments[1]->as.text, control->value)));
    }
    case CHIMI_INTRINSIC_MAP_MAP_VALUES: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_MAP || arguments[1]->kind != VALUE_CALLABLE)
        return intrinsic_shape_defect(call_node);
      MapNode *output = NULL;
      ChimiControl *control = map_values_node(runtime, arguments[0]->as.map, arguments[1]->as.callable,
                                              &output, span, call_node, environment);
      return control->kind == CONTROL_SUCCESS ? success(map_value(output)) : control;
    }
    case CHIMI_INTRINSIC_JSON_DECODE: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      JsonParser parser = {arguments[0]->as.text, strlen(arguments[0]->as.text), 0, NULL};
      ChimiValue *value = json_parse_value(&parser, 0); json_skip_space(&parser);
      if (value && parser.offset == parser.length) return success(value);
      if (!parser.message) parser.message = "unexpected trailing JSON input";
      ChimiValue *fields[] = {text_value(parser.message), integer_value((int64_t)parser.offset)};
      return failure(standard_value(CHIMI_CONSTRUCTOR_DECODE_INVALID_JSON, fields, 2));
    }
    case CHIMI_INTRINSIC_JSON_ENCODE: {
      if (argument_count != 1) return intrinsic_shape_defect(call_node);
      TextBuffer buffer = {0};
      if (!encode_json_value(&buffer, arguments[0])) return intrinsic_shape_defect(call_node);
      if (!buffer.data) buffer.data = strdup("");
      return success(text_value(buffer.data));
    }
    case CHIMI_INTRINSIC_JSON_BOOLEAN: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_BOOLEAN) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_BOOLEAN, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_INTEGER) return intrinsic_shape_defect(call_node);
      char *number = allocate(32, 1); snprintf(number, 32, "%" PRId64, arguments[0]->as.integer);
      ChimiValue *raw = json_number_value(number); ChimiValue *fields[] = {raw};
      return success(standard_value(CHIMI_CONSTRUCTOR_JSON_NUMBER, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_TEXT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_TEXT, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_ARRAY: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_LIST) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_ARRAY, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_OBJECT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_MAP) return intrinsic_shape_defect(call_node);
      ChimiValue *fields[] = {arguments[0]}; return success(standard_value(CHIMI_CONSTRUCTOR_JSON_OBJECT, fields, 1));
    }
    case CHIMI_INTRINSIC_JSON_NUMBER_TO_INT: {
      if (argument_count != 1 || arguments[0]->kind != VALUE_JSON_NUMBER) return intrinsic_shape_defect(call_node);
      const char *number = arguments[0]->as.json_number;
      if (strpbrk(number, ".eE")) {
        ChimiValue *fields[] = {arguments[0]};
        return failure(standard_value(CHIMI_CONSTRUCTOR_NUMBER_NOT_INTEGER, fields, 1));
      }
      errno = 0; char *end = NULL; int64_t value = strtoll(number, &end, 10);
      if (errno == ERANGE || !end || *end) {
        ChimiValue *fields[] = {arguments[0]};
        return failure(standard_value(CHIMI_CONSTRUCTOR_NUMBER_OUT_OF_RANGE, fields, 1));
      }
      return success(integer_value(value));
    }
    case CHIMI_INTRINSIC_TEXT_APPEND: {
      if (argument_count != 2 || arguments[0]->kind != VALUE_TEXT ||
          arguments[1]->kind != VALUE_TEXT) return intrinsic_shape_defect(call_node);
      size_t left = strlen(arguments[0]->as.text), right = strlen(arguments[1]->as.text);
      char *output = allocate(left + right + 1, 1);
      memcpy(output, arguments[0]->as.text, left);
      memcpy(output + left, arguments[1]->as.text, right);
      return success(text_value(output));
    }
  }
  return defect("unknown standard intrinsic", call_node);
}

static ChimiControl *eval_reference(ChimiRuntime *runtime, uint32_t node_id,
                                    ChimiEnvironment *environment, uint64_t span) {
  uint32_t symbol = CHIMI_NODES[node_id].a; const ChimiSymbol *shape = &CHIMI_SYMBOLS[symbol];
  switch (shape->kind) {
    case CHIMI_SYMBOL_FUNCTION: {
      const ChimiFunction *function = find_function(symbol);
      ChimiCallable *callable = allocate(1, sizeof(*callable)); callable->kind = CALL_DECLARED;
      callable->symbol = symbol; callable->body = function->body;
      callable->parameters_offset = function->parameters_offset; callable->parameters_length = function->parameters_length;
      callable->environment = clone_environment(environment); callable->name = "function";
      return success(callable_value(callable));
    }
    case CHIMI_SYMBOL_BINDING: {
      if (runtime->active_bindings[symbol]) return defect("module binding is cyclic", -1);
      runtime->active_bindings[symbol] = true;
      ChimiControl *control = run_node(shape->binding, runtime, environment, span);
      runtime->active_bindings[symbol] = false; return control;
    }
    case CHIMI_SYMBOL_REQUIREMENT:
      if (environment->requirements[symbol]) return success(environment->requirements[symbol]);
      return defect("closed core has no provider", -1);
    case CHIMI_SYMBOL_LOCAL:
      if (environment->locals[symbol]) return success(environment->locals[symbol]);
      return defect("verified local is not bound", -1);
    case CHIMI_SYMBOL_INTRINSIC: {
      ChimiCallable *callable = allocate(1, sizeof(*callable)); callable->kind = CALL_INTRINSIC;
      callable->intrinsic = shape->intrinsic; callable->name = CHIMI_STRINGS[shape->name];
      return success(callable_value(callable));
    }
    default: return defect("symbol is not a runtime value", -1);
  }
}

#define NODE_WRAPPER(NAME, OPERATION, ...) \
  ChimiControl *chimi_rt_##NAME(ChimiRuntime *runtime, uint32_t node_id, ChimiEnvironment *incoming, uint64_t parent) { \
    uint64_t span = begin_node(runtime, node_id, parent, OPERATION); ChimiEnvironment *environment = clone_environment(incoming); \
    (void)environment; \
    ChimiControl *result = NULL; do { __VA_ARGS__ } while (0); return finish_node(runtime, node_id, parent, span, OPERATION, result); \
  }
#define RUN(CHILD) run_node((CHILD), runtime, environment, span)
#define REQUIRE_SUCCESS(CONTROL, TARGET) do { ChimiControl *_control = (CONTROL); \
  if (_control->kind != CONTROL_SUCCESS) { result = _control; break; } (TARGET) = _control->value; } while (0)

NODE_WRAPPER(constant, "constant", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value;
  if (node->a == CHIMI_LITERAL_INTEGER) value = integer_value(node->integer);
  else if (node->a == CHIMI_LITERAL_TEXT) value = text_value(CHIMI_STRINGS[node->b]);
  else value = boolean_value(node->integer != 0); result = success(value);
})
NODE_WRAPPER(reference, "reference", { result = eval_reference(runtime, node_id, environment, span); })
NODE_WRAPPER(select, "select", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *control = RUN(node->a);
  if (control->kind != CONTROL_SUCCESS) { result = control; break; }
  ChimiValue *value = select_value(control->value, CHIMI_STRINGS[node->b]);
  result = value ? success(value) : defect("verified value has no field", node_id);
})
NODE_WRAPPER(dependency_method, "dependency_method", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *provider = environment->requirements[node->a];
  if (!provider) { result = defect("closed core has no provider", node_id); break; }
  ChimiValue *method = select_value(provider, CHIMI_STRINGS[node->d]);
  if (!method || method->kind != VALUE_CALLABLE) { result = defect("provider has no callable field", node_id); break; }
  ChimiCallable *callable = allocate(1, sizeof(*callable)); callable->kind = CALL_DEPENDENCY;
  callable->inner = method->as.callable; callable->requirement = node->a; callable->interface_symbol = node->b;
  callable->method = node->c; callable->name = CHIMI_STRINGS[node->d]; result = success(callable_value(callable));
})

NODE_WRAPPER(construct, "construct", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; const ChimiSymbol *symbol = &CHIMI_SYMBOLS[node->a];
  const ChimiConstructor *shape = &CHIMI_CONSTRUCTORS[symbol->constructor];
  ChimiValue **values = allocate(node->length, sizeof(*values));
  bool stopped = false;
  for (uint32_t index = 0; index < node->length; ++index) {
    ChimiControl *control = RUN(CHIMI_NAMED_NODES[node->offset + index].node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    values[index] = control->value;
  }
  if (stopped) break;
  if (shape->outcome_success >= 0) { result = shape->outcome_success ? success(values[0]) : failure(values[0]); break; }
  ChimiValue *value = new_value(VALUE_CONSTRUCTED); value->as.constructed.constructor = node->a;
  value->as.constructed.type_name = CHIMI_STRINGS[shape->type_name];
  value->as.constructed.variant_name = shape->variant_name == UINT32_MAX ? NULL : CHIMI_STRINGS[shape->variant_name];
  value->as.constructed.length = shape->fields_length;
  value->as.constructed.fields = allocate(shape->fields_length, sizeof(*value->as.constructed.fields));
  for (uint32_t field = 0; field < shape->fields_length; ++field) {
    const char *name = CHIMI_STRINGS[CHIMI_IDS[shape->fields_offset + field]]; uint32_t selected = field;
    for (uint32_t argument = 0; argument < node->length; ++argument) {
      uint32_t label = CHIMI_NAMED_NODES[node->offset + argument].name;
      if (label != UINT32_MAX && strcmp(CHIMI_STRINGS[label], name) == 0) { selected = argument; break; }
    }
    value->as.constructed.fields[field] = (RuntimeField){name, values[selected]};
  }
  result = success(value);
})

NODE_WRAPPER(call, "call", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *callee = RUN(node->a);
  if (callee->kind != CONTROL_SUCCESS) { result = callee; break; }
  if (callee->value->kind != VALUE_CALLABLE) { result = defect("verified call target is not callable", node_id); break; }
  ChimiValue **arguments = allocate(node->length, sizeof(*arguments)); bool stopped = false;
  for (uint32_t index = 0; index < node->length; ++index) {
    ChimiControl *control = RUN(CHIMI_NAMED_NODES[node->offset + index].node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    arguments[index] = control->value;
  }
  if (!stopped) result = invoke(runtime, callee->value->as.callable, arguments, node->length, span, node_id, environment);
})

NODE_WRAPPER(list, "list", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value = new_value(VALUE_LIST);
  value->as.list.length = node->length; value->as.list.items = allocate(node->length, sizeof(*value->as.list.items));
  bool stopped = false; for (uint32_t i = 0; i < node->length; ++i) {
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + i]);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    value->as.list.items[i] = control->value;
  } if (!stopped) result = success(value);
})

NODE_WRAPPER(record, "record", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *value = new_value(VALUE_RECORD);
  value->as.record.length = node->length; value->as.record.fields = allocate(node->length, sizeof(*value->as.record.fields));
  bool stopped = false; for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiNamedNode *field = &CHIMI_NAMED_NODES[node->offset + i]; ChimiControl *control = RUN(field->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    value->as.record.fields[i] = (RuntimeField){CHIMI_STRINGS[field->name], control->value};
  } if (!stopped) result = success(value);
})

NODE_WRAPPER(unary, "unary", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *control = RUN(node->a);
  if (control->kind != CONTROL_SUCCESS) { result = control; break; }
  if (control->value->kind != VALUE_INTEGER) { result = defect("verified unary operand is not an integer", node_id); break; }
  if (control->value->as.integer == INT64_MIN) { result = defect("integer negation overflow", node_id); break; }
  result = success(integer_value(-control->value->as.integer));
})

NODE_WRAPPER(binary, "binary", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiControl *left = RUN(node->b);
  if (left->kind != CONTROL_SUCCESS) { result = left; break; }
  ChimiControl *right = RUN(node->c); if (right->kind != CONTROL_SUCCESS) { result = right; break; }
  if (node->a == CHIMI_BINARY_PIPE) {
    if (right->value->kind != VALUE_CALLABLE) { result = defect("verified pipeline target is not callable", node_id); break; }
    ChimiValue *arguments[] = {left->value}; result = invoke(runtime, right->value->as.callable, arguments, 1, span, node_id, environment); break;
  }
  if (left->value->kind != VALUE_INTEGER || right->value->kind != VALUE_INTEGER) {
    result = defect("verified binary operands are not integers", node_id); break;
  }
  int64_t l = left->value->as.integer, r = right->value->as.integer, output = 0; bool overflow = false;
  switch (node->a) {
    case CHIMI_BINARY_ADD: overflow = __builtin_add_overflow(l, r, &output); break;
    case CHIMI_BINARY_SUBTRACT: overflow = __builtin_sub_overflow(l, r, &output); break;
    case CHIMI_BINARY_MULTIPLY: overflow = __builtin_mul_overflow(l, r, &output); break;
    case CHIMI_BINARY_DIVIDE:
      if (r == 0) { result = defect("integer division by zero", node_id); break; }
      if (l == INT64_MIN && r == -1) overflow = true; else output = l / r; break;
    case CHIMI_BINARY_LESS: result = success(boolean_value(l < r)); break;
    case CHIMI_BINARY_LESS_EQUAL: result = success(boolean_value(l <= r)); break;
    case CHIMI_BINARY_GREATER: result = success(boolean_value(l > r)); break;
    case CHIMI_BINARY_GREATER_EQUAL: result = success(boolean_value(l >= r)); break;
  }
  if (result) break;
  result = overflow ? defect("integer arithmetic overflow", node_id) : success(integer_value(output));
})

NODE_WRAPPER(identity, "identity", { result = RUN(CHIMI_NODES[node_id].a); })

NODE_WRAPPER(provision, "provision", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; bool stopped = false;
  for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiNamedNode *provider = &CHIMI_NAMED_NODES[node->offset + i]; ChimiControl *control = RUN(provider->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    const char *name = CHIMI_STRINGS[provider->name];
    for (uint32_t symbol = 0; symbol < CHIMI_SYMBOL_COUNT; ++symbol)
      if (CHIMI_SYMBOLS[symbol].kind == CHIMI_SYMBOL_REQUIREMENT &&
          strcmp(CHIMI_STRINGS[CHIMI_SYMBOLS[symbol].name], name) == 0) environment->requirements[symbol] = control->value;
  } if (!stopped) result = RUN(node->a);
})

NODE_WRAPPER(sequence, "sequence", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiValue *last = NULL; bool stopped = false;
  for (uint32_t i = 0; i < node->length; ++i) {
    const ChimiStep *step = &CHIMI_STEPS[node->offset + i]; ChimiControl *control = RUN(step->node);
    if (control->kind != CONTROL_SUCCESS) { result = control; stopped = true; break; }
    last = control->value; if (step->kind == 0) environment->locals[step->symbol] = last;
  } if (!stopped) result = success(last);
})

NODE_WRAPPER(closure, "closure", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; ChimiCallable *callable = allocate(1, sizeof(*callable));
  callable->kind = CALL_CLOSURE; callable->body = node->a; callable->parameters_offset = node->offset;
  callable->parameters_length = node->length; callable->environment = clone_environment(environment);
  callable->name = "closure"; result = success(callable_value(callable));
})

typedef enum { MATCH_PLAIN, MATCH_SUCCESS, MATCH_FAILURE } MatchKind;
typedef struct { MatchKind kind; ChimiValue *value; } MatchValue;

static bool literal_matches(const ChimiPattern *pattern, const ChimiValue *value) {
  if (pattern->a == CHIMI_LITERAL_INTEGER) return value->kind == VALUE_INTEGER && value->as.integer == pattern->integer;
  if (pattern->a == CHIMI_LITERAL_TEXT) return value->kind == VALUE_TEXT && strcmp(value->as.text, CHIMI_STRINGS[pattern->offset]) == 0;
  return value->kind == VALUE_BOOLEAN && value->as.boolean == (pattern->integer != 0);
}
static bool match_plain(uint32_t pattern_id, ChimiValue *value, ChimiEnvironment *environment) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  switch (pattern->kind) {
    case CHIMI_PATTERN_WILDCARD: return true;
    case CHIMI_PATTERN_BINDING: environment->locals[pattern->a] = value; return true;
    case CHIMI_PATTERN_LITERAL: return literal_matches(pattern, value);
    case CHIMI_PATTERN_CONSTRUCTOR:
      if (value->kind != VALUE_CONSTRUCTED || value->as.constructed.constructor != pattern->a ||
          value->as.constructed.length != pattern->length) return false;
      for (uint32_t i = 0; i < pattern->length; ++i)
        if (!match_plain(CHIMI_IDS[pattern->offset + i], value->as.constructed.fields[i].value, environment)) return false;
      return true;
    case CHIMI_PATTERN_OUTCOME: return false;
  }
  return false;
}
static bool match_value(uint32_t pattern_id, MatchValue value, ChimiEnvironment *environment) {
  const ChimiPattern *pattern = &CHIMI_PATTERNS[pattern_id];
  if (pattern->kind == CHIMI_PATTERN_OUTCOME) {
    MatchKind expected = pattern->a ? MATCH_SUCCESS : MATCH_FAILURE;
    return value.kind == expected && match_plain(pattern->offset, value.value, environment);
  }
  return value.kind == MATCH_PLAIN && match_plain(pattern_id, value.value, environment);
}

NODE_WRAPPER(match, "match", {
  const ChimiNode *node = &CHIMI_NODES[node_id]; MatchValue *values = allocate(node->length, sizeof(*values)); bool stopped = false;
  for (uint32_t subject = 0; subject < node->length; ++subject) {
    bool contextual = false;
    for (uint32_t arm_index = 0; arm_index < node->b; ++arm_index) {
      const ChimiArm *arm = &CHIMI_ARMS[node->a + arm_index];
      if (CHIMI_PATTERNS[CHIMI_IDS[arm->patterns_offset + subject]].kind == CHIMI_PATTERN_OUTCOME) contextual = true;
    }
    ChimiControl *control = RUN(CHIMI_IDS[node->offset + subject]);
    if (contextual && control->kind == CONTROL_SUCCESS) values[subject] = (MatchValue){MATCH_SUCCESS, control->value};
    else if (contextual && control->kind == CONTROL_FAILURE) values[subject] = (MatchValue){MATCH_FAILURE, control->value};
    else if (!contextual && control->kind == CONTROL_SUCCESS) values[subject] = (MatchValue){MATCH_PLAIN, control->value};
    else { result = control; stopped = true; break; }
  }
  if (stopped) break;
  for (uint32_t arm_index = 0; arm_index < node->b; ++arm_index) {
    const ChimiArm *arm = &CHIMI_ARMS[node->a + arm_index]; ChimiEnvironment *arm_environment = clone_environment(environment); bool matched = true;
    for (uint32_t subject = 0; subject < node->length; ++subject)
      if (!match_value(CHIMI_IDS[arm->patterns_offset + subject], values[subject], arm_environment)) { matched = false; break; }
    if (!matched) continue;
    if (arm->guard != UINT32_MAX) {
      ChimiControl *guard = run_node(arm->guard, runtime, arm_environment, span);
      if (guard->kind != CONTROL_SUCCESS) { result = guard; stopped = true; break; }
      if (guard->value->kind != VALUE_BOOLEAN) { result = defect("verified case guard is not Boolean", node_id); stopped = true; break; }
      if (!guard->value->as.boolean) continue;
    }
    result = run_node(arm->body, runtime, arm_environment, span); stopped = true; break;
  }
  if (!stopped) {
    for (uint32_t i = 0; i < node->length; ++i) if (values[i].kind == MATCH_FAILURE) { result = failure(values[i].value); stopped = true; break; }
    if (!stopped) result = defect("verified exhaustive match had no matching arm", node_id);
  }
})

static void print_outcome(const ChimiControl *control) {
  switch (control->kind) {
    case CONTROL_SUCCESS: fputs("{\"outcome\":\"success\",\"value\":", stdout); json_value(control->value); break;
    case CONTROL_FAILURE: fputs("{\"outcome\":\"expected_failure\",\"error\":", stdout); json_value(control->value); break;
    case CONTROL_DEFECT:
      fputs("{\"outcome\":\"defect\",\"message\":", stdout); json_string(control->message);
      if (control->node < 0) fputs(",\"core_node_id\":null,\"origin_id\":null", stdout);
      else printf(",\"core_node_id\":%" PRId64 ",\"origin_id\":%u", control->node, CHIMI_NODES[control->node].origin);
      break;
    case CONTROL_CANCEL:
      fputs("{\"outcome\":\"cancellation\",\"reason\":\"requested before the next semantic operation\"", stdout); break;
  }
  fputs("}\n", stdout);
}

int32_t chimi_rt_main(ChimiNodeFn entry, int32_t argc, const char **argv) {
  struct timespec identity_time;
  clock_gettime(CLOCK_REALTIME, &identity_time);
  char generated_execution_id[96];
  snprintf(generated_execution_id, sizeof(generated_execution_id),
           "execution-%ld-%" PRId64 "-%ld", (long)getpid(),
           (int64_t)identity_time.tv_sec, identity_time.tv_nsec);
  ChimiRuntime runtime = {.execution_id = generated_execution_id, .next_span = 2};
  uint32_t benchmark_repeats = 0;
  runtime.active_bindings = allocate(CHIMI_SYMBOL_COUNT, sizeof(*runtime.active_bindings));
  for (int32_t index = 1; index < argc; ++index) {
    if (strcmp(argv[index], "--trace=full") == 0) { runtime.full_trace = true; runtime.silent = false; }
    else if (strcmp(argv[index], "--trace=boundaries") == 0) { runtime.full_trace = false; runtime.silent = false; }
    else if (strcmp(argv[index], "--trace=off") == 0) { runtime.full_trace = false; runtime.silent = true; }
    else if (strncmp(argv[index], "--execution-id=", 15) == 0) runtime.execution_id = argv[index] + 15;
    else if (strcmp(argv[index], "--cancel") == 0) runtime.cancel = true;
    else if (strncmp(argv[index], "--benchmark-repeats=", 20) == 0)
      benchmark_repeats = (uint32_t)strtoul(argv[index] + 20, NULL, 10);
    else { fprintf(stderr, "unknown Chimi runtime argument: %s\n", argv[index]); return 64; }
  }
  uint32_t entry_node = 0;
  for (uint32_t i = 0; i < CHIMI_FUNCTION_COUNT; ++i)
    if (CHIMI_NODE_FUNCTIONS[CHIMI_FUNCTIONS[i].body] == entry) { entry_node = CHIMI_FUNCTIONS[i].body; break; }
  if (benchmark_repeats > 0) {
    struct timespec started, finished;
    clock_gettime(CLOCK_MONOTONIC, &started);
    for (uint32_t repeat = 0; repeat < benchmark_repeats; ++repeat) {
      runtime.next_sequence = 0;
      runtime.next_span = 2;
      if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
      ChimiControl *control = runtime.cancel ? cancelled() : entry(&runtime, new_environment(), 1);
      if (control == NULL) return 70;
      if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
    }
    fflush(stdout);
    clock_gettime(CLOCK_MONOTONIC, &finished);
    int64_t elapsed = (int64_t)(finished.tv_sec - started.tv_sec) * INT64_C(1000000000)
                    + (int64_t)finished.tv_nsec - (int64_t)started.tv_nsec;
    fprintf(stderr, "%" PRIu64 "\n", elapsed / benchmark_repeats);
    return 0;
  }
  if (!runtime.silent) emit_event(&runtime, "computation_started", entry_node, 1, -1, "computation", NULL);
  ChimiControl *control = runtime.cancel ? cancelled() : entry(&runtime, new_environment(), 1);
  if (!runtime.silent) emit_event(&runtime, "computation_completed", entry_node, 1, -1, "computation", control);
  print_outcome(control);
  return 0;
}
